//! Telling a PR's author something on Microsoft Teams — "approved", "three
//! comments waiting" — without waiting on an Entra app registration.
//!
//! One sender, two routes, whoever asks (the header button or the
//! assistant's confirmed tool call):
//!
//! - **Webhook**: a Power Automate flow the user builds once in Teams'
//!   Workflows app — "When a Teams webhook request is received" → "Create a
//!   chat" (user + author) → "Post message in a chat or channel" as the
//!   user. Cora POSTs `{to, text, …}`; the message lands in the 1:1 chat as
//!   if typed. The trigger URL is the whole credential and lives in the
//!   Keychain.
//! - **Deep link**: with no webhook, Teams' documented chat link opens the
//!   1:1 with the text in the compose box. One Enter to send — honest about
//!   what it is, and it needs nothing from anyone.
//!
//! Either way the message is recorded in the user's action history, so the
//! PR's History tab reads "messaged the author on Teams" beside the review
//! it followed.

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::github::query::GraphQlClient;
use crate::models::{events, TeamsOutcome, TeamsRecipient};
use crate::secrets;

/// Teams' "start a chat" deep link: the 1:1 with `users`, `message`
/// pre-filled. Documented under "Deep link to start a new chat". The
/// `msteams:` form goes straight to the desktop client; the https one
/// takes a detour through the browser but works with no client installed.
const CHAT_DEEP_LINK_APP: &str = "msteams://teams.microsoft.com/l/chat/0/0";
const CHAT_DEEP_LINK_WEB: &str = "https://teams.microsoft.com/l/chat/0/0";
/// Deep links ride in a URL; keep well under what launchers truncate.
const MAX_DEEP_LINK_TEXT_CHARS: usize = 1500;
const WEBHOOK_TIMEOUT_SECS: u64 = 20;
/// Audit action for a message to the author — the History tab's row.
pub const AUDIT_ACTION: &str = "teams-messaged";

/// Who the author is on Teams. Settings first (it's also the override for a
/// wrong guess), then GitHub's public profile email, then the address they
/// author their commits with — corporate SSO orgs almost always sign
/// commits with the work address, which is the Teams one.
pub async fn resolve_recipient(app: &AppHandle, pr_id: &str) -> AppResult<TeamsRecipient> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let login = pr.info.author.clone();
    let settings = store.settings()?;
    if let Some(email) = settings.author_emails.get(&login).map(|e| e.trim()).filter(|e| !e.is_empty()) {
        return Ok(TeamsRecipient { login, email: email.to_string(), source: "settings".into() });
    }

    let (owner, name) = pr.info.repo.split_once('/').unwrap();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?
        .with_health(GraphQlClient::shared_health(app));
    let data = client
        .run(
            "query($login: String!, $owner: String!, $name: String!, $number: Int!) {
              user(login: $login) { email }
              repository(owner: $owner, name: $name) {
                pullRequest(number: $number) {
                  commits(last: 30) { nodes { commit { author { email user { login } } } } }
                }
              }
            }",
            &json!({ "login": login, "owner": owner, "name": name, "number": pr.info.number }),
        )
        .await?;

    if let Some(email) = data.pointer("/user/email").and_then(Value::as_str).filter(|e| usable_email(e)) {
        return Ok(TeamsRecipient { login, email: email.to_string(), source: "profile".into() });
    }
    let commit_emails = data
        .pointer("/repository/pullRequest/commits/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter(|n| {
                    n.pointer("/commit/author/user/login").and_then(Value::as_str) == Some(login.as_str())
                })
                .filter_map(|n| n.pointer("/commit/author/email").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match commonest_email(&commit_emails) {
        Some(email) => Ok(TeamsRecipient { login, email, source: "commits".into() }),
        None => Err(AppError::Other(format!(
            "No Teams address for @{login} — GitHub shows no email and their commits don't carry one. Set it under Settings → Users."
        ))),
    }
}

/// Real, deliverable addresses only: GitHub's noreply aliases would bounce.
fn usable_email(email: &str) -> bool {
    let e = email.trim();
    e.contains('@') && !e.ends_with("noreply.github.com") && !e.starts_with("noreply@")
}

/// The address the author signs most of their commits with — a stray
/// personal-laptop commit shouldn't outvote the work address.
fn commonest_email(emails: &[&str]) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for e in emails.iter().map(|e| e.trim().to_lowercase()).filter(|e| usable_email(e)) {
        match counts.iter_mut().find(|(k, _)| *k == e) {
            Some((_, n)) => *n += 1,
            None => counts.push((e, 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(e, _)| e)
}

/// Send `text` to the PR's author: through the webhook when one is
/// configured, else by opening Teams with it drafted. Records the action
/// either way; `via` marks the assistant's.
pub async fn message_author(
    app: &AppHandle,
    pr_id: &str,
    text: &str,
    via: Option<&str>,
) -> AppResult<TeamsOutcome> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::Other("the message is empty".into()));
    }
    let recipient = resolve_recipient(app, pr_id).await?;
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;

    let delivery = match secrets::teams_webhook()? {
        Some(url) => {
            post_webhook(
                &url,
                &json!({
                    "to": recipient.email,
                    "text": text,
                    "pr": {
                        "repo": pr.info.repo,
                        "number": pr.info.number,
                        "title": pr.info.title,
                        "url": pr.info.url,
                        "author": pr.info.author,
                    }
                }),
            )
            .await?;
            "sent"
        }
        None => {
            open_deep_link(&recipient.email, text)?;
            "drafted"
        }
    };

    // The History tab's row: "sent → @login: text" or "drafted → @login: text".
    let detail = format!("{delivery} → @{}: {text}", recipient.login);
    crate::commands::audit_pr_action(&store, AUDIT_ACTION, pr_id, &detail, via);
    let _ = app.emit(events::REVIEWS_CHANGED, ());

    Ok(TeamsOutcome { delivery: delivery.into(), recipient, text: text.to_string() })
}

/// A test message to an address of the user's choosing — proves the flow
/// is wired before anything real rides on it. Not audited: no PR involved.
pub async fn send_test(email: &str, text: &str) -> AppResult<()> {
    let url = secrets::teams_webhook()?
        .ok_or_else(|| AppError::Other("no Teams webhook configured".into()))?;
    post_webhook(&url, &json!({ "to": email.trim(), "text": text, "pr": Value::Null })).await
}

async fn post_webhook(url: &str, payload: &Value) -> AppResult<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
        .build()?;
    let resp = client.post(url).json(payload).send().await?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let body = body.chars().take(300).collect::<String>();
    Err(AppError::Other(format!("Teams webhook answered {status}: {body}")))
}

/// The desktop client first; with no `msteams:` handler registered the OS
/// refuses, and the web link lands on the same chat.
fn open_deep_link(email: &str, text: &str) -> AppResult<()> {
    let text: String = text.chars().take(MAX_DEEP_LINK_TEXT_CHARS).collect();
    if tauri_plugin_opener::open_url(chat_deep_link(CHAT_DEEP_LINK_APP, email, &text), None::<&str>)
        .is_ok()
    {
        return Ok(());
    }
    tauri_plugin_opener::open_url(chat_deep_link(CHAT_DEEP_LINK_WEB, email, &text), None::<&str>)
        .map_err(|e| AppError::Other(format!("couldn't open Teams: {e}")))
}

fn chat_deep_link(base: &str, email: &str, text: &str) -> String {
    format!("{base}?users={}&message={}", percent_encode(email), percent_encode(text))
}

/// RFC 3986 query-component encoding — unreserved bytes pass, everything
/// else (including the `&`, `=` and `#` that would end the parameter, and
/// the newlines Teams keeps as line breaks) is escaped.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Only https, and only something that looks like a trigger endpoint —
/// pasting a Teams *channel* link here by mistake should fail at save time,
/// not at the first real message.
pub fn validate_webhook_url(url: &str) -> AppResult<String> {
    let url = url.trim();
    if !url.starts_with("https://") {
        return Err(AppError::Other("the webhook URL must start with https://".into()));
    }
    if !url.contains("/triggers/") && !url.contains("/workflows/") {
        return Err(AppError::Other(
            "that doesn't look like a Power Automate trigger URL (expected …/workflows/…/triggers/…)".into(),
        ));
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_address_outvotes_a_stray_personal_one() {
        let emails = ["Adam@Corp.com", "adam@corp.com", "adam@gmail.com"];
        assert_eq!(commonest_email(&emails).as_deref(), Some("adam@corp.com"));
    }

    #[test]
    fn noreply_addresses_never_qualify() {
        assert_eq!(commonest_email(&["12345+adam@users.noreply.github.com"]), None);
        assert!(!usable_email("noreply@github.com"));
        assert!(usable_email("adam@corp.com"));
        assert_eq!(commonest_email(&[]), None);
    }

    #[test]
    fn the_deep_link_escapes_what_would_end_the_parameter() {
        let url = chat_deep_link(
            CHAT_DEEP_LINK_APP,
            "adam@corp.com",
            "Approved #309 — see a.py & b.py\nhttps://x/y?z=1",
        );
        assert!(url.starts_with("msteams://teams.microsoft.com/l/chat/0/0?users=adam%40corp.com&message="));
        let msg = url.split("&message=").nth(1).unwrap();
        assert!(!msg.contains('&') && !msg.contains('=') && !msg.contains('#') && !msg.contains('\n'));
        assert!(msg.contains("%0A"), "newlines survive as line breaks: {msg}");
    }

    #[test]
    fn webhook_urls_are_checked_at_save_time() {
        assert!(validate_webhook_url("http://prod-1.logic.azure.com/workflows/x/triggers/manual/paths/invoke").is_err());
        assert!(validate_webhook_url("https://teams.microsoft.com/l/channel/abc").is_err());
        assert!(validate_webhook_url(
            "  https://prod-1.westus.logic.azure.com:443/workflows/abc/triggers/manual/paths/invoke?sig=xyz "
        )
        .is_ok());
    }
}

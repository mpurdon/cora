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
//!   if typed. The trigger URL lives in the Keychain. A tenant that has
//!   removed the trigger's "Anyone" option makes it demand a signed-in
//!   tenant identity on every call; that comes from the Azure CLI, the same
//!   way Microsoft's own SDKs' `AzureCliCredential` gets it — no app
//!   registration involved.
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
/// The audience a tenant-restricted Power Automate trigger validates.
const FLOW_RESOURCE: &str = "https://service.flow.microsoft.com/";
/// Where Homebrew and the official installer put `az`; a GUI app's PATH
/// on macOS has neither, so the bare name is tried last.
const AZ_CANDIDATES: &[&str] = &["/opt/homebrew/bin/az", "/usr/local/bin/az", "az"];
/// Audit action for a message to the author — the History tab's row.
pub const AUDIT_ACTION: &str = "teams-messaged";

/// Who the author is on Teams. Settings first (it's also the override for
/// a wrong guess); then what GitHub knows — the profile's public email and
/// the addresses they author commits with, on this PR and on the repo's
/// main branch — ranked by `pick_recipient`, which is where the work-domain
/// rules live.
pub async fn resolve_recipient(app: &AppHandle, pr_id: &str) -> AppResult<TeamsRecipient> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let login = pr.info.author.clone();
    let settings = store.settings()?;
    if let Some(email) =
        settings.author_emails.get(&login).map(|e| e.trim()).filter(|e| !e.is_empty())
    {
        return Ok(TeamsRecipient { login, email: email.to_string(), source: "settings".into() });
    }

    let (owner, name) = pr.info.repo.split_once('/').unwrap();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?
        .with_health(GraphQlClient::shared_health(app));
    // The main-branch history serves twice: the author's commits beyond this
    // PR, and everyone else's (name, address) pairs, from which the org's
    // local-part convention is learned for the guess.
    let data = client
        .run(
            "query($login: String!, $owner: String!, $name: String!, $number: Int!) {
              user(login: $login) { name }
              repository(owner: $owner, name: $name) {
                pullRequest(number: $number) {
                  commits(last: 30) { nodes { commit { author { email user { login } } } } }
                }
                defaultBranchRef { target { ... on Commit {
                  history(first: 100) { nodes { author { email user { login name } } } }
                } } }
              }
            }",
            &json!({ "login": login, "owner": owner, "name": name, "number": pr.info.number }),
        )
        .await?;

    let authors = |ptr: &str| -> Vec<CommitAuthor> {
        data.pointer(ptr)
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| {
                        let a = n.pointer("/commit/author").or_else(|| n.pointer("/author"))?;
                        Some(CommitAuthor {
                            login: a.pointer("/user/login").and_then(Value::as_str)?.to_string(),
                            name: a
                                .pointer("/user/name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            email: a.get("email").and_then(Value::as_str)?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut commits = authors("/repository/pullRequest/commits/nodes");
    commits.extend(authors("/repository/defaultBranchRef/target/history/nodes"));

    // The profile email is the one field behind a PAT scope (read:user /
    // user:email) most tokens lack, and GitHub fails the whole query over
    // it — so it's asked for separately and treated as a bonus. Missing
    // scope, private email, no such field: all the same "nothing there".
    let profile_email = client
        .run("query($login: String!) { user(login: $login) { email } }", &json!({ "login": login }))
        .await
        .ok()
        .and_then(|d| d.pointer("/user/email").and_then(Value::as_str).map(String::from))
        .unwrap_or_default();

    let known = GitHubKnows {
        login: login.clone(),
        name: data.pointer("/user/name").and_then(Value::as_str).unwrap_or_default().to_string(),
        profile_email,
        commits,
    };
    pick_recipient(&known, &settings.teams_email_domains).ok_or_else(|| {
        AppError::Other(format!(
            "No Teams address for @{login} — GitHub shows no usable email and their commits don't carry one. Set it under Settings → Users."
        ))
    })
}

/// One commit's author as GitHub reports it: the account it's linked to,
/// that account's display name, and the address on the commit.
#[derive(Debug, Clone)]
struct CommitAuthor {
    login: String,
    name: String,
    email: String,
}

/// Everything GitHub could tell us about the author, gathered so the
/// ranking is a pure function of it.
#[derive(Debug, Clone, Default)]
struct GitHubKnows {
    login: String,
    name: String,
    profile_email: String,
    /// This PR's commits and the repo's recent main-branch history — the
    /// author's own and everyone else's.
    commits: Vec<CommitAuthor>,
}

/// The ranking. With work domains configured: the profile email if it's
/// on one, else the on-domain address they sign most commits with, else a
/// guess from their display name in the org's convention, else — flagged
/// — whatever off-domain address exists. Without domains: profile, then
/// commits, no guessing (there's no domain to guess on).
fn pick_recipient(known: &GitHubKnows, domains: &[String]) -> Option<TeamsRecipient> {
    let domains: Vec<String> =
        domains.iter().map(|d| d.trim().trim_start_matches('@').to_lowercase()).filter(|d| !d.is_empty()).collect();
    let on_domain = |email: &str| -> bool {
        domains.is_empty() || email.rsplit('@').next().is_some_and(|d| domains.contains(&d.to_lowercase()))
    };
    let recipient = |email: String, source: &str| {
        Some(TeamsRecipient { login: known.login.clone(), email, source: source.into() })
    };

    let profile = known.profile_email.trim().to_lowercase();
    if usable_email(&profile) && on_domain(&profile) {
        return recipient(profile, "profile");
    }
    let mine: Vec<&str> = known
        .commits
        .iter()
        .filter(|c| c.login == known.login)
        .map(|c| c.email.as_str())
        .collect();
    let work: Vec<&str> = mine.iter().copied().filter(|e| on_domain(&e.to_lowercase())).collect();
    if let Some(email) = commonest_email(&work) {
        return recipient(email, "commits");
    }
    if let Some(primary) = domains.first() {
        let convention = learn_convention(&known.commits, &domains).unwrap_or(Convention::FirstDotLast);
        if let Some(local) = convention.local_part(&known.name) {
            return recipient(format!("{local}@{primary}"), "guessed");
        }
    }
    // Off-domain, and only because nothing better exists: the header says
    // so, and the send still sits behind the user's confirmation.
    if usable_email(&profile) {
        return recipient(profile, "personal");
    }
    commonest_email(&mine).and_then(|email| recipient(email, "personal"))
}

/// How an org forms the local part of a work address from a person's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Convention {
    FirstDotLast,
    FirstLast,
    FLast,
    FirstL,
    First,
}

impl Convention {
    const ALL: [Convention; 5] = [
        Convention::FirstDotLast,
        Convention::FirstLast,
        Convention::FLast,
        Convention::FirstL,
        Convention::First,
    ];

    /// The local part this convention gives a display name — None when the
    /// name can't supply the parts ("mp", a handle, a single word).
    fn local_part(self, name: &str) -> Option<String> {
        let parts: Vec<String> = name
            .to_lowercase()
            .split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>())
            .filter(|w| !w.is_empty())
            .collect();
        let (first, last) = match parts.as_slice() {
            [] => return None,
            [only] => (only.as_str(), ""),
            [first, .., last] => (first.as_str(), last.as_str()),
        };
        if last.is_empty() && self != Convention::First {
            return None;
        }
        Some(match self {
            Convention::FirstDotLast => format!("{first}.{last}"),
            Convention::FirstLast => format!("{first}{last}"),
            Convention::FLast => format!("{}{last}", &first[..1]),
            Convention::FirstL => format!("{first}{}", &last[..1]),
            Convention::First => first.to_string(),
        })
    }
}

/// The convention most on-domain (name, address) pairs in the history
/// follow — one vote per person, and only when at least two people agree,
/// since one pair matches several conventions by coincidence.
fn learn_convention(commits: &[CommitAuthor], domains: &[String]) -> Option<Convention> {
    let mut seen: Vec<&str> = Vec::new();
    let mut votes = [0usize; 5];
    for c in commits {
        let email = c.email.trim().to_lowercase();
        let Some((local, domain)) = email.rsplit_once('@') else { continue };
        if !domains.contains(&domain.to_string()) || seen.contains(&c.login.as_str()) {
            continue;
        }
        seen.push(&c.login);
        for (i, conv) in Convention::ALL.iter().enumerate() {
            if conv.local_part(&c.name).as_deref() == Some(local) {
                votes[i] += 1;
            }
        }
    }
    let (i, best) = votes.iter().enumerate().max_by_key(|(_, n)| **n)?;
    (*best >= 2).then_some(Convention::ALL[i])
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
            let token = flow_token(&settings_tenant(app)?).await?;
            post_webhook(
                &url,
                token.as_deref(),
                &json!({
                    "to": recipient.email,
                    "text": text,
                    "html": message_html(text, &pr.info.repo, pr.info.number, &pr.info.url),
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
pub async fn send_test(app: &AppHandle, email: &str, text: &str) -> AppResult<()> {
    let url = secrets::teams_webhook()?
        .ok_or_else(|| AppError::Other("no Teams webhook configured".into()))?;
    let token = flow_token(&settings_tenant(app)?).await?;
    let payload = json!({
        "to": email.trim(),
        "text": text,
        "html": message_html(text, "", 0, ""),
        "pr": Value::Null,
    });
    post_webhook(&url, token.as_deref(), &payload).await
}

fn settings_tenant(app: &AppHandle) -> AppResult<String> {
    Ok(app.state::<crate::orgs::Orgs>().active().settings()?.teams_tenant)
}

async fn post_webhook(url: &str, bearer: Option<&str>, payload: &Value) -> AppResult<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
        .build()?;
    let mut req = client.post(url).json(payload);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let body = body.chars().take(300).collect::<String>();
    let hint = match status.as_u16() {
        401 | 403 if bearer.is_none() => {
            " — the trigger wants a signed-in tenant user; install the Azure CLI and run `az login --tenant <your tenant>`"
        }
        401 | 403 => " — the token was refused; check Settings → Teams → Microsoft tenant matches the flow's tenant, and `az login` there",
        _ => "",
    };
    Err(AppError::Other(format!("Teams webhook answered {status}: {body}{hint}")))
}

/// A bearer token for the Flow service from the signed-in Azure CLI, for
/// the tenant the user named. None when there is no CLI at all (a tenant
/// that still allows "Anyone" needs no token). A CLI that is present but
/// not signed in to that tenant is an error with the fix in it — the
/// alternative, sending unsigned and reading a 401, tells the user less.
async fn flow_token(tenant: &str) -> AppResult<Option<String>> {
    let tenant = tenant.trim().to_string();
    let tenant_arg = tenant.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        let tenant = tenant_arg;
        for az in AZ_CANDIDATES {
            let mut cmd = std::process::Command::new(az);
            cmd.args(["account", "get-access-token", "--resource", FLOW_RESOURCE, "-o", "json"]);
            if !tenant.is_empty() {
                cmd.args(["--tenant", &tenant]);
            }
            match cmd.output() {
                Ok(out) => return Some(out),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Some(std::process::Output {
                    status: std::process::ExitStatus::default(),
                    stdout: Vec::new(),
                    stderr: e.to_string().into_bytes(),
                }),
            }
        }
        None
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?;
    let Some(output) = output else { return Ok(None) };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim().lines().last().unwrap_or("").chars().take(300).collect::<String>();
        let login = if tenant.is_empty() { "az login".to_string() } else { format!("az login --tenant {tenant}") };
        return Err(AppError::Other(format!(
            "Azure CLI couldn't issue a token for the Teams flow ({stderr}). Run `{login}` and try again."
        )));
    }
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| AppError::Other(format!("unexpected Azure CLI output: {e}")))?;
    parsed
        .get("accessToken")
        .and_then(Value::as_str)
        .map(|t| Some(t.to_string()))
        .ok_or_else(|| AppError::Other("Azure CLI returned no access token".into()))
}

/// The message as the flow's HTML field wants it: escaped, line breaks
/// kept, the PR's own reference ("widgets#42", "acme/widgets#42") a link
/// to the PR, and bare URLs clickable. The plain `text` stays the source of
/// truth; this is a rendering of it. A line that is only the PR's URL is
/// dropped once the ref carries the link: the plain text needs it (a
/// compose box can't link a word), the HTML doesn't.
fn message_html(text: &str, repo: &str, number: i64, url: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut out = String::with_capacity(escaped.len() + 64);
    let refs: Vec<String> = if number > 0 && !url.is_empty() {
        let short = repo.rsplit('/').next().unwrap_or(repo);
        vec![format!("{repo}#{number}"), format!("{short}#{number}")]
    } else {
        Vec::new()
    };
    let is_ref = |token: &str| refs.iter().any(|r| r == token);
    let ref_linked = escaped
        .split_whitespace()
        .any(|t| is_ref(t.trim_end_matches(['.', ',', ';', ':', ')', '!', '?'])));
    let redundant_url = |line: &str| ref_linked && line.trim() == url_attr(url);
    // One pass over whitespace-separated tokens: a token is the ref (with
    // trailing punctuation allowed), a URL, or plain text.
    for line in escaped.split('\n').filter(|l| !redundant_url(l)) {
        if !out.is_empty() {
            out.push_str("<br>");
        }
        let mut first = true;
        for token in line.split(' ') {
            if !first {
                out.push(' ');
            }
            first = false;
            let trimmed = token.trim_end_matches(['.', ',', ';', ':', ')', '!', '?']);
            let tail = &token[trimmed.len()..];
            if is_ref(trimmed) {
                out.push_str(&format!("<a href=\"{}\">{trimmed}</a>{tail}", url_attr(url)));
            } else if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
                out.push_str(&format!("<a href=\"{trimmed}\">{trimmed}</a>{tail}"));
            } else {
                out.push_str(token);
            }
        }
    }
    out
}

fn url_attr(url: &str) -> String {
    url.replace('&', "&amp;").replace('"', "&quot;")
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

    fn author(login: &str, name: &str, email: &str) -> CommitAuthor {
        CommitAuthor { login: login.into(), name: name.into(), email: email.into() }
    }

    fn known(profile: &str, commits: Vec<CommitAuthor>) -> GitHubKnows {
        GitHubKnows {
            login: "adamh".into(),
            name: "Adam Hare".into(),
            profile_email: profile.into(),
            commits,
        }
    }

    fn domains() -> Vec<String> {
        vec!["corp.com".into(), "corp-services.com".into()]
    }

    #[test]
    fn with_domains_set_the_gmail_on_the_profile_loses_to_the_work_commit_address() {
        let k = known("adam@gmail.com", vec![author("adamh", "Adam Hare", "Adam.Hare@Corp.com")]);
        let r = pick_recipient(&k, &domains()).unwrap();
        assert_eq!((r.email.as_str(), r.source.as_str()), ("adam.hare@corp.com", "commits"));
        // Without domains the profile email is taken at its word.
        let r = pick_recipient(&k, &[]).unwrap();
        assert_eq!((r.email.as_str(), r.source.as_str()), ("adam@gmail.com", "profile"));
    }

    #[test]
    fn a_work_profile_email_wins_outright() {
        let k = known("ahare@corp-services.com", vec![author("adamh", "Adam Hare", "adam.hare@corp.com")]);
        let r = pick_recipient(&k, &domains()).unwrap();
        assert_eq!((r.email.as_str(), r.source.as_str()), ("ahare@corp-services.com", "profile"));
    }

    #[test]
    fn nothing_on_domain_means_a_guess_in_the_orgs_convention_on_the_first_domain() {
        // Two colleagues establish first.last; the author only ever used noreply.
        let k = known(
            "",
            vec![
                author("adamh", "Adam Hare", "1+adamh@users.noreply.github.com"),
                author("jo", "Jo Bloggs", "jo.bloggs@corp.com"),
                author("sam", "Sam Lee", "sam.lee@corp-services.com"),
            ],
        );
        let r = pick_recipient(&k, &domains()).unwrap();
        assert_eq!((r.email.as_str(), r.source.as_str()), ("adam.hare@corp.com", "guessed"));

        // The convention is learned, not assumed: flast colleagues → flast guess.
        let k = known(
            "",
            vec![author("jo", "Jo Bloggs", "jbloggs@corp.com"), author("sam", "Sam Lee", "slee@corp.com")],
        );
        assert_eq!(pick_recipient(&k, &domains()).unwrap().email, "ahare@corp.com");
    }

    #[test]
    fn a_guess_needs_a_two_word_name_and_a_domain() {
        let mut k = known("", vec![]);
        k.name = "mp".into();
        assert!(pick_recipient(&k, &domains()).is_none());
        k.name = "Adam Hare".into();
        assert!(pick_recipient(&k, &[]).is_none(), "no domain to guess on");
        assert_eq!(pick_recipient(&k, &domains()).unwrap().source, "guessed");
    }

    #[test]
    fn an_off_domain_address_is_the_flagged_last_resort() {
        let mut k = known("adam@gmail.com", vec![]);
        k.name = "mp".into(); // no guess possible
        let r = pick_recipient(&k, &domains()).unwrap();
        assert_eq!((r.email.as_str(), r.source.as_str()), ("adam@gmail.com", "personal"));
    }

    #[test]
    fn one_matching_pair_is_not_a_convention() {
        // "Jo Bloggs" → "jobloggs" and "jbloggs" can't both be right; one
        // person can't settle it.
        let commits = vec![author("jo", "Jo Bloggs", "jobloggs@corp.com")];
        assert_eq!(learn_convention(&commits, &domains()), None);
        let commits = vec![
            author("jo", "Jo Bloggs", "jobloggs@corp.com"),
            author("jo", "Jo Bloggs", "jobloggs@corp.com"), // same person twice: one vote
            author("sam", "Sam Lee", "samlee@corp.com"),
        ];
        assert_eq!(learn_convention(&commits, &domains()), Some(Convention::FirstLast));
    }

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
    fn the_html_copy_links_the_ref_and_bare_urls_and_keeps_line_breaks() {
        let html = message_html(
            "Approved widgets#42. See a.py & b.py.\nhttps://github.com/acme/widgets/pull/42?x=1",
            "acme/widgets",
            42,
            "https://github.com/acme/widgets/pull/42",
        );
        assert_eq!(
            html,
            "Approved <a href=\"https://github.com/acme/widgets/pull/42\">widgets#42</a>. See a.py &amp; b.py.<br>\
             <a href=\"https://github.com/acme/widgets/pull/42?x=1\">https://github.com/acme/widgets/pull/42?x=1</a>"
        );
        // The seed's own trailing URL line is redundant once the ref links.
        let html = message_html(
            "Approved widgets#42. Nothing blocking from me.\nhttps://github.com/acme/widgets/pull/42",
            "acme/widgets",
            42,
            "https://github.com/acme/widgets/pull/42",
        );
        assert_eq!(
            html,
            "Approved <a href=\"https://github.com/acme/widgets/pull/42\">widgets#42</a>. Nothing blocking from me."
        );
        // Without the ref in the text, the URL is the only link: kept.
        let html = message_html(
            "Approved your PR.\nhttps://github.com/acme/widgets/pull/42",
            "acme/widgets",
            42,
            "https://github.com/acme/widgets/pull/42",
        );
        assert!(html.ends_with("<br><a href=\"https://github.com/acme/widgets/pull/42\">https://github.com/acme/widgets/pull/42</a>"));
        // The long form links too; another PR's number does not.
        assert!(message_html("acme/widgets#42,", "acme/widgets", 42, "u").starts_with("<a href=\"u\">acme/widgets#42</a>,"));
        assert_eq!(message_html("widgets#43", "acme/widgets", 42, "u"), "widgets#43");
        // No PR (the test message): escape only.
        assert_eq!(message_html("<b>hi</b>", "", 0, ""), "&lt;b&gt;hi&lt;/b&gt;");
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

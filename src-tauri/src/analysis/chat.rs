//! Assistant chat: a per-PR conversation with the analysis (assessment,
//! graph, research trace) in context. Research tools run freely; mutating
//! app actions pause as a pending action until the user confirms, then run
//! through the same audited paths as a click — so the app remembers them as
//! steps taken in the review, whoever typed them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message, StopReason};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::analysis::engine::{
    bedrock_client, converse_once, describe_tool_call, document_to_value, tool_result,
};
use crate::analysis::tools::RepoTools;
use crate::analysis::types::{
    events, AnalysisResult, ChatEvent, ChatItem, ChatItemKind, ChatPendingAction, ChatTranscript,
};
use crate::devlog;
use crate::error::{AppError, AppResult};
use crate::models::{Settings, TrackedPr};
use crate::secrets;
use crate::store::Store;

const MAX_CHAT_TURNS: usize = 12;
const MAX_OUTPUT_TOKENS: i32 = 4096;
const MAX_CONVERSATION_CHARS: usize = 30_000;

const CHAT_SYSTEM_PROMPT: &str = r#"You are CORA's review assistant — a principal engineer pairing with the user on a pull request they are reviewing. The architecture analysis below (when present) is shared context; ground your answers in it and cite concrete findings rather than re-deriving them. Use the research tools when a question needs evidence the context doesn't hold. Stay at review altitude: boundaries, contracts, risk — not style nits.

You can also act through the app: posting comments, replying to or resolving threads, submitting a review. Each action tool call pauses until the user confirms it in the panel; the app then executes it and records it in the user's own action history. Propose one action at a time, with the exact text you intend to post. Never claim an action happened unless its tool result says so.

mark_files_viewed is the exception: it is local review-progress bookkeeping (the checkboxes in the file list), so it runs immediately without confirmation. When the user asks to mark files viewed, call it with the exact diff paths — or all=true when they mean everything.

Comment etiquette: prefix comments that need no action with "praise:", "note:", or "fyi:" (or add "(non-blocking)") — the app excludes those threads from the approve gate. Leave actionable comments unprefixed so they demand resolution.

Keep answers tight and skimmable (markdown). Short paragraphs, no preamble."#;

/// One PR's live conversation. Bedrock messages are kept verbatim so the
/// prompt cache stays warm; items are the UI transcript.
pub struct ChatSession {
    system: String,
    head_sha: String,
    messages: Vec<Message>,
    items: Vec<ChatItem>,
    pending: Option<PendingAction>,
    /// Tool results collected before the turn paused on an action —
    /// completed with the action's own result before resuming.
    pending_results: Vec<ContentBlock>,
    busy: bool,
}

struct PendingAction {
    tool_use_id: String,
    name: String,
    input: Value,
    public: ChatPendingAction,
}

#[derive(Default)]
pub struct ChatSessions {
    map: Mutex<HashMap<String, ChatSession>>,
    /// Bedrock client reused across messages; rebuilt when AWS settings change.
    client: tokio::sync::Mutex<Option<(String, aws_sdk_bedrockruntime::Client)>>,
}

/// Loading the AWS config chain (profile files, credentials, region) per
/// message adds real latency — cache the client until settings change.
async fn client_for(app: &AppHandle, settings: &Settings) -> aws_sdk_bedrockruntime::Client {
    let key = format!(
        "{}|{}|{}",
        settings.aws_profile, settings.aws_region, settings.aws_endpoint_url
    );
    let sessions = app.state::<ChatSessions>();
    let mut cached = sessions.client.lock().await;
    if let Some((k, client)) = cached.as_ref() {
        if *k == key {
            return client.clone();
        }
    }
    let client = bedrock_client(settings).await;
    *cached = Some((key, client.clone()));
    client
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// -- session plumbing ---------------------------------------------------------

fn with_session<R>(
    app: &AppHandle,
    pr_id: &str,
    f: impl FnOnce(&mut ChatSession) -> R,
) -> AppResult<R> {
    let sessions = app.state::<ChatSessions>();
    let mut map = sessions.map.lock().unwrap();
    let session = map
        .get_mut(pr_id)
        .ok_or_else(|| AppError::Other("no chat session".into()))?;
    Ok(f(session))
}

/// Append the collected tool results as the next user message.
fn push_results(app: &AppHandle, pr_id: &str, results: Vec<ContentBlock>) -> AppResult<()> {
    with_session(app, pr_id, |s| {
        if let Ok(msg) = Message::builder()
            .role(ConversationRole::User)
            .set_content(Some(results))
            .build()
        {
            s.messages.push(msg);
        }
    })
}

fn emit_event(app: &AppHandle, pr_id: &str, item: Option<ChatItem>) {
    let (busy, pending) = with_session(app, pr_id, |s| {
        (s.busy, s.pending.as_ref().map(|p| p.public.clone()))
    })
    .unwrap_or((false, None));
    let _ = app.emit(
        events::CHAT_EVENT,
        ChatEvent { pr_id: pr_id.to_string(), item, busy, pending },
    );
}

fn push_item(app: &AppHandle, pr_id: &str, kind: ChatItemKind, text: impl Into<String>) {
    let item = ChatItem { at: now(), kind, text: text.into() };
    let _ = with_session(app, pr_id, |s| s.items.push(item.clone()));
    emit_event(app, pr_id, Some(item));
}

fn set_busy(app: &AppHandle, pr_id: &str, busy: bool) {
    let _ = with_session(app, pr_id, |s| s.busy = busy);
    emit_event(app, pr_id, None);
}

// -- context seeding ----------------------------------------------------------

fn build_system(pr: &TrackedPr, analysis: Option<&AnalysisResult>, settings: &Settings) -> String {
    let mut s = String::from(CHAT_SYSTEM_PROMPT);
    s.push_str(&crate::analysis::engine::conventions_section(settings));
    s.push_str(&format!(
        "\n\n## Pull request\n{repo} #{num}: {title}\nAuthor: {author} · head {head} · +{add} −{del} across {files} files\nURL: {url}",
        repo = pr.info.repo,
        num = pr.info.number,
        title = pr.info.title,
        author = pr.info.author,
        head = pr.info.head_sha,
        add = pr.info.additions,
        del = pr.info.deletions,
        files = pr.info.changed_files,
        url = pr.info.url,
    ));
    match analysis {
        Some(a) => {
            s.push_str("\n\n## Architecture analysis (completed for this head)\n");
            s.push_str(
                &serde_json::to_string(&json!({
                    "assessment": a.assessment,
                    "graph": a.graph,
                }))
                .unwrap_or_default(),
            );
            let steps: Vec<&str> = a
                .trace
                .iter()
                .filter(|t| t.kind == "tool")
                .map(|t| t.message.as_str())
                .take(80)
                .collect();
            if !steps.is_empty() {
                s.push_str("\n\n### Research already done during the analysis\n");
                s.push_str(&steps.join("\n"));
            }
        }
        None => s.push_str(
            "\n\n(No architecture analysis has been run for this head yet — explore with the research tools as needed.)",
        ),
    }
    s
}

/// Create the session if absent; refresh its context when the head moved.
fn ensure_session(app: &AppHandle, pr_id: &str) -> AppResult<()> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;

    let sessions = app.state::<ChatSessions>();
    {
        let map = sessions.map.lock().unwrap();
        if let Some(existing) = map.get(pr_id) {
            if existing.head_sha == pr.info.head_sha {
                return Ok(());
            }
        }
    }

    let analysis = store
        .get_analysis(pr_id, "context", "", &pr.info.head_sha)?
        .and_then(|json| serde_json::from_str::<AnalysisResult>(&json).ok());
    let settings = store.settings()?;
    let system = build_system(&pr, analysis.as_ref(), &settings);

    let mut map = sessions.map.lock().unwrap();
    match map.get_mut(pr_id) {
        // Head moved mid-conversation: keep the transcript, swap the context.
        Some(existing) => {
            existing.system = system;
            existing.head_sha = pr.info.head_sha.clone();
        }
        None => {
            map.insert(
                pr_id.to_string(),
                ChatSession {
                    system,
                    head_sha: pr.info.head_sha.clone(),
                    messages: Vec::new(),
                    items: Vec::new(),
                    pending: None,
                    pending_results: Vec::new(),
                    busy: false,
                },
            );
        }
    }
    Ok(())
}

// -- tools --------------------------------------------------------------------

/// Mutating app actions: paused for confirmation, executed via the same
/// audited paths as the UI buttons.
fn action_specs() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "post_pr_comment",
            "Post a top-level comment on the PR conversation. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "body": {"type": "string", "description": "Markdown comment text"}
            }, "required": ["body"]}),
        ),
        (
            "post_diff_comment",
            "Post a line-anchored review comment on the diff. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "line": {"type": "integer", "description": "Line number on the new side of the diff"},
                "body": {"type": "string"}
            }, "required": ["path", "line", "body"]}),
        ),
        (
            "reply_to_thread",
            "Reply to a review thread (get thread ids from get_pr_conversation). Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "thread_id": {"type": "string"},
                "body": {"type": "string"}
            }, "required": ["thread_id", "body"]}),
        ),
        (
            "resolve_thread",
            "Resolve or unresolve a review thread. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "thread_id": {"type": "string"},
                "resolve": {"type": "boolean"}
            }, "required": ["thread_id", "resolve"]}),
        ),
        (
            "submit_review",
            "Submit the user's review: approve, request-changes, or comment. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "event": {"type": "string", "enum": ["approve", "request-changes", "comment"]},
                "body": {"type": "string", "description": "Review comment (required for request-changes)"}
            }, "required": ["event", "body"]}),
        ),
    ]
}

const ACTION_NAMES: [&str; 5] = [
    "post_pr_comment",
    "post_diff_comment",
    "reply_to_thread",
    "resolve_thread",
    "submit_review",
];

fn is_action(name: &str) -> bool {
    ACTION_NAMES.contains(&name)
}

fn chat_specs() -> Vec<(&'static str, &'static str, Value)> {
    let mut specs = RepoTools::specs();
    specs.push((
        "get_pr_conversation",
        "Read the PR's comments and review threads, including thread ids for reply_to_thread / resolve_thread.",
        json!({"type": "object", "properties": {}, "required": []}),
    ));
    specs.push((
        "mark_files_viewed",
        "Mark diff files as viewed (or unviewed) in the reviewer's file list. Local review-progress bookkeeping — applies immediately, no confirmation. Use exact paths from the diff, or all=true for every file.",
        json!({"type": "object", "properties": {
            "paths": {"type": "array", "items": {"type": "string"}, "description": "File paths exactly as they appear in the diff"},
            "all": {"type": "boolean", "description": "Mark every file in the diff; ignores paths"},
            "viewed": {"type": "boolean", "description": "true to mark viewed (default), false to unmark"}
        }, "required": []}),
    ));
    specs.extend(action_specs());
    specs
}

fn str_arg(input: &Value, key: &str) -> AppResult<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::Other(format!("missing {key}")))
}

/// The confirm card the user sees: what would run, and the exact text.
fn describe_action(name: &str, input: &Value) -> ChatPendingAction {
    let body = input.get("body").and_then(Value::as_str).unwrap_or("").to_string();
    let (summary, detail) = match name {
        "post_pr_comment" => ("Comment on the PR".to_string(), body),
        "post_diff_comment" => (
            format!(
                "Comment on {}:{}",
                input.get("path").and_then(Value::as_str).unwrap_or("?"),
                input.get("line").and_then(Value::as_i64).unwrap_or(0)
            ),
            body,
        ),
        "reply_to_thread" => ("Reply to a review thread".to_string(), body),
        "resolve_thread" => (
            if input.get("resolve").and_then(Value::as_bool).unwrap_or(true) {
                "Resolve a review thread".to_string()
            } else {
                "Unresolve a review thread".to_string()
            },
            String::new(),
        ),
        "submit_review" => (
            format!(
                "Submit review: {}",
                input.get("event").and_then(Value::as_str).unwrap_or("comment")
            ),
            body,
        ),
        other => (other.to_string(), body),
    };
    ChatPendingAction { name: name.to_string(), summary, detail }
}

/// Execute a confirmed action through the app's command layer, and audit the
/// ones that aren't already audited there — the history reads as the user's.
async fn execute_action(app: &AppHandle, pr_id: &str, action: &PendingAction) -> AppResult<String> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let label = crate::commands::pr_label(&store, pr_id);
    let input = &action.input;
    match action.name.as_str() {
        "post_pr_comment" => {
            crate::commands::add_pr_comment(app.clone(), pr_id.to_string(), str_arg(input, "body")?)
                .await?;
            store.add_audit("commented", pr_id, &label, "", "via assistant")?;
            Ok("Posted a comment on the PR".into())
        }
        "post_diff_comment" => {
            let path = str_arg(input, "path")?;
            let line = input
                .get("line")
                .and_then(Value::as_i64)
                .ok_or_else(|| AppError::Other("missing line".into()))?;
            crate::commands::add_diff_comment(
                app.clone(),
                pr_id.to_string(),
                path.clone(),
                line,
                str_arg(input, "body")?,
            )
            .await?;
            store.add_audit(
                "diff-commented",
                pr_id,
                &label,
                "",
                &format!("{path}:{line} · via assistant"),
            )?;
            Ok(format!("Posted a review comment on {path}:{line}"))
        }
        "reply_to_thread" => {
            crate::commands::reply_to_thread(
                app.clone(),
                str_arg(input, "thread_id")?,
                str_arg(input, "body")?,
            )
            .await?;
            store.add_audit("replied", pr_id, &label, "", "via assistant")?;
            Ok("Replied to the review thread".into())
        }
        "resolve_thread" => {
            let resolve = input.get("resolve").and_then(Value::as_bool).unwrap_or(true);
            crate::commands::resolve_thread(app.clone(), str_arg(input, "thread_id")?, resolve)
                .await?;
            let verb = if resolve { "thread-resolved" } else { "thread-unresolved" };
            store.add_audit(verb, pr_id, &label, "", "via assistant")?;
            Ok(if resolve {
                "Resolved the review thread".into()
            } else {
                "Unresolved the review thread".into()
            })
        }
        // submit_review audits itself ("approved" / "changes-requested").
        "submit_review" => {
            let event = str_arg(input, "event")?;
            let body = input.get("body").and_then(Value::as_str).unwrap_or("").to_string();
            crate::commands::submit_review(app.clone(), pr_id.to_string(), event.clone(), body)
                .await?;
            Ok(format!("Submitted review: {event}"))
        }
        other => Err(AppError::Other(format!("unknown action: {other}"))),
    }
}

async fn execute_research(
    app: &AppHandle,
    tools: &RepoTools,
    pr_id: &str,
    name: &str,
    input: &Value,
) -> AppResult<String> {
    if name == "get_pr_conversation" {
        let convo = crate::commands::get_pr_comments(app.clone(), pr_id.to_string()).await?;
        let mut text =
            serde_json::to_string(&convo).map_err(|e| AppError::Other(e.to_string()))?;
        if text.len() > MAX_CONVERSATION_CHARS {
            text.truncate(MAX_CONVERSATION_CHARS);
            text.push_str("\n[truncated]");
        }
        return Ok(text);
    }
    if name == "mark_files_viewed" {
        let all = input.get("all").and_then(Value::as_bool).unwrap_or(false);
        let viewed = input.get("viewed").and_then(Value::as_bool).unwrap_or(true);
        let paths: Vec<String> = input
            .get("paths")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if !all && paths.is_empty() {
            return Err(AppError::Other("pass paths or all=true".into()));
        }
        let _ = app.emit(
            crate::models::events::MARK_VIEWED,
            crate::models::MarkViewedEvent {
                pr_id: pr_id.to_string(),
                paths: paths.clone(),
                all,
                viewed,
            },
        );
        let verb = if viewed { "viewed" } else { "unviewed" };
        return Ok(if all {
            format!("Marked every file in the diff as {verb}.")
        } else {
            format!("Marked {} file(s) as {verb}: {}", paths.len(), paths.join(", "))
        });
    }
    tools.execute(name, input).await
}

// -- public entry points (called from commands) --------------------------------

pub fn transcript(app: &AppHandle, pr_id: &str) -> AppResult<ChatTranscript> {
    ensure_session(app, pr_id)?;
    with_session(app, pr_id, |s| ChatTranscript {
        items: s.items.clone(),
        busy: s.busy,
        pending: s.pending.as_ref().map(|p| p.public.clone()),
    })
}

pub fn clear(app: &AppHandle, pr_id: &str) -> AppResult<()> {
    let sessions = app.state::<ChatSessions>();
    sessions.map.lock().unwrap().remove(pr_id);
    let _ = app.emit(
        events::CHAT_EVENT,
        ChatEvent { pr_id: pr_id.to_string(), item: None, busy: false, pending: None },
    );
    Ok(())
}

pub fn send(app: AppHandle, pr_id: String, text: String) -> AppResult<()> {
    if text.trim().is_empty() {
        return Err(AppError::Other("message is empty".into()));
    }
    ensure_session(&app, &pr_id)?;
    with_session(&app, &pr_id, |s| {
        if s.busy {
            return Err(AppError::Other("the assistant is still working".into()));
        }
        if s.pending.is_some() {
            return Err(AppError::Other(
                "confirm or dismiss the pending action first".into(),
            ));
        }
        s.messages.push(
            Message::builder()
                .role(ConversationRole::User)
                .content(ContentBlock::Text(text.clone()))
                .build()
                .map_err(|e| AppError::Other(e.to_string()))?,
        );
        s.busy = true;
        Ok(())
    })??;
    push_item(&app, &pr_id, ChatItemKind::User, text);
    tauri::async_runtime::spawn(async move { drive(&app, &pr_id).await });
    Ok(())
}

pub fn confirm(app: AppHandle, pr_id: String, approve: bool) -> AppResult<()> {
    // Claim the pending action before spawning so a double-click can't run it twice.
    let (pending, mut results) = with_session(&app, &pr_id, |s| {
        let Some(pending) = s.pending.take() else {
            return Err(AppError::Other("no pending action".into()));
        };
        s.busy = true;
        Ok((pending, std::mem::take(&mut s.pending_results)))
    })??;
    emit_event(&app, &pr_id, None);

    tauri::async_runtime::spawn(async move {
        let (content, is_error) = if approve {
            match execute_action(&app, &pr_id, &pending).await {
                Ok(done) => {
                    push_item(&app, &pr_id, ChatItemKind::Action, format!("✓ {done}"));
                    (format!("Done: {done}"), false)
                }
                Err(e) => {
                    push_item(
                        &app,
                        &pr_id,
                        ChatItemKind::Error,
                        format!("{} failed: {e}", pending.public.summary),
                    );
                    (format!("error: {e}"), true)
                }
            }
        } else {
            push_item(
                &app,
                &pr_id,
                ChatItemKind::Action,
                format!("✕ Declined: {}", pending.public.summary),
            );
            ("The user declined this action.".into(), false)
        };

        match tool_result(&pending.tool_use_id, content, is_error) {
            Ok(block) => {
                results.push(block);
                if push_results(&app, &pr_id, results).is_ok() {
                    drive(&app, &pr_id).await;
                } else {
                    set_busy(&app, &pr_id, false);
                }
            }
            Err(e) => {
                push_item(&app, &pr_id, ChatItemKind::Error, e.to_string());
                set_busy(&app, &pr_id, false);
            }
        }
    });
    Ok(())
}

// -- the agent loop -------------------------------------------------------------

async fn drive(app: &AppHandle, pr_id: &str) {
    if let Err(e) = drive_inner(app, pr_id).await {
        devlog::warn(app, "chat", format!("chat turn failed: {e}"));
        push_item(app, pr_id, ChatItemKind::Error, e.to_string());
        set_busy(app, pr_id, false);
    }
}

async fn drive_inner(app: &AppHandle, pr_id: &str) -> AppResult<()> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let settings = store.settings()?;
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let client = client_for(app, &settings).await;
    let tools = RepoTools::new(
        &settings.github_graphql_url,
        &pr.info.repo,
        pr.info.number,
        &pr.info.head_sha,
        &token,
    )?;
    let specs = chat_specs();
    let mut use_cache = true;

    for _turn in 0..MAX_CHAT_TURNS {
        let (system, messages) =
            with_session(app, pr_id, |s| (s.system.clone(), s.messages.clone()))?;

        let resp = converse_once(
            app,
            "chat",
            &client,
            &settings.bedrock_model_id,
            &system,
            &messages,
            &specs,
            MAX_OUTPUT_TOKENS,
            &mut use_cache,
            &settings.aws_profile,
        )
        .await?;

        let Some(message) = resp.output().and_then(|o| o.as_message().ok().cloned()) else {
            return Err(AppError::Other("Bedrock returned no message".into()));
        };

        let mut research: Vec<(String, String, Value)> = Vec::new();
        let mut action: Option<(String, String, Value)> = None;
        let mut declined_extras: Vec<String> = Vec::new();
        for block in message.content() {
            match block {
                ContentBlock::Text(t) => {
                    if !t.trim().is_empty() {
                        push_item(app, pr_id, ChatItemKind::Assistant, t.clone());
                    }
                }
                ContentBlock::ToolUse(tu) => {
                    let input = document_to_value(tu.input());
                    if is_action(tu.name()) {
                        if action.is_none() {
                            action = Some((tu.tool_use_id().into(), tu.name().into(), input));
                        } else {
                            declined_extras.push(tu.tool_use_id().into());
                        }
                    } else {
                        push_item(
                            app,
                            pr_id,
                            ChatItemKind::Tool,
                            describe_tool_call(tu.name(), &input),
                        );
                        research.push((tu.tool_use_id().into(), tu.name().into(), input));
                    }
                }
                _ => {}
            }
        }

        with_session(app, pr_id, |s| s.messages.push(message))?;

        let mut results: Vec<ContentBlock> = Vec::new();
        if !research.is_empty() {
            let executed = futures::future::join_all(research.iter().map(|(_, name, input)| {
                let tools = &tools;
                async move { execute_research(app, tools, pr_id, name, input).await }
            }))
            .await;
            for ((id, name, _), result) in research.iter().zip(executed) {
                let (content, is_error) = match result {
                    Ok(text) => (text, false),
                    Err(e) => {
                        devlog::warn(app, "chat", format!("tool {name} failed: {e}"));
                        (format!("error: {e}"), true)
                    }
                };
                results.push(tool_result(id, content, is_error)?);
            }
        }
        for id in declined_extras {
            results.push(tool_result(
                &id,
                "One action at a time — re-issue this after the pending action resolves.".into(),
                true,
            )?);
        }

        if let Some((tool_use_id, name, input)) = action {
            let public = describe_action(&name, &input);
            with_session(app, pr_id, |s| {
                s.pending = Some(PendingAction { tool_use_id, name, input, public });
                s.pending_results = results;
                s.busy = false;
            })?;
            emit_event(app, pr_id, None);
            return Ok(());
        }

        if !results.is_empty() {
            push_results(app, pr_id, results)?;
            continue;
        }

        if matches!(resp.stop_reason(), StopReason::EndTurn | StopReason::StopSequence) {
            set_busy(app, pr_id, false);
            return Ok(());
        }
    }

    push_item(
        app,
        pr_id,
        ChatItemKind::Error,
        format!("stopped after {MAX_CHAT_TURNS} tool turns — ask again to continue"),
    );
    set_busy(app, pr_id, false);
    Ok(())
}

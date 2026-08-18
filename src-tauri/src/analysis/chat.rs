//! Assistant chat: a per-PR conversation with the analysis (assessment,
//! graph, research trace) in context. Research tools run freely; mutating
//! app actions pause as a pending action until the user confirms, then run
//! through the same audited paths as a click — so the app remembers them as
//! steps taken in the review, whoever typed them.

use std::collections::HashMap;
use std::sync::Mutex;

use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, Message, StopReason, ToolResultContentBlock,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::analysis::engine::{
    bedrock_client, converse_once, describe_tool_call, document_to_value, strip_trailing_reasoning,
    tool_result,
};
use crate::analysis::tools::RepoTools;
use crate::analysis::types::{
    events, AnalysisResult, ChatContext, ChatEvent, ChatItem, ChatItemKind, ChatPendingAction,
    ChatTranscript, ChatUsage, ContextGroup, ContextPart,
};
use crate::devlog;
use crate::error::{AppError, AppResult};
use crate::models::{Settings, TrackedPr};
use crate::secrets;

const MAX_CHAT_TURNS: usize = 12;
const MAX_OUTPUT_TOKENS: i32 = 4096;
const MAX_CONVERSATION_CHARS: usize = 30_000;

const CHAT_SYSTEM_PROMPT: &str = r#"You are CORA's review assistant — a principal engineer pairing with the user on a pull request they are reviewing. The architecture analysis below (when present) is shared context; ground your answers in it and cite concrete findings rather than re-deriving them. Use the research tools when a question needs evidence the context doesn't hold. Stay at review altitude: boundaries, contracts, risk — not style nits.

You can drive the whole app, not just answer about it. Anything the user can do from the UI you can do too, split by consequence:

Actions that change GitHub — commenting, replying, resolving threads, submitting a review, merging, closing, reopening, reacting — or that take a PR out of the user's queue (muting, untracking) pause until the user confirms them in the panel. The app then executes them and records them in the user's own action history. Propose one at a time, with the exact text you intend to post. Never claim an action happened unless its tool result says so.

Local review state applies immediately, no confirmation: mark_files_viewed (the checkboxes in the file list), set_pr_priority, set_repo_priority, set_author_priority, set_review_mark, refresh_pr, run_analysis. These only change Cora's own view and the user can undo them from the UI. When the user asks to mark files viewed, call mark_files_viewed with the exact diff paths — or all=true when they mean everything.

Prefer doing over describing: if the user asks for something an action tool covers, call the tool rather than explaining how they could do it themselves.

When you can name the fix in code, attach it: post_diff_comment takes a `suggestion` — replacement lines the author accepts in one click. Anchor it to the exact range you are replacing (`start_line`..`line`) and make the suggestion the complete replacement for that range, indented as it must appear in the file. Put your reasoning in `body` and only the code in `suggestion`. Prefer a suggestion over describing an edit in prose whenever the change is small and you can see the surrounding lines; skip it when the fix spans files or you would be guessing at code you haven't read.

The user can edit your text before it posts, so write the real thing rather than a sketch.

Comment etiquette: prefix comments that need no action with "praise:", "note:", or "fyi:" (or add "(non-blocking)") — the app excludes those threads from the approve gate. Leave actionable comments unprefixed so they demand resolution.

Keep answers tight and skimmable (markdown). Short paragraphs, no preamble."#;

/// One PR's live conversation. Bedrock messages are kept verbatim so the
/// prompt cache stays warm; items are the UI transcript.
pub struct ChatSession {
    /// The system prompt as labelled sections. Kept split (rather than as
    /// one string) so the context inspector can attribute every byte to
    /// where it came from; joined verbatim on the way to Bedrock.
    system_parts: Vec<SystemPart>,
    head_sha: String,
    messages: Vec<Message>,
    items: Vec<ChatItem>,
    pending: Option<PendingAction>,
    /// Tool results collected before the turn paused on an action —
    /// completed with the action's own result before resuming.
    pending_results: Vec<ContentBlock>,
    busy: bool,
    /// What the last request cost, and how many we've made this session.
    usage: Option<ChatUsage>,
    requests: i64,
    /// Our own estimate for the request `usage` measured, so the running
    /// total can be scaled to what Bedrock actually counts.
    last_sent_est: i64,
    /// Context outside the conversation moved (an analysis finished) —
    /// rebuild the system prompt before the next turn.
    system_stale: bool,
}

impl ChatSession {
    fn system(&self) -> String {
        self.system_parts.iter().map(|p| p.text.as_str()).collect()
    }
}

/// A labelled section of the system prompt.
#[derive(Clone)]
struct SystemPart {
    label: &'static str,
    origin: &'static str,
    text: String,
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
    let out = with_session(app, pr_id, |s| {
        if let Ok(msg) = Message::builder()
            .role(ConversationRole::User)
            .set_content(Some(results))
            .build()
        {
            s.messages.push(msg);
        }
    });
    // Tool results are usually the largest thing that ever enters the
    // context, and they arrive between transcript items — announce them so
    // the context meter moves when the bytes land, not a step later.
    emit_event(app, pr_id, None);
    out
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

fn build_system(
    pr: &TrackedPr,
    analysis: Option<&AnalysisResult>,
    settings: &Settings,
) -> Vec<SystemPart> {
    let mut parts = vec![SystemPart {
        label: "Assistant instructions",
        origin: "app",
        text: String::from(CHAT_SYSTEM_PROMPT),
    }];
    let conventions = crate::analysis::engine::conventions_section(settings);
    if !conventions.is_empty() {
        parts.push(SystemPart {
            label: "Team conventions",
            origin: "your settings",
            text: conventions,
        });
    }
    if let Some(text) =
        crate::analysis::engine::repo_instructions_section(settings, &pr.info.repo)
    {
        parts.push(SystemPart {
            label: "Repo review instructions",
            origin: "repo settings",
            text,
        });
    }
    parts.push(SystemPart {
        label: "PR facts",
        origin: "github",
        text: format!(
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
        ),
    });
    match analysis {
        Some(a) => {
            parts.push(SystemPart {
                label: "Architecture analysis",
                origin: "analysis",
                text: format!(
                    "\n\n## Architecture analysis (completed for this head)\n{}",
                    serde_json::to_string(&json!({
                        "assessment": a.assessment,
                        "graph": a.graph,
                    }))
                    .unwrap_or_default()
                ),
            });
            let steps: Vec<&str> = a
                .trace
                .iter()
                .filter(|t| t.kind == "tool")
                .map(|t| t.message.as_str())
                .take(80)
                .collect();
            if !steps.is_empty() {
                parts.push(SystemPart {
                    label: "Analysis research trace",
                    origin: "analysis",
                    text: format!(
                        "\n\n### Research already done during the analysis\n{}",
                        steps.join("\n")
                    ),
                });
            }
        }
        None => parts.push(SystemPart {
            label: "No analysis yet",
            origin: "app",
            text: String::from(
                "\n\n(No architecture analysis has been run for this head yet — explore with the research tools as needed.)",
            ),
        }),
    }
    parts
}

/// Create the session if absent; refresh its context when the head moved.
fn ensure_session(app: &AppHandle, pr_id: &str) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;

    let sessions = app.state::<ChatSessions>();
    {
        let map = sessions.map.lock().unwrap();
        if let Some(existing) = map.get(pr_id) {
            if existing.head_sha == pr.info.head_sha && !existing.system_stale {
                return Ok(());
            }
        }
    }

    let analysis = store
        .get_analysis(pr_id, "context", "", &pr.info.head_sha)?
        .and_then(|json| serde_json::from_str::<AnalysisResult>(&json).ok());
    let settings = store.settings()?;
    let system_parts = build_system(&pr, analysis.as_ref(), &settings);

    let mut map = sessions.map.lock().unwrap();
    match map.get_mut(pr_id) {
        // Head moved mid-conversation: keep the transcript, swap the context.
        Some(existing) => {
            existing.system_parts = system_parts;
            existing.head_sha = pr.info.head_sha.clone();
            existing.system_stale = false;
        }
        None => {
            map.insert(
                pr_id.to_string(),
                ChatSession {
                    system_parts,
                    head_sha: pr.info.head_sha.clone(),
                    messages: Vec::new(),
                    items: Vec::new(),
                    pending: None,
                    pending_results: Vec::new(),
                    busy: false,
                    usage: None,
                    requests: 0,
                    last_sent_est: 0,
                    system_stale: false,
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
            "Post a line-anchored review comment on the diff, optionally with a suggested change the author can accept in one click. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "line": {"type": "integer", "description": "Line number on the new side of the diff. Must be an ADDED or CONTEXT line inside a changed hunk — GitHub rejects anchors on unchanged code outside the diff. With a suggestion, the LAST line being replaced"},
                "start_line": {"type": "integer", "description": "First line of the range, when the comment or suggestion covers several lines. Same rule: must be inside a changed hunk, above `line`"},
                "body": {"type": "string", "description": "The prose. With a suggestion, say why — the replacement code goes in `suggestion`, not here"},
                "suggestion": {"type": "string", "description": "Replacement code for lines start_line..line, verbatim and correctly indented. Must be the complete replacement for every line in the range — GitHub swaps the whole range for this text"}
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
        (
            "merge_pr",
            "Merge the pull request. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "method": {"type": "string", "enum": ["squash", "merge", "rebase"], "description": "Defaults to squash"}
            }, "required": []}),
        ),
        (
            "close_pr",
            "Close the pull request without merging. Pauses for user confirmation.",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
        (
            "reopen_pr",
            "Reopen a closed pull request. Pauses for user confirmation.",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
        (
            "toggle_reaction",
            "Add or remove an emoji reaction on a comment (ids from get_pr_conversation). Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "subject_id": {"type": "string", "description": "Comment id to react to"},
                "content": {"type": "string", "enum": ["THUMBS_UP", "THUMBS_DOWN", "LAUGH", "HOORAY", "CONFUSED", "HEART", "ROCKET", "EYES"]},
                "remove": {"type": "boolean", "description": "true to take the reaction back"}
            }, "required": ["subject_id", "content"]}),
        ),
        (
            "set_pr_muted",
            "Mute or unmute this PR — muted PRs drop out of the queue and stop notifying. Pauses for user confirmation.",
            json!({"type": "object", "properties": {
                "muted": {"type": "boolean"}
            }, "required": ["muted"]}),
        ),
        (
            "untrack_pr",
            "Stop tracking this PR entirely (undoable from History). Pauses for user confirmation.",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
    ]
}

/// Derived from `action_specs()` rather than listed again: a second list
/// could fall out of sync, and the failure mode is silent and bad — an
/// action missing from it would be treated as a research tool and execute
/// against GitHub with no confirm card.
fn is_action(name: &str) -> bool {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| action_specs().into_iter().map(|(name, _, _)| name).collect())
        .contains(&name)
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
    // Local review-state controls. These change only Cora's own view of the
    // queue and are reversible from the UI, so they run without confirmation
    // — the same bargain as mark_files_viewed.
    specs.push((
        "set_pr_priority",
        "Set this PR's priority in the reviewer's queue. Applies immediately.",
        json!({"type": "object", "properties": {
            "priority": {"type": "string", "enum": ["high", "normal", "low"]}
        }, "required": ["priority"]}),
    ));
    specs.push((
        "set_repo_priority",
        "Set a repository's priority, or ignore it entirely. Applies immediately.",
        json!({"type": "object", "properties": {
            "repo": {"type": "string", "description": "owner/name; defaults to this PR's repo"},
            "priority": {"type": "string", "enum": ["high", "normal", "low", "ignored"]}
        }, "required": ["priority"]}),
    ));
    specs.push((
        "set_author_priority",
        "Set an author's priority — how much their PRs are surfaced. Applies immediately.",
        json!({"type": "object", "properties": {
            "author": {"type": "string", "description": "Login; defaults to this PR's author"},
            "priority": {"type": "string", "enum": ["high", "normal", "low", "ignored"]}
        }, "required": ["priority"]}),
    ));
    specs.push((
        "set_review_mark",
        "Mark this PR reviewed up to its current head — later commits then show as new. Applies immediately.",
        json!({"type": "object", "properties": {}, "required": []}),
    ));
    specs.push((
        "refresh_pr",
        "Re-fetch this PR from GitHub (state, checks, reviews). Applies immediately.",
        json!({"type": "object", "properties": {}, "required": []}),
    ));
    specs.push((
        "run_analysis",
        "Run the architecture analysis for this PR. Applies immediately; results arrive in the panel, not as a tool result.",
        json!({"type": "object", "properties": {
            "force": {"type": "boolean", "description": "Re-run even if a cached analysis exists for this head"}
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

/// Fold a proposed action's arguments into the exact text that will post, so
/// the confirm card, the user's edits, and what GitHub receives are all the
/// same string. Today that means turning a `suggestion` argument into the
/// fenced block GitHub renders as an accept-able change.
fn normalize_action_input(name: &str, mut input: Value) -> Value {
    if name != "post_diff_comment" {
        return input;
    }
    let Some(suggestion) = input.get("suggestion").and_then(Value::as_str).map(String::from) else {
        return input;
    };
    let body = input.get("body").and_then(Value::as_str).unwrap_or("").trim().to_string();
    // Trailing newlines inside the fence become blank lines in the applied
    // patch, so trim to exactly the replacement lines.
    let block = format!("```suggestion\n{}\n```", suggestion.trim_end_matches('\n'));
    let composed = if body.is_empty() { block } else { format!("{body}\n\n{block}") };
    input["body"] = Value::String(composed);
    if let Some(obj) = input.as_object_mut() {
        obj.remove("suggestion");
    }
    input
}

/// Actions whose `detail` is prose the user may rewrite before confirming.
fn carries_text(name: &str) -> bool {
    matches!(
        name,
        "post_pr_comment" | "post_diff_comment" | "reply_to_thread" | "submit_review"
    )
}

/// The confirm card the user sees: what would run, and the exact text.
fn describe_action(name: &str, input: &Value) -> ChatPendingAction {
    let body = input.get("body").and_then(Value::as_str).unwrap_or("").to_string();
    let summary = match name {
        "post_pr_comment" => "Comment on the PR".to_string(),
        "post_diff_comment" => format!(
            "Comment on {}:{}",
            input.get("path").and_then(Value::as_str).unwrap_or("?"),
            input.get("line").and_then(Value::as_i64).unwrap_or(0)
        ),
        "reply_to_thread" => "Reply to a review thread".to_string(),
        "resolve_thread" => {
            if input.get("resolve").and_then(Value::as_bool).unwrap_or(true) {
                "Resolve a review thread".to_string()
            } else {
                "Unresolve a review thread".to_string()
            }
        }
        "submit_review" => format!(
            "Submit review: {}",
            input.get("event").and_then(Value::as_str).unwrap_or("comment")
        ),
        "merge_pr" => format!(
            "Merge this PR ({})",
            input.get("method").and_then(Value::as_str).unwrap_or("squash")
        ),
        "close_pr" => "Close this PR without merging".to_string(),
        "reopen_pr" => "Reopen this PR".to_string(),
        "toggle_reaction" => format!(
            "{} {} on a comment",
            if input.get("remove").and_then(Value::as_bool).unwrap_or(false) {
                "Remove"
            } else {
                "React"
            },
            input.get("content").and_then(Value::as_str).unwrap_or("?")
        ),
        "set_pr_muted" => {
            if input.get("muted").and_then(Value::as_bool).unwrap_or(true) {
                "Mute this PR — it leaves your queue".to_string()
            } else {
                "Unmute this PR".to_string()
            }
        }
        "untrack_pr" => "Stop tracking this PR".to_string(),
        other => other.to_string(),
    };
    // The detail is the body for exactly the actions that carry one, which is
    // the same question `editable` answers — so ask it once.
    let editable = carries_text(name);
    ChatPendingAction {
        name: name.to_string(),
        summary,
        detail: if editable { body } else { String::new() },
        editable,
    }
}

/// Execute a confirmed action through the app's command layer, and audit the
/// ones that aren't already audited there — the history reads as the user's.
async fn execute_action(app: &AppHandle, pr_id: &str, action: &PendingAction) -> AppResult<String> {
    let store = app.state::<crate::orgs::Orgs>().active();
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
            let start_line = input.get("start_line").and_then(Value::as_i64);
            let body = str_arg(input, "body")?;
            let suggested = body.contains("```suggestion");
            crate::commands::add_diff_comment(
                app.clone(),
                pr_id.to_string(),
                path.clone(),
                line,
                body,
                start_line,
            )
            .await?;
            store.add_audit(
                "diff-commented",
                pr_id,
                &label,
                "",
                &format!("{path}:{line} · via assistant"),
            )?;
            Ok(if suggested {
                format!("Posted a suggested change on {path}:{line}")
            } else {
                format!("Posted a review comment on {path}:{line}")
            })
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
        // merge / close / reopen audit themselves in the command layer, the
        // same as the buttons do.
        "merge_pr" => {
            let method = input
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("squash")
                .to_string();
            crate::commands::merge_pr(app.clone(), pr_id.to_string(), method.clone()).await?;
            Ok(format!("Merged the PR ({method})"))
        }
        "close_pr" => {
            crate::commands::close_pr(app.clone(), pr_id.to_string()).await?;
            Ok("Closed the PR".into())
        }
        "reopen_pr" => {
            crate::commands::reopen_pr(app.clone(), pr_id.to_string()).await?;
            Ok("Reopened the PR".into())
        }
        "toggle_reaction" => {
            let content = str_arg(input, "content")?;
            let remove = input.get("remove").and_then(Value::as_bool).unwrap_or(false);
            crate::commands::toggle_reaction(
                app.clone(),
                str_arg(input, "subject_id")?,
                content.clone(),
                remove,
            )
            .await?;
            Ok(if remove {
                format!("Removed the {content} reaction")
            } else {
                format!("Reacted {content}")
            })
        }
        "set_pr_muted" => {
            let muted = input.get("muted").and_then(Value::as_bool).unwrap_or(true);
            crate::commands::set_pr_muted(
                app.clone(),
                app.state::<crate::orgs::Orgs>(),
                pr_id.to_string(),
                muted,
            )?;
            Ok(if muted { "Muted the PR".into() } else { "Unmuted the PR".to_string() })
        }
        "untrack_pr" => {
            crate::commands::untrack_pr(
                app.clone(),
                app.state::<crate::orgs::Orgs>(),
                pr_id.to_string(),
            )?;
            Ok("Stopped tracking the PR".into())
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
    if let Some(result) = execute_local(app, pr_id, name, input)? {
        return Ok(result);
    }
    tools.execute(name, input).await
}

/// Local review-state tools. Return None when `name` isn't one of ours, so
/// the caller falls through to the repo research tools.
fn execute_local(
    app: &AppHandle,
    pr_id: &str,
    name: &str,
    input: &Value,
) -> AppResult<Option<String>> {
    let orgs = app.state::<crate::orgs::Orgs>();
    let store = orgs.active();
    let pr = || -> AppResult<crate::models::TrackedPr> {
        store
            .get_pr(pr_id)?
            .ok_or_else(|| AppError::Other("PR not found".into()))
    };
    let result = match name {
        "set_pr_priority" => {
            let priority = crate::models::PrPriority::parse(&str_arg(input, "priority")?);
            crate::commands::set_pr_priority(app.clone(), orgs, pr_id.to_string(), priority)?;
            format!("Priority set to {}", priority.as_str())
        }
        "set_repo_priority" => {
            let repo = match input.get("repo").and_then(Value::as_str) {
                Some(r) if !r.trim().is_empty() => r.to_string(),
                _ => pr()?.info.repo,
            };
            let raw = str_arg(input, "priority")?;
            crate::commands::set_repo_priority(
                app.clone(),
                orgs,
                app.state::<crate::github::poller::PollTrigger>(),
                repo.clone(),
                repo_priority(&raw)?,
            )?;
            format!("{repo} priority set to {raw}")
        }
        "set_author_priority" => {
            let author = match input.get("author").and_then(Value::as_str) {
                Some(a) if !a.trim().is_empty() => a.to_string(),
                _ => pr()?.info.author,
            };
            let raw = str_arg(input, "priority")?;
            crate::commands::set_author_priority(
                app.clone(),
                orgs,
                app.state::<crate::github::poller::PollTrigger>(),
                author.clone(),
                repo_priority(&raw)?,
            )?;
            format!("{author}'s PRs set to {raw}")
        }
        "set_review_mark" => {
            let mark = crate::commands::set_review_mark(orgs, pr_id.to_string())?;
            format!("Marked reviewed through {}", &mark.head_sha[..7.min(mark.head_sha.len())])
        }
        "refresh_pr" => {
            // The refresh itself is async; the tool loop is not, so hand it to
            // the runtime and report that it's under way.
            let app = app.clone();
            let id = pr_id.to_string();
            tauri::async_runtime::spawn(async move {
                let _ = crate::commands::refresh_pr(app, id).await;
            });
            "Refreshing this PR from GitHub".to_string()
        }
        "run_analysis" => {
            let force = input.get("force").and_then(Value::as_bool).unwrap_or(false);
            crate::commands::spawn_analysis_task(
                app.clone(),
                orgs.active_login(),
                pr_id.to_string(),
                crate::analysis::types::AnalysisLevel::Context,
                None,
                force,
            );
            "Started the architecture analysis — it lands in the panel when done".to_string()
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// The tool's enum is closed, so an unknown value is the model's mistake and
/// worth reporting back rather than silently defaulting.
fn repo_priority(s: &str) -> AppResult<crate::models::RepoPriority> {
    crate::models::RepoPriority::parse(s)
        .ok_or_else(|| AppError::Other(format!("unknown priority '{s}'")))
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

/// ~4 chars per token. Deliberately crude: the exact number for a whole
/// request only comes back from Bedrock, and `ChatContext::usage` carries it.
fn est_tokens(text: &str) -> i64 {
    (text.chars().count() as i64 + 3) / 4
}

/// Context window for a model id, 0 when we can't claim to know it. Only
/// used to draw a "how full is it" bar, never to make a decision.
///
/// The id is usually an inference-profile ARN naming no model, which is the
/// same problem pricing has — so use the same answer: the ARN → model mapping
/// read out of the Claude Code settings that configured it.
fn window_tokens(model_id: &str, aliases: &[crate::usage::ModelAlias]) -> i64 {
    if model_id.to_lowercase().contains("claude") || aliases.iter().any(|a| a.model == model_id) {
        200_000
    } else {
        0
    }
}

/// Where a tool's output comes from, so the inspector can say whether bytes
/// are repo source, GitHub state, or the app's own bookkeeping.
fn tool_origin(name: &str) -> &'static str {
    match name {
        "get_file" | "list_tree" | "search_code" | "get_readme_and_docs" => "repo file",
        "get_pr_diff" | "get_commit_diff" | "list_commits" | "list_recent_prs"
        | "get_pr_conversation" => "github",
        "mark_files_viewed" | "set_pr_priority" | "set_repo_priority" | "set_author_priority"
        | "set_review_mark" | "refresh_pr" | "run_analysis" => "app",
        _ => "tool",
    }
}

/// Borrows the text: the sizes-only path is by far the most common caller
/// (every chat event, every turn), and a tool result can be megabytes — it
/// must not be cloned just to be counted and dropped.
fn part(
    group: ContextGroup,
    id: String,
    label: String,
    origin: &str,
    text: &str,
    include_text: bool,
) -> ContextPart {
    ContextPart {
        id,
        group,
        label,
        origin: origin.to_string(),
        est_tokens: est_tokens(text),
        text: if include_text { text.to_string() } else { String::new() },
    }
}

/// Estimated tokens for a whole request: the session's own bytes plus the
/// tool definitions, which ride along on every turn.
fn est_session(s: &ChatSession, tools_est: i64) -> i64 {
    build_parts(s, false).iter().map(|p| p.est_tokens).sum::<i64>() + tools_est
}

/// The tool definitions as context parts — one per tool, schema included.
fn tool_parts(include_text: bool) -> Vec<ContextPart> {
    chat_specs()
        .into_iter()
        .map(|(name, description, schema)| {
            let text = format!(
                "{name}: {description}\n{}",
                serde_json::to_string_pretty(&schema).unwrap_or_default()
            );
            part(
                ContextGroup::Tools,
                format!("tool-{name}"),
                name.to_string(),
                if is_action(name) { "app action" } else { "research tool" },
                &text,
                include_text,
            )
        })
        .collect()
}

/// The tool definitions never change within a build, so their size is a
/// constant — worth caching, since it's summed on every context read.
fn tools_est() -> i64 {
    static TOTAL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(|| tool_parts(false).iter().map(|p| p.est_tokens).sum())
}

/// The session's own bytes: system prompt sections, then every message
/// block. Runs under the session lock, so it borrows rather than clones.
fn build_parts(s: &ChatSession, include_text: bool) -> Vec<ContextPart> {
    let mut parts: Vec<ContextPart> = s
        .system_parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            part(
                ContextGroup::System,
                format!("system-{i}"),
                p.label.to_string(),
                p.origin,
                &p.text,
                include_text,
            )
        })
        .collect();

    // Tool results identify their call by id only — walk forward so each
    // result can be labelled with the tool that produced it.
    let mut call_names: HashMap<&str, &str> = HashMap::new();
    for (m, message) in s.messages.iter().enumerate() {
        let from_user = message.role() == &ConversationRole::User;
        for (b, block) in message.content().iter().enumerate() {
            let id = format!("msg-{m}-{b}");
            match block {
                ContentBlock::Text(t) => {
                    let (label, origin) =
                        if from_user { ("You", "you") } else { ("Assistant", "model") };
                    parts.push(part(
                        ContextGroup::Messages,
                        id,
                        label.to_string(),
                        origin,
                        t,
                        include_text,
                    ));
                }
                ContentBlock::ToolUse(tu) => {
                    call_names.insert(tu.tool_use_id(), tu.name());
                    let input = document_to_value(tu.input());
                    parts.push(part(
                        ContextGroup::Messages,
                        id,
                        format!("call {}", describe_tool_call(tu.name(), &input)),
                        "model",
                        &serde_json::to_string(&input).unwrap_or_default(),
                        include_text,
                    ));
                }
                ContentBlock::ToolResult(tr) => {
                    let name = call_names.get(tr.tool_use_id()).copied().unwrap_or("tool");
                    let text: String = tr
                        .content()
                        .iter()
                        .filter_map(|c| match c {
                            ToolResultContentBlock::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    parts.push(part(
                        ContextGroup::Messages,
                        id,
                        format!("{name} result"),
                        tool_origin(name),
                        &text,
                        include_text,
                    ));
                }
                _ => {}
            }
        }
    }
    parts
}

/// Mark a session's system prompt for rebuild — called when the context it
/// was built from changes (an analysis completing) rather than the head.
/// Silent when there is no session for the PR: the next one builds fresh.
pub fn invalidate_context(app: &AppHandle, pr_id: &str) {
    let _ = with_session(app, pr_id, |s| s.system_stale = true);
    emit_event(app, pr_id, None);
}

/// Same, for every open session — used when something global to the prompt
/// changes, like the team conventions in settings.
pub fn invalidate_all_contexts(app: &AppHandle) {
    let ids: Vec<String> = {
        let sessions = app.state::<ChatSessions>();
        let mut map = sessions.map.lock().unwrap();
        map.iter_mut()
            .map(|(id, s)| {
                s.system_stale = true;
                id.clone()
            })
            .collect()
    };
    for id in ids {
        emit_event(app, &id, None);
    }
}

/// Everything the assistant sees on its next turn, itemised by where it came
/// from. `include_text` is false for the running total in the panel footer —
/// tool results can be megabytes and the meter only needs sizes.
pub fn context(app: &AppHandle, pr_id: &str, include_text: bool) -> AppResult<ChatContext> {
    ensure_session(app, pr_id)?;
    let store = app.state::<crate::orgs::Orgs>().active();
    let model_id = store.settings()?.bedrock_model_id;
    // Built inside the session lock: the meter refetches on every chat event
    // and tool results are large — cloning the message list to read its size
    // would be the most expensive thing this command does.
    let (mut parts, usage, requests, last_sent_est) = with_session(app, pr_id, |s| {
        (build_parts(s, include_text), s.usage.clone(), s.requests, s.last_sent_est)
    })?;

    // Only build the tool schemas when someone will read them; their size on
    // its own is a cached constant.
    let est_tokens = if include_text {
        parts.extend(tool_parts(true));
        parts.iter().map(|p| p.est_tokens).sum()
    } else {
        parts.iter().map(|p| p.est_tokens).sum::<i64>() + tools_est()
    };

    Ok(ChatContext {
        // Raw ~4-chars-per-token, then scaled by how far that estimate was
        // off on the last measured request. Before any request there is
        // nothing to calibrate against, so the raw estimate stands.
        projected_tokens: match (&usage, last_sent_est) {
            (Some(u), sent) if sent > 0 => est_tokens * u.prompt_tokens / sent,
            _ => est_tokens,
        },
        est_tokens,
        window_tokens: window_tokens(&model_id, &crate::usage::claude_settings_aliases()),
        model_id,
        parts,
        usage,
        requests,
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

pub fn confirm(
    app: AppHandle,
    pr_id: String,
    approve: bool,
    edited: Option<String>,
) -> AppResult<()> {
    // Claim the pending action before spawning so a double-click can't run it twice.
    let (mut pending, mut results) = with_session(&app, &pr_id, |s| {
        let Some(pending) = s.pending.take() else {
            return Err(AppError::Other("no pending action".into()));
        };
        s.busy = true;
        Ok((pending, std::mem::take(&mut s.pending_results)))
    })??;

    // What the user approved is what they were looking at. The card shows the
    // composed body verbatim, so an edit replaces it wholesale — no merging
    // back into the arguments the model proposed.
    if let Some(text) = edited.filter(|_| approve && carries_text(&pending.name)) {
        pending.input["body"] = Value::String(text.clone());
        pending.public.detail = text;
    }
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
    let store = app.state::<crate::orgs::Orgs>().active();
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
    // Tool definitions don't change mid-conversation; price them once.
    let specs_est = tools_est();
    let mut use_cache = true;

    for _turn in 0..MAX_CHAT_TURNS {
        let (system, messages) =
            with_session(app, pr_id, |s| (s.system(), s.messages.clone()))?;

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

        // What that request actually cost — the only exact numbers we get,
        // and what the panel's context meter reports.
        if let Some(u) = resp.usage() {
            // input_tokens is only the part the cache didn't serve, so on a
            // warm prefix it reads near zero. The prompt is all three buckets.
            let cache_read = u.cache_read_input_tokens().unwrap_or(0) as i64;
            let cache_write = u.cache_write_input_tokens().unwrap_or(0) as i64;
            let usage = ChatUsage {
                prompt_tokens: u.input_tokens() as i64 + cache_read + cache_write,
                input_tokens: u.input_tokens() as i64,
                output_tokens: u.output_tokens() as i64,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
            };
            // The session still holds exactly what we sent (the reply is
            // pushed below), so this is the estimate that measurement
            // corresponds to — the ratio calibrates every later total.
            with_session(app, pr_id, |s| {
                s.last_sent_est = est_session(s, specs_est);
                s.usage = Some(usage);
                s.requests += 1;
            })?;
            crate::usage::record(app, &pr, "chat", &settings.bedrock_model_id, u);
        }

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

        with_session(app, pr_id, |s| s.messages.push(strip_trailing_reasoning(message)))?;

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
            let input = normalize_action_input(&name, input);
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

        // The turn produced no tool results and didn't cleanly end — most often
        // a MaxTokens truncation. Looping now would re-send a conversation
        // ending on an assistant message, which some Bedrock models reject as a
        // prefill ("must end with a user message"). Append a user turn so the
        // model can continue where it was cut off.
        push_results(
            app,
            pr_id,
            vec![ContentBlock::Text(
                "Your previous response was cut off by the output-token limit. Continue your answer.".into(),
            )],
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The inspector's value is provenance, so every research tool has to say
    /// where its bytes come from. A tool added later that falls through to the
    /// generic label is a gap, not a harmless default.
    #[test]
    fn every_research_tool_declares_an_origin() {
        for (name, _, _) in chat_specs() {
            if is_action(name) {
                continue;
            }
            assert_ne!(
                tool_origin(name),
                "tool",
                "{name} has no origin — add it to tool_origin()"
            );
        }
    }

    /// A suggestion becomes the fenced block GitHub renders as an acceptable
    /// change, folded into the body at propose time — so the confirm card,
    /// the user's edit, and what posts are all the same string.
    #[test]
    fn a_suggestion_becomes_a_fenced_block_in_the_body() {
        let input = json!({
            "path": "src/a.ts",
            "line": 12,
            "body": "This drops the error.",
            "suggestion": "if (!ok) throw new Error(msg);\n",
        });
        let out = normalize_action_input("post_diff_comment", input);
        assert_eq!(
            out["body"],
            json!("This drops the error.\n\n```suggestion\nif (!ok) throw new Error(msg);\n```"),
            "trailing newlines inside the fence would post as blank lines"
        );
        assert!(out.get("suggestion").is_none(), "folded in, not sent twice");
    }

    #[test]
    fn a_suggestion_with_no_prose_is_still_a_valid_body() {
        let out = normalize_action_input(
            "post_diff_comment",
            json!({"path": "a", "line": 1, "body": "", "suggestion": "x = 1"}),
        );
        assert_eq!(out["body"], json!("```suggestion\nx = 1\n```"));
    }

    /// Only the text-carrying actions offer an edit box; merging or resolving
    /// has nothing to rewrite.
    #[test]
    fn only_prose_actions_are_editable() {
        for name in ["post_pr_comment", "post_diff_comment", "reply_to_thread", "submit_review"] {
            assert!(carries_text(name), "{name} should be editable");
        }
        for name in ["merge_pr", "close_pr", "resolve_thread", "untrack_pr"] {
            assert!(!carries_text(name), "{name} has no text to edit");
        }
    }

    /// The system prompt is sent as one string; splitting it into labelled
    /// parts must not change a byte of what the model receives.
    #[test]
    fn system_parts_join_to_the_prompt_verbatim() {
        let parts = vec![
            SystemPart { label: "a", origin: "app", text: "one".into() },
            SystemPart { label: "b", origin: "app", text: "\n\ntwo".into() },
        ];
        let session = ChatSession {
            system_parts: parts,
            head_sha: String::new(),
            messages: Vec::new(),
            items: Vec::new(),
            pending: None,
            pending_results: Vec::new(),
            busy: false,
            usage: None,
            requests: 0,
            last_sent_est: 0,
            system_stale: false,
        };
        assert_eq!(session.system(), "one\n\ntwo");
    }
}

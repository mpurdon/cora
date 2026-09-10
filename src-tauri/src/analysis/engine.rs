use aws_sdk_bedrockruntime::types::{
    CachePointBlock, CachePointType, ContentBlock, ConversationRole, InferenceConfiguration,
    Message, ReasoningContentBlock, ReasoningTextBlock, StopReason, SystemContentBlock,
    TokenUsage, Tool, ToolConfiguration, ToolInputSchema, ToolResultBlock,
    ToolResultContentBlock, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::Document;
use chrono::Utc;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::analysis::metrics::{diff_metrics, FileMetrics};
use crate::analysis::tools::RepoTools;
use crate::devlog;
use crate::analysis::types::{
    events, AnalysisLevel, AnalysisProgress, AnalysisResult, AnalysisUsage, Assessment, C4Graph,
    C4NodeKind, ChangeStatus, ReviewPlanEntry, Severity, Significance, TraceStep,
};
use crate::error::{AppError, AppResult};
use crate::models::{Settings, TrackedPr};

const MAX_TURNS: usize = 30;
/// Turns the model may spend exploring before the research tools are
/// withdrawn and only submit_analysis remains. A run that reads on turn 29
/// with no idea it is out of road ends with nothing; the landing gives it
/// enough turns to write up what it has, plus the resubmit allowance.
const EXPLORE_TURNS: usize = 24;
/// How much of a PR description to carry into the kickoff. Long enough for a
/// real write-up, short enough that a template full of checkboxes and pasted
/// logs can't crowd out the diff.
const MAX_BODY_CHARS: usize = 4000;

/// The author's own account of the change: what it is for, what was left out
/// on purpose, which ticket it answers to. The diff cannot show intent, and
/// re-deriving it burns turns on a question already answered. Capped because a
/// description is unbounded — templates, checklists and pasted logs routinely
/// run longer than the code — and truncation is announced, so the model goes
/// and reads the rest rather than assuming it has all of it.
fn body_section(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    if body.chars().count() > MAX_BODY_CHARS {
        let head: String = body.chars().take(MAX_BODY_CHARS).collect();
        format!(
            "\n\n## PR description (author's own words, truncated — full text at the PR URL)\n{head}\n[…truncated]"
        )
    } else {
        format!("\n\n## PR description (author's own words)\n{body}")
    }
}
// The per-pass output-token ceilings live in Settings (arch_max_output_tokens,
// code_max_output_tokens) so they can be tuned to the configured model's cap.
// Submissions carry a whole graph + assessment in one tool call, so the
// architecture ceiling must stay generous — an 8k ceiling truncated them
// mid-JSON on big PRs (the "missing assessment" failures: the tail of the
// payload is what gets cut).

pub const SYSTEM_PROMPT: &str = r#"You are a principal engineer reviewing a pull request. Your job is to understand the change in relation to the whole system and explain it to a reviewer who lacks full context. Style preferences (naming, ternaries vs if, .reduce vs loops, formatting) are NOT your job — but code-level DEFECTS with real consequences are never "just style": wrong or referentially-unstable hook/memo dependencies, unhandled error or empty states, loosened tests, races, and leaks are material findings when you encounter them.

Priorities, strictly in this order:
1. EXTERNAL boundaries: effects on external systems — third-party APIs, other teams' services, queues/topics, webhooks, contracts, data leaving the system. These are always the most important findings.
2. SERVICE boundaries: effects that cross containers/services within the system — API shape changes, new dependencies between services, shared datastore access.
3. INTERNAL structure: module responsibilities, dependency direction, pattern consistency with the rest of the repo. Mention only when material. Pattern consistency includes REUSE: when the diff hand-rolls a component or utility, search the repo and its shared packages/design system for an existing equivalent before accepting it — duplicating an existing primitive is a material finding.

Writing style: the summary is a TLDR — two short sentences maximum, no mechanism walkthrough. Everything else goes in the detail field. A reviewer should absorb the summary in three seconds.

Risk calibration for routine bumps: version bumps of pinned dependencies, GitHub Actions, and toolchains (workflow files, lockfiles, version manifests) are low-risk housekeeping by default. Frame them accordingly — reserve elevated risk for major-version jumps, security-relevant dependencies, or bumps that change behavior the repo visibly relies on. Do not dress a patch-level action bump up as an architectural event.

Review plan: classify the changed files for the reviewer — critical (must be read carefully), important (real logic worth reading), or mechanical (renames, imports, fallout from the real change, config echoes, boilerplate following an established repo pattern). Order most-important-first. This drives the reviewer's reading order, so be honest about what's mechanical. Any changed file you leave out of the plan is given a metrics-derived default (mechanical when it adds no logic), so classify what you have read and judged — you do not need to read every file before you submit.

Significance is priced by BLAST RADIUS, not by layer or category. Ask of each file: who is affected beyond the acting user, what data can be lost or corrupted, is the effect reversible? "New endpoint", "API contract", "persists to a collection", "schema change" are categories, not risks — a self-scoped per-user preference toggle and a payment mutation both match those phrases, and only one is critical. Reserve critical for changes that can affect OTHER users' data, money, permissions/auth, irreversible operations, or contracts external teams/systems depend on. A cosmetic or self-scoped feature caps at important no matter how many endpoints, schemas, and handlers it spans. A handler that is entirely the repo's standard middleware boilerplate around a one-line effect is mechanical or important, never critical, regardless of being "a new write path".

One vertical slice shares one risk budget: when a feature adds the same low-stakes change across layers (schema + endpoint + BFF passthrough + hook + UI), rank the single most consequential file and let the rest be important/mechanical — do not price every layer as if each independently carried the risk. Repetitive pattern-following additions (wiring, node/edge registrations, tRPC/BFF passthroughs copying the adjacent procedure, simple predicate helpers) are mechanical, even when behavior-adjacent. When computed per-file diff metrics are provided, calibrate against them: a file with few added branch points and no new definitions is not critical unless it changes an interface others depend on.

Also evaluate the change against the AWS Well-Architected pillars (operational excellence, security, reliability, performance efficiency, cost optimization, sustainability). Report only MATERIAL findings — a missing retry on a new external call matters; a variable name does not.

Method: explore the repository first (README/docs, tree, targeted file reads and searches) until you understand the architecture well enough to place this change in it. Be economical — fetch what you need, not everything. Issue independent reads together in one turn: get_files and get_file_diff take lists, and every call in a turn runs in parallel, so one file per turn is the slow way. You have a fixed budget of exploration turns, stated in the kickoff; when it is spent the research tools are withdrawn and you must submit with what you have.

C4 graph rules:
- Build the graph at the requested C4 level, scoped to AFFECTED elements plus their immediate neighbors. Do not map the whole system.
- Keep the environment at EVERY level: the people and external systems that interact with the elements in view must appear as periphery nodes (change: unchanged) even at COMPONENT and CODE levels. A drilled diagram that loses its actors and externals loses the point of C4 — the reader must always see who uses this and what it talks to outside the boundary.
- Node ids must be deterministic and derived from stable names: "person:<role>", "ext:<system>", "system:<name>", "container:<name>", "component:<path>", "code:<path>#<class>". Lowercase, kebab-case.
- Node names are short display names (2-4 words); move file paths, package scopes, and qualifiers into `description` or `technology`. Edge labels are terse verb phrases (2-4 words, e.g. "reads validations"); put transport/format in `protocol`, never in the label. Long labels turn the diagram into spaghetti.
- Use `boundary` to nest: containers inside their system, components inside their container.
- Mark every node/edge with its change status. Edges that cross a boundary MUST set crossesBoundary=true.

When you are done exploring, you MUST call submit_analysis exactly once with the complete result. Include EVERY field in the schema — when a list has nothing to report, pass an empty array, never omit the field. Do not produce a final text answer."#;

/// Team knowledge no diff reveals, appended to every model-facing prompt.
/// The voice the app drafts in when the reviewer's settings name none.
/// Deliberately plain: a colleague leaving a note, not a tool filing a
/// report. Anything more particular is the reviewer's to write.
pub const DEFAULT_REVIEW_VOICE: &str = "Plain and direct, like a colleague leaving a note — not a tool filing a report. Short sentences. Say what is wrong and what to do, then stop. No preamble, no sign-off, no \"Consider...\" when you mean \"do this\". Contractions are fine. No emoji, no bold labels, no bullet lists for a one-point comment. Criticize the code, never the author, and be specific enough that they can act without asking a follow-up.";

/// The reviewer's voice, as a prompt section: everything the model writes
/// for the reviewer to post — comments, replies, review summaries, the code
/// pass's finding and suggestion text — should read as if they wrote it.
/// Falls back to [`DEFAULT_REVIEW_VOICE`] when the setting is empty.
pub(crate) fn voice_section(settings: &Settings) -> String {
    let v = settings.review_voice.trim();
    let v = if v.is_empty() { DEFAULT_REVIEW_VOICE } else { v };
    format!(
        "\n\n## The reviewer's voice\nAnything you write for the reviewer to post under their name — comments, replies, review summaries, finding and suggestion text — is read as THEIR words, so write it the way they write. Match this voice for wording, register, and rhythm, while keeping the substance rules above (what to say, how long, what to anchor to):\n{v}"
    )
}

/// `repo` ("owner/name") adds that repo's review instructions, when set,
/// after the global conventions block.
pub(crate) fn conventions_section(settings: &Settings, repo: &str) -> String {
    let c = settings.review_conventions.trim();
    let mut section = if c.is_empty() {
        String::new()
    } else {
        format!("\n\n## Team review conventions (from the reviewer's settings — treat as ground truth)\n{c}")
    };
    if let Some(extra) = settings.repo_review_instructions.get(repo) {
        let extra = extra.trim();
        if !extra.is_empty() {
            section.push_str(&format!(
                "\n\n## Review instructions for {repo} (from the reviewer's settings — treat as ground truth)\n{extra}"
            ));
        }
    }
    section
}

fn level_instructions(level: AnalysisLevel, focus: Option<&str>) -> String {
    match level {
        AnalysisLevel::Context => "Requested C4 level: CONTEXT + CONTAINER. Show the system in its environment (people, external systems) and the affected containers. One container per deployable or distinct-technology unit: a web app and its BFF/API layer are SEPARATE containers even when they live in one repo or directory — never merge different tech stacks into one container node. This is the default view — keep it at architecture altitude.".into(),
        AnalysisLevel::Component => format!(
            "Requested C4 level: COMPONENT. The user drilled into node '{}'. Anchor everything in the PR diff: deep-dive the components the diff touches, their responsibility shifts, dependency-direction violations, and pattern consistency. Include untouched components only as thin context (mark them unchanged), never as the subject. Carry over the wider environment: the people, external systems, and sibling containers this element serves or calls stay on the diagram as unchanged periphery nodes.",
            focus.unwrap_or("(unspecified)")
        ),
        AnalysisLevel::Code => format!(
            "Requested C4 level: CODE. The user drilled into component '{}'. Show the classes/modules the diff changes and what those changes do to coupling, interfaces, and cohesion. Untouched classes appear only when needed to explain an affected relationship. Keep the actors and external systems that reach this code on the diagram as unchanged periphery nodes. Still architectural framing; no style nits.",
            focus.unwrap_or("(unspecified)")
        ),
    }
}

/// Schema for the terminal submit_analysis tool — mirrors AnalysisResult's
/// graph + assessment (camelCase, as serde serializes them).
fn submit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "graph": {
                "type": "object",
                "description": "A JSON object with nodes and edges. Pass it as an object in the tool input — never as a JSON-encoded string. Keep it lean: omit any optional field you would fill with null, an empty string, or its default — technology and boundary when unknown, description when the name says it all, protocol when unremarkable, crossesBoundary when false, and change when unchanged.",
                "properties": {
                    "nodes": {"type": "array", "items": {"type": "object", "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "kind": {"type": "string", "enum": ["person", "external-system", "system", "container", "component", "code", "data-store", "queue"]},
                        "technology": {"type": "string"},
                        "description": {"type": "string"},
                        "boundary": {"type": "string"},
                        "change": {"type": "string", "enum": ["added", "modified", "removed", "affected", "unchanged"]}
                    }, "required": ["id", "name", "kind"]}},
                    "edges": {"type": "array", "items": {"type": "object", "properties": {
                        "id": {"type": "string"},
                        "source": {"type": "string"},
                        "target": {"type": "string"},
                        "label": {"type": "string"},
                        "protocol": {"type": "string"},
                        "crossesBoundary": {"type": "boolean"},
                        "change": {"type": "string", "enum": ["added", "modified", "removed", "affected", "unchanged"]}
                    }, "required": ["id", "source", "target", "label"]}}
                },
                "required": ["nodes", "edges"]
            },
            "assessment": {
                "type": "object",
                "description": "A JSON object with the fields below. Pass it as an object in the tool input — never as a JSON-encoded string.",
                "properties": {
                    "summary": {"type": "string", "description": "TLDR: at most TWO short sentences (under 40 words total). What the change is and the single most important thing about it. No mechanism detail here."},
                    "detail": {"type": "string", "description": "The fuller explanation: mechanism, how it flows through the system, why it's safe or risky. 1-2 paragraphs."},
                    "fit": {"type": "string", "enum": ["fits", "tension", "misfit"]},
                    "fitRationale": {"type": "string"},
                    "boundaryImpacts": {"type": "array", "items": {"type": "object", "properties": {
                        "kind": {"type": "string", "enum": ["external", "service", "internal"]},
                        "description": {"type": "string"},
                        "nodeIds": {"type": "array", "items": {"type": "string"}}
                    }, "required": ["kind", "description"]}},
                    "wellArchitected": {"type": "array", "items": {"type": "object", "properties": {
                        "pillar": {"type": "string", "enum": ["operational-excellence", "security", "reliability", "performance-efficiency", "cost-optimization", "sustainability"]},
                        "severity": {"type": "string", "enum": ["info", "low", "medium", "high"]},
                        "finding": {"type": "string"},
                        "recommendation": {"type": "string"},
                        "nodeIds": {"type": "array", "items": {"type": "string"}}
                    }, "required": ["pillar", "severity", "finding", "recommendation"]}},
                    "contextNotes": {"type": "array", "items": {"type": "string"}},
                    "reviewPlan": {"type": "array", "description": "EVERY changed file, ordered most-important-first, classified for the reviewer.", "items": {"type": "object", "properties": {
                        "path": {"type": "string"},
                        "significance": {"type": "string", "enum": ["critical", "important", "mechanical"], "description": "critical = can hurt other users' data, money, auth, or external contracts; important = real logic worth reading; mechanical = renames, fallout, config echoes, pattern-following boilerplate"},
                        "reason": {"type": "string", "description": "One short clause: why this file matters (or doesn't)"}
                    }, "required": ["path", "significance", "reason"]}}
                },
                "required": ["summary", "detail", "fit", "fitRationale", "boundaryImpacts", "wellArchitected", "contextNotes", "reviewPlan"]
            }
        },
        "required": ["graph", "assessment"]
    })
}

/// Bedrock runtime client from the user's profile/region/endpoint settings.
/// Shared by the analysis engine and the assistant chat.
pub(crate) async fn bedrock_client(settings: &Settings) -> aws_sdk_bedrockruntime::Client {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if !settings.aws_profile.is_empty() {
        loader = loader.profile_name(&settings.aws_profile);
    }
    if !settings.aws_region.is_empty() {
        loader = loader.region(aws_config::Region::new(settings.aws_region.clone()));
    }
    let sdk_config = loader.load().await;
    let mut conf = aws_sdk_bedrockruntime::config::Builder::from(&sdk_config);
    // Only apply a real URL — a pasted ARN here breaks dispatch cryptically.
    if settings.aws_endpoint_url.starts_with("http") {
        conf = conf.endpoint_url(&settings.aws_endpoint_url);
    }
    conf = conf.timeout_config(
        aws_sdk_bedrockruntime::config::timeout::TimeoutConfig::builder()
            .operation_timeout(std::time::Duration::from_secs(300))
            .build(),
    );
    aws_sdk_bedrockruntime::Client::from_conf(conf.build())
}

/// The Bedrock client every caller shares, rebuilt when the AWS settings
/// change — loading the config chain per call adds real latency. Dropped on
/// an expired-token failure: credentials rotate underneath a long-lived
/// client (a session token rewritten by an external tool, a multi-turn run
/// that crosses an SSO refresh), and the SDK keeps serving the stale ones
/// until the client is built again.
#[derive(Default)]
pub struct BedrockClients(
    tokio::sync::Mutex<Option<(String, aws_sdk_bedrockruntime::Client)>>,
);

pub(crate) async fn client_for(app: &AppHandle, settings: &Settings) -> aws_sdk_bedrockruntime::Client {
    let key = format!(
        "{}|{}|{}",
        settings.aws_profile, settings.aws_region, settings.aws_endpoint_url
    );
    let clients = app.state::<BedrockClients>();
    let mut cached = clients.0.lock().await;
    if let Some((k, client)) = cached.as_ref() {
        if *k == key {
            return client.clone();
        }
    }
    let client = bedrock_client(settings).await;
    *cached = Some((key, client.clone()));
    client
}

/// Forget the cached client so the next call reloads credentials.
pub(crate) async fn drop_client(app: &AppHandle) {
    *app.state::<BedrockClients>().0.lock().await = None;
}

/// A signed request refused because the credentials behind it have rotated
/// or lapsed since the client loaded them — worth one retry on a fresh
/// client before it is reported. Distinct from a missing or unresolvable
/// credential, which a rebuild cannot fix.
fn is_expired_token(code: Option<&str>, lower: &str) -> bool {
    matches!(code, Some("ExpiredTokenException" | "ExpiredToken"))
        || (lower.contains("security token") && lower.contains("expired"))
}

// -- serde_json::Value <-> aws_smithy_types::Document -------------------------

pub(crate) fn value_to_document(v: &Value) -> Document {
    match v {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Document::Number(aws_smithy_types::Number::NegInt(i))
            } else {
                Document::Number(aws_smithy_types::Number::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(a) => Document::Array(a.iter().map(value_to_document).collect()),
        Value::Object(o) => Document::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), value_to_document(v)))
                .collect(),
        ),
    }
}

pub(crate) fn document_to_value(d: &Document) -> Value {
    match d {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(n) => match n {
            aws_smithy_types::Number::PosInt(i) => json!(i),
            aws_smithy_types::Number::NegInt(i) => json!(i),
            aws_smithy_types::Number::Float(f) => json!(f),
        },
        Document::String(s) => Value::String(s.clone()),
        Document::Array(a) => Value::Array(a.iter().map(document_to_value).collect()),
        Document::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), document_to_value(v)))
                .collect(),
        ),
    }
}

// -----------------------------------------------------------------------------

pub(crate) fn cache_point() -> CachePointBlock {
    CachePointBlock::builder()
        .r#type(CachePointType::Default)
        .build()
        .expect("cache point")
}

/// Assemble a Converse tool config from spec triples, ending with a cache
/// checkpoint when enabled — tool schemas are the largest static prefix.
pub(crate) fn build_tool_config(
    specs: &[(&'static str, &'static str, Value)],
    with_cache: bool,
) -> AppResult<ToolConfiguration> {
    let mut builder = ToolConfiguration::builder();
    for (name, description, schema) in specs {
        builder = builder.tools(Tool::ToolSpec(
            ToolSpecification::builder()
                .name(*name)
                .description(*description)
                .input_schema(ToolInputSchema::Json(value_to_document(schema)))
                .build()
                .map_err(|e| AppError::Other(e.to_string()))?,
        ));
    }
    if with_cache {
        builder = builder.tools(Tool::CachePoint(cache_point()));
    }
    builder.build().map_err(|e| AppError::Other(e.to_string()))
}

fn analysis_specs() -> Vec<(&'static str, &'static str, Value)> {
    let mut specs = RepoTools::specs();
    specs.push((
        "submit_analysis",
        "Submit the final architecture analysis. Call exactly once, when your exploration is complete.",
        submit_schema(),
    ));
    specs
}

/// One Converse request under the shared failure policy: models or gateways
/// that reject cache points get a cache-free retry, and credential-shaped
/// errors carry the SSO hint. Used by the analysis run and the chat loop.
#[allow(clippy::too_many_arguments)]
/// True when a Bedrock error's (lowercased) text says the requested max_tokens
/// exceeds the model's hard output cap — distinct from a credential/SSO error,
/// which also contains "token" but none of these "over the limit" signals.
/// Covers the phrasings Bedrock/Anthropic emit: "…exceeds the model limit of
/// 8192", "…the maximum allowed number of output tokens", and the JSON-schema
/// form "maxTokens: 32000 is not less or equal to 8192".
fn is_max_tokens_over_cap(lower: &str) -> bool {
    lower.contains("token")
        && (lower.contains("exceed")
            || lower.contains("maximum allowed")
            || lower.contains("less or equal")
            || lower.contains("not less"))
}

/// Waits between our own retries of a transient Bedrock failure, in seconds.
/// The SDK already retries internally, but its default envelope is three
/// attempts inside ~3s — too tight to outlast the failures we actually see:
/// a proxy/gateway restart, or a resolver that negatively caches an NXDOMAIN
/// for tens of seconds. Total added wait is ~53s, comfortably inside the 5min
/// prompt-cache TTL, so a recovered run still lands on a warm prefix.
const TRANSIENT_BACKOFF_SECS: [u64; 4] = [2, 6, 15, 30];

/// A short reason when the failure is worth retrying as-is, `None` when it is
/// terminal. Classified off the error's shape rather than its text: a request
/// that never reached the model (DNS, connect, timeout, gateway 5xx) or was
/// shed under load (429) costs nothing to repeat, while validation and
/// credential failures repeat identically forever.
fn transient_reason<E>(e: &aws_sdk_bedrockruntime::error::SdkError<E>) -> Option<&'static str> {
    use aws_sdk_bedrockruntime::error::SdkError;
    match e {
        // A dispatch failure is anything that stopped the request leaving the
        // process — including a credential provider that couldn't mint
        // credentials (an expired SSO session). Only the network kinds are
        // worth resending; a credentials failure repeats identically and,
        // retried, buries the "run aws sso login" hint under a false outage.
        SdkError::DispatchFailure(d) => match d.as_connector_error() {
            Some(c) if c.is_io() => Some("upstream unreachable"),
            Some(c) if c.is_timeout() => Some("request timed out"),
            _ => None,
        },
        SdkError::TimeoutError(_) => Some("request timed out"),
        SdkError::ResponseError(_) => Some("malformed response"),
        SdkError::ServiceError(se) => match se.raw().status().as_u16() {
            429 => Some("throttled"),
            502 | 503 | 504 => Some("gateway error"),
            500 => Some("service error"),
            _ => None,
        },
        _ => None,
    }
}

/// One model turn, assembled from the response stream.
pub(crate) struct Turn {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Option<TokenUsage>,
}

impl Turn {
    pub fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }

    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }
}

/// Called with (tool name, the tool input streamed so far) on every tool-use
/// delta — how the write-up reaches the reader while it is still being
/// generated.
pub(crate) type StreamObserver<'a> = Option<&'a (dyn Fn(&str, &str) + Send + Sync)>;

/// What the stream is being reassembled into, per content block index.
enum Partial {
    Text(String),
    Reasoning {
        text: String,
        signature: Option<String>,
        redacted: Option<aws_smithy_types::Blob>,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
}

fn partial_slot(
    blocks: &mut Vec<(i32, Partial)>,
    idx: i32,
    make: impl FnOnce() -> Partial,
) -> &mut Partial {
    let pos = match blocks.iter().position(|(i, _)| *i == idx) {
        Some(pos) => pos,
        None => {
            blocks.push((idx, make()));
            blocks.len() - 1
        }
    };
    &mut blocks[pos].1
}

/// Drain a ConverseStream into one message. The stream is the only way to
/// see the write-up as it is written; everything else about the turn —
/// reasoning blocks passed back intact, tool input parsed once complete —
/// comes out the same as the unary call did.
async fn collect_stream(
    out: aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamOutput,
    observer: StreamObserver<'_>,
) -> Result<Turn, String> {
    use aws_sdk_bedrockruntime::types::{
        ContentBlockDelta, ContentBlockStart, ConverseStreamOutput as Ev,
        ReasoningContentBlockDelta,
    };
    let mut stream = out.stream;
    let mut blocks: Vec<(i32, Partial)> = Vec::new();
    let mut stop_reason = StopReason::EndTurn;
    let mut usage = None;
    loop {
        let ev = stream
            .recv()
            .await
            .map_err(|e| format!("{}", aws_smithy_types::error::display::DisplayErrorContext(&e)))?;
        let Some(ev) = ev else { break };
        match ev {
            Ev::ContentBlockStart(s) => {
                if let Some(ContentBlockStart::ToolUse(t)) = s.start {
                    let slot = partial_slot(&mut blocks, s.content_block_index, || {
                        Partial::Text(String::new())
                    });
                    *slot = Partial::ToolUse {
                        id: t.tool_use_id,
                        name: t.name,
                        input: String::new(),
                    };
                }
            }
            Ev::ContentBlockDelta(d) => {
                let idx = d.content_block_index;
                match d.delta {
                    Some(ContentBlockDelta::Text(t)) => {
                        if let Partial::Text(s) =
                            partial_slot(&mut blocks, idx, || Partial::Text(String::new()))
                        {
                            s.push_str(&t);
                        }
                    }
                    Some(ContentBlockDelta::ReasoningContent(r)) => {
                        if let Partial::Reasoning {
                            text,
                            signature,
                            redacted,
                        } = partial_slot(&mut blocks, idx, || Partial::Reasoning {
                            text: String::new(),
                            signature: None,
                            redacted: None,
                        }) {
                            match r {
                                ReasoningContentBlockDelta::Text(t) => text.push_str(&t),
                                ReasoningContentBlockDelta::Signature(s) => *signature = Some(s),
                                ReasoningContentBlockDelta::RedactedContent(b) => {
                                    *redacted = Some(b)
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(ContentBlockDelta::ToolUse(t)) => {
                        if let Partial::ToolUse { name, input, .. } =
                            partial_slot(&mut blocks, idx, || Partial::ToolUse {
                                id: String::new(),
                                name: String::new(),
                                input: String::new(),
                            })
                        {
                            input.push_str(&t.input);
                            if let Some(observe) = observer {
                                observe(name, input);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ev::MessageStop(m) => stop_reason = m.stop_reason,
            Ev::Metadata(m) => usage = m.usage,
            _ => {}
        }
    }
    blocks.sort_by_key(|(i, _)| *i);
    let mut content = Vec::with_capacity(blocks.len());
    for (_, p) in blocks {
        content.push(match p {
            Partial::Text(t) => ContentBlock::Text(t),
            Partial::Reasoning {
                redacted: Some(b), ..
            } => ContentBlock::ReasoningContent(ReasoningContentBlock::RedactedContent(b)),
            Partial::Reasoning {
                text, signature, ..
            } => ContentBlock::ReasoningContent(ReasoningContentBlock::ReasoningText(
                ReasoningTextBlock::builder()
                    .text(text)
                    .set_signature(signature)
                    .build()
                    .map_err(|e| e.to_string())?,
            )),
            Partial::ToolUse { id, name, input } => {
                let value = if input.trim().is_empty() {
                    json!({})
                } else {
                    parse_lenient(&input).unwrap_or(Value::String(input))
                };
                ContentBlock::ToolUse(
                    ToolUseBlock::builder()
                        .tool_use_id(id)
                        .name(name)
                        .input(value_to_document(&value))
                        .build()
                        .map_err(|e| e.to_string())?,
                )
            }
        });
    }
    let message = Message::builder()
        .role(ConversationRole::Assistant)
        .set_content(Some(content))
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Turn {
        message,
        stop_reason,
        usage,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn converse_once(
    app: &AppHandle,
    log_scope: &str,
    settings: &Settings,
    model_id: &str,
    system: &str,
    messages: &[Message],
    specs: &[(&'static str, &'static str, Value)],
    max_tokens: i32,
    use_cache: &mut bool,
    effort: &mut Option<String>,
    observer: StreamObserver<'_>,
) -> AppResult<Turn> {
    let sso_profile = settings.aws_profile.as_str();
    // Fail fast while the endpoint is known-down, so a queue of runs doesn't
    // each grind through the ladder to learn what the first one already found.
    let health = app.state::<crate::health::BedrockHealth>();
    if let crate::health::Admission::Blocked(wait) = health.admit() {
        devlog::warn(
            app,
            log_scope,
            format!("Bedrock unavailable — holding calls for {}s", wait.as_secs()),
        );
        return Err(AppError::Other(format!(
            "Bedrock is unreachable. Cora is holding requests for {}s, then trying one automatically.",
            wait.as_secs()
        )));
    }

    let mut transient_attempts = 0usize;
    let mut rebuilt_client = false;
    loop {
        // Fetched per attempt, so a rebuild below is seen by the retry.
        let client = client_for(app, settings).await;
        let config = build_tool_config(specs, *use_cache)?;
        let mut system_blocks = vec![SystemContentBlock::Text(system.to_string())];
        if *use_cache {
            system_blocks.push(SystemContentBlock::CachePoint(cache_point()));
        }
        let request_messages = if *use_cache {
            with_message_cache_points(messages)
        } else {
            messages.to_vec()
        };
        // Anthropic-specific request fields ride in the Converse escape
        // hatch. Effort is the only one sent; thinking stays at the model's
        // default (adaptive on the current tiers).
        let extra_fields = effort
            .as_deref()
            .map(|level| value_to_document(&json!({ "output_config": { "effort": level } })));
        let attempt = client
            .converse_stream()
            .model_id(model_id)
            .set_system(Some(system_blocks))
            .set_messages(Some(request_messages))
            .tool_config(config)
            .inference_config(InferenceConfiguration::builder().max_tokens(max_tokens).build())
            .set_additional_model_request_fields(extra_fields)
            .send()
            .await;
        match attempt {
            Ok(out) => match collect_stream(out, observer).await {
                Ok(turn) => {
                    health.record_success();
                    return Ok(turn);
                }
                // The connection opened and then broke. Nothing the model
                // wrote reached history, so a retry is as clean as a first
                // attempt — while the ladder has rungs.
                Err(e) => {
                    if let Some(&wait) = TRANSIENT_BACKOFF_SECS.get(transient_attempts) {
                        transient_attempts += 1;
                        devlog::warn(
                            app,
                            log_scope,
                            format!(
                                "response stream broke ({e}) — retrying in {wait}s ({transient_attempts}/{})",
                                TRANSIENT_BACKOFF_SECS.len()
                            ),
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        continue;
                    }
                    return Err(AppError::Other(format!("Bedrock response stream failed: {e}")));
                }
            },
            Err(e) => {
                let detail =
                    format!("{}", aws_smithy_types::error::display::DisplayErrorContext(&e));
                let lower = detail.to_lowercase();
                if *use_cache && (lower.contains("cache") || lower.contains("cachepoint")) {
                    devlog::warn(app, log_scope, "model rejected prompt caching — disabling");
                    *use_cache = false;
                    continue;
                }
                // A model without the effort parameter (an older tier behind
                // a profile) rejects the request outright; run at its default
                // rather than fail, and say so once.
                if effort.is_some() && (lower.contains("effort") || lower.contains("output_config")) {
                    devlog::warn(
                        app,
                        log_scope,
                        format!("model rejected effort={} — running at its default", effort.as_deref().unwrap_or("")),
                    );
                    *effort = None;
                    continue;
                }
                // Credentials rotated under the client — a session token
                // rewritten by an external tool, an SSO refresh crossed by a
                // long multi-turn run. The history is intact; only the
                // signature is stale. Reload once, then let it surface.
                let code = match &e {
                    aws_sdk_bedrockruntime::error::SdkError::ServiceError(se) => {
                        aws_sdk_bedrockruntime::error::ProvideErrorMetadata::code(se.err())
                    }
                    _ => None,
                };
                if !rebuilt_client && is_expired_token(code, &lower) {
                    rebuilt_client = true;
                    drop_client(app).await;
                    devlog::warn(
                        app,
                        log_scope,
                        "credentials expired mid-call — rebuilding the Bedrock client and retrying once",
                    );
                    continue;
                }
                // A blip between us and the model — the turn's accumulated
                // history and tool results are still valid, so wait it out and
                // resend rather than losing the whole run to a few seconds of
                // bad network. Only the classifier's terminal cases fall through.
                if let Some(reason) = transient_reason(&e) {
                    // A run alongside us has already proven the endpoint down.
                    // Stop climbing our own ladder to reach the same answer.
                    if health.is_open() {
                        devlog::warn(
                            app,
                            log_scope,
                            format!("{reason} — abandoning retries, Bedrock already held"),
                        );
                        return Err(AppError::Other(
                            "Bedrock is unreachable — Cora is holding requests and will retry one automatically.".into(),
                        ));
                    }
                    if let Some(&wait) = TRANSIENT_BACKOFF_SECS.get(transient_attempts) {
                        transient_attempts += 1;
                        devlog::warn(
                            app,
                            log_scope,
                            format!(
                                "{reason} — retrying in {wait}s ({transient_attempts}/{})",
                                TRANSIENT_BACKOFF_SECS.len()
                            ),
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        continue;
                    }
                    // Retrying did not clear it, so treat the endpoint as down
                    // and let everyone behind us skip straight to the message.
                    let cooldown = health.record_outage();
                    devlog::error(
                        app,
                        log_scope,
                        format!(
                            "{reason} — giving up after {transient_attempts} retries; holding Bedrock calls for {}s",
                            cooldown.as_secs()
                        ),
                    );
                    return Err(AppError::Other(format!(
                        "Bedrock {reason} — still failing after {transient_attempts} retries over {}s. Cora is holding further calls for {}s. Check that your endpoint or proxy is reachable. ({detail})",
                        TRANSIENT_BACKOFF_SECS.iter().sum::<u64>(),
                        cooldown.as_secs()
                    )));
                }
                // The requested max_tokens is above the model's hard output cap —
                // the likeliest failure now that the per-pass ceilings are
                // user-editable. Catch it before the SSO/credential hint below,
                // which also matches "token" and would mislead. The inference
                // profile hides the model, so we can't validate ahead of time;
                // name the exact setting to lower instead.
                if is_max_tokens_over_cap(&lower) {
                    let which = match log_scope {
                        "code-pass" => "the code-pass output-token ceiling",
                        "chat" => "the chat output limit (built-in)",
                        _ => "the architecture output-token ceiling",
                    };
                    devlog::warn(
                        app,
                        log_scope,
                        format!("max_tokens={max_tokens} exceeds the model's output cap"),
                    );
                    return Err(AppError::Other(format!(
                        "Bedrock rejected max_tokens={max_tokens}: it exceeds this model's hard output cap. Lower {which} in Settings → Bedrock to at or below the model's limit. ({detail})"
                    )));
                }
                let hint = if lower.contains("token")
                    || lower.contains("expired")
                    || lower.contains("credential")
                    || lower.contains("sso")
                {
                    format!(
                        " — your SSO session may have expired; run: aws sso login --profile {sso_profile}"
                    )
                } else {
                    String::new()
                };
                return Err(AppError::Other(format!("Bedrock: {detail}{hint}")));
            }
        }
    }
}

/// A tool_result content block, error-flagged when the call failed.
pub(crate) fn tool_result(id: &str, content: String, is_error: bool) -> AppResult<ContentBlock> {
    let mut trb = ToolResultBlock::builder()
        .tool_use_id(id)
        .content(ToolResultContentBlock::Text(content));
    if is_error {
        trb = trb.status(aws_sdk_bedrockruntime::types::ToolResultStatus::Error);
    }
    Ok(ContentBlock::ToolResult(
        trb.build().map_err(|e| AppError::Other(e.to_string()))?,
    ))
}

/// Rolling cache checkpoint: mark the latest user message so the next turn's
/// prefix (everything up to and including it) is a cache hit.
pub(crate) fn with_message_cache_points(messages: &[Message]) -> Vec<Message> {
    let last_user = messages.iter().rposition(|m| m.role() == &ConversationRole::User);
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if Some(i) == last_user {
                let mut content = m.content().to_vec();
                content.push(ContentBlock::CachePoint(cache_point()));
                Message::builder()
                    .role(m.role().clone())
                    .set_content(Some(content))
                    .build()
                    .expect("message rebuild")
            } else {
                m.clone()
            }
        })
        .collect()
}

/// Bedrock rejects replayed history when an assistant message's final
/// content block is reasoning ("cannot be `thinking`") — the shape a
/// max-tokens truncation leaves behind. Trim trailing reasoning blocks
/// before a message enters history.
pub(crate) fn strip_trailing_reasoning(message: Message) -> Message {
    let mut content = message.content().to_vec();
    while matches!(content.last(), Some(ContentBlock::ReasoningContent(_))) {
        content.pop();
    }
    if content.len() == message.content().len() {
        return message;
    }
    if content.is_empty() {
        content.push(ContentBlock::Text("…".into()));
    }
    Message::builder()
        .role(message.role().clone())
        .set_content(Some(content))
        .build()
        .unwrap_or(message)
}

tokio::task_local! {
    /// Set around a run made by the lab. Its progress goes to the lab's own
    /// channel and its draft is not shown, so the review screen never
    /// mistakes an experiment for an analysis of its own.
    pub(crate) static EXPERIMENT_SCOPE: bool;
}

fn in_experiment() -> bool {
    EXPERIMENT_SCOPE.try_with(|v| *v).unwrap_or(false)
}

fn progress(
    app: &AppHandle,
    pr_id: &str,
    level: AnalysisLevel,
    focus: &str,
    message: impl Into<String>,
) {
    let _ = app.emit(
        if in_experiment() {
            crate::experiments::events::PROGRESS
        } else {
            events::ANALYSIS_PROGRESS
        },
        AnalysisProgress {
            pr_id: pr_id.to_string(),
            level,
            focus: focus.to_string(),
            message: message.into(),
        },
    );
}

/// Emit progress to the UI and record it in the persistent trace.
#[allow(clippy::too_many_arguments)]
fn note(
    app: &AppHandle,
    trace: &mut Vec<TraceStep>,
    pr_id: &str,
    level: AnalysisLevel,
    focus: &str,
    kind: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    trace.push(TraceStep {
        at: Utc::now().to_rfc3339(),
        kind: kind.to_string(),
        message: message.clone(),
    });
    progress(app, pr_id, level, focus, message);
}

pub(crate) fn describe_tool_call(name: &str, input: &Value) -> String {
    match name {
        "get_pr_diff" => "reading the PR diff".into(),
        "get_file" => format!(
            "reading {}",
            input.get("path").and_then(Value::as_str).unwrap_or("a file")
        ),
        "list_tree" => format!(
            "exploring {}/",
            input.get("path").and_then(Value::as_str).unwrap_or("")
        ),
        "search_code" => format!(
            "searching \"{}\"",
            input.get("query").and_then(Value::as_str).unwrap_or("")
        ),
        "get_files" => format!(
            "reading {} files",
            input.get("paths").and_then(Value::as_array).map_or(0, Vec::len)
        ),
        "get_file_diff" => format!(
            "reading hunks for {} files",
            input.get("paths").and_then(Value::as_array).map_or(0, Vec::len)
        ),
        "get_readme_and_docs" => "reading README and docs".into(),
        "list_recent_prs" => "checking recent PRs".into(),
        "list_commits" => "listing the PR's commits".into(),
        "get_commit_diff" => format!(
            "reading commit {}",
            input
                .get("sha")
                .and_then(Value::as_str)
                .map(|s| &s[..s.len().min(10)])
                .unwrap_or("?")
        ),
        "submit_analysis" => "assembling the assessment".into(),
        "mark_files_viewed" => {
            let all = input.get("all").and_then(Value::as_bool).unwrap_or(false);
            let n = input.get("paths").and_then(Value::as_array).map_or(0, Vec::len);
            let unmark = input.get("viewed").and_then(Value::as_bool) == Some(false);
            let verb = if unmark { "unmarking" } else { "marking" };
            if all {
                format!("{verb} all files viewed")
            } else {
                format!("{verb} {n} file(s) viewed")
            }
        }
        other => other.to_string(),
    }
}

/// The per-file diff metrics, computed ahead of a run so the code pass can
/// be targeted from them while the architecture pass is still exploring.
pub async fn file_metrics(
    app: &AppHandle,
    settings: &Settings,
    token: &str,
    pr: &TrackedPr,
) -> Vec<(String, FileMetrics)> {
    match RepoTools::new(
        &settings.github_graphql_url,
        &pr.info.repo,
        pr.info.number,
        &pr.info.head_sha,
        token,
    ) {
        Ok(tools) => metrics_via(app, &tools).await,
        Err(e) => {
            devlog::warn(app, "analysis", format!("diff metrics unavailable: {e}"));
            Vec::new()
        }
    }
}

async fn metrics_via(app: &AppHandle, tools: &RepoTools) -> Vec<(String, FileMetrics)> {
    match tools.pr_diff_full().await {
        Ok(d) => diff_metrics(&d),
        Err(e) => {
            devlog::warn(app, "analysis", format!("diff metrics unavailable: {e}"));
            Vec::new()
        }
    }
}

/// The files the code pass should read: every non-mechanical file, the
/// most logic first. This stands in for the review plan, which the
/// architecture pass only produces at the end of its run.
pub fn code_focus_paths(metrics: &[(String, FileMetrics)]) -> Vec<String> {
    let mut logic: Vec<&(String, FileMetrics)> =
        metrics.iter().filter(|(_, m)| !m.is_mechanical()).collect();
    logic.sort_by_key(|(_, m)| {
        std::cmp::Reverse(m.added_branches * 2 + m.new_defs * 3 + m.max_nesting)
    });
    logic.into_iter().take(20).map(|(p, _)| p.clone()).collect()
}

/// A change with one or two files of real logic does not need a
/// forty-node graph and six pillar essays. The write-up is the slowest turn
/// of the run — the top model emitting JSON — so size it to the change.
fn scope_hint(level: AnalysisLevel, metrics: &[(String, FileMetrics)]) -> String {
    if level != AnalysisLevel::Context || metrics.is_empty() {
        return String::new();
    }
    let logic = metrics.iter().filter(|(_, m)| !m.is_mechanical()).count();
    if logic > 2 {
        return String::new();
    }
    format!(
        "\n\nScope: only {logic} of the {} changed files carry logic. Size the write-up to the change: the graph keeps every container that calls or is called by the changed code and the external systems that feel the effect, and leaves out the rest of the system; pillar findings are one sentence each and only where there is something real to say; detail is one paragraph.",
        metrics.len()
    )
}

/// An earlier context-level result for a previous head of the same PR. A
/// push does not start the review over: the run updates this instead.
pub struct PriorAnalysis {
    pub head_sha: String,
    /// `{graph, assessment}` of the earlier result, as JSON.
    pub context: String,
}

/// What the caller already knows before the run starts.
#[derive(Default)]
pub struct RunExtras {
    pub metrics: Option<Vec<(String, FileMetrics)>>,
    pub prior: Option<PriorAnalysis>,
}

/// Files with the most added logic ride in the kickoff — every trace opened
/// by reading them anyway.
const PRELOAD_FILES: usize = 3;
const PRELOAD_CHARS: usize = 80_000;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &AppHandle,
    settings: &Settings,
    token: &str,
    pr: &TrackedPr,
    level: AnalysisLevel,
    focus_node_id: Option<String>,
    parent_context: Option<String>,
    extras: RunExtras,
) -> AppResult<AnalysisResult> {
    let pr_id = pr.info.id.clone();
    let focus_key = focus_node_id.clone().unwrap_or_default();
    progress(app, &pr_id, level, &focus_key, "connecting to Bedrock");
    devlog::info(
        app,
        "analysis",
        format!(
            "starting {} analysis for {}#{} (head {})",
            level.as_str(),
            pr.info.repo,
            pr.info.number,
            &pr.info.head_sha[..8.min(pr.info.head_sha.len())]
        ),
    );

    let mut system_prompt = if settings.custom_system_prompt.trim().is_empty() {
        SYSTEM_PROMPT.to_string()
    } else {
        devlog::warn(app, "analysis", "using CUSTOM system prompt from developer settings");
        settings.custom_system_prompt.clone()
    };
    system_prompt.push_str(&conventions_section(settings, &pr.info.repo));

    let tools = RepoTools::new(
        &settings.github_graphql_url,
        &pr.info.repo,
        pr.info.number,
        &pr.info.head_sha,
        token,
    )?;

    // Objective per-file signals: ground the model's review-plan calls in
    // measured complexity, and backstop them after submission. Context level
    // only — drills don't own the review plan.
    let file_metrics = match extras.metrics {
        Some(m) => m,
        None if level == AnalysisLevel::Context => metrics_via(app, &tools).await,
        None => Vec::new(),
    };
    // Every file, not a prefix: a cut-off list is exactly the signal a big PR
    // needs most, and 400 lines of it is still under the cost of one file read.
    let metrics_section = if file_metrics.is_empty() {
        String::new()
    } else {
        let mut s = String::from(
            "\n\nComputed per-file diff metrics for EVERY changed file — calibrate the review plan against these. Files with no added branches or definitions are wiring/data, not critical, unless they change a contract:\n",
        );
        for (path, m) in file_metrics.iter().take(MAX_METRICS_ROWS) {
            s.push_str(&format!("- {path}: {}\n", m.summary()));
        }
        if file_metrics.len() > MAX_METRICS_ROWS {
            s.push_str(&format!("… and {} more files\n", file_metrics.len() - MAX_METRICS_ROWS));
        }
        s
    };

    let mut trace: Vec<TraceStep> = Vec::new();

    // Drill-downs analyze code, not system-wide architecture — the faster
    // tier fits and roughly halves per-turn latency. A routine context-level
    // run (a bump, a rename sweep) goes there too when the setting allows:
    // its two-sentence summary and review plan don't need the top tier.
    let routine = level == AnalysisLevel::Context
        && settings.route_routine_prs_to_drill_model
        && settings.drill_model() != settings.bedrock_model_id
        && is_routine(&file_metrics);
    let model_id = if level != AnalysisLevel::Context || routine {
        settings.drill_model().to_string()
    } else {
        settings.bedrock_model_id.clone()
    };
    if routine {
        note(
            app,
            &mut trace,
            &pr_id,
            level,
            &focus_key,
            "status",
            "routine change by the diff metrics — running on the drill model",
        );
    }
    devlog::debug(app, "bedrock", format!("model for {} level: {model_id}", level.as_str()));
    let run_started = std::time::Instant::now();
    let on_drill_tier = model_id == settings.drill_model() && model_id != settings.bedrock_model_id;
    let mut effort: Option<String> = crate::models::effort_level(if on_drill_tier {
        &settings.bedrock_effort_drill
    } else {
        &settings.bedrock_effort_arch
    })
    .map(str::to_string);
    if let Some(level_name) = &effort {
        note(app, &mut trace, &pr_id, level, &focus_key, "status", format!("effort: {level_name}"));
    }

    // A push moves the head; it does not change the system. When an earlier
    // context result exists, the run gets it plus the diff between the two
    // heads and verifies only what moved. If the old head is gone (a
    // rebase), the compare fails and the run falls back to a full read.
    let prior_section = match &extras.prior {
        Some(p) if level == AnalysisLevel::Context => match tools.compare_diff(&p.head_sha).await {
            Ok(delta) => {
                let old = &p.head_sha[..8.min(p.head_sha.len())];
                let new = &pr.info.head_sha[..8.min(pr.info.head_sha.len())];
                note(
                    app,
                    &mut trace,
                    &pr_id,
                    level,
                    &focus_key,
                    "status",
                    format!("updating the previous analysis (head {old} → {new})"),
                );
                format!(
                    "\n\n## Previous analysis of this PR (head {old}) — UPDATE it, do not rebuild\nA context-level analysis of this PR already exists for an earlier head. Since then the branch moved to {new}; the diff between the two heads follows. Treat the previous result as your system map: keep node ids and names stable, re-verify only what the interim changes touch, and revise the assessment where they change the picture. Your submit_analysis call must still include the COMPLETE graph and assessment.\n<previous_analysis>\n{}\n</previous_analysis>\n\n## Changes since the previous analysis ({old}..{new})\n{}",
                    p.context,
                    crate::analysis::tools::diff_view(&delta)
                )
            }
            Err(e) => {
                devlog::warn(app, "analysis", format!("compare against previous head unavailable ({e}) — full run"));
                String::new()
            }
        },
        _ => String::new(),
    };
    let incremental = !prior_section.is_empty();

    // A diff too big to show whole gets a scout pre-read on the cheap tier:
    // a map of feature slices and boundary flags, so the main model reads
    // the five files that matter instead of thirty. Best-effort — without it
    // the run still has the index. An incremental run has a map already.
    let scout_section = if level == AnalysisLevel::Context
        && !incremental
        && !settings.bedrock_scout_model_id.is_empty()
        && tools.diff_oversized().await.unwrap_or(false)
    {
        note(app, &mut trace, &pr_id, level, &focus_key, "status", "diff is oversized — scouting it first");
        match tools.pr_diff_full().await {
            Ok(full) => match crate::analysis::scout::report(app, settings, pr, &full).await {
                Ok(report) => {
                    note(app, &mut trace, &pr_id, level, &focus_key, "status", "scout report ready");
                    report
                }
                Err(e) => {
                    devlog::warn(app, "scout", format!("scout pre-read failed: {e}"));
                    note(app, &mut trace, &pr_id, level, &focus_key, "status", "scout pre-read failed — continuing without it");
                    String::new()
                }
            },
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    let body_section = body_section(&pr.info.body);

    // Every run opened with the same fetches — the diff, and for the
    // context pass the README — so they ride in the kickoff instead of
    // costing the first turn or two of the top model. The diff is the
    // model's view (indexed when oversized) and was cached by the metrics
    // pass; the two requests here are cheap and run together.
    let (diff_res, readme_res) = futures::future::join(tools.pr_diff(), async {
        if level == AnalysisLevel::Context {
            Some(tools.readme_and_docs().await)
        } else {
            None
        }
    })
    .await;
    let kickoff_diff = match diff_res {
        Ok(d) => format!("\n\n## PR diff (already fetched — do NOT call get_pr_diff)\n{d}"),
        Err(e) => {
            devlog::warn(app, "analysis", format!("kickoff diff unavailable: {e}"));
            String::new()
        }
    };
    let kickoff_docs = match readme_res {
        Some(Ok(r)) => {
            format!("\n\n## README and docs (already fetched — do NOT call get_readme_and_docs)\n{r}")
        }
        Some(Err(e)) => {
            devlog::warn(app, "analysis", format!("kickoff README unavailable: {e}"));
            String::new()
        }
        None => String::new(),
    };
    // The files with the most added logic, at head. A file the PR removes
    // 404s and is simply left out.
    let preload_section = if level == AnalysisLevel::Context && !incremental {
        let paths: Vec<String> = code_focus_paths(&file_metrics)
            .into_iter()
            .take(PRELOAD_FILES)
            .collect();
        let fetched = futures::future::join_all(paths.iter().map(|p| tools.file(p, None))).await;
        let mut section = String::new();
        let mut used = 0usize;
        for (path, result) in paths.iter().zip(fetched) {
            match result {
                Ok(text) => {
                    if used + text.len() > PRELOAD_CHARS {
                        break;
                    }
                    used += text.len();
                    section.push_str(&format!("\n\n### {path}\n{text}"));
                }
                Err(e) => devlog::debug(app, "analysis", format!("preload of {path} skipped: {e}")),
            }
        }
        if section.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n## Current contents of the files with the most added logic (at head, already fetched — do NOT call get_file or get_files for these){section}"
            )
        }
    } else {
        String::new()
    };

    // The parent's assessment doubles as a salvage value: a drill whose
    // submission has a good graph but a broken/missing assessment ships with
    // this instead of sinking the whole run.
    let parent_assessment: Option<Assessment> = parent_context
        .as_ref()
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
        .and_then(|v| v.get("assessment").cloned())
        .and_then(|a| serde_json::from_value(a).ok());

    // Drilled runs inherit the higher-level result so the model doesn't
    // re-derive (and re-fetch) the system map it already built.
    let parent_section = match &parent_context {
        Some(json) => format!(
            "\n\nA higher-level analysis of this PR was already completed. Its result is below — trust it as your system map, do NOT re-explore what it already covers, and keep your node ids consistent with it. Focus your exploration budget on the drill target only. Your submit_analysis call must still include BOTH a complete graph and a complete assessment for the requested level — never omit or abbreviate either because this higher-level result exists.\n<previous_analysis>\n{json}\n</previous_analysis>"
        ),
        None => String::new(),
    };

    let closing = if kickoff_diff.is_empty() {
        "Start by getting the diff and whatever repository context you need."
    } else if incremental {
        "The previous analysis, the changes since it, the full diff, and the README are included below — verify only what changed, then submit."
    } else if kickoff_docs.is_empty() {
        "The diff is included below — read only the extra repository context you still need, then submit."
    } else {
        "The diff and the README are included below — read only the extra repository context you still need (tree, targeted files, searches), then submit."
    };
    let scope_section = scope_hint(level, &file_metrics);
    let budget = format!(
        "Budget: {EXPLORE_TURNS} exploration turns, each of which may carry many tool calls in parallel. After that the research tools are withdrawn and only submit_analysis remains, so plan to submit before then. When you submit, write the assessment before the graph, summary first — the reader sees it as it streams."
    );
    let kickoff = format!(
        "Analyze this pull request.\n\nRepository: {}\nPR #{}: {}\nAuthor: {}\nBranch head: {}\nStats: +{} −{} across {} files\nURL: {}{body_section}\n\n{}{}{}{}{prior_section}\n\n{budget}{scope_section}\n\n{closing}{kickoff_diff}{kickoff_docs}{preload_section}",
        pr.info.repo,
        pr.info.number,
        pr.info.title,
        pr.info.author,
        pr.info.head_sha,
        pr.info.additions,
        pr.info.deletions,
        pr.info.changed_files,
        pr.info.url,
        level_instructions(level, focus_node_id.as_deref()),
        metrics_section,
        scout_section,
        parent_section,
    );

    let mut messages = vec![Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(kickoff))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?];

    // Prompt caching slashes per-turn input cost/latency; some models or
    // gateways reject cache points, so fall back cleanly on first complaint.
    let mut use_cache = true;
    let mut nudged = false;
    let mut resubmits = 0u32;
    let (mut total_in, mut total_out) = (0i32, 0i32);
    let (mut total_cache_read, mut total_cache_write) = (0i32, 0i32);
    note(app, &mut trace, &pr_id, level, &focus_key, "status", "starting exploration");

    // The write-up is the run's longest turn. As submit_analysis streams in,
    // the summary and detail written so far go to the reader, so the verdict
    // lands while the graph is still being generated.
    let draft_gate = std::sync::Mutex::new(None::<std::time::Instant>);
    let draft_started = std::sync::atomic::AtomicBool::new(false);
    let observer = |name: &str, partial: &str| {
        if name != "submit_analysis" {
            return;
        }
        if !draft_started.swap(true, std::sync::atomic::Ordering::Relaxed) {
            progress(app, &pr_id, level, &focus_key, "writing the assessment");
        }
        let mut last = draft_gate.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(600)) {
            return;
        }
        *last = Some(std::time::Instant::now());
        let summary = json_string_field(partial, "summary");
        if summary.is_none() || in_experiment() {
            return;
        }
        let _ = app.emit(
            events::ANALYSIS_DRAFT,
            crate::analysis::types::AnalysisDraft {
                pr_id: pr_id.clone(),
                level,
                focus: focus_key.clone(),
                summary,
                detail: json_string_field(partial, "detail"),
            },
        );
    };

    let explore_specs = analysis_specs();
    // Once the budget is spent the model sees one tool. Withdrawing the rest
    // is what makes "submit now" stick — a reminder alone competes with the
    // pull of one more file.
    let submit_specs: Vec<(&'static str, &'static str, Value)> = explore_specs
        .iter()
        .filter(|(name, _, _)| *name == "submit_analysis")
        .cloned()
        .collect();
    for turn in 0..MAX_TURNS {
        let exploring = turn < EXPLORE_TURNS;
        let specs = if exploring { &explore_specs } else { &submit_specs };
        let started = std::time::Instant::now();
        let resp = converse_once(
            app,
            "bedrock",
            settings,
            &model_id,
            &system_prompt,
            &messages,
            specs,
            settings.arch_max_output_tokens as i32,
            &mut use_cache,
            &mut effort,
            Some(&observer),
        )
        .await?;

        if let Some(usage) = resp.usage() {
            total_in += usage.input_tokens();
            total_out += usage.output_tokens();
            total_cache_read += usage.cache_read_input_tokens().unwrap_or(0);
            total_cache_write += usage.cache_write_input_tokens().unwrap_or(0);
            crate::usage::record(app, &pr, "analysis", &model_id, usage);
            devlog::debug(
                app,
                "bedrock",
                format!(
                    "turn {turn}: {}ms, {} in / {} out tokens (total {} / {})",
                    started.elapsed().as_millis(),
                    usage.input_tokens(),
                    usage.output_tokens(),
                    total_in,
                    total_out,
                ),
            );
        }

        let message = resp.message.clone();

        let mut tool_calls: Vec<(String, String, Value)> = Vec::new();
        let mut submitted: Option<(String, Value)> = None;

        for block in message.content() {
            match block {
                ContentBlock::Text(t) => {
                    let snippet: String = t.chars().take(400).collect();
                    if !snippet.trim().is_empty() {
                        note(app, &mut trace, &pr_id, level, &focus_key, "thought", snippet);
                    }
                }
                ContentBlock::ToolUse(tu) => {
                    let input = document_to_value(tu.input());
                    note(
                        app,
                        &mut trace,
                        &pr_id,
                        level,
                        &focus_key,
                        "tool",
                        describe_tool_call(tu.name(), &input),
                    );
                    if tu.name() == "submit_analysis" {
                        submitted = Some((tu.tool_use_id().to_string(), input));
                    } else {
                        tool_calls.push((tu.tool_use_id().to_string(), tu.name().to_string(), input));
                    }
                }
                _ => {}
            }
        }

        // Execute the turn's tool calls concurrently — multi-file reads are
        // the common case and serial awaits were pure added latency.
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        if !tool_calls.is_empty() && !exploring {
            // The tools were withdrawn from the spec; a call that arrives
            // anyway gets a refusal, not a read.
            for (id, _, _) in &tool_calls {
                tool_results.push(tool_result(
                    id,
                    "exploration budget is spent — call submit_analysis with what you have".into(),
                    true,
                )?);
            }
        } else if !tool_calls.is_empty() {
            let executed = futures::future::join_all(tool_calls.iter().map(
                |(_, name, input)| {
                    let tools = &tools;
                    async move { tools.execute(name, input).await }
                },
            ))
            .await;
            for ((id, name, _), result) in tool_calls.iter().zip(executed) {
                let (content, is_error) = match result {
                    Ok(text) => (text, false),
                    Err(e) => {
                        devlog::warn(app, "analysis", format!("tool {name} failed: {e}"));
                        (format!("error: {e}"), true)
                    }
                };
                devlog::debug(app, "analysis", format!("tool {name} → {} chars", content.len()));
                tool_results.push(tool_result(id, content, is_error)?);
            }
        }
        // The last exploration turn's results carry the landing notice, so
        // the model learns the tools are gone from the same message that
        // answers its final reads.
        let landing = exploring && turn + 1 == EXPLORE_TURNS && submitted.is_none();
        if landing {
            note(
                app,
                &mut trace,
                &pr_id,
                level,
                &focus_key,
                "status",
                "exploration budget spent — asking for the submission",
            );
            tool_results.push(ContentBlock::Text(LANDING_NOTICE.into()));
        }

        if let Some((submit_id, mut payload)) = submitted {
            let coerced = sanitize_payload(&mut payload);
            if !coerced.is_empty() {
                devlog::debug(
                    app,
                    "analysis",
                    format!("submission coerced into schema shape: {}", coerced.join(", ")),
                );
            }
            match parse_payload(&payload) {
                Ok((graph, mut assessment)) => {
                    let grounded = ground_review_plan(&mut assessment, &file_metrics);
                    if !grounded.is_empty() {
                        devlog::info(
                            app,
                            "analysis",
                            format!("review plan grounded by metrics: {}", grounded.join(", ")),
                        );
                    }
                    devlog::info(
                        app,
                        "analysis",
                        format!(
                            "complete after {} turns — {total_in} in / {total_out} out tokens",
                            turn + 1
                        ),
                    );
                    note(app, &mut trace, &pr_id, level, &focus_key, "status", "assessment assembled");
                    let usage = AnalysisUsage {
                        input_tokens: total_in as i64,
                        output_tokens: total_out as i64,
                        cache_read_tokens: total_cache_read as i64,
                        cache_write_tokens: total_cache_write as i64,
                        turns: (turn + 1) as i64,
                        elapsed_ms: run_started.elapsed().as_millis() as i64,
                        effort: effort.clone(),
                    };
                    return Ok(build_result(pr, level, focus_node_id, graph, assessment, trace, usage));
                }
                Err(e) if resubmits < 2 => {
                    // Don't waste the whole run on a malformed submission —
                    // bounce the validation error back and let it fix itself.
                    resubmits += 1;
                    let truncated = matches!(resp.stop_reason(), StopReason::MaxTokens);
                    let keys = payload
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_else(|| "(non-object)".into());
                    // A stringified sub-struct even the lenient parser could
                    // not rescue: keep its head in the log so the next repair
                    // is informed, and tell the model exactly what to change.
                    let stringified: Vec<&str> = ["graph", "assessment"]
                        .into_iter()
                        .filter(|k| payload.get(k).is_some_and(Value::is_string))
                        .collect();
                    for k in &stringified {
                        let head: String =
                            payload[k].as_str().unwrap_or_default().chars().take(300).collect();
                        devlog::warn(app, "analysis", format!("{k} arrived as an unparseable string: {head}"));
                    }
                    devlog::warn(
                        app,
                        "analysis",
                        format!(
                            "invalid submission ({e}) — keys: [{keys}]{} — asking model to resubmit",
                            if truncated { ", output truncated at max tokens" } else { "" }
                        ),
                    );
                    note(
                        app,
                        &mut trace,
                        &pr_id,
                        level,
                        &focus_key,
                        "status",
                        format!("submission incomplete ({e}) — retrying"),
                    );
                    let feedback = if truncated {
                        format!(
                            "Submission rejected: {e}. Your submission was CUT OFF by the output-token limit. Resubmit the complete payload but much tighter: descriptions and reasons in a few words each, no prose, and only the nodes/files that matter. Every required field must still be present."
                        )
                    } else if !stringified.is_empty() {
                        format!(
                            "Submission rejected: {e}. The field(s) {} arrived as JSON-encoded STRINGS. Call submit_analysis again passing them as real JSON objects in the tool input — never serialize a field to text.",
                            stringified.join(", ")
                        )
                    } else {
                        format!(
                            "Submission rejected: {e}. Call submit_analysis again with EVERY field present (empty arrays where nothing applies)."
                        )
                    };
                    tool_results.push(tool_result(&submit_id, feedback, true)?);
                }
                Err(e) => {
                    // Retries exhausted. A drill's product is the graph — if
                    // that part parsed, ship it with the parent's assessment
                    // rather than sinking the run on the missing half.
                    let salvaged_graph = payload
                        .get("graph")
                        .cloned()
                        .and_then(|g| serde_json::from_value::<C4Graph>(g).ok());
                    if let (Some(graph), Some(assessment)) =
                        (salvaged_graph, parent_assessment.clone())
                    {
                        devlog::warn(
                            app,
                            "analysis",
                            format!("salvaging drill submission ({e}): graph kept, assessment inherited"),
                        );
                        note(
                            app,
                            &mut trace,
                            &pr_id,
                            level,
                            &focus_key,
                            "status",
                            "assessment omitted by the model — inherited from the parent level",
                        );
                        let usage = AnalysisUsage {
                            input_tokens: total_in as i64,
                            output_tokens: total_out as i64,
                            cache_read_tokens: total_cache_read as i64,
                            cache_write_tokens: total_cache_write as i64,
                            turns: (turn + 1) as i64,
                            elapsed_ms: run_started.elapsed().as_millis() as i64,
                            effort: effort.clone(),
                        };
                        return Ok(build_result(
                            pr,
                            level,
                            focus_node_id,
                            graph,
                            assessment,
                            trace,
                            usage,
                        ));
                    }
                    return Err(e);
                }
            }
        }

        messages.push(strip_trailing_reasoning(message));

        if !tool_results.is_empty() {
            messages.push(
                Message::builder()
                    .role(ConversationRole::User)
                    .set_content(Some(tool_results))
                    .build()
                    .map_err(|e| AppError::Other(e.to_string()))?,
            );
            continue;
        }

        // No tool calls and no submission: the conversation now ends on an
        // assistant message, which some Bedrock models reject as a prefill
        // ("the conversation must end with a user message"). We must append a
        // user turn before looping. A MaxTokens truncation cut the turn short
        // before a tool call landed — tell it to continue. A clean stop
        // without submission gets one reminder before we give up.
        let nudge = if matches!(resp.stop_reason(), StopReason::MaxTokens) {
            "Your previous response was cut off by the output-token limit. Continue, and call submit_analysis with your complete result."
        } else {
            if nudged {
                return Err(AppError::Other(
                    "analysis ended without submit_analysis".into(),
                ));
            }
            nudged = true;
            if exploring {
                "Call submit_analysis now with your complete result."
            } else {
                LANDING_NOTICE
            }
        };
        messages.push(
            Message::builder()
                .role(ConversationRole::User)
                .content(ContentBlock::Text(nudge.into()))
                .build()
                .map_err(|e| AppError::Other(e.to_string()))?,
        );
    }

    Err(AppError::Other(format!(
        "analysis did not complete within {MAX_TURNS} turns ({EXPLORE_TURNS} exploring, {} to submit)",
        MAX_TURNS - EXPLORE_TURNS
    )))
}

/// Told to the model once its exploration turns are gone, and again if it
/// idles afterwards.
const LANDING_NOTICE: &str = "Exploration budget is spent and the research tools have been withdrawn. Call submit_analysis now with your complete result — every field present, empty arrays where nothing applies. Files you did not read get a metrics-derived default in the review plan; do not invent classifications for them.";

/// Rows of per-file metrics the kickoff will carry. Above this a PR is a
/// generated-code dump, and the index inside get_pr_diff still has them all.
const MAX_METRICS_ROWS: usize = 400;

/// A PR whose diff carries no real logic: every file mechanical, or a
/// handful of files with almost none. The judgement the drill tier is
/// already trusted with at the code level.
fn is_routine(metrics: &[(String, FileMetrics)]) -> bool {
    if metrics.is_empty() {
        return false;
    }
    if metrics.iter().all(|(_, m)| m.is_mechanical()) {
        return true;
    }
    let branches: i64 = metrics.iter().map(|(_, m)| m.added_branches).sum();
    let defs: i64 = metrics.iter().map(|(_, m)| m.new_defs).sum();
    metrics.len() <= 4 && branches <= 4 && defs <= 2
}

// -- second stage: line-anchored code findings --------------------------------

const CODE_PASS_PROMPT: &str = r#"You are doing the code-level pass of a pull request review. An architecture review runs separately — do not repeat it. Hunt for exactly two classes of finding:

1. DEFECTS — code-level problems with real consequences: wrong or referentially-unstable hook/memo/effect dependencies, unhandled error or empty states, loosened or weakened tests, race conditions, resource leaks, incorrect boundary conditions, state machines that can skip states.
2. REUSE — new code that re-implements something that already exists in this repository or its shared packages/design system. Use search_code and list_tree to check before accepting new utilities or UI primitives; report the existing equivalent by name/path. The most common real-world miss: a component defines a local formatting/derivation helper (formatX, getY, toZ) that a shared utility module (lib/utils, format.ts, *-utils.ts, helpers) or the design system already provides — when the diff ADDS such a helper or inlines that logic, spend a search on the helper's concept (e.g. "formatRating") before accepting it.

Explicitly NOT your job: style preferences — naming, ternaries vs if, .reduce vs loops, enum tastes, formatting. If a finding is a preference rather than a consequence, drop it.

Method: get the diff, then read only what you need — the changed hunks' surrounding context via get_file, and targeted search_code for reuse checks. Be economical. search_code is heavily rate-limited: budget 3-4 well-chosen queries per review, issued one at a time — prefer one broad query over several narrow ones.

Writing style: finding and suggestion text gets posted nearly verbatim as review comments the PR AUTHOR reads. One sentence each — finding names the defect and its consequence; suggestion is one imperative sentence naming the fix. No walls of text, no lecture, no restating the code back at them. If the mechanism needs explaining, one tight clause, not a paragraph.

When done, call submit_code_findings exactly once. Anchor each finding to a path from the diff and the new-side line number of the changed line it concerns. An empty findings list is a valid, good result — never invent findings to fill space."#;

fn code_findings_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "findings": {"type": "array", "items": {"type": "object", "properties": {
                "path": {"type": "string", "description": "File path exactly as it appears in the diff"},
                "line": {"type": "integer", "description": "New-side line number of the changed line this concerns"},
                "severity": {"type": "string", "enum": ["info", "low", "medium", "high"]},
                "kind": {"type": "string", "enum": ["defect", "reuse", "convention"]},
                "finding": {"type": "string", "description": "What is wrong and why it matters — 1-2 sentences"},
                "suggestion": {"type": "string", "description": "Concrete fix or the existing code to reuse"}
            }, "required": ["path", "severity", "kind", "finding", "suggestion"]}}
        },
        "required": ["findings"]
    })
}

fn parse_code_findings(payload: &mut Value) -> AppResult<Vec<crate::analysis::types::CodeFinding>> {
    let mut unwrap_notes = Vec::new();
    unwrap_stringified(payload, "findings", &mut unwrap_notes);
    camelize_keys(payload);
    const SEVERITY: &[(&str, &str)] = &[
        ("critical", "high"),
        ("blocker", "high"),
        ("warning", "medium"),
        ("moderate", "medium"),
        ("minor", "low"),
        ("informational", "info"),
    ];
    const KIND: &[(&str, &str)] = &[
        ("bug", "defect"),
        ("correctness", "defect"),
        ("duplication", "reuse"),
        ("duplicate", "reuse"),
        ("standard", "convention"),
        ("conventions", "convention"),
    ];
    let mut notes = Vec::new();
    if let Some(arr) = payload.get_mut("findings").and_then(Value::as_array_mut) {
        for f in arr {
            normalize_enum(f, "severity", SEVERITY, &mut notes);
            coerce_unknown::<Severity>(f, "severity", "low", &mut notes);
            normalize_enum(f, "kind", KIND, &mut notes);
            coerce_unknown::<crate::analysis::types::CodeFindingKind>(
                f, "kind", "defect", &mut notes,
            );
        }
    }
    serde_json::from_value(
        payload
            .get("findings")
            .cloned()
            .ok_or_else(|| AppError::Other("submit_code_findings: missing findings".into()))?,
    )
    .map_err(|e| AppError::Other(format!("submit_code_findings: {e}")))
}

/// The code-level second stage: a short agentic run over the review plan's
/// critical/important files, on the cheaper drill model. Failures here never
/// sink the architecture result — the caller logs and moves on.
pub async fn code_findings(
    app: &AppHandle,
    settings: &Settings,
    token: &str,
    pr: &TrackedPr,
    focus_paths: &[String],
) -> AppResult<Vec<crate::analysis::types::CodeFinding>> {
    let pr_id = pr.info.id.clone();
    let level = AnalysisLevel::Context;
    progress(app, &pr_id, level, "", "code pass: starting");

    let focus_section = if focus_paths.is_empty() {
        "Focus on the changed files that carry logic; skip renames, config echoes, and import-only changes.".to_string()
    } else {
        format!(
            "Focus on these files — the ones whose diff metrics show added logic:\n{}",
            focus_paths
                .iter()
                .take(20)
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let model_id = settings.drill_model().to_string();
    let tools = RepoTools::new(
        &settings.github_graphql_url,
        &pr.info.repo,
        pr.info.number,
        &pr.info.head_sha,
        token,
    )?;

    let mut system = CODE_PASS_PROMPT.to_string();
    system.push_str(&conventions_section(settings, &pr.info.repo));
    system.push_str(&voice_section(settings));

    let mut specs = RepoTools::specs();
    specs.push((
        "submit_code_findings",
        "Submit the final list of code-level findings. Call exactly once.",
        code_findings_schema(),
    ));

    let kickoff = format!(
        "Review this pull request at code level.\n\nRepository: {}\nPR #{}: {}\nHead: {}{}\n\n{}\n\nBudget: 12 exploration turns, then only submit_code_findings remains. Start with get_pr_diff (indexed when the diff is large — get_file_diff returns the hunks for the files above), and read surrounding context with get_files in one call rather than one file per turn.",
        pr.info.repo,
        pr.info.number,
        pr.info.title,
        pr.info.head_sha,
        body_section(&pr.info.body),
        focus_section,
    );
    let mut messages = vec![Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(kickoff))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?];

    let mut use_cache = true;
    let mut effort: Option<String> =
        crate::models::effort_level(&settings.bedrock_effort_code).map(str::to_string);
    let mut resubmits = 0u32;
    const MAX_CODE_TURNS: usize = 16;
    const CODE_EXPLORE_TURNS: usize = 12;
    let submit_specs: Vec<(&'static str, &'static str, Value)> = specs
        .iter()
        .filter(|(name, _, _)| *name == "submit_code_findings")
        .cloned()
        .collect();

    for turn in 0..MAX_CODE_TURNS {
        let exploring = turn < CODE_EXPLORE_TURNS;
        let resp = converse_once(
            app,
            "code-pass",
            settings,
            &model_id,
            &system,
            &messages,
            if exploring { &specs } else { &submit_specs },
            settings.code_max_output_tokens as i32,
            &mut use_cache,
            &mut effort,
            None,
        )
        .await?;

        if let Some(usage) = resp.usage() {
            crate::usage::record(app, &pr, "code-pass", &model_id, usage);
        }

        let message = resp.message.clone();

        let mut tool_calls: Vec<(String, String, Value)> = Vec::new();
        let mut submitted: Option<(String, Value)> = None;
        for block in message.content() {
            if let ContentBlock::ToolUse(tu) = block {
                let input = document_to_value(tu.input());
                progress(
                    app,
                    &pr_id,
                    level,
                    "",
                    format!("code pass: {}", describe_tool_call(tu.name(), &input)),
                );
                if tu.name() == "submit_code_findings" {
                    submitted = Some((tu.tool_use_id().to_string(), input));
                } else {
                    tool_calls.push((tu.tool_use_id().to_string(), tu.name().to_string(), input));
                }
            }
        }

        let mut tool_results: Vec<ContentBlock> = Vec::new();
        if !tool_calls.is_empty() && !exploring {
            for (id, _, _) in &tool_calls {
                tool_results.push(tool_result(
                    id,
                    "exploration budget is spent — call submit_code_findings with what you have".into(),
                    true,
                )?);
            }
        } else if !tool_calls.is_empty() {
            let executed = futures::future::join_all(tool_calls.iter().map(|(_, name, input)| {
                let tools = &tools;
                async move { tools.execute(name, input).await }
            }))
            .await;
            for ((id, name, _), result) in tool_calls.iter().zip(executed) {
                let (content, is_error) = match result {
                    Ok(text) => (text, false),
                    Err(e) => {
                        devlog::warn(app, "code-pass", format!("tool {name} failed: {e}"));
                        (format!("error: {e}"), true)
                    }
                };
                tool_results.push(tool_result(id, content, is_error)?);
            }
        }
        if exploring && turn + 1 == CODE_EXPLORE_TURNS && submitted.is_none() {
            progress(app, &pr_id, level, "", "code pass: budget spent — asking for findings");
            tool_results.push(ContentBlock::Text(
                "Exploration budget is spent and the research tools have been withdrawn. Call submit_code_findings now with the findings you have — an empty list is a valid result.".into(),
            ));
        }

        if let Some((submit_id, mut payload)) = submitted {
            match parse_code_findings(&mut payload) {
                Ok(findings) => {
                    devlog::info(
                        app,
                        "code-pass",
                        format!("complete — {} finding(s)", findings.len()),
                    );
                    progress(app, &pr_id, level, "", "code pass: done");
                    return Ok(findings);
                }
                Err(e) if resubmits < 2 => {
                    resubmits += 1;
                    let truncated = matches!(resp.stop_reason(), StopReason::MaxTokens);
                    devlog::warn(
                        app,
                        "code-pass",
                        format!(
                            "invalid submission ({e}){} — retrying",
                            if truncated { ", output truncated at max tokens" } else { "" }
                        ),
                    );
                    let feedback = if truncated {
                        format!("Submission rejected: {e}. Your submission was CUT OFF by the output-token limit. Resubmit complete but tighter: keep only the strongest findings, one short sentence each.")
                    } else {
                        format!("Submission rejected: {e}. Call submit_code_findings again with every required field present.")
                    };
                    tool_results.push(tool_result(&submit_id, feedback, true)?);
                }
                Err(e) => return Err(e),
            }
        }

        messages.push(strip_trailing_reasoning(message));
        if !tool_results.is_empty() {
            messages.push(
                Message::builder()
                    .role(ConversationRole::User)
                    .set_content(Some(tool_results))
                    .build()
                    .map_err(|e| AppError::Other(e.to_string()))?,
            );
            continue;
        }
        // No tool calls and no submission — the conversation ends on an
        // assistant message, which some Bedrock models reject as a prefill
        // ("the conversation must end with a user message"). Always append a
        // user turn before looping. A MaxTokens truncation cut the turn short
        // before submit_code_findings landed; tell it to continue tighter.
        let nudge = if matches!(resp.stop_reason(), StopReason::MaxTokens) {
            "Your previous response was cut off by the output-token limit. Continue, and call submit_code_findings with your complete result — keep only the strongest findings, one short sentence each."
        } else {
            "Call submit_code_findings now with your complete result."
        };
        messages.push(
            Message::builder()
                .role(ConversationRole::User)
                .content(ContentBlock::Text(nudge.into()))
                .build()
                .map_err(|e| AppError::Other(e.to_string()))?,
        );
    }
    Err(AppError::Other(format!(
        "code pass did not complete within {MAX_CODE_TURNS} turns"
    )))
}

/// Bedrock doesn't validate tool input against the schema, so submissions
/// drift: snake_case keys, camelCase or Title Case enum values, severities
/// like "critical" that aren't in our enum, a missing `change` on unchanged
/// nodes. Each strict-parse failure costs a full model turn to retry, so
/// coerce the recoverable drift into shape first and keep the retry loop for
/// genuinely broken payloads. Returns a description of each coercion applied.
fn sanitize_payload(payload: &mut Value) -> Vec<String> {
    let mut notes = Vec::new();
    // The entire payload occasionally arrives as one JSON-encoded string.
    if let Some(s) = payload.as_str() {
        if let Some(v) = parse_lenient(s).filter(Value::is_object) {
            notes.push("whole payload arrived as a JSON string; unwrapped".into());
            *payload = v;
        }
    }
    // {"submission": {graph, assessment}} — the real payload one wrapper
    // key down.
    if payload.get("graph").is_none() && payload.get("assessment").is_none() {
        let inner = payload
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.values().next())
            .filter(|v| v.get("graph").is_some() || v.get("assessment").is_some())
            .cloned();
        if let Some(v) = inner {
            notes.push("payload unwrapped from a wrapper key".into());
            *payload = v;
        }
    }
    // A whole sub-struct occasionally arrives JSON-encoded as a string —
    // unwrap first so the coercions below see real objects.
    unwrap_stringified(payload, "graph", &mut notes);
    unwrap_stringified(payload, "assessment", &mut notes);
    // Flattened graph: nodes/edges emitted at the top level instead of
    // under "graph".
    if payload.get("graph").is_none() && payload.get("nodes").is_some() {
        if let Some(obj) = payload.as_object_mut() {
            let nodes = obj.remove("nodes").unwrap_or_else(|| json!([]));
            let edges = obj.remove("edges").unwrap_or_else(|| json!([]));
            obj.insert("graph".into(), json!({ "nodes": nodes, "edges": edges }));
            notes.push("graph assembled from top-level nodes/edges".into());
        }
    }
    // Assessment tucked inside the graph object.
    if payload.get("assessment").is_none() {
        if let Some(a) = payload.pointer_mut("/graph/assessment").map(Value::take) {
            if a.is_object() {
                notes.push("assessment lifted out of the graph object".into());
                payload["assessment"] = a;
            }
        }
    }
    // Flattened assessment: its fields emitted at the top level instead of
    // under "assessment". Drain everything but the graph into it — no field
    // list to keep in sync with the Assessment struct; serde ignores strays
    // and camelize_keys below handles snake_case.
    if payload.get("assessment").is_none() {
        if let Some(obj) = payload.as_object_mut() {
            if obj.contains_key("summary") || obj.contains_key("detail") || obj.contains_key("fit")
            {
                let keys: Vec<String> =
                    obj.keys().filter(|k| *k != "graph").cloned().collect();
                let mut a = serde_json::Map::new();
                for k in keys {
                    if let Some(v) = obj.remove(&k) {
                        a.insert(k, v);
                    }
                }
                obj.insert("assessment".into(), Value::Object(a));
                notes.push("assessment assembled from top-level fields".into());
            }
        }
    }
    camelize_keys(payload);

    const NODE_KIND: &[(&str, &str)] = &[
        ("external", "external-system"),
        ("external-service", "external-system"),
        ("external-api", "external-system"),
        ("third-party", "external-system"),
        ("database", "data-store"),
        ("datastore", "data-store"),
        ("db", "data-store"),
        ("storage", "data-store"),
        ("bucket", "data-store"),
        ("table", "data-store"),
        ("cache", "data-store"),
        ("message-queue", "queue"),
        ("topic", "queue"),
        ("event-bus", "queue"),
        ("stream", "queue"),
        ("service", "container"),
        ("microservice", "container"),
        ("application", "container"),
        ("app", "container"),
        ("module", "component"),
        ("class", "code"),
        ("function", "code"),
        ("user", "person"),
        ("actor", "person"),
    ];
    const CHANGE: &[(&str, &str)] = &[
        ("new", "added"),
        ("created", "added"),
        ("add", "added"),
        ("updated", "modified"),
        ("changed", "modified"),
        ("update", "modified"),
        ("modify", "modified"),
        ("deleted", "removed"),
        ("delete", "removed"),
        ("remove", "removed"),
        ("impacted", "affected"),
        ("indirect", "affected"),
        ("unchanged-neighbor", "unchanged"),
        ("neighbor", "unchanged"),
        ("none", "unchanged"),
        ("existing", "unchanged"),
        ("no-change", "unchanged"),
    ];
    const FIT: &[(&str, &str)] = &[
        ("fit", "fits"),
        ("good-fit", "fits"),
        ("aligned", "fits"),
        ("aligns", "fits"),
        ("mismatch", "misfit"),
        ("misaligned", "misfit"),
        ("poor-fit", "misfit"),
        ("does-not-fit", "misfit"),
        ("conflict", "misfit"),
        ("tensions", "tension"),
        ("minor-tension", "tension"),
        ("some-tension", "tension"),
    ];
    const IMPACT_KIND: &[(&str, &str)] = &[
        ("external-system", "external"),
        ("external-boundary", "external"),
        ("third-party", "external"),
        ("cross-service", "service"),
        ("service-boundary", "service"),
        ("container", "service"),
        ("internal-module", "internal"),
        ("module", "internal"),
    ];
    const PILLAR: &[(&str, &str)] = &[
        ("performance", "performance-efficiency"),
        ("cost", "cost-optimization"),
        ("costs", "cost-optimization"),
        ("operations", "operational-excellence"),
        ("operational", "operational-excellence"),
        ("ops", "operational-excellence"),
    ];
    const SEVERITY: &[(&str, &str)] = &[
        ("critical", "high"),
        ("blocker", "high"),
        ("severe", "high"),
        ("warning", "medium"),
        ("moderate", "medium"),
        ("minor", "low"),
        ("informational", "info"),
        ("note", "info"),
    ];
    const SIGNIFICANCE: &[(&str, &str)] = &[
        ("high", "critical"),
        ("major", "critical"),
        ("medium", "important"),
        ("moderate", "important"),
        ("notable", "important"),
        ("low", "mechanical"),
        ("minor", "mechanical"),
        ("trivial", "mechanical"),
        ("noise", "mechanical"),
    ];

    if let Some(nodes) = payload.pointer_mut("/graph/nodes").and_then(Value::as_array_mut) {
        for n in nodes {
            normalize_enum(n, "kind", NODE_KIND, &mut notes);
            // An off-schema kind renders as a slightly-wrong card; a rejected
            // submission costs a whole turn. Coerce and note it.
            coerce_unknown::<C4NodeKind>(n, "kind", "component", &mut notes);
            fill_missing(n, "change", "unchanged", &mut notes);
            normalize_enum(n, "change", CHANGE, &mut notes);
            coerce_unknown::<ChangeStatus>(n, "change", "unchanged", &mut notes);
            if str_field(n, "id").is_none() {
                if let Some(name) = str_field(n, "name") {
                    let slug = format!("node:{}", kebab(&name));
                    notes.push(format!("synthesized node id {slug}"));
                    n["id"] = json!(slug);
                }
            }
        }
    }
    if let Some(edges) = payload.pointer_mut("/graph/edges").and_then(Value::as_array_mut) {
        for e in edges {
            fill_missing(e, "change", "unchanged", &mut notes);
            normalize_enum(e, "change", CHANGE, &mut notes);
            coerce_unknown::<ChangeStatus>(e, "change", "unchanged", &mut notes);
            if str_field(e, "id").is_none() {
                let s = str_field(e, "source").unwrap_or_else(|| "?".into());
                let t = str_field(e, "target").unwrap_or_else(|| "?".into());
                notes.push(format!("synthesized edge id {s}->{t}"));
                e["id"] = json!(format!("{s}->{t}"));
            }
        }
    }
    if let Some(a) = payload.get_mut("assessment") {
        normalize_enum(a, "fit", FIT, &mut notes);
        // A missing TLDR shouldn't sink an otherwise-good submission.
        if str_field(a, "summary").is_none() {
            if let Some(detail) = str_field(a, "detail") {
                let cut: String = detail.chars().take(160).collect();
                notes.push("summary derived from detail".into());
                a["summary"] = json!(cut);
            }
        }
        if let Some(impacts) = a.get_mut("boundaryImpacts").and_then(Value::as_array_mut) {
            for i in impacts {
                normalize_enum(i, "kind", IMPACT_KIND, &mut notes);
            }
        }
        if let Some(findings) = a.get_mut("wellArchitected").and_then(Value::as_array_mut) {
            for f in findings {
                normalize_enum(f, "pillar", PILLAR, &mut notes);
                normalize_enum(f, "severity", SEVERITY, &mut notes);
                coerce_unknown::<Severity>(f, "severity", "medium", &mut notes);
            }
        }
        if let Some(plan) = a.get_mut("reviewPlan").and_then(Value::as_array_mut) {
            for p in plan {
                normalize_enum(p, "significance", SIGNIFICANCE, &mut notes);
                coerce_unknown::<Significance>(p, "significance", "important", &mut notes);
            }
        }
    }
    notes
}

/// Models sometimes emit a whole struct field as a JSON-encoded string
/// ("assessment": "{\"summary\":...}"). Parse and unwrap it in place rather
/// than burning a retry turn; tolerates the common botch of a spurious brace
/// closing the top-level value mid-string.
fn unwrap_stringified(payload: &mut Value, field: &str, notes: &mut Vec<String>) {
    let Some(s) = payload.get(field).and_then(Value::as_str) else {
        return;
    };
    if let Some(v) = parse_lenient(s) {
        if v.is_object() || v.is_array() {
            notes.push(format!("{field} arrived as a JSON string; unwrapped"));
            payload[field] = v;
        }
    }
}

/// Parse JSON the model hand-assembled into a string. A model that
/// stringifies a sub-struct also tends to leave raw newlines inside string
/// literals, trailing commas, a code fence, or one closer too many — each
/// of which serde rejects outright, and each of which used to cost a
/// resubmit turn at the top model's output speed. Try the cheap repairs in
/// order, strictest first, so a clean string is never altered.
pub(crate) fn parse_lenient(s: &str) -> Option<Value> {
    let body = strip_fence(s);
    // Two starting points: the text as written, and the text with quotes
    // inside string values escaped — a shell snippet quoted into a
    // description ends the string early and nothing after it parses.
    let starts = [body.to_string(), escape_inner_quotes(body)];
    starts.iter().find_map(|start| {
        let escaped = escape_raw_controls(start);
        let uncommaed = drop_trailing_commas(&escaped);
        let candidates = [
            start.clone(),
            escaped.clone(),
            uncommaed.clone(),
            drop_spurious_closers(&uncommaed),
            close_open(&uncommaed),
        ];
        candidates
            .iter()
            .find_map(|c| serde_json::from_str::<Value>(c).ok())
    })
}

/// A `"` inside a string value that is not followed by a structural
/// character (`,` `}` `]` `:`, or the end of what arrived) cannot be the
/// closing quote — it is content, and gets escaped.
fn escape_inner_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let (mut in_str, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if !in_str {
            if c == '"' {
                in_str = true;
            }
            out.push(c);
            continue;
        }
        if escaped {
            escaped = false;
            out.push(c);
        } else if c == '\\' {
            escaped = true;
            out.push(c);
        } else if c == '"' {
            let next = chars[i + 1..].iter().find(|ch| !ch.is_whitespace());
            if matches!(next, None | Some(',') | Some('}') | Some(']') | Some(':')) {
                in_str = false;
                out.push(c);
            } else {
                out.push_str("\\\"");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The value of a top-level-ish string field out of JSON that is still
/// being written: everything after `"key": "` up to the closing quote, or
/// to the end of what has arrived. Basic escapes are decoded; the rest is
/// left alone — this is for showing, not parsing.
pub(crate) fn json_string_field(partial: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = partial.find(&needle)? + needle.len();
    let rest = partial[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => {}
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Trim a ```json fence and any prose around the outermost value.
fn strip_fence(s: &str) -> &str {
    let t = s.trim();
    let start = t.find(['{', '[']).unwrap_or(0);
    let end = t.rfind(['}', ']']).map(|i| i + 1).unwrap_or(t.len());
    if start < end {
        &t[start..end]
    } else {
        t
    }
}

/// Walk `s` as JSON text, calling `f` with each char and whether it sits
/// inside a string literal (the opening/closing quotes count as outside).
fn walk_json(s: &str, mut f: impl FnMut(char, bool)) {
    let (mut in_str, mut escaped) = (false, false);
    for c in s.chars() {
        if in_str {
            if escaped {
                escaped = false;
                f(c, true);
            } else if c == '\\' {
                escaped = true;
                f(c, true);
            } else if c == '"' {
                in_str = false;
                f(c, false);
            } else {
                f(c, true);
            }
        } else {
            if c == '"' {
                in_str = true;
            }
            f(c, false);
        }
    }
}

/// Raw control characters inside string literals — a newline typed into a
/// description instead of `\n` — are the commonest reason a stringified
/// payload fails to parse.
fn escape_raw_controls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    walk_json(s, |c, in_str| match (in_str, c) {
        (true, '\n') => out.push_str("\\n"),
        (true, '\r') => out.push_str("\\r"),
        (true, '\t') => out.push_str("\\t"),
        (true, c) if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
        (_, c) => out.push(c),
    });
    out
}

/// `,}` and `,]` outside strings.
fn drop_trailing_commas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_comma = false;
    walk_json(s, |c, in_str| {
        if !in_str && pending_comma {
            if c.is_whitespace() {
                return;
            }
            pending_comma = false;
            if c != '}' && c != ']' {
                out.push(',');
            }
        }
        if !in_str && c == ',' {
            pending_comma = true;
        } else {
            out.push(c);
        }
    });
    if pending_comma {
        out.push(',');
    }
    out
}

/// A payload cut off mid-value: close the open string and every open
/// container so what was written survives.
fn close_open(s: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    walk_json(s, |c, inside| {
        if !inside {
            match c {
                '{' => stack.push('}'),
                '[' => stack.push(']'),
                '}' | ']' => {
                    stack.pop();
                }
                _ => {}
            }
        }
    });
    let mut out = s.trim_end().to_string();
    // The walk reports the closing quote as outside; an unterminated string
    // ends the walk inside.
    let unterminated = {
        let mut inside = false;
        walk_json(s, |_, i| inside = i);
        inside
    };
    if unterminated {
        out.push('"');
    }
    if out.ends_with(',') {
        out.pop();
    }
    if out.ends_with(':') {
        out.push_str(" null");
    }
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    out
}

/// Remove `}`/`]` characters that would close the top-level value while
/// meaningful content still follows — the typical shape of a hand-assembled
/// JSON string with one closer too many in the middle.
fn drop_spurious_closers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '{' | '[' => {
                depth += 1;
                out.push(c);
            }
            '}' | ']' => {
                let tail_empty = chars[i + 1..].iter().all(|t| t.is_whitespace());
                if depth <= 1 && !tail_empty {
                    continue; // would close the top level with content remaining
                }
                depth -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// "externalSystem" / "External System" / "external_system" → "external-system".
fn kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_alnum = false;
    for c in s.trim().chars() {
        if c == '_' || c == ' ' || c == '-' || c == '/' {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            prev_alnum = false;
        } else if c.is_uppercase() {
            if prev_alnum && !out.ends_with('-') {
                out.push('-');
            }
            out.extend(c.to_lowercase());
            prev_alnum = false;
        } else {
            out.push(c);
            prev_alnum = c.is_alphanumeric();
        }
    }
    out.trim_matches('-').to_string()
}

fn str_field(obj: &Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Kebab-normalize a string field, then map known synonyms onto schema values.
fn normalize_enum(obj: &mut Value, key: &str, synonyms: &[(&str, &str)], notes: &mut Vec<String>) {
    let Some(raw) = str_field(obj, key) else { return };
    let mut v = kebab(&raw);
    if let Some((_, mapped)) = synonyms.iter().find(|(from, _)| *from == v) {
        v = (*mapped).to_string();
    }
    if v != raw {
        notes.push(format!("{key} \"{raw}\" → \"{v}\""));
        obj[key] = json!(v);
    }
}

/// Last resort for values still outside the schema after normalization.
/// Validity is derived from the target enum's own serde form, so new
/// variants added in types.rs are automatically accepted here.
fn coerce_unknown<T: serde::de::DeserializeOwned>(
    obj: &mut Value,
    key: &str,
    fallback: &str,
    notes: &mut Vec<String>,
) {
    let Some(v) = str_field(obj, key) else { return };
    if serde_json::from_value::<T>(Value::String(v.clone())).is_err() {
        notes.push(format!("unknown {key} \"{v}\" → \"{fallback}\""));
        obj[key] = json!(fallback);
    }
}

fn fill_missing(obj: &mut Value, key: &str, default: &str, notes: &mut Vec<String>) {
    if str_field(obj, key).is_none() {
        notes.push(format!("missing {key} → \"{default}\""));
        obj[key] = json!(default);
    }
}

/// Convert snake_case object keys to camelCase, recursively — models slip
/// into Rust-style field names ("fit_rationale", "crosses_boundary").
fn camelize_keys(v: &mut Value) {
    match v {
        Value::Object(map) => {
            let snake: Vec<String> = map
                .keys()
                .filter(|k| k.contains('_'))
                .cloned()
                .collect();
            for key in snake {
                let camel: String = {
                    let mut out = String::with_capacity(key.len());
                    let mut upper_next = false;
                    for c in key.chars() {
                        if c == '_' {
                            upper_next = true;
                        } else if upper_next {
                            out.extend(c.to_uppercase());
                            upper_next = false;
                        } else {
                            out.push(c);
                        }
                    }
                    out
                };
                if let Some(val) = map.remove(&key) {
                    map.entry(camel).or_insert(val);
                }
            }
            for val in map.values_mut() {
                camelize_keys(val);
            }
        }
        Value::Array(items) => {
            for item in items {
                camelize_keys(item);
            }
        }
        _ => {}
    }
}

/// Reconcile the model's review plan with computed diff metrics: attach the
/// metrics for UI transparency, downgrade "critical" tags on files whose
/// additions contain no actual logic, and give files the model skipped a
/// metric-derived default. Returns a description of each adjustment.
fn ground_review_plan(
    assessment: &mut Assessment,
    metrics: &[(String, FileMetrics)],
) -> Vec<String> {
    let mut notes = Vec::new();
    if metrics.is_empty() {
        return notes;
    }
    let by_path: std::collections::HashMap<&str, &FileMetrics> =
        metrics.iter().map(|(p, m)| (p.as_str(), m)).collect();

    for entry in &mut assessment.review_plan {
        let Some(m) = by_path.get(entry.path.as_str()) else { continue };
        entry.metrics = Some((*m).clone());
        if entry.significance == Significance::Critical && m.is_logicless() {
            entry.significance = Significance::Important;
            if !entry.reason.is_empty() {
                entry.reason.push_str(" · ");
            }
            entry.reason.push_str("downgraded: no added logic (0 branches, 0 new defs)");
            notes.push(format!("{} critical→important (logicless)", entry.path));
        }
    }

    // Files present in the diff but absent from the plan get a floor entry so
    // the reading order covers everything.
    let planned: std::collections::HashSet<&str> =
        assessment.review_plan.iter().map(|e| e.path.as_str()).collect();
    let additions: Vec<ReviewPlanEntry> = metrics
        .iter()
        .filter(|(path, _)| !planned.contains(path.as_str()))
        .map(|(path, m)| {
            let significance = if m.is_mechanical() {
                Significance::Mechanical
            } else {
                Significance::Important
            };
            notes.push(format!("{path} unclassified → {}", significance.as_str()));
            ReviewPlanEntry {
                path: path.clone(),
                significance,
                reason: "not classified by the analysis — defaulted from diff metrics".into(),
                metrics: Some(m.clone()),
            }
        })
        .collect();
    assessment.review_plan.extend(additions);
    notes
}

fn parse_payload(payload: &Value) -> AppResult<(C4Graph, Assessment)> {
    let graph: C4Graph = serde_json::from_value(
        payload
            .get("graph")
            .cloned()
            .ok_or_else(|| AppError::Other("submit_analysis: missing graph".into()))?,
    )
    .map_err(|e| AppError::Other(format!("submit_analysis graph: {e}")))?;
    let mut assessment: Assessment = serde_json::from_value(
        payload
            .get("assessment")
            .cloned()
            .ok_or_else(|| AppError::Other("submit_analysis: missing assessment".into()))?,
    )
    .map_err(|e| AppError::Other(format!("submit_analysis assessment: {e}")))?;

    // Enforce the priority ordering regardless of what the model emitted.
    assessment
        .boundary_impacts
        .sort_by_key(|impact| match impact.kind {
            crate::analysis::types::ImpactKind::External => 0,
            crate::analysis::types::ImpactKind::Service => 1,
            crate::analysis::types::ImpactKind::Internal => 2,
        });
    Ok((graph, assessment))
}

#[allow(clippy::too_many_arguments)]
fn build_result(
    pr: &TrackedPr,
    level: AnalysisLevel,
    focus_node_id: Option<String>,
    graph: C4Graph,
    assessment: Assessment,
    trace: Vec<TraceStep>,
    usage: AnalysisUsage,
) -> AnalysisResult {
    AnalysisResult {
        pr_id: pr.info.id.clone(),
        head_sha: pr.info.head_sha.clone(),
        level,
        focus_node_id,
        graph,
        assessment,
        created_at: Utc::now().to_rfc3339(),
        trace,
        usage,
        code_findings: Vec::new(),
        code_pass: None,
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_description_reaches_the_model_whole_or_visibly_cut() {
        assert_eq!(body_section("   \n  "), "", "an empty description adds no section");

        let short = body_section("Switches billing to the new provider. Flag stays off until Q3.");
        assert!(short.contains("Flag stays off until Q3."));
        assert!(!short.contains("truncated"));

        // Over the cap the model must be told it is holding a fragment —
        // silently cutting it invites conclusions drawn from half a sentence.
        let long = body_section(&"x".repeat(MAX_BODY_CHARS + 1));
        assert!(long.contains("truncated"));
        assert!(long.contains("[…truncated]"));
        assert!(long.contains(&"x".repeat(MAX_BODY_CHARS)));
        assert!(!long.contains(&"x".repeat(MAX_BODY_CHARS + 1)));
    }
    use super::*;

    #[test]
    fn the_voice_is_always_in_the_prompt_the_reviewers_own_or_the_default() {
        // No setting: the default voice, not an empty section — a model told
        // nothing about voice writes like a tool, which is the thing the
        // section exists to prevent.
        let mut settings = Settings::default();
        let section = voice_section(&settings);
        assert!(section.contains("## The reviewer's voice"));
        assert!(section.contains(DEFAULT_REVIEW_VOICE));

        // Whitespace is no voice either.
        settings.review_voice = "  \n ".into();
        assert!(voice_section(&settings).contains(DEFAULT_REVIEW_VOICE));

        // A voice of their own replaces the default outright rather than
        // stacking on it: the two would contradict.
        settings.review_voice = "lowercase, blunt, prolly a 'brutal' or two".into();
        let section = voice_section(&settings);
        assert!(section.contains("prolly a 'brutal' or two"));
        assert!(!section.contains(DEFAULT_REVIEW_VOICE));
    }

    #[test]
    fn detects_max_tokens_over_cap() {
        // Real Bedrock/Anthropic phrasings for an over-cap max_tokens.
        for s in [
            "the maximum tokens you requested exceeds the model limit of 8192.",
            "max_tokens: 32000 > 8192, which is the maximum allowed number of output tokens",
            "malformed input request: #/inferenceconfig/maxtokens: 32000 is not less or equal to 8192, please reformat",
        ] {
            assert!(is_max_tokens_over_cap(&s.to_lowercase()), "should flag: {s}");
        }
    }

    #[test]
    fn credential_errors_are_not_mistaken_for_over_cap() {
        // These contain "token" but are auth failures — must NOT match, or the
        // user gets sent to lower a ceiling when their SSO session expired.
        for s in [
            "the security token included in the request is expired",
            "unable to load credentials; your sso session token is invalid",
            "throttlingexception: rate exceeded",
        ] {
            assert!(!is_max_tokens_over_cap(&s.to_lowercase()), "should not flag: {s}");
        }
    }

    /// A `ServiceError` carrying `status`, which is all `transient_reason`
    /// reads off it — the error body's type is irrelevant, so use a String.
    fn service_error(status: u16) -> aws_sdk_bedrockruntime::error::SdkError<String> {
        aws_sdk_bedrockruntime::error::SdkError::service_error(
            String::new(),
            aws_sdk_bedrockruntime::config::http::HttpResponse::new(
                status.try_into().unwrap(),
                aws_smithy_types::body::SdkBody::empty(),
            ),
        )
    }

    #[test]
    fn retries_failures_that_never_reached_the_model() {
        // The observed outage: the proxy could not resolve its upstream and
        // answered 502. Alongside it, the rest of the shed/blip family.
        for status in [429, 500, 502, 503, 504] {
            assert!(transient_reason(&service_error(status)).is_some(), "should retry {status}");
        }
        let dns = aws_sdk_bedrockruntime::error::SdkError::<String>::dispatch_failure(
            aws_sdk_bedrockruntime::error::ConnectorError::io("nxdomain".into()),
        );
        assert_eq!(transient_reason(&dns), Some("upstream unreachable"));
    }

    #[test]
    fn does_not_retry_failures_that_will_repeat_identically() {
        // Resending these burns the backoff ladder to reach the same error,
        // and buries the message that tells the user how to fix it.
        for status in [400, 403, 404, 413] {
            assert!(transient_reason(&service_error(status)).is_none(), "should not retry {status}");
        }
        // The observed case: an expired SSO session. The SDK reports the
        // credential provider's failure as a dispatch failure of kind
        // "other" — the same variant as a dead network, but not the same
        // kind. Treating it as an outage held every call for a false 25s.
        let expired = aws_sdk_bedrockruntime::error::SdkError::<String>::dispatch_failure(
            aws_sdk_bedrockruntime::error::ConnectorError::other(
                "an error occurred while loading credentials: UnauthorizedException: Session token not found or invalid".into(),
                None,
            ),
        );
        assert_eq!(transient_reason(&expired), None, "credentials are not a network blip");
        let user = aws_sdk_bedrockruntime::error::SdkError::<String>::dispatch_failure(
            aws_sdk_bedrockruntime::error::ConnectorError::user("bad request shape".into()),
        );
        assert_eq!(transient_reason(&user), None);
    }

    #[test]
    fn an_expired_token_is_a_rebuild_not_an_outage_or_a_login_hint() {
        // Bedrock's code for a rotated session token, and the STS phrasing
        // that reaches us when the SDK signs with lapsed credentials.
        assert!(is_expired_token(Some("ExpiredTokenException"), ""));
        assert!(is_expired_token(Some("ExpiredToken"), ""));
        assert!(is_expired_token(None, "the security token included in the request is expired"));
        // Not a rotation: a token that was never valid, or a plain 403.
        assert!(!is_expired_token(Some("UnrecognizedClientException"), "the security token included in the request is invalid"));
        assert!(!is_expired_token(Some("AccessDeniedException"), "access denied"));
        // A run's own expiry setting mentions neither token nor security.
        assert!(!is_expired_token(None, "max_tokens exceeds the model limit"));
    }

    #[test]
    fn kebab_normalizes_model_drift() {
        assert_eq!(kebab("externalSystem"), "external-system");
        assert_eq!(kebab("External System"), "external-system");
        assert_eq!(kebab("external_system"), "external-system");
        assert_eq!(kebab("data-store"), "data-store");
        assert_eq!(kebab("Fits"), "fits");
    }

    #[test]
    fn sanitize_rescues_typical_drift() {
        let mut payload = json!({
            "graph": {
                "nodes": [
                    {"id": "s:api", "name": "API", "kind": "Service", "change": "Updated"},
                    {"name": "Postgres", "kind": "database"}
                ],
                "edges": [
                    {"source": "s:api", "target": "node:postgres", "label": "reads", "crosses_boundary": true, "change": "unchanged-neighbor"}
                ]
            },
            "assessment": {
                "detail": "A longer explanation of the change and its mechanics.",
                "fit": "Fits",
                "fit_rationale": "matches the existing pattern",
                "boundaryImpacts": [{"kind": "external-system", "description": "calls a partner API"}],
                "wellArchitected": [{"pillar": "Performance", "severity": "critical", "finding": "f", "recommendation": "r"}],
                "reviewPlan": [{"path": "src/a.ts", "significance": "High", "reason": "core"}]
            }
        });
        let notes = sanitize_payload(&mut payload);
        assert!(!notes.is_empty());
        let (graph, assessment) = parse_payload(&payload).expect("sanitized payload parses");

        assert_eq!(graph.nodes[0].kind, crate::analysis::types::C4NodeKind::Container);
        assert_eq!(graph.nodes[0].change, crate::analysis::types::ChangeStatus::Modified);
        assert_eq!(graph.nodes[1].kind, crate::analysis::types::C4NodeKind::DataStore);
        assert_eq!(graph.nodes[1].change, crate::analysis::types::ChangeStatus::Unchanged);
        assert_eq!(graph.nodes[1].id, "node:postgres");
        assert_eq!(graph.edges[0].change, crate::analysis::types::ChangeStatus::Unchanged);
        assert!(graph.edges[0].crosses_boundary, "snake_case key camelized");
        assert!(!graph.edges[0].id.is_empty(), "edge id synthesized");

        assert_eq!(assessment.fit, crate::analysis::types::FitVerdict::Fits);
        assert!(!assessment.summary.is_empty(), "summary derived from detail");
        assert_eq!(assessment.fit_rationale, "matches the existing pattern");
        assert_eq!(
            assessment.boundary_impacts[0].kind,
            crate::analysis::types::ImpactKind::External
        );
        let finding = &assessment.well_architected[0];
        assert_eq!(finding.pillar, crate::analysis::types::Pillar::PerformanceEfficiency);
        assert_eq!(finding.severity, crate::analysis::types::Severity::High);
        assert_eq!(assessment.review_plan[0].significance, Significance::Critical);
    }

    #[test]
    fn sanitize_leaves_valid_payloads_alone() {
        let mut payload = json!({
            "graph": {
                "nodes": [{"id": "c:api", "name": "API", "kind": "container", "change": "modified"}],
                "edges": []
            },
            "assessment": {
                "summary": "s", "detail": "d", "fit": "fits", "fitRationale": "r",
                "boundaryImpacts": [], "wellArchitected": [], "contextNotes": [], "reviewPlan": []
            }
        });
        let notes = sanitize_payload(&mut payload);
        assert!(notes.is_empty(), "no coercions expected, got: {notes:?}");
        parse_payload(&payload).expect("valid payload parses");
    }

    #[test]
    fn sanitize_recovers_missing_graph_shapes() {
        let graph = json!({
            "nodes": [{"id": "c:api", "name": "API", "kind": "container", "change": "modified"}],
            "edges": []
        });
        let assessment = json!({
            "summary": "s", "detail": "d", "fit": "fits", "fitRationale": "r",
            "boundaryImpacts": [], "wellArchitected": [], "contextNotes": [], "reviewPlan": []
        });

        // Real payload one wrapper key down.
        let mut wrapped = json!({ "submission": { "graph": graph, "assessment": assessment } });
        sanitize_payload(&mut wrapped);
        parse_payload(&wrapped).expect("wrapper key unwrapped");

        // nodes/edges flattened to the top level instead of under "graph".
        let mut flat = json!({
            "nodes": graph["nodes"], "edges": graph["edges"], "assessment": assessment
        });
        sanitize_payload(&mut flat);
        parse_payload(&flat).expect("graph assembled from top-level nodes/edges");

        // The whole payload as one JSON-encoded string.
        let mut stringified =
            Value::String(json!({ "graph": graph, "assessment": assessment }).to_string());
        sanitize_payload(&mut stringified);
        parse_payload(&stringified).expect("whole-string payload unwrapped");

        // Assessment fields flattened to the top level next to the graph.
        let mut flat_assessment = json!({
            "graph": graph,
            "summary": "s", "detail": "d", "fit": "fits", "fitRationale": "r",
            "boundaryImpacts": [], "wellArchitected": [], "contextNotes": [], "reviewPlan": []
        });
        sanitize_payload(&mut flat_assessment);
        parse_payload(&flat_assessment).expect("assessment assembled from top-level fields");

        // Assessment tucked inside the graph object.
        let mut nested = json!({
            "graph": {
                "nodes": graph["nodes"], "edges": graph["edges"], "assessment": assessment
            }
        });
        sanitize_payload(&mut nested);
        parse_payload(&nested).expect("assessment lifted out of the graph");
    }

    #[test]
    fn sanitize_unwraps_stringified_assessment() {
        // Observed in the wild: the model JSON-encodes the whole assessment
        // as a string, with a spurious `}` after "detail" closing the object
        // mid-stream.
        let assessment_str = r#"{"summary":"Frontend-only change.","detail":"The PR is presentational."},"fit":"fits","fitRationale":"follows the pattern","boundaryImpacts":[],"wellArchitected":[],"contextNotes":["note"],"reviewPlan":[{"path":"a.tsx","significance":"important","reason":"r"}]}"#;
        let mut payload = json!({
            "graph": {
                "nodes": [{"id": "c:api", "name": "API", "kind": "container", "change": "modified"}],
                "edges": []
            },
            "assessment": assessment_str
        });
        let notes = sanitize_payload(&mut payload);
        assert!(
            notes.iter().any(|n| n.contains("unwrapped")),
            "expected an unwrap note, got: {notes:?}"
        );
        let (_, assessment) = parse_payload(&payload).expect("unwrapped payload parses");
        assert_eq!(assessment.summary, "Frontend-only change.");
        assert_eq!(assessment.fit, crate::analysis::types::FitVerdict::Fits);
        assert_eq!(assessment.review_plan.len(), 1);
    }

    #[test]
    fn lenient_parse_rescues_the_ways_a_stringified_assessment_breaks() {
        // Raw newlines inside string literals — the hand-assembled shape.
        let raw = "{\"summary\": \"line one\nline two\", \"fit\": \"fits\"}";
        assert!(serde_json::from_str::<Value>(raw).is_err(), "serde rejects raw control chars");
        let v = parse_lenient(raw).expect("escaped");
        assert_eq!(v["summary"], "line one\nline two");

        // Trailing commas, in an object and an array.
        let v = parse_lenient("{\"a\": [1, 2,], \"b\": {\"c\": 1,},}").expect("commas");
        assert_eq!(v["a"], json!([1, 2]));
        assert_eq!(v["b"]["c"], 1);

        // A code fence with prose around it.
        let v = parse_lenient("Here it is:\n```json\n{\"fit\": \"tension\"}\n```\n").expect("fence");
        assert_eq!(v["fit"], "tension");

        // Cut off mid-string: what was written survives.
        let v = parse_lenient("{\"summary\": \"partial\", \"contextNotes\": [\"one\", \"tw").expect("closed");
        assert_eq!(v["summary"], "partial");
        assert_eq!(v["contextNotes"][0], "one");

        // Raw quotes inside a value — a shell snippet quoted into a
        // description, the shape that slipped past the first repair.
        let raw = "{\"summary\": \"Adds a job.\", \"detail\": \"finds the newest `\"'\"'\"<app>/vX.Y.Z\"'\"'\"` tag, increments\"}";
        let v = parse_lenient(raw).expect("inner quotes escaped");
        assert_eq!(v["summary"], "Adds a job.");
        assert_eq!(v["detail"], "finds the newest `\"'\"'\"<app>/vX.Y.Z\"'\"'\"` tag, increments");
        // Already-escaped quotes and structural quotes are left alone.
        let v = parse_lenient("{\"a\": \"say \\\"hi\\\"\", \"b\": {\"c\": \"d\"}}").expect("escaped quotes kept");
        assert_eq!(v["a"], "say \"hi\"");
        assert_eq!(v["b"]["c"], "d");

        // A clean string is never altered.
        let v = parse_lenient("{\"s\": \"a,}\\n]\"}").expect("clean");
        assert_eq!(v["s"], "a,}\n]");
        assert!(parse_lenient("not json at all").is_none());
    }

    #[test]
    fn draft_fields_read_out_of_half_written_json() {
        let partial = "{\"assessment\": {\"summary\": \"Reorders the pick.\\nSecond line.\", \"detail\": \"It starts by mov";
        assert_eq!(json_string_field(partial, "summary").as_deref(), Some("Reorders the pick.\nSecond line."));
        assert_eq!(json_string_field(partial, "detail").as_deref(), Some("It starts by mov"));
        assert_eq!(json_string_field(partial, "fit"), None);
        assert_eq!(json_string_field("{\"summary\": \"", "summary"), None);
        assert_eq!(json_string_field("{\"summary\": [", "summary"), None);
    }

    #[test]
    fn graph_parses_without_the_change_fields() {
        let g: C4Graph = serde_json::from_value(json!({
            "nodes": [{"id": "a", "name": "A", "kind": "container"}],
            "edges": [{"id": "e", "source": "a", "target": "a", "label": "calls"}]
        }))
        .expect("lean graph");
        assert_eq!(g.nodes[0].change, ChangeStatus::Unchanged);
        assert_eq!(g.edges[0].change, ChangeStatus::Unchanged);
        assert!(!g.edges[0].crosses_boundary);
    }

    #[test]
    fn sanitize_unwraps_a_stringified_assessment_with_raw_newlines() {
        let mut payload = json!({
            "graph": {"nodes": [], "edges": []},
            "assessment": "{\"summary\": \"Reorders x\nso y\", \"detail\": \"d\", \"fit\": \"fits\", \"fitRationale\": \"r\", \"boundaryImpacts\": [], \"wellArchitected\": [], \"contextNotes\": [], \"reviewPlan\": [],}"
        });
        let notes = sanitize_payload(&mut payload);
        assert!(notes.iter().any(|n| n.contains("assessment arrived as a JSON string")), "{notes:?}");
        assert!(parse_payload(&payload).is_ok(), "{payload}");
    }

    #[test]
    fn code_focus_is_the_logic_files_most_logic_first() {
        let m = |b, d, imp: f32| FileMetrics {
            additions: 10,
            deletions: 0,
            added_branches: b,
            new_defs: d,
            import_share: imp,
            max_nesting: 1,
        };
        let metrics = vec![
            ("wiring.ts".to_string(), m(0, 0, 0.0)),
            ("small.ts".to_string(), m(1, 0, 0.0)),
            ("imports.ts".to_string(), m(3, 1, 0.9)),
            ("core.ts".to_string(), m(6, 2, 0.1)),
        ];
        assert_eq!(code_focus_paths(&metrics), vec!["core.ts", "small.ts"]);
        assert!(code_focus_paths(&[]).is_empty());
    }

    #[test]
    fn drop_spurious_closers_ignores_braces_inside_strings() {
        let s = r#"{"a":"has } and ] inside","b":[1,2]}"#;
        assert_eq!(drop_spurious_closers(s), s);
    }
}



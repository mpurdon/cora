use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// C4 abstraction levels. L1 covers Context+Container in one pass; deeper
/// levels are generated on demand when the user drills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum AnalysisLevel {
    Context,
    Component,
    Code,
}

impl AnalysisLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnalysisLevel::Context => "context",
            AnalysisLevel::Component => "component",
            AnalysisLevel::Code => "code",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum C4NodeKind {
    Person,
    ExternalSystem,
    System,
    Container,
    Component,
    Code,
    DataStore,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Removed,
    Affected,
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct C4Node {
    /// Deterministic id derived from repo/module path so re-analyses don't
    /// reshuffle layouts (e.g. "container:api-service").
    pub id: String,
    pub name: String,
    pub kind: C4NodeKind,
    #[serde(default)]
    pub technology: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Parent boundary node id (system a container belongs to, etc.).
    #[serde(default)]
    pub boundary: Option<String>,
    pub change: ChangeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct C4Edge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub crosses_boundary: bool,
    pub change: ChangeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct C4Graph {
    pub nodes: Vec<C4Node>,
    pub edges: Vec<C4Edge>,
}

/// Priority order the whole product is built around:
/// external systems > service boundaries > internal modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ImpactKind {
    External,
    Service,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryImpact {
    pub kind: ImpactKind,
    pub description: String,
    #[serde(default)]
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum Pillar {
    OperationalExcellence,
    Security,
    Reliability,
    PerformanceEfficiency,
    CostOptimization,
    Sustainability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct WaFinding {
    pub pillar: Pillar,
    pub severity: Severity,
    pub finding: String,
    pub recommendation: String,
    #[serde(default)]
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum FitVerdict {
    Fits,
    Tension,
    Misfit,
}

/// Per-file attention level, most demanding first — ordering is meaningful
/// (reading order derives from the ordinal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum Significance {
    Critical,
    Important,
    Mechanical,
}

impl Significance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Significance::Critical => "critical",
            Significance::Important => "important",
            Significance::Mechanical => "mechanical",
        }
    }
}

/// Model-assigned per-file review guidance: where to spend attention.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPlanEntry {
    pub path: String,
    pub significance: Significance,
    #[serde(default)]
    pub reason: String,
    /// Computed diff metrics, attached post-hoc (never emitted by the model).
    #[serde(default)]
    #[ts(optional)]
    pub metrics: Option<crate::analysis::metrics::FileMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    /// The TLDR — at most two short sentences.
    pub summary: String,
    /// The fuller explanation, shown behind a "more detail" expander.
    #[serde(default)]
    pub detail: String,
    pub fit: FitVerdict,
    /// Defaults tolerate models that omit "empty" fields despite the schema.
    #[serde(default)]
    pub fit_rationale: String,
    /// Ordered most-important-first; external impacts always lead.
    #[serde(default)]
    pub boundary_impacts: Vec<BoundaryImpact>,
    #[serde(default)]
    pub well_architected: Vec<WaFinding>,
    /// What a reviewer without full context needs to know.
    #[serde(default)]
    pub context_notes: Vec<String>,
    /// Per-changed-file attention guidance, most important first.
    #[serde(default)]
    pub review_plan: Vec<ReviewPlanEntry>,
}

/// One step of the agent's exploration, kept for the activity drawer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TraceStep {
    pub at: String,
    /// "tool" (fetched something) | "thought" (model narration) | "status"
    pub kind: String,
    pub message: String,
}

/// What class of code-level finding the second pass reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum CodeFindingKind {
    /// A consequence-bearing bug: wrong deps, unhandled paths, races, leaks.
    Defect,
    /// Hand-rolled code duplicating an existing repo/design-system primitive.
    Reuse,
    /// A team-conventions violation (from the reviewer's settings).
    Convention,
}

/// Line-anchored output of the code-findings pass — the altitude below the
/// architecture assessment, still above style nits.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CodeFinding {
    pub path: String,
    /// New-side diff line the finding anchors to, when the model gave one.
    #[serde(default)]
    #[ts(type = "number | null")]
    pub line: Option<i64>,
    pub severity: Severity,
    pub kind: CodeFindingKind,
    pub finding: String,
    pub suggestion: String,
}

/// Token cost of one analysis run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisUsage {
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub turns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub pr_id: String,
    pub head_sha: String,
    pub level: AnalysisLevel,
    /// For Component/Code levels: the node the user drilled into.
    #[serde(default)]
    pub focus_node_id: Option<String>,
    pub graph: C4Graph,
    pub assessment: Assessment,
    pub created_at: String,
    /// The agent's exploration steps, for re-reading after the fact.
    #[serde(default)]
    pub trace: Vec<TraceStep>,
    #[serde(default)]
    pub usage: AnalysisUsage,
    /// Second-stage line-anchored findings (context level only).
    #[serde(default)]
    pub code_findings: Vec<CodeFinding>,
    /// How the code pass ended: "ok" | "failed: <reason>" | "off". None on
    /// analyses that predate the field or on drilled levels — an empty
    /// findings list is only trustworthy when this says "ok".
    #[serde(default)]
    pub code_pass: Option<String>,
}

/// Streaming progress for the UI ("reading src/payments/…").
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProgress {
    pub pr_id: String,
    pub level: AnalysisLevel,
    /// Drill focus node id; empty for the root run. Lets the UI key
    /// concurrent runs of the same level precisely.
    pub focus: String,
    pub message: String,
}

/// Coarse failure classes the UI can build recovery flows around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum AnalysisErrorKind {
    AwsAuth,
    GithubAuth,
    Other,
}

pub fn classify_error(message: &str) -> AnalysisErrorKind {
    let lower = message.to_lowercase();
    if lower.contains("github token") {
        return AnalysisErrorKind::GithubAuth;
    }
    if lower.contains("credential")
        || lower.contains("sso")
        || lower.contains("expired")
        || lower.contains("dispatch failure")
        || lower.contains("unauthorized")
        || lower.contains("access denied")
        || lower.contains("security token")
        || lower.contains("forbidden")
    {
        return AnalysisErrorKind::AwsAuth;
    }
    AnalysisErrorKind::Other
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisError {
    pub pr_id: String,
    pub level: AnalysisLevel,
    /// Drill focus node id; empty for the root run.
    pub focus: String,
    pub error: String,
    pub kind: AnalysisErrorKind,
}

// -- assistant chat -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ChatItemKind {
    /// A message the user typed.
    User,
    /// Assistant prose (markdown).
    Assistant,
    /// A research step the assistant took ("reading src/…").
    Tool,
    /// An app action executed after user confirmation.
    Action,
    Error,
}

/// One entry in the assistant conversation transcript.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatItem {
    pub at: String,
    pub kind: ChatItemKind,
    pub text: String,
}

/// A mutating app action the assistant wants to take — held until the user
/// confirms, so every outward effect stays a human decision.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatPendingAction {
    pub name: String,
    /// One-line human description ("comment on src/api.ts:41").
    pub summary: String,
    /// The full text/body the action would post, verbatim — this is exactly
    /// what gets sent, so the panel can offer it for editing before it runs.
    pub detail: String,
    /// Whether `detail` is text the user may rewrite before confirming.
    /// False for actions that carry no prose (merge, close, resolve).
    pub editable: bool,
}

/// Snapshot of a PR's chat session for (re)mounting the panel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatTranscript {
    pub items: Vec<ChatItem>,
    pub busy: bool,
    pub pending: Option<ChatPendingAction>,
}

/// Where a slice of context sits in the request the model receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ContextGroup {
    /// The system prompt: instructions, PR facts, the analysis.
    System,
    /// Tool definitions — names, descriptions and input schemas.
    Tools,
    /// The conversation: your messages, the model's, and tool results.
    Messages,
}

/// One labelled slice of the model's context, with the exact text that
/// occupies it. Nothing here is a summary — `text` is what gets sent.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ContextPart {
    pub id: String,
    pub group: ContextGroup,
    /// Human label: "PR facts", "call get_file", "get_file → src/api.ts".
    pub label: String,
    /// Where the bytes came from: repo file, github, analysis, you, model…
    pub origin: String,
    #[ts(type = "number")]
    pub chars: i64,
    /// Estimated tokens (~4 chars each). The real count for the whole
    /// request is in `usage` — only Bedrock can give an exact number.
    #[ts(type = "number")]
    pub est_tokens: i64,
    /// The verbatim text. Empty when the caller asked for sizes only.
    pub text: String,
}

/// What the last request to the model actually cost, straight from Bedrock.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    /// The whole prompt Bedrock counted: `input_tokens` plus both cache
    /// buckets. This is "how big is the context", and the number to
    /// calibrate estimates against.
    #[ts(type = "number")]
    pub prompt_tokens: i64,
    /// Billed at full rate — the part of the prompt the cache did NOT serve.
    /// On a warm cache this is a handful of tokens, not the context size.
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    /// Prefix served from the prompt cache — in context, but cheap.
    #[ts(type = "number")]
    pub cache_read_tokens: i64,
    #[ts(type = "number")]
    pub cache_write_tokens: i64,
}

/// Everything the assistant will see on its next turn, itemised.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatContext {
    pub pr_id: String,
    pub model_id: String,
    pub parts: Vec<ContextPart>,
    /// Best answer to "what will the next request carry": the raw estimate
    /// scaled by how far it missed on the last measured request.
    #[ts(type = "number")]
    pub projected_tokens: i64,
    /// The uncalibrated ~4-chars-per-token sum of `parts`.
    #[ts(type = "number")]
    pub est_tokens: i64,
    /// The model's context window, 0 when we don't know it for this id.
    #[ts(type = "number")]
    pub window_tokens: i64,
    /// Measured cost of the most recent request, when one has been made.
    pub usage: Option<ChatUsage>,
    /// Requests made in this session (a turn with tool calls makes several).
    #[ts(type = "number")]
    pub requests: i64,
}

/// Payload for chat:item / chat:state events.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatEvent {
    pub pr_id: String,
    pub item: Option<ChatItem>,
    pub busy: bool,
    pub pending: Option<ChatPendingAction>,
}

pub mod events {
    pub const ANALYSIS_PROGRESS: &str = "analysis:progress";
    pub const ANALYSIS_COMPLETE: &str = "analysis:complete";
    pub const ANALYSIS_ERROR: &str = "analysis:error";
    pub const CHAT_EVENT: &str = "chat:event";
}

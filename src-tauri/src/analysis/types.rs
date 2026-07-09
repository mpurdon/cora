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

/// Model-assigned per-file review guidance: where to spend attention.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPlanEntry {
    pub path: String,
    /// critical | important | mechanical
    pub significance: String,
    #[serde(default)]
    pub reason: String,
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
}

/// Streaming progress for the UI ("reading src/payments/…").
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProgress {
    pub pr_id: String,
    pub level: AnalysisLevel,
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
    pub error: String,
    pub kind: AnalysisErrorKind,
}

pub mod events {
    pub const ANALYSIS_PROGRESS: &str = "analysis:progress";
    pub const ANALYSIS_COMPLETE: &str = "analysis:complete";
    pub const ANALYSIS_ERROR: &str = "analysis:error";
}

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
    pub label: String,
    #[serde(default)]
    pub protocol: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    /// What this change *is*, in system terms — 2-4 sentences.
    pub summary: String,
    pub fit: FitVerdict,
    pub fit_rationale: String,
    /// Ordered most-important-first; external impacts always lead.
    pub boundary_impacts: Vec<BoundaryImpact>,
    pub well_architected: Vec<WaFinding>,
    /// What a reviewer without full context needs to know.
    pub context_notes: Vec<String>,
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

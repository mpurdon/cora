use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a PR entered the tracked set. A PR can match several scopes at once.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum PrSource {
    ReviewRequested,
    Authored,
    Involved,
    WatchedRepo,
    Chat,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Label {
    pub name: String,
    pub color: String,
}

/// State of a PR as reported by GitHub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrInfo {
    /// GraphQL node id — stable across renames/pushes.
    pub id: String,
    #[ts(type = "number")]
    pub number: i64,
    pub title: String,
    pub url: String,
    /// "owner/name"
    pub repo: String,
    pub author: String,
    pub is_draft: bool,
    /// OPEN | MERGED | CLOSED
    pub state: String,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED
    pub review_decision: Option<String>,
    /// SUCCESS | FAILURE | ERROR | PENDING | EXPECTED
    pub ci_status: Option<String>,
    /// MERGEABLE | CONFLICTING | UNKNOWN
    pub mergeable: String,
    #[ts(type = "number")]
    pub additions: i64,
    #[ts(type = "number")]
    pub deletions: i64,
    #[ts(type = "number")]
    pub changed_files: i64,
    /// Issue + review comments; drives the "new comments" attention signal.
    #[serde(default)]
    #[ts(type = "number")]
    pub total_comments: i64,
    /// Logins of the most recent commenters, oldest first; bot accounts are
    /// normalized to end in "[bot]" so change detection can skip automation.
    #[serde(default)]
    pub recent_comment_authors: Vec<String>,
    pub head_sha: String,
    pub updated_at: String,
    pub labels: Vec<Label>,
}

/// What changed between two observations of the same PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    New,
    CiChanged,
    ReviewChanged,
    NewCommits,
    NewComments,
    TitleChanged,
    Merged,
    Closed,
    Reopened,
    DraftChanged,
}

/// A PR plus Cora-local tracking state. This is what both windows render.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TrackedPr {
    #[serde(flatten)]
    pub info: PrInfo,
    pub sources: Vec<PrSource>,
    pub muted: bool,
    #[serde(default = "default_pr_priority")]
    pub priority: PrPriority,
    /// Unacknowledged changes, newest last.
    pub unread: Vec<ChangeKind>,
    pub first_seen: String,
    pub last_change_at: String,
}

/// Per-PR attention weighting, set from the PR tree's context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum PrPriority {
    High,
    Normal,
    Low,
}

impl PrPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            PrPriority::High => "high",
            PrPriority::Normal => "normal",
            PrPriority::Low => "low",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "high" => PrPriority::High,
            "low" => PrPriority::Low,
            _ => PrPriority::Normal,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReactionGroup {
    /// GitHub reaction content: THUMBS_UP, HEART, ROCKET, …
    pub content: String,
    #[ts(type = "number")]
    pub count: i64,
    pub viewer_has_reacted: bool,
}

/// A single comment on a PR (conversation or review thread).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrComment {
    pub id: String,
    pub author: String,
    #[serde(default)]
    pub is_bot: bool,
    pub body: String,
    pub created_at: String,
    pub url: String,
    #[serde(default)]
    pub reactions: Vec<ReactionGroup>,
}

/// A review thread anchored to code.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: Option<String>,
    #[ts(type = "number | null")]
    pub line: Option<i64>,
    #[ts(type = "number | null")]
    pub start_line: Option<i64>,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<PrComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrConversation {
    /// Top-level conversation comments, oldest first.
    pub comments: Vec<PrComment>,
    /// Code-anchored review threads.
    pub threads: Vec<ReviewThread>,
}

/// Per-repo attention weighting. Ignored repos are never tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum RepoPriority {
    High,
    Normal,
    Low,
    Ignored,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub watched_repos: Vec<String>,
    /// "owner/name" → priority; absent means Normal.
    #[serde(default)]
    pub repo_priorities: std::collections::HashMap<String, RepoPriority>,
    #[ts(type = "number")]
    pub poll_interval_secs: u64,
    /// Show the always-on-top callout window when the app starts.
    #[serde(default = "default_true")]
    pub show_callout_on_startup: bool,
    pub github_graphql_url: String,
    pub aws_profile: String,
    /// Explicit region override; some SSO profiles don't carry one.
    #[serde(default = "default_aws_region")]
    pub aws_region: String,
    pub aws_endpoint_url: String,
    /// Bedrock model id — accepts application-inference-profile ARNs.
    pub bedrock_model_id: String,
    /// Unlocks the Developer settings pane (logs, prompt editing, internals).
    #[serde(default)]
    pub developer_mode: bool,
    /// Overrides the analysis system prompt when non-empty.
    #[serde(default)]
    pub custom_system_prompt: String,
}

fn default_aws_region() -> String {
    "us-east-2".into()
}

fn default_true() -> bool {
    true
}

fn default_pr_priority() -> PrPriority {
    PrPriority::Normal
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            watched_repos: Vec::new(),
            repo_priorities: std::collections::HashMap::new(),
            poll_interval_secs: 45,
            show_callout_on_startup: true,
            github_graphql_url: "https://api.github.com/graphql".into(),
            aws_profile: "claude-code-bedrock".into(),
            aws_region: default_aws_region(),
            aws_endpoint_url: String::new(),
            bedrock_model_id:
                "arn:aws:bedrock:us-east-2:000000000000:application-inference-profile/abcd1234efgh"
                    .into(),
            developer_mode: false,
            custom_system_prompt: String::new(),
        }
    }
}

/// One user-taken action, recorded for the History view. `old_value` is what
/// undo restores.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    #[ts(type = "number")]
    pub id: i64,
    pub at: String,
    /// muted | unmuted | untracked | tracked | pr-priority | repo-priority
    pub action: String,
    pub subject_id: String,
    /// Human-readable subject ("owner/repo#123 — title" or "owner/repo").
    pub subject_label: String,
    pub old_value: String,
    pub new_value: String,
    pub undone: bool,
}

/// Emitted after every poll cycle so the UI can show connection health.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PollStatus {
    pub ok: bool,
    pub message: Option<String>,
    pub at: String,
    #[ts(type = "number")]
    pub rate_limit_remaining: Option<i64>,
}

/// Payload for the `pr:changed` event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrChangedEvent {
    pub pr: TrackedPr,
    pub changes: Vec<ChangeKind>,
}

pub mod events {
    /// Full tracked-set snapshot: `Vec<TrackedPr>`.
    pub const PRS_SNAPSHOT: &str = "prs:snapshot";
    /// Single PR delta: `PrChangedEvent`.
    pub const PR_CHANGED: &str = "pr:changed";
    /// Poll cycle health: `PollStatus`.
    pub const POLL_STATUS: &str = "poll:status";
    /// Main window should focus a PR: payload is the PR node id.
    pub const FOCUS_PR: &str = "focus:pr";
}

/// Are any of the `delta` newest commenters human? Automation (SonarQube,
/// CI bots) shouldn't light up the attention signal. When we can't see far
/// enough back, assume human rather than silently dropping a real comment.
fn has_human_among_latest(authors: &[String], delta: usize) -> bool {
    if authors.is_empty() || delta > authors.len() {
        return true;
    }
    authors[authors.len() - delta..]
        .iter()
        .any(|a| !a.ends_with("[bot]"))
}

/// Pure diff between two observations; drives events and unread badges.
pub fn compute_changes(old: &PrInfo, new: &PrInfo) -> Vec<ChangeKind> {
    let mut changes = Vec::new();
    if old.state != new.state {
        match new.state.as_str() {
            "MERGED" => changes.push(ChangeKind::Merged),
            "CLOSED" => changes.push(ChangeKind::Closed),
            "OPEN" => changes.push(ChangeKind::Reopened),
            _ => {}
        }
    }
    if old.ci_status != new.ci_status {
        changes.push(ChangeKind::CiChanged);
    }
    if old.review_decision != new.review_decision {
        changes.push(ChangeKind::ReviewChanged);
    }
    if old.head_sha != new.head_sha {
        changes.push(ChangeKind::NewCommits);
    }
    // `old == 0 && new > 1` is almost always the one-time migration blip from
    // rows stored before comment tracking existed — don't spam on upgrade.
    if new.total_comments > old.total_comments
        && !(old.total_comments == 0 && new.total_comments > 1)
        && has_human_among_latest(
            &new.recent_comment_authors,
            (new.total_comments - old.total_comments) as usize,
        )
    {
        changes.push(ChangeKind::NewComments);
    }
    if old.title != new.title {
        changes.push(ChangeKind::TitleChanged);
    }
    if old.is_draft != new.is_draft {
        changes.push(ChangeKind::DraftChanged);
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrInfo {
        PrInfo {
            id: "PR_1".into(),
            number: 1,
            title: "t".into(),
            url: "u".into(),
            repo: "o/r".into(),
            author: "a".into(),
            is_draft: false,
            state: "OPEN".into(),
            review_decision: Some("REVIEW_REQUIRED".into()),
            ci_status: Some("PENDING".into()),
            mergeable: "MERGEABLE".into(),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            total_comments: 0,
            recent_comment_authors: vec![],
            head_sha: "abc".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            labels: vec![],
        }
    }

    #[test]
    fn no_changes_for_identical() {
        assert!(compute_changes(&pr(), &pr()).is_empty());
    }

    #[test]
    fn detects_merge_ci_and_push() {
        let old = pr();
        let mut new = pr();
        new.state = "MERGED".into();
        new.ci_status = Some("SUCCESS".into());
        new.head_sha = "def".into();
        let changes = compute_changes(&old, &new);
        assert!(changes.contains(&ChangeKind::Merged));
        assert!(changes.contains(&ChangeKind::CiChanged));
        assert!(changes.contains(&ChangeKind::NewCommits));
        assert_eq!(changes.len(), 3);
    }
}

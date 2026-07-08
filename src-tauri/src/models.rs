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
    /// Unacknowledged changes, newest last.
    pub unread: Vec<ChangeKind>,
    pub first_seen: String,
    pub last_change_at: String,
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
    pub github_graphql_url: String,
    pub aws_profile: String,
    /// Explicit region override; some SSO profiles don't carry one.
    #[serde(default = "default_aws_region")]
    pub aws_region: String,
    pub aws_endpoint_url: String,
    /// Bedrock model id — accepts application-inference-profile ARNs.
    pub bedrock_model_id: String,
}

fn default_aws_region() -> String {
    "us-east-2".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            watched_repos: Vec::new(),
            repo_priorities: std::collections::HashMap::new(),
            poll_interval_secs: 45,
            github_graphql_url: "https://api.github.com/graphql".into(),
            aws_profile: "claude-code-bedrock".into(),
            aws_region: default_aws_region(),
            aws_endpoint_url: String::new(),
            bedrock_model_id:
                "arn:aws:bedrock:us-east-2:000000000000:application-inference-profile/abcd1234efgh"
                    .into(),
        }
    }
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

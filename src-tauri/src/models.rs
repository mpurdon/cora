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
    /// YOUR latest review's state (APPROVED | CHANGES_REQUESTED | COMMENTED…).
    pub my_review_state: Option<String>,
    pub my_reviewed_at: Option<String>,
    /// Someone re-requested your review after you reviewed — always resurface.
    #[serde(default)]
    pub my_review_rerequested: bool,
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
    /// The most recent comments, oldest first — drives the human-comment
    /// attention signal and reply notifications.
    #[serde(default)]
    pub recent_comments: Vec<RecentComment>,
    pub head_sha: String,
    pub updated_at: String,
    pub labels: Vec<Label>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RecentComment {
    pub id: String,
    pub author: String,
    pub is_bot: bool,
    /// First ~120 chars, for notification bodies.
    pub snippet: String,
}

/// Where the reviewer left off on a PR — the head SHA at their last look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMark {
    pub head_sha: String,
    pub at: String,
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
    /// Whether the PAT owner wrote this — the UI only offers Edit on your own.
    #[serde(default)]
    pub viewer_can_edit: bool,
    /// A review-thread comment edits through a different GitHub mutation than
    /// a conversation comment, so the frontend has to tell them apart.
    #[serde(default)]
    pub is_review_comment: bool,
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
    /// Review verdicts (approve / request changes) with their summary bodies —
    /// these live on the review object, not in comments or threads.
    #[serde(default)]
    pub reviews: Vec<ReviewVerdict>,
}

/// A submitted review's verdict + summary text, shown in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewVerdict {
    pub id: String,
    pub author: String,
    /// APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED
    pub state: String,
    pub body: String,
    pub submitted_at: String,
    pub url: String,
}

/// One commit in a PR's history tab.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrCommit {
    pub sha: String,
    pub short_sha: String,
    /// Headline only; full message stays on GitHub.
    pub message: String,
    pub author: String,
    pub at: String,
    #[ts(type = "number")]
    pub additions: i64,
    #[ts(type = "number")]
    pub deletions: i64,
    /// SUCCESS | FAILURE | ERROR | PENDING | EXPECTED — of this commit's checks.
    pub ci_status: Option<String>,
    pub url: String,
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

impl RepoPriority {
    /// None for anything unrecognised — the undo path stores an empty string
    /// to mean "there was no entry", and callers that need strictness say so.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "high" => Some(RepoPriority::High),
            "normal" => Some(RepoPriority::Normal),
            "low" => Some(RepoPriority::Low),
            "ignored" => Some(RepoPriority::Ignored),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub watched_repos: Vec<String>,
    /// "owner/name" → priority; absent means Normal.
    #[serde(default)]
    pub repo_priorities: std::collections::HashMap<String, RepoPriority>,
    /// PR author login → priority; absent means Normal. Ignored authors'
    /// PRs (dependabot…) never enter tracking; High authors' activity is
    /// always important.
    #[serde(default)]
    pub author_priorities: std::collections::HashMap<String, RepoPriority>,
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
    /// Cheaper/faster model for Component/Code drill-downs; empty = use main.
    #[serde(default = "default_drill_model")]
    pub bedrock_drill_model_id: String,
    /// Dollars per million tokens, per model id, for the usage dashboard.
    /// Inference-profile ARNs name no model, so their rate can only be told
    /// to us; recognizable Claude ids fall back to published rates.
    #[serde(default)]
    pub model_prices: Vec<ModelPrice>,
    /// Unlocks the Developer settings pane (logs, prompt editing, internals).
    #[serde(default)]
    pub developer_mode: bool,
    /// Overrides the analysis system prompt when non-empty.
    #[serde(default)]
    pub custom_system_prompt: String,
    /// Glob patterns for insignificant files — auto-skipped in diff review.
    #[serde(default = "default_ignore_globs")]
    pub review_ignore_globs: Vec<String>,
    /// Pre-warm: auto-run L1 analysis when a PR enters the review queue.
    #[serde(default = "default_true")]
    pub auto_analyze_review_requests: bool,
    /// Spend guard for pre-warming.
    #[serde(default = "default_auto_analyze_cap")]
    #[ts(type = "number")]
    pub auto_analyze_daily_cap: u64,
    /// Team knowledge no diff reveals — design-system packages, shared
    /// libraries, review standards. Injected into analysis and chat prompts.
    #[serde(default)]
    pub review_conventions: String,
    /// Second analysis stage: line-anchored defect + reuse findings over the
    /// review plan's critical/important files.
    #[serde(default = "default_true")]
    pub code_findings_pass: bool,
    /// PRs with no activity inside this window are hidden from the list and
    /// excluded from search discovery. 0 disables the filter.
    #[serde(default = "default_pr_max_age_days")]
    #[ts(type = "number")]
    pub pr_max_age_days: u64,
    /// Poll cadence when this org is NOT the active one in the org selector
    /// — background awareness at a gentler rate than the active org.
    #[serde(default = "default_background_poll_secs")]
    #[ts(type = "number")]
    pub background_poll_secs: u64,
    /// Output-token ceiling for the architecture pass (graph + assessment +
    /// Well-Architected pillar findings, all in one submission — the large
    /// output). A ceiling, not a reservation: costs nothing unless the model
    /// generates that many. Raise only as high as your configured Bedrock
    /// model's hard output cap allows — set above it and Bedrock errors.
    #[serde(default = "default_arch_max_tokens")]
    #[ts(type = "number")]
    pub arch_max_output_tokens: u64,
    /// Output-token ceiling for the code-level findings pass. Its submission
    /// is small; the budget is mostly the model's reasoning before it calls
    /// submit_code_findings. Same caveat as the architecture ceiling.
    #[serde(default = "default_code_max_tokens")]
    #[ts(type = "number")]
    pub code_max_output_tokens: u64,
    /// How many activity rows the callout shows — a query cap only. Older
    /// rows stay in the local database (kept up to a high runaway guard, not
    /// deleted), so shrinking the feed just makes it easier to reach the
    /// bottom; it never throws away history.
    #[serde(default = "default_callout_feed_limit")]
    #[ts(type = "number")]
    pub callout_feed_limit: u64,
    /// Approve-review body seeded when nothing else summarizes the review.
    /// Empty/absent falls back to the hardcoded default.
    #[serde(default)]
    pub default_approve_message: Option<String>,
    /// "owner/name" → approve message, overriding `default_approve_message`.
    #[serde(default)]
    pub repo_approve_messages: std::collections::HashMap<String, String>,
    /// "owner/name" → extra review instructions appended to analysis and
    /// chat prompts for that repo only.
    #[serde(default)]
    pub repo_review_instructions: std::collections::HashMap<String, String>,
}

fn default_callout_feed_limit() -> u64 {
    150
}

fn default_arch_max_tokens() -> u64 {
    16384
}

fn default_code_max_tokens() -> u64 {
    16384
}

fn default_background_poll_secs() -> u64 {
    300
}

/// A GitHub org (or the viewer's personal account) the PAT can see.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GithubOrg {
    pub login: String,
    pub name: String,
    /// True for the viewer's own account row.
    pub personal: bool,
}

/// Registry snapshot for the org selector: which orgs are on, which is
/// active, and each org's unread-feed count for dropdown badges.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OrgState {
    pub active: String,
    pub enabled: Vec<String>,
    #[ts(type = "Record<string, number>")]
    pub unread: std::collections::HashMap<String, i64>,
}

fn default_auto_analyze_cap() -> u64 {
    15
}

fn default_ignore_globs() -> Vec<String> {
    [
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "go.sum",
        "poetry.lock",
        "Gemfile.lock",
        "composer.lock",
        "*.snap",
        "**/__snapshots__/**",
        "**/generated/**",
        "*.generated.*",
        "**/dist/**",
        "**/build/**",
        "*.min.js",
        "*.map",
        "**/vendor/**",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_aws_region() -> String {
    "us-east-2".into()
}

fn default_true() -> bool {
    true
}

fn default_drill_model() -> String {
    // Drill-downs analyze code, not system-wide architecture, so the faster
    // tier fits. A cross-region inference profile id, like the main model:
    // it works for any account with Bedrock access, where an application
    // inference profile ARN would only work for the account that owns it.
    "us.anthropic.claude-sonnet-5".into()
}

fn default_pr_priority() -> PrPriority {
    PrPriority::Normal
}

fn default_pr_max_age_days() -> u64 {
    // Top of the settings ladder — effectively "show everything" so the
    // filter is opt-in rather than a surprise on upgrade.
    365
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            watched_repos: Vec::new(),
            repo_priorities: std::collections::HashMap::new(),
            author_priorities: std::collections::HashMap::new(),
            poll_interval_secs: 45,
            show_callout_on_startup: true,
            github_graphql_url: "https://api.github.com/graphql".into(),
            aws_profile: "claude-code-bedrock".into(),
            aws_region: default_aws_region(),
            aws_endpoint_url: String::new(),
            bedrock_model_id: "us.anthropic.claude-opus-5".into(),
            bedrock_drill_model_id: default_drill_model(),
            model_prices: Vec::new(),
            developer_mode: false,
            custom_system_prompt: String::new(),
            review_ignore_globs: default_ignore_globs(),
            auto_analyze_review_requests: true,
            auto_analyze_daily_cap: default_auto_analyze_cap(),
            review_conventions: String::new(),
            code_findings_pass: true,
            pr_max_age_days: default_pr_max_age_days(),
            background_poll_secs: default_background_poll_secs(),
            arch_max_output_tokens: default_arch_max_tokens(),
            code_max_output_tokens: default_code_max_tokens(),
            callout_feed_limit: default_callout_feed_limit(),
            default_approve_message: None,
            repo_approve_messages: std::collections::HashMap::new(),
            repo_review_instructions: std::collections::HashMap::new(),
        }
    }
}

/// Who's been asked to review and what reviews exist — shown before analyzing.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummary {
    pub author: String,
    /// APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED | PENDING
    pub state: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrReviews {
    /// Logins/teams still requested to review.
    pub requested: Vec<String>,
    /// Latest review per reviewer.
    pub reviews: Vec<ReviewSummary>,
    /// The PAT owner's login — identifies "my" review.
    #[serde(default)]
    pub viewer_login: String,
    /// When the head commit landed; newer than my review = re-enable actions.
    #[serde(default)]
    pub last_commit_at: Option<String>,
    #[serde(default)]
    #[ts(type = "number")]
    pub open_threads: i64,
    /// Unresolved threads the viewer started — approving over your own open
    /// questions is almost always a mistake.
    #[serde(default)]
    #[ts(type = "number")]
    pub my_open_threads: i64,
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
    /// A poll cycle is in flight right now ("refreshing…" feedback).
    #[serde(default)]
    pub syncing: bool,
}

/// Payload for the `pr:changed` event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrChangedEvent {
    pub pr: TrackedPr,
    pub changes: Vec<ChangeKind>,
}

/// What one model id costs, in dollars per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    /// Exact model id or ARN as configured — matched literally.
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Payload for the `review:submitted` event — the review GitHub just created,
/// carried to the UI because GitHub's own read-back lags the mutation.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSubmittedEvent {
    pub pr_id: String,
    pub verdict: ReviewVerdict,
}

/// One row in the callout's activity feed: something happened on a PR the
/// reviewer cares about. Written by the poller as it detects changes;
/// read/flag state is the reviewer's own.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    #[ts(type = "number")]
    pub id: i64,
    pub at: String,
    pub pr_id: String,
    pub repo: String,
    #[ts(type = "number")]
    pub number: i64,
    pub pr_title: String,
    /// comment | review | commits | ci | merged | closed | reopened | new | ready
    pub kind: String,
    /// GitHub login behind the event, when known ("" otherwise).
    pub actor: String,
    /// Human line for the feed, e.g. `responded: "looks good but…"`.
    pub summary: String,
    /// Comment to deep-link to, when the event is a comment.
    pub comment_id: String,
    /// Authored-by-me PRs and high-priority repos/PRs are always important.
    pub important: bool,
    pub read: bool,
    /// "" | "must-review" | "follow-up"
    pub flag: String,
}

/// Assistant-initiated viewed-marking. The frontend owns diff parsing and
/// per-file digests, so Rust only announces intent and the UI applies it
/// through the same path as a manual checkbox.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MarkViewedEvent {
    pub pr_id: String,
    /// Ignored when `all` is set.
    pub paths: Vec<String>,
    pub all: bool,
    pub viewed: bool,
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
    /// Assistant asks the UI to mark diff files viewed: `MarkViewedEvent`.
    pub const MARK_VIEWED: &str = "viewed:mark";
    /// The activity feed changed (new rows, read/flag updates): no payload —
    /// consumers refetch.
    pub const ACTIVITY_CHANGED: &str = "activity:changed";
    /// Active org switched — frontend stores reset and refetch.
    pub const ORG_CHANGED: &str = "org:changed";
    /// Review/thread state changed (resolve, new diff comment, submitted
    /// review, refresh): the approve gate should refetch. No payload.
    pub const REVIEWS_CHANGED: &str = "reviews:changed";
    /// You submitted a review: `ReviewSubmittedEvent`. Always emitted before
    /// `REVIEWS_CHANGED`, so the UI can show the verdict before the refetch
    /// that may not yet know about it.
    pub const REVIEW_SUBMITTED: &str = "review:submitted";
}

/// The login-only half of bot detection (GitHub App logins end in "[bot]").
/// Callers with the author's GraphQL `__typename` in hand should OR this
/// with a `== "Bot"` check, which also catches suffix-less bots.
pub fn is_bot_login(login: &str) -> bool {
    login.ends_with("[bot]")
}

/// A review comment that opens a thread but shouldn't demand resolution:
/// conventional-comments style `praise:` / `note:` / `fyi:` prefixes, or an
/// explicit `(non-blocking)` decoration on the first line. Used to exclude
/// the viewer's own threads from the approve gate.
pub fn is_non_blocking_comment(body: &str) -> bool {
    // Normalize markdown emphasis so `**praise:**` and `praise:` read alike.
    let first: String = body
        .trim_start()
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`' | '~'))
        .collect::<String>()
        .trim_start()
        .to_lowercase();
    first.starts_with("praise:")
        || first.starts_with("note:")
        || first.starts_with("fyi:")
        || first.contains("(non-blocking)")
        || first.contains("non-blocking:")
}

/// The newest human comment among the `delta` most recent, if any.
/// Automation (SonarQube, CI bots) shouldn't light up the attention signal.
/// When we can't see far enough back, fall back to the newest human we have —
/// better a slightly-off notification than a silently dropped one.
pub fn latest_human_comment(comments: &[RecentComment], delta: usize) -> Option<&RecentComment> {
    let window = if delta > comments.len() {
        comments
    } else {
        &comments[comments.len() - delta..]
    };
    window.iter().rev().find(|c| !c.is_bot)
}

/// The feed kind recording how a PR ended, for the states that end one.
/// `None` for OPEN — and for anything unrecognised, which must never be
/// mistaken for finished: that would retire a live PR's unread rows and hide it
/// from the rail. Lives here, beside `compute_changes` and the `state` field it
/// classifies, so the poller, the IPC layer and the store all read one rule.
pub fn terminal_kind(state: &str) -> Option<&'static str> {
    match state {
        "CLOSED" => Some("closed"),
        "MERGED" => Some("merged"),
        _ => None,
    }
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
        // No visibility into recent authors → assume human rather than drop.
        && (new.recent_comments.is_empty()
            || latest_human_comment(
                &new.recent_comments,
                (new.total_comments - old.total_comments) as usize,
            )
            .is_some())
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

    #[test]
    fn only_closed_and_merged_are_terminal() {
        assert_eq!(terminal_kind("CLOSED"), Some("closed"));
        assert_eq!(terminal_kind("MERGED"), Some("merged"));
        assert_eq!(terminal_kind("OPEN"), None);
        // An unrecognised state must never read as finished — that would retire
        // a live PR's unread rows and hide it behind the chip.
        assert_eq!(terminal_kind(""), None);
        assert_eq!(terminal_kind("closed"), None);
        assert_eq!(terminal_kind("DRAFT"), None);
    }

    #[test]
    fn non_blocking_comment_detection() {
        assert!(is_non_blocking_comment("praise: clean refactor!"));
        assert!(is_non_blocking_comment("**praise:** nice idea!"));
        assert!(is_non_blocking_comment("**Note**: for a future PR"));
        assert!(is_non_blocking_comment("FYI: this moves in v2"));
        assert!(is_non_blocking_comment("nit (non-blocking): rename?"));
        assert!(is_non_blocking_comment("suggestion, non-blocking: could memoize"));
        assert!(!is_non_blocking_comment("this breaks retries on 429"));
        assert!(!is_non_blocking_comment("nit: rename this"));
        assert!(!is_non_blocking_comment("question: why drop the lock?"));
    }

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
            my_review_state: None,
            my_reviewed_at: None,
            my_review_rerequested: false,
            ci_status: Some("PENDING".into()),
            mergeable: "MERGEABLE".into(),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            total_comments: 0,
            recent_comments: vec![],
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

    /// Persisted settings JSON predating the approve-message fields must still
    /// load — no migration exists, so absence has to mean "unset", not a parse
    /// failure that locks the user out of their own settings.
    #[test]
    fn settings_without_approve_message_fields_deserializes() {
        let json = r#"{
            "watchedRepos": [],
            "repoPriorities": {},
            "authorPriorities": {},
            "pollIntervalSecs": 45,
            "showCalloutOnStartup": true,
            "githubGraphqlUrl": "https://api.github.com/graphql",
            "awsProfile": "default",
            "awsRegion": "us-east-2",
            "awsEndpointUrl": "",
            "bedrockModelId": "us.anthropic.claude-opus-5",
            "bedrockDrillModelId": "us.anthropic.claude-sonnet-5",
            "modelPrices": [],
            "developerMode": false,
            "customSystemPrompt": "",
            "reviewIgnoreGlobs": [],
            "autoAnalyzeReviewRequests": true,
            "autoAnalyzeDailyCap": 15,
            "reviewConventions": "",
            "codeFindingsPass": true,
            "prMaxAgeDays": 365,
            "backgroundPollSecs": 300,
            "archMaxOutputTokens": 16384,
            "codeMaxOutputTokens": 16384,
            "calloutFeedLimit": 150
        }"#;
        let settings: Settings = serde_json::from_str(json).expect("must deserialize without a migration");
        assert_eq!(settings.default_approve_message, None);
        assert!(settings.repo_approve_messages.is_empty());
        assert!(settings.repo_review_instructions.is_empty());
    }
}

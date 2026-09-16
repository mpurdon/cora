//! Writing the callout's activity feed.
//!
//! Two observers can be first to see a PR move: the poll loop, and an
//! on-demand refresh (the ⟳ button, a merge/close, an assistant action).
//! Whichever gets there first writes the row — and once it has upserted the
//! new snapshot the other computes no change and stays silent. So they have
//! to agree on what a change looks like in the feed, which is why the mapping
//! lives here rather than inside the poller.

use std::sync::Arc;

use crate::models::ChangeKind;
use crate::models::{PrSource, RepoPriority};
use crate::store::{Store, MECHANICAL_KINDS};

/// Turn an observed change into a feed row for the callout's activity log.
/// Importance: your own PRs always matter; so do high-priority repos/PRs.
///
/// `self_acted` marks a change you caused yourself — you merged it, you closed
/// it — so its rows arrive already read: a record, not news. Returns whether
/// anything was written, so the caller knows whether to announce it.
pub fn record(
    store: &Arc<Store>,
    settings: &crate::models::Settings,
    pr: &crate::models::TrackedPr,
    changes: &[ChangeKind],
    last_opinion: Option<&(String, String)>,
    now: &str,
    self_acted: bool,
) -> bool {
    // Bot-authored PRs (dependabot, renovate) churn constantly; their noise
    // stays out of the feed. The tiles and main list still track them.
    if crate::models::is_bot_login(&pr.info.author) {
        return false;
    }
    // Check suppression: unimportant repos/authors/PRs produce zero feed rows.
    let repo_priority = settings.repo_priorities.get(&pr.info.repo).copied().unwrap_or(RepoPriority::Standard);
    let author_priority = settings.author_priorities.get(&pr.info.author).copied().unwrap_or(RepoPriority::Standard);
    if priority::is_suppressed(repo_priority, author_priority, pr.priority) {
        return false;
    }
    let important = pr.sources.contains(&PrSource::Authored)
        || priority::is_important(priority::effective_priority(repo_priority, author_priority, pr.priority));
    // A poll delta has no per-event timestamps, so impose causal order on
    // same-cycle rows: setup → work → verdicts → terminal state. Insertion
    // order is the feed's tie-breaker for identical timestamps.
    let causal_rank = |c: &ChangeKind| match c {
        ChangeKind::New => 0,
        ChangeKind::DraftChanged => 1,
        ChangeKind::NewCommits => 2,
        // A push dismisses the verdict, so the dismissal follows the push.
        ChangeKind::ApprovalDismissed | ChangeKind::ChangesRequestDismissed => 3,
        ChangeKind::CiChanged => 4,
        ChangeKind::NewComments => 5,
        ChangeKind::ReviewChanged => 6,
        ChangeKind::Reopened => 7,
        ChangeKind::TitleChanged => 8,
        ChangeKind::Closed => 9,
        ChangeKind::Merged => 10,
    };
    let mut ordered: Vec<&ChangeKind> = changes.iter().collect();
    ordered.sort_by_key(|c| causal_rank(c));
    let mut wrote = false;
    for change in ordered {
        let (kind, actor, summary, comment_id) = match change {
            ChangeKind::NewComments => {
                let Some(c) = crate::models::latest_human_comment(&pr.info.recent_comments, 5)
                else {
                    continue; // bot chatter doesn't belong in the feed
                };
                ("comment", c.author.clone(), format!("commented: “{}”", c.snippet), c.id.clone())
            }
            ChangeKind::ReviewChanged => {
                // Only human verdicts are news. Any other `reviewDecision`
                // transition (null / REVIEW_REQUIRED) is mechanical — reviewers
                // got assigned, or the PR became ready — and is already implied
                // by the "review requested" / "marked ready" rows. Skip it
                // rather than emit an actor-less "review state changed".
                let state = match pr.info.review_decision.as_deref() {
                    Some("APPROVED") => "review approved",
                    Some("CHANGES_REQUESTED") => "changes requested",
                    _ => continue,
                };
                // Attribute the verdict when the newest opinionated review
                // matches the decision (it flipped it, or agrees with it).
                let actor = match (pr.info.review_decision.as_deref(), last_opinion) {
                    (Some(d), Some((author, review_state))) if d == review_state => author.clone(),
                    _ => String::new(),
                };
                ("review", actor, state.to_string(), String::new())
            }
            ChangeKind::NewCommits => {
                ("commits", pr.info.author.clone(), "pushed new commits".into(), String::new())
            }
            // Your verdict stopped counting. The poll delta can't name the
            // dismisser; when it came with a push, the pusher is the author
            // for all practical purposes. Otherwise leave it actor-less
            // rather than guess — the PR header carries the timeline's
            // precise answer.
            ChangeKind::ApprovalDismissed | ChangeKind::ChangesRequestDismissed => {
                let what = if *change == ChangeKind::ApprovalDismissed {
                    "approval"
                } else {
                    "change request"
                };
                let pushed = changes.contains(&ChangeKind::NewCommits);
                let actor = if pushed { pr.info.author.clone() } else { String::new() };
                let summary = if pushed {
                    format!("pushed commits that dismissed your {what} — review again")
                } else {
                    format!("your {what} was dismissed — review again")
                };
                ("dismissed", actor, summary, String::new())
            }
            ChangeKind::CiChanged => match pr.info.ci_status.as_deref() {
                Some("FAILURE") | Some("ERROR") => {
                    ("ci", String::new(), "CI went red".into(), String::new())
                }
                Some("SUCCESS") => {
                    // Green isn't news — but it does make an unread red moot.
                    let _ = store.supersede_activity(&pr.info.id, &["ci"]);
                    continue;
                }
                _ => continue, // pending flapping is noise
            },
            ChangeKind::Merged => ("merged", String::new(), "merged".into(), String::new()),
            ChangeKind::Closed => ("closed", String::new(), "closed".into(), String::new()),
            ChangeKind::Reopened => ("reopened", String::new(), "reopened".into(), String::new()),
            // New PRs feed only when they matter: a direct review ask, or a
            // PR in something marked important. Merely entering the tracked
            // set (involves:, watched) is noise.
            ChangeKind::New if pr.sources.contains(&PrSource::ReviewRequested) => {
                ("new", pr.info.author.clone(), "review requested".into(), String::new())
            }
            ChangeKind::New if important => {
                ("new", pr.info.author.clone(), "opened a PR".into(), String::new())
            }
            ChangeKind::DraftChanged if !pr.info.is_draft => {
                ("ready", pr.info.author.clone(), "marked ready for review".into(), String::new())
            }
            // Title edits and draft flips into draft are noise.
            _ => continue,
        };
        // A newer event marks the unread rows it makes moot as read — never
        // flagged rows, never comments (see supersede_activity). Terminal
        // states retire everything mechanical; repeatable kinds retire their
        // own older siblings. A finished PR retires comments too, but that
        // sweep lives in the caller's terminal-state branch — it has to run
        // whether or not this cycle saw the transition.
        let superseded: &[&str] = match kind {
            "merged" | "closed" | "reopened" => MECHANICAL_KINDS,
            "commits" | "ci" | "review" | "dismissed" => &[kind],
            _ => &[],
        };
        let _ = store.supersede_activity(&pr.info.id, superseded);
        // A reopen also makes the close that preceded it moot — otherwise the
        // terminal row outlives the terminal state, unread, on a live PR.
        if kind == "reopened" {
            let _ = store.supersede_activity(&pr.info.id, &["closed"]);
        }
        // A red CI on someone else's PR is their problem — record it for the
        // history but arrive pre-read. On your own PRs it demands attention.
        let pre_read =
            self_acted || (kind == "ci" && !pr.sources.contains(&PrSource::Authored));
        // A dismissed verdict is about you whatever the PR's priority: it
        // features in the callout so the author never has to ping you.
        let important = important || kind == "dismissed";
        if store
            .add_activity(now, &pr.info, kind, &actor, &summary, &comment_id, important, pre_read)
            .is_ok()
        {
            wrote = true;
        }
    }
    // Once YOU have reviewed (and nobody re-requested), the "review
    // requested" row has served its purpose. Someone else's review never
    // retires it — they still want yours. Nor does a dismissed review of
    // yours: that state means your verdict no longer counts.
    if pr.info.my_review_state.as_deref().is_some_and(|s| s != "DISMISSED")
        && !pr.info.my_review_rerequested
    {
        let _ = store.supersede_activity(&pr.info.id, &["new"]);
    }
    wrote
}

/// Reconciling a PR's repo/author/pr priority dimensions into one verdict,
/// and the predicates poller.rs's notification gating asks of it.
///
/// The three dimensions can disagree — an `Unimportant` repo hosting a
/// `Critical` PR, say — and FR-6/FR-7 require the highest-ranked dimension
/// to win rather than an unimportant one suppressing an otherwise-critical
/// PR. `RepoPriority`/`PrPriority`'s declaration order is already
/// attention-ascending (their derived `Ord` reflects it), so reconciliation
/// is just a `max()` over the three dimensions mapped onto one shared scale.
pub mod priority {
    use crate::models::{PrPriority, RepoPriority};

    /// The reconciled verdict for a PR, attention-ascending like the enums
    /// it's built from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum EffectivePriority {
        Unimportant,
        Someday,
        Standard,
        Important,
        Critical,
    }

    impl From<RepoPriority> for EffectivePriority {
        /// `Ignored` repos never reach this far (they're filtered out of the
        /// tracked set upstream), but if one ever does, it carries no more
        /// weight than `Unimportant`.
        fn from(p: RepoPriority) -> Self {
            match p {
                RepoPriority::Ignored | RepoPriority::Unimportant => EffectivePriority::Unimportant,
                RepoPriority::Someday => EffectivePriority::Someday,
                RepoPriority::Standard => EffectivePriority::Standard,
                RepoPriority::Important => EffectivePriority::Important,
                RepoPriority::Critical => EffectivePriority::Critical,
            }
        }
    }

    impl From<PrPriority> for EffectivePriority {
        fn from(p: PrPriority) -> Self {
            match p {
                PrPriority::Unimportant => EffectivePriority::Unimportant,
                PrPriority::Someday => EffectivePriority::Someday,
                PrPriority::Standard => EffectivePriority::Standard,
                PrPriority::Important => EffectivePriority::Important,
                PrPriority::Critical => EffectivePriority::Critical,
            }
        }
    }

    /// The highest-ranked of the three dimensions wins — a single unimportant
    /// dimension can never drag down an otherwise-critical PR.
    pub fn effective_priority(
        repo: RepoPriority,
        author: RepoPriority,
        pr: PrPriority,
    ) -> EffectivePriority {
        EffectivePriority::from(repo).max(EffectivePriority::from(author)).max(EffectivePriority::from(pr))
    }

    /// Worth surfacing as important attention.
    pub fn is_important(effective: EffectivePriority) -> bool {
        effective >= EffectivePriority::Important
    }

    /// Worth suppressing entirely. A repo/author/pr marked `Unimportant` or
    /// `Ignored` should produce zero notifications — but `Critical` PRs are
    /// always exempt (FR-7). So suppress if any dimension is marked
    /// unimportant, unless the PR itself is Critical.
    pub fn is_suppressed(
        repo: RepoPriority,
        author: RepoPriority,
        pr: PrPriority,
    ) -> bool {
        // PR itself is Unimportant -> always suppress
        if pr == PrPriority::Unimportant {
            return true;
        }
        // Repo is Unimportant/Ignored -> suppress (unless PR is Critical)
        if repo == RepoPriority::Unimportant || repo == RepoPriority::Ignored {
            return pr != PrPriority::Critical;
        }
        // Author is Unimportant/Ignored -> suppress (unless PR is Critical)
        if author == RepoPriority::Unimportant || author == RepoPriority::Ignored {
            return pr != PrPriority::Critical;
        }
        false
    }

    /// The narrower condition that re-arms a persistent critical alert:
    /// both the repo and the PR itself (not just the reconciled effective
    /// priority) have to be `Critical`.
    pub fn is_persistent_critical_condition(repo: RepoPriority, pr: PrPriority) -> bool {
        repo == RepoPriority::Critical && pr == PrPriority::Critical
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unimportant_repo_suppresses_standard_pr() {
            assert!(is_suppressed(RepoPriority::Unimportant, RepoPriority::Standard, PrPriority::Standard));
        }

        #[test]
        fn unimportant_repo_does_not_suppress_critical_pr() {
            assert!(!is_suppressed(RepoPriority::Unimportant, RepoPriority::Standard, PrPriority::Critical));
        }

        #[test]
        fn unimportant_author_suppresses_standard_pr() {
            assert!(is_suppressed(RepoPriority::Standard, RepoPriority::Unimportant, PrPriority::Standard));
        }

        #[test]
        fn unimportant_pr_always_suppresses() {
            assert!(is_suppressed(RepoPriority::Critical, RepoPriority::Critical, PrPriority::Unimportant));
        }

        #[test]
        fn ignored_repo_suppresses_standard_pr() {
            assert!(is_suppressed(RepoPriority::Ignored, RepoPriority::Standard, PrPriority::Standard));
        }

        #[test]
        fn ignored_repo_does_not_suppress_critical_pr() {
            assert!(!is_suppressed(RepoPriority::Ignored, RepoPriority::Standard, PrPriority::Critical));
        }

        #[test]
        fn all_standard_does_not_suppress() {
            assert!(!is_suppressed(RepoPriority::Standard, RepoPriority::Standard, PrPriority::Standard));
        }

        #[test]
        fn persistent_critical_requires_both_repo_and_pr() {
            assert!(is_persistent_critical_condition(RepoPriority::Critical, PrPriority::Critical));
            assert!(!is_persistent_critical_condition(RepoPriority::Critical, PrPriority::Important));
            assert!(!is_persistent_critical_condition(RepoPriority::Important, PrPriority::Critical));
        }

        #[test]
        fn is_important_boundary() {
            assert!(is_important(EffectivePriority::Important));
            assert!(!is_important(EffectivePriority::Standard));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PrInfo, PrPriority, TrackedPr};

    fn info() -> PrInfo {
        PrInfo {
            id: "PR_1".into(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            url: "u".into(),
            repo: "o/r".into(),
            author: "author".into(),
            is_draft: false,
            state: "OPEN".into(),
            review_decision: Some("REVIEW_REQUIRED".into()),
            my_review_state: Some("DISMISSED".into()),
            my_reviewed_at: Some("2026-01-01T00:00:00Z".into()),
            my_review_rerequested: false,
            ci_status: None,
            mergeable: "UNKNOWN".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            total_comments: 0,
            recent_comments: vec![],
            head_sha: "abc".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            labels: vec![],
        }
    }

    /// A tracked PR in a standard repo, requested of you — the case where
    /// nothing else would make its rows important.
    fn tracked(store: &Arc<Store>, info: &PrInfo) -> TrackedPr {
        let now = "2026-01-01T00:00:00Z";
        let mut pr = store
            .upsert_pr(info, &[PrSource::ReviewRequested], &[ChangeKind::New], now)
            .unwrap();
        pr.priority = PrPriority::Standard;
        pr
    }

    #[test]
    fn a_dismissed_approval_features_and_names_the_push() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let settings = crate::models::Settings::default();
        let pr = tracked(&store, &info());
        let now = "2026-01-01T01:00:00Z";
        assert!(record(
            &store,
            &settings,
            &pr,
            &[ChangeKind::ApprovalDismissed, ChangeKind::NewCommits],
            None,
            now,
            false,
        ));
        let rows = store.list_activity(10).unwrap();
        let row = rows.iter().find(|r| r.kind == "dismissed").expect("a dismissed row");
        assert!(row.important, "about you, so it features whatever the repo's priority");
        assert!(!row.read);
        assert_eq!(row.actor, "author");
        assert!(row.summary.contains("dismissed your approval"), "{}", row.summary);
        // Push first, dismissal second: the feed's causal order.
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        let push = kinds.iter().position(|k| *k == "commits").unwrap();
        let dismissed = kinds.iter().position(|k| *k == "dismissed").unwrap();
        assert!(dismissed < push, "newest first, so the dismissal lists above the push: {kinds:?}");
    }

    #[test]
    fn a_manual_dismissal_has_no_actor_to_blame() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let settings = crate::models::Settings::default();
        let pr = tracked(&store, &info());
        let at = "2026-01-01T01:00:00Z";
        record(&store, &settings, &pr, &[ChangeKind::ChangesRequestDismissed], None, at, false);
        let rows = store.list_activity(10).unwrap();
        let row = rows.iter().find(|r| r.kind == "dismissed").unwrap();
        assert_eq!(row.actor, "");
        assert!(row.summary.starts_with("your change request was dismissed"), "{}", row.summary);
    }

    #[test]
    fn a_dismissed_review_does_not_retire_the_review_request() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let settings = crate::models::Settings::default();
        let now = "2026-01-01T00:00:00Z";
        // The "review requested" row, written when the PR was first seen.
        let pr = tracked(&store, &info());
        store
            .add_activity(now, &pr.info, "new", "author", "review requested", "", true, false)
            .unwrap();
        let unread_request = || {
            !store.list_activity(10).unwrap().into_iter().find(|r| r.kind == "new").unwrap().read
        };
        // Any later change on a PR whose review of yours is DISMISSED.
        record(&store, &settings, &pr, &[ChangeKind::CiChanged], None, "2026-01-01T01:00:00Z", false);
        assert!(unread_request(), "your verdict no longer counts, so they still want your review");

        // Whereas a standing verdict of yours does retire it.
        let mut approved = info();
        approved.my_review_state = Some("APPROVED".into());
        let pr = store.upsert_pr(&approved, &[PrSource::ReviewRequested], &[], now).unwrap();
        record(&store, &settings, &pr, &[ChangeKind::CiChanged], None, "2026-01-01T02:00:00Z", false);
        assert!(!unread_request());
    }
}

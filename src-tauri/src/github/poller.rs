use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::error::AppResult;
use crate::github::{parse_pr, query::{split_by_alias, GraphQlClient, PollRequest}};
use crate::models::{
    compute_changes, events, ChangeKind, PollStatus, PrChangedEvent, PrInfo, PrSource,
    RepoPriority,
};
use crate::secrets;
use crate::store::Store;

/// Wake the poller immediately (settings change, PAT set, manual refresh).
pub struct PollTrigger(pub Arc<Notify>);

const MAX_BACKOFF_SECS: u64 = 300;
const LOW_RATE_LIMIT: i64 = 100;

use crate::store::MECHANICAL_KINDS;

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let trigger = app.state::<PollTrigger>().0.clone();
        let mut failures: u32 = 0;
        loop {
            let base_interval = {
                let store = app.state::<Arc<Store>>();
                store.settings().map(|s| s.poll_interval_secs.max(5)).unwrap_or(45)
            };
            emit_syncing(&app);
            crate::devlog::debug(&app, "poller", "poll cycle starting");
            let sleep_secs = match poll_once(&app).await {
                Ok(rate_remaining) => {
                    failures = 0;
                    crate::devlog::debug(
                        &app,
                        "poller",
                        format!(
                            "poll cycle ok — rate remaining {}",
                            rate_remaining.map_or("?".into(), |r| r.to_string())
                        ),
                    );
                    if rate_remaining.is_some_and(|r| r < LOW_RATE_LIMIT) {
                        base_interval * 4
                    } else {
                        base_interval
                    }
                }
                Err(e) => {
                    failures += 1;
                    emit_status(&app, false, Some(e.to_string()), None);
                    (base_interval * 2u64.saturating_pow(failures.min(4))).min(MAX_BACKOFF_SECS)
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                _ = trigger.notified() => {}
            }
        }
    });
}

fn emit_status(app: &AppHandle, ok: bool, message: Option<String>, rate: Option<i64>) {
    if let Some(msg) = &message {
        crate::devlog::warn(app, "poller", msg.clone());
    }
    let _ = app.emit(
        events::POLL_STATUS,
        PollStatus {
            ok,
            message,
            at: Utc::now().to_rfc3339(),
            rate_limit_remaining: rate,
            syncing: false,
        },
    );
}

/// Announce a cycle starting, so the UI can show "refreshing…".
fn emit_syncing(app: &AppHandle) {
    let _ = app.emit(
        events::POLL_STATUS,
        PollStatus {
            ok: true,
            message: None,
            at: Utc::now().to_rfc3339(),
            rate_limit_remaining: None,
            syncing: true,
        },
    );
}

fn source_for_alias(alias: &str) -> Option<PrSource> {
    match alias {
        "reviewRequested" => Some(PrSource::ReviewRequested),
        "authored" => Some(PrSource::Authored),
        "involved" => Some(PrSource::Involved),
        "watched" => Some(PrSource::WatchedRepo),
        // "tracked" re-fetches already-known PRs; it adds no new source.
        _ => None,
    }
}

/// Native notifications for the changes worth interrupting for: a human
/// replied, or CI went red on your own PR.
fn notify_for_changes(app: &AppHandle, pr: &crate::models::TrackedPr, changes: &[ChangeKind]) {
    let short = format!(
        "{}#{}",
        pr.info.repo.split('/').nth(1).unwrap_or(&pr.info.repo),
        pr.info.number
    );

    if changes.contains(&ChangeKind::NewComments) {
        let newest = crate::models::latest_human_comment(&pr.info.recent_comments, 5);
        let (title, body) = match newest {
            Some(c) => (
                format!("{} commented on {short}", c.author),
                c.snippet.clone(),
            ),
            None => (format!("New comments on {short}"), pr.info.title.clone()),
        };
        crate::notify::send(
            app,
            &title,
            &body,
            Some(crate::notify::FocusTarget {
                pr_id: pr.info.id.clone(),
                comment_id: newest.map(|c| c.id.clone()),
            }),
        );
    }

    if changes.contains(&ChangeKind::CiChanged)
        && pr.sources.contains(&PrSource::Authored)
        && matches!(pr.info.ci_status.as_deref(), Some("FAILURE") | Some("ERROR"))
    {
        crate::notify::send(
            app,
            &format!("CI failing on your PR {short}"),
            &pr.info.title,
            Some(crate::notify::FocusTarget { pr_id: pr.info.id.clone(), comment_id: None }),
        );
    }
}

/// Turn a poll-cycle change into a feed row for the callout's activity log.
/// Importance: your own PRs always matter; so do high-priority repos/PRs.
fn record_activity(
    store: &Arc<Store>,
    settings: &crate::models::Settings,
    pr: &crate::models::TrackedPr,
    changes: &[ChangeKind],
    last_opinion: Option<&(String, String)>,
    now: &str,
) -> bool {
    // Bot-authored PRs (dependabot, renovate) churn constantly; their noise
    // stays out of the feed. The tiles and main list still track them.
    if crate::models::is_bot_login(&pr.info.author) {
        return false;
    }
    let important = pr.sources.contains(&PrSource::Authored)
        || pr.priority == crate::models::PrPriority::High
        || settings.repo_priorities.get(&pr.info.repo) == Some(&RepoPriority::High);
    // A poll delta has no per-event timestamps, so impose causal order on
    // same-cycle rows: setup → work → verdicts → terminal state. Insertion
    // order is the feed's tie-breaker for identical timestamps.
    let causal_rank = |c: &ChangeKind| match c {
        ChangeKind::New => 0,
        ChangeKind::DraftChanged => 1,
        ChangeKind::NewCommits => 2,
        ChangeKind::CiChanged => 3,
        ChangeKind::NewComments => 4,
        ChangeKind::ReviewChanged => 5,
        ChangeKind::Reopened => 6,
        ChangeKind::TitleChanged => 7,
        ChangeKind::Closed => 8,
        ChangeKind::Merged => 9,
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
                let state = match pr.info.review_decision.as_deref() {
                    Some("APPROVED") => "review approved",
                    Some("CHANGES_REQUESTED") => "changes requested",
                    _ => "review state changed",
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
        // own older siblings.
        let superseded: &[&str] = match kind {
            "merged" | "closed" | "reopened" => MECHANICAL_KINDS,
            "commits" | "ci" | "review" => &[kind],
            _ => &[],
        };
        let _ = store.supersede_activity(&pr.info.id, superseded);
        // A red CI on someone else's PR is their problem — record it for the
        // history but arrive pre-read. On your own PRs it demands attention.
        let pre_read = kind == "ci" && !pr.sources.contains(&PrSource::Authored);
        if store
            .add_activity(now, &pr.info, kind, &actor, &summary, &comment_id, important, pre_read)
            .is_ok()
        {
            wrote = true;
        }
    }
    // Once YOU have reviewed (and nobody re-requested), the "review
    // requested" row has served its purpose. Someone else's review never
    // retires it — they still want yours.
    if pr.info.my_review_state.is_some() && !pr.info.my_review_rerequested {
        let _ = store.supersede_activity(&pr.info.id, &["new"]);
    }
    wrote
}

/// Kick off a background L1 analysis for a PR that just entered the review
/// queue (or got new commits while in it), so results are warm before the
/// reviewer opens it. Bounded by a daily cap to keep Bedrock costs sane.
fn maybe_prewarm(
    app: &AppHandle,
    store: &Arc<Store>,
    settings: &crate::models::Settings,
    info: &PrInfo,
    head_moved: bool,
) {
    use crate::analysis::types::AnalysisLevel;
    if !settings.auto_analyze_review_requests {
        return;
    }
    // Already have a fresh L1 for this head? Nothing to do. (When the head
    // just moved, the poll cycle invalidated the cache — skip the lookup.)
    if !head_moved
        && store
            .has_analysis(&info.id, AnalysisLevel::Context.as_str(), "", &info.head_sha)
            .unwrap_or(false)
    {
        return;
    }
    match store.try_consume_daily_budget("auto_analyze", settings.auto_analyze_daily_cap) {
        Ok(Some(used)) => crate::devlog::info(
            app,
            "poller",
            format!(
                "pre-warming L1 analysis for {}#{} ({used}/{} today)",
                info.repo, info.number, settings.auto_analyze_daily_cap
            ),
        ),
        Ok(None) => {
            crate::devlog::debug(
                app,
                "poller",
                format!(
                    "pre-warm skipped for {}: daily cap {} reached",
                    info.id, settings.auto_analyze_daily_cap
                ),
            );
            return;
        }
        Err(_) => return,
    }
    crate::commands::spawn_analysis_task(
        app.clone(),
        info.id.clone(),
        AnalysisLevel::Context,
        None,
        false,
    );
}

/// One poll cycle. Returns remaining rate limit on success.
async fn poll_once(app: &AppHandle) -> AppResult<Option<i64>> {
    let Some(token) = secrets::github_pat()? else {
        emit_status(app, false, Some("no GitHub token configured".into()), None);
        return Ok(None);
    };

    let store = app.state::<Arc<Store>>().inner().clone();
    let settings = store.settings()?;
    let request = PollRequest {
        watched_repos: settings.watched_repos.clone(),
        tracked_ids: store.tracked_ids()?,
        updated_since: (settings.pr_max_age_days > 0).then(|| {
            (Utc::now() - chrono::Duration::days(settings.pr_max_age_days as i64))
                .format("%Y-%m-%d")
                .to_string()
        }),
    };
    let (doc, vars) = request.build();
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?;
    let data = client.run(&doc, &vars).await?;

    // Merge every alias into one map: id → (PrInfo, sources).
    let mut merged: HashMap<String, (PrInfo, Vec<PrSource>)> = HashMap::new();
    for (alias, nodes) in split_by_alias(&data) {
        let source = source_for_alias(alias);
        for node in &nodes {
            let Some(info) = parse_pr(node) else { continue };
            let entry = merged
                .entry(info.id.clone())
                .or_insert_with(|| (info.clone(), Vec::new()));
            entry.0 = info;
            if let Some(s) = &source {
                if !entry.1.contains(s) {
                    entry.1.push(s.clone());
                }
            }
        }
    }

    let now = Utc::now().to_rfc3339();
    let existing: HashMap<String, _> = store
        .list_prs()?
        .into_iter()
        .map(|p| (p.info.id.clone(), p))
        .collect();

    // Viewer-scoped review state comes from separate cheap chunked queries
    // (see fetch_viewer_reviews). Best-effort: on failure keep last known
    // values instead of failing the whole cycle or wiping state.
    let ids: Vec<String> = merged.keys().cloned().collect();
    let viewer_reviews = match crate::github::query::fetch_viewer_reviews(&client, &ids).await {
        Ok(map) => map,
        Err(e) => {
            crate::devlog::warn(app, "poller", format!("viewer-review fetch failed: {e}"));
            std::collections::HashMap::new()
        }
    };
    for (id, (info, _)) in merged.iter_mut() {
        match viewer_reviews.get(id) {
            Some(v) => {
                info.my_review_state = v.my_state.clone();
                info.my_reviewed_at = v.my_at.clone();
                info.my_review_rerequested = v.rerequested;
            }
            None => {
                if let Some(prev) = existing.get(id) {
                    info.my_review_state = prev.info.my_review_state.clone();
                    info.my_reviewed_at = prev.info.my_reviewed_at.clone();
                    info.my_review_rerequested = prev.info.my_review_rerequested;
                }
            }
        }
    }

    let mut activity_written = false;
    for (id, (info, sources)) in &merged {
        // Ignored repos never enter (or stay in) the tracked set.
        if settings.repo_priorities.get(&info.repo) == Some(&RepoPriority::Ignored) {
            if existing.contains_key(id) {
                store.untrack(id)?;
            }
            continue;
        }
        let changes = match existing.get(id) {
            Some(prev) => compute_changes(&prev.info, info),
            None => {
                // GitHub's search index lags: a just-merged/closed PR can keep
                // matching `is:open` for a while. Don't resurrect it as new —
                // but do retire any stale unread feed rows it left behind.
                if info.state != "OPEN" {
                    let _ = store.supersede_activity(id, MECHANICAL_KINDS);
                    continue;
                }
                vec![ChangeKind::New]
            }
        };
        if changes.contains(&ChangeKind::NewCommits) {
            // New commits invalidate every cached analysis for this PR.
            store.invalidate_analyses(id)?;
        }
        let stored = store.upsert_pr(info, sources, &changes, &now)?;
        if stored.info.state != "OPEN" {
            // Backstop for rows written before (or without) the terminal
            // event: a non-open PR never has live mechanical feed rows.
            let _ = store.supersede_activity(id, MECHANICAL_KINDS);
        }
        // Merged/closed PRs whose changes were already acknowledged drop off.
        if stored.info.state != "OPEN" && stored.unread.is_empty() {
            store.untrack(id)?;
            continue;
        }
        // Pre-warm: the L1 analysis is the slowest part of a review, so start
        // it when a PR enters the review-requested bucket or its head moves
        // while there.
        let requested = stored.sources.contains(&PrSource::ReviewRequested);
        let was_requested = existing
            .get(id)
            .is_some_and(|p| p.sources.contains(&PrSource::ReviewRequested));
        let head_moved = changes.contains(&ChangeKind::NewCommits);
        if requested
            && (!was_requested || head_moved)
            && !stored.muted
            && !stored.info.is_draft
            && stored.info.state == "OPEN"
        {
            maybe_prewarm(app, &store, &settings, &stored.info, head_moved);
        }
        if !changes.is_empty() && !stored.muted {
            let last_opinion = viewer_reviews.get(id).and_then(|v| v.last_opinion.as_ref());
            if record_activity(&store, &settings, &stored, &changes, last_opinion, &now) {
                activity_written = true;
            }
            notify_for_changes(app, &stored, &changes);
            let _ = app.emit(events::PR_CHANGED, PrChangedEvent { pr: stored, changes });
        }
    }
    // Once-per-cycle feed hygiene (get_activity is a pure read).
    if store.reconcile_activity().unwrap_or(0) > 0 {
        activity_written = true;
    }
    if activity_written {
        let _ = app.emit(events::ACTIVITY_CHANGED, ());
    }

    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    let rate = data.pointer("/rateLimit/remaining").and_then(serde_json::Value::as_i64);
    crate::devlog::debug(
        app,
        "poller",
        format!(
            "cycle complete: {} PRs merged from search, rate limit remaining {}",
            merged.len(),
            rate.unwrap_or(-1)
        ),
    );
    emit_status(app, true, None, rate);
    Ok(rate)
}

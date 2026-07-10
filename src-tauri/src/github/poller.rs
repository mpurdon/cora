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

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let trigger = app.state::<PollTrigger>().0.clone();
        let mut failures: u32 = 0;
        loop {
            let base_interval = {
                let store = app.state::<Arc<Store>>();
                store.settings().map(|s| s.poll_interval_secs.max(15)).unwrap_or(45)
            };
            let sleep_secs = match poll_once(&app).await {
                Ok(rate_remaining) => {
                    failures = 0;
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
        PollStatus { ok, message, at: Utc::now().to_rfc3339(), rate_limit_remaining: rate },
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
            None => vec![ChangeKind::New],
        };
        if changes.contains(&ChangeKind::NewCommits) {
            // New commits invalidate every cached analysis for this PR.
            store.invalidate_analyses(id)?;
        }
        let stored = store.upsert_pr(info, sources, &changes, &now)?;
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
            notify_for_changes(app, &stored, &changes);
            let _ = app.emit(events::PR_CHANGED, PrChangedEvent { pr: stored, changes });
        }
    }

    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
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

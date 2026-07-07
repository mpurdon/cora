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
        let changes = match existing.get(id) {
            Some(prev) => compute_changes(&prev.info, info),
            None => vec![ChangeKind::New],
        };
        let stored = store.upsert_pr(info, sources, &changes, &now)?;
        // Merged/closed PRs whose changes were already acknowledged drop off.
        if stored.info.state != "OPEN" && stored.unread.is_empty() {
            store.untrack(id)?;
            continue;
        }
        if !changes.is_empty() && !stored.muted {
            let _ = app.emit(events::PR_CHANGED, PrChangedEvent { pr: stored, changes });
        }
    }

    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
    let rate = data.pointer("/rateLimit/remaining").and_then(serde_json::Value::as_i64);
    emit_status(app, true, None, rate);
    Ok(rate)
}

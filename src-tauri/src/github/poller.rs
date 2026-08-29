use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::error::AppResult;
use crate::github::{parse_pr, query::{GraphQlClient, PollRequest}};
use crate::models::{
    compute_changes, events, ChangeKind, PollStatus, PrChangedEvent, PrInfo, PrSource,
    RepoPriority,
};
use crate::secrets;
use crate::store::Store;

/// Wake the pollers immediately (settings change, PAT set, manual refresh).
/// `notify_waiters` wakes every org loop at once.
pub struct PollTrigger(pub Arc<Notify>);

/// Which org loops are running — lets enabling an org spawn its loop
/// exactly once; a loop removes itself when its org is disabled.
#[derive(Default)]
pub struct OrgLoops(pub std::sync::Mutex<std::collections::HashSet<String>>);

const MAX_BACKOFF_SECS: u64 = 300;
const LOW_RATE_LIMIT: i64 = 100;
/// How often a persistent-critical PR re-pings after its first alert — long
/// enough not to spam, short enough that a stuck-critical PR keeps
/// interrupting rather than going quiet for a work session. Not
/// user-configurable: this path exists specifically to override suppression
/// and mute, so it shouldn't inherit the poll interval's tuning.
const PERSISTENT_REASSERT_INTERVAL: Duration = Duration::from_secs(5 * 60);

use crate::store::MECHANICAL_KINDS;

/// One poll loop per enabled org.
pub fn spawn_all(app: AppHandle) {
    for login in app.state::<crate::orgs::Orgs>().enabled() {
        spawn_org(app.clone(), login);
    }
}

pub fn spawn_org(app: AppHandle, login: String) {
    {
        let loops = app.state::<OrgLoops>();
        if !loops.0.lock().unwrap().insert(login.clone()) {
            return; // already running
        }
    }
    tauri::async_runtime::spawn(async move {
        let trigger = app.state::<PollTrigger>().0.clone();
        let mut failures: u32 = 0;
        loop {
            let (enabled, active) = {
                let orgs = app.state::<crate::orgs::Orgs>();
                (orgs.enabled().iter().any(|o| o == &login), orgs.is_active(&login))
            };
            if !enabled {
                app.state::<OrgLoops>().0.lock().unwrap().remove(&login);
                crate::devlog::info(&app, "poller", format!("org {login} disabled — loop exiting"));
                return;
            }
            // The active org polls at its normal cadence; background orgs at
            // their own (slower) per-org interval.
            let base_interval = app
                .state::<crate::orgs::Orgs>()
                .store(&login)
                .ok()
                .and_then(|store| store.settings().ok())
                .map(|s| {
                    if active {
                        s.poll_interval_secs.max(5)
                    } else {
                        s.background_poll_secs.max(30)
                    }
                })
                .unwrap_or(300);
            if active {
                emit_syncing(&app);
            }
            crate::devlog::debug(&app, "poller", format!("poll cycle starting ({login})"));
            let sleep_secs = match poll_once(&app, &login, active).await {
                Ok(rate_remaining) => {
                    failures = 0;
                    crate::devlog::debug(
                        &app,
                        "poller",
                        format!(
                            "poll cycle ok ({login}) — rate remaining {}",
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
                    if active {
                        emit_status(&app, false, Some(e.to_string()), None);
                    } else {
                        crate::devlog::warn(&app, "poller", format!("({login}) {e}"));
                    }
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
fn notify_for_changes(
    app: &AppHandle,
    settings: &crate::models::Settings,
    pr: &crate::models::TrackedPr,
    changes: &[ChangeKind],
    // Set when the change belongs to a background org — the notification
    // says whose news it is.
    org_prefix: Option<&str>,
) {
    // A repo/author/PR combination that resolves to a suppressed effective
    // priority stays silent — but stays in the rail; suppression is about
    // notifications, not tracking. A `Critical` PR always escapes this (see
    // `activity::priority::is_suppressed`), so the persistent-critical path
    // below never needs to route around it.
    let repo_priority =
        settings.repo_priorities.get(&pr.info.repo).copied().unwrap_or(RepoPriority::Standard);
    let author_priority =
        settings.author_priorities.get(&pr.info.author).copied().unwrap_or(RepoPriority::Standard);
    let effective =
        crate::activity::priority::effective_priority(repo_priority, author_priority, pr.priority);
    if crate::activity::priority::is_suppressed(effective) {
        return;
    }

    let short = format!(
        "{}{}#{}",
        org_prefix.map(|o| format!("[{o}] ")).unwrap_or_default(),
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

/// Per-PR timestamp of the last persistent-critical re-alert. In-memory
/// only: a restart just costs one extra alert on the first tick a stuck
/// PR is seen again, which beats a whole ack-adjacent table for a value
/// this disposable.
static LAST_CRITICAL_REASSERT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn last_critical_reassert() -> &'static Mutex<HashMap<String, Instant>> {
    LAST_CRITICAL_REASSERT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The fingerprint a critical acknowledgment is pinned to: the head SHA, or
/// `updated_at` for a PR with no commits yet. Mirrors
/// `commands::acknowledge_critical_pr`'s choice, so a stored ack and a
/// freshly observed PR agree on what "still the same" means.
fn critical_fingerprint(pr: &crate::models::TrackedPr) -> &str {
    if pr.info.head_sha.is_empty() {
        &pr.info.updated_at
    } else {
        &pr.info.head_sha
    }
}

/// A PR that's `Critical` in a `Critical` repo is exempt from suppression
/// (handled in `notify_for_changes`) *and* keeps re-alerting on an interval
/// until it's acknowledged or a new commit invalidates the stale ack —
/// muting or a quiet effective priority elsewhere can't make it go away.
/// Runs inside the existing per-PR poll loop; no separate timer. Returns
/// whether this PR should carry `needs_attention` in this cycle's snapshot.
fn maybe_reassert_critical(
    app: &AppHandle,
    store: &Arc<Store>,
    settings: &crate::models::Settings,
    pr: &crate::models::TrackedPr,
    org_prefix: Option<&str>,
) -> bool {
    let repo_priority =
        settings.repo_priorities.get(&pr.info.repo).copied().unwrap_or(RepoPriority::Standard);
    if !crate::activity::priority::is_persistent_critical_condition(repo_priority, pr.priority) {
        last_critical_reassert().lock().unwrap().remove(&pr.info.id);
        return false;
    }

    let fingerprint = critical_fingerprint(pr);
    let acknowledged = matches!(
        store.get_critical_ack(&pr.info.id),
        Ok(Some((acked_fingerprint, _))) if acked_fingerprint == fingerprint
    );
    if acknowledged {
        last_critical_reassert().lock().unwrap().remove(&pr.info.id);
        return false;
    }

    let mut last_emitted = last_critical_reassert().lock().unwrap();
    let due = match last_emitted.get(&pr.info.id) {
        Some(at) => at.elapsed() >= PERSISTENT_REASSERT_INTERVAL,
        None => true,
    };
    if due {
        crate::notify::emit_persistent_alert(app, pr, org_prefix);
        last_emitted.insert(pr.info.id.clone(), Instant::now());
    }
    true
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
    // Bot PRs (dependabot, renovate) never earn a speculative analysis —
    // they'd burn the daily budget on version bumps.
    if crate::models::is_bot_login(&info.author) {
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
    // Check health before the budget: a run that cannot reach Bedrock would
    // spend a slot of the daily cap and return nothing, so an outage would
    // quietly eat the day's pre-warm allowance.
    if app.state::<crate::health::BedrockHealth>().is_open() {
        crate::devlog::debug(
            app,
            "poller",
            format!("pre-warm skipped for {}: Bedrock unavailable", info.id),
        );
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
        // The PR's owner names the org whose store this run belongs to.
        info.repo.split('/').next().unwrap_or_default().to_string(),
        info.id.clone(),
        AnalysisLevel::Context,
        None,
        false,
    );
}

/// One poll cycle for one org. Returns remaining rate limit on success.
/// `active` gates every UI event — background orgs write silently and only
/// surface through (org-prefixed) native notifications.
async fn poll_once(app: &AppHandle, login: &str, active: bool) -> AppResult<Option<i64>> {
    let Some(token) = secrets::github_pat()? else {
        if active {
            emit_status(app, false, Some("no GitHub token configured".into()), None);
        }
        return Ok(None);
    };

    let store = app.state::<crate::orgs::Orgs>().store(login)?;
    let settings = store.settings()?;
    let request = PollRequest {
        org: login.to_string(),
        watched_repos: settings
            .watched_repos
            .iter()
            .filter(|r| r.starts_with(&format!("{login}/")))
            .cloned()
            .collect(),
        tracked_ids: store.tracked_ids()?,
        updated_since: (settings.pr_max_age_days > 0).then(|| {
            (Utc::now() - chrono::Duration::days(settings.pr_max_age_days as i64))
                .format("%Y-%m-%d")
                .to_string()
        }),
    };
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(app));
    let (aliased, rate_remaining) =
        crate::github::query::run_poll(&client, &request).await?;

    // Merge every alias into one map: id → (PrInfo, sources).
    let owner_prefix = format!("{login}/");
    let mut merged: HashMap<String, (PrInfo, Vec<PrSource>)> = HashMap::new();
    for (alias, nodes) in aliased {
        let source = source_for_alias(alias);
        for node in &nodes {
            let Some(info) = parse_pr(node) else { continue };
            // Belt and braces on top of the `user:` search qualifier — a
            // foreign-org PR must never enter this org's store.
            if !info.repo.starts_with(&owner_prefix) {
                continue;
            }
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

    // One cutoff for the whole cycle rather than a fresh `now` per finished PR.
    let stale_cutoff = (settings.pr_max_age_days > 0)
        .then(|| Utc::now() - chrono::Duration::days(settings.pr_max_age_days as i64));
    let mut activity_written = false;
    // Persistent-critical verdicts computed this cycle, applied to the
    // snapshot below — `store.visible_prs()` never persists `needs_attention`
    // itself, so this is the one place that can populate it without it going
    // stale between ticks.
    let mut needs_attention: HashMap<String, bool> = HashMap::new();
    for (id, (info, sources)) in &merged {
        // Ignored repos and ignored authors never enter (or stay in) the
        // tracked set — dependabot with author priority "ignored" vanishes.
        if settings.repo_priorities.get(&info.repo) == Some(&RepoPriority::Ignored)
            || settings.author_priorities.get(&info.author) == Some(&RepoPriority::Ignored)
        {
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
        // Persistent critical re-assertion runs every cycle regardless of
        // whether anything changed, and regardless of mute — that's the
        // point of the "unconditional exemption": a stuck-critical PR keeps
        // demanding attention on its own schedule, not the poll's delta.
        needs_attention.insert(
            id.clone(),
            maybe_reassert_critical(
                app,
                &store,
                &settings,
                &stored,
                if active { None } else { Some(login) },
            ),
        );
        // Nothing moved, or you've muted this PR: no feed row, no ping, no event.
        let announce = !changes.is_empty() && !stored.muted;
        if announce {
            let opinion = viewer_reviews.get(id).and_then(|v| v.last_opinion.as_ref());
            if crate::activity::record(&store, &settings, &stored, &changes, opinion, &now, false) {
                activity_written = true;
            }
            // Awareness crosses org boundaries; data does not. Background
            // orgs still ping natively (org-prefixed), but never touch the
            // active org's UI events.
            notify_for_changes(app, &settings, &stored, &changes, if active { None } else { Some(login) });
        }
        // A finished PR — closed or merged — has nothing left to review, so it
        // drops out of the rail (the "finished" chip reveals it on demand) and
        // settles: no unread rows but the one saying how it ended. It stays
        // *tracked*, because hiding is the rail's job and Reopen needs the
        // stored row. This runs after activity::record so a cycle that sees a
        // comment and the close together retires the comment too, rather than
        // leaving it unread on a PR nobody will look at again.
        if let Some(terminal) = crate::models::terminal_kind(&stored.info.state) {
            store.retire_finished(id, terminal)?;
            // Tracked, but not forever: past the visibility window nothing can
            // show it anyway, so dropping it there bounds the store.
            if stale_cutoff
                .is_some_and(|c| {
                    chrono::DateTime::parse_from_rfc3339(&stored.info.updated_at)
                        .is_ok_and(|t| t < c)
                })
            {
                store.untrack(id)?;
            }
        }
        if active && announce {
            let _ = app.emit(events::PR_CHANGED, PrChangedEvent { pr: stored, changes });
        }
    }
    // Once-per-cycle feed hygiene (get_activity is a pure read).
    if store.reconcile_activity().unwrap_or(0) > 0 {
        activity_written = true;
    }
    if active && activity_written {
        let _ = app.emit(events::ACTIVITY_CHANGED, ());
    }

    if active {
        let mut snapshot = store.visible_prs()?;
        for pr in &mut snapshot {
            // Defaults to false for anything this cycle didn't touch (e.g. a
            // PR the search query didn't return) — it self-corrects the next
            // time that PR appears in `merged`.
            pr.needs_attention = needs_attention.get(&pr.info.id).copied().unwrap_or(false);
        }
        let _ = app.emit(events::PRS_SNAPSHOT, snapshot);
    }
    crate::devlog::debug(
        app,
        "poller",
        format!(
            "cycle complete ({login}): {} PRs merged from search, rate limit remaining {}",
            merged.len(),
            rate_remaining.unwrap_or(-1)
        ),
    );
    if active {
        emit_status(app, true, None, rate_remaining);
    }
    Ok(rate_remaining)
}

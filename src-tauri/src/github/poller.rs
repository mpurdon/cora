use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
    pr: &crate::models::TrackedPr,
    changes: &[ChangeKind],
    // Set when the change belongs to a background org — the notification
    // says whose news it is.
    org_prefix: Option<&str>,
) {
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
        || settings.repo_priorities.get(&pr.info.repo) == Some(&RepoPriority::High)
        || settings.author_priorities.get(&pr.info.author) == Some(&RepoPriority::High);
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
            "commits" | "ci" | "review" => &[kind],
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
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?;
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
        if !changes.is_empty() && !stored.muted {
            let last_opinion = viewer_reviews.get(id).and_then(|v| v.last_opinion.as_ref());
            if record_activity(&store, &settings, &stored, &changes, last_opinion, &now) {
                activity_written = true;
            }
            // Awareness crosses org boundaries; data does not. Background
            // orgs still ping natively (org-prefixed), but never touch the
            // active org's UI events.
            notify_for_changes(app, &stored, &changes, if active { None } else { Some(login) });
        }
        // A finished PR — closed or merged — has nothing left to review, so it
        // drops out of the rail (the "finished" chip reveals it on demand) and
        // settles: no unread rows but the one saying how it ended. It stays
        // *tracked*, because hiding is the rail's job and Reopen needs the
        // stored row. This runs after record_activity so a cycle that sees a
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
        if active && !changes.is_empty() && !stored.muted {
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
        let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
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

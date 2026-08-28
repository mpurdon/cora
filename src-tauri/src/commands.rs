use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::analysis::types::{events as analysis_events, AnalysisLevel, AnalysisResult};
use crate::error::{AppError, AppResult};
use crate::github::poller::PollTrigger;
use crate::github::query::{GraphQlClient, PR_FRAGMENT};
use crate::github::{parse_pr, parse_pr_url};
use crate::models::{
    events, ChangeKind, PrComment, PrConversation, PrSource, ReviewMark, ReviewThread, Settings,
    TrackedPr,
};
use crate::secrets;
use crate::store::Store;

/// Credential-adjacent commands only accept calls from the main window.
/// (Custom commands sit outside the capability ACL, so we enforce it here.)
fn require_main(window: &WebviewWindow) -> AppResult<()> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(AppError::Other(format!(
            "command not permitted from window '{}'",
            window.label()
        )))
    }
}

// -- settings ---------------------------------------------------------------

#[tauri::command]
pub fn get_settings(orgs: State<'_, crate::orgs::Orgs>) -> AppResult<Settings> {
    let store = orgs.active();
    store.settings()
}

#[tauri::command]
pub fn set_settings(
    window: WebviewWindow,
    orgs: State<'_, crate::orgs::Orgs>,
    trigger: State<'_, PollTrigger>,
    settings: Settings,
) -> AppResult<()> {
    let store = orgs.active();
    require_main(&window)?;
    store.save_settings(&settings)?;
    // Team conventions are part of every chat session's system prompt, so a
    // settings save has to reach open sessions the same way a finished
    // analysis does — otherwise they keep the conventions they were built with.
    crate::analysis::chat::invalidate_all_contexts(window.app_handle());
    // Re-emit immediately so settings that shape the visible set (the PR age
    // window) take effect without waiting for the poll cycle the trigger kicks.
    let _ = window
        .app_handle()
        .emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    trigger.0.notify_waiters();
    Ok(())
}

// -- organizations ------------------------------------------------------------

/// Every org bucket the PAT can see: the viewer's own account first, then
/// their organizations.
#[tauri::command]
pub async fn list_github_orgs(app: AppHandle) -> AppResult<Vec<crate::models::GithubOrg>> {
    let settings = app.state::<crate::orgs::Orgs>().active().settings()?;
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let data = client
        .run(
            "query { viewer { login name organizations(first: 100) { nodes { login name } } } }",
            &serde_json::json!({}),
        )
        .await?;
    let mut out = Vec::new();
    if let Some(login) = data.pointer("/viewer/login").and_then(|v| v.as_str()) {
        out.push(crate::models::GithubOrg {
            login: login.to_string(),
            name: data
                .pointer("/viewer/name")
                .and_then(|v| v.as_str())
                .unwrap_or(login)
                .to_string(),
            personal: true,
        });
    }
    for node in data
        .pointer("/viewer/organizations/nodes")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let Some(login) = node.get("login").and_then(|v| v.as_str()) else { continue };
        out.push(crate::models::GithubOrg {
            login: login.to_string(),
            name: node.get("name").and_then(|v| v.as_str()).unwrap_or(login).to_string(),
            personal: false,
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn get_org_state(orgs: State<'_, crate::orgs::Orgs>) -> AppResult<crate::models::OrgState> {
    let mut unread = std::collections::HashMap::new();
    for login in orgs.enabled() {
        if let Ok(store) = orgs.store(&login) {
            unread.insert(login.clone(), store.count_unread_activity().unwrap_or(0));
        }
    }
    Ok(crate::models::OrgState { active: orgs.active_login(), enabled: orgs.enabled(), unread })
}

/// Switch the active org: every window swaps to that org's data. Callable
/// from any window — the selector lives in the callout too.
#[tauri::command]
pub fn set_active_org(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    trigger: State<'_, PollTrigger>,
    login: String,
) -> AppResult<()> {
    orgs.set_active(&login)?;
    let store = orgs.active();
    let _ = app.emit(events::ORG_CHANGED, login);
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    let _ = app.emit(events::ACTIVITY_CHANGED, ());
    // Fresh cycle right away — the new active org polls at full cadence.
    trigger.0.notify_waiters();
    Ok(())
}

#[tauri::command]
pub fn set_enabled_orgs(
    app: AppHandle,
    window: WebviewWindow,
    orgs: State<'_, crate::orgs::Orgs>,
    logins: Vec<String>,
) -> AppResult<Vec<String>> {
    require_main(&window)?;
    let enabled = orgs.set_enabled(logins)?;
    for login in &enabled {
        crate::github::poller::spawn_org(app.clone(), login.clone());
    }
    let store = orgs.active();
    let _ = app.emit(events::ORG_CHANGED, orgs.active_login());
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    let _ = app.emit(events::ACTIVITY_CHANGED, ());
    Ok(enabled)
}

// -- GitHub PAT (token never leaves Rust) ------------------------------------

#[tauri::command]
pub fn set_github_pat(
    window: WebviewWindow,
    trigger: State<'_, PollTrigger>,
    token: String,
) -> AppResult<()> {
    require_main(&window)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::Other("token is empty".into()));
    }
    secrets::set_github_pat(token)?;
    trigger.0.notify_waiters();
    Ok(())
}

#[tauri::command]
pub fn github_pat_present() -> AppResult<bool> {
    secrets::github_pat_present()
}

#[tauri::command]
pub fn clear_github_pat(window: WebviewWindow) -> AppResult<()> {
    require_main(&window)?;
    secrets::clear_github_pat()
}

// -- PR list ------------------------------------------------------------------

#[tauri::command]
pub fn list_prs(orgs: State<'_, crate::orgs::Orgs>) -> AppResult<Vec<TrackedPr>> {
    let store = orgs.active();
    store.visible_prs()
}

#[tauri::command]
pub fn mark_pr_read(app: AppHandle, orgs: State<'_, crate::orgs::Orgs>, id: String) -> AppResult<()> {
    let store = orgs.active();
    store.mark_read(&id)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

#[tauri::command]
pub fn mark_pr_read_kinds(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    kinds: Vec<ChangeKind>,
) -> AppResult<()> {
    let store = orgs.active();
    store.mark_read_kinds(&id, &kinds)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

pub(crate) fn pr_label(store: &Store, id: &str) -> String {
    store
        .get_pr(id)
        .ok()
        .flatten()
        .map(|p| format!("{}#{} — {}", p.info.repo, p.info.number, p.info.title))
        .unwrap_or_else(|| id.to_string())
}

#[tauri::command]
pub fn set_pr_muted(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    muted: bool,
) -> AppResult<()> {
    let store = orgs.active();
    let label = pr_label(&store, &id);
    store.set_muted(&id, muted)?;
    store.add_audit(
        if muted { "muted" } else { "unmuted" },
        &id,
        &label,
        &(!muted).to_string(),
        &muted.to_string(),
    )?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

#[tauri::command]
pub fn untrack_pr(app: AppHandle, orgs: State<'_, crate::orgs::Orgs>, id: String) -> AppResult<()> {
    let store = orgs.active();
    let label = pr_label(&store, &id);
    store.untrack(&id)?;
    store.add_audit("untracked", &id, &label, "tracked", "untracked")?;
    // Untracking strands this PR's mechanical feed rows — retire them now
    // rather than waiting for the next poll cycle.
    if store.reconcile_activity().unwrap_or(0) > 0 {
        let _ = app.emit(events::ACTIVITY_CHANGED, ());
    }
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

/// Repo priority as a first-class, audited action (context menus + detail).
#[tauri::command]
pub fn set_repo_priority(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    trigger: State<'_, PollTrigger>,
    repo: String,
    priority: crate::models::RepoPriority,
) -> AppResult<()> {
    let store = orgs.active();
    let mut settings = store.settings()?;
    let old = settings
        .repo_priorities
        .get(&repo)
        .copied()
        .unwrap_or(crate::models::RepoPriority::Normal);
    if priority == crate::models::RepoPriority::Normal {
        settings.repo_priorities.remove(&repo);
    } else {
        settings.repo_priorities.insert(repo.clone(), priority);
    }
    store.save_settings(&settings)?;
    store.add_audit(
        "repo-priority",
        &repo,
        &repo,
        &format!("{old:?}").to_lowercase(),
        &format!("{priority:?}").to_lowercase(),
    )?;
    trigger.0.notify_waiters();
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

/// Author priority as a first-class, audited action — high for the people
/// whose PRs always matter, ignored for the bots whose never do.
#[tauri::command]
pub fn set_author_priority(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    trigger: State<'_, PollTrigger>,
    author: String,
    priority: crate::models::RepoPriority,
) -> AppResult<()> {
    let store = orgs.active();
    let mut settings = store.settings()?;
    let old = settings
        .author_priorities
        .get(&author)
        .copied()
        .unwrap_or(crate::models::RepoPriority::Normal);
    if priority == crate::models::RepoPriority::Normal {
        settings.author_priorities.remove(&author);
    } else {
        settings.author_priorities.insert(author.clone(), priority);
    }
    store.save_settings(&settings)?;
    store.add_audit(
        "author-priority",
        &format!("author:{author}"),
        &format!("@{author}"),
        &format!("{old:?}").to_lowercase(),
        &format!("{priority:?}").to_lowercase(),
    )?;
    // Ignoring takes effect immediately — untrack + scrub the feed rather
    // than waiting a poll cycle.
    if priority == crate::models::RepoPriority::Ignored {
        let _ = store.purge_author(&author);
        let _ = app.emit(events::ACTIVITY_CHANGED, ());
    }
    trigger.0.notify_waiters();
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

#[tauri::command]
pub fn get_audit_log(orgs: State<'_, crate::orgs::Orgs>) -> AppResult<Vec<crate::models::AuditEntry>> {
    let store = orgs.active();
    store.list_audit(200)
}

// -- activity feed (callout) ----------------------------------------------

#[tauri::command]
pub fn get_activity(
    orgs: State<'_, crate::orgs::Orgs>,
) -> AppResult<Vec<crate::models::ActivityItem>> {
    let store = orgs.active();
    // Pure read — feed hygiene runs once per poll cycle and on untrack. This
    // only caps the query; older rows stay in SQLite (see ACTIVITY_RETENTION_CAP).
    let limit = store
        .settings()
        .map(|s| s.callout_feed_limit)
        .unwrap_or_else(|_| crate::models::Settings::default().callout_feed_limit)
        as i64;
    store.list_activity(limit)
}

/// Empty `ids` marks the whole feed.
#[tauri::command]
pub fn mark_activity_read(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    ids: Vec<i64>,
    read: bool,
) -> AppResult<()> {
    let store = orgs.active();
    store.mark_activity_read(&ids, read)?;
    let _ = app.emit(crate::models::events::ACTIVITY_CHANGED, ());
    Ok(())
}

/// Every flagged row, feed limit or not — the main window's flagged worklist.
#[tauri::command]
pub fn get_flagged(
    orgs: State<'_, crate::orgs::Orgs>,
) -> AppResult<Vec<crate::models::ActivityItem>> {
    orgs.active().list_flagged()
}

#[tauri::command]
pub fn set_activity_flag(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: i64,
    flag: String,
) -> AppResult<()> {
    let store = orgs.active();
    store.set_activity_flag(id, &flag)?;
    let _ = app.emit(crate::models::events::ACTIVITY_CHANGED, ());
    Ok(())
}

/// Clear every flag on one PR, in one write and one event.
#[tauri::command]
pub fn clear_activity_flags(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    pr_id: String,
) -> AppResult<()> {
    orgs.active().clear_activity_flags(&pr_id)?;
    let _ = app.emit(crate::models::events::ACTIVITY_CHANGED, ());
    Ok(())
}

/// Reverse a recorded action by restoring its old value.
#[tauri::command]
pub fn undo_audit(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    trigger: State<'_, PollTrigger>,
    id: i64,
) -> AppResult<()> {
    let store = orgs.active();
    let entry = store
        .get_audit(id)?
        .ok_or_else(|| AppError::Other("audit entry not found".into()))?;
    if entry.undone {
        return Err(AppError::Other("already undone".into()));
    }
    match entry.action.as_str() {
        "muted" => store.set_muted(&entry.subject_id, false)?,
        "unmuted" => store.set_muted(&entry.subject_id, true)?,
        "untracked" => store.retrack(&entry.subject_id)?,
        "tracked" => store.untrack(&entry.subject_id)?,
        "pr-priority" => {
            store.set_pr_priority(
                &entry.subject_id,
                crate::models::PrPriority::parse(&entry.old_value),
            )?;
        }
        "repo-priority" => {
            let mut settings = store.settings()?;
            // Normal is the absence of an override, so it undoes to a removal
            // rather than to an explicit entry.
            let old = crate::models::RepoPriority::parse(&entry.old_value)
                .filter(|p| *p != crate::models::RepoPriority::Normal);
            match old {
                Some(p) => {
                    settings.repo_priorities.insert(entry.subject_id.clone(), p);
                }
                None => {
                    settings.repo_priorities.remove(&entry.subject_id);
                }
            }
            store.save_settings(&settings)?;
            trigger.0.notify_waiters();
        }
        "author-priority" => {
            let mut settings = store.settings()?;
            let login = entry.subject_id.strip_prefix("author:").unwrap_or(&entry.subject_id);
            // Normal is the absence of an override, so it undoes to a removal
            // rather than to an explicit entry.
            let old = crate::models::RepoPriority::parse(&entry.old_value)
                .filter(|p| *p != crate::models::RepoPriority::Normal);
            match old {
                Some(p) => {
                    settings.author_priorities.insert(login.to_string(), p);
                }
                None => {
                    settings.author_priorities.remove(login);
                }
            }
            store.save_settings(&settings)?;
            trigger.0.notify_waiters();
        }
        other => return Err(AppError::Other(format!("cannot undo action: {other}"))),
    }
    store.mark_audit_undone(id)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

/// Manually track a PR by URL (also the entry point chat ingestion will use).
#[tauri::command]
pub async fn track_pr_url(
    app: AppHandle,
    window: WebviewWindow,
    url: String,
) -> AppResult<TrackedPr> {
    require_main(&window)?;
    let (repo, number) =
        parse_pr_url(&url).ok_or_else(|| AppError::Other("not a GitHub PR URL".into()))?;
    let (owner, name) = repo.split_once('/').unwrap();

    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let store = app.state::<crate::orgs::Orgs>().active();
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let doc = format!(
        "query($owner: String!, $name: String!, $number: Int!) {{
           repository(owner: $owner, name: $name) {{
             pullRequest(number: $number) {{ ...PrFields }}
           }}
         }}\n{PR_FRAGMENT}"
    );
    let data = client
        .run(&doc, &serde_json::json!({ "owner": owner, "name": name, "number": number }))
        .await?;
    let node = data
        .pointer("/repository/pullRequest")
        .filter(|v| !v.is_null())
        .ok_or_else(|| AppError::github(format!("PR {repo}#{number} not found")))?;
    let info = parse_pr(node).ok_or_else(|| AppError::github("unexpected PR shape"))?;

    let now = Utc::now().to_rfc3339();
    let stored = store.upsert_pr(&info, &[PrSource::Manual], &[ChangeKind::New], &now)?;
    store.add_audit(
        "tracked",
        &stored.info.id,
        &format!("{}#{} — {}", stored.info.repo, stored.info.number, stored.info.title),
        "untracked",
        "tracked",
    )?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(stored)
}

// -- analysis -------------------------------------------------------------------

/// Keys of currently-running analyses, to prevent duplicate runs.
pub struct AnalysisRuns(pub std::sync::Mutex<std::collections::HashSet<String>>);

fn analysis_key(pr_id: &str, level: AnalysisLevel, focus: &Option<String>) -> String {
    format!("{pr_id}:{}:{}", level.as_str(), focus.as_deref().unwrap_or(""))
}

#[tauri::command]
pub fn get_analysis(
    orgs: State<'_, crate::orgs::Orgs>,
    pr_id: String,
    level: AnalysisLevel,
    focus: Option<String>,
) -> AppResult<Option<AnalysisResult>> {
    let store = orgs.active();
    let Some(pr) = store.get_pr(&pr_id)? else {
        return Ok(None);
    };
    let cached = store.get_analysis(
        &pr_id,
        level.as_str(),
        focus.as_deref().unwrap_or(""),
        &pr.info.head_sha,
    )?;
    Ok(cached.and_then(|json| serde_json::from_str(&json).ok()))
}

/// Kick off an analysis in the background. Results arrive as events:
/// analysis:complete / analysis:error, with analysis:progress along the way.
#[tauri::command]
pub fn run_analysis(
    app: AppHandle,
    window: WebviewWindow,
    pr_id: String,
    level: AnalysisLevel,
    focus: Option<String>,
    force: Option<bool>,
) -> AppResult<()> {
    require_main(&window)?;
    let org = app.state::<crate::orgs::Orgs>().active_login();
    spawn_analysis_task(app, org, pr_id, level, focus, force.unwrap_or(false));
    Ok(())
}

/// Shared by the Analyze button and the poller's pre-warm path. `org` names
/// which org's store the run reads/writes — the poller passes its own loop's
/// org so background analyses never touch another org's data.
pub fn spawn_analysis_task(
    app: AppHandle,
    org: String,
    pr_id: String,
    level: AnalysisLevel,
    focus: Option<String>,
    force: bool,
) {
    let key = analysis_key(&pr_id, level, &focus);
    {
        let runs = app.state::<AnalysisRuns>();
        let mut running = runs.0.lock().unwrap();
        if !running.insert(key.clone()) {
            return; // already running
        }
    }
    tauri::async_runtime::spawn(async move {
        let Ok(store) = app.state::<crate::orgs::Orgs>().store(&org) else {
            let runs = app.state::<AnalysisRuns>();
            runs.0.lock().unwrap().remove(&key);
            return;
        };
        let outcome = execute_analysis(&app, &store, &pr_id, level, focus.clone(), force).await;
        {
            let runs = app.state::<AnalysisRuns>();
            runs.0.lock().unwrap().remove(&key);
        }
        match outcome {
            Ok(result) => {
                // A finished context-level analysis is the "this PR is ready
                // for you" moment: native ping (external-boundary impacts are
                // the headline when present) plus a featured feed row. Drills
                // stay quiet — they're sub-steps and often prefetched.
                if level == AnalysisLevel::Context {
                    let org_active = app.state::<crate::orgs::Orgs>().is_active(&org);
                    if let Ok(Some(pr)) = store.get_pr(&result.pr_id) {
                        let externals = result
                            .assessment
                            .boundary_impacts
                            .iter()
                            .filter(|i| i.kind == crate::analysis::types::ImpactKind::External)
                            .count();
                        let repo_short =
                            pr.info.repo.split('/').nth(1).unwrap_or(&pr.info.repo).to_string();
                        let mut title = if externals > 0 {
                            format!(
                                "{externals} external-boundary impact{} in {repo_short}#{}",
                                if externals == 1 { "" } else { "s" },
                                pr.info.number
                            )
                        } else {
                            format!("analysis ready · {repo_short}#{}", pr.info.number)
                        };
                        // Background-org completions still ping — awareness —
                        // but say whose news it is.
                        if !org_active {
                            title = format!("[{org}] {title}");
                        }
                        crate::notify::send(
                            &app,
                            &title,
                            &result.assessment.summary,
                            Some(crate::notify::FocusTarget {
                                pr_id: result.pr_id.clone(),
                                comment_id: None,
                            }),
                        );
                        // One live "analysis ready" row per PR — a re-run
                        // retires the previous one like any state tracker.
                        let summary = if externals > 0 {
                            format!(
                                "analysis ready — {externals} external impact{}",
                                if externals == 1 { "" } else { "s" }
                            )
                        } else {
                            "analysis ready".to_string()
                        };
                        let now = chrono::Utc::now().to_rfc3339();
                        let _ = store.supersede_activity(&pr.info.id, &["analysis"]);
                        let _ = store.add_activity(
                            &now, &pr.info, "analysis", "", &summary, "", true, false,
                        );
                        // The feed shows the active org — only its writes
                        // should nudge the callout to refresh.
                        if org_active {
                            let _ = app.emit(events::ACTIVITY_CHANGED, ());
                        }
                    }
                }
                // A finished assessment is new context for the assistant.
                // Without this its session keeps the "no analysis yet" prompt
                // until the head moves — the analysis it should be grounded in
                // would never reach it.
                if level == AnalysisLevel::Context {
                    crate::analysis::chat::invalidate_context(&app, &result.pr_id);
                }
                let _ = app.emit(analysis_events::ANALYSIS_COMPLETE, result);
            }
            Err(e) => {
                let error = e.to_string();
                let _ = app.emit(
                    analysis_events::ANALYSIS_ERROR,
                    crate::analysis::types::AnalysisError {
                        pr_id,
                        level,
                        focus: focus.clone().unwrap_or_default(),
                        kind: crate::analysis::types::classify_error(&error),
                        error,
                    },
                );
            }
        }
    });
}

async fn execute_analysis(
    app: &AppHandle,
    store: &Arc<Store>,
    pr_id: &str,
    level: AnalysisLevel,
    focus: Option<String>,
    force: bool,
) -> AppResult<AnalysisResult> {
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;

    // Serve from cache when the head hasn't moved — unless the user forced
    // a fresh run (Re-run always rebuilds).
    if !force {
        if let Some(json) = store.get_analysis(
            pr_id,
            level.as_str(),
            focus.as_deref().unwrap_or(""),
            &pr.info.head_sha,
        )? {
            if let Ok(cached) = serde_json::from_str::<AnalysisResult>(&json) {
                return Ok(cached);
            }
        }
    }

    let settings = store.settings()?;
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;

    // Ground drilled runs in the context-level result (graph + assessment
    // only — the trace would just be noise) so they don't re-explore.
    let parent_context = if level != AnalysisLevel::Context {
        store
            .get_analysis(pr_id, AnalysisLevel::Context.as_str(), "", &pr.info.head_sha)?
            .and_then(|json| serde_json::from_str::<AnalysisResult>(&json).ok())
            .map(|parent| {
                serde_json::json!({
                    "graph": parent.graph,
                    "assessment": parent.assessment,
                })
                .to_string()
            })
    } else {
        None
    };

    let mut result =
        crate::analysis::engine::run(app, &settings, &token, &pr, level, focus, parent_context)
            .await?;

    // Second stage: line-anchored defect/reuse findings over the review
    // plan's significant files. Best-effort — its failure never sinks the
    // architecture result.
    if level == AnalysisLevel::Context {
        if settings.code_findings_pass {
            match crate::analysis::engine::code_findings(
                app,
                &settings,
                &token,
                &pr,
                &result.assessment.review_plan,
            )
            .await
            {
                Ok(findings) => {
                    result.code_findings = findings;
                    result.code_pass = Some("ok".into());
                }
                Err(e) => {
                    crate::devlog::warn(app, "code-pass", format!("code findings pass failed: {e}"));
                    // Failure stays visible on the result — an empty findings
                    // list must be distinguishable from a crashed pass.
                    result.code_pass = Some(format!("failed: {e}"));
                }
            }
        } else {
            result.code_pass = Some("off".into());
        }
    }

    store.put_analysis(
        &result.pr_id,
        result.level.as_str(),
        result.focus_node_id.as_deref().unwrap_or(""),
        &result.head_sha,
        &serde_json::to_string(&result).map_err(|e| AppError::Other(e.to_string()))?,
        &result.created_at,
    )?;
    Ok(result)
}

#[tauri::command]
pub fn set_pr_priority(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    priority: crate::models::PrPriority,
) -> AppResult<()> {
    let store = orgs.active();
    let label = pr_label(&store, &id);
    let old = store
        .get_pr(&id)?
        .map(|p| p.priority)
        .unwrap_or(crate::models::PrPriority::Normal);
    store.set_pr_priority(&id, priority)?;
    store.add_audit("pr-priority", &id, &label, old.as_str(), priority.as_str())?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    Ok(())
}

/// On-demand refresh of a single PR (the ⟳ button in the PR panel).
#[tauri::command]
pub async fn refresh_pr(app: AppHandle, pr_id: String) -> AppResult<TrackedPr> {
    refresh_pr_inner(&app, &pr_id, false).await
}

/// `self_acted` marks a refresh that follows your own merge/close, so the feed
/// rows it writes arrive already read: a record of what you did, not news.
async fn refresh_pr_inner(
    app: &AppHandle,
    pr_id: &str,
    self_acted: bool,
) -> AppResult<TrackedPr> {
    let app = app.clone();
    let pr_id = pr_id.to_string();
    let store = app.state::<crate::orgs::Orgs>().active();
    let existing = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));

    // A single node is cheap enough to carry the viewer-scoped review fields
    // inline (the batched poll fetches them separately — see fetch_viewer_reviews).
    let doc = format!(
        "query($id: ID!) {{ node(id: $id) {{ ... on PullRequest {{ ...PrFields
           viewerLatestReview {{ state submittedAt }}
           viewerLatestReviewRequest {{ id }}
        }} }} }}\n{PR_FRAGMENT}"
    );
    let data = client.run(&doc, &serde_json::json!({ "id": pr_id })).await?;
    let node = data
        .pointer("/node")
        .filter(|v| !v.is_null())
        .ok_or_else(|| AppError::github("PR no longer accessible"))?;
    let info = parse_pr(node).ok_or_else(|| AppError::github("unexpected PR shape"))?;

    let changes = crate::models::compute_changes(&existing.info, &info);
    if changes.contains(&ChangeKind::NewCommits) {
        store.invalidate_analyses(&pr_id)?;
    }
    let now = Utc::now().to_rfc3339();
    let stored = store.upsert_pr(&info, &[], &changes, &now)?;
    // Whoever sees a change first has to report it: once this row holds the new
    // snapshot the poller computes no transition and stays silent forever, so a
    // refresh that beat it there (the ⟳ button, a merge, an assistant action)
    // owes the feed the same rows the poller would have written.
    let wrote = !changes.is_empty()
        && !stored.muted
        && crate::activity::record(&store, &settings, &stored, &changes, None, &now, self_acted);
    // Same rule the poller applies to a finished PR — see Store::retire_finished.
    if let Some(terminal) = crate::models::terminal_kind(&stored.info.state) {
        store.retire_finished(&pr_id, terminal)?;
    }
    if wrote {
        let _ = app.emit(events::ACTIVITY_CHANGED, ());
    }
    let _ = app.emit(events::PRS_SNAPSHOT, store.visible_prs()?);
    // Covers every "review state may have moved" path that funnels through a
    // refresh: manual refresh button, submitted reviews, assistant actions.
    let _ = app.emit(events::REVIEWS_CHANGED, ());
    Ok(stored)
}

/// Post a top-level comment on the PR conversation.
#[tauri::command]
pub async fn add_pr_comment(app: AppHandle, pr_id: String, body: String) -> AppResult<()> {
    if body.trim().is_empty() {
        return Err(AppError::Other("comment is empty".into()));
    }
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    client
        .run(
            "mutation($subjectId: ID!, $body: String!) {
               addComment(input: { subjectId: $subjectId, body: $body }) { clientMutationId }
             }",
            &serde_json::json!({ "subjectId": pr_id, "body": body }),
        )
        .await?;
    Ok(())
}

/// Edit one of your own comments. GitHub splits this across two mutations by
/// where the comment lives, so the caller says which.
#[tauri::command]
pub async fn update_comment(
    app: AppHandle,
    comment_id: String,
    body: String,
    is_review_comment: bool,
) -> AppResult<()> {
    if body.trim().is_empty() {
        return Err(AppError::Other("comment is empty".into()));
    }
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let doc = if is_review_comment {
        "mutation($id: ID!, $body: String!) {
           updatePullRequestReviewComment(input: { pullRequestReviewCommentId: $id, body: $body }) {
             clientMutationId
           }
         }"
    } else {
        "mutation($id: ID!, $body: String!) {
           updateIssueComment(input: { id: $id, body: $body }) { clientMutationId }
         }"
    };
    client
        .run(doc, &serde_json::json!({ "id": comment_id, "body": body }))
        .await?;
    // An edit can flip the approve gate: `my_open_threads` is computed from the
    // *first* comment of each thread, so adding a `(non-blocking)` marker to one
    // stops it holding up your approval. Without this the conversation reloads
    // and shows the new tag while the gate keeps its stale count.
    let _ = app.emit(events::REVIEWS_CHANGED, ());
    Ok(())
}

/// Reply to a code review thread.
#[tauri::command]
pub async fn reply_to_thread(app: AppHandle, thread_id: String, body: String) -> AppResult<()> {
    if body.trim().is_empty() {
        return Err(AppError::Other("reply is empty".into()));
    }
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    client
        .run(
            "mutation($threadId: ID!, $body: String!) {
               addPullRequestReviewThreadReply(
                 input: { pullRequestReviewThreadId: $threadId, body: $body }
               ) { clientMutationId }
             }",
            &serde_json::json!({ "threadId": thread_id, "body": body }),
        )
        .await?;
    Ok(())
}

/// Requested reviewers + latest review states, for the detail header strip.
#[tauri::command]
pub async fn get_pr_reviews(app: AppHandle, pr_id: String) -> AppResult<crate::models::PrReviews> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let (owner, name) = pr.info.repo.split_once('/').unwrap();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let data = client
        .run(
            "query($owner: String!, $name: String!, $number: Int!) {
              viewer { login }
              repository(owner: $owner, name: $name) {
                pullRequest(number: $number) {
                  reviewRequests(first: 20) {
                    nodes { requestedReviewer {
                      ... on User { login }
                      ... on Team { name }
                    } }
                  }
                  latestReviews(first: 30) {
                    nodes { author { login } state submittedAt }
                  }
                  commits(last: 1) { nodes { commit { committedDate } } }
                  reviewThreads(first: 100) {
                    nodes {
                      isResolved
                      comments(first: 1) { nodes { author { login } body } }
                    }
                  }
                }
              }
            }",
            &serde_json::json!({ "owner": owner, "name": name, "number": pr.info.number }),
        )
        .await?;

    let requested = data
        .pointer("/repository/pullRequest/reviewRequests/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    n.pointer("/requestedReviewer/login")
                        .or_else(|| n.pointer("/requestedReviewer/name"))
                        .and_then(serde_json::Value::as_str)
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    let reviews = data
        .pointer("/repository/pullRequest/latestReviews/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    Some(crate::models::ReviewSummary {
                        author: n
                            .pointer("/author/login")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("ghost")
                            .to_string(),
                        state: n.get("state")?.as_str()?.to_string(),
                        submitted_at: n
                            .get("submittedAt")
                            .and_then(serde_json::Value::as_str)
                            .map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let viewer_login = data
        .pointer("/viewer/login")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let last_commit_at = data
        .pointer("/repository/pullRequest/commits/nodes/0/commit/committedDate")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let (open_threads, my_open_threads) = data
        .pointer("/repository/pullRequest/reviewThreads/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| {
            let open: Vec<_> = nodes
                .iter()
                .filter(|n| {
                    !n.get("isResolved").and_then(serde_json::Value::as_bool).unwrap_or(false)
                })
                .collect();
            let mine = open
                .iter()
                .filter(|n| {
                    n.pointer("/comments/nodes/0/author/login")
                        .and_then(serde_json::Value::as_str)
                        == Some(viewer_login.as_str())
                })
                // Non-blocking comments (praise, notes, FYIs) open normal
                // GitHub threads but shouldn't gate your own approval.
                .filter(|n| {
                    !n.pointer("/comments/nodes/0/body")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(crate::models::is_non_blocking_comment)
                })
                .count() as i64;
            (open.len() as i64, mine)
        })
        .unwrap_or((0, 0));

    Ok(crate::models::PrReviews {
        requested,
        reviews,
        viewer_login,
        last_commit_at,
        open_threads,
        my_open_threads,
    })
}

/// Resolve or unresolve a review thread.
#[tauri::command]
pub async fn resolve_thread(app: AppHandle, thread_id: String, resolve: bool) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let doc = if resolve {
        "mutation($threadId: ID!) {
           resolveReviewThread(input: { threadId: $threadId }) { clientMutationId }
         }"
    } else {
        "mutation($threadId: ID!) {
           unresolveReviewThread(input: { threadId: $threadId }) { clientMutationId }
         }"
    };
    client.run(doc, &serde_json::json!({ "threadId": thread_id })).await?;
    let _ = app.emit(events::REVIEWS_CHANGED, ());
    Ok(())
}

/// Submit a review: approve / request-changes / comment. Returns the review
/// GitHub created — its own read-back lags the mutation by a second or more,
/// so the UI shows this until a refetch agrees.
#[tauri::command]
pub async fn submit_review(
    app: AppHandle,
    pr_id: String,
    event: String,
    body: String,
) -> AppResult<crate::models::ReviewVerdict> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let gh_event = match event.as_str() {
        "approve" => "APPROVE",
        "request-changes" | "request_changes" => "REQUEST_CHANGES",
        "comment" => "COMMENT",
        other => {
            return Err(AppError::Other(format!(
                "unknown review event '{other}' — expected approve, request-changes, or comment"
            )))
        }
    };
    if gh_event == "REQUEST_CHANGES" && body.trim().is_empty() {
        return Err(AppError::Other("requesting changes needs a comment".into()));
    }
    let doc = format!(
        "mutation($prId: ID!, $body: String) {{
           addPullRequestReview(input: {{ pullRequestId: $prId, event: {gh_event}, body: $body }}) {{
             pullRequestReview {{
               id
               state
               body
               url
               submittedAt
               author {{ login }}
             }}
           }}
         }}"
    );
    let data = client
        .run(&doc, &serde_json::json!({ "prId": pr_id, "body": body }))
        .await?;
    let created = data.pointer("/addPullRequestReview/pullRequestReview");
    let str_at = |path: &str| {
        created
            .and_then(|r| r.pointer(path))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let verdict = crate::models::ReviewVerdict {
        id: str_at("/id"),
        author: str_at("/author/login"),
        // GitHub echoes the resulting state; fall back to the event we sent.
        state: match str_at("/state").as_str() {
            "" => match gh_event {
                "APPROVE" => "APPROVED".into(),
                "REQUEST_CHANGES" => "CHANGES_REQUESTED".into(),
                _ => "COMMENTED".into(),
            },
            s => s.to_string(),
        },
        body: body.clone(),
        submitted_at: str_at("/submittedAt"),
        url: str_at("/url"),
    };
    let label = pr_label(&store, &pr_id);
    let action = match gh_event {
        "APPROVE" => "approved",
        "REQUEST_CHANGES" => "changes-requested",
        _ => "review-commented",
    };
    store.add_audit(action, &pr_id, &label, "", gh_event)?;
    // Before the refresh below emits REVIEWS_CHANGED — whoever refetches then
    // may still get pre-mutation data back from GitHub.
    let _ = app.emit(
        events::REVIEW_SUBMITTED,
        crate::models::ReviewSubmittedEvent { pr_id: pr_id.clone(), verdict: verdict.clone() },
    );
    refresh_pr_inner(&app, &pr_id, false).await?;
    Ok(verdict)
}

/// Merge / close / reopen — the PR lifecycle controls. All audited.
#[tauri::command]
pub async fn merge_pr(app: AppHandle, pr_id: String, method: String) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let merge_method = match method.as_str() {
        "merge" => "MERGE",
        "rebase" => "REBASE",
        _ => "SQUASH",
    };
    let doc = format!(
        "mutation($prId: ID!) {{
           mergePullRequest(input: {{ pullRequestId: $prId, mergeMethod: {merge_method} }}) {{
             clientMutationId
           }}
         }}"
    );
    client.run(&doc, &serde_json::json!({ "prId": pr_id })).await?;
    let label = pr_label(&store, &pr_id);
    store.add_audit("merged", &pr_id, &label, "open", &format!("merged ({method})"))?;
    refresh_pr_inner(&app, &pr_id, true).await?;
    Ok(())
}

#[tauri::command]
pub async fn close_pr(app: AppHandle, pr_id: String) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    client
        .run(
            "mutation($prId: ID!) {
               closePullRequest(input: { pullRequestId: $prId }) { clientMutationId }
             }",
            &serde_json::json!({ "prId": pr_id }),
        )
        .await?;
    let label = pr_label(&store, &pr_id);
    store.add_audit("closed", &pr_id, &label, "open", "closed")?;
    refresh_pr_inner(&app, &pr_id, true).await?;
    Ok(())
}

#[tauri::command]
pub async fn reopen_pr(app: AppHandle, pr_id: String) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    client
        .run(
            "mutation($prId: ID!) {
               reopenPullRequest(input: { pullRequestId: $prId }) { clientMutationId }
             }",
            &serde_json::json!({ "prId": pr_id }),
        )
        .await?;
    // Closing untracked it, so bring it back before anything reads the stored
    // row — the label, the audit entry and the refresh all go through `get_pr`,
    // which only sees tracked PRs.
    store.retrack(&pr_id)?;
    let label = pr_label(&store, &pr_id);
    store.add_audit("reopened", &pr_id, &label, "closed", "open")?;
    refresh_pr_inner(&app, &pr_id, false).await?;
    Ok(())
}

// -- assistant chat -----------------------------------------------------------

#[tauri::command]
pub fn chat_history(
    app: AppHandle,
    pr_id: String,
) -> AppResult<crate::analysis::types::ChatTranscript> {
    crate::analysis::chat::transcript(&app, &pr_id)
}

#[tauri::command]
pub fn chat_send(app: AppHandle, window: WebviewWindow, pr_id: String, text: String) -> AppResult<()> {
    require_main(&window)?;
    crate::analysis::chat::send(app, pr_id, text)
}

/// `edited` carries the text as the user rewrote it in the confirm card,
/// when they changed it.
#[tauri::command]
pub fn chat_confirm(
    app: AppHandle,
    window: WebviewWindow,
    pr_id: String,
    approve: bool,
    edited: Option<String>,
) -> AppResult<()> {
    require_main(&window)?;
    crate::analysis::chat::confirm(app, pr_id, approve, edited)
}

#[tauri::command]
pub fn chat_clear(app: AppHandle, pr_id: String) -> AppResult<()> {
    crate::analysis::chat::clear(&app, &pr_id)
}

/// Token spend across every Bedrock request this org has made. All of it is
/// blocking — a whole table read, a settings parse, and a scan of the Claude
/// settings files — so it runs off the async runtime's workers.
#[tauri::command]
pub async fn get_usage_stats(app: AppHandle) -> AppResult<crate::usage::UsageStats> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<crate::orgs::Orgs>().active();
        let rows = store.usage_rows()?;
        let prices = store.settings()?.model_prices;
        // Read the ARN → model mapping fresh: editing a Claude settings file
        // should reprice the dashboard on the next look, with nothing to migrate.
        let aliases = crate::usage::claude_settings_aliases();
        Ok(crate::usage::summarize(
            &rows,
            &prices,
            &aliases,
            &chrono::Utc::now().to_rfc3339(),
        ))
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

/// Set (or clear, with a zero rate) the price for one model id.
#[tauri::command]
pub fn set_model_price(
    app: AppHandle,
    model: String,
    input_per_mtok: f64,
    output_per_mtok: f64,
) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let mut settings = store.settings()?;
    settings.model_prices.retain(|p| p.model != model);
    if input_per_mtok > 0.0 || output_per_mtok > 0.0 {
        settings.model_prices.push(crate::models::ModelPrice {
            model,
            input_per_mtok,
            output_per_mtok,
        });
    }
    store.save_settings(&settings)
}

/// What the assistant will see on its next turn, itemised. `include_text`
/// false returns sizes only — the panel's running total asks for that on
/// every chat event, and tool results can be megabytes. Async so assembling
/// and serialising a large context stays off the UI thread.
#[tauri::command]
pub async fn get_chat_context(
    app: AppHandle,
    pr_id: String,
    include_text: bool,
) -> AppResult<crate::analysis::types::ChatContext> {
    crate::analysis::chat::context(&app, &pr_id, include_text)
}

/// The store → PR → token → settings dance shared by every REST-backed
/// per-PR command.
fn repo_tools_for(
    app: &AppHandle,
    pr_id: &str,
) -> AppResult<(crate::analysis::tools::RepoTools, TrackedPr)> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let tools = crate::analysis::tools::RepoTools::new(
        &settings.github_graphql_url,
        &pr.info.repo,
        pr.info.number,
        &pr.info.head_sha,
        &token,
    )?;
    Ok((tools, pr))
}

/// New-side (RIGHT) line numbers a review comment can anchor to for `path`:
/// every added and context line inside a hunk. GitHub rejects an anchor on any
/// other line — unchanged code outside the changed hunks, or the removed side.
fn commentable_new_lines(diff: &str, path: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut in_file = false;
    let mut in_hunk = false;
    let mut new_ln = 0i64;
    for raw in diff.lines() {
        if raw.starts_with("diff --git ") {
            in_file = false;
            in_hunk = false;
        } else if let Some(p) = raw.strip_prefix("+++ b/") {
            in_file = p == path;
            in_hunk = false;
        } else if raw.starts_with("+++ ") || raw.starts_with("--- ") {
            in_hunk = false;
        } else if in_file && raw.starts_with("@@") {
            new_ln = hunk_new_start(raw);
            in_hunk = true;
        } else if in_file && in_hunk {
            match raw.as_bytes().first() {
                // Added or context line: present on the RIGHT side, commentable.
                Some(b'+') | Some(b' ') => {
                    out.push(new_ln);
                    new_ln += 1;
                }
                Some(b'-') => {} // removed: left side, no new-side line
                Some(b'\\') => {} // "\ No newline at end of file"
                _ => in_hunk = false, // blank/other → past the hunk body
            }
        }
    }
    out
}

/// The new-side start line `c` from a `@@ -a,b +c,d @@` hunk header.
fn hunk_new_start(hunk: &str) -> i64 {
    hunk
        .split('+')
        .nth(1)
        .and_then(|s| s.split([',', ' ']).next())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(1)
}

/// Collapse a sorted line list into compact ranges: `[1,2,3,7] → "1-3, 7"`.
fn format_line_ranges(lines: &[i64]) -> String {
    let mut v = lines.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        while i + 1 < v.len() && v[i + 1] == v[i] + 1 {
            i += 1;
        }
        parts.push(if v[i] == start {
            start.to_string()
        } else {
            format!("{start}-{}", v[i])
        });
        i += 1;
    }
    parts.join(", ")
}

/// Turn a rejected anchor into a message that says where a comment *can* go.
/// `None` means the anchor wasn't the problem — let the original error stand.
async fn anchor_hint(
    tools: &crate::analysis::tools::RepoTools,
    path: &str,
    line: i64,
    start_line: Option<i64>,
) -> Option<String> {
    let diff = tools.pr_diff_full().await.ok()?;
    let allowed = commentable_new_lines(&diff, path);
    if allowed.is_empty() {
        return Some(format!(
            "Can't anchor a review comment on {path} — it has no changed lines in this PR's diff. Comment in the PR conversation instead."
        ));
    }
    let anchor_bad = !allowed.contains(&line)
        || start_line.is_some_and(|s| s < line && !allowed.contains(&s));
    anchor_bad.then(|| {
        format!(
            "Can't anchor a review comment at {path}:{line} — that line isn't in the PR diff, so GitHub rejected it. Anchor to a changed line; commentable new-side lines for {path}: {}.",
            format_line_ranges(&allowed),
        )
    })
}

/// New line-anchored review comment from the diff view. Uses the REST
/// endpoint because it creates a standalone comment without a pending review.
#[tauri::command]
pub async fn add_diff_comment(
    app: AppHandle,
    pr_id: String,
    path: String,
    line: i64,
    body: String,
    start_line: Option<i64>,
) -> AppResult<()> {
    if body.trim().is_empty() {
        return Err(AppError::Other("comment is empty".into()));
    }
    let (tools, _) = repo_tools_for(&app, &pr_id)?;
    let mut payload = serde_json::json!({
        "body": body,
        "commit_id": tools.head_ref(),
        "path": path,
        "line": line,
        "side": "RIGHT",
    });
    // A multi-line anchor is what lets a suggestion replace a whole block
    // rather than one line. GitHub wants the first line of the range, and
    // rejects a start that isn't above `line`.
    if let Some(start) = start_line.filter(|s| *s < line) {
        payload["start_line"] = serde_json::json!(start);
        payload["start_side"] = serde_json::json!("RIGHT");
    }
    let url = format!("repos/{}/pulls/{}/comments", tools.repo(), tools.pr_number());
    if let Err(e) = tools.post(&url, &payload).await {
        // A review comment must anchor to a line in the diff's changed hunks
        // (RIGHT side); GitHub's 422 for a bad anchor is opaque, and the caller
        // otherwise re-reads the whole diff to recover. When that's the cause,
        // replace the error with the lines that ARE commentable, so it (the
        // assistant, or a user) re-anchors in one step. Other errors pass through.
        if e.status() == Some(422) {
            if let Some(hint) = anchor_hint(&tools, &path, line, start_line).await {
                return Err(AppError::Other(hint));
            }
        }
        return Err(e);
    }
    // A new thread of yours can gate Approve — tell the UI to refetch.
    let _ = app.emit(events::REVIEWS_CHANGED, ());
    Ok(())
}

/// Toggle an emoji reaction on any comment.
#[tauri::command]
pub async fn toggle_reaction(
    app: AppHandle,
    subject_id: String,
    content: String,
    remove: bool,
) -> AppResult<()> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));
    let doc = if remove {
        "mutation($subjectId: ID!, $content: ReactionContent!) {
           removeReaction(input: { subjectId: $subjectId, content: $content }) { clientMutationId }
         }"
    } else {
        "mutation($subjectId: ID!, $content: ReactionContent!) {
           addReaction(input: { subjectId: $subjectId, content: $content }) { clientMutationId }
         }"
    };
    client
        .run(doc, &serde_json::json!({ "subjectId": subject_id, "content": content }))
        .await?;
    Ok(())
}

/// Conversation + review threads for the Comments tab.
#[tauri::command]
pub async fn get_pr_comments(app: AppHandle, pr_id: String) -> AppResult<PrConversation> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let (owner, name) = pr.info.repo.split_once('/').unwrap();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));

    let doc = "query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) {
        pullRequest(number: $number) {
          comments(first: 100) {
            nodes {
              id author { login __typename } body createdAt url viewerCanUpdate
              reactionGroups { content viewerHasReacted reactors { totalCount } }
            }
          }
          reviewThreads(first: 100) {
            nodes {
              id isResolved isOutdated path line startLine
              comments(first: 100) {
                nodes {
                  id author { login __typename } body createdAt url viewerCanUpdate
                  reactionGroups { content viewerHasReacted reactors { totalCount } }
                }
              }
            }
          }
          reviews(first: 50) {
            nodes { id author { login } state body submittedAt url }
          }
        }
      }
    }";
    let data = client
        .run(doc, &serde_json::json!({ "owner": owner, "name": name, "number": pr.info.number }))
        .await?;

    let parse_comment = |v: &serde_json::Value, is_review_comment: bool| -> Option<PrComment> {
        let author = v
            .pointer("/author/login")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("ghost")
            .to_string();
        let is_bot = v.pointer("/author/__typename").and_then(serde_json::Value::as_str)
            == Some("Bot")
            || crate::models::is_bot_login(&author);
        let reactions = v
            .get("reactionGroups")
            .and_then(serde_json::Value::as_array)
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|g| {
                        let count = g.pointer("/reactors/totalCount")?.as_i64()?;
                        if count == 0 {
                            return None; // only surface reactions that exist
                        }
                        Some(crate::models::ReactionGroup {
                            content: g.get("content")?.as_str()?.to_string(),
                            count,
                            viewer_has_reacted: g
                                .get("viewerHasReacted")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(PrComment {
            id: v.get("id")?.as_str()?.to_string(),
            author,
            is_bot,
            body: v.get("body")?.as_str()?.to_string(),
            created_at: v.get("createdAt")?.as_str()?.to_string(),
            url: v.get("url")?.as_str()?.to_string(),
            reactions,
            viewer_can_edit: v
                .get("viewerCanUpdate")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            is_review_comment,
        })
    };

    let comments = data
        .pointer("/repository/pullRequest/comments/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| nodes.iter().filter_map(|c| parse_comment(c, false)).collect())
        .unwrap_or_default();

    let threads = data
        .pointer("/repository/pullRequest/reviewThreads/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|t| {
                    Some(ReviewThread {
                        id: t.get("id")?.as_str()?.to_string(),
                        path: t.get("path").and_then(serde_json::Value::as_str).map(String::from),
                        line: t.get("line").and_then(serde_json::Value::as_i64),
                        start_line: t.get("startLine").and_then(serde_json::Value::as_i64),
                        resolved: t.get("isResolved").and_then(serde_json::Value::as_bool).unwrap_or(false),
                        outdated: t.get("isOutdated").and_then(serde_json::Value::as_bool).unwrap_or(false),
                        comments: t
                            .pointer("/comments/nodes")
                            .and_then(serde_json::Value::as_array)
                            .map(|nodes| nodes.iter().filter_map(|c| parse_comment(c, true)).collect())
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Verdicts worth showing: a decision, or a summary body. Bare COMMENTED
    // reviews (one per inline-comment batch) are noise — their content is
    // already in the threads.
    let reviews = data
        .pointer("/repository/pullRequest/reviews/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|r| {
                    let state = r.get("state")?.as_str()?.to_string();
                    let body = r.get("body").and_then(serde_json::Value::as_str).unwrap_or("");
                    if state == "PENDING" || (state == "COMMENTED" && body.trim().is_empty()) {
                        return None;
                    }
                    Some(crate::models::ReviewVerdict {
                        id: r.get("id")?.as_str()?.to_string(),
                        author: r
                            .pointer("/author/login")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("ghost")
                            .to_string(),
                        state,
                        body: body.to_string(),
                        submitted_at: r
                            .get("submittedAt")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        url: r.get("url").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(PrConversation { comments, threads, reviews })
}

/// Commit history for the PR's History tab, oldest first.
#[tauri::command]
pub async fn get_pr_commits(
    app: AppHandle,
    pr_id: String,
) -> AppResult<Vec<crate::models::PrCommit>> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let pr = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let (owner, name) = pr.info.repo.split_once('/').unwrap();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?.with_health(crate::github::query::GraphQlClient::shared_health(&app));

    let doc = "query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) {
        pullRequest(number: $number) {
          commits(first: 250) {
            nodes { commit {
              oid abbreviatedOid messageHeadline committedDate additions deletions url
              author { name user { login } }
              statusCheckRollup { state }
            } }
          }
        }
      }
    }";
    let data = client
        .run(doc, &serde_json::json!({ "owner": owner, "name": name, "number": pr.info.number }))
        .await?;

    Ok(data
        .pointer("/repository/pullRequest/commits/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    let c = n.get("commit")?;
                    Some(crate::models::PrCommit {
                        sha: c.get("oid")?.as_str()?.to_string(),
                        short_sha: c
                            .get("abbreviatedOid")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        message: c
                            .get("messageHeadline")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        author: c
                            .pointer("/author/user/login")
                            .or_else(|| c.pointer("/author/name"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        at: c
                            .get("committedDate")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        additions: c.get("additions").and_then(serde_json::Value::as_i64).unwrap_or(0),
                        deletions: c.get("deletions").and_then(serde_json::Value::as_i64).unwrap_or(0),
                        ci_status: c
                            .pointer("/statusCheckRollup/state")
                            .and_then(serde_json::Value::as_str)
                            .map(String::from),
                        url: c.get("url").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Full file contents at the PR head, for the comment code drawer.
#[tauri::command]
pub async fn get_file_at_head(app: AppHandle, pr_id: String, path: String) -> AppResult<String> {
    let (tools, _) = repo_tools_for(&app, &pr_id)?;
    tools
        .execute("get_file", &serde_json::json!({ "path": path }))
        .await
}

/// Raw unified diff for the Diff tab.
#[tauri::command]
pub async fn get_pr_diff(app: AppHandle, pr_id: String) -> AppResult<String> {
    let (tools, _) = repo_tools_for(&app, &pr_id)?;
    tools.pr_diff_full().await
}

// -- review marks ("changes since my last look") ---------------------------------

/// Where the reviewer left off — created at the current head on first ask, so
/// every surface shares one planting rule and one stored timestamp.
#[tauri::command]
pub fn ensure_review_mark(orgs: State<'_, crate::orgs::Orgs>, pr_id: String) -> AppResult<ReviewMark> {
    let store = orgs.active();
    if let Some(mark) = store.review_mark(&pr_id)? {
        return Ok(mark);
    }
    let pr = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    store.set_review_mark(&pr_id, &pr.info.head_sha)
}

/// Record "I'm caught up here" at the PR's current head.
#[tauri::command]
pub fn set_review_mark(orgs: State<'_, crate::orgs::Orgs>, pr_id: String) -> AppResult<ReviewMark> {
    let store = orgs.active();
    let pr = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    store.set_review_mark(&pr_id, &pr.info.head_sha)
}

/// Diff of only the commits pushed since the review mark.
#[tauri::command]
pub async fn get_diff_since(app: AppHandle, pr_id: String) -> AppResult<String> {
    let store = app.state::<crate::orgs::Orgs>().active();
    let mark = store
        .review_mark(&pr_id)?
        .ok_or_else(|| AppError::Other("no review mark for this PR".into()))?;
    let (tools, _) = repo_tools_for(&app, &pr_id)?;
    tools.compare_diff(&mark.head_sha).await
}

/// Frontend crash reporter: devlog + a file we can read even when the UI is
/// a white screen.
#[tauri::command]
pub fn log_frontend_error(app: AppHandle, message: String) {
    crate::devlog::error(&app, "ui", message.clone());
    if let Ok(dir) = app.path().app_data_dir() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("frontend-errors.log"))
        {
            let _ = writeln!(f, "{} {}", chrono::Utc::now().to_rfc3339(), message);
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewedFile {
    pub path: String,
    pub digest: String,
}

#[tauri::command]
pub fn get_viewed_files(
    orgs: State<'_, crate::orgs::Orgs>,
    pr_id: String,
) -> AppResult<Vec<ViewedFile>> {
    let store = orgs.active();
    Ok(store
        .viewed_files(&pr_id)?
        .into_iter()
        .map(|(path, digest)| ViewedFile { path, digest })
        .collect())
}

#[tauri::command]
pub fn set_file_viewed(
    orgs: State<'_, crate::orgs::Orgs>,
    pr_id: String,
    path: String,
    digest: String,
    viewed: bool,
) -> AppResult<()> {
    let store = orgs.active();
    store.set_file_viewed(&pr_id, &path, &digest, viewed)
}

/// Jump to macOS System Settings → Notifications.
#[tauri::command]
pub fn open_notification_settings() -> AppResult<()> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.Notifications-Settings.extension")
        .spawn()
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

/// Claim the pending notification deep-link, if fresh. Called by the main
/// window when it gains focus.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingFocusPayload {
    pub pr_id: String,
    pub comment_id: Option<String>,
}

#[tauri::command]
pub fn take_pending_focus(
    pending: State<'_, crate::notify::PendingFocus>,
) -> Option<PendingFocusPayload> {
    pending.take().map(|t| PendingFocusPayload {
        pr_id: t.pr_id,
        comment_id: t.comment_id,
    })
}

// -- developer mode ---------------------------------------------------------------

#[tauri::command]
pub fn get_dev_logs(logs: State<'_, crate::devlog::DevLog>) -> Vec<crate::devlog::LogEntry> {
    logs.entries()
}

#[tauri::command]
pub fn clear_dev_logs(logs: State<'_, crate::devlog::DevLog>) {
    logs.clear();
}

/// The built-in analysis system prompt, so the Developer pane can show and
/// diff against it.
#[tauri::command]
pub fn get_default_system_prompt() -> String {
    crate::analysis::engine::SYSTEM_PROMPT.to_string()
}

/// Which build is running — version, dev vs release, branch and commit.
/// Cheap and constant, so the Settings footer can show it without gating it
/// behind Developer mode the way the rest of the internals are.
#[tauri::command]
pub fn get_build_info() -> crate::build_info::BuildInfo {
    crate::build_info::BuildInfo::current()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInternals {
    pub data_dir: String,
    pub db_path: String,
    pub version: String,
}

#[tauri::command]
pub fn get_app_internals(app: AppHandle) -> AppResult<AppInternals> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(AppInternals {
        db_path: data_dir.join("cora.sqlite").display().to_string(),
        data_dir: data_dir.display().to_string(),
        version: app.package_info().version.to_string(),
    })
}

// -- AWS session helpers ---------------------------------------------------------

/// Run `aws sso login` for the configured profile. Opens the browser and
/// blocks until the CLI reports the session is established.
#[tauri::command]
pub async fn aws_sso_login(window: WebviewWindow, profile: String) -> AppResult<()> {
    require_main(&window)?;
    if profile.trim().is_empty() {
        return Err(AppError::Other("no AWS profile configured".into()));
    }
    let output = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new("aws")
            .args(["sso", "login", "--profile", profile.trim()])
            .output()
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
    .map_err(|e| {
        AppError::Other(format!(
            "could not run the AWS CLI ({e}) — is `aws` installed and on PATH?"
        ))
    })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(AppError::Other(stderr.trim().to_string()))
    }
}

/// Cheap credentials probe: resolves the profile's credential chain without
/// calling Bedrock. Catches expired SSO sessions and missing profiles.
#[tauri::command]
pub async fn check_aws(profile: String, region: String) -> AppResult<String> {
    use aws_credential_types::provider::ProvideCredentials;
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if !profile.is_empty() {
        loader = loader.profile_name(&profile);
    }
    if !region.is_empty() {
        loader = loader.region(aws_config::Region::new(region));
    }
    let config = loader.load().await;
    let provider = config
        .credentials_provider()
        .ok_or_else(|| AppError::Other("no credentials provider for this profile".into()))?;
    provider.provide_credentials().await.map_err(|e| {
        AppError::Other(format!(
            "{}",
            aws_smithy_types::error::display::DisplayErrorContext(&e)
        ))
    })?;
    Ok("credentials resolved".into())
}

// -- windows / poll control ----------------------------------------------------

#[tauri::command]
pub fn poll_now(trigger: State<'_, PollTrigger>) {
    trigger.0.notify_waiters();
}

/// Callout rows call this: surface the main window on a specific PR.
#[tauri::command]
pub fn show_main_window(app: AppHandle, pr_id: Option<String>) -> AppResult<()> {
    if let Some(main) = app.get_webview_window("main") {
        // An explicit click is authoritative. Drop any armed notification
        // deep-link (e.g. a just-completed analysis for a *different* PR)
        // first, so the focus-triggered claim from `set_focus` below can't
        // race the FOCUS_PR emit and land on that other PR instead.
        if pr_id.is_some() {
            let _ = app.state::<crate::notify::PendingFocus>().take();
        }
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
        if let Some(id) = pr_id {
            let _ = app.emit_to("main", events::FOCUS_PR, id);
        }
    }
    Ok(())
}

/// Double-clicking a callout stat tile lands the main window on that bucket.
#[tauri::command]
pub fn show_main_filtered(app: AppHandle, bucket: String) -> AppResult<()> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
        let _ = app.emit_to("main", "focus:bucket", bucket);
    }
    Ok(())
}

#[tauri::command]
pub fn toggle_callout(app: AppHandle) -> AppResult<()> {
    if let Some(callout) = app.get_webview_window("callout") {
        if callout.is_visible().unwrap_or(false) {
            // Capture the position/size before hiding — a hidden window
            // reports nothing useful to the state plugin.
            crate::flush_window_state(&app);
            let _ = callout.hide();
        } else {
            // A monitor may have changed while it was hidden — make sure it
            // comes back somewhere reachable.
            crate::ensure_callout_on_screen(&callout);
            let _ = callout.show();
        }
    }
    Ok(())
}

/// Force the callout back to a visible, grabbable spot — recovery for when a
/// monitor change has stranded it off-screen and it's still "visible" (so a
/// toggle wouldn't re-place it). Reachable from the tray.
#[tauri::command]
pub fn reset_callout_position(app: AppHandle) -> AppResult<()> {
    if let Some(callout) = app.get_webview_window("callout") {
        crate::ensure_callout_on_screen(&callout);
        let _ = callout.show();
        let _ = callout.set_focus();
        crate::flush_window_state(&app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/foo.ts b/foo.ts
index e69de29..abc1234 100644
--- a/foo.ts
+++ b/foo.ts
@@ -1,3 +1,4 @@
 line1
+added2
 line3
 line4
@@ -20,2 +21,3 @@
 ctx21
+added22
 ctx23
diff --git a/bar.ts b/bar.ts
index 111..222 100644
--- a/bar.ts
+++ b/bar.ts
@@ -5,2 +5,2 @@
-gone
+new6
 ctx7
";

    #[test]
    fn commentable_lines_are_added_and_context_only() {
        // Added + context on the RIGHT side; removed lines don't advance it.
        assert_eq!(commentable_new_lines(DIFF, "foo.ts"), vec![1, 2, 3, 4, 21, 22, 23]);
        assert_eq!(commentable_new_lines(DIFF, "bar.ts"), vec![5, 6]);
    }

    #[test]
    fn a_file_not_in_the_diff_has_none() {
        assert!(commentable_new_lines(DIFF, "nope.ts").is_empty());
    }

    #[test]
    fn hunk_start_parses_the_new_side() {
        assert_eq!(hunk_new_start("@@ -1,3 +1,4 @@"), 1);
        assert_eq!(hunk_new_start("@@ -20,2 +21,3 @@ fn foo()"), 21);
        assert_eq!(hunk_new_start("@@ -0,0 +1 @@"), 1);
    }

    #[test]
    fn ranges_collapse_contiguous_runs() {
        assert_eq!(format_line_ranges(&[1, 2, 3, 4, 21, 22, 23]), "1-4, 21-23");
        assert_eq!(format_line_ranges(&[5]), "5");
        assert_eq!(format_line_ranges(&[3, 1, 2, 7]), "1-3, 7");
    }
}

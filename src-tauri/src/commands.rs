use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::analysis::types::{events as analysis_events, AnalysisLevel, AnalysisResult};
use crate::error::{AppError, AppResult};
use crate::github::poller::PollTrigger;
use crate::github::query::{GraphQlClient, PR_FRAGMENT};
use crate::github::{parse_pr, parse_pr_url};
use crate::models::{
    events, ChangeKind, PrComment, PrConversation, PrSource, ReviewThread, Settings, TrackedPr,
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
pub fn get_settings(store: State<'_, Arc<Store>>) -> AppResult<Settings> {
    store.settings()
}

#[tauri::command]
pub fn set_settings(
    window: WebviewWindow,
    store: State<'_, Arc<Store>>,
    trigger: State<'_, PollTrigger>,
    settings: Settings,
) -> AppResult<()> {
    require_main(&window)?;
    store.save_settings(&settings)?;
    trigger.0.notify_one();
    Ok(())
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
    trigger.0.notify_one();
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
pub fn list_prs(store: State<'_, Arc<Store>>) -> AppResult<Vec<TrackedPr>> {
    store.list_prs()
}

#[tauri::command]
pub fn mark_pr_read(app: AppHandle, store: State<'_, Arc<Store>>, id: String) -> AppResult<()> {
    store.mark_read(&id)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
    Ok(())
}

#[tauri::command]
pub fn set_pr_muted(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    id: String,
    muted: bool,
) -> AppResult<()> {
    store.set_muted(&id, muted)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
    Ok(())
}

#[tauri::command]
pub fn untrack_pr(app: AppHandle, store: State<'_, Arc<Store>>, id: String) -> AppResult<()> {
    store.untrack(&id)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
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
    let store = app.state::<Arc<Store>>().inner().clone();
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?;
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
        .ok_or_else(|| AppError::GitHub(format!("PR {repo}#{number} not found")))?;
    let info = parse_pr(node).ok_or_else(|| AppError::GitHub("unexpected PR shape".into()))?;

    let now = Utc::now().to_rfc3339();
    let stored = store.upsert_pr(&info, &[PrSource::Manual], &[ChangeKind::New], &now)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
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
    store: State<'_, Arc<Store>>,
    pr_id: String,
    level: AnalysisLevel,
    focus: Option<String>,
) -> AppResult<Option<AnalysisResult>> {
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
    runs: State<'_, AnalysisRuns>,
    pr_id: String,
    level: AnalysisLevel,
    focus: Option<String>,
    force: Option<bool>,
) -> AppResult<()> {
    require_main(&window)?;
    let key = analysis_key(&pr_id, level, &focus);
    {
        let mut running = runs.0.lock().unwrap();
        if !running.insert(key.clone()) {
            return Ok(()); // already running
        }
    }
    let force = force.unwrap_or(false);

    tauri::async_runtime::spawn(async move {
        let outcome = execute_analysis(&app, &pr_id, level, focus.clone(), force).await;
        {
            let runs = app.state::<AnalysisRuns>();
            runs.0.lock().unwrap().remove(&key);
        }
        match outcome {
            Ok(result) => {
                let _ = app.emit(analysis_events::ANALYSIS_COMPLETE, result);
            }
            Err(e) => {
                let error = e.to_string();
                let _ = app.emit(
                    analysis_events::ANALYSIS_ERROR,
                    crate::analysis::types::AnalysisError {
                        pr_id,
                        level,
                        kind: crate::analysis::types::classify_error(&error),
                        error,
                    },
                );
            }
        }
    });
    Ok(())
}

async fn execute_analysis(
    app: &AppHandle,
    pr_id: &str,
    level: AnalysisLevel,
    focus: Option<String>,
    force: bool,
) -> AppResult<AnalysisResult> {
    let store = app.state::<Arc<Store>>().inner().clone();
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

    let result = crate::analysis::engine::run(app, &settings, &token, &pr, level, focus).await?;

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
    store: State<'_, Arc<Store>>,
    id: String,
    priority: crate::models::PrPriority,
) -> AppResult<()> {
    store.set_pr_priority(&id, priority)?;
    let _ = app.emit(events::PRS_SNAPSHOT, store.list_prs()?);
    Ok(())
}

/// Conversation + review threads for the Comments tab.
#[tauri::command]
pub async fn get_pr_comments(app: AppHandle, pr_id: String) -> AppResult<PrConversation> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let pr = store
        .get_pr(&pr_id)?
        .ok_or_else(|| AppError::Other("PR not found".into()))?;
    let (owner, name) = pr.info.repo.split_once('/').unwrap();
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = store.settings()?;
    let client = GraphQlClient::new(&settings.github_graphql_url, &token)?;

    let doc = "query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) {
        pullRequest(number: $number) {
          comments(first: 100) {
            nodes { id author { login } body createdAt url }
          }
          reviewThreads(first: 100) {
            nodes {
              id isResolved isOutdated path line startLine
              comments(first: 100) { nodes { id author { login } body createdAt url } }
            }
          }
        }
      }
    }";
    let data = client
        .run(doc, &serde_json::json!({ "owner": owner, "name": name, "number": pr.info.number }))
        .await?;

    let parse_comment = |v: &serde_json::Value| -> Option<PrComment> {
        Some(PrComment {
            id: v.get("id")?.as_str()?.to_string(),
            author: v
                .pointer("/author/login")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("ghost")
                .to_string(),
            body: v.get("body")?.as_str()?.to_string(),
            created_at: v.get("createdAt")?.as_str()?.to_string(),
            url: v.get("url")?.as_str()?.to_string(),
        })
    };

    let comments = data
        .pointer("/repository/pullRequest/comments/nodes")
        .and_then(serde_json::Value::as_array)
        .map(|nodes| nodes.iter().filter_map(parse_comment).collect())
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
                            .map(|nodes| nodes.iter().filter_map(parse_comment).collect())
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(PrConversation { comments, threads })
}

/// Full file contents at the PR head, for the comment code drawer.
#[tauri::command]
pub async fn get_file_at_head(app: AppHandle, pr_id: String, path: String) -> AppResult<String> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let pr = store
        .get_pr(&pr_id)?
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
    tools
        .execute("get_file", &serde_json::json!({ "path": path }))
        .await
}

/// Raw unified diff for the Diff tab.
#[tauri::command]
pub async fn get_pr_diff(app: AppHandle, pr_id: String) -> AppResult<String> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let pr = store
        .get_pr(&pr_id)?
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
    tools.pr_diff_full().await
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
    trigger.0.notify_one();
}

/// Callout rows call this: surface the main window on a specific PR.
#[tauri::command]
pub fn show_main_window(app: AppHandle, pr_id: Option<String>) -> AppResult<()> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
        if let Some(id) = pr_id {
            let _ = app.emit_to("main", events::FOCUS_PR, id);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn toggle_callout(app: AppHandle) -> AppResult<()> {
    if let Some(callout) = app.get_webview_window("callout") {
        if callout.is_visible().unwrap_or(false) {
            let _ = callout.hide();
        } else {
            let _ = callout.show();
        }
    }
    Ok(())
}

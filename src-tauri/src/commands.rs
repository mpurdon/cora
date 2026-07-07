use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::error::{AppError, AppResult};
use crate::github::poller::PollTrigger;
use crate::github::query::{GraphQlClient, PR_FRAGMENT};
use crate::github::{parse_pr, parse_pr_url};
use crate::models::{events, ChangeKind, PrSource, Settings, TrackedPr};
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

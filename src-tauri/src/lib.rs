mod analysis;
mod commands;
mod devlog;
mod error;
mod github;
mod models;
mod notify;
mod orgs;
mod secrets;
mod store;
mod usage;

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tokio::sync::Notify;

use github::poller::PollTrigger;

/// Persist window geometry — never visibility; our own setting governs
/// whether the callout shows at launch.
pub(crate) fn flush_window_state(app: &tauri::AppHandle) {
    use tauri_plugin_window_state::{AppHandleExt, StateFlags};
    let _ = app.save_window_state(StateFlags::all() - StateFlags::VISIBLE);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            // Remember positions/sizes, but let our own setting govern
            // whether the callout is visible at launch.
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            // Per-org isolated stores; opening runs the one-time legacy
            // split migration on first launch with org support.
            app.manage(orgs::Orgs::open(&data_dir)?);
            app.manage(PollTrigger(Arc::new(Notify::new())));
            app.manage(github::poller::OrgLoops::default());
            app.manage(commands::AnalysisRuns(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )));
            app.manage(devlog::DevLog::new());
            app.manage(notify::PendingFocus::new());
            app.manage(analysis::chat::ChatSessions::default());

            // Respect the "open callout at launch" preference.
            let show_callout = app
                .state::<orgs::Orgs>()
                .active()
                .settings()
                .map(|s| s.show_callout_on_startup)
                .unwrap_or(true);
            if !show_callout {
                if let Some(callout) = app.get_webview_window("callout") {
                    let _ = callout.hide();
                }
            }

            setup_tray(app.handle())?;
            github::poller::spawn_all(app.handle().clone());

            // Closing the main window hides it (tray app); callout stays up.
            if let Some(main) = app.get_webview_window("main") {
                let main_clone = main.clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = main_clone.hide();
                    }
                });
            }

            // The callout is never "closed", only hidden — and dev restarts /
            // Cmd+Q don't pass through the tray quit handler — so the
            // window-state plugin's own save hooks rarely fire for it.
            // Persist explicitly: throttled while dragging/resizing, and on
            // every focus loss (catches the end of a drag).
            if let Some(callout) = app.get_webview_window("callout") {
                let handle = app.handle().clone();
                let last_save = std::sync::Mutex::new(std::time::Instant::now());
                callout.on_window_event(move |event| match event {
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                        let mut last = last_save.lock().unwrap();
                        if last.elapsed() >= std::time::Duration::from_millis(800) {
                            *last = std::time::Instant::now();
                            flush_window_state(&handle);
                        }
                    }
                    tauri::WindowEvent::Focused(false) => flush_window_state(&handle),
                    _ => {}
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::list_github_orgs,
            commands::get_org_state,
            commands::set_active_org,
            commands::set_enabled_orgs,
            commands::set_github_pat,
            commands::github_pat_present,
            commands::clear_github_pat,
            commands::list_prs,
            commands::mark_pr_read,
            commands::mark_pr_read_kinds,
            commands::set_pr_muted,
            commands::set_pr_priority,
            commands::set_repo_priority,
            commands::set_author_priority,
            commands::get_audit_log,
            commands::get_activity,
            commands::mark_activity_read,
            commands::set_activity_flag,
            commands::undo_audit,
            commands::get_pr_comments,
            commands::get_pr_commits,
            commands::get_file_at_head,
            commands::refresh_pr,
            commands::get_pr_reviews,
            commands::submit_review,
            commands::resolve_thread,
            commands::merge_pr,
            commands::close_pr,
            commands::reopen_pr,
            commands::add_pr_comment,
            commands::reply_to_thread,
            commands::add_diff_comment,
            commands::toggle_reaction,
            commands::untrack_pr,
            commands::track_pr_url,
            commands::get_analysis,
            commands::run_analysis,
            commands::chat_history,
            commands::chat_send,
            commands::chat_confirm,
            commands::chat_clear,
            commands::get_chat_context,
            commands::get_usage_stats,
            commands::set_model_price,
            commands::get_pr_diff,
            commands::ensure_review_mark,
            commands::set_review_mark,
            commands::get_diff_since,
            commands::aws_sso_login,
            commands::check_aws,
            commands::take_pending_focus,
            commands::open_notification_settings,
            commands::get_viewed_files,
            commands::set_file_viewed,
            commands::log_frontend_error,
            commands::get_dev_logs,
            commands::clear_dev_logs,
            commands::get_default_system_prompt,
            commands::get_app_internals,
            commands::poll_now,
            commands::show_main_window,
            commands::show_main_filtered,
            commands::toggle_callout,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Cmd+Q and other non-tray exits still flush window positions.
            if let tauri::RunEvent::Exit = event {
                flush_window_state(app);
            }
        });
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle-callout", "Toggle Callout", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open-main", "Open CORA", true, None::<&str>)?;
    let poll = MenuItem::with_id(app, "poll-now", "Refresh Now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit CORA", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &poll, &quit])?;

    TrayIconBuilder::with_id("cora-tray")
        .icon(app.default_window_icon().expect("bundled icon").clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle-callout" => {
                let _ = commands::toggle_callout(app.clone());
            }
            "open-main" => {
                let _ = commands::show_main_window(app.clone(), None);
            }
            "poll-now" => {
                app.state::<PollTrigger>().0.notify_waiters();
            }
            "quit" => {
                // Tray quit bypasses window close events — flush positions
                // (callout included) so they restore next launch.
                flush_window_state(app);
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

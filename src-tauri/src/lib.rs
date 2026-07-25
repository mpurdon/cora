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

/// Minimum horizontal strip of the header that must stay on a monitor to remain
/// grabbable (physical px). The callout is frameless, so if its top edge lands
/// off every screen there's no titlebar to drag it back.
const CALLOUT_MIN_HEADER_VISIBLE: i32 = 120;

/// Restored geometry can land the callout off every screen after a monitor
/// change (e.g. unplugging an external display) — and with no decorations there
/// is no way to drag it back. If its header isn't grabbable on any monitor,
/// clamp the window fully onto the screen it most overlaps (else the primary).
/// No-op when the header is already reachable, so a deliberately edge-tucked
/// callout is left alone.
pub(crate) fn ensure_callout_on_screen(window: &tauri::WebviewWindow) {
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let monitors = window.available_monitors().unwrap_or_default();
    if monitors.is_empty() {
        return;
    }
    let (w, h) = (size.width as i32, size.height as i32);

    // Already reachable? Need the top edge on a monitor with room below it, and
    // a wide enough strip of the top exposed to click.
    let header_reachable = monitors.iter().any(|m| {
        let (mp, ms) = (m.position(), m.size());
        let top_on = pos.y >= mp.y && pos.y <= mp.y + ms.height as i32 - 20;
        let x_strip = (pos.x + w).min(mp.x + ms.width as i32) - pos.x.max(mp.x);
        top_on && x_strip >= CALLOUT_MIN_HEADER_VISIBLE
    });
    if header_reachable {
        return;
    }

    // Reposition onto the most-overlapping monitor, falling back to the primary
    // (or the first available) when the window overlaps none.
    let overlap = |m: &tauri::Monitor| -> i64 {
        let (mp, ms) = (m.position(), m.size());
        let ix = ((pos.x + w).min(mp.x + ms.width as i32) - pos.x.max(mp.x)).max(0) as i64;
        let iy = ((pos.y + h).min(mp.y + ms.height as i32) - pos.y.max(mp.y)).max(0) as i64;
        ix * iy
    };
    let target = monitors
        .iter()
        .max_by_key(|m| overlap(m))
        .filter(|m| overlap(m) > 0)
        .cloned()
        .or_else(|| window.primary_monitor().ok().flatten())
        .unwrap_or_else(|| monitors[0].clone());

    let (mp, ms) = (target.position(), target.size());
    let top_inset = 4; // keep the header off the very top edge / menu bar
    let nx = if w >= ms.width as i32 {
        mp.x
    } else {
        pos.x.clamp(mp.x, mp.x + ms.width as i32 - w)
    };
    let ny = if h >= ms.height as i32 {
        mp.y + top_inset
    } else {
        pos.y.clamp(mp.y + top_inset, mp.y + ms.height as i32 - h)
    };
    if nx != pos.x || ny != pos.y {
        let _ = window.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
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
                // The window-state plugin has restored geometry by now; pull the
                // callout back on-screen if that geometry is off every monitor.
                ensure_callout_on_screen(&callout);

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
            commands::update_comment,
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
            commands::reset_callout_position,
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
    let reset = MenuItem::with_id(
        app,
        "reset-callout",
        "Reset Callout Position",
        true,
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, "open-main", "Open CORA", true, None::<&str>)?;
    let poll = MenuItem::with_id(app, "poll-now", "Refresh Now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit CORA", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &reset, &open, &poll, &quit])?;

    TrayIconBuilder::with_id("cora-tray")
        .icon(app.default_window_icon().expect("bundled icon").clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle-callout" => {
                let _ = commands::toggle_callout(app.clone());
            }
            "reset-callout" => {
                let _ = commands::reset_callout_position(app.clone());
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

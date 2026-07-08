mod analysis;
mod commands;
mod devlog;
mod error;
mod github;
mod models;
mod secrets;
mod store;

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tokio::sync::Notify;

use github::poller::PollTrigger;
use store::Store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = Arc::new(Store::open(&data_dir.join("cora.sqlite"))?);
            app.manage(store);
            app.manage(PollTrigger(Arc::new(Notify::new())));
            app.manage(commands::AnalysisRuns(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )));
            app.manage(devlog::DevLog::new());

            // Respect the "open callout at launch" preference.
            let show_callout = app
                .state::<Arc<Store>>()
                .settings()
                .map(|s| s.show_callout_on_startup)
                .unwrap_or(true);
            if !show_callout {
                if let Some(callout) = app.get_webview_window("callout") {
                    let _ = callout.hide();
                }
            }

            setup_tray(app.handle())?;
            github::poller::spawn(app.handle().clone());

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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::set_github_pat,
            commands::github_pat_present,
            commands::clear_github_pat,
            commands::list_prs,
            commands::mark_pr_read,
            commands::set_pr_muted,
            commands::set_pr_priority,
            commands::get_pr_comments,
            commands::get_file_at_head,
            commands::untrack_pr,
            commands::track_pr_url,
            commands::get_analysis,
            commands::run_analysis,
            commands::get_pr_diff,
            commands::aws_sso_login,
            commands::check_aws,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
                app.state::<PollTrigger>().0.notify_one();
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

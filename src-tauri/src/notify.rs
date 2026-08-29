use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// Where a notification click should land. macOS gives us no click callback,
/// so the next app activation within the window claims this instead — right
/// after a notification, that activation *is* the click.
#[derive(Debug, Clone)]
pub struct FocusTarget {
    pub pr_id: String,
    pub comment_id: Option<String>,
}

pub struct PendingFocus(Mutex<Option<(FocusTarget, Instant)>>);

const CLAIM_WINDOW: Duration = Duration::from_secs(120);

impl PendingFocus {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub fn set(&self, target: FocusTarget) {
        *self.0.lock().unwrap() = Some((target, Instant::now()));
    }

    pub fn take(&self) -> Option<FocusTarget> {
        let mut slot = self.0.lock().unwrap();
        match slot.take() {
            Some((target, at)) if at.elapsed() <= CLAIM_WINDOW => Some(target),
            _ => None,
        }
    }
}

/// Fire a native notification and optionally arm the deep link.
pub fn send(app: &AppHandle, title: &str, body: &str, target: Option<FocusTarget>) {
    if let Some(t) = target {
        if let Some(pending) = app.try_state::<PendingFocus>() {
            pending.set(t);
        }
    }
    let result = app.notification().builder().title(title).body(body).show();
    if let Err(e) = result {
        crate::devlog::warn(app, "app", format!("notification failed: {e}"));
    } else {
        crate::devlog::debug(app, "app", format!("notified: {title}"));
    }
}

/// Re-ping for a PR stuck in the persistent-critical state — both its repo
/// and the PR itself marked Critical, still unacknowledged. Reuses the same
/// notification + focus-target path as an ordinary `send`; only the wording
/// marks it as a re-assertion rather than a first alert, so a user who
/// glances at notification history can tell the two apart.
pub fn emit_persistent_alert(app: &AppHandle, pr: &crate::models::TrackedPr, org_prefix: Option<&str>) {
    let short = format!(
        "{}{}#{}",
        org_prefix.map(|o| format!("[{o}] ")).unwrap_or_default(),
        pr.info.repo.split('/').nth(1).unwrap_or(&pr.info.repo),
        pr.info.number
    );
    send(
        app,
        &format!("Still critical: {short}"),
        &pr.info.title,
        Some(FocusTarget { pr_id: pr.info.id.clone(), comment_id: None }),
    );
}

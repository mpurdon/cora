use keyring::Entry;

use crate::error::AppResult;

const SERVICE: &str = "com.mp.cora";
const GITHUB_PAT_USER: &str = "github-pat";
/// The Power Automate trigger URL that posts Teams messages as the user.
/// Its `sig` query parameter is the whole authentication, so it is a
/// credential and lives where the PAT does.
const TEAMS_WEBHOOK_USER: &str = "teams-webhook";

fn github_entry() -> AppResult<Entry> {
    Ok(Entry::new(SERVICE, GITHUB_PAT_USER)?)
}

pub fn set_github_pat(token: &str) -> AppResult<()> {
    github_entry()?.set_password(token)?;
    Ok(())
}

/// The token itself never crosses the IPC boundary — Rust-side callers only.
pub fn github_pat() -> AppResult<Option<String>> {
    match github_entry()?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn github_pat_present() -> AppResult<bool> {
    Ok(github_pat()?.is_some())
}

pub fn clear_github_pat() -> AppResult<()> {
    match github_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn teams_entry() -> AppResult<Entry> {
    Ok(Entry::new(SERVICE, TEAMS_WEBHOOK_USER)?)
}

pub fn set_teams_webhook(url: &str) -> AppResult<()> {
    teams_entry()?.set_password(url)?;
    Ok(())
}

/// Rust-side callers only — the URL is a bearer credential in disguise.
pub fn teams_webhook() -> AppResult<Option<String>> {
    match teams_entry()?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn teams_webhook_present() -> AppResult<bool> {
    Ok(teams_webhook()?.is_some())
}

pub fn clear_teams_webhook() -> AppResult<()> {
    match teams_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

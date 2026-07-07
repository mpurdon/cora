use keyring::Entry;

use crate::error::AppResult;

const SERVICE: &str = "com.mp.cora";
const GITHUB_PAT_USER: &str = "github-pat";

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

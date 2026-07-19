//! Multi-org isolation: one SQLite database per GitHub org under
//! `app_data/orgs/<login>/cora.sqlite`, with everything org-scoped —
//! settings, PRs, activity, analyses — living inside that org's DB. The
//! only cross-org state is this registry's `orgs.json` (which orgs are
//! enabled and which is active). Data never crosses org boundaries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::store::Store;

const LEGACY_DB: &str = "cora.sqlite";
/// Pre-org database, preserved forever: orgs enabled later seed their
/// history (analyses, read state) from it instead of starting cold.
const LEGACY_BACKUP: &str = "legacy-backup.sqlite";
const CONFIG_FILE: &str = "orgs.json";
/// The org the existing installation's data belongs to first.
const DEFAULT_ORG: &str = "team-and-tech";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OrgConfig {
    active: String,
    enabled: Vec<String>,
}

pub struct Orgs {
    data_root: PathBuf,
    active: RwLock<String>,
    enabled: RwLock<Vec<String>>,
    stores: Mutex<HashMap<String, Arc<Store>>>,
}

impl Orgs {
    /// Open the registry, running the one-time legacy split migration if
    /// this is the first launch with org support.
    pub fn open(data_root: &Path) -> AppResult<Self> {
        let config_path = data_root.join(CONFIG_FILE);
        let config: OrgConfig = match std::fs::read_to_string(&config_path) {
            Ok(json) => serde_json::from_str(&json)
                .map_err(|e| AppError::Other(format!("orgs.json unreadable: {e}")))?,
            Err(_) => {
                let config = migrate_legacy(data_root)?;
                write_config(&config_path, &config)?;
                config
            }
        };
        Ok(Self {
            data_root: data_root.to_path_buf(),
            active: RwLock::new(config.active),
            enabled: RwLock::new(config.enabled),
            stores: Mutex::new(HashMap::new()),
        })
    }

    pub fn active_login(&self) -> String {
        self.active.read().unwrap().clone()
    }

    pub fn enabled(&self) -> Vec<String> {
        self.enabled.read().unwrap().clone()
    }

    pub fn is_active(&self, login: &str) -> bool {
        *self.active.read().unwrap() == login
    }

    /// The active org's store — what every UI-facing command operates on.
    pub fn active(&self) -> Arc<Store> {
        let login = self.active_login();
        self.store(&login).expect("active org store must open")
    }

    /// The store for a specific org, opened lazily and cached. Creates the
    /// org's directory and database on first touch, seeding from the legacy
    /// backup when one exists (recovering that org's pre-split history).
    pub fn store(&self, login: &str) -> AppResult<Arc<Store>> {
        if let Some(store) = self.stores.lock().unwrap().get(login) {
            return Ok(store.clone());
        }
        let dir = self.data_root.join("orgs").join(login);
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join("cora.sqlite");
        if !db_path.exists() {
            let backup = self.data_root.join(LEGACY_BACKUP);
            if backup.exists() {
                std::fs::copy(&backup, &db_path)?;
            }
        }
        let store = Arc::new(Store::open(&db_path)?);
        store.prune_to_org(login)?;
        self.stores.lock().unwrap().insert(login.to_string(), store.clone());
        Ok(store)
    }

    pub fn set_active(&self, login: &str) -> AppResult<()> {
        if !self.enabled().iter().any(|o| o == login) {
            return Err(AppError::Other(format!("org not enabled: {login}")));
        }
        *self.active.write().unwrap() = login.to_string();
        self.persist()
    }

    /// Enable/disable orgs. Newly enabled orgs get their DB created (seeded
    /// from the legacy backup when present, else fresh with the active
    /// org's settings as a starting point). The active org can't be
    /// disabled out from under the UI — it falls back to the first entry.
    pub fn set_enabled(&self, logins: Vec<String>) -> AppResult<Vec<String>> {
        if logins.is_empty() {
            return Err(AppError::Other("at least one org must stay enabled".into()));
        }
        let previously = self.enabled();
        let seed_settings = self.active().settings().ok();
        for login in &logins {
            if !previously.iter().any(|o| o == login) {
                let fresh = !self.data_root.join("orgs").join(login).join("cora.sqlite").exists()
                    && !self.data_root.join(LEGACY_BACKUP).exists();
                let store = self.store(login)?;
                if fresh {
                    if let Some(settings) = &seed_settings {
                        let _ = store.save_settings(settings);
                    }
                }
            }
        }
        *self.enabled.write().unwrap() = logins.clone();
        let mut active = self.active.write().unwrap();
        if !logins.iter().any(|o| o == &*active) {
            *active = logins[0].clone();
        }
        drop(active);
        self.persist()?;
        Ok(self.enabled())
    }

    fn persist(&self) -> AppResult<()> {
        let config = OrgConfig { active: self.active_login(), enabled: self.enabled() };
        write_config(&self.data_root.join(CONFIG_FILE), &config)
    }
}

fn write_config(path: &Path, config: &OrgConfig) -> AppResult<()> {
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| AppError::Other(e.to_string()))?;
    std::fs::write(path, json)?;
    Ok(())
}

/// First launch with org support: move the legacy single DB aside as a
/// permanent backup and carve the default org's copy out of it. Other orgs
/// seed from the same backup when enabled later.
fn migrate_legacy(data_root: &Path) -> AppResult<OrgConfig> {
    let legacy = data_root.join(LEGACY_DB);
    let backup = data_root.join(LEGACY_BACKUP);
    if legacy.exists() && !backup.exists() {
        std::fs::rename(&legacy, &backup)?;
    }
    Ok(OrgConfig { active: DEFAULT_ORG.into(), enabled: vec![DEFAULT_ORG.into()] })
}

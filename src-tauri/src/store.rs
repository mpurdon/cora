use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::error::{AppError, AppResult};
use crate::models::{ChangeKind, PrInfo, PrSource, Settings, TrackedPr};

/// SQLite-backed app state. Connection is behind a Mutex; all access is
/// short-lived synchronous work so contention is negligible at our scale.
pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS kv (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS prs (
  id             TEXT PRIMARY KEY,
  data           TEXT NOT NULL,
  sources        TEXT NOT NULL DEFAULT '[]',
  muted          INTEGER NOT NULL DEFAULT 0,
  tracked        INTEGER NOT NULL DEFAULT 1,
  unread         TEXT NOT NULL DEFAULT '[]',
  first_seen     TEXT NOT NULL,
  last_change_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS analyses (
  pr_id      TEXT NOT NULL,
  level      TEXT NOT NULL,
  focus      TEXT NOT NULL DEFAULT '',
  head_sha   TEXT NOT NULL,
  data       TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (pr_id, level, focus)
);
";

impl Store {
    pub fn open(path: &Path) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // -- settings ---------------------------------------------------------

    pub fn settings(&self) -> AppResult<Settings> {
        let conn = self.conn.lock().unwrap();
        let json: Option<String> = conn
            .query_row("SELECT value FROM kv WHERE key = 'settings'", [], |r| r.get(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        let mut settings: Settings = match json {
            Some(j) => serde_json::from_str(&j).unwrap_or_default(),
            None => Settings::default(),
        };
        // Migration: "default" was the pre-Bedrock-config placeholder profile;
        // anyone still on it never configured AWS, so adopt the new defaults.
        if settings.aws_profile == "default" {
            let fresh = Settings::default();
            settings.aws_profile = fresh.aws_profile;
            settings.aws_region = fresh.aws_region;
            settings.bedrock_model_id = fresh.bedrock_model_id;
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> AppResult<()> {
        let json = serde_json::to_string(settings).map_err(|e| AppError::Other(e.to_string()))?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO kv (key, value) VALUES ('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value = ?1",
            params![json],
        )?;
        Ok(())
    }

    // -- PRs ---------------------------------------------------------------

    pub fn list_prs(&self) -> AppResult<Vec<TrackedPr>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT data, sources, muted, unread, first_seen, last_change_at
             FROM prs WHERE tracked = 1 ORDER BY last_change_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let data: String = row.get(0)?;
            let sources: String = row.get(1)?;
            let muted: bool = row.get(2)?;
            let unread: String = row.get(3)?;
            let first_seen: String = row.get(4)?;
            let last_change_at: String = row.get(5)?;
            Ok((data, sources, muted, unread, first_seen, last_change_at))
        })?;
        let mut prs = Vec::new();
        for row in rows {
            let (data, sources, muted, unread, first_seen, last_change_at) = row?;
            let info: PrInfo = match serde_json::from_str(&data) {
                Ok(i) => i,
                Err(_) => continue, // schema drift: skip rather than poison the list
            };
            prs.push(TrackedPr {
                info,
                sources: serde_json::from_str(&sources).unwrap_or_default(),
                muted,
                unread: serde_json::from_str(&unread).unwrap_or_default(),
                first_seen,
                last_change_at,
            });
        }
        Ok(prs)
    }

    pub fn get_pr(&self, id: &str) -> AppResult<Option<TrackedPr>> {
        Ok(self.list_prs()?.into_iter().find(|p| p.info.id == id))
    }

    /// Insert or update an observation. Returns the stored row after merge.
    pub fn upsert_pr(
        &self,
        info: &PrInfo,
        sources: &[PrSource],
        new_changes: &[ChangeKind],
        now: &str,
    ) -> AppResult<TrackedPr> {
        let existing = self.get_pr(&info.id)?;
        let (first_seen, muted, mut unread, mut merged_sources, last_change_at) = match existing {
            Some(p) => (p.first_seen, p.muted, p.unread, p.sources, p.last_change_at),
            None => (now.to_string(), false, Vec::new(), Vec::new(), now.to_string()),
        };
        for s in sources {
            if !merged_sources.contains(s) {
                merged_sources.push(s.clone());
            }
        }
        unread.extend_from_slice(new_changes);
        let last_change_at = if new_changes.is_empty() { last_change_at } else { now.to_string() };

        let data = serde_json::to_string(info).map_err(|e| AppError::Other(e.to_string()))?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO prs (id, data, sources, muted, tracked, unread, first_seen, last_change_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               data = ?2, sources = ?3, unread = ?5, last_change_at = ?7",
            params![
                info.id,
                data,
                serde_json::to_string(&merged_sources).unwrap(),
                muted,
                serde_json::to_string(&unread).unwrap(),
                first_seen,
                last_change_at,
            ],
        )?;
        Ok(TrackedPr {
            info: info.clone(),
            sources: merged_sources,
            muted,
            unread,
            first_seen,
            last_change_at,
        })
    }

    pub fn mark_read(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE prs SET unread = '[]' WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_muted(&self, id: &str, muted: bool) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE prs SET muted = ?2 WHERE id = ?1", params![id, muted])?;
        Ok(())
    }

    pub fn untrack(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE prs SET tracked = 0 WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- analysis cache ------------------------------------------------------

    /// Cached analysis for (pr, level, focus) — only valid for `head_sha`.
    pub fn get_analysis(
        &self,
        pr_id: &str,
        level: &str,
        focus: &str,
        head_sha: &str,
    ) -> AppResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT data FROM analyses
                 WHERE pr_id = ?1 AND level = ?2 AND focus = ?3 AND head_sha = ?4",
                params![pr_id, level, focus, head_sha],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        Ok(row)
    }

    pub fn put_analysis(
        &self,
        pr_id: &str,
        level: &str,
        focus: &str,
        head_sha: &str,
        data: &str,
        created_at: &str,
    ) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO analyses (pr_id, level, focus, head_sha, data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pr_id, level, focus) DO UPDATE SET
               head_sha = ?4, data = ?5, created_at = ?6",
            params![pr_id, level, focus, head_sha, data, created_at],
        )?;
        Ok(())
    }

    /// Drop all cached analyses for a PR (new commits invalidate everything).
    pub fn invalidate_analyses(&self, pr_id: &str) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM analyses WHERE pr_id = ?1", params![pr_id])?;
        Ok(())
    }

    /// Ids of tracked PRs we must keep querying individually even when they
    /// stop matching the open-PR searches (chat/manual adds, just-closed PRs).
    pub fn tracked_ids(&self) -> AppResult<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM prs WHERE tracked = 1")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(id: &str) -> PrInfo {
        PrInfo {
            id: id.into(),
            number: 1,
            title: "t".into(),
            url: "u".into(),
            repo: "o/r".into(),
            author: "a".into(),
            is_draft: false,
            state: "OPEN".into(),
            review_decision: None,
            ci_status: None,
            mergeable: "UNKNOWN".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            head_sha: "abc".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            labels: vec![],
        }
    }

    #[test]
    fn upsert_merges_sources_and_accumulates_unread() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_pr(&pr("PR_1"), &[PrSource::Authored], &[ChangeKind::New], "2026-01-01T00:00:00Z")
            .unwrap();
        let updated = store
            .upsert_pr(
                &pr("PR_1"),
                &[PrSource::ReviewRequested],
                &[ChangeKind::CiChanged],
                "2026-01-02T00:00:00Z",
            )
            .unwrap();
        assert_eq!(updated.sources, vec![PrSource::Authored, PrSource::ReviewRequested]);
        assert_eq!(updated.unread, vec![ChangeKind::New, ChangeKind::CiChanged]);
        assert_eq!(updated.first_seen, "2026-01-01T00:00:00Z");
        assert_eq!(updated.last_change_at, "2026-01-02T00:00:00Z");
    }

    #[test]
    fn mark_read_and_untrack() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_pr(&pr("PR_1"), &[PrSource::Authored], &[ChangeKind::New], "2026-01-01T00:00:00Z")
            .unwrap();
        store.mark_read("PR_1").unwrap();
        assert!(store.get_pr("PR_1").unwrap().unwrap().unread.is_empty());
        store.untrack("PR_1").unwrap();
        assert!(store.list_prs().unwrap().is_empty());
    }
}

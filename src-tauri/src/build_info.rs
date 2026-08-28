use ts_rs::TS;

/// Which build this is. Answers the question a version number can't: the
/// binary in front of you says 0.1.0 whether it came from `tauri dev` five
/// minutes ago or a bundle from July.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// From Cargo.toml — shared with tauri.conf.json's bundle version.
    pub version: String,
    /// "dev" for a debug build (what `npm run tauri dev` produces, serving
    /// the frontend from the Vite dev server), "release" for a bundle.
    pub profile: String,
    /// Short commit the binary was compiled from, or "unknown" outside a
    /// git checkout.
    pub commit: String,
    /// Branch at compile time, or "detached". Meaningful mostly for dev
    /// builds, where it says which line of work is actually running.
    pub branch: String,
    /// Tracked files were modified when this was compiled — so the commit
    /// identifies the base, not the exact source.
    pub dirty: bool,
    /// Compile time, RFC 3339 UTC. The one field that separates two dev
    /// builds of the same dirty tree.
    pub built_at: String,
    /// The whole thing on one line, formatted once here so the UI and any
    /// future log line or bug report can't drift into two conventions:
    /// `0.1.0 · dev · main a1b2c3d*`, asterisk meaning a dirty tree, as in
    /// a shell prompt.
    pub label: String,
}

impl BuildInfo {
    pub fn current() -> Self {
        let secs: i64 = env!("CORA_BUILT_AT").parse().unwrap_or(0);
        let version = env!("CARGO_PKG_VERSION").to_string();
        let profile = if cfg!(debug_assertions) { "dev" } else { "release" }.to_string();
        let commit = env!("CORA_GIT_COMMIT").to_string();
        let branch = env!("CORA_GIT_BRANCH").to_string();
        let dirty = env!("CORA_GIT_DIRTY") == "true";
        let label = format!(
            "{version} · {profile} · {branch} {commit}{}",
            if dirty { "*" } else { "" }
        );
        Self {
            version,
            profile,
            commit,
            branch,
            dirty,
            built_at: chrono::DateTime::from_timestamp(secs, 0)
                .unwrap_or_default()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_says_which_build_this_is() {
        let b = BuildInfo::current();
        assert_eq!(b.version, env!("CARGO_PKG_VERSION"));
        // Tests only ever run under cfg(debug_assertions).
        assert_eq!(b.profile, "dev");
        assert!(!b.commit.is_empty() && !b.branch.is_empty());
        // Built in a checkout, so the commit resolved rather than falling back.
        assert_ne!(b.commit, "unknown");
        assert!(b.built_at.ends_with('Z'), "timestamp should be UTC: {}", b.built_at);
        // The label carries every field the user is meant to read off it.
        assert!(b.label.contains(&b.version));
        assert!(b.label.contains(&b.profile));
        assert!(b.label.contains(&b.branch));
        assert!(b.label.contains(&b.commit));
        assert_eq!(b.label.ends_with('*'), b.dirty, "asterisk must track dirty: {}", b.label);
    }
}

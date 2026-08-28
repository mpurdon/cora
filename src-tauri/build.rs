use std::process::Command;

/// Stamp the binary with where it came from. A version number alone can't
/// answer "is this the build I just made?" — CORA has been 0.1.0 since the
/// scaffold — so the commit, the branch and whether the tree was dirty are
/// what actually identify a build. All of it is optional: a source tarball
/// with no .git still compiles, it just reports "unknown".
fn main() {
    // Rebuild when the checkout moves. HEAD covers commits and branch
    // switches; the packed refs file covers the branch tip advancing. Only
    // declared when present — naming a path that doesn't exist makes cargo
    // rebuild on every invocation.
    for p in ["../.git/HEAD", "../.git/packed-refs"] {
        if std::path::Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }

    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (!s.is_empty()).then_some(s)
    };

    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    // A detached HEAD reports "HEAD" here, which reads as a branch named HEAD.
    let branch = match git(&["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(b) if b != "HEAD" => b,
        _ => "detached".into(),
    };
    // Tracked changes only. Untracked files are usually scratch and would
    // mark almost every dev build dirty, which would make the flag useless.
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let built_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    println!("cargo:rustc-env=CORA_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=CORA_GIT_BRANCH={branch}");
    println!("cargo:rustc-env=CORA_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=CORA_BUILT_AT={built_at}");

    tauri_build::build()
}

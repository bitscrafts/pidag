//! Commit-before-modify safety for the autonomous driver.
//!
//! The driver snapshots the project's working tree *before* dispatching any
//! work that may mutate files, so every change is attributable and rollbackable.
//! If a project is not a git repository, we `git init` it automatically (a
//! private, local-only repo) so all projects get the same safety net.

use std::path::Path;
use std::process::Command;

/// Result of a pre-work snapshot.
#[derive(Debug, Clone)]
pub struct GitState {
    /// The SHA of the snapshot commit (empty if not a repo and init failed).
    pub snapshot_sha: String,
    /// Whether the project was initialized as a new git repo this run.
    pub initialized: bool,
}

/// Run `git` in `cwd`, returning `Ok((exit_success, stdout))` or an error on
/// spawn failure.
fn git(cwd: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    let out_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((out.status.success(), out_text))
}

/// True if `cwd` is inside a git working tree.
pub fn is_git_repo(cwd: &Path) -> bool {
    matches!(
        git(cwd, &["rev-parse", "--is-inside-work-tree"]),
        Ok((true, _))
    )
}

/// Ensure the project is a git repo (init if absent). Returns whether we just
/// initialized it.
pub fn ensure_git_repo(cwd: &Path) -> Result<bool, String> {
    if is_git_repo(cwd) {
        return Ok(false);
    }
    let (ok, _) = git(cwd, &["init", "-q"])?;
    if !ok {
        return Err("git init failed".into());
    }
    // Configure a local identity so commits work without a global user.
    let _ = git(cwd, &["config", "user.email", "pidag-auto@local"]);
    let _ = git(cwd, &["config", "user.name", "pidag auto"]);
    Ok(true)
}

/// Deterministically set a git user identity for a repo so `git commit` works
/// even when no global identity is configured.
fn ensure_identity(cwd: &Path) {
    let _ = git(cwd, &["config", "user.email", "pidag-auto@local"]);
    let _ = git(cwd, &["config", "user.name", "pidag auto"]);
}

/// Snapshot the working tree with `git add -A && git commit`, capturing the
/// resulting commit SHA. Commits made just before we modify anything so a
/// mistake is easily reverted.
pub fn snapshot(cwd: &Path, message: &str) -> Result<String, String> {
    // No repo -> init first so we always get a snapshot (drop-in safety).
    ensure_git_repo(cwd)?;
    ensure_identity(cwd);

    let (_, _) = git(cwd, &["add", "-A"])?;

    // If there is nothing to commit, use the current HEAD SHA (idempotent).
    if let Ok((true, sha)) = git(cwd, &["rev-parse", "HEAD"]) {
        // Check whether there are staged changes before committing.
        let staged = git(cwd, &["diff", "--cached", "--quiet"]);
        if let Ok((true, _)) = staged {
            // Nothing staged -> nothing to snapshot; return current HEAD.
            return Ok(sha);
        }
    }

    let commit_msg = format!(
        "pidag-auto: {} @ {}",
        message,
        chrono::Utc::now().to_rfc3339()
    );
    match git(cwd, &["commit", "-q", "-m", &commit_msg]) {
        Ok((true, _)) => {
            let (_ok, sha) = git(cwd, &["rev-parse", "HEAD"])?;
            Ok(sha)
        }
        Ok((false, _)) => {
            // Commit failed (e.g. no changes) — fall back to current HEAD.
            Ok(git(cwd, &["rev-parse", "HEAD"])
                .map(|(_, s)| s)
                .unwrap_or_default())
        }
        Err(e) => Err(e),
    }
}

/// Capture a full pre-work snapshot, returning the SHA and whether we init'd.
pub fn pre_work_snapshot(cwd: &Path, label: &str) -> Result<GitState, String> {
    let initialized = ensure_git_repo(cwd)?;
    let sha = snapshot(cwd, &format!("pre-work {label}"))?;
    Ok(GitState {
        snapshot_sha: sha,
        initialized,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_git_repo_false_for_new_dir() {
        let dir = std::env::temp_dir().join(format!("pidag-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_git_repo(&dir), "a fresh dir is not a git repo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_init_and_commit() {
        let dir = std::env::temp_dir().join(format!("pidag-git-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();

        let init = ensure_git_repo(&dir).unwrap();
        assert!(init, "init should succeed on a fresh dir");
        let _ = pre_work_snapshot(&dir, "test").unwrap();

        assert!(is_git_repo(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

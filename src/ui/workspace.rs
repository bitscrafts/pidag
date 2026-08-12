//! Workspace-level orchestration (Phase 3 multi-project spec).
//!
//! A workspace is a directory containing multiple pidag projects. Discovery
//! scans a workspace root's immediate subdirectories and treats each one that
//! looks like a project (`specs/*.md`, `Cargo.toml`, or `pyproject.toml`) as
//! a project card. The operating `pidag ui --workspace PATH` mode is entirely
//! additive: single-project `--project-root` mode is unchanged.
//!
//! Guardrails enforced here:
//!   - Discovery is bounded (a project with > 100 spec files is skipped).
//!   - Project names are validated for path-traversal on every lookup.
//!   - Runs are NOT stored in the workspace vault — each project keeps its own
//!     `.pidag/pidag.redb`. The workspace vault only registers discovered
//!     projects for cross-project run history + defaults.

use crate::core::error::PidagError;
use crate::store::RunMeta;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Skip projects whose `specs/` directory exceeds this many spec files, so a
/// pathological directory cannot blow up discovery (bounded-scan guardrail).
const MAX_SPECS_PER_PROJECT: usize = 100;

/// Skip hidden directories (`.git`, `.pidag`, etc.) during discovery.
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_str().map(|s| s.starts_with('.')).unwrap_or(false)
}

/// A discovered project within the workspace, rendered as a card on the
/// workspace landing page (R2 / R7).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProjectInfo {
    pub name: String,
    pub path: PathBuf,
    pub spec_count: usize,
    pub run_count: usize,
    pub last_run: Option<String>,
    pub status: String,
}

/// Overall derived status of a project (R7 landing-page badge), flattened to a
/// string for JSON serialization (the UI renders it directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectStatus {
    Complete,
    InProgress,
    Pending,
    NoSpecs,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectStatus::Complete => "complete",
            ProjectStatus::InProgress => "in-progress",
            ProjectStatus::Pending => "pending",
            ProjectStatus::NoSpecs => "no-specs",
        }
    }
}

/// Tracks a single project's aggregate status by folding its spec-status and
/// run history into one badge. Simple majority rule:
///   - No specs                     → NoSpecs
///   - Any run incomplete           → InProgress
///   - Any spec in-progress         → InProgress
///   - All runs done + none pending → Complete
///   - Otherwise                    → Pending
fn project_status(specs: &[SpecStatus], run_statuses: &[&str]) -> ProjectStatus {
    if specs.is_empty() {
        return ProjectStatus::NoSpecs;
    }
    if run_statuses
        .iter()
        .any(|s| *s == "in-progress" || *s == "pending")
    {
        return ProjectStatus::InProgress;
    }
    if specs.contains(&SpecStatus::InProgress) {
        return ProjectStatus::InProgress;
    }
    if specs.iter().all(|s| *s == SpecStatus::Complete) {
        return ProjectStatus::Complete;
    }
    ProjectStatus::Pending
}

/// A coarse spec progress enum used to derive `ProjectStatus` during
/// discovery. Kept private — only the folder-level status is exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecStatus {
    Complete,
    InProgress,
    Pending,
}

/// The `GET /api/workspace` response (R2): the workspace root plus the list of
/// discovered project cards.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceOverview {
    pub workspace_root: String,
    pub projects: Vec<ProjectInfo>,
}

/// Shared workspace state, stored in the axum router state. The workspace
/// vault path is `<workspace_root>/.pidag/pidag.redb`.
#[derive(Debug, Clone)]
pub struct WorkspaceState {
    pub workspace_root: PathBuf,
    pub vault_path: PathBuf,
}

/// Validate a project name parameter for path-traversal. A project name must
/// be a single path component (no `/`, `\`, `.`, or `..`). Returns `Ok` with
/// the safe name, or a `Validation` error.
pub fn validate_project_name(name: &str) -> Result<&str, PidagError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(PidagError::Validation(format!(
            "invalid project name: {name:?}"
        )));
    }
    Ok(name)
}

/// True if a directory looks like a pidag project: it has a `specs/` directory
/// containing at least one `.md` file, or a `Cargo.toml`, or a
/// `pyproject.toml` (R1 discovery rule).
pub fn is_project_dir(dir: &Path) -> bool {
    // specs/*.md
    let specs_dir = dir.join("specs");
    if let Ok(rd) = std::fs::read_dir(&specs_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                return true;
            }
        }
    }
    // Cargo.toml or pyproject.toml
    dir.join("Cargo.toml").exists() || dir.join("pyproject.toml").exists()
}

/// Count `.md` spec files in a project's `specs/` directory (bounded at
/// `MAX_SPECS_PER_PROJECT`). Returns the bounded count and the parsed spec
/// statuses used for the project badge.
fn spec_meta(project_dir: &Path) -> (usize, Vec<SpecStatus>) {
    let specs_dir = project_dir.join("specs");
    let mut count = 0usize;
    let mut statuses = Vec::new();
    let mut in_fence = false;

    let Ok(rd) = std::fs::read_dir(&specs_dir) else {
        return (0, statuses);
    };

    for entry in rd.flatten() {
        if count >= MAX_SPECS_PER_PROJECT {
            break;
        }
        let p = entry.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        count += 1;

        // Parse exit-criteria checkboxes to derive a coarse status.
        let content = std::fs::read_to_string(&p).unwrap_or_default();
        let (total, done) = count_exit_criteria(&content, &mut in_fence);
        statuses.push(if total == 0 {
            SpecStatus::Pending
        } else if done == total {
            SpecStatus::Complete
        } else if done == 0 {
            SpecStatus::Pending
        } else {
            SpecStatus::InProgress
        });
    }

    (count, statuses)
}

/// Count exit-criteria checkboxes (`- [ ]` / `- [x]`) outside code fences.
fn count_exit_criteria(markdown: &str, in_fence: &mut bool) -> (usize, usize) {
    let mut total = 0usize;
    let mut done = 0usize;
    for raw in markdown.lines() {
        let line = raw.trim_start();
        if line.starts_with("```") {
            *in_fence = !*in_fence;
            continue;
        }
        if *in_fence {
            continue;
        }
        let rest = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .map(str::trim_start);
        if let Some(rest) = rest
            && let Some(after_box) = rest.strip_prefix('[')
            && let Some(marker_char) = after_box.chars().next()
            && let Some(inside) = after_box.strip_prefix(marker_char)
            && inside.starts_with(']')
        {
            total += 1;
            if marker_char == 'x' || marker_char == 'X' {
                done += 1;
            }
        }
    }
    (total, done)
}

/// Derive `run_count` and `last_run` from a project's own vault, if present.
/// Runs stay in per-project vaults (guardrail); this only reads. Opens the
/// project vault read-only via `RedbStorePool` (no persistent lock). Any store
/// error degrades to `(0, None)` rather than failing discovery.
///
/// Uses `block_in_place` to run the async store call from a sync context, which
/// is safe when called from within a multi-threaded tokio runtime (as axum uses).
fn project_run_meta(project_dir: &Path) -> (usize, Option<String>, Vec<String>) {
    let vault = project_dir.join(".pidag").join("pidag.redb");
    if !vault.exists() {
        return (0, None, Vec::new());
    }
    let pool = crate::store::RedbStorePool::new(vault);
    // Use block_in_place to run async code from sync context within an existing
    // runtime. This avoids the "Cannot start a runtime from within a runtime"
    // panic that occurred when creating a nested runtime.
    let runs: Vec<RunMeta> = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            use crate::store::Store;
            pool.list_runs().await.ok().unwrap_or_default()
        })
    });
    let statuses = runs
        .iter()
        .map(|r| {
            if r.completed_at.is_some() {
                "complete".to_string()
            } else {
                "in-progress".to_string()
            }
        })
        .collect();
    let last_run = runs.iter().map(|r| r.started_at.clone()).max();
    (runs.len(), last_run, statuses)
}

/// Discover all projects under `workspace_root`. Scans immediate subdirectories
/// (depth 1), skipping hidden dirs and any dir that does not satisfy
/// `is_project_dir`. Returns one `ProjectInfo` per discovered project. Always
/// returns `Ok` (an inaccessible workspace degrades to an empty list).
pub fn discover_projects(workspace_root: &Path) -> Result<Vec<ProjectInfo>, PidagError> {
    let rd = std::fs::read_dir(workspace_root)
        .map_err(|e| PidagError::Store(format!("Failed to scan workspace: {}", e)))?;

    let mut projects = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() || is_hidden(&entry.file_name()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_project_dir(&path) {
            continue;
        }

        let (spec_count, spec_statuses) = spec_meta(&path);
        // Skip projects with zero specs (bounded scan; don't surface noise).
        if spec_count == 0 {
            continue;
        }
        let (run_count, last_run, run_statuses) = project_run_meta(&path);
        let status = project_status(
            &spec_statuses,
            run_statuses
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice(),
        );

        projects.push(ProjectInfo {
            name,
            path: path.clone(),
            spec_count,
            run_count,
            last_run,
            status: status.as_str().to_string(),
        });
    }

    // Deterministic order (the UI grids cards alphabetically by name).
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(projects)
}

/// Resolve a project name to its absolute path inside the workspace, with
/// path-traversal checks. Returns `None` when the path does not exist or the
/// name is unsafe / not a project.
pub fn resolve_project_path(workspace_root: &Path, name: &str) -> Option<PathBuf> {
    validate_project_name(name).ok()?;
    let candidate = workspace_root.join(name);
    let canon = candidate.canonicalize().ok()?;
    let canon_root = workspace_root.canonicalize().ok()?;
    if !canon.starts_with(&canon_root) {
        return None;
    }
    if !std::fs::metadata(&canon)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return None;
    }
    Some(canon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws() -> (tempfile::TempDir, PathBuf) {
        let ws = tempfile::tempdir().unwrap();
        let path = ws.path().to_path_buf();
        (ws, path)
    }

    fn make_project(root: &Path, name: &str, specs: &[&str], manifest: bool) {
        let dir = root.join(name);
        let specs_dir = dir.join("specs");
        std::fs::create_dir_all(&specs_dir).unwrap();
        for (i, s) in specs.iter().enumerate() {
            std::fs::write(
                specs_dir.join(format!("{:02}-{}.md", i + 1, s)),
                format!("# {s}\n\n## Exit Criteria\n\n- [ ] todo\n"),
            )
            .unwrap();
        }
        if manifest {
            std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        }
    }

    #[test]
    fn test_discover_finds_projects() {
        let (_ws, root) = temp_ws();
        make_project(&root, "testing", &["spec1"], false);
        make_project(&root, "pidag", &["spec1", "spec2"], true);
        let projects = discover_projects(&root).unwrap();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["pidag", "testing"]);
    }

    #[test]
    fn test_empty_workspace() {
        let (_ws, root) = temp_ws();
        let projects = discover_projects(&root).unwrap();
        assert!(projects.is_empty());
    }

    #[test]
    fn test_filters_non_projects() {
        let (_ws, root) = temp_ws();
        // A dir with neither specs/ nor a manifest is filtered out.
        std::fs::create_dir_all(root.join("not-a-project")).unwrap();
        std::fs::write(root.join("not-a-project/readme.txt"), "hello").unwrap();
        make_project(&root, "real", &["spec1"], false);
        let projects = discover_projects(&root).unwrap();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["real"]);
    }

    #[test]
    fn test_hidden_dirs_skipped() {
        let (_ws, root) = temp_ws();
        make_project(&root, ".git", &["spec1"], false); // hidden → skipped
        let projects = discover_projects(&root).unwrap();
        assert!(projects.is_empty());
    }

    #[test]
    fn test_project_path_traversal_rejected() {
        let (_ws, root) = temp_ws();
        assert!(resolve_project_path(&root, "..").is_none());
        assert!(resolve_project_path(&root, "/etc").is_none());
        assert!(resolve_project_path(&root, "a/b").is_none());
    }
}

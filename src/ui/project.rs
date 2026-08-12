//! Project Overview handlers for the trace UI.
//!
//! These handlers serve the single-project Project Overview view: spec
//! enumeration, Exit Criteria progress, and the full spec content endpoint.

use super::UiState;
use super::handlers::run_summary;
use super::spec_parser::{is_safe_spec_name, parse_spec, spec_status};
use super::types::{AppError, ProjectOverview, RunSummary, SpecDetail, SpecSummary};
use crate::store::Store;
use axum::extract::{Path, State};
use axum::response::Json;
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;

/// Maximum spec file size we'll read + parse. Guards against pathological
/// inputs (a 1 GiB spec would block the handler). 1 MiB is comfortably larger
/// than any real spec in this workspace.
const MAX_SPEC_FILE_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// Project Overview
// ---------------------------------------------------------------------------

/// `GET /api/project` -- the Project Overview dashboard. Enumerates specs in
/// `project_root/specs/*.md` (sorted by filename) and lists all runs in the
/// vault. When `project_root` is `None`, returns `specs: []` and
/// `project_root: null` so the UI can show a "no project root configured"
/// hint. Per R10, this is a read-only projection -- no schema changes.
pub async fn get_project(
    State(state): State<Arc<UiState>>,
) -> Result<Json<ProjectOverview>, AppError> {
    // R10: no project root is not an error -- return an empty overview so the
    // UI can show a "no project root configured" hint (specs: [], null root).
    let overview = match state.project_root.as_ref() {
        Some(root) => build_project_overview(&root.clone(), &state.store).await,
        None => ProjectOverview {
            project_root: None,
            project_name: None,
            specs: Vec::new(),
            runs: listable_runs(&state.store).await,
        },
    };
    Ok(Json(overview))
}

/// Build a `ProjectOverview` for a single project root: enumerate its
/// `specs/*.md` (sorted, size-guarded, exit-criteria parsed) and list all
/// runs from the given store. Used by both single-project mode (`GET
/// /api/project`, store = the UI's own vault) and workspace project-attach
/// (`GET /api/workspace/projects/:name`, store = a per-project vault).
pub async fn build_project_overview(
    root: &std::path::Path,
    store: &Arc<dyn Store>,
) -> ProjectOverview {
    let runs = listable_runs(store).await; // Vec<RunSummary>

    let specs_dir = root.join("specs");
    let specs: Vec<PathBuf> = match std::fs::read_dir(&specs_dir) {
        Ok(rd) => {
            let mut entries: Vec<_> = rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("md") {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect();
            entries.sort();
            entries
        }
        Err(_) => Vec::new(),
    };
    let mut summaries = Vec::with_capacity(specs.len());
    for path in &specs {
        // Skip files larger than the cap (R-guardrail).
        if let Ok(meta) = std::fs::metadata(path)
            && meta.len() > MAX_SPEC_FILE_BYTES
        {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let parsed = parse_spec(&content);
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let title = if parsed.title.is_empty() {
            name.clone()
        } else {
            parsed.title.clone()
        };
        let status = spec_status(parsed.exit_criteria_total, parsed.exit_criteria_done);
        summaries.push(SpecSummary {
            name,
            title,
            file: path
                .strip_prefix(root)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string(),
            exit_criteria_total: parsed.exit_criteria_total,
            exit_criteria_done: parsed.exit_criteria_done,
            status: status.to_string(),
        });
    }

    ProjectOverview {
        project_root: Some(root.display().to_string()),
        project_name: root
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string()),
        specs: summaries,
        runs,
    }
}

/// Load all runs from a store and convert to `RunSummary`. Degrades to an
/// empty list on store error (a broken vault should not take down the whole
/// project overview). Async so it can `.await` the store directly from an
/// axum handler without out-of-runtime blocking.
pub async fn listable_runs(store: &Arc<dyn Store>) -> Vec<RunSummary> {
    let runs = store.list_runs().await.unwrap_or_default();
    runs.into_iter().map(run_summary).collect()
}

// ---------------------------------------------------------------------------
// Spec Detail
// ---------------------------------------------------------------------------

/// `GET /api/project/specs/:name` -- fetch a single spec's full content +
/// parsed metadata. `name` is the file stem (e.g. `shell-node-dispatch`).
/// Path traversal is rejected (R-guardrail): `name` must match
/// `^[A-Za-z0-9._-]+$` and the resolved path must stay inside
/// `project_root/specs/`.
pub async fn get_spec(
    State(state): State<Arc<UiState>>,
    Path(name): Path<String>,
) -> Result<Json<SpecDetail>, AppError> {
    let root = state.project_root.as_ref().ok_or(AppError::NotFound)?; // no project root -> 404
    Ok(Json(spec_detail(root, &name)?))
}

/// Build a `SpecDetail` for a spec in a project: parse its markdown content
/// and metadata with the path-traversal and size guardrails.
///
/// Shared between the single-project handler (`get_spec`, root = `project_root`)
/// and the workspace project-spec endpoint (root = a per-project directory).
pub fn spec_detail(root: &FilePath, name: &str) -> Result<SpecDetail, AppError> {
    if !is_safe_spec_name(name) {
        return Err(AppError::NotFound);
    }

    let specs_dir = root.join("specs");
    let spec_path = specs_dir.join(format!("{}.md", name));

    // Defense-in-depth: canonicalize and confirm the path is inside specs_dir.
    // If the file doesn't exist, canonicalize fails -> 404.
    let canon_spec = match spec_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return Err(AppError::NotFound),
    };
    let canon_dir = match specs_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return Err(AppError::NotFound),
    };
    if !canon_spec.starts_with(&canon_dir) {
        return Err(AppError::NotFound);
    }

    // Size guard.
    if let Ok(meta) = std::fs::metadata(&canon_spec)
        && meta.len() > MAX_SPEC_FILE_BYTES
    {
        return Err(AppError::NotFound);
    }

    let content = std::fs::read_to_string(&canon_spec).map_err(|_| AppError::NotFound)?;
    let parsed = parse_spec(&content);
    let title = if parsed.title.is_empty() {
        name.to_string()
    } else {
        parsed.title.clone()
    };

    Ok(SpecDetail {
        name: name.to_string(),
        title,
        file: canon_spec
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_string(),
        content,
        exit_criteria_total: parsed.exit_criteria_total,
        exit_criteria_done: parsed.exit_criteria_done,
    })
}

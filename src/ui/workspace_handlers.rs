//! Workspace handlers for the trace UI (Phase 3 multi-project spec).
//!
//! These handlers serve the multi-project workspace views: project discovery,
//! project-attach, and per-project spec/run access.

use super::handlers::build_run_detail;
use super::project::{build_project_overview, spec_detail};
use super::types::{AppError, ProjectOverview, RunDetailResponse, SpecDetail};
use super::{UiState, WORKSPACE_CACHE_TTL, WorkspaceCacheEntry};
use crate::core::error::PidagError;
use crate::store::{RedbStorePool, Store};
use crate::ui::workspace::{WorkspaceOverview, discover_projects, resolve_project_path};
use axum::extract::{Path, State};
use axum::response::Json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Workspace Landing
// ---------------------------------------------------------------------------

/// `GET /api/workspace` -- the workspace landing page (R2). Discovers the
/// immediate subdirectories under `workspace_root` that look like pidag
/// projects (`specs/*.md`, `Cargo.toml`, or `pyproject.toml`), and returns a
/// `WorkspaceOverview` with one `ProjectInfo` card per project. When the UI
/// is running in single-project mode (`workspace_root` is `None` -- no
/// `--workspace` flag), this endpoint 404s.
///
/// NF1 (lazy discovery): the scan runs only on the first request and the
/// result is cached in `state.workspace_cache` for `WORKSPACE_CACHE_TTL`.
/// Subsequent requests within that window reuse the cached cards without a
/// filesystem scan.
pub async fn get_workspace(
    State(state): State<Arc<UiState>>,
) -> Result<Json<WorkspaceOverview>, AppError> {
    let ws_root = state.workspace_root.as_ref().ok_or(AppError::NotFound)?; // single-project mode -> 404

    // NF1 lazy scan with a 60s TTL cache. Re-fetch only when the previous
    // scan has expired. The lock is held only to read/clone the cached list.
    {
        let cache = state
            .workspace_cache
            .lock()
            .map_err(|_| AppError::Store(PidagError::Store("cache poisoned".into())))?;
        if let Some(entry) = cache.as_ref()
            && entry.scanned_at.elapsed() < WORKSPACE_CACHE_TTL
        {
            return Ok(Json(WorkspaceOverview {
                workspace_root: ws_root.display().to_string(),
                projects: entry.projects.clone(),
            }));
        }
    }

    // Cache miss or expired: scan and repopulate.
    let projects = discover_projects(ws_root).map_err(AppError::Store)?;
    if let Ok(mut cache) = state.workspace_cache.lock() {
        *cache = Some(WorkspaceCacheEntry {
            scanned_at: Instant::now(),
            projects: projects.clone(),
        });
    }
    Ok(Json(WorkspaceOverview {
        workspace_root: ws_root.display().to_string(),
        projects,
    }))
}

// ---------------------------------------------------------------------------
// Project Resolution
// ---------------------------------------------------------------------------

/// Resolve a project name inside the workspace to its project path AND its
/// own vault store. Shared by the project-attach, project-spec, and
/// project-run workspace endpoints so they all apply the same path-traversal
/// checks and open the same per-project pool. Returns the project path.
fn open_workspace_project(
    state: &UiState,
    name: &str,
) -> Result<(PathBuf, Arc<dyn Store>), AppError> {
    let ws_root = state.workspace_root.as_ref().ok_or(AppError::NotFound)?; // single-project mode -> 404
    let project_path = resolve_project_path(ws_root, name).ok_or(AppError::NotFound)?;
    let vault = project_path.join(".pidag").join("pidag.redb");
    let store: Arc<dyn Store> = Arc::new(RedbStorePool::new(vault));
    Ok((project_path, store))
}

// ---------------------------------------------------------------------------
// Project Attach
// ---------------------------------------------------------------------------

/// `GET /api/workspace/projects/{name}` -- attach to a single project (R3).
/// Resolves the project name inside the workspace (path-traversal check via
/// `resolve_project_path`), opens the project's OWN vault
/// (`<project>/.pidag/pidag.redb`) so run data stays in per-project vaults
/// (guardrail), and returns the same `ProjectOverview` shape as the
/// single-project `GET /api/project`. Unknown or unsafe names -> 404.
pub async fn get_workspace_project(
    State(state): State<Arc<UiState>>,
    Path(name): Path<String>,
) -> Result<Json<ProjectOverview>, AppError> {
    let (project_path, store) = open_workspace_project(&state, &name)?;
    let overview = build_project_overview(&project_path, &store).await;
    Ok(Json(overview))
}

// ---------------------------------------------------------------------------
// Project Spec
// ---------------------------------------------------------------------------

/// `GET /api/workspace/projects/{name}/specs/{spec}` -- fetch a single spec
/// from a workspace project. Scope-equivalent of the single-project
/// `GET /api/project/specs/:name` but read from the named project's own
/// directory. Unknown project, unsafe spec name, or missing file -> 404.
pub async fn get_workspace_project_spec(
    State(state): State<Arc<UiState>>,
    Path((name, spec)): Path<(String, String)>,
) -> Result<Json<SpecDetail>, AppError> {
    let (project_path, _store) = open_workspace_project(&state, &name)?;
    Ok(Json(spec_detail(&project_path, &spec)?))
}

// ---------------------------------------------------------------------------
// Project Run
// ---------------------------------------------------------------------------

/// `GET /api/workspace/projects/{name}/runs/{run_id}` -- fetch a single run
/// (metadata + nodes) from a workspace project's own vault. Scope-equivalent
/// of the single-project `GET /api/runs/:id` but read from the named
/// project's vault. The run_id is validated against the store (a run that
/// belongs to a different project's vault will 404 here).
pub async fn get_workspace_project_run(
    State(state): State<Arc<UiState>>,
    Path((name, run_id)): Path<(String, String)>,
) -> Result<Json<RunDetailResponse>, AppError> {
    let (_project_path, store) = open_workspace_project(&state, &name)?;
    Ok(Json(build_run_detail(&store, &run_id).await?))
}

//! Trace UI web server for `pidag` DAG runs.
//!
//! Implements the `pidag ui` subcommand (P1 #9 of the SSSF patterns adoption
//! spec): a self-contained axum web server that visualizes DAG runs from the
//! redb vault. The frontend is a vanilla-JS single-page app embedded in the
//! binary via `include_str!` -- no Node/Bun toolchain, no external assets.
//!
//! # Architecture
//!
//! ```text
//! pidag ui --port 4600 --vault .pidag/pidag.redb
//!     |
//!     +-- axum web server (this module)
//!             +-- GET /              -> embedded index.html (SPA shell)
//!             +-- GET /api/health    -> {"status":"ok","runs":N}
//!             +-- GET /api/runs      -> Vec<RunMeta> as JSON
//!             +-- GET /api/runs/:id  -> {run, nodes} as JSON
//!             +-- GET /api/runs/:id/events?since=N -> Vec<{seq, event}>
//!             +-- GET /api/runs/:id/status -> {"text": "<render_status output>"}
//!             +-- GET /api/project   -> ProjectOverview
//!             +-- GET /api/workspace -> WorkspaceOverview
//!
//!     +-- Arc<dyn Store> (RedbStore in production, MockStore in tests)
//! ```
//!
//! # Safety
//!
//! The server binds to `127.0.0.1` by default (local-only) because the UI has
//! no authentication. Use `--host 0.0.0.0` to expose it on all interfaces only
//! when you understand the implications.
//!
//! See `specs/96-sssf-patterns-adoption.md` P1 section for the full spec.

mod handlers;
mod project;
pub mod render;
mod spec_parser;
mod types;
pub mod workspace;
mod workspace_handlers;

use crate::store::Store;
use crate::ui::workspace::ProjectInfo;
use axum::Router;
use axum::routing::get;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// Re-export types for external use
pub use types::{
    AppError, ArtifactResponse, EventsQuery, HealthResponse, ProjectOverview, RunDetailResponse,
    RunSummary, SeqEvent, SpecDetail, SpecSummary, StatusResponse, TimelineGroup, TimelineItem,
    TimelineResponse,
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared state for the UI server: a thread-safe handle to the vault.
/// `RedbStore` in production, `MockStore` in tests -- both implement `Store`.
pub struct UiState {
    /// The vault store backing the UI. All reads go through this; redb
    /// supports concurrent readers (reads never block the running writer),
    /// so polling is safe during an active DAG run.
    pub store: Arc<dyn Store>,
    /// Optional project root for the Project Overview view. When `Some`,
    /// the `GET /api/project` endpoint enumerates `specs/*.md` under this
    /// path and parses each spec's Exit Criteria. When `None`, the Project
    /// view reports no specs (the UI falls back to the sessions list).
    /// Set via `pidag ui --project-root PATH`.
    pub project_root: Option<PathBuf>,
    /// Optional workspace root for the multi-project Workspace view. When
    /// `Some`, the `GET /api/workspace` and
    /// `GET /api/workspace/projects/:name` endpoints (Phase 3 multi-project
    /// spec) are enabled. Set via `pidag ui --workspace PATH`. When `None`,
    /// workspace endpoints 404 (single-project mode / existing tests).
    pub workspace_root: Option<PathBuf>,
    /// Path to the workspace vault (`<workspace_root>/.pidag/pidag.redb`).
    /// Present when `workspace_root` is `Some`. Runs are never stored here
    /// (guardrail) -- it only registers discovered projects + defaults.
    pub workspace_vault_path: Option<PathBuf>,
    /// NF1 lazy-discovery cache for the workspace landing page. Projects are
    /// scanned on first `/api/workspace` request and the result cached for
    /// `WORKSPACE_CACHE_TTL` (60s). Mutating the cache requires non-`Send`
    /// short-lived lock which is fine here (the clone is cheap); we use a
    /// `std::sync::Mutex` and avoid holding it across `.await`.
    pub workspace_cache: Mutex<Option<WorkspaceCacheEntry>>,
}

/// Cache entry for NF1 lazy workspace discovery: an instant of first
/// population plus the discovered project cards.
pub struct WorkspaceCacheEntry {
    pub scanned_at: Instant,
    pub projects: Vec<ProjectInfo>,
}

/// How long a lazy discovery cache lives before it is re-scanned (NF1: 60s).
const WORKSPACE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

impl UiState {
    pub fn new(store: Arc<dyn Store>) -> Self {
        UiState {
            store,
            project_root: None,
            workspace_root: None,
            workspace_vault_path: None,
            workspace_cache: Mutex::new(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Start the trace UI web server. Blocks until the server is shut down.
///
/// # Errors
///
/// Returns `std::io::Error` if the TCP listener cannot bind or if the server
/// encounters a fatal I/O error. Store errors are handled per-request via
/// `AppError` and never propagate to this level.
pub async fn serve(
    store: Arc<dyn Store>,
    project_root: Option<PathBuf>,
    host: &str,
    port: u16,
) -> Result<(), std::io::Error> {
    let app = router(store, project_root);
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port)));
    eprintln!("pidag ui: serving on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

/// Start the trace UI in multi-project workspace mode (Phase 3). Equivalent
/// to [`serve`] but enables the `GET /api/workspace` and
/// `GET /api/workspace/projects/:name` endpoints via `router_for_workspace`.
/// The workspace vault path is `<workspace_root>/.pidag/pidag.redb`.
pub async fn serve_workspace(
    store: Arc<dyn Store>,
    workspace_root: PathBuf,
    workspace_vault_path: PathBuf,
    host: &str,
    port: u16,
) -> Result<(), std::io::Error> {
    let app = router_for_workspace(store, workspace_root, workspace_vault_path);
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port)));
    eprintln!("pidag ui: serving workspace on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

// ---------------------------------------------------------------------------
// Router builders
// ---------------------------------------------------------------------------

/// Build the axum `Router` for the single-project UI server. Exposed as `pub`
/// so the binary (`bin/pidag.rs`) and integration tests (`tests/ui_tests.rs`)
/// share the same route table. `workspace_root`/`workspace_vault_path` are
/// `None`, so workspace endpoints are disabled (single-project mode).
pub fn router(store: Arc<dyn Store>, project_root: Option<PathBuf>) -> Router {
    let state = Arc::new(UiState {
        store,
        project_root,
        workspace_root: None,
        workspace_vault_path: None,
        workspace_cache: Mutex::new(None),
    });
    build_router().with_state(state)
}

/// Build the router with a `Arc<UiState>` state type. Also exposed as
/// `router_for_test` for the test suite, which needs the un-bound router to
/// drive requests via `tower::ServiceExt::oneshot`. Uses `project_root:
/// None` so existing tests (which have no project root) keep working
/// unchanged.
pub fn router_for_test(store: Arc<dyn Store>) -> Router {
    let state = Arc::new(UiState {
        store,
        project_root: None,
        workspace_root: None,
        workspace_vault_path: None,
        workspace_cache: Mutex::new(None),
    });
    build_router().with_state(state)
}

/// Build the router with an explicit project root for tests that exercise
/// the Project Overview endpoints (`GET /api/project`, `GET /api/project/
/// specs/:name`). The `project_root` must point at a directory containing a
/// `specs/` subdirectory.
pub fn router_for_test_with_root(store: Arc<dyn Store>, project_root: PathBuf) -> Router {
    let state = Arc::new(UiState {
        store,
        project_root: Some(project_root),
        workspace_root: None,
        workspace_vault_path: None,
        workspace_cache: Mutex::new(None),
    });
    build_router().with_state(state)
}

/// Build a router in workspace mode (Phase 3 multi-project spec). The state
/// carries a `workspace_root` (the directory scanned for project cards) and
/// the path to the workspace vault. Project-vault reads for the attach
/// endpoint go through per-project `RedbStorePool`s opened on demand -- the
/// workspace vault is never used to store run data (guardrail).
pub fn router_for_workspace(
    store: Arc<dyn Store>,
    workspace_root: PathBuf,
    workspace_vault_path: PathBuf,
) -> Router {
    let state = Arc::new(UiState {
        store,
        project_root: None,
        workspace_root: Some(workspace_root),
        workspace_vault_path: Some(workspace_vault_path),
        workspace_cache: Mutex::new(None),
    });
    build_router().with_state(state)
}

/// The route table. Shared between `serve` (binds to a port) and
/// `router_for_test` (used with `oneshot` in tests). All routes are GET-only;
/// the UI is read-only by design (the vault is mutated by the scheduler,
/// not by the UI).
fn build_router() -> Router<Arc<UiState>> {
    Router::new()
        .route("/", get(handlers::index))
        .route("/api/health", get(handlers::health))
        .route("/api/runs", get(handlers::list_runs))
        .route("/api/runs/{run_id}", get(handlers::get_run))
        .route("/api/runs/{run_id}/events", get(handlers::get_events))
        .route("/api/runs/{run_id}/status", get(handlers::get_status))
        .route("/api/runs/{run_id}/timeline", get(handlers::get_timeline))
        .route(
            "/api/runs/{run_id}/nodes/{node_id}/artifact",
            get(handlers::get_artifact),
        )
        .route("/api/project", get(project::get_project))
        .route("/api/project/specs/{name}", get(project::get_spec))
        .route("/api/workspace", get(workspace_handlers::get_workspace))
        .route(
            "/api/workspace/projects/{name}",
            get(workspace_handlers::get_workspace_project),
        )
        .route(
            "/api/workspace/projects/{name}/specs/{spec}",
            get(workspace_handlers::get_workspace_project_spec),
        )
        .route(
            "/api/workspace/projects/{name}/runs/{run_id}",
            get(workspace_handlers::get_workspace_project_run),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Tests are in spec_parser.rs (unit tests for parsing)
    // and tests/ui_tests.rs (integration tests for endpoints)
}

//! Response and error types for the trace UI web server.

use crate::core::error::PidagError;
use crate::core::event::Event;
use crate::store::{NodeRecord, RunMeta};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub runs: usize,
}

#[derive(Serialize)]
pub struct RunDetailResponse {
    pub run: RunMeta,
    /// The spec (phase) name derived from the DAG's `metadata.spec`, so the
    /// run-detail view can show which phase this run belongs to. `None` for
    /// plain `pidag run dag.json` runs (no metadata) or legacy runs.
    pub spec: Option<String>,
    pub nodes: Vec<NodeRecord>,
}

#[derive(Serialize)]
pub struct SeqEvent {
    pub seq: u64,
    pub event: Event,
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub run_id: String,
    pub text: String,
}

/// Timeline response for `GET /api/runs/:id/timeline`. `groups` has one
/// entry per DAG node (topological order); `items` has one entry per
/// dispatched node. Pending nodes appear as group-only (no item).
#[derive(Serialize)]
pub struct TimelineResponse {
    pub run_id: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub groups: Vec<TimelineGroup>,
    pub items: Vec<TimelineItem>,
}

#[derive(Serialize)]
pub struct TimelineGroup {
    pub id: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct TimelineItem {
    pub id: String,
    pub group: String,
    pub content: String,
    pub start: String,       // RFC3339
    pub end: Option<String>, // RFC3339 or null (running)
    /// vis-timeline expects the camelCase `className` JSON key. Rename so
    /// the Rust field is snake_case (clippy-clean) while the wire format
    /// stays compatible with vis-timeline's item API.
    #[serde(rename = "className")]
    pub class_name: String, // "done", "running", "failed", etc.
    pub title: String,       // HTML tooltip
}

/// Artifact response for `GET /api/runs/:id/nodes/:node_id/artifact`.
/// `artifact` is `null` when the node has no stored output.
#[derive(Serialize)]
pub struct ArtifactResponse {
    pub run_id: String,
    pub node_id: String,
    pub artifact: Option<String>,
}

/// A `RunMeta` augmented with `spec` -- the spec (phase) name derived from
/// the DAG's `metadata.spec` field at read time. The `#[serde(flatten)]`
/// keeps all existing `RunMeta` fields at the top level so the response is
/// backward-compatible with the frontend (the new `spec` field is additive).
/// This avoids a schema change to `RunMeta` (which is bincode-serialized).
#[derive(Serialize)]
pub struct RunSummary {
    #[serde(flatten)]
    pub run: RunMeta,
    pub spec: Option<String>,
}

/// Project Overview response for `GET /api/project`. `project_root` is
/// `null` when no `--project-root` was passed (UI shows a hint). `specs` is
/// sorted by filename. `runs` is the same data as `GET /api/runs`.
/// `project_name` is the directory name of `project_root` (e.g. "pidag" for
/// "crates/pidag") -- `null` when no root is configured. The frontend uses it
/// for the page title and header so multiple pidag UIs are distinguishable.
#[derive(Serialize)]
pub struct ProjectOverview {
    pub project_root: Option<String>,
    pub project_name: Option<String>,
    pub specs: Vec<SpecSummary>,
    pub runs: Vec<RunSummary>,
}

#[derive(Serialize)]
pub struct SpecSummary {
    pub name: String,  // file stem, e.g. "shell-node-dispatch"
    pub title: String, // first "# " heading, or the name if none
    pub file: String,  // path relative to project_root, e.g. "specs/foo.md"
    pub exit_criteria_total: usize,
    pub exit_criteria_done: usize,
    pub status: String, // "complete" | "in-progress" | "pending" | "no-criteria"
}

/// Spec detail response for `GET /api/project/specs/:name`. `content` is
/// the full markdown text of the spec file.
#[derive(Serialize)]
pub struct SpecDetail {
    pub name: String,
    pub title: String,
    pub file: String,
    pub content: String,
    pub exit_criteria_total: usize,
    pub exit_criteria_done: usize,
}

#[derive(Deserialize)]
pub struct EventsQuery {
    pub since: Option<u64>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for axum handlers. Converts to HTTP status + plain-text body.
/// `Store` errors become 500, missing runs become 404, JSON parse errors
/// become 422.
pub enum AppError {
    Store(PidagError),
    NotFound,
    Json(serde_json::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::Store(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("store error: {}", e),
            ),
            AppError::NotFound => (StatusCode::NOT_FOUND, "run not found".to_string()),
            AppError::Json(e) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("failed to parse stored DAG JSON: {}", e),
            ),
        };
        (status, msg).into_response()
    }
}

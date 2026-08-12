//! Core handlers for the trace UI: health, runs, events, timeline, artifacts.
//!
//! These handlers serve the single-project trace views (sessions list,
//! run-detail, event polling, Gantt timeline).

use super::UiState;
use super::types::{
    AppError, ArtifactResponse, EventsQuery, HealthResponse, RunDetailResponse, RunSummary,
    SeqEvent, StatusResponse, TimelineGroup, TimelineItem, TimelineResponse,
};
use crate::core::dag::Dag;
use crate::store::{RunMeta, Store};
use crate::ui::render::render_status;
use axum::extract::{Path, Query, State};
use axum::response::{Html, Json};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

/// Serve the embedded SPA shell. The HTML, CSS, and JS are all inline in
/// `ui_assets/index.html` so the binary has no external file dependencies.
pub async fn index() -> Html<&'static str> {
    Html(include_str!("../ui_assets/index.html"))
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Health check. Returns the run count so the frontend can show a quick
/// "vault is reachable" indicator. A store error degrades gracefully to
/// `runs: 0` rather than failing the health probe.
pub async fn health(State(state): State<Arc<UiState>>) -> Json<HealthResponse> {
    let count = state.store.list_runs().await.map(|r| r.len()).unwrap_or(0);
    Json(HealthResponse {
        status: "ok".to_string(),
        runs: count,
    })
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// List all runs in the vault. Used by the sessions-list view. Each run is
/// returned as a `RunSummary` (RunMeta + the spec/phase name parsed from the
/// DAG's `metadata.spec`) so the UI can show which phase each run belongs to.
pub async fn list_runs(
    State(state): State<Arc<UiState>>,
) -> Result<Json<Vec<RunSummary>>, AppError> {
    let runs = state.store.list_runs().await.map_err(AppError::Store)?;
    Ok(Json(runs.into_iter().map(run_summary).collect()))
}

/// Fetch a single run's metadata plus all its node records. Used by the
/// run-detail view to render the node list.
pub async fn get_run(
    State(state): State<Arc<UiState>>,
    Path(run_id): Path<String>,
) -> Result<Json<RunDetailResponse>, AppError> {
    Ok(Json(build_run_detail(&state.store, &run_id).await?))
}

/// Build a `RunDetailResponse` for a run (metadata + node records) from a
/// given store. Shared between the single-project handler (`get_run`, store =
/// the UI's own vault) and the workspace project-run endpoint (store = the
/// project's own vault).
pub async fn build_run_detail(
    store: &Arc<dyn Store>,
    run_id: &str,
) -> Result<RunDetailResponse, AppError> {
    let run = store
        .get_run(run_id)
        .await
        .map_err(AppError::Store)?
        .ok_or(AppError::NotFound)?;
    let spec = run_spec(&run);
    let nodes = store.list_nodes(run_id).await.map_err(AppError::Store)?;
    Ok(RunDetailResponse { run, spec, nodes })
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Poll for new events since a sequence cursor. The `since` query parameter
/// is the last seq the client has already seen; the response contains only
/// events with `seq > since`, sorted by `seq` ascending. This is the polling
/// endpoint that powers the trace waterfall.
pub async fn get_events(
    State(state): State<Arc<UiState>>,
    Path(run_id): Path<String>,
    Query(params): Query<EventsQuery>,
) -> Result<Json<Vec<SeqEvent>>, AppError> {
    let since = params.since.unwrap_or(0);
    let events = state
        .store
        .load_events_since(&run_id, since)
        .await
        .map_err(AppError::Store)?;
    let result: Vec<SeqEvent> = events
        .into_iter()
        .map(|(seq, ev)| SeqEvent { seq, event: ev })
        .collect();
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Render the run status as text via `render_status` (the same output as
/// `pidag show`). The DAG is parsed from the `dag_json` field of `RunMeta`;
/// if parsing fails (corrupted vault or non-standard DAG JSON), we return
/// a 500 with the parse error rather than a misleading empty status.
pub async fn get_status(
    State(state): State<Arc<UiState>>,
    Path(run_id): Path<String>,
) -> Result<Json<StatusResponse>, AppError> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .map_err(AppError::Store)?
        .ok_or(AppError::NotFound)?;
    let nodes = state
        .store
        .list_nodes(&run_id)
        .await
        .map_err(AppError::Store)?;

    // Parse the stored DAG JSON so render_status can walk the topology.
    // A failed parse is a real error (the vault is corrupt or the run was
    // created with a non-standard DAG), so we surface it rather than
    // silently returning an empty status.
    let dag: Dag = serde_json::from_str(&run.dag_json).map_err(AppError::Json)?;

    // Build the states map: node_id -> (state, model). We use the model
    // field (not the artifact/output) because render_status labels this
    // field as "model" and the full LLM output can be megabytes -- not
    // suitable for a status overview. The detailed node output is
    // available via the node list / artifacts endpoints.
    let mut states: HashMap<String, (String, Option<String>)> = HashMap::new();
    for n in &nodes {
        states.insert(
            n.node_id.clone(),
            (n.state.as_str().to_string(), n.model.clone()),
        );
    }
    let text = render_status(&dag, &states);
    Ok(Json(StatusResponse { run_id, text }))
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

/// Fetch the Gantt timeline data for a run: one group per DAG node (in
/// topological order) and one item per node that has been dispatched (with
/// start/end timestamps and a state-derived CSS class). Powers the trace
/// UI's vis-timeline view. Pending nodes (no `NodeDispatched` yet) appear
/// as groups with no item.
pub async fn get_timeline(
    State(state): State<Arc<UiState>>,
    Path(run_id): Path<String>,
) -> Result<Json<TimelineResponse>, AppError> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .map_err(AppError::Store)?
        .ok_or(AppError::NotFound)?;
    let timings = state
        .store
        .list_node_timings(&run_id)
        .await
        .map_err(AppError::Store)?;
    let nodes = state
        .store
        .list_nodes(&run_id)
        .await
        .map_err(AppError::Store)?;

    // Parse the stored DAG so we can emit groups for ALL nodes (including
    // pending ones that have no timing yet) in topological order. A failed
    // parse is a real error (corrupt vault), surfaced as 422.
    let dag: Dag = serde_json::from_str(&run.dag_json).map_err(AppError::Json)?;
    let topo = dag.topo_sort().unwrap_or_default();
    let groups: Vec<TimelineGroup> = topo
        .iter()
        .map(|id| TimelineGroup {
            id: id.to_string(),
            content: id.to_string(),
        })
        .collect();

    // Build a node_id -> state lookup so timeline items can carry the
    // current state (used for the CSS class and the bar content label).
    let node_state: HashMap<String, &str> = nodes
        .iter()
        .map(|n| (n.node_id.clone(), n.state.as_str()))
        .collect();
    let node_model: HashMap<String, &str> = nodes
        .iter()
        .filter_map(|n| n.model.as_ref().map(|m| (n.node_id.clone(), m.as_str())))
        .collect();

    let items: Vec<TimelineItem> = if timings.is_empty() {
        // Legacy-run fallback: this run's events were emitted before the
        // NodeTiming table existed (P1 #10), so `list_node_timings` returns
        // nothing and the timeline would be empty. Synthesize thin range
        // bars from `NodeRecord.timestamp`: parse the RFC3339 timestamp,
        // add a 60-second offset to produce a visible rectangle (end), and
        // render as a `range` item. The tooltip explains this is a legacy
        // estimate, not a real start->end measurement. NEW runs get proper
        // range bars from the NodeTiming projection.
        const LEGACY_BAR_SECS: i64 = 60;
        nodes
            .iter()
            .map(|n| {
                let state = n.state.as_str();
                let cls = state_to_css_class(state);
                let model = n.model.as_deref().unwrap_or("--");
                // Try to produce a synthetic end = start + 60s. If the
                // timestamp can't be parsed, fall back to None (point).
                let end = chrono::DateTime::parse_from_rfc3339(&n.timestamp)
                    .ok()
                    .map(|t| {
                        (t + chrono::Duration::seconds(LEGACY_BAR_SECS))
                            .with_timezone(&chrono::Utc)
                            .to_rfc3339()
                    });
                let bar_note = if end.is_some() {
                    "estimated bar (+/-60s) -- legacy run has no start/end timing"
                } else {
                    "point at last state-change -- legacy run, timestamp unparseable"
                };
                let title = format!(
                    "<b>{}</b><br/>state: {}<br/>model: {}<br/>last-seen: {}<br/><i>({})</i>",
                    n.node_id, state, model, n.timestamp, bar_note
                );
                TimelineItem {
                    id: n.node_id.clone(),
                    group: n.node_id.clone(),
                    content: state.to_string(),
                    start: n.timestamp.clone(),
                    end, // Some -> thin rectangle; None -> point (parse failed)
                    class_name: cls.to_string(),
                    title,
                }
            })
            .collect()
    } else {
        timings
            .iter()
            .map(|(node_id, t)| {
                let state = node_state.get(node_id).copied().unwrap_or("pending");
                let cls = state_to_css_class(state);
                let model = node_model.get(node_id).copied().unwrap_or("--");
                // Tooltip (vis-timeline `title` property) -- HTML allowed.
                let ended_str = t.ended_at.as_deref().unwrap_or("running");
                let title = format!(
                    "<b>{}</b><br/>state: {}<br/>model: {}<br/>started: {}<br/>ended: {}",
                    node_id, state, model, t.started_at, ended_str
                );
                TimelineItem {
                    id: node_id.clone(),
                    group: node_id.clone(),
                    content: state.to_string(),
                    start: t.started_at.clone(),
                    end: t.ended_at.clone(),
                    class_name: cls.to_string(),
                    title,
                }
            })
            .collect()
    };

    Ok(Json(TimelineResponse {
        run_id: run_id.clone(),
        started_at: run.started_at,
        completed_at: run.completed_at,
        groups,
        items,
    }))
}

/// Map node state to CSS class for timeline items.
fn state_to_css_class(state: &str) -> &'static str {
    match state {
        "Done" => "done",
        "Running" => "running",
        "Failed" => "failed",
        "Blocked" => "blocked",
        "Skipped" => "skipped",
        _ => "pending",
    }
}

// ---------------------------------------------------------------------------
// Artifacts
// ---------------------------------------------------------------------------

/// Fetch a single node's artifact (output text). Used by the timeline
/// item click-to-detail panel. Returns `null` for the `artifact` field when
/// the node has no stored output (pending, running, or skipped nodes).
pub async fn get_artifact(
    State(state): State<Arc<UiState>>,
    Path((run_id, node_id)): Path<(String, String)>,
) -> Result<Json<ArtifactResponse>, AppError> {
    // Verify the run exists (404 if not) before reading the artifact.
    let _run = state
        .store
        .get_run(&run_id)
        .await
        .map_err(AppError::Store)?
        .ok_or(AppError::NotFound)?;
    let artifact = state
        .store
        .get_artifact(&run_id, &node_id)
        .await
        .map_err(AppError::Store)?;
    Ok(Json(ArtifactResponse {
        run_id,
        node_id,
        artifact,
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the spec (phase) name from a run's DAG JSON. Returns `None` when
/// the DAG has no `metadata.spec` (e.g. a plain `pidag run dag.json` with a
/// hand-written DAG, or a legacy run predating the metadata field).
pub fn run_spec(run: &RunMeta) -> Option<String> {
    let dag: Dag = serde_json::from_str(&run.dag_json).ok()?;
    dag.metadata.and_then(|m| m.get("spec").cloned())
}

/// Convert a `RunMeta` into a `RunSummary` by parsing its DAG JSON for the
/// spec provenance.
pub fn run_summary(run: RunMeta) -> RunSummary {
    let spec = run_spec(&run);
    RunSummary { run, spec }
}

//! A2A (Agent-to-Agent) protocol server for pidag
//!
//! Enables external A2A clients to:
//! 1. Discover pidag via Agent Card at `/.well-known/agent-card.json`
//! 2. Submit DAG tasks via `POST tasks/send`
//! 3. Poll task status via `GET tasks/get?id=<task_id>`
//! 4. Retrieve task artifacts on completion
//!
//! Architecture:
//! - Agent Card: JSON metadata about pidag's capabilities
//! - tasks/send: JSON-RPC 2.0 endpoint that creates a pidag run
//! - tasks/get: Query task state mapped from pidag run state
//! - (Future) SSE streaming for real-time status updates

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::rpc::ServerState;
use crate::store::Store;

// ---------------------------------------------------------------------------
// Agent Card
// ---------------------------------------------------------------------------

/// Agent Card per A2A Protocol specification.
/// Served at `/.well-known/agent-card.json` for discovery by A2A clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub version: String,
    pub description: String,
    pub endpoint: String,
    pub capabilities: Vec<String>,
    pub modalities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AuthConfig>,
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub r#type: String,
}

impl Default for AgentCard {
    fn default() -> Self {
        Self {
            name: "pidag".to_string(),
            version: "0.1.0".to_string(),
            description: "Deterministic, resilient multi-node LLM DAG executor".to_string(),
            endpoint: "http://localhost:8080".to_string(),
            capabilities: vec![
                "dag_execution".to_string(),
                "sdd_spec_processing".to_string(),
            ],
            modalities: vec!["text".to_string(), "json".to_string()],
            authentication: Some(AuthConfig {
                r#type: "none".to_string(),
            }),
            streaming: false,
        }
    }
}

// ---------------------------------------------------------------------------
// A2A Request/Response types
// ---------------------------------------------------------------------------

/// A2A tasks/send request body (JSON-RPC 2.0 wrapper not included here;
/// this is the inner "params" object)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksSendParams {
    /// DAG JSON to execute
    pub dag: Value,
    /// Optional SDD spec path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_path: Option<String>,
}

/// A2A tasks/get query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksGetQuery {
    /// Task ID returned from tasks/send
    pub id: String,
}

/// A2A task state response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStateResponse {
    /// Task ID (run ID)
    pub id: String,
    /// Task state: submitted, working, completed, failed, canceled
    pub state: String,
    /// Optional error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Node artifacts if completed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Value>,
    /// Node states for partial progress
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Value>,
}

// ---------------------------------------------------------------------------
// A2A Server State and Handler
// ---------------------------------------------------------------------------

/// A2A server state wraps the shared ServerState
pub struct A2aServerState {
    pub inner: Arc<ServerState>,
    pub agent_card: AgentCard,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /.well-known/agent-card.json
async fn agent_card_handler(State(state): State<Arc<A2aServerState>>) -> impl IntoResponse {
    Json(state.agent_card.clone())
}

/// POST /tasks/send -- Submit a DAG for execution
async fn tasks_send_handler(
    State(state): State<Arc<A2aServerState>>,
    Json(body): Json<TasksSendParams>,
) -> impl IntoResponse {
    // Parse the DAG JSON
    let dag = match crate::core::dag::Dag::deserialize(body.dag.clone()) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "code": -32602,
                        "message": format!("Invalid DAG JSON: {}", e)
                    }
                })),
            )
                .into_response();
        }
    };

    // Validate the DAG
    if let Err(e) = dag.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "code": -32000,
                    "message": format!("DAG validation failed: {}", e)
                }
            })),
        )
            .into_response();
    }

    // Submit the DAG via handler
    match crate::rpc::handle_dag_submit(&state.inner, &body.dag).await {
        Ok(result) => (StatusCode::OK, Json(json!({"result": result}))).into_response(),
        Err((code, msg)) => (
            StatusCode::OK,
            Json(json!({
                "error": {
                    "code": code,
                    "message": msg
                }
            })),
        )
            .into_response(),
    }
}

/// GET /tasks/get -- Query task status
async fn tasks_get_handler(
    State(state): State<Arc<A2aServerState>>,
    Query(query): Query<TasksGetQuery>,
) -> impl IntoResponse {
    // Get the run status from handler
    match crate::rpc::handle_dag_status(&state.inner, &query.id).await {
        Ok(result) => {
            // Map pidag run state to A2A state
            // pidag states: "submitted", "working", "completed", "failed"
            // Note: The handler may return states in various formats
            let a2a_state = if let Some(status_obj) = result.get("status") {
                if let Some(state_str) = status_obj.as_str() {
                    match state_str {
                        "Pending" | "submitted" => "submitted".to_string(),
                        "Running" | "working" => "working".to_string(),
                        "Done" | "completed" => "completed".to_string(),
                        "Failed" | "failed" => "failed".to_string(),
                        _ => state_str.to_lowercase(),
                    }
                } else {
                    "unknown".to_string()
                }
            } else if let Some(state_str) = result.get("state").and_then(|v| v.as_str()) {
                match state_str {
                    "Pending" | "submitted" => "submitted".to_string(),
                    "Running" | "working" => "working".to_string(),
                    "Done" | "completed" => "completed".to_string(),
                    "Failed" | "failed" => "failed".to_string(),
                    _ => state_str.to_lowercase(),
                }
            } else {
                // Default to submitted if run exists but no state returned
                "submitted".to_string()
            };

            let response = TaskStateResponse {
                id: query.id,
                state: a2a_state,
                error: result
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                artifacts: result.get("artifacts").cloned(),
                nodes: result.get("nodes").cloned(),
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err((code, msg)) => (
            StatusCode::OK,
            Json(json!({
                "error": {
                    "code": code,
                    "message": msg
                }
            })),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Router construction
// ---------------------------------------------------------------------------

/// Create the A2A server router with all endpoints
pub fn router(state: Arc<A2aServerState>) -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card_handler))
        .route("/tasks/send", post(tasks_send_handler))
        .route("/tasks/get", get(tasks_get_handler))
        .with_state(state)
}

/// Create the A2A server router for testing with a MockStore
pub fn router_for_test(store: Arc<dyn Store>) -> Router {
    let config = crate::Config::default();
    let server_state = Arc::new(ServerState::new(
        std::path::PathBuf::from("_tmp/test-a2a"),
        4,
        store,
        config,
    ));
    let a2a_state = Arc::new(A2aServerState {
        inner: server_state,
        agent_card: AgentCard::default(),
    });
    router(a2a_state)
}

// Implement serialization for DAG that uses serde_json
impl crate::core::dag::Dag {
    fn deserialize(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

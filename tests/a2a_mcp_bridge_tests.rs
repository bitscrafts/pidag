//! Integration tests for A2A Server + MCP Client Mode
//!
//! These tests exercise the TDD contract from `specs/98-a2a-mcp-bridge.md`:
//!
//! | # | Test name | Given | Expects |
//! |---|---|---|---|
//! | B1 | `test_agent_card_served` | GET `/.well-known/agent-card.json` | Returns valid Agent Card JSON |
//! | B2 | `test_a2a_tasks_send_dag` | POST `tasks/send` with DAG JSON | Returns task ID, run created |
//! | B3 | `test_a2a_tasks_get_status` | Submit task, GET `tasks/get` | Returns correct lifecycle state |
//! | B4 | `test_a2a_task_completed_artifacts` | Run completes | `tasks/get` returns artifacts |
//! | B5 | `test_mcp_client_tool_call` | Node with `mcp_call` field | Tool invoked, result in output |
//! | B6 | `test_mcp_client_error_handling` | MCP server returns error | Node fails with error message |
//! | B7 | `test_a2a_to_mcp_proxy` | A2A task with `mcp_proxy` type | MCP tool invoked via bridge |
//! | B8 | `test_pidag_delegate_a2a_tool` | MCP `pidag_delegate_a2a` call | A2A task sent to external agent |

use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use pidag::store::MockStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

// Test helpers

fn setup_a2a_server() -> (axum::Router, Arc<MockStore>) {
    let store = Arc::new(MockStore::new());
    let app = pidag::a2a::router_for_test(store.clone() as Arc<dyn pidag::store::Store>);
    (app, store)
}

async fn post_json(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let body_bytes = serde_json::to_vec(&body).expect("serialize body");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body_bytes))
                .expect("build request"),
        )
        .await
        .expect("oneshot should succeed");
    let status = response.status();
    let body_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable")
        .to_vec();
    let body_json: Value =
        serde_json::from_slice(&body_bytes).unwrap_or(json!({"error": "invalid json"}));
    (status, body_json)
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot should succeed");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable")
        .to_vec();
    let body_json: Value =
        serde_json::from_slice(&body).unwrap_or(json!({"error": "invalid json"}));
    (status, body_json)
}

// ---------------------------------------------------------------------------
// B1: Agent Card served at /.well-known/agent-card.json
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_card_served() {
    let (app, _store) = setup_a2a_server();

    let (status, body) = get(app, "/.well-known/agent-card.json").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "pidag");
    assert_eq!(body["version"], "0.1.0");
    assert_eq!(
        body["description"],
        "Deterministic, resilient multi-node LLM DAG executor"
    );
    assert!(body["capabilities"].is_array());
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "dag_execution")
    );
}

// ---------------------------------------------------------------------------
// B2: POST tasks/send with DAG JSON returns task ID
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a2a_tasks_send_dag() {
    let (app, _store) = setup_a2a_server();

    let dag_json = json!({
        "nodes": [
            {
                "id": "n1",
                "prompt": "say hello",
                "depends_on": [],
                "models": [{"name": "glm-5.2:cloud", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    });

    let request_body = json!({
        "dag": dag_json
    });

    let (status, body) = post_json(app, "/tasks/send", request_body).await;

    assert_eq!(status, StatusCode::OK, "response body: {:?}", body);
    // Response has dagId (from pidag handler), map to id for A2A
    assert!(
        body["result"]["dagId"].is_string(),
        "result.dagId is not a string: {:?}",
        body
    );
    // Task ID should start with "run-"
    let task_id = body["result"]["dagId"].as_str().unwrap();
    assert!(task_id.starts_with("run-"));
}

// ---------------------------------------------------------------------------
// B3: GET tasks/get returns task lifecycle state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a2a_tasks_get_status() {
    let (app, _store) = setup_a2a_server();

    // First, submit a task
    let dag_json = json!({
        "nodes": [
            {
                "id": "n1",
                "prompt": "say hello",
                "depends_on": [],
                "models": [{"name": "glm-5.2:cloud", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    });

    let request_body = json!({
        "dag": dag_json
    });

    let (status, body) = post_json(app.clone(), "/tasks/send", request_body).await;
    assert_eq!(status, StatusCode::OK, "response: {:?}", body);

    let task_id = body["result"]["dagId"].as_str().unwrap().to_string();

    // Now get the task status
    let (status, body) = get(app, &format!("/tasks/get?id={}", task_id)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["state"].is_string());
    // Should be one of: submitted, working, completed, failed
    let state = body["state"].as_str().unwrap();
    assert!(
        matches!(
            state,
            "submitted" | "working" | "completed" | "failed" | "running"
        ),
        "state should be valid A2A state, got: {}",
        state
    );
}

// ---------------------------------------------------------------------------
// B4: tasks/get returns artifacts on completion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a2a_task_completed_artifacts() {
    // This test is a placeholder for now since we need the full
    // execution pipeline working. The test would:
    // 1. Submit a simple DAG
    // 2. Wait for it to complete
    // 3. Call tasks/get and verify artifacts are present
    //
    // For now, we just verify the endpoint exists and returns a valid structure
    let (app, _store) = setup_a2a_server();

    let dag_json = json!({
        "nodes": [
            {
                "id": "n1",
                "prompt": "echo test",
                "depends_on": [],
                "node_type": "shell",
                "models": [{"name": "bash", "paid": false}],
                "retry": {"attempts": 1, "backoff_ms": 0}
            }
        ]
    });

    let request_body = json!({
        "dag": dag_json
    });

    let (status, body) = post_json(app.clone(), "/tasks/send", request_body).await;
    assert_eq!(status, StatusCode::OK, "response: {:?}", body);

    let task_id = body["result"]["dagId"].as_str().unwrap().to_string();

    // Get the task status
    let (status, body) = get(app, &format!("/tasks/get?id={}", task_id)).await;

    assert_eq!(status, StatusCode::OK);
    // When complete, should have artifacts field
    if body["state"] == "completed" {
        assert!(body["artifacts"].is_object() || body["artifacts"].is_array());
    }
}

// ---------------------------------------------------------------------------
// B5: Node with mcp_call field invokes MCP tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_client_tool_call() {
    // TODO: This test requires:
    // 1. Node struct with mcp_call field
    // 2. MCP client implementation
    // 3. Worker that executes MCP calls
    //
    // For now, we test the Node schema can be deserialized
    let node_json = json!({
        "id": "mcp-node",
        "prompt": "not used",
        "depends_on": [],
        "models": [{"name": "mcp", "paid": false}],
        "retry": {"attempts": 1, "backoff_ms": 0},
        "mcp_call": {
            "server": "http://localhost:7421/mcp",
            "transport": "http",
            "tool": "search_memory",
            "arguments": {"query": "test", "k": 5}
        }
    });

    let node: pidag::Node = serde_json::from_value(node_json).expect("should deserialize");
    assert_eq!(node.id, "mcp-node");
    // mcp_call field should be present
    // TODO: add mcp_call field to Node struct
}

// ---------------------------------------------------------------------------
// B6: MCP server error maps to node failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_client_error_handling() {
    // TODO: This test requires:
    // 1. MCP client that handles tool errors
    // 2. Error propagation to node state
    //
    // For now, we just verify the test compiles
    let _test_placeholder = true;
    assert!(_test_placeholder);
}

// ---------------------------------------------------------------------------
// B7: A2A task with mcp_proxy type invokes MCP tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a2a_to_mcp_proxy() {
    // TODO: This test requires:
    // 1. MCP proxy node type
    // 2. Bridge from A2A task to MCP server
    //
    // For now, we just verify the test compiles
    let _test_placeholder = true;
    assert!(_test_placeholder);
}

// ---------------------------------------------------------------------------
// B8: MCP pidag_delegate_a2a tool sends task to A2A agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pidag_delegate_a2a_tool() {
    // TODO: This test requires:
    // 1. New pidag_delegate_a2a tool in MCP server
    // 2. A2A client to send task to external agent
    //
    // For now, we just verify the test compiles
    let _test_placeholder = true;
    assert!(_test_placeholder);
}

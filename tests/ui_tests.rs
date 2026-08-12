//! Integration tests for the trace UI (P1 #9 of the SSSF patterns adoption
//! spec).
//!
//! These tests exercise the axum HTTP API against a `MockStore` (no redb
//! needed) using `tower::ServiceExt::oneshot` to drive the router without
//! binding a port. The 8 tests cover the TDD contract from
//! `specs/96-sssf-patterns-adoption.md`:
//!
//! | # | Test                                  | What it checks                    |
//! |---|---------------------------------------|-----------------------------------|
//! | T1 | test_ui_list_runs_returns_json        | GET /api/runs returns 2 runs       |
//! | T2 | test_ui_get_run_returns_nodes         | GET /api/runs/:id returns run+nodes|
//! | T3 | test_ui_events_since_filters_by_seq   | GET .../events?since=1 filters     |
//! | T4 | test_ui_health_endpoint               | GET /api/health returns status+count |
//! | T5 | test_ui_status_endpoint               | GET .../status returns render text |
//! | T6 | test_ui_serves_index_html             | GET / returns the embedded HTML    |
//! | T7 | test_list_runs_store_method           | MockStore::list_runs returns all   |
//! | T8 | test_load_events_since_store_method   | MockStore::load_events_since filters |

use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use pidag::Event;
use pidag::NodeStatus;
use pidag::store::{MockStore, NodeRecord, RunMeta, Store};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a test router backed by a fresh MockStore, returning both the
/// router (for sending requests) and the store handle (for inserting data).
fn setup() -> (axum::Router, Arc<MockStore>) {
    let store = Arc::new(MockStore::new());
    let app = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    (app, store)
}

/// A minimal valid DAG JSON with one node, used for RunMeta.dag_json so the
/// status endpoint can parse it into a Dag and call render_status.
const ONE_NODE_DAG_JSON: &str = r#"{"nodes":[{"id":"n1","prompt":"do","depends_on":[],"models":[{"name":"glm-5.2:cloud","paid":false}],"retry":{"attempts":1,"backoff_ms":0}}]}"#;

fn sample_run(run_id: &str, completed: bool) -> RunMeta {
    RunMeta {
        run_id: run_id.to_string(),
        dag_json: ONE_NODE_DAG_JSON.to_string(),
        started_at: "2026-08-03T12:00:00Z".to_string(),
        completed_at: if completed {
            Some("2026-08-03T12:01:00Z".to_string())
        } else {
            None
        },
        successful_nodes: if completed { 1 } else { 0 },
        failed_nodes: 0,
    }
}

fn sample_node(node_id: &str, state: NodeStatus) -> NodeRecord {
    NodeRecord {
        node_id: node_id.to_string(),
        state,
        model: Some("glm-5.2:cloud".to_string()),
        attempt: 1,
        timestamp: "2026-08-03T12:00:30Z".to_string(),
    }
}

/// Insert a run and optionally its nodes into the store.
async fn insert_run(store: &Arc<MockStore>, run: &RunMeta, nodes: &[NodeRecord]) {
    store.put_run(run).await.expect("put_run should succeed");
    for n in nodes {
        store
            .put_node_state(&run.run_id, &n.node_id, n)
            .await
            .expect("put_node_state should succeed");
    }
}

/// Send a GET request to the router and return (status, body bytes).
async fn get(app: axum::Router, uri: &str) -> (StatusCode, Vec<u8>) {
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
    (status, body)
}

// ---------------------------------------------------------------------------
// T1: GET /api/runs returns all runs as JSON
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ui_list_runs_returns_json() {
    let (app, store) = setup();
    insert_run(&store, &sample_run("run-001", true), &[]).await;
    insert_run(&store, &sample_run("run-002", false), &[]).await;

    let (status, body) = get(app, "/api/runs").await;
    assert_eq!(status, StatusCode::OK);
    let runs: Vec<RunMeta> = serde_json::from_slice(&body).expect("body is Vec<RunMeta> JSON");
    assert_eq!(runs.len(), 2, "should return both inserted runs");
    // MockStore::list_runs sorts by run_id, so order is deterministic.
    assert_eq!(runs[0].run_id, "run-001");
    assert_eq!(runs[1].run_id, "run-002");
}

// ---------------------------------------------------------------------------
// T2: GET /api/runs/:id returns run metadata + node records
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ui_get_run_returns_nodes() {
    let (app, store) = setup();
    let run = sample_run("run-nodes", false);
    let nodes = vec![
        sample_node("n1", NodeStatus::Done),
        sample_node("n2", NodeStatus::Running),
    ];
    insert_run(&store, &run, &nodes).await;

    let (status, body) = get(app, "/api/runs/run-nodes").await;
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("body is RunDetailResponse JSON");
    assert_eq!(json["run"]["run_id"], "run-nodes");
    assert_eq!(json["nodes"].as_array().expect("nodes is array").len(), 2);
    assert_eq!(json["nodes"][0]["node_id"], "n1");
    assert_eq!(json["nodes"][0]["state"], "Done");
    assert_eq!(json["nodes"][1]["node_id"], "n2");
    assert_eq!(json["nodes"][1]["state"], "Running");
}

// ---------------------------------------------------------------------------
// T3: GET /api/runs/:id/events?since=N returns only events with seq > N
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ui_events_since_filters_by_seq() {
    let (_, store) = setup();
    let run = sample_run("run-ev", false);
    store.put_run(&run).await.expect("put_run");

    // Append 3 events (seq 0, 1, 2).
    store
        .append_event(&run.run_id, &Event::DagSubmitted)
        .await
        .expect("append");
    store
        .append_event(
            &run.run_id,
            &Event::NodeDispatched {
                node_id: "n1".to_string(),
                model: "glm-5.2:cloud".to_string(),
                attempt: 1,
            },
        )
        .await
        .expect("append");
    store
        .append_event(
            &run.run_id,
            &Event::NodeDone {
                node_id: "n1".to_string(),
                model: "glm-5.2:cloud".to_string(),
                output: "done".to_string(),
            },
        )
        .await
        .expect("append");

    // since=1 → only seq > 1, i.e. seq 2 only. Build a fresh router for
    // each request because tower::ServiceExt::oneshot consumes the router.
    let app1 = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    let (status, body) = get(app1, "/api/runs/run-ev/events?since=1").await;
    assert_eq!(status, StatusCode::OK);
    let events: serde_json::Value =
        serde_json::from_slice(&body).expect("body is Vec<SeqEvent> JSON");
    let arr = events.as_array().expect("events is array");
    assert_eq!(arr.len(), 1, "since=1 should return only seq 2 (one event)");
    assert_eq!(arr[0]["seq"], 2);
    assert!(
        arr[0]["event"]["NodeDone"].is_object(),
        "the event should be NodeDone"
    );

    // since=0 → seq 1 and 2 (two events; filter is strictly >, not >=).
    let app2 = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    let (status2, body2) = get(app2, "/api/runs/run-ev/events?since=0").await;
    assert_eq!(status2, StatusCode::OK);
    let events2: serde_json::Value = serde_json::from_slice(&body2).expect("JSON");
    let arr2 = events2.as_array().expect("array");
    assert_eq!(arr2.len(), 2, "since=0 should return seq 1 and 2");
    for ev in arr2 {
        let seq = ev["seq"].as_u64().expect("seq is u64");
        assert!(seq > 0, "no event with seq <= since=0 should be returned");
    }

    // since=5 → no events (all seqs are < 5).
    let app3 = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    let (status3, body3) = get(app3, "/api/runs/run-ev/events?since=5").await;
    assert_eq!(status3, StatusCode::OK);
    let events3: serde_json::Value = serde_json::from_slice(&body3).expect("JSON");
    assert!(
        events3.as_array().expect("array").is_empty(),
        "since=5 should return no events"
    );
}

// ---------------------------------------------------------------------------
// T4: GET /api/health returns {"status":"ok","runs":N}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ui_health_endpoint() {
    let (app, store) = setup();
    insert_run(&store, &sample_run("run-h1", true), &[]).await;
    insert_run(&store, &sample_run("run-h2", true), &[]).await;
    insert_run(&store, &sample_run("run-h3", false), &[]).await;

    let (status, body) = get(app, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("body is HealthResponse JSON");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["runs"], 3, "should count all inserted runs");
}

// ---------------------------------------------------------------------------
// T5: GET /api/runs/:id/status returns render_status output as text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ui_status_endpoint() {
    let (app, store) = setup();
    let run = sample_run("run-status", false);
    let nodes = vec![sample_node("n1", NodeStatus::Done)];
    insert_run(&store, &run, &nodes).await;

    let (status, body) = get(app, "/api/runs/run-status/status").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("body is StatusResponse JSON");
    assert_eq!(json["run_id"], "run-status");
    let text = json["text"].as_str().expect("text is a string");
    // render_status output contains the node id and state glyph.
    assert!(
        text.contains("n1"),
        "status text should include node id, got: {text}"
    );
    assert!(
        text.contains("Done"),
        "status text should include the Done state, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// T6: GET / returns the embedded index.html
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ui_serves_index_html() {
    let (app, _store) = setup();
    let (status, body) = get(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8(body).expect("body is UTF-8 HTML");
    assert!(
        html.contains("pidag trace"),
        "index.html should contain the page title 'pidag trace'"
    );
    assert!(
        html.contains("<!DOCTYPE html>"),
        "index.html should be a valid HTML document"
    );
}

// ---------------------------------------------------------------------------
// T7: MockStore::list_runs returns all inserted runs (store-level test)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_runs_store_method() {
    let store = MockStore::new();
    store
        .put_run(&sample_run("run-a", true))
        .await
        .expect("put_run a");
    store
        .put_run(&sample_run("run-b", false))
        .await
        .expect("put_run b");
    store
        .put_run(&sample_run("run-c", true))
        .await
        .expect("put_run c");

    let runs = store.list_runs().await.expect("list_runs");
    assert_eq!(runs.len(), 3, "should return all 3 inserted runs");
    // list_runs sorts by run_id.
    assert_eq!(runs[0].run_id, "run-a");
    assert_eq!(runs[1].run_id, "run-b");
    assert_eq!(runs[2].run_id, "run-c");

    // Empty store returns empty vec.
    let empty = MockStore::new();
    let r = empty.list_runs().await.expect("list_runs on empty store");
    assert!(r.is_empty(), "empty store should return no runs");
}

// ---------------------------------------------------------------------------
// T8: MockStore::load_events_since filters by seq > N (store-level test)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_load_events_since_store_method() {
    let store = MockStore::new();
    let run_id = "run-since";

    // Append 5 events (seq 0..4).
    for i in 0..5 {
        store
            .append_event(
                run_id,
                &Event::NodeDone {
                    node_id: format!("n{}", i),
                    model: "m".to_string(),
                    output: format!("out {}", i),
                },
            )
            .await
            .expect("append_event");
    }

    // since=2 → seq 3, 4 (two events).
    let events = store
        .load_events_since(run_id, 2)
        .await
        .expect("load_events_since");
    assert_eq!(events.len(), 2, "since=2 should return seq 3 and 4");
    assert_eq!(events[0].0, 3, "first event seq should be 3");
    assert_eq!(events[1].0, 4, "second event seq should be 4");

    // since=4 → no events (only seq > 4, but max seq is 4).
    let none = store
        .load_events_since(run_id, 4)
        .await
        .expect("load_events_since");
    assert!(none.is_empty(), "since=4 should return no events");

    // since=0 → seq 1,2,3,4 (four events; NOT seq 0 since filter is >).
    let some = store
        .load_events_since(run_id, 0)
        .await
        .expect("load_events_since");
    assert_eq!(some.len(), 4, "since=0 should return seq 1..4");
    assert_eq!(some[0].0, 1, "first should be seq 1");

    // Unknown run → empty vec, not an error.
    let missing = store
        .load_events_since("nonexistent", 0)
        .await
        .expect("load_events_since on missing run");
    assert!(missing.is_empty(), "missing run should return empty vec");
}

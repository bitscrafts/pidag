//! Integration tests for the trace UI timeline (P1 #10: live Gantt view).
//!
//! These tests exercise:
//!   - The `NodeTiming` store projection (T1, T2): `RedbSink` writes
//!     start/end timing on `NodeDispatched` / `NodeDone` / `NodeFailed`.
//!   - The `GET /api/runs/:id/timeline` endpoint (T3, T4, T5): groups for
//!     ALL DAG nodes (including pending), items with `end: null` for
//!     running nodes, `className: "failed"` for failed nodes.
//!   - The `GET /api/runs/:id/nodes/:node_id/artifact` endpoint (T6):
//!     returns the stored output for a done node, `null` for a pending one.
//!
//! All tests use `MockStore` (no redb needed) and drive the axum router via
//! `tower::ServiceExt::oneshot`, matching the pattern in `ui_tests.rs`.

use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use pidag::Event;
use pidag::EventSink;
use pidag::NodeStatus;
use pidag::RedbSink;
use pidag::store::{MockStore, NodeRecord, RunMeta, Store};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Two-node DAG: n1 (root) → n2 (depends on n1). Used so T3 can verify the
/// timeline returns a group for the pending node n2 even when only n1 has
/// been dispatched.
const TWO_NODE_DAG_JSON: &str = r#"{"nodes":[{"id":"n1","prompt":"do n1","depends_on":[],"models":[{"name":"glm-5.2:cloud","paid":false}],"retry":{"attempts":1,"backoff_ms":0}},{"id":"n2","prompt":"do n2","depends_on":["n1"],"models":[{"name":"glm-5.2:cloud","paid":false}],"retry":{"attempts":1,"backoff_ms":0}}]}"#;

fn sample_run(run_id: &str, completed: bool) -> RunMeta {
    RunMeta {
        run_id: run_id.to_string(),
        dag_json: TWO_NODE_DAG_JSON.to_string(),
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

/// Build a test router backed by a fresh MockStore.
fn setup() -> (axum::Router, Arc<MockStore>) {
    let store = Arc::new(MockStore::new());
    let app = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    (app, store)
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
// T1: RedbSink::emit(NodeDispatched) writes NodeTiming { started_at, None }
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_node_timing_dispatched() {
    let store = Arc::new(MockStore::new()) as Arc<dyn Store>;
    let mut sink = RedbSink::new(Arc::clone(&store), "run-t1".to_string());

    // Put the run first (RedbSink::DagSubmitted would, but we test the
    // NodeDispatched projection in isolation).
    store
        .put_run(&sample_run("run-t1", false))
        .await
        .expect("put_run");

    sink.emit(&Event::NodeDispatched {
        node_id: "n1".to_string(),
        model: "glm-5.2:cloud".to_string(),
        attempt: 1,
    })
    .await
    .expect("emit NodeDispatched");

    let timing = store
        .get_node_timing("run-t1", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing should exist after dispatch");
    assert!(
        !timing.started_at.is_empty(),
        "started_at should be set on dispatch"
    );
    assert!(
        timing.ended_at.is_none(),
        "ended_at should be None while running, got {:?}",
        timing.ended_at
    );
}

// ---------------------------------------------------------------------------
// T2: After NodeDone, get_node_timing returns ended_at: Some, started_at preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_node_timing_done() {
    let store = Arc::new(MockStore::new()) as Arc<dyn Store>;
    let mut sink = RedbSink::new(Arc::clone(&store), "run-t2".to_string());
    store
        .put_run(&sample_run("run-t2", false))
        .await
        .expect("put_run");

    sink.emit(&Event::NodeDispatched {
        node_id: "n1".to_string(),
        model: "glm-5.2:cloud".to_string(),
        attempt: 1,
    })
    .await
    .expect("emit NodeDispatched");

    let after_dispatch = store
        .get_node_timing("run-t2", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing after dispatch");
    let started = after_dispatch.started_at.clone();

    sink.emit(&Event::NodeDone {
        node_id: "n1".to_string(),
        model: "glm-5.2:cloud".to_string(),
        output: "result text".to_string(),
    })
    .await
    .expect("emit NodeDone");

    let after_done = store
        .get_node_timing("run-t2", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing after done");
    assert_eq!(
        after_done.started_at, started,
        "started_at must be preserved from dispatch, not overwritten"
    );
    assert!(
        after_done.ended_at.is_some(),
        "ended_at should be Some after NodeDone"
    );
    assert!(
        after_done.ended_at.as_deref().unwrap_or("") >= started.as_str(),
        "ended_at ({:?}) should be >= started_at ({})",
        after_done.ended_at,
        started
    );
}

// ---------------------------------------------------------------------------
// T3: GET /api/runs/:id/timeline returns groups for ALL DAG nodes
//     (including pending ones that have no timing yet)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timeline_endpoint_returns_groups() {
    let (app, store) = setup();
    let run = sample_run("run-t3", false);
    // Only n1 has been dispatched; n2 is pending (no timing, no node record).
    let nodes = vec![sample_node("n1", NodeStatus::Running)];
    store.put_run(&run).await.expect("put_run");
    for n in &nodes {
        store
            .put_node_state(&run.run_id, &n.node_id, n)
            .await
            .expect("put_node_state");
    }
    // Manually insert a timing for n1 (the RedbSink path would do this).
    store
        .put_node_timing(
            &run.run_id,
            "n1",
            &pidag::store::NodeTiming {
                started_at: "2026-08-03T12:00:00Z".to_string(),
                ended_at: None,
            },
        )
        .await
        .expect("put_node_timing");

    let (status, body) = get(app, "/api/runs/run-t3/timeline").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("TimelineResponse JSON");
    let groups = json["groups"].as_array().expect("groups is array");
    assert_eq!(
        groups.len(),
        2,
        "should have a group for EVERY DAG node (n1 + n2), got {groups:?}"
    );
    let ids: Vec<&str> = groups
        .iter()
        .map(|g| g["id"].as_str().expect("group id is string"))
        .collect();
    assert!(ids.contains(&"n1"), "n1 must be a group, got {ids:?}");
    assert!(
        ids.contains(&"n2"),
        "n2 (pending) must be a group, got {ids:?}"
    );

    // Items: only n1 has timing, so exactly one item.
    let items = json["items"].as_array().expect("items is array");
    assert_eq!(items.len(), 1, "only n1 should have an item");
    assert_eq!(items[0]["id"], "n1");
}

// ---------------------------------------------------------------------------
// T4: Running node has end: null in the timeline items
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timeline_endpoint_running_node() {
    let (app, store) = setup();
    let run = sample_run("run-t4", false);
    store.put_run(&run).await.expect("put_run");
    store
        .put_node_state(&run.run_id, "n1", &sample_node("n1", NodeStatus::Running))
        .await
        .expect("put_node_state");
    store
        .put_node_timing(
            &run.run_id,
            "n1",
            &pidag::store::NodeTiming {
                started_at: "2026-08-03T12:00:00Z".to_string(),
                ended_at: None,
            },
        )
        .await
        .expect("put_node_timing");

    let (status, body) = get(app, "/api/runs/run-t4/timeline").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert!(
        items[0]["end"].is_null(),
        "running node end must be null, got {:?}",
        items[0]["end"]
    );
    assert_eq!(items[0]["className"], "running");
}

// ---------------------------------------------------------------------------
// T5: Failed node has end: Some(...) and className: "failed"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_timeline_endpoint_failed_node() {
    let (app, store) = setup();
    let run = sample_run("run-t5", false);
    store.put_run(&run).await.expect("put_run");
    store
        .put_node_state(&run.run_id, "n1", &sample_node("n1", NodeStatus::Failed))
        .await
        .expect("put_node_state");
    store
        .put_node_timing(
            &run.run_id,
            "n1",
            &pidag::store::NodeTiming {
                started_at: "2026-08-03T12:00:00Z".to_string(),
                ended_at: Some("2026-08-03T12:00:10Z".to_string()),
            },
        )
        .await
        .expect("put_node_timing");

    let (status, body) = get(app, "/api/runs/run-t5/timeline").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["className"], "failed", "failed node className");
    assert!(
        items[0]["end"].is_string(),
        "failed node end must be a string (Some), got {:?}",
        items[0]["end"]
    );
    assert_eq!(items[0]["end"], "2026-08-03T12:00:10Z");
}

// ---------------------------------------------------------------------------
// T6: GET /api/runs/:id/nodes/:node_id/artifact returns output for done,
//     null for pending
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_artifact_endpoint() {
    let (app, store) = setup();
    let run = sample_run("run-t6", false);
    store.put_run(&run).await.expect("put_run");
    // n1 is Done with an artifact; n2 is pending (no artifact).
    store
        .put_node_state(&run.run_id, "n1", &sample_node("n1", NodeStatus::Done))
        .await
        .expect("put_node_state n1");
    store
        .put_artifact(&run.run_id, "n1", "the LLM output text")
        .await
        .expect("put_artifact");

    // Done node → artifact text.
    let (status, body) = get(app, "/api/runs/run-t6/nodes/n1/artifact").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("ArtifactResponse JSON");
    assert_eq!(json["node_id"], "n1");
    assert_eq!(json["artifact"], "the LLM output text");

    // Pending node → null artifact. Build a fresh router (oneshot consumes).
    let app2 = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    let (status2, body2) = get(app2, "/api/runs/run-t6/nodes/n2/artifact").await;
    assert_eq!(status2, StatusCode::OK);
    let json2: serde_json::Value = serde_json::from_slice(&body2).expect("JSON");
    assert_eq!(json2["node_id"], "n2");
    assert!(
        json2["artifact"].is_null(),
        "pending node artifact must be null, got {:?}",
        json2["artifact"]
    );
}

// ---------------------------------------------------------------------------
// T7: NodeRetry and re-dispatch PRESERVE the original started_at so a
//     retried node's Gantt bar spans the full retry+backoff window, not
//     just the final attempt. Regression test for the bug where a 4-minute
//     node with retries rendered as a 14ms bar (the last attempt only).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_node_retry_preserves_started_at() {
    let store = Arc::new(MockStore::new()) as Arc<dyn Store>;
    let mut sink = RedbSink::new(Arc::clone(&store), "run-t7".to_string());
    store
        .put_run(&sample_run("run-t7", false))
        .await
        .expect("put_run");

    // First dispatch — sets started_at to T0.
    sink.emit(&Event::NodeDispatched {
        node_id: "n1".to_string(),
        model: "glm-5.2:cloud".to_string(),
        attempt: 1,
    })
    .await
    .expect("emit first dispatch");

    let after_first = store
        .get_node_timing("run-t7", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing after first dispatch");
    let original_start = after_first.started_at.clone();
    assert!(
        !original_start.is_empty(),
        "started_at set on first dispatch"
    );
    assert!(
        after_first.ended_at.is_none(),
        "ended_at None while running"
    );

    // Retry — must NOT overwrite started_at; must clear ended_at.
    sink.emit(&Event::NodeRetry {
        node_id: "n1".to_string(),
        reason: "attempt failed".to_string(),
    })
    .await
    .expect("emit NodeRetry");

    let after_retry = store
        .get_node_timing("run-t7", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing after retry");
    assert_eq!(
        after_retry.started_at, original_start,
        "NodeRetry must preserve original started_at, not overwrite with now"
    );
    assert!(
        after_retry.ended_at.is_none(),
        "NodeRetry must clear ended_at (node is in-flight again)"
    );

    // Re-dispatch (attempt 2) — must ALSO preserve the original started_at.
    // Without this fix, the retry dispatch would reset started_at to now and
    // the Gantt bar would shrink to just the final attempt's duration.
    sink.emit(&Event::NodeDispatched {
        node_id: "n1".to_string(),
        model: "glm-5.2:cloud".to_string(),
        attempt: 2,
    })
    .await
    .expect("emit re-dispatch");

    let after_redispatch = store
        .get_node_timing("run-t7", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing after re-dispatch");
    assert_eq!(
        after_redispatch.started_at, original_start,
        "retry dispatch (attempt 2) must preserve original started_at"
    );
    assert!(
        after_redispatch.ended_at.is_none(),
        "re-dispatch must clear ended_at"
    );

    // Final failure — ended_at set, started_at still the original T0.
    sink.emit(&Event::NodeFailed {
        node_id: "n1".to_string(),
        error: "execution failed".to_string(),
    })
    .await
    .expect("emit NodeFailed");

    let after_fail = store
        .get_node_timing("run-t7", "n1")
        .await
        .expect("get_node_timing")
        .expect("timing after fail");
    assert_eq!(
        after_fail.started_at, original_start,
        "NodeFailed must preserve original started_at across the whole retry window"
    );
    assert!(after_fail.ended_at.is_some(), "ended_at set on failure");
}

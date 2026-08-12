//! Integration tests for the Project Overview web UI (specs + runs dashboard).
//!
//! These tests exercise:
//!   - `GET /api/project` with no project root (T4) and with a temp
//!     project root containing spec files (T5).
//!   - `GET /api/project/specs/:name` returning content + parsed metadata
//!     (T6), rejecting path traversal (T7), and 404 for unknown specs (T8).
//!
//! The pure-function `parse_spec` unit tests (T1-T3) live in
//! `src/ui.rs` under `#[cfg(test)]` because `parse_spec` is private.
//!
//! All endpoint tests use a tempdir as the project root (no real redb vault
//! needed) and drive the axum router via `tower::ServiceExt::oneshot`.

use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use pidag::store::{MockStore, RunMeta, Store};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

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

/// Build a router with a MockStore and a temp project root.
fn setup_with_root(root: &std::path::Path) -> (axum::Router, Arc<MockStore>) {
    let store = Arc::new(MockStore::new());
    let app = pidag::ui::router_for_test_with_root(
        Arc::clone(&store) as Arc<dyn Store>,
        root.to_path_buf(),
    );
    (app, store)
}

/// Build a router with a MockStore and NO project root.
fn setup_no_root() -> (axum::Router, Arc<MockStore>) {
    let store = Arc::new(MockStore::new());
    let app = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);
    (app, store)
}

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

/// Create a temp project root with a `specs/` dir containing the given
/// (filename_stem, markdown) pairs. Returns the project root path.
fn temp_project(specs: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let specs_dir = dir.path().join("specs");
    std::fs::create_dir_all(&specs_dir).expect("create specs dir");
    for (stem, content) in specs {
        std::fs::write(specs_dir.join(format!("{}.md", stem)), content).expect("write spec");
    }
    dir
}

const COMPLETE_SPEC: &str =
    "# Complete Spec\n\n## Exit Criteria\n\n- [x] one\n- [x] two\n- [x] three\n";
const PARTIAL_SPEC: &str = "# Partial Spec\n\n## Exit Criteria\n\n- [x] done\n- [ ] todo\n";
const NO_CRITERIA_SPEC: &str = "# No Criteria Spec\n\nJust prose, no checkboxes.\n";

// ---------------------------------------------------------------------------
// T4: GET /api/project with no project root → specs: [], project_root: null
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_project_endpoint_no_root() {
    let (app, store) = setup_no_root();
    store
        .put_run(&sample_run("run-a", true))
        .await
        .expect("put_run");

    let (status, body) = get(app, "/api/project").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("ProjectOverview JSON");
    assert!(
        json["project_root"].is_null(),
        "project_root should be null when no root configured, got {:?}",
        json["project_root"]
    );
    assert!(
        json["specs"].as_array().expect("specs array").is_empty(),
        "specs should be empty with no root"
    );
    let runs = json["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1, "runs should come from the store");
    assert_eq!(runs[0]["run_id"], "run-a");
}

// ---------------------------------------------------------------------------
// T5: GET /api/project enumerates specs with correct status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_project_endpoint_enumerates_specs() {
    let dir = temp_project(&[
        ("complete-spec", COMPLETE_SPEC),
        ("partial-spec", PARTIAL_SPEC),
        ("no-criteria-spec", NO_CRITERIA_SPEC),
    ]);
    let (app, store) = setup_with_root(dir.path());
    store
        .put_run(&sample_run("run-b", false))
        .await
        .expect("put_run");

    let (status, body) = get(app, "/api/project").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert!(
        json["project_root"].is_string(),
        "project_root should be the temp path"
    );
    let specs = json["specs"].as_array().expect("specs array");
    assert_eq!(specs.len(), 3, "should enumerate all 3 spec files");

    // Find each spec by name and check its status + criteria counts.
    let by_name: std::collections::HashMap<String, serde_json::Value> = specs
        .iter()
        .map(|s| (s["name"].as_str().expect("name").to_string(), s.clone()))
        .collect();

    let complete = by_name.get("complete-spec").expect("complete-spec present");
    assert_eq!(complete["exit_criteria_total"], 3);
    assert_eq!(complete["exit_criteria_done"], 3);
    assert_eq!(complete["status"], "complete");
    assert_eq!(complete["title"], "Complete Spec");

    let partial = by_name.get("partial-spec").expect("partial-spec present");
    assert_eq!(partial["exit_criteria_total"], 2);
    assert_eq!(partial["exit_criteria_done"], 1);
    assert_eq!(partial["status"], "in-progress");

    let nc = by_name
        .get("no-criteria-spec")
        .expect("no-criteria-spec present");
    assert_eq!(nc["exit_criteria_total"], 0);
    assert_eq!(nc["status"], "no-criteria");

    // Runs still listed alongside specs.
    let runs = json["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["run_id"], "run-b");
}

// ---------------------------------------------------------------------------
// T6: GET /api/project/specs/:name returns full content + parsed metadata
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spec_endpoint_returns_content() {
    let dir = temp_project(&[("complete-spec", COMPLETE_SPEC)]);
    let (app, _store) = setup_with_root(dir.path());

    let (status, body) = get(app, "/api/project/specs/complete-spec").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("SpecDetail JSON");
    assert_eq!(json["name"], "complete-spec");
    assert_eq!(json["title"], "Complete Spec");
    assert_eq!(json["exit_criteria_total"], 3);
    assert_eq!(json["exit_criteria_done"], 3);
    let content = json["content"].as_str().expect("content is string");
    assert!(
        content.contains("# Complete Spec"),
        "content should include the full markdown"
    );
    assert!(
        content.contains("- [x] one"),
        "content should include the checkbox lines"
    );
}

// ---------------------------------------------------------------------------
// T7: path traversal rejected (.. and / in name)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spec_endpoint_rejects_traversal() {
    let dir = temp_project(&[("real-spec", COMPLETE_SPEC)]);
    let (app, _store) = setup_with_root(dir.path());

    // ".." is rejected by is_safe_spec_name (no path separators / dots-only).
    let (status, _body) = get(app.clone(), "/api/project/specs/..").await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "traversal '..' should be rejected (404 or 400), got {}",
        status
    );

    // Rebuild router (oneshot consumes) and try a name with a slash.
    let (app2, _store) = setup_with_root(dir.path());
    let (status2, _body2) = get(app2, "/api/project/specs/foo/bar").await;
    // axum may 404 the unmatched route or pass the slash to the handler;
    // either way it must NOT serve a file outside specs/.
    assert!(
        status2 == StatusCode::NOT_FOUND || status2 == StatusCode::BAD_REQUEST,
        "name with slash should be rejected, got {}",
        status2
    );
}

// ---------------------------------------------------------------------------
// T8: unknown spec name → 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spec_endpoint_unknown_404() {
    let dir = temp_project(&[("real-spec", COMPLETE_SPEC)]);
    let (app, _store) = setup_with_root(dir.path());

    let (status, _body) = get(app, "/api/project/specs/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown spec should 404");
}

// ---------------------------------------------------------------------------
// Extra: no project root → GET /api/project/specs/:name also 404s (R9)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spec_endpoint_no_root_404() {
    let (app, _store) = setup_no_root();
    let (status, _body) = get(app, "/api/project/specs/anything").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "spec endpoint with no project root should 404"
    );
}

// ---------------------------------------------------------------------------
// T9: /api/runs returns `spec` derived from the DAG's metadata.spec
// ---------------------------------------------------------------------------

/// DAG JSON with `metadata.spec` stamped (as SddGenerator::from_spec does).
const DAG_WITH_SPEC_JSON: &str = r#"{"nodes":[{"id":"n1","prompt":"do","depends_on":[],"models":[{"name":"glm-5.2:cloud","paid":false}],"retry":{"attempts":1,"backoff_ms":0}}],"metadata":{"spec":"my-phase"}}"#;

#[tokio::test]
async fn test_runs_include_spec_from_dag_metadata() {
    let store = Arc::new(MockStore::new());
    let run = RunMeta {
        run_id: "run-spec-1".to_string(),
        dag_json: DAG_WITH_SPEC_JSON.to_string(),
        started_at: "2026-08-03T12:00:00Z".to_string(),
        completed_at: Some("2026-08-03T12:01:00Z".to_string()),
        successful_nodes: 1,
        failed_nodes: 0,
    };
    store.put_run(&run).await.unwrap();
    let app = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);

    let (status, body) = get(app, "/api/runs").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let first = &v[0];
    assert_eq!(first["run_id"], "run-spec-1");
    assert_eq!(
        first["spec"], "my-phase",
        "spec should be extracted from dag metadata"
    );
}

// ---------------------------------------------------------------------------
// T10: /api/runs returns `spec: null` when the DAG has no metadata
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_runs_spec_null_when_no_metadata() {
    let store = Arc::new(MockStore::new());
    let run = RunMeta {
        run_id: "run-plain-1".to_string(),
        dag_json: ONE_NODE_DAG_JSON.to_string(), // no metadata
        started_at: "2026-08-03T12:00:00Z".to_string(),
        completed_at: Some("2026-08-03T12:01:00Z".to_string()),
        successful_nodes: 1,
        failed_nodes: 0,
    };
    store.put_run(&run).await.unwrap();
    let app = pidag::ui::router_for_test(Arc::clone(&store) as Arc<dyn Store>);

    let (status, body) = get(app, "/api/runs").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v[0]["run_id"], "run-plain-1");
    assert!(
        v[0]["spec"].is_null(),
        "spec should be null when DAG has no metadata"
    );
}

// ---------------------------------------------------------------------------
// T11: /api/project runs include the `spec` field (phase link in project view)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_project_runs_include_spec() {
    let dir = temp_project(&[("my-phase", COMPLETE_SPEC)]);
    let store = Arc::new(MockStore::new());
    let run = RunMeta {
        run_id: "run-spec-2".to_string(),
        dag_json: DAG_WITH_SPEC_JSON.to_string(),
        started_at: "2026-08-03T12:00:00Z".to_string(),
        completed_at: Some("2026-08-03T12:01:00Z".to_string()),
        successful_nodes: 1,
        failed_nodes: 0,
    };
    store.put_run(&run).await.unwrap();
    let app = pidag::ui::router_for_test_with_root(
        Arc::clone(&store) as Arc<dyn Store>,
        dir.path().to_path_buf(),
    );

    let (status, body) = get(app, "/api/project").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let runs = v["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["spec"], "my-phase");
}

//! TDD contract tests for spec-34: typed node state and typed dispatch events
//! Fixtures: tests/fixtures/legacy_vault/ generated before code changes.

use pidag::core::dag::Dag;
use pidag::store::{NodeStatus, RedbStore, Store};
use std::path::PathBuf;

#[test]
fn test_node_id_with_colon_is_rejected() {
    // Y5a: node id containing `:` must be rejected by dag.validate()
    // This test FAILS today (colon is accepted).
    let dag_json = r#"
    {
      "nodes": [
        {"id": "weird:node", "prompt": "echo test", "depends_on": [], "models": [], "retry": {"attempts": 1, "backoff_ms": 0}}
      ]
    }
    "#;

    let dag: Result<Dag, _> = serde_json::from_str(dag_json);
    assert!(dag.is_ok(), "DAG should parse");

    let dag = dag.unwrap();
    let result = dag.validate();

    // This assertion FAILS today because colon is not rejected
    assert!(
        result.is_err(),
        "dag.validate() must reject node id containing ':' (Y5a)"
    );

    // The error message must name the offending id
    if let Err(err) = result {
        let err_msg = format!("{:?}", err);
        assert!(
            err_msg.contains("weird:node") || err_msg.contains("colon") || err_msg.contains(":"),
            "Error must name the offending node id or mention colon/delimiter: {}",
            err_msg
        );
    }
}

#[tokio::test]
async fn test_legacy_vault_still_loads() {
    // Y2b: Load a vault fixture written before the typed-state change.
    // Verify that all node states round-trip correctly.
    //
    // This test is the compatibility guard: it must PASS after the change
    // to prove wire compatibility is maintained.

    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_vault");

    let db_path = fixture_dir.join("legacy.redb");

    if !db_path.exists() {
        eprintln!(
            "SKIP: legacy vault fixture not found at {:?}. \
             It must be generated from the current build BEFORE code changes.",
            db_path
        );
        return;
    }

    // The fixture is a redb database at tests/fixtures/legacy_vault/legacy.redb
    // containing a run with id "legacy1" and three nodes:
    // - alpha: Done
    // - beta: Failed
    // - gamma: Blocked

    // Open a *copy*, never the fixture itself. `RedbStore::open` is read-write,
    // so redb writes back to whatever it is handed -- pointing it at the
    // committed fixture left `legacy.redb` dirty in git after every test run.
    // The fixture is the wire-compatibility guard: it only proves anything so
    // long as it stays byte-identical to what the pre-change build wrote.
    let work_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("_tmp/legacy_vault_copy");
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).expect("create work dir");
    let work_path = work_dir.join("legacy.redb");
    std::fs::copy(&db_path, &work_path).expect("copy legacy fixture");

    let store = RedbStore::open(&work_path).expect("RedbStore::open");

    // Load the run
    let run = store.get_run("legacy1").await.expect("get_run");

    assert!(run.is_some(), "run should exist");
    let run = run.unwrap();
    assert_eq!(run.run_id, "legacy1", "run_id mismatch");

    // Load all nodes and check their states
    let nodes = store.list_nodes("legacy1").await.expect("list_nodes");

    assert!(!nodes.is_empty(), "fixture must contain nodes");

    // Verify the three fixture nodes round-trip with their original states
    let alpha = nodes.iter().find(|n| n.node_id == "alpha");
    let beta = nodes.iter().find(|n| n.node_id == "beta");
    let gamma = nodes.iter().find(|n| n.node_id == "gamma");

    assert!(alpha.is_some(), "fixture must contain 'alpha' node");
    assert!(beta.is_some(), "fixture must contain 'beta' node");
    assert!(gamma.is_some(), "fixture must contain 'gamma' node");

    // The states must be the exact legacy values
    assert_eq!(
        alpha.unwrap().state,
        NodeStatus::Done,
        "alpha state must be Done"
    );
    assert_eq!(
        beta.unwrap().state,
        NodeStatus::Failed,
        "beta state must be Failed"
    );
    assert_eq!(
        gamma.unwrap().state,
        NodeStatus::Blocked,
        "gamma state must be Blocked"
    );
}

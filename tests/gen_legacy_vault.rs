/// Generate the legacy vault fixture for spec-34 Y2b compatibility test.
/// This creates a redb vault containing a run with the three required node states.
/// Run this test ONCE before any code changes to establish the fixture baseline.
use pidag::store::{NodeRecord, NodeStatus, RedbStore, RunMeta, Store};
use std::path::PathBuf;

/// IGNORED ON PURPOSE -- run only via `cargo test --test gen_legacy_vault -- --ignored`.
///
/// Without this attribute the generator ran on every `cargo test` and rewrote
/// the fixture using the *current* build, which `test_legacy_vault_still_loads`
/// then read back with that same build. The compatibility guard was circular:
/// it could not fail, no matter how the wire format changed. That is exactly
/// what happened -- the spec-34 refactor commit carried a regenerated fixture,
/// so the guard proved nothing from the moment the change it guards landed.
/// The committed fixture must only ever be produced by a build that predates
/// the change under test.
#[tokio::test]
#[ignore]
async fn gen_legacy_vault_fixture() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_vault");

    // Create the directory if it doesn't exist
    std::fs::create_dir_all(&fixture_dir).expect("create fixture dir");

    // Open the redb database at the expected path
    let db_path = fixture_dir.join("legacy.redb");
    let store = RedbStore::open(&db_path).expect("open store");

    // Create a run with id "legacy1"
    let run = RunMeta {
        run_id: "legacy1".to_string(),
        dag_json: r#"{"nodes":[{"id":"alpha","prompt":"echo alpha","depends_on":[],"models":[],"retry":{"attempts":1,"backoff_ms":0}},{"id":"beta","prompt":"echo beta","depends_on":[],"models":[],"retry":{"attempts":1,"backoff_ms":0}},{"id":"gamma","prompt":"echo gamma","depends_on":[],"models":[],"retry":{"attempts":1,"backoff_ms":0}}]}"#.to_string(),
        started_at: "2026-08-12T00:00:00Z".to_string(),
        completed_at: Some("2026-08-12T00:01:00Z".to_string()),
        successful_nodes: 2,
        failed_nodes: 1,
    };

    store.put_run(&run).await.expect("put_run");

    // Create three nodes with the required states (exactly matching the wire format)
    let alpha = NodeRecord {
        node_id: "alpha".to_string(),
        state: NodeStatus::Done,
        model: None,
        attempt: 1,
        timestamp: "2026-08-12T00:00:10Z".to_string(),
    };

    let beta = NodeRecord {
        node_id: "beta".to_string(),
        state: NodeStatus::Failed,
        model: None,
        attempt: 1,
        timestamp: "2026-08-12T00:00:20Z".to_string(),
    };

    let gamma = NodeRecord {
        node_id: "gamma".to_string(),
        state: NodeStatus::Blocked,
        model: None,
        attempt: 0,
        timestamp: "2026-08-12T00:00:30Z".to_string(),
    };

    store
        .put_node_state("legacy1", "alpha", &alpha)
        .await
        .expect("put alpha");
    store
        .put_node_state("legacy1", "beta", &beta)
        .await
        .expect("put beta");
    store
        .put_node_state("legacy1", "gamma", &gamma)
        .await
        .expect("put gamma");

    // Verify the fixture loaded correctly
    let loaded_run = store
        .get_run("legacy1")
        .await
        .expect("get_run")
        .expect("run exists");
    assert_eq!(loaded_run.run_id, "legacy1");

    let nodes = store.list_nodes("legacy1").await.expect("list_nodes");
    assert_eq!(nodes.len(), 3, "fixture must contain exactly 3 nodes");

    let loaded_alpha = nodes.iter().find(|n| n.node_id == "alpha").expect("alpha");
    let loaded_beta = nodes.iter().find(|n| n.node_id == "beta").expect("beta");
    let loaded_gamma = nodes.iter().find(|n| n.node_id == "gamma").expect("gamma");

    assert_eq!(loaded_alpha.state, NodeStatus::Done);
    assert_eq!(loaded_beta.state, NodeStatus::Failed);
    assert_eq!(loaded_gamma.state, NodeStatus::Blocked);

    println!(
        "✓ Legacy vault fixture created at {:?}",
        fixture_dir.join("legacy.redb")
    );
    println!("  Run: legacy1");
    println!("  Nodes: alpha (Done), beta (Failed), gamma (Blocked)");
    println!("  This fixture is the compatibility guard for spec-34 Y2b.");
    println!("  Do NOT regenerate after code changes - commit this fixture first!");
}

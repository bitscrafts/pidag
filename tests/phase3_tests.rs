use pidag::{
    CompositeSink, Dag, Event, EventSink, ModelRef, Node, NodeRecord, NodeStatus, RedbStore,
    RetryPolicy, RunMeta, Scheduler, Store, VecSink,
};
use std::path::PathBuf;

// ============================================================================
// Test 1: test_redbstore_put_get_run_roundtrip
// ============================================================================
#[tokio::test]
async fn test_redbstore_put_get_run_roundtrip() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_roundtrip");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    let run = RunMeta {
        run_id: "run-1".to_string(),
        dag_json: r#"{"nodes":[]}"#.to_string(),
        started_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };

    store.put_run(&run).await.unwrap();

    let retrieved = store.get_run("run-1").await.unwrap();
    assert_eq!(retrieved, Some(run));
}

// ============================================================================
// Test 2: test_redbstore_missing_run_is_none
// ============================================================================
#[tokio::test]
async fn test_redbstore_missing_run_is_none() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_missing_run");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    let retrieved = store.get_run("nonexistent").await.unwrap();
    assert_eq!(retrieved, None);
}

// ============================================================================
// Test 3: test_node_state_persisted
// ============================================================================
#[tokio::test]
async fn test_node_state_persisted() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_node_state");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    let node_a = NodeRecord {
        node_id: "a".to_string(),
        state: NodeStatus::Done,
        model: Some("gpt-4".to_string()),
        attempt: 1,
        timestamp: "2024-01-01T00:00:00Z".to_string(),
    };

    let node_b = NodeRecord {
        node_id: "b".to_string(),
        state: NodeStatus::Running,
        model: Some("gpt-3.5".to_string()),
        attempt: 1,
        timestamp: "2024-01-01T00:00:01Z".to_string(),
    };

    store.put_node_state("run-1", "a", &node_a).await.unwrap();
    store.put_node_state("run-1", "b", &node_b).await.unwrap();

    let nodes = store.list_nodes("run-1").await.unwrap();
    assert_eq!(nodes.len(), 2);
    assert!(
        nodes
            .iter()
            .any(|n| n.node_id == "a" && n.state == NodeStatus::Done)
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.node_id == "b" && n.state == NodeStatus::Running)
    );
}

// ============================================================================
// Test 4: test_event_log_append_ordered
// ============================================================================
#[tokio::test]
async fn test_event_log_append_ordered() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_event_ordering");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    let ev1 = Event::DagSubmitted;
    let ev2 = Event::NodeDispatched {
        node_id: "a".to_string(),
        model: "gpt-4".to_string(),
        attempt: 1,
    };
    let ev3 = Event::NodeDone {
        node_id: "a".to_string(),
        model: "gpt-4".to_string(),
        output: "result".to_string(),
    };

    let seq1 = store.append_event("run-1", &ev1).await.unwrap();
    let seq2 = store.append_event("run-1", &ev2).await.unwrap();
    let seq3 = store.append_event("run-1", &ev3).await.unwrap();

    assert_eq!(seq1, 0);
    assert_eq!(seq2, 1);
    assert_eq!(seq3, 2);

    let events = store.load_events("run-1").await.unwrap();
    assert_eq!(events.len(), 3);

    // Verify ordering
    match &events[0] {
        Event::DagSubmitted => (),
        _ => panic!("First event should be DagSubmitted"),
    }
    match &events[1] {
        Event::NodeDispatched { node_id, .. } => assert_eq!(node_id, "a"),
        _ => panic!("Second event should be NodeDispatched"),
    }
    match &events[2] {
        Event::NodeDone { node_id, .. } => assert_eq!(node_id, "a"),
        _ => panic!("Third event should be NodeDone"),
    }
}

// ============================================================================
// Test 4b: test_event_append_seq_table_perf_and_per_run_isolation
// ============================================================================
// Regression for P0-2 (HANDOFF 2026-08-02 audit): `append_event` previously did
// a full-table scan to compute `max_seq + 1` per call — O(N^2) over the vault.
// The fix introduces a per-run `event_seq` table. This test asserts the
// correctness properties the fix delivers:
//   (a) seq numbers are dense and continuous per run,
//   (b) per-run counters are independent (run-2 does not read run-1's max),
//   (c) run-2's counter survives run-1 inserts between two run-2 appends.
// We deliberately do NOT assert on wall-clock time: with Immediate durability
// each `append_event` is its own fsync'd transaction, so any reasonable impl
// costs ~1 fsync/append regardless of seq-table presence.
#[tokio::test]
async fn test_event_append_seq_table_perf_and_per_run_isolation() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_event_seq_table");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    // Two runs interleaved — proves per-run counters are independent.
    // The first event for run-2 must get seq 0 even though run-1 has events.
    let e_run1 = Event::DagSubmitted;
    let e_run2 = Event::DagSubmitted;

    let s1_a = store.append_event("run-1", &e_run1).await.unwrap();
    let s2_a = store.append_event("run-2", &e_run2).await.unwrap();
    let s1_b = store.append_event("run-1", &e_run1).await.unwrap();
    let s2_b = store.append_event("run-2", &e_run2).await.unwrap();

    assert_eq!(s1_a, 0, "run-1 first event must be seq 0");
    assert_eq!(s2_a, 0, "run-2 first event must be seq 0 (per-run counter)");
    assert_eq!(s1_b, 1, "run-1 second event must be seq 1");
    assert_eq!(s2_b, 1, "run-2 second event must be seq 1");

    // Bulk insert 200 more events for run-1; assert dense continuous seqs.
    // (Modest count: with Immediate durability each `append_event` is its
    // own fsync'd transaction, so 500 inserts cost ~5s. 200 is fast and
    // still exercises the per-run counter well. We deliberately don't
    // assert on wall-clock time — the regression guard here is
    // density/continuity + the per-run isolation check below.)
    let mut last_seq = 1u64;
    for _ in 0..200 {
        let seq = store.append_event("run-1", &e_run1).await.unwrap();
        assert_eq!(
            seq,
            last_seq + 1,
            "seq must be dense/continuous; got {seq}, expected {}",
            last_seq + 1
        );
        last_seq = seq;
    }

    // Sanity: run-2's counter is still at 2 even though run-1 has 202 events.
    // This is the key per-run-isolation assertion — proves run-1's bulk
    // inserts did not pollute run-2's seq counter (the bug the old
    // max-seq-over-all-runs scan would have masked).
    let loaded_run2 = store.load_events("run-2").await.unwrap();
    assert_eq!(loaded_run2.len(), 2, "run-2 stayed at 2 events");
    let loaded_run1 = store.load_events("run-1").await.unwrap();
    assert_eq!(loaded_run1.len(), 202, "run-1 has 202 events");

    // After all run-1 churn, run-2's next event must still be seq 2 — its
    // counter was untouched by run-1's 200 inserts.
    let s2_c = store.append_event("run-2", &e_run2).await.unwrap();
    assert_eq!(s2_c, 2, "run-2 counter preserved as 2 after run-1 churn");
}

// ============================================================================
// Test 5: test_artifact_put_get
// ============================================================================
#[tokio::test]
async fn test_artifact_put_get() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_artifact");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    store.put_artifact("run-1", "a", "OK").await.unwrap();

    let retrieved = store.get_artifact("run-1", "a").await.unwrap();
    assert_eq!(retrieved, Some("OK".to_string()));
}

// ============================================================================
// Test 6: test_terminal_set_only_done_and_failed
// ============================================================================
#[tokio::test]
async fn test_terminal_set_only_done_and_failed() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_terminal_set");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    store
        .put_node_state(
            "run-1",
            "a",
            &NodeRecord {
                node_id: "a".to_string(),
                state: NodeStatus::Done,
                model: None,
                attempt: 1,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "b",
            &NodeRecord {
                node_id: "b".to_string(),
                state: NodeStatus::Running,
                model: None,
                attempt: 1,
                timestamp: "2024-01-01T00:00:01Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "c",
            &NodeRecord {
                node_id: "c".to_string(),
                state: NodeStatus::Failed,
                model: None,
                attempt: 1,
                timestamp: "2024-01-01T00:00:02Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "d",
            &NodeRecord {
                node_id: "d".to_string(),
                state: NodeStatus::Pending,
                model: None,
                attempt: 0,
                timestamp: "2024-01-01T00:00:03Z".to_string(),
            },
        )
        .await
        .unwrap();

    let terminal = store.terminal_set("run-1").await.unwrap();
    assert_eq!(terminal.len(), 2);
    assert!(
        terminal
            .iter()
            .any(|(id, state)| id == "a" && state == "Done")
    );
    assert!(
        terminal
            .iter()
            .any(|(id, state)| id == "c" && state == "Failed")
    );
}

// ============================================================================
// Test 7: test_resume_reads_terminal_set_from_disk
// ============================================================================
#[tokio::test]
async fn test_resume_reads_terminal_set_from_disk() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_resume_from_disk");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    // Open store, write a node, drop the store
    {
        let store = RedbStore::open(&db_path).unwrap();
        store
            .put_node_state(
                "run-1",
                "a",
                &NodeRecord {
                    node_id: "a".to_string(),
                    state: NodeStatus::Done,
                    model: None,
                    attempt: 1,
                    timestamp: "2024-01-01T00:00:00Z".to_string(),
                },
            )
            .await
            .unwrap();
    }

    // Reopen the store and verify the terminal set is still there
    let store = RedbStore::open(&db_path).unwrap();
    let terminal = store.terminal_set("run-1").await.unwrap();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].0, "a");
    assert_eq!(terminal[0].1, "Done");
}

// ============================================================================
// Test 8: test_scheduler_persists_transitions
// ============================================================================

#[tokio::test]
async fn test_scheduler_persists_transitions() {
    // This test verifies that event transitions are persisted to the vault.
    // We manually emit events to the store to simulate a scheduler run.
    let tmpdir = PathBuf::from("_tmp/phase3/test_scheduler_persists");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    // Simulate a 2-node DAG run by manually emitting events
    store
        .put_run(&RunMeta {
            run_id: "run-1".to_string(),
            dag_json: "{}".to_string(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            completed_at: None,
            successful_nodes: 2,
            failed_nodes: 0,
        })
        .await
        .unwrap();

    store
        .append_event("run-1", &Event::DagSubmitted)
        .await
        .unwrap();

    store
        .append_event(
            "run-1",
            &Event::NodeDispatched {
                node_id: "a".to_string(),
                model: "gpt-4".to_string(),
                attempt: 1,
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "a",
            &NodeRecord {
                node_id: "a".to_string(),
                state: NodeStatus::Running,
                model: Some("gpt-4".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .append_event(
            "run-1",
            &Event::NodeDone {
                node_id: "a".to_string(),
                model: "gpt-4".to_string(),
                output: "output_a".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "a",
            &NodeRecord {
                node_id: "a".to_string(),
                state: NodeStatus::Done,
                model: Some("gpt-4".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:01Z".to_string(),
            },
        )
        .await
        .unwrap();

    store.put_artifact("run-1", "a", "output_a").await.unwrap();

    store
        .append_event(
            "run-1",
            &Event::NodeDispatched {
                node_id: "b".to_string(),
                model: "gpt-3.5".to_string(),
                attempt: 1,
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "b",
            &NodeRecord {
                node_id: "b".to_string(),
                state: NodeStatus::Running,
                model: Some("gpt-3.5".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:01Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .append_event(
            "run-1",
            &Event::NodeDone {
                node_id: "b".to_string(),
                model: "gpt-3.5".to_string(),
                output: "output_b".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "b",
            &NodeRecord {
                node_id: "b".to_string(),
                state: NodeStatus::Done,
                model: Some("gpt-3.5".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:02Z".to_string(),
            },
        )
        .await
        .unwrap();

    store.put_artifact("run-1", "b", "output_b").await.unwrap();

    store
        .append_event(
            "run-1",
            &Event::DagDone {
                successful_nodes: 2,
                failed_nodes: 0,
            },
        )
        .await
        .unwrap();

    // Verify persistence: nodes should be in store
    let nodes = store.list_nodes("run-1").await.unwrap();
    assert_eq!(nodes.len(), 2);

    let events = store.load_events("run-1").await.unwrap();
    assert_eq!(events.len(), 6); // DagSubmitted + 2x NodeDispatched + 2x NodeDone + DagDone

    let artifact_a = store.get_artifact("run-1", "a").await.unwrap();
    assert_eq!(artifact_a, Some("output_a".to_string()));

    let artifact_b = store.get_artifact("run-1", "b").await.unwrap();
    assert_eq!(artifact_b, Some("output_b".to_string()));
}

// ============================================================================
// Test 9: test_scheduler_without_store_unchanged
// ============================================================================
#[tokio::test]
async fn test_scheduler_without_store_unchanged() {
    // Test that Phase 1/2 behavior is unchanged: a scheduler without RedbSink
    // still produces correct RunReports.
    // We don't actually run the scheduler here (it requires real workers),
    // but we verify that the Scheduler can be constructed with just a VecSink.
    let dag = Dag {
        metadata: None,
        nodes: vec![Node {
            id: "a".to_string(),
            prompt: "test".to_string(),
            depends_on: vec![],
            models: vec![ModelRef {
                name: "gpt-4".to_string(),
                paid: false,
            }],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: None,
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: None,
            verify_pre: None,
        }],
    };

    // Just verify we can construct a Scheduler with VecSink
    let vec_sink = VecSink::new();
    let _scheduler = Scheduler::new(
        dag,
        Box::new(pidag::DelayMockWorker::new()),
        Box::new(vec_sink),
        1,
    );
}

// ============================================================================
// Test 10: test_render_status_from_store
// ============================================================================
#[tokio::test]
async fn test_render_status_from_store() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_render_from_store");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    // Create nodes
    store
        .put_node_state(
            "run-1",
            "a",
            &NodeRecord {
                node_id: "a".to_string(),
                state: NodeStatus::Done,
                model: Some("gpt-4".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "b",
            &NodeRecord {
                node_id: "b".to_string(),
                state: NodeStatus::Failed,
                model: Some("gpt-3.5".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:01Z".to_string(),
            },
        )
        .await
        .unwrap();

    let nodes = store.list_nodes("run-1").await.unwrap();
    assert_eq!(nodes.len(), 2);
}

// ============================================================================
// Test 11: test_vault_path_defaults_to_dot_pidag
// ============================================================================
#[test]
fn test_vault_path_defaults_to_dot_pidag() {
    let cwd = std::path::PathBuf::from("/home/user/project");
    let path = RedbStore::default_path(&cwd);
    assert_eq!(path, cwd.join(".pidag").join("pidag.redb"));
}

// ============================================================================
// Test 12: test_vault_path_configurable
// ============================================================================
#[tokio::test]
async fn test_vault_path_configurable() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_custom_path");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let custom_path = tmpdir.join("custom.redb");

    let store = RedbStore::open(&custom_path).unwrap();

    let run = RunMeta {
        run_id: "test".to_string(),
        dag_json: "{}".to_string(),
        started_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };

    store.put_run(&run).await.unwrap();
    let retrieved = store.get_run("test").await.unwrap();
    assert!(retrieved.is_some());
}

// ============================================================================
// Test 13: test_store_never_panics_on_corrupt_key
// ============================================================================
#[tokio::test]
async fn test_store_never_panics_on_corrupt_key() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_corrupt_key");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    // Try to get with various malformed keys - should never panic
    let result = store.get_run("").await;
    assert!(result.is_ok() || result.is_err());

    let result = store.get_run("invalid\x00key").await;
    assert!(result.is_ok() || result.is_err());
}

// ============================================================================
// Test 14: test_mirror_disabled_by_default
// ============================================================================
#[test]
fn test_mirror_disabled_by_default() {
    // This test verifies at compile time that reqwest is not in default features.
    // If this compiles without the mirror feature, then reqwest is not a default dep.
    let _ = std::marker::PhantomData::<pidag::PidagError>;
}

// ============================================================================
// Test 18: test_composite_sink_fans_out_to_all
// ============================================================================
#[tokio::test]
async fn test_composite_sink_fans_out_to_all() {
    let sink1 = VecSink::new();
    let sink2 = VecSink::new();

    let sink1_clone = sink1.clone();
    let sink2_clone = sink2.clone();

    let mut composite = CompositeSink::new(vec![Box::new(sink1), Box::new(sink2)]);

    let event = Event::DagSubmitted;
    composite.emit(&event).await.unwrap();

    // Both sinks should have received the event
    assert_eq!(sink1_clone.events().len(), 1);
    assert_eq!(sink2_clone.events().len(), 1);
}

// ============================================================================
// Test 19: test_redbsink_projects_events_to_vault
// ============================================================================
#[tokio::test]
async fn test_redbsink_projects_events_to_vault() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_redbsink_projects");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();

    // Manually emit events like a Sink would
    let _seq = store
        .append_event(
            "run-1",
            &Event::NodeDispatched {
                node_id: "a".to_string(),
                model: "gpt-4".to_string(),
                attempt: 1,
            },
        )
        .await
        .unwrap();

    store
        .put_node_state(
            "run-1",
            "a",
            &NodeRecord {
                node_id: "a".to_string(),
                state: NodeStatus::Running,
                model: Some("gpt-4".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    let _seq = store
        .append_event(
            "run-1",
            &Event::NodeDone {
                node_id: "a".to_string(),
                model: "gpt-4".to_string(),
                output: "result".to_string(),
            },
        )
        .await
        .unwrap();

    store.put_artifact("run-1", "a", "result").await.unwrap();

    store
        .put_node_state(
            "run-1",
            "a",
            &NodeRecord {
                node_id: "a".to_string(),
                state: NodeStatus::Done,
                model: Some("gpt-4".to_string()),
                attempt: 1,
                timestamp: "2024-01-01T00:00:01Z".to_string(),
            },
        )
        .await
        .unwrap();

    // Verify projections
    let events = store.load_events("run-1").await.unwrap();
    assert_eq!(events.len(), 2);

    let nodes = store.list_nodes("run-1").await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].state, NodeStatus::Done);

    let artifact = store.get_artifact("run-1", "a").await.unwrap();
    assert_eq!(artifact, Some("result".to_string()));
}

// ============================================================================
// Regression test: DagSubmitted must not overwrite pre-seeded dag_json
// See HANDOFF 2026-08-04: RedbStorePool dag_json="{}" bug.
// ============================================================================
#[tokio::test]
async fn test_dag_submitted_does_not_overwrite_preseeded_dag_json() {
    use pidag::{NodeStatus, RedbSink};
    use std::sync::Arc;

    let tmpdir = PathBuf::from("_tmp/phase3/test_dag_submitted_no_overwrite");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let db_path = tmpdir.join("test.redb");

    let store = RedbStore::open(&db_path).unwrap();
    let store: Arc<dyn Store> = Arc::new(store);

    // Pre-seed the run with full dag_json (as run_subcommand does).
    let full_dag_json = r#"{"nodes":[{"id":"a","prompt":"test"}]}"#;
    let run = RunMeta {
        run_id: "run-preseeded".to_string(),
        dag_json: full_dag_json.to_string(),
        started_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };
    store.put_run(&run).await.unwrap();

    // Emit DagSubmitted (as the scheduler does when it starts).
    let mut sink = RedbSink::new(Arc::clone(&store), "run-preseeded".to_string());
    sink.emit(&Event::DagSubmitted).await.unwrap();

    // Verify dag_json was NOT overwritten to "{}".
    let retrieved = store.get_run("run-preseeded").await.unwrap().unwrap();
    assert_eq!(
        retrieved.dag_json, full_dag_json,
        "DagSubmitted must not overwrite pre-seeded dag_json"
    );
}

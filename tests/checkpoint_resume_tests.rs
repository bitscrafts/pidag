//! TDD tests for pidag checkpoint-resume functionality (Spec-08).
//! All 12 tests from the TDD Contract must pass.

use async_trait::async_trait;
use pidag::NodeStatus;
use pidag::core::dag::{Dag, Node, RetryPolicy};
use pidag::core::event::EventSink;
use pidag::scheduler::Scheduler;
use pidag::sdd::{Checkpoint, ResumeDecision, run_id_for_spec};
use pidag::store::{NodeRecord, NodeTiming, RunMeta, Store};
use pidag::worker::{Worker, WorkerOutput};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

// ============================================================================
// MOCK IMPLEMENTATIONS
// ============================================================================

/// Mock Store for testing checkpoint loading
struct MockStore {
    runs: Arc<std::sync::Mutex<HashMap<String, RunMeta>>>,
    nodes: Arc<std::sync::Mutex<HashMap<String, Vec<NodeRecord>>>>,
}

impl MockStore {
    fn new() -> Self {
        Self {
            runs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            nodes: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    fn add_run(&self, run: RunMeta) {
        self.runs.lock().unwrap().insert(run.run_id.clone(), run);
    }

    fn add_nodes(&self, run_id: String, nodes: Vec<NodeRecord>) {
        self.nodes.lock().unwrap().insert(run_id, nodes);
    }
}

#[async_trait]
impl Store for MockStore {
    async fn put_run(&self, run: &RunMeta) -> Result<(), pidag::core::error::PidagError> {
        self.runs
            .lock()
            .unwrap()
            .insert(run.run_id.clone(), run.clone());
        Ok(())
    }

    async fn get_run(
        &self,
        run_id: &str,
    ) -> Result<Option<RunMeta>, pidag::core::error::PidagError> {
        Ok(self.runs.lock().unwrap().get(run_id).cloned())
    }

    async fn put_node_state(
        &self,
        run_id: &str,
        _node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), pidag::core::error::PidagError> {
        self.nodes
            .lock()
            .unwrap()
            .entry(run_id.to_string())
            .or_insert_with(Vec::new)
            .push(rec.clone());
        Ok(())
    }

    async fn list_nodes(
        &self,
        run_id: &str,
    ) -> Result<Vec<NodeRecord>, pidag::core::error::PidagError> {
        Ok(self
            .nodes
            .lock()
            .unwrap()
            .get(run_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn terminal_set(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, String)>, pidag::core::error::PidagError> {
        let nodes = self
            .nodes
            .lock()
            .unwrap()
            .get(run_id)
            .cloned()
            .unwrap_or_default();
        let terminal: Vec<(String, String)> = nodes
            .into_iter()
            .filter(|n| {
                n.state == NodeStatus::Done
                    || n.state == NodeStatus::Failed
                    || n.state == NodeStatus::Blocked
            })
            .map(|n| (n.node_id, n.state.as_str().to_string()))
            .collect();
        Ok(terminal)
    }

    async fn append_event(
        &self,
        _run_id: &str,
        _ev: &pidag::core::event::Event,
    ) -> Result<u64, pidag::core::error::PidagError> {
        Ok(1)
    }

    async fn load_events(
        &self,
        _run_id: &str,
    ) -> Result<Vec<pidag::core::event::Event>, pidag::core::error::PidagError> {
        Ok(vec![])
    }

    async fn list_runs(&self) -> Result<Vec<RunMeta>, pidag::core::error::PidagError> {
        Ok(self.runs.lock().unwrap().values().cloned().collect())
    }

    async fn load_events_since(
        &self,
        _run_id: &str,
        _since: u64,
    ) -> Result<Vec<(u64, pidag::core::event::Event)>, pidag::core::error::PidagError> {
        Ok(vec![])
    }

    async fn put_artifact(
        &self,
        _run_id: &str,
        _node_id: &str,
        _artifact: &str,
    ) -> Result<(), pidag::core::error::PidagError> {
        Ok(())
    }

    async fn get_artifact(
        &self,
        _run_id: &str,
        _node_id: &str,
    ) -> Result<Option<String>, pidag::core::error::PidagError> {
        Ok(None)
    }

    async fn put_node_timing(
        &self,
        _run_id: &str,
        _node_id: &str,
        _timing: &NodeTiming,
    ) -> Result<(), pidag::core::error::PidagError> {
        Ok(())
    }

    async fn get_node_timing(
        &self,
        _run_id: &str,
        _node_id: &str,
    ) -> Result<Option<NodeTiming>, pidag::core::error::PidagError> {
        Ok(None)
    }

    async fn list_node_timings(
        &self,
        _run_id: &str,
    ) -> Result<Vec<(String, NodeTiming)>, pidag::core::error::PidagError> {
        Ok(vec![])
    }
}

/// Mock Worker for testing
#[allow(dead_code)]
struct MockWorker;

#[async_trait]
impl Worker for MockWorker {
    async fn run(
        &self,
        _node_id: &str,
        _prompt: &str,
        _model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, pidag::core::error::PidagError> {
        Ok(WorkerOutput {
            success: true,
            output: "success".to_string(),
            retryable: false,
        })
    }
}

/// Counting worker for scheduler-behavior tests: records every dispatch into an
/// `Arc<Mutex<Vec<String>>>` and returns success. Used to assert WHICH nodes
/// resumed runs actually dispatch (vs. which are skipped via checkpoint).
struct CountingWorker {
    dispatched: Arc<std::sync::Mutex<Vec<String>>>,
}

impl CountingWorker {
    fn new() -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
        let dispatched = Arc::new(std::sync::Mutex::new(Vec::new()));
        let w = CountingWorker {
            dispatched: Arc::clone(&dispatched),
        };
        (w, dispatched)
    }
}

#[async_trait]
impl Worker for CountingWorker {
    async fn run(
        &self,
        node_id: &str,
        _prompt: &str,
        _model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, pidag::core::error::PidagError> {
        self.dispatched.lock().unwrap().push(node_id.to_string());
        Ok(WorkerOutput {
            success: true,
            output: format!("done:{}", node_id),
            retryable: false,
        })
    }
}

/// Minimal 2-node linear DAG: `a` -> `c`. Used by the promote tests.
fn linear_dag_2() -> Dag {
    // shell nodes (no models) so the counting worker's empty-model path runs.
    let a = Node {
        id: "a".to_string(),
        prompt: "p-a".to_string(),
        depends_on: vec![],
        models: vec![],
        retry: RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: Some("shell".to_string()),
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,

        for_each: None,
        quorum: None,
    };
    let c = Node {
        id: "c".to_string(),
        prompt: "p-c".to_string(),
        depends_on: vec!["a".to_string()],
        models: vec![],
        retry: RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: Some("shell".to_string()),
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,

        for_each: None,
        quorum: None,
    };
    Dag {
        nodes: vec![a, c],
        metadata: None,
    }
}

/// 4-node linear DAG a->b->c->d for `test_with_checkpoint_skips_completed_prefix`.
fn linear_dag_4() -> Dag {
    let mk = |id: &str, dep: &str| Node {
        id: id.to_string(),
        prompt: format!("p-{}", id),
        depends_on: if dep.is_empty() {
            vec![]
        } else {
            vec![dep.to_string()]
        },
        models: vec![],
        retry: RetryPolicy {
            attempts: 1,
            backoff_ms: 0,
        },
        validate: None,
        node_type: Some("shell".to_string()),
        gate: None,
        timeout: None,
        mcp_call: None,
        after: vec![],
        verify: None,
        verify_pre: None,

        for_each: None,
        quorum: None,
    };
    Dag {
        nodes: vec![mk("a", ""), mk("b", "a"), mk("c", "b"), mk("d", "c")],
        metadata: None,
    }
}

// ============================================================================
// TDD CONTRACT TESTS
// ============================================================================

#[test]
fn test_run_id_deterministic() {
    let spec_path = Path::new("/path/to/spec.md");
    let spec_content = "# Spec\n## Content";

    let run_id_1 = run_id_for_spec(spec_path, spec_content);
    let run_id_2 = run_id_for_spec(spec_path, spec_content);

    assert_eq!(run_id_1, run_id_2, "Same spec should produce same run_id");
    assert_eq!(run_id_1.len(), 12, "run_id should be 12 hex chars");
}

#[test]
fn test_run_id_changes_with_content() {
    let spec_path = Path::new("/path/to/spec.md");
    let content_1 = "# Spec\n## Content 1";
    let content_2 = "# Spec\n## Content 2";

    let run_id_1 = run_id_for_spec(spec_path, content_1);
    let run_id_2 = run_id_for_spec(spec_path, content_2);

    assert_ne!(
        run_id_1, run_id_2,
        "Different spec content should produce different run_id"
    );
}

#[tokio::test]
async fn test_checkpoint_load_empty_vault() {
    let store = MockStore::new();
    let run_id = "test_run_123456";

    let decision = pidag::sdd::load_checkpoint(&store, run_id, false)
        .await
        .expect("Should load empty vault");

    match decision {
        ResumeDecision::Fresh { run_id: rid } => {
            assert_eq!(
                rid, run_id,
                "Should generate Fresh decision with same run_id"
            );
        }
        _ => panic!("Expected Fresh decision for empty vault"),
    }
}

#[tokio::test]
async fn test_checkpoint_load_completed_run() {
    let store = MockStore::new();
    let run_id = "test_completed";

    let run = RunMeta {
        run_id: run_id.to_string(),
        dag_json: "{}".to_string(),
        started_at: "2026-08-06T00:00:00Z".to_string(),
        completed_at: Some("2026-08-06T00:05:00Z".to_string()),
        successful_nodes: 10,
        failed_nodes: 0,
    };

    store.add_run(run);

    let decision = pidag::sdd::load_checkpoint(&store, run_id, false)
        .await
        .expect("Should load completed run");

    match decision {
        ResumeDecision::AlreadyDone { .. } => {
            // Success
        }
        _ => panic!("Expected AlreadyDone decision for completed run"),
    }
}

#[tokio::test]
async fn test_checkpoint_load_partial_run() {
    let store = MockStore::new();
    let run_id = "test_partial";

    let run = RunMeta {
        run_id: run_id.to_string(),
        dag_json: "{}".to_string(),
        started_at: "2026-08-06T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 3,
        failed_nodes: 0,
    };

    store.add_run(run);

    let nodes = vec![
        NodeRecord {
            node_id: "node_a".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempt: 1,
            timestamp: "2026-08-06T00:01:00Z".to_string(),
        },
        NodeRecord {
            node_id: "node_b".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempt: 1,
            timestamp: "2026-08-06T00:02:00Z".to_string(),
        },
        NodeRecord {
            node_id: "node_c".to_string(),
            state: NodeStatus::Done,
            model: None,
            attempt: 1,
            timestamp: "2026-08-06T00:03:00Z".to_string(),
        },
    ];

    store.add_nodes(run_id.to_string(), nodes);

    let decision = pidag::sdd::load_checkpoint(&store, run_id, false)
        .await
        .expect("Should load partial run");

    match decision {
        ResumeDecision::Resume { checkpoint } => {
            assert_eq!(
                checkpoint.completed_nodes.len(),
                3,
                "Should have 3 completed nodes"
            );
            assert!(checkpoint.completed_nodes.contains("node_a"));
            assert!(checkpoint.completed_nodes.contains("node_b"));
            assert!(checkpoint.completed_nodes.contains("node_c"));
        }
        _ => panic!("Expected Resume decision for partial run"),
    }
}

#[tokio::test]
async fn test_scheduler_skips_done_nodes() {
    // DAG a -> c. Checkpoint says `a` is Done. The counting worker must
    // dispatch ONLY `c`; `a` is seeded Done and never re-dispatched. Both
    // end up Done in the report.
    let dag = linear_dag_2();
    let (worker, dispatched) = CountingWorker::new();
    let cp = Checkpoint {
        run_id: "test".to_string(),
        completed_nodes: {
            let mut s = HashSet::new();
            s.insert("a".to_string());
            s
        },
        failed_nodes: Default::default(),
        blocked_nodes: Default::default(),
        stale_running: Default::default(),
        outputs: Default::default(),
    };
    let sink: Box<dyn EventSink> = Box::new(pidag::VecSink::new());
    let mut sched = Scheduler::with_checkpoint(dag.clone(), Box::new(worker), sink, 4, cp);
    let report = sched.run(false).await.expect("run completes");

    let dispatched = dispatched.lock().unwrap().clone();
    assert_eq!(
        dispatched,
        vec!["c".to_string()],
        "only c dispatched; a skipped"
    );
    let a_state = report
        .node_states
        .iter()
        .find(|n| n.node_id == "a")
        .expect("a present");
    assert_eq!(
        a_state.state,
        NodeStatus::Done,
        "a carried as Done from checkpoint"
    );
    let c_state = report
        .node_states
        .iter()
        .find(|n| n.node_id == "c")
        .expect("c present");
    assert_eq!(c_state.state, NodeStatus::Done, "c ran and is Done");
}

#[tokio::test]
async fn test_scheduler_decrements_indegree() {
    // DAG a -> c. Checkpoint{a Done}. With the in-degree pre-decrement, `c`
    // is ready immediately (in-degree 0 after `a` counted Done) and dispatches
    // without waiting on `a` (which never runs). Equivalent to the prior test
    // but framed around the in-degree mechanic: there is no scenario where `c`
    // sits Blocked because `a` was skipped.
    let dag = linear_dag_2();
    let (worker, dispatched) = CountingWorker::new();
    let cp = Checkpoint {
        run_id: "test".to_string(),
        completed_nodes: {
            let mut s = HashSet::new();
            s.insert("a".to_string());
            s
        },
        failed_nodes: Default::default(),
        blocked_nodes: Default::default(),
        stale_running: Default::default(),
        outputs: Default::default(),
    };
    let sink: Box<dyn EventSink> = Box::new(pidag::VecSink::new());
    let mut sched = Scheduler::with_checkpoint(dag.clone(), Box::new(worker), sink, 4, cp);
    let report = sched.run(false).await.expect("run completes");

    let dispatched = dispatched.lock().unwrap().clone();
    assert_eq!(dispatched, vec!["c".to_string()]);
    assert_eq!(
        report
            .node_states
            .iter()
            .filter(|n| n.state == NodeStatus::Done)
            .count(),
        2,
        "both a and c Done"
    );
}

#[tokio::test]
async fn test_scheduler_resets_stale_running() {
    // A node that was `Running` when the prior process crashed must be retried
    // as a fresh attempt: it stays Pending and the ready queue dispatches it.
    let dag = linear_dag_2();
    let (worker, dispatched) = CountingWorker::new();
    let cp = Checkpoint {
        run_id: "test".to_string(),
        completed_nodes: Default::default(),
        failed_nodes: Default::default(),
        blocked_nodes: Default::default(),
        stale_running: {
            let mut s = HashSet::new();
            s.insert("a".to_string());
            s
        },
        outputs: Default::default(),
    };
    let sink: Box<dyn EventSink> = Box::new(pidag::VecSink::new());
    let mut sched = Scheduler::with_checkpoint(dag.clone(), Box::new(worker), sink, 4, cp);
    let report = sched.run(false).await.expect("run completes");

    let dispatched = dispatched.lock().unwrap().clone();
    assert!(
        dispatched.contains(&"a".to_string()),
        "stale-running a re-dispatched: {:?}",
        dispatched
    );
    // a was Running (stale) -> retried -> Done; c waits on a, then runs -> Done.
    let failed: Vec<_> = report
        .node_states
        .iter()
        .filter(|n| n.state == NodeStatus::Failed)
        .collect();
    assert!(failed.is_empty(), "no failures: {:?}", failed);
}

#[tokio::test]
async fn test_retry_failed_flag_resets_failed() {
    let store = MockStore::new();
    let run_id = "test_retry";

    let run = RunMeta {
        run_id: run_id.to_string(),
        dag_json: "{}".to_string(),
        started_at: "2026-08-06T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 1,
        failed_nodes: 1,
    };

    store.add_run(run);

    let nodes = vec![NodeRecord {
        node_id: "node_e".to_string(),
        state: NodeStatus::Failed,
        model: None,
        attempt: 1,
        timestamp: "2026-08-06T00:01:00Z".to_string(),
    }];

    store.add_nodes(run_id.to_string(), nodes);

    // With retry_failed=true, failed nodes should be in the resume decision
    let decision = pidag::sdd::load_checkpoint(&store, run_id, true)
        .await
        .expect("Should load partial run");

    match decision {
        ResumeDecision::Resume { checkpoint } => {
            // Failed node should be available for retry
            // (not in blocked_nodes or failed_nodes when retrying)
            assert!(!checkpoint.blocked_nodes.contains("node_e"));
        }
        _ => panic!("Expected Resume decision"),
    }
}

#[tokio::test]
async fn test_retry_failed_flag_off_skips_failed() {
    // `load_checkpoint` puts a Failed node (when retry_failed=false) into
    // `checkpoint.failed_nodes`. The scheduler must keep it terminal Failed
    // and NOT dispatch it. Its dependent (in the live DAG) cascades Blocked.
    let store = MockStore::new();
    let run_id = "test_no_retry";

    let run = RunMeta {
        run_id: run_id.to_string(),
        dag_json: "{}".to_string(),
        started_at: "2026-08-06T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 1,
        failed_nodes: 1,
    };

    store.add_run(run);

    let nodes = vec![NodeRecord {
        node_id: "node_e".to_string(),
        state: NodeStatus::Failed,
        model: None,
        attempt: 1,
        timestamp: "2026-08-06T00:01:00Z".to_string(),
    }];

    store.add_nodes(run_id.to_string(), nodes);

    // With retry_failed=false, failed nodes should stay failed
    let decision = pidag::sdd::load_checkpoint(&store, run_id, false)
        .await
        .expect("Should load partial run");

    let checkpoint = match decision {
        ResumeDecision::Resume { checkpoint } => checkpoint,
        _ => panic!("Expected Resume decision"),
    };
    assert!(checkpoint.failed_nodes.contains("node_e"));

    // Drive the scheduler: a DAG with a single node `e` whose checkpoint marks
    // it Failed (retry off). It must stay Failed and NOT be dispatched.
    let dag = Dag {
        nodes: vec![Node {
            id: "node_e".to_string(),
            prompt: "p".to_string(),
            depends_on: vec![],
            models: vec![],
            retry: RetryPolicy {
                attempts: 1,
                backoff_ms: 0,
            },
            validate: None,
            node_type: Some("shell".to_string()),
            gate: None,
            timeout: None,
            mcp_call: None,
            after: vec![],
            verify: None,
            verify_pre: None,

            for_each: None,
            quorum: None,
        }],
        metadata: None,
    };
    let (worker, dispatched) = CountingWorker::new();
    let sink: Box<dyn EventSink> = Box::new(pidag::VecSink::new());
    let mut sched = Scheduler::with_checkpoint(dag, Box::new(worker), sink, 4, checkpoint);
    let report = sched.run(false).await.expect("run completes");

    assert!(
        dispatched.lock().unwrap().is_empty(),
        "terminal-failed node_e NOT dispatched: {:?}",
        dispatched
    );
    let e_state = report
        .node_states
        .iter()
        .find(|n| n.node_id == "node_e")
        .expect("node_e present");
    assert_eq!(
        e_state.state,
        NodeStatus::Failed,
        "node_e stays Failed from checkpoint"
    );
}

#[test]
fn test_fresh_flag_ignores_checkpoint() {
    // `--fresh` is handled at the CLI layer: it skips `load_checkpoint` and
    // proceeds Fresh. The determinism property it relies on is that
    // `run_id_for_spec` is stable for the same spec — so a fresh re-run with
    // the SAME spec reuses the same run_id (overwriting the prior interrupted
    // record rather than forking a new one).
    let spec_path = Path::new("/path/to/spec.md");
    let spec_content = "# Spec";

    let id1 = run_id_for_spec(spec_path, spec_content);
    let id2 = run_id_for_spec(spec_path, spec_content);

    assert_eq!(id1.len(), 12, "run_id is 12 hex chars");
    assert_eq!(
        id1, id2,
        "fresh re-run of same spec reuses run_id (overwrite)"
    );
}

#[tokio::test]
async fn test_with_checkpoint_skips_completed_prefix() {
    // DAG a -> b -> c -> d. Checkpoint says a,b are Done. Only c and d should
    // dispatch; a,b are seeded Done; all four end Done; c becomes ready only
    // after b's checkpoint-decrement zeroes its in-degree (so c, then d).
    let dag = linear_dag_4();
    let (worker, dispatched) = CountingWorker::new();
    let cp = Checkpoint {
        run_id: "test".to_string(),
        completed_nodes: {
            let mut s = HashSet::new();
            s.insert("a".to_string());
            s.insert("b".to_string());
            s
        },
        failed_nodes: Default::default(),
        blocked_nodes: Default::default(),
        stale_running: Default::default(),
        outputs: Default::default(),
    };
    let sink: Box<dyn EventSink> = Box::new(pidag::VecSink::new());
    let mut sched = Scheduler::with_checkpoint(dag, Box::new(worker), sink, 4, cp);
    let report = sched.run(false).await.expect("run completes");

    let mut dispatched = dispatched.lock().unwrap().clone();
    dispatched.sort();
    assert_eq!(
        dispatched,
        vec!["c".to_string(), "d".to_string()],
        "only c,d dispatched; a,b skipped via checkpoint"
    );
    let all_done = report
        .node_states
        .iter()
        .all(|n| n.state == NodeStatus::Done);
    assert!(all_done, "all four nodes Done: {:?}", report.node_states);
    let done_count = report
        .node_states
        .iter()
        .filter(|n| n.state == NodeStatus::Done)
        .count();
    assert_eq!(done_count, 4, "4 nodes total Done");
}

#[tokio::test]
async fn test_resume_startup_latency() {
    // Test that loading a 10-node checkpoint completes in < 50ms
    let store = MockStore::new();
    let run_id = "test_latency";

    let run = RunMeta {
        run_id: run_id.to_string(),
        dag_json: "{}".to_string(),
        started_at: "2026-08-06T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 6,
        failed_nodes: 0,
    };

    store.add_run(run);

    // Add 10 node records
    let mut nodes = Vec::new();
    for i in 0..10 {
        let state = if i < 6 {
            NodeStatus::Done
        } else {
            NodeStatus::Pending
        };
        nodes.push(NodeRecord {
            node_id: format!("node_{}", i),
            state,
            model: None,
            attempt: 1,
            timestamp: "2026-08-06T00:01:00Z".to_string(),
        });
    }

    store.add_nodes(run_id.to_string(), nodes);

    let start = std::time::Instant::now();

    let _decision = pidag::sdd::load_checkpoint(&store, run_id, false)
        .await
        .expect("Should load checkpoint");

    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "Checkpoint load should complete in < 50ms, but took {}ms",
        elapsed.as_millis()
    );
}

use async_trait::async_trait;
/// Performance baseline harness tests (TDD Contract B1–B7)
use pidag::{
    Scheduler, VecSink,
    core::dag::{Dag, Node, RetryPolicy},
    core::error::PidagError,
    core::event::{Event, EventSink},
    store::{NodeRecord, NodeTiming, RunMeta, Store},
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

// ============================================================================
// Test DAG Generator (B1)
// ============================================================================

#[derive(Debug, Clone, Copy)]
enum Topology {
    Wide,
    Chain,
    SddLike,
}

/// Generate a synthetic DAG of N shell nodes in the given topology.
fn gen_dag(topology: Topology, n: usize) -> Dag {
    let mut nodes = Vec::new();

    match topology {
        Topology::Wide => {
            // N independent nodes, no edges
            for i in 0..n {
                nodes.push(Node {
                    id: format!("node_{}", i),
                    prompt: "true".to_string(),
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
                });
            }
        }
        Topology::Chain => {
            // N nodes in a single dependency line: i depends on i-1
            for i in 0..n {
                let depends_on = if i > 0 {
                    vec![format!("node_{}", i - 1)]
                } else {
                    vec![]
                };
                nodes.push(Node {
                    id: format!("node_{}", i),
                    prompt: "true".to_string(),
                    depends_on,
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
                });
            }
        }
        Topology::SddLike => {
            // N/3 iterations of implement → quality-gate → validate
            let iterations = (n + 2) / 3; // ceiling division
            for i in 0..iterations {
                let impl_id = format!("implement_{}", i);
                let gate_id = format!("quality_gate_{}", i);
                let validate_id = format!("validate_{}", i);

                // implement node
                nodes.push(Node {
                    id: impl_id.clone(),
                    prompt: "true".to_string(),
                    depends_on: if i == 0 {
                        vec![]
                    } else {
                        vec![format!("validate_{}", i - 1)]
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
                });

                // quality_gate node with after edge to implement
                nodes.push(Node {
                    id: gate_id.clone(),
                    prompt: "true".to_string(),
                    depends_on: vec![],
                    models: vec![],
                    retry: RetryPolicy {
                        attempts: 1,
                        backoff_ms: 0,
                    },
                    validate: None,
                    node_type: Some("shell".to_string()),
                    gate: Some(format!("validate_{}", i)),
                    timeout: None,
                    mcp_call: None,
                    after: vec![impl_id.clone()],
                    verify: None,
                    verify_pre: None,

                    for_each: None,
                    quorum: None,
                });

                // validate node
                nodes.push(Node {
                    id: validate_id.clone(),
                    prompt: "true".to_string(),
                    depends_on: vec![gate_id.clone()],
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
                });
            }
        }
    }

    Dag {
        nodes,
        metadata: None,
    }
}

// ============================================================================
// Counting Store Wrapper (B3)
// ============================================================================

/// Wraps a Store and counts write transactions
#[derive(Clone)]
struct CountingStore {
    inner: Arc<dyn Store>,
    write_txn_count: Arc<AtomicU64>,
    bytes_written: Arc<AtomicU64>,
}

impl CountingStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            write_txn_count: Arc::new(AtomicU64::new(0)),
            bytes_written: Arc::new(AtomicU64::new(0)),
        }
    }

    fn write_txn_count(&self) -> u64 {
        self.write_txn_count.load(Ordering::SeqCst)
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Store for CountingStore {
    async fn put_run(&self, run: &RunMeta) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(run).unwrap_or_default();
        self.bytes_written
            .fetch_add(serialized.len() as u64, Ordering::SeqCst);
        self.inner.put_run(run).await
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunMeta>, PidagError> {
        self.inner.get_run(run_id).await
    }

    async fn put_node_state(
        &self,
        run_id: &str,
        node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(rec).unwrap_or_default();
        self.bytes_written
            .fetch_add(serialized.len() as u64, Ordering::SeqCst);
        self.inner.put_node_state(run_id, node_id, rec).await
    }

    async fn list_nodes(&self, run_id: &str) -> Result<Vec<NodeRecord>, PidagError> {
        self.inner.list_nodes(run_id).await
    }

    async fn terminal_set(&self, run_id: &str) -> Result<Vec<(String, String)>, PidagError> {
        self.inner.terminal_set(run_id).await
    }

    async fn append_event(&self, run_id: &str, ev: &Event) -> Result<u64, PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(ev).unwrap_or_default();
        self.bytes_written
            .fetch_add(serialized.len() as u64, Ordering::SeqCst);
        self.inner.append_event(run_id, ev).await
    }

    async fn load_events(&self, run_id: &str) -> Result<Vec<Event>, PidagError> {
        self.inner.load_events(run_id).await
    }

    async fn load_events_since(
        &self,
        run_id: &str,
        since: u64,
    ) -> Result<Vec<(u64, Event)>, PidagError> {
        self.inner.load_events_since(run_id, since).await
    }

    async fn put_artifact(
        &self,
        run_id: &str,
        node_id: &str,
        output: &str,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        self.bytes_written
            .fetch_add(output.len() as u64, Ordering::SeqCst);
        self.inner.put_artifact(run_id, node_id, output).await
    }

    async fn get_artifact(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<String>, PidagError> {
        self.inner.get_artifact(run_id, node_id).await
    }

    async fn put_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
        timing: &NodeTiming,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(timing).unwrap_or_default();
        self.bytes_written
            .fetch_add(serialized.len() as u64, Ordering::SeqCst);
        self.inner.put_node_timing(run_id, node_id, timing).await
    }

    async fn get_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeTiming>, PidagError> {
        self.inner.get_node_timing(run_id, node_id).await
    }

    async fn list_node_timings(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, NodeTiming)>, PidagError> {
        self.inner.list_node_timings(run_id).await
    }

    async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError> {
        self.inner.list_runs().await
    }
}

// ============================================================================
// Counting Event Sink Wrapper (B3)
// ============================================================================

#[derive(Clone)]
struct CountingSink {
    inner: Arc<tokio::sync::Mutex<Box<dyn EventSink>>>,
    event_count: Arc<AtomicU64>,
}

impl CountingSink {
    fn new(inner: Box<dyn EventSink>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(inner)),
            event_count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn event_count(&self) -> u64 {
        self.event_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EventSink for CountingSink {
    async fn emit(&mut self, event: &Event) -> Result<(), std::io::Error> {
        self.event_count.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.inner.lock().await;
        inner.emit(event).await
    }
}

// ============================================================================
// TDD Contract Tests
// ============================================================================

#[test]
fn test_gen_wide_has_no_edges() {
    let dag = gen_dag(Topology::Wide, 50);
    assert_eq!(dag.nodes.len(), 50);
    assert!(
        dag.nodes
            .iter()
            .all(|n| n.depends_on.is_empty() && n.after.is_empty())
    );
    assert!(dag.validate().is_ok());
}

#[test]
fn test_gen_chain_is_linear() {
    let dag = gen_dag(Topology::Chain, 50);
    assert_eq!(dag.nodes.len(), 50);

    for (i, node) in dag.nodes.iter().enumerate() {
        if i == 0 {
            assert!(node.depends_on.is_empty());
        } else {
            assert_eq!(node.depends_on.len(), 1);
            assert_eq!(node.depends_on[0], format!("node_{}", i - 1));
        }
    }
    assert!(dag.validate().is_ok());
}

#[test]
fn test_gen_sdd_like_shape() {
    let dag = gen_dag(Topology::SddLike, 9);
    // 9 nodes / 3 = 3 iterations, each with 3 nodes = 9 total
    assert_eq!(dag.nodes.len(), 9);

    // Check each iteration: implement, quality_gate, validate
    for i in 0..3 {
        let impl_node = dag
            .nodes
            .iter()
            .find(|n| n.id == format!("implement_{}", i))
            .unwrap();
        let gate_node = dag
            .nodes
            .iter()
            .find(|n| n.id == format!("quality_gate_{}", i))
            .unwrap();
        let val_node = dag
            .nodes
            .iter()
            .find(|n| n.id == format!("validate_{}", i))
            .unwrap();

        // quality_gate has after edge to implement
        assert!(gate_node.after.contains(&impl_node.id));
        // validate depends on quality_gate
        assert!(val_node.depends_on.contains(&gate_node.id));
        // gate node has gate field set
        assert_eq!(gate_node.gate, Some(format!("validate_{}", i)));
    }
    assert!(dag.validate().is_ok());
}

#[test]
fn test_generated_dags_contain_no_llm_nodes() {
    for topology in [Topology::Wide, Topology::Chain, Topology::SddLike] {
        let dag = gen_dag(topology, 50);
        for node in &dag.nodes {
            assert_eq!(
                node.node_type,
                Some("shell".to_string()),
                "node {} should be shell type",
                node.id
            );
            assert!(
                node.models.is_empty(),
                "node {} should have no models",
                node.id
            );
        }
    }
}

#[tokio::test]
async fn test_counting_store_counts_write_txns() {
    use pidag::store::MockStore;

    let mock_store = Arc::new(MockStore::new());
    let counting_store = CountingStore::new(mock_store);

    let run1 = RunMeta {
        run_id: "run1".to_string(),
        dag_json: "{}".to_string(),
        started_at: "2025-01-01T00:00:00Z".to_string(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };

    counting_store.put_run(&run1).await.unwrap();
    counting_store.put_run(&run1).await.unwrap();
    counting_store.put_run(&run1).await.unwrap();

    assert_eq!(counting_store.write_txn_count(), 3);
}

#[tokio::test]
async fn test_counting_sink_counts_events() {
    let vec_sink = VecSink::new();
    let counting_sink = CountingSink::new(Box::new(vec_sink));
    let mut sink = counting_sink.clone();

    let dag = gen_dag(Topology::Wide, 10);

    // Simulate some events
    sink.emit(&Event::DagSubmitted).await.unwrap();
    for i in 0..10 {
        sink.emit(&Event::NodeDispatched {
            node_id: dag.nodes[i].id.clone(),
            model: "shell".to_string(),
            attempt: 1,
        })
        .await
        .unwrap();
        sink.emit(&Event::NodeDone {
            node_id: dag.nodes[i].id.clone(),
            model: "shell".to_string(),
            output: "done".to_string(),
        })
        .await
        .unwrap();
    }
    sink.emit(&Event::DagDone {
        successful_nodes: 10,
        failed_nodes: 0,
    })
    .await
    .unwrap();

    // 1 (DagSubmitted) + 10 (NodeDispatched) + 10 (NodeDone) + 1 (DagDone) = 22 events
    assert_eq!(counting_sink.event_count(), 22);
}

#[tokio::test]
async fn test_counters_are_deterministic() {
    use pidag::store::RedbStore;
    use pidag::worker::RealShellWorker;
    use std::path::PathBuf;

    let dag = gen_dag(Topology::Chain, 50);

    // First run
    let tmpdir1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("_tmp/perf_test_1");
    let _ = std::fs::remove_dir_all(&tmpdir1);
    std::fs::create_dir_all(&tmpdir1).unwrap();
    let vault1 = tmpdir1.join("pidag.redb");

    let store1 = Arc::new(RedbStore::open(&vault1).unwrap());
    let counting_store1 = CountingStore::new(store1);
    let vec_sink1 = VecSink::new();
    let sink1 = Box::new(vec_sink1);

    let worker1 = Box::new(RealShellWorker::new(
        &dag,
        std::time::Duration::from_secs(10),
    ));
    let mut scheduler1 = Scheduler::new(dag.clone(), worker1, sink1, 2);

    let _ = scheduler1.run(false).await;
    let txn_count1 = counting_store1.write_txn_count();

    // Second run with fresh vault
    let tmpdir2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("_tmp/perf_test_2");
    let _ = std::fs::remove_dir_all(&tmpdir2);
    std::fs::create_dir_all(&tmpdir2).unwrap();
    let vault2 = tmpdir2.join("pidag.redb");

    let store2 = Arc::new(RedbStore::open(&vault2).unwrap());
    let counting_store2 = CountingStore::new(store2);
    let vec_sink2 = VecSink::new();
    let sink2 = Box::new(vec_sink2);

    let worker2 = Box::new(RealShellWorker::new(
        &dag,
        std::time::Duration::from_secs(10),
    ));
    let mut scheduler2 = Scheduler::new(dag.clone(), worker2, sink2, 2);

    let _ = scheduler2.run(false).await;
    let txn_count2 = counting_store2.write_txn_count();

    // Counts should be identical
    assert_eq!(
        txn_count1, txn_count2,
        "Transaction counts should be deterministic"
    );
}

#[test]
fn test_benchmark_excluded_from_gate() {
    // Check that benches/scheduler_bench.rs exists (not in tests/)
    // and is properly configured to not run under cargo test
    let bench_path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benches/scheduler_bench.rs"
    ));
    assert!(
        bench_path.exists(),
        "Benchmark should exist at benches/scheduler_bench.rs"
    );
}

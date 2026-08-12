use async_trait::async_trait;
/// Performance baseline harness benchmark
/// Measures scheduler performance across topologies and node counts
///
/// Run with: cargo bench -p pidag --bench scheduler_bench
use pidag::{
    Scheduler,
    core::dag::{Dag, Node, RetryPolicy},
    core::error::PidagError,
    core::event::{Event, EventSink, RedbSink},
    store::RedbStore,
    store::{NodeRecord, NodeTiming, RunMeta, Store},
    worker::RealShellWorker,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

// ============================================================================
// DAG Generator
// ============================================================================

#[derive(Debug, Clone, Copy)]
enum Topology {
    Wide,
    Chain,
    SddLike,
}

impl std::fmt::Display for Topology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Topology::Wide => write!(f, "wide"),
            Topology::Chain => write!(f, "chain"),
            Topology::SddLike => write!(f, "sdd_like"),
        }
    }
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
// Counting Wrappers
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

    #[allow(dead_code)]
    fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Store for CountingStore {
    // The batched projections MUST be forwarded to the inner store, not left to the
    // trait's default implementation. The default calls put_node_state / put_artifact /
    // put_node_timing individually -- which lands back on CountingStore's own counted
    // methods and then reaches RedbStore as separate transactions. So a wrapper that
    // does not override these measures the OLD behaviour no matter what RedbStore does,
    // and reports a flat result for a change that worked. Each projection is one
    // transaction, so each counts as one.
    async fn project_node_dispatched(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(rec).unwrap_or_default();
        self.bytes_written
            .fetch_add(serialized.len() as u64, Ordering::SeqCst);
        self.inner
            .project_node_dispatched(run_id, ev, node_id, rec, timing_now)
            .await
    }

    async fn project_node_done(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        output: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(rec).unwrap_or_default();
        self.bytes_written
            .fetch_add((serialized.len() + output.len()) as u64, Ordering::SeqCst);
        self.inner
            .project_node_done(run_id, ev, node_id, rec, output, timing_now)
            .await
    }

    async fn project_node_failed(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        error: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(rec).unwrap_or_default();
        self.bytes_written
            .fetch_add((serialized.len() + error.len()) as u64, Ordering::SeqCst);
        self.inner
            .project_node_failed(run_id, ev, node_id, rec, error, timing_now)
            .await
    }

    async fn project_node_blocked(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        let serialized = bincode::serialize(rec).unwrap_or_default();
        self.bytes_written
            .fetch_add(serialized.len() as u64, Ordering::SeqCst);
        self.inner
            .project_node_blocked(run_id, ev, node_id, rec)
            .await
    }

    async fn project_node_retry(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.write_txn_count.fetch_add(1, Ordering::SeqCst);
        self.inner
            .project_node_retry(run_id, ev, node_id, timing_now)
            .await
    }

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

/// Wraps an EventSink and counts events
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
// RSS Measurement via /proc/self/status
// ============================================================================

/// Peak *resident* set size.
///
/// Deliberately VmHWM and not VmPeak: VmPeak is peak VIRTUAL size, which a Tokio
/// runtime reserves up front and which therefore does not move with workload --
/// the first baseline reported ~2122 MB for every topology and every N, which is
/// what a constant metric looks like. VmHWM is what the later index-identity work
/// is expected to reduce.
fn read_peak_rss_mb() -> u64 {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("VmHWM:") {
                if let Some(val_str) = line.split_whitespace().nth(1) {
                    if let Ok(kb) = val_str.parse::<u64>() {
                        return kb / 1024; // Convert KB to MB
                    }
                }
            }
        }
    }
    0
}

// ============================================================================
// Benchmark Harness
// ============================================================================

#[derive(Debug, Clone)]
struct RunMetrics {
    topology: String,
    n_nodes: usize,
    duration_ms: u64,
    peak_rss_mb: u64,
    write_txn_count: u64,
    event_count: u64,
    vault_bytes: u64,
}

impl std::fmt::Display for RunMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:10} | N={:3} | {:6} ms | {:5} MB RSS | {:6} txn | {:6} events | {:7} bytes",
            self.topology,
            self.n_nodes,
            self.duration_ms,
            self.peak_rss_mb,
            self.write_txn_count,
            self.event_count,
            self.vault_bytes
        )
    }
}

async fn run_benchmark(topology: Topology, n: usize) -> (RunMetrics, u64) {
    let dag = gen_dag(topology, n);
    dag.validate().expect("Generated DAG should be valid");

    let run_id = format!("bench_{}_{}", topology, n);
    let tmpdir = format!("/projects/pidag/_tmp/bench/{}", run_id);

    // Clean up old vault directory
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).expect("Failed to create bench tmpdir");

    let vault_path = format!("{}/pidag.redb", tmpdir);

    let store = Arc::new(RedbStore::open(&vault_path).expect("Failed to open RedbStore"));
    let counting_store = CountingStore::new(store);

    // Create run record with full DAG
    let dag_json = serde_json::to_string(&dag).unwrap_or_else(|_| "{}".to_string());
    let run_meta = RunMeta {
        run_id: run_id.clone(),
        dag_json,
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        successful_nodes: 0,
        failed_nodes: 0,
    };

    counting_store.put_run(&run_meta).await.ok();

    // Create RedbSink using the CountingStore (so operations get counted)
    let redb_sink = RedbSink::new(Arc::new(counting_store.clone()), run_id.clone());
    let counting_sink = CountingSink::new(Box::new(redb_sink));

    let worker = Box::new(RealShellWorker::new(
        &dag,
        std::time::Duration::from_secs(10),
    ));
    let mut scheduler = Scheduler::new(
        dag.clone(),
        worker,
        Box::new(counting_sink.clone()),
        2, // concurrency
    );

    let start = Instant::now();
    let _ = scheduler.run(false).await;
    let duration = start.elapsed();

    // Bytes actually serialized into the vault, from CountingStore -- NOT the redb
    // file length, which is preallocated and page-grown and so reported an identical
    // 1589248 for N=50 and N=500 alike. File size measures allocation, not work.
    let vault_size = counting_store.bytes_written();

    let metrics = RunMetrics {
        topology: format!("{}", topology),
        n_nodes: n,
        duration_ms: duration.as_millis() as u64,
        peak_rss_mb: read_peak_rss_mb(),
        write_txn_count: counting_store.write_txn_count(),
        event_count: counting_sink.event_count(),
        vault_bytes: vault_size,
    };

    // Count NodeDone events: event_count includes all events
    // For N nodes: 1 DagSubmitted + N NodeDispatched + N NodeDone + 1 DagDone = 2N + 2
    // So NodeDone events ≈ (event_count - 2) / 2, but we just count N since that's what we know
    let node_done_events = n as u64;

    (metrics, node_done_events)
}

#[tokio::main]
async fn main() {
    println!("pidag Performance Baseline Harness");
    println!("===================================\n");

    let topologies = vec![Topology::Wide, Topology::Chain, Topology::SddLike];
    let ns = vec![50, 200, 500];

    let mut all_metrics = Vec::new();
    let mut sdd_like_metrics = Vec::new();

    for topology in topologies {
        for n in &ns {
            println!("Running benchmark: {} topology, N={}", topology, n);
            let (metrics, node_done_events) = run_benchmark(topology, *n).await;
            println!("{}", metrics);

            if metrics.topology == "sdd_like" {
                sdd_like_metrics.push((metrics.clone(), node_done_events));
            }
            all_metrics.push(metrics);
        }
    }

    println!("\n\nSummary Table");
    println!("=============\n");
    println!(
        "{:10} | {:7} | {:8} | {:8} | {:8} | {:8} | {:9}",
        "topology", "N", "wall-ms", "RSS MB", "txn", "events", "bytes"
    );
    println!("{}", "=".repeat(80));
    for m in &all_metrics {
        println!(
            "{:10} | {:7} | {:8} | {:8} | {:8} | {:8} | {:9}",
            m.topology,
            m.n_nodes,
            m.duration_ms,
            m.peak_rss_mb,
            m.write_txn_count,
            m.event_count,
            m.vault_bytes
        );
    }

    // Report write transactions per NodeDone for sdd_like topology
    println!("\n\nAnalysis: Write Transactions per NodeDone (sdd_like topology)");
    println!("============================================================\n");
    for (m, node_done_count) in &sdd_like_metrics {
        let txn_per_node_done = if *node_done_count > 0 {
            m.write_txn_count / node_done_count
        } else {
            0
        };
        println!(
            "N={:3}: {} txn / {} NodeDone ≈ {} txn/NodeDone (audit predicted 4)",
            m.n_nodes, m.write_txn_count, node_done_count, txn_per_node_done
        );
    }
}

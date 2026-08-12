mod await_loop;
pub mod execute;
pub use execute::DispatchEvent;

use crate::core::dag::Dag;
use crate::core::error::PidagError;
use crate::core::event::EventSink;
use crate::worker::Worker;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Node execution status. Five states with wire-format compatibility to existing strings.
/// Copy semantics avoid clones per transition (audit P1-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum NodeStatus {
    Pending,
    Running,
    Done,
    Failed,
    Blocked,
}

impl NodeStatus {
    /// Parse a status string (for deserialization from legacy data).
    /// Returns an error if the string is not recognized.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "Pending" => Ok(NodeStatus::Pending),
            "Running" => Ok(NodeStatus::Running),
            "Done" => Ok(NodeStatus::Done),
            "Failed" => Ok(NodeStatus::Failed),
            "Blocked" => Ok(NodeStatus::Blocked),
            _ => Err(format!("unknown status: {}", s)),
        }
    }

    /// Serialize to the wire format (same as serde via rename_all).
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeStatus::Pending => "Pending",
            NodeStatus::Running => "Running",
            NodeStatus::Done => "Done",
            NodeStatus::Failed => "Failed",
            NodeStatus::Blocked => "Blocked",
        }
    }
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub node_id: String,
    pub state: NodeStatus,
    pub model: Option<String>,
    pub attempts: usize,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub node_states: Vec<NodeState>,
    pub failed: Vec<String>,
    pub store_write_failures: usize,
}

/// Checkpoint state loaded from vault on resume (Spec-08).
///
/// Canonical home is the scheduler (no `sdd -> scheduler -> sdd` cycle); the
/// `sdd::resume` module re-exports it so existing import paths keep working.
/// `completed_nodes` / `failed_nodes` / `blocked_nodes` are the terminal
/// states carried over from the prior (interrupted) run; `stale_running`
/// captures nodes that were `Running` when the process died (to be retried
/// as fresh `Pending`). `outputs` (I7) maps terminal node ids to their
/// captured outputs for hydration during interpolation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub run_id: String,
    pub completed_nodes: HashSet<String>, // state == Done
    pub failed_nodes: HashSet<String>,    // state == Failed (when retry_failed is OFF)
    pub blocked_nodes: HashSet<String>,   // state == Blocked
    pub stale_running: HashSet<String>,   // state == Running (crash-stale, reset to Pending)
    #[serde(default)]
    pub outputs: HashMap<String, String>, // terminal node id -> captured output (I7)
}

/// Authoritative, shareable snapshot of run progress. Updated on every
/// terminal node transition (Done/Failed/Blocked) and on DAG completion.
/// A `watch` channel carries this so awaiters never miss a transition:
/// each subscriber reads the *current* value first (no replay needed) and
/// only parks on `changed()` when nothing new is available yet.
#[derive(Debug, Clone, Default)]
struct Snapshot {
    node_state: HashMap<String, NodeState>,
    /// Node ids in the order they became terminal (Done/Failed/Blocked).
    terminal_order: Vec<String>,
    dag_done: bool,
    report: Option<RunReport>,
}

impl Snapshot {
    fn record_terminal(&mut self, node_id: &str, state: NodeState) {
        if !self.terminal_order.iter().any(|id| id == node_id) {
            self.terminal_order.push(node_id.to_string());
        }
        self.node_state.insert(node_id.to_string(), state);
    }
}

/// Shared, `'static`-cloneable run state: the piece of a `Scheduler` that a
/// background-spawned run task and any number of `await_dag`/`wait_any`
/// callers both need a handle to.
struct Inner {
    all_node_ids: Vec<String>,
    tx: watch::Sender<Snapshot>,
}

/// The outcome of an `await_dag` / `wait_any` / `resume_await` call.
#[derive(Debug, Clone)]
pub enum AwaitOutcome {
    /// The whole DAG reached a terminal state.
    Done(RunReport),
    /// A single node reached a terminal state; `still_running` lists the
    /// node ids that have not yet reached a terminal state.
    Node {
        node_id: String,
        state: NodeStatus,
        output: Option<String>,
        still_running: Vec<String>,
    },
    /// The requested timeout elapsed before the awaited condition was met.
    /// Carries a token that resumes waiting on the *same* run without
    /// restarting it or missing any transition that happens in between.
    Running(ResumeToken),
}

/// An opaque handle that re-subscribes to the run that produced it. Pass it
/// to `Scheduler::resume_await` to keep waiting from the current state.
#[derive(Clone)]
pub struct ResumeToken(watch::Receiver<Snapshot>);

impl std::fmt::Debug for ResumeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeToken").finish_non_exhaustive()
    }
}

pub struct Scheduler {
    dag: Dag,
    worker: Arc<dyn Worker>,
    event_sink: Arc<tokio::sync::Mutex<Box<dyn EventSink>>>,
    concurrency: usize,
    inner: Arc<Inner>,
    started: bool,
    /// Node ids already handed back by `wait_any` on this scheduler.
    consumed: HashSet<String>,
    /// Optional checkpoint from a prior interrupted run. Applied at the start
    /// of `execute()`: completed nodes are seeded as `Done` (and their
    /// dependents' in-degree pre-decremented), terminal Failed/Blocked nodes
    /// stay terminal, and stale Running / retryable Failed nodes are left
    /// `Pending` to be dispatched as fresh attempts. `None` for a fresh run.
    checkpoint: Option<Checkpoint>,
}

impl Scheduler {
    pub fn new(
        dag: Dag,
        worker: Box<dyn Worker>,
        event_sink: Box<dyn EventSink>,
        concurrency: usize,
    ) -> Self {
        Self::build(dag, worker, event_sink, concurrency, None)
    }

    /// Construct a scheduler that resumes from `checkpoint` (Spec-08). The
    /// checkpoint carries terminal node states from a prior interrupted run
    /// with the same deterministic `run_id`. The event sink / store backing
    /// it MUST be the same vault the checkpoint was loaded from, so resumed
    /// events continue the same run record rather than forking a new one.
    pub fn with_checkpoint(
        dag: Dag,
        worker: Box<dyn Worker>,
        event_sink: Box<dyn EventSink>,
        concurrency: usize,
        checkpoint: Checkpoint,
    ) -> Self {
        Self::build(dag, worker, event_sink, concurrency, Some(checkpoint))
    }

    fn build(
        dag: Dag,
        worker: Box<dyn Worker>,
        event_sink: Box<dyn EventSink>,
        concurrency: usize,
        checkpoint: Option<Checkpoint>,
    ) -> Self {
        let all_node_ids: Vec<String> = dag.nodes.iter().map(|n| n.id.clone()).collect();
        let (tx, _rx) = watch::channel(Snapshot::default());
        Self {
            dag,
            worker: Arc::from(worker),
            event_sink: Arc::new(tokio::sync::Mutex::new(event_sink)),
            concurrency,
            inner: Arc::new(Inner { all_node_ids, tx }),
            started: false,
            consumed: HashSet::new(),
            checkpoint,
        }
    }

    /// Run the DAG to completion, blocking the caller for the whole run.
    /// Kept for direct/offline use (Phase 1 behaviour, unchanged). Terminal
    /// transitions are also published to the internal snapshot channel, so
    /// a subsequent `await_dag`/`wait_any` call on the same `Scheduler`
    /// observes completion immediately instead of starting a second run.
    pub async fn run(&mut self, allow_paid: bool) -> Result<RunReport, PidagError> {
        self.started = true;
        Self::execute(
            self.dag.clone(),
            Arc::clone(&self.worker),
            Arc::clone(&self.event_sink),
            self.concurrency,
            Arc::clone(&self.inner),
            allow_paid,
            self.checkpoint.as_ref(),
        )
        .await
    }

    /// Block until the DAG reaches a terminal state (no polling: parks on
    /// the internal `watch` channel). Starts the run in the background on
    /// first call. If `timeout` elapses first, returns a `ResumeToken`
    /// instead of hanging.
    pub async fn await_dag(&mut self, timeout: Option<Duration>) -> AwaitOutcome {
        self.ensure_started(false);
        let rx = self.inner.tx.subscribe();
        Self::wait_for_done(rx, timeout).await
    }

    /// Continue waiting for DAG completion from a token returned by a
    /// previous timed-out `await_dag`/`wait_any` call. Re-subscribes to the
    /// same run; never restarts it, never replays events (the snapshot is
    /// authoritative state, not a log).
    pub async fn resume_await(
        &mut self,
        token: ResumeToken,
        timeout: Option<Duration>,
    ) -> AwaitOutcome {
        Self::wait_for_done(token.0, timeout).await
    }

    /// Block until the next node (not yet returned by a prior `wait_any`
    /// call on this `Scheduler`) reaches a terminal state. Starts the run
    /// in the background on first call.
    pub async fn wait_any(&mut self, timeout: Option<Duration>) -> AwaitOutcome {
        self.ensure_started(false);
        let rx = self.inner.tx.subscribe();
        self.wait_any_inner(rx, timeout).await
    }

    fn ensure_started(&mut self, allow_paid: bool) {
        if self.started {
            return;
        }
        self.started = true;
        let dag = self.dag.clone();
        let worker = Arc::clone(&self.worker);
        let event_sink = Arc::clone(&self.event_sink);
        let concurrency = self.concurrency;
        let inner = Arc::clone(&self.inner);
        // Clone the checkpoint by value into the spawned task (it is `Send +
        // 'static`): `HashSet<String>` clone is cheap for node-id-sized sets.
        // Owned value passes `&cp.as_ref()` into `execute` inside the async
        // block (the borrow outlives the task because the owned value lives
        // in the task).
        let checkpoint = self.checkpoint.clone();
        tokio::spawn(async move {
            let _ = Self::execute(
                dag,
                worker,
                event_sink,
                concurrency,
                inner,
                allow_paid,
                checkpoint.as_ref(),
            )
            .await;
        });
    }
}

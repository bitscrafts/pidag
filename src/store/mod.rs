use crate::core::error::PidagError;
use crate::core::event::Event;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// Type alias for event storage complexity
type EventMap = Arc<Mutex<HashMap<String, Vec<(u64, Event)>>>>;

pub mod legacy;
pub mod redb_pool;
pub mod redb_store;
pub use crate::scheduler::{BudgetCounters, NodeStatus};
pub use redb_pool::RedbStorePool;
pub use redb_store::RedbStore;

/// Metadata about a run, stored persistently.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunMeta {
    pub run_id: String,
    pub dag_json: String, // Full DAG definition stored as JSON
    pub started_at: String,
    pub completed_at: Option<String>,
    pub successful_nodes: usize,
    pub failed_nodes: usize,
}

/// State and metadata for a single node within a run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeRecord {
    pub node_id: String,
    pub state: NodeStatus,
    pub model: Option<String>,
    pub attempt: usize,
    pub timestamp: String,
}

/// Start/end timing for a single node within a run, used by the trace UI's
/// Gantt timeline view. `started_at` is set on `NodeDispatched`; `ended_at`
/// is set on `NodeDone` / `NodeFailed` and remains `None` while the node is
/// running. This is stored in a dedicated table (not folded into
/// `NodeRecord`) because `NodeRecord.timestamp` is overwritten on every
/// state change and would lose the dispatch time — which the timeline needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeTiming {
    pub started_at: String,       // RFC3339, set on NodeDispatched
    pub ended_at: Option<String>, // RFC3339, set on NodeDone/NodeFailed
}

/// The `Store` trait abstracts persistence: put/get runs, nodes, events, and artifacts.
/// Implementations include `RedbStore` (production) and `MockStore` (testing).
#[async_trait]
pub trait Store: Send + Sync {
    /// Insert or update a run's metadata.
    async fn put_run(&self, run: &RunMeta) -> Result<(), PidagError>;

    /// Retrieve a run's metadata by run_id. Returns None if not found.
    async fn get_run(&self, run_id: &str) -> Result<Option<RunMeta>, PidagError>;

    /// Record or update a node's state within a run.
    async fn put_node_state(
        &self,
        run_id: &str,
        node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), PidagError>;

    /// List all nodes in a run (in arbitrary order).
    async fn list_nodes(&self, run_id: &str) -> Result<Vec<NodeRecord>, PidagError>;

    /// Get the terminal set: (node_id, state) pairs for Done and Failed nodes.
    /// Used to reconstruct the run on resume (crash-durable).
    async fn terminal_set(&self, run_id: &str) -> Result<Vec<(String, String)>, PidagError>;

    /// Append an event to the run's event log. Returns the sequence number.
    async fn append_event(&self, run_id: &str, ev: &Event) -> Result<u64, PidagError>;

    /// Load all events for a run in insertion order.
    async fn load_events(&self, run_id: &str) -> Result<Vec<Event>, PidagError>;

    /// List all runs in the vault (for the trace UI sessions list).
    /// Implementations should return runs sorted by `run_id` for deterministic
    /// UI ordering. Used by the `pidag ui` trace visualizer (P1 #9).
    async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError>;

    /// Load events with sequence number strictly greater than `since` (for
    /// the trace UI polling endpoint `/api/runs/:id/events?since=N`).
    /// Returns `(seq, Event)` pairs sorted by `seq` ascending. The `since`
    /// parameter is the last seq the client has already seen, so the
    /// response contains only newer events.
    async fn load_events_since(
        &self,
        run_id: &str,
        since: u64,
    ) -> Result<Vec<(u64, Event)>, PidagError>;

    /// Store an artifact (node output) for a node.
    async fn put_artifact(
        &self,
        run_id: &str,
        node_id: &str,
        output: &str,
    ) -> Result<(), PidagError>;

    /// Retrieve an artifact by run_id and node_id. Returns None if not found.
    async fn get_artifact(&self, run_id: &str, node_id: &str)
    -> Result<Option<String>, PidagError>;

    /// Store or update the start/end timing for a node within a run.
    /// Used by the trace UI timeline (P1 #10). Set `started_at` on
    /// `NodeDispatched`, then update `ended_at` on `NodeDone`/`NodeFailed`.
    async fn put_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
        timing: &NodeTiming,
    ) -> Result<(), PidagError>;

    /// Retrieve the timing for a single node. Returns None if the node
    /// has not been dispatched yet (no `NodeDispatched` event observed).
    async fn get_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeTiming>, PidagError>;

    /// List timings for all dispatched nodes in a run. Returns `(node_id,
    /// NodeTiming)` pairs in arbitrary order. Used by the timeline endpoint.
    async fn list_node_timings(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, NodeTiming)>, PidagError>;

    /// Project a NodeDispatched event: put_node_state and put_node_timing in one transaction.
    /// The timing_now is the current timestamp; RedbStore uses this to read-modify-write the timing atomically.
    /// Default implementation calls the individual methods; RedbStore overrides for atomicity (T1).
    async fn project_node_dispatched(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.append_event(run_id, ev).await?;
        self.put_node_state(run_id, node_id, rec).await?;
        // Read-modify-write timing: preserve started_at if it exists, else use timing_now
        let timing = match self.get_node_timing(run_id, node_id).await {
            Ok(Some(t)) => NodeTiming {
                started_at: t.started_at,
                ended_at: None,
            },
            _ => NodeTiming {
                started_at: timing_now.to_string(),
                ended_at: None,
            },
        };
        self.put_node_timing(run_id, node_id, &timing).await?;
        Ok(())
    }

    /// Project a NodeDone event: put_node_state, put_artifact, and put_node_timing in one transaction.
    /// The timing_now is the current timestamp; RedbStore uses this to read-modify-write the timing atomically.
    /// Default implementation calls the individual methods; RedbStore overrides for atomicity (T1).
    async fn project_node_done(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        output: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.append_event(run_id, ev).await?;
        self.put_node_state(run_id, node_id, rec).await?;
        self.put_artifact(run_id, node_id, output).await?;
        // Read-modify-write timing: preserve started_at if it exists, else use timing_now
        let timing = match self.get_node_timing(run_id, node_id).await {
            Ok(Some(t)) => NodeTiming {
                started_at: t.started_at,
                ended_at: Some(timing_now.to_string()),
            },
            _ => NodeTiming {
                started_at: timing_now.to_string(),
                ended_at: Some(timing_now.to_string()),
            },
        };
        self.put_node_timing(run_id, node_id, &timing).await?;
        Ok(())
    }

    /// Project a NodeFailed event: put_node_state, put_artifact, and put_node_timing in one transaction.
    /// The timing_now is the current timestamp; RedbStore uses this to read-modify-write the timing atomically.
    /// Default implementation calls the individual methods; RedbStore overrides for atomicity (T1).
    async fn project_node_failed(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        error: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.append_event(run_id, ev).await?;
        self.put_node_state(run_id, node_id, rec).await?;
        self.put_artifact(run_id, node_id, error).await?;
        // Read-modify-write timing: preserve started_at if it exists, else use timing_now
        let timing = match self.get_node_timing(run_id, node_id).await {
            Ok(Some(t)) => NodeTiming {
                started_at: t.started_at,
                ended_at: Some(timing_now.to_string()),
            },
            _ => NodeTiming {
                started_at: timing_now.to_string(),
                ended_at: Some(timing_now.to_string()),
            },
        };
        self.put_node_timing(run_id, node_id, &timing).await?;
        Ok(())
    }

    /// Project a NodeRetry event: put_node_timing in one transaction (or a write txn if batched).
    /// The timing_now is the current timestamp; RedbStore uses this to read-modify-write the timing atomically.
    /// Default implementation calls the individual method; RedbStore may batch with other operations.
    /// Project a NodeBlocked event: one write transaction.
    ///
    /// Blocked was previously not persisted at all -- `RedbSink::emit` had no arm
    /// for it -- so a node blocked by a failed dependency was read back as
    /// `Pending`. Combined with `terminal_set` filtering to Done|Failed, that made
    /// `load_checkpoint`'s `"Blocked"` branch dead code: resume re-dispatched nodes
    /// whose dependency had already failed.
    async fn project_node_blocked(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), PidagError> {
        self.append_event(run_id, ev).await?;
        self.put_node_state(run_id, node_id, rec).await?;
        Ok(())
    }

    async fn project_node_retry(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        self.append_event(run_id, ev).await?;
        // Read-modify-write timing: preserve started_at, clear ended_at
        let timing = match self.get_node_timing(run_id, node_id).await {
            Ok(Some(t)) => NodeTiming {
                started_at: t.started_at,
                ended_at: None,
            },
            _ => NodeTiming {
                started_at: timing_now.to_string(),
                ended_at: None,
            },
        };
        self.put_node_timing(run_id, node_id, &timing).await?;
        Ok(())
    }

    /// Read the cumulative budget counters persisted for a run (spec-39,
    /// B7). Returns `BudgetCounters::default()` (zero) for a run that has
    /// never recorded any -- including every run predating this spec, and
    /// any `Store` implementation that does not override this default (a
    /// third-party or test double never wired to budget ceilings).
    async fn get_budget(&self, run_id: &str) -> Result<BudgetCounters, PidagError> {
        let _ = run_id;
        Ok(BudgetCounters::default())
    }

    /// Persist the cumulative budget counters for a run (spec-39, B7). The
    /// default implementation is a no-op -- overridden by `RedbStore` (and
    /// `MockStore`, for tests) so the counters actually survive a resume.
    /// A `Store` that never overrides this simply never bounds anything
    /// across resumes, which is only correct for a store nothing ever
    /// passes budget ceilings against.
    async fn put_budget(&self, run_id: &str, counters: &BudgetCounters) -> Result<(), PidagError> {
        let _ = (run_id, counters);
        Ok(())
    }

    /// Project a `BudgetUpdated` event: append it, then persist the latest
    /// counters snapshot. Default implementation calls the individual
    /// methods; `RedbStore` may combine into one transaction.
    async fn project_budget_updated(
        &self,
        run_id: &str,
        ev: &Event,
        counters: &BudgetCounters,
    ) -> Result<(), PidagError> {
        self.append_event(run_id, ev).await?;
        self.put_budget(run_id, counters).await?;
        Ok(())
    }
}

/// MockStore for testing: stores everything in memory.
pub struct MockStore {
    runs: Arc<Mutex<HashMap<String, RunMeta>>>,
    nodes: Arc<Mutex<HashMap<String, Vec<NodeRecord>>>>,
    events: EventMap,
    artifacts: Arc<Mutex<HashMap<(String, String), String>>>,
    event_seq: Arc<Mutex<HashMap<String, u64>>>,
    timings: Arc<Mutex<HashMap<(String, String), NodeTiming>>>,
    budgets: Arc<Mutex<HashMap<String, BudgetCounters>>>,
}

impl Default for MockStore {
    fn default() -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            nodes: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(Mutex::new(HashMap::new())),
            artifacts: Arc::new(Mutex::new(HashMap::new())),
            event_seq: Arc::new(Mutex::new(HashMap::new())),
            timings: Arc::new(Mutex::new(HashMap::new())),
            budgets: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl MockStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Store for MockStore {
    async fn put_run(&self, run: &RunMeta) -> Result<(), PidagError> {
        self.runs
            .lock()
            .await
            .insert(run.run_id.clone(), run.clone());
        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunMeta>, PidagError> {
        Ok(self.runs.lock().await.get(run_id).cloned())
    }

    async fn put_node_state(
        &self,
        run_id: &str,
        node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), PidagError> {
        let mut nodes = self.nodes.lock().await;
        let key = run_id.to_string();
        nodes
            .entry(key)
            .or_insert_with(Vec::new)
            .retain(|n| n.node_id != node_id);
        nodes
            .entry(run_id.to_string())
            .or_insert_with(Vec::new)
            .push(rec.clone());
        Ok(())
    }

    async fn list_nodes(&self, run_id: &str) -> Result<Vec<NodeRecord>, PidagError> {
        Ok(self
            .nodes
            .lock()
            .await
            .get(run_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn terminal_set(&self, run_id: &str) -> Result<Vec<(String, String)>, PidagError> {
        let nodes = self.list_nodes(run_id).await?;
        let terminal: Vec<_> = nodes
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

    async fn append_event(&self, run_id: &str, ev: &Event) -> Result<u64, PidagError> {
        let mut seq_map = self.event_seq.lock().await;
        let seq = seq_map.entry(run_id.to_string()).or_insert(0);
        let current_seq = *seq;
        *seq += 1;

        let mut events = self.events.lock().await;
        events
            .entry(run_id.to_string())
            .or_insert_with(Vec::new)
            .push((current_seq, ev.clone()));

        Ok(current_seq)
    }

    async fn load_events(&self, run_id: &str) -> Result<Vec<Event>, PidagError> {
        let events = self.events.lock().await;
        if let Some(event_list) = events.get(run_id) {
            let mut sorted = event_list.clone();
            sorted.sort_by_key(|(seq, _)| *seq);
            Ok(sorted.into_iter().map(|(_, e)| e).collect())
        } else {
            Ok(Vec::new())
        }
    }

    async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError> {
        let runs = self.runs.lock().await;
        let mut all: Vec<RunMeta> = runs.values().cloned().collect();
        all.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        Ok(all)
    }

    async fn load_events_since(
        &self,
        run_id: &str,
        since: u64,
    ) -> Result<Vec<(u64, Event)>, PidagError> {
        let events = self.events.lock().await;
        if let Some(event_list) = events.get(run_id) {
            let mut filtered: Vec<(u64, Event)> = event_list
                .iter()
                .filter(|(seq, _)| *seq > since)
                .cloned()
                .collect();
            filtered.sort_by_key(|(seq, _)| *seq);
            Ok(filtered)
        } else {
            Ok(Vec::new())
        }
    }

    async fn put_artifact(
        &self,
        run_id: &str,
        node_id: &str,
        output: &str,
    ) -> Result<(), PidagError> {
        self.artifacts.lock().await.insert(
            (run_id.to_string(), node_id.to_string()),
            output.to_string(),
        );
        Ok(())
    }

    async fn get_artifact(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<String>, PidagError> {
        Ok(self
            .artifacts
            .lock()
            .await
            .get(&(run_id.to_string(), node_id.to_string()))
            .cloned())
    }

    async fn put_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
        timing: &NodeTiming,
    ) -> Result<(), PidagError> {
        self.timings
            .lock()
            .await
            .insert((run_id.to_string(), node_id.to_string()), timing.clone());
        Ok(())
    }

    async fn get_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeTiming>, PidagError> {
        Ok(self
            .timings
            .lock()
            .await
            .get(&(run_id.to_string(), node_id.to_string()))
            .cloned())
    }

    async fn list_node_timings(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, NodeTiming)>, PidagError> {
        Ok(self
            .timings
            .lock()
            .await
            .iter()
            .filter(|((rid, _), _)| rid == run_id)
            .map(|((_, nid), t)| (nid.clone(), t.clone()))
            .collect())
    }

    async fn get_budget(&self, run_id: &str) -> Result<BudgetCounters, PidagError> {
        Ok(self
            .budgets
            .lock()
            .await
            .get(run_id)
            .copied()
            .unwrap_or_default())
    }

    async fn put_budget(&self, run_id: &str, counters: &BudgetCounters) -> Result<(), PidagError> {
        self.budgets
            .lock()
            .await
            .insert(run_id.to_string(), *counters);
        Ok(())
    }
}

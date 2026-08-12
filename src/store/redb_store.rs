use super::{NodeRecord, NodeStatus, NodeTiming, RunMeta, Store};
use crate::core::error::PidagError;
use crate::core::event::Event;
use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Table definitions
const RUNS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("runs");
const NODES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const EVENTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("events");
const ARTIFACTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("artifacts");
// Per-node start/end timing for the trace UI Gantt timeline (P1 #10).
// Keyed by "run_id\x00node_id" — same prefix-range pattern as NODES_TABLE.
const NODE_TIMING_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("node_timings");
// O(1) per-run event sequence counter (P0-2 fix; see HANDOFF 2026-08-02).
// Keyed by run_id; the stored u64 (BE) is the NEXT seq number to assign.
const SEQ_TABLE: TableDefinition<&str, &[u8; 8]> = TableDefinition::new("event_seq");

/// `RedbStore` persists pidag runs to an embedded redb database.
/// Schema:
///   - `runs`: run_id -> RunMeta (bincode)
///   - `nodes`: "run_id\x00node_id" -> NodeRecord (bincode)
///   - `events`: "run_id\x00seq(u64 BE)" -> Event (bincode)
///   - `artifacts`: "run_id\x00node_id" -> output text (UTF-8)
///   - `node_timings`: "run_id\x00node_id" -> NodeTiming (bincode)
///   - `event_seq`: run_id -> next-seq u64 (BE)
pub struct RedbStore {
    db: Arc<Database>,
}

impl RedbStore {
    /// Default path for the pidag vault: `<cwd>/.pidag/pidag.redb`
    pub fn default_path(cwd: &Path) -> PathBuf {
        cwd.join(".pidag").join("pidag.redb")
    }

    /// Open or create a store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PidagError> {
        let path = path.as_ref();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| PidagError::Store(format!("Failed to create vault dir: {}", e)))?;
        }

        let db = Database::create(path)
            .map_err(|e| PidagError::Store(format!("Failed to open redb: {}", e)))?;

        // Durability note (P0-1, 2026-08-02 audit): redb's `WriteTransaction`
        // defaults to `Durability::Immediate` — i.e. `commit()` fsyncs before
        // returning, so a host crash within ~1s of a commit cannot lose
        // just-written Run/Node/Event rows. No explicit `set_durability` call
        // is needed (redb 1.5.2 exposes it only on `WriteTransaction`, not on
        // `Database`, and the default already matches what we want). Throughput
        // cost is irrelevant for pidag's ~1-event-per-node workload.

        // Create tables if they don't exist
        {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let _ = write_txn
                .open_table(RUNS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open runs table: {}", e)))?;
            let _ = write_txn
                .open_table(NODES_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open nodes table: {}", e)))?;
            let _ = write_txn
                .open_table(EVENTS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open events table: {}", e)))?;
            let _ = write_txn
                .open_table(ARTIFACTS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open artifacts table: {}", e)))?;
            let _ = write_txn
                .open_table(SEQ_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open seq table: {}", e)))?;
            let _ = write_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                PidagError::Store(format!("Failed to open node_timings table: {}", e))
            })?;
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
        }

        Ok(Self { db: Arc::new(db) })
    }
}

/// Append an event to the log **inside a caller-supplied transaction**.
///
/// Extracted so the projection methods can fold the event append into the same
/// write transaction as the state/artifact/timing writes. Before this, every
/// event cost two commits -- one for `append_event`, one for the projection --
/// which is why the measured figure sat at 2 per event rather than the 1 that
/// spec-32 T1 requires.
///
/// The per-run sequence counter is read and written in this same transaction,
/// preserving the O(log rows) property that replaced an O(N^2) full-table scan
/// (HANDOFF 2026-08-02, P0-2).
fn append_event_in_txn(
    write_txn: &redb::WriteTransaction,
    run_id: &str,
    ev: &Event,
) -> Result<(), PidagError> {
    let mut seq_table = write_txn
        .open_table(SEQ_TABLE)
        .map_err(|e| PidagError::Store(format!("Failed to open seq table: {}", e)))?;
    let mut events = write_txn
        .open_table(EVENTS_TABLE)
        .map_err(|e| PidagError::Store(format!("Failed to open events table: {}", e)))?;

    let current: u64 = seq_table
        .get(run_id)
        .map_err(|e| PidagError::Store(format!("Seq get failed: {}", e)))?
        .map(|v| u64::from_be_bytes(*v.value()))
        .unwrap_or(0);

    let key = format!("{}\x00{:020}", run_id, current);
    let serialized = bincode::serialize(ev)
        .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
    events
        .insert(key.as_str(), serialized.as_slice())
        .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

    let next = current + 1;
    seq_table
        .insert(run_id, &next.to_be_bytes())
        .map_err(|e| PidagError::Store(format!("Seq insert failed: {}", e)))?;
    Ok(())
}

#[async_trait]
impl Store for RedbStore {
    async fn put_run(&self, run: &RunMeta) -> Result<(), PidagError> {
        let db = self.db.clone();
        let run = run.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                let mut runs = write_txn
                    .open_table(RUNS_TABLE)
                    .map_err(|e| PidagError::Store(format!("Failed to open runs table: {}", e)))?;
                let serialized = bincode::serialize(&run)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                runs.insert(run.run_id.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunMeta>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let runs = read_txn
                .open_table(RUNS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open runs table: {}", e)))?;
            match runs
                .get(run_id.as_str())
                .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
            {
                Some(access) => {
                    let bytes = access.value().to_vec();
                    let run: RunMeta = bincode::deserialize(&bytes)
                        .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                    Ok(Some(run))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn put_node_state(
        &self,
        run_id: &str,
        node_id: &str,
        rec: &NodeRecord,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let rec = rec.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                let mut nodes = write_txn
                    .open_table(NODES_TABLE)
                    .map_err(|e| PidagError::Store(format!("Failed to open nodes table: {}", e)))?;
                let key = format!("{}\x00{}", run_id, node_id);
                let serialized = bincode::serialize(&rec)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                nodes
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn list_nodes(&self, run_id: &str) -> Result<Vec<NodeRecord>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let nodes = read_txn
                .open_table(NODES_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open nodes table: {}", e)))?;

            // P1 #3 (2026-08-02 audit): range-query over the run prefix instead of a
            // full-table `.iter()` scan filtered by `starts_with`. redb iterates keys
            // in sorted order, so bounding the range to `["run_id\x00", "run_id\x01)`
            // skips every run's rows. Keys are `run_id\x00node_id`; the byte after the
            // `\x00` separator is the smallest possible node-id byte, so the next
            // lexicographic prefix (`run_id\x01`) bounds exactly this run's rows.
            let prefix_start = format!("{}\x00", run_id);
            let prefix_end = format!("{}\x01", run_id);
            let mut result = Vec::new();

            let range = nodes
                .range(prefix_start.as_str()..prefix_end.as_str())
                .map_err(|e| PidagError::Store(format!("Range iteration failed: {}", e)))?;
            for entry in range {
                let (_key, value) =
                    entry.map_err(|e| PidagError::Store(format!("Entry read failed: {}", e)))?;
                let bytes = value.value().to_vec();
                let rec: NodeRecord = bincode::deserialize(&bytes)
                    .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                result.push(rec);
            }

            Ok(result)
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn terminal_set(&self, run_id: &str) -> Result<Vec<(String, String)>, PidagError> {
        let nodes = self.list_nodes(run_id).await?;
        let terminal: Vec<_> = nodes
            .into_iter()
            // Blocked is terminal. Omitting it made load_checkpoint's "Blocked"
            // branch unreachable and resume re-dispatched nodes whose dependency
            // had already failed.
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
        // O(1) per-run event sequence counter (P0-2 fix; see HANDOFF 2026-08-02):
        // every previous version of this method did a full-table scan over ALL
        // events in the vault to compute `max_seq + 1` — O(N²) event indexing.
        // We now keep the next-seq counter for this run in a dedicated table,
        // read+write it inside the SAME write transaction as the insert —
        // so `append_event` is O(log rows) instead of O(rows).
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let ev = ev.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let next_seq = {
                let mut seq_table = write_txn
                    .open_table(SEQ_TABLE)
                    .map_err(|e| PidagError::Store(format!("Failed to open seq table: {}", e)))?;
                let mut events = write_txn.open_table(EVENTS_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open events table: {}", e))
                })?;

                // Read current next-seq (default to 0 if first event for this run).
                let current: u64 = seq_table
                    .get(run_id.as_str())
                    .map_err(|e| PidagError::Store(format!("Seq get failed: {}", e)))?
                    .map(|v| u64::from_be_bytes(*v.value()))
                    .unwrap_or(0);

                // Insert the event at this seq, advance the counter.
                let key = format!("{}\x00{:020}", run_id, current);
                let serialized = bincode::serialize(&ev)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                events
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

                let next = current + 1;
                seq_table
                    .insert(run_id.as_str(), &next.to_be_bytes())
                    .map_err(|e| PidagError::Store(format!("Seq insert failed: {}", e)))?;
                current
            };
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;

            Ok(next_seq)
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn load_events(&self, run_id: &str) -> Result<Vec<Event>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let events = read_txn
                .open_table(EVENTS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open events table: {}", e)))?;

            // P1 #3 (2026-08-02 audit): range-query over the run prefix. Same closed
            // range technique as `list_nodes` above; the zero-padded 20-digit seq
            // suffix means lexicographic iteration order equals numeric order, so
            // the post-sort below matches what redb already yields, but is kept as a
            // defensive invariant in case the key format ever changes.
            let prefix_start = format!("{}\x00", run_id);
            let prefix_end = format!("{}\x01", run_id);
            let mut result: Vec<(u64, Event)> = Vec::new();

            let range = events
                .range(prefix_start.as_str()..prefix_end.as_str())
                .map_err(|e| PidagError::Store(format!("Range iteration failed: {}", e)))?;
            for entry in range {
                let (key, value) =
                    entry.map_err(|e| PidagError::Store(format!("Entry read failed: {}", e)))?;
                let key_str = key.value();
                if let Some(seq_str) = key_str.strip_prefix(&prefix_start)
                    && let Ok(seq) = seq_str.parse::<u64>()
                {
                    let ev_bytes = value.value().to_vec();
                    let ev: Event = bincode::deserialize(&ev_bytes)
                        .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                    result.push((seq, ev));
                }
            }

            // Sort by sequence number (defensive; see comment above).
            result.sort_by_key(|(seq, _)| *seq);
            Ok(result.into_iter().map(|(_, e)| e).collect())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let runs = read_txn
                .open_table(RUNS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open runs table: {}", e)))?;

            // P1 #9 (2026-08-03): full-table iter is fine here — runs are few
            // (one row per `pidag run` invocation), so a scan over RUNS_TABLE
            // is O(runs) and cheaper than maintaining a secondary index. The
            // range-query optimisation from P1 #3 (list_nodes/load_events) is
            // unnecessary at this cardinality.
            let mut result = Vec::new();
            let iter = runs
                .iter()
                .map_err(|e| PidagError::Store(format!("Runs iteration failed: {}", e)))?;
            for entry in iter {
                let (_key, value) =
                    entry.map_err(|e| PidagError::Store(format!("Entry read failed: {}", e)))?;
                let bytes = value.value().to_vec();
                let run: RunMeta = bincode::deserialize(&bytes)
                    .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                result.push(run);
            }

            // Sort by run_id for deterministic UI ordering (matches MockStore).
            result.sort_by(|a, b| a.run_id.cmp(&b.run_id));
            Ok(result)
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn load_events_since(
        &self,
        run_id: &str,
        since: u64,
    ) -> Result<Vec<(u64, Event)>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let events = read_txn
                .open_table(EVENTS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open events table: {}", e)))?;

            // P1 #9 (2026-08-03): same prefix-range technique as `load_events`,
            // but the lower bound starts at `since + 1` (zero-padded 20-digit
            // seq) so only events with seq > `since` are returned. The key
            // format is `"run_id\x00{seq:020}"` and lexicographic order equals
            // numeric order because of the zero padding. `saturating_add`
            // guards against the degenerate `since == u64::MAX` case.
            let lower = format!("{}\x00{:020}", run_id, since.saturating_add(1));
            let upper = format!("{}\x01", run_id);
            let prefix_start = format!("{}\x00", run_id);
            let mut result: Vec<(u64, Event)> = Vec::new();

            let range = events
                .range(lower.as_str()..upper.as_str())
                .map_err(|e| PidagError::Store(format!("Range iteration failed: {}", e)))?;
            for entry in range {
                let (key, value) =
                    entry.map_err(|e| PidagError::Store(format!("Entry read failed: {}", e)))?;
                let key_str = key.value();
                if let Some(seq_str) = key_str.strip_prefix(&prefix_start)
                    && let Ok(seq) = seq_str.parse::<u64>()
                {
                    let ev_bytes = value.value().to_vec();
                    let ev: Event = bincode::deserialize(&ev_bytes)
                        .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                    result.push((seq, ev));
                }
            }

            result.sort_by_key(|(seq, _)| *seq);
            Ok(result)
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn put_artifact(
        &self,
        run_id: &str,
        node_id: &str,
        output: &str,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let output = output.to_string();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                let mut artifacts = write_txn.open_table(ARTIFACTS_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open artifacts table: {}", e))
                })?;
                let key = format!("{}\x00{}", run_id, node_id);
                let serialized = bincode::serialize(&output)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                artifacts
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn get_artifact(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<String>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let artifacts = read_txn
                .open_table(ARTIFACTS_TABLE)
                .map_err(|e| PidagError::Store(format!("Failed to open artifacts table: {}", e)))?;

            let key = format!("{}\x00{}", run_id, node_id);
            match artifacts
                .get(key.as_str())
                .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
            {
                Some(access) => {
                    let bytes = access.value().to_vec();
                    let output: String = bincode::deserialize(&bytes)
                        .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                    Ok(Some(output))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn put_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
        timing: &NodeTiming,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let timing = timing.clone();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                let mut timings = write_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open node_timings table: {}", e))
                })?;
                let key = format!("{}\x00{}", run_id, node_id);
                let serialized = bincode::serialize(&timing)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                timings
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn get_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeTiming>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let timings = read_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                PidagError::Store(format!("Failed to open node_timings table: {}", e))
            })?;
            let key = format!("{}\x00{}", run_id, node_id);
            match timings
                .get(key.as_str())
                .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
            {
                Some(access) => {
                    let bytes = access.value().to_vec();
                    let timing: NodeTiming = bincode::deserialize(&bytes)
                        .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                    Ok(Some(timing))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    async fn list_node_timings(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, NodeTiming)>, PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            let timings = read_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                PidagError::Store(format!("Failed to open node_timings table: {}", e))
            })?;

            // Same prefix-range technique as `list_nodes`: keys are
            // "run_id\x00node_id", so the range `["run_id\x00", "run_id\x01")`
            // covers exactly this run's rows in sorted order.
            let prefix_start = format!("{}\x00", run_id);
            let prefix_end = format!("{}\x01", run_id);
            let mut result = Vec::new();

            let range = timings
                .range(prefix_start.as_str()..prefix_end.as_str())
                .map_err(|e| PidagError::Store(format!("Range iteration failed: {}", e)))?;
            for entry in range {
                let (key, value) =
                    entry.map_err(|e| PidagError::Store(format!("Entry read failed: {}", e)))?;
                let key_str = key.value();
                let node_id = key_str
                    .strip_prefix(&prefix_start)
                    .unwrap_or(key_str)
                    .to_string();
                let bytes = value.value().to_vec();
                let timing: NodeTiming = bincode::deserialize(&bytes)
                    .map_err(|e| PidagError::Store(format!("Deserialization failed: {}", e)))?;
                result.push((node_id, timing));
            }

            Ok(result)
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    /// Project NodeDispatched: put_node_state and put_node_timing in ONE write transaction.
    /// Reads existing timing inside the transaction for atomicity.
    async fn project_node_dispatched(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let ev = ev.clone();
        let rec = rec.clone();
        let timing_now = timing_now.to_string();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                // T1: the event append joins THIS transaction, so one event costs
                // one commit rather than two.
                append_event_in_txn(&write_txn, &run_id, &ev)?;
                let mut nodes = write_txn
                    .open_table(NODES_TABLE)
                    .map_err(|e| PidagError::Store(format!("Failed to open nodes table: {}", e)))?;
                let key = format!("{}\x00{}", run_id, node_id);
                let serialized = bincode::serialize(&rec)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                nodes
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

                // Read-modify-write timing inside the transaction for atomicity
                let mut timings = write_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open node_timings table: {}", e))
                })?;
                let timing_key = format!("{}\x00{}", run_id, node_id);
                let timing = match timings
                    .get(timing_key.as_str())
                    .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
                {
                    Some(access) => {
                        let bytes = access.value().to_vec();
                        let t: NodeTiming = bincode::deserialize(&bytes).map_err(|e| {
                            PidagError::Store(format!("Deserialization failed: {}", e))
                        })?;
                        NodeTiming {
                            started_at: t.started_at,
                            ended_at: None,
                        }
                    }
                    None => NodeTiming {
                        started_at: timing_now.to_string(),
                        ended_at: None,
                    },
                };
                let timing_serialized = bincode::serialize(&timing)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                timings
                    .insert(timing_key.as_str(), timing_serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    /// Project NodeDone: put_node_state, put_artifact, and put_node_timing in ONE write transaction.
    /// Reads existing timing inside the transaction for atomicity.
    async fn project_node_done(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        output: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let ev = ev.clone();
        let rec = rec.clone();
        let output = output.to_string();
        let timing_now = timing_now.to_string();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                // T1: the event append joins THIS transaction, so one event costs
                // one commit rather than two.
                append_event_in_txn(&write_txn, &run_id, &ev)?;
                let mut nodes = write_txn
                    .open_table(NODES_TABLE)
                    .map_err(|e| PidagError::Store(format!("Failed to open nodes table: {}", e)))?;
                let key = format!("{}\x00{}", run_id, node_id);
                let serialized = bincode::serialize(&rec)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                nodes
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

                let mut artifacts = write_txn.open_table(ARTIFACTS_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open artifacts table: {}", e))
                })?;
                let artifact_key = format!("{}\x00{}", run_id, node_id);
                let artifact_serialized = bincode::serialize(&output)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                artifacts
                    .insert(artifact_key.as_str(), artifact_serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

                // Read-modify-write timing inside the transaction for atomicity
                let mut timings = write_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open node_timings table: {}", e))
                })?;
                let timing_key = format!("{}\x00{}", run_id, node_id);
                let timing = match timings
                    .get(timing_key.as_str())
                    .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
                {
                    Some(access) => {
                        let bytes = access.value().to_vec();
                        let t: NodeTiming = bincode::deserialize(&bytes).map_err(|e| {
                            PidagError::Store(format!("Deserialization failed: {}", e))
                        })?;
                        NodeTiming {
                            started_at: t.started_at,
                            ended_at: Some(timing_now.to_string()),
                        }
                    }
                    None => NodeTiming {
                        started_at: timing_now.to_string(),
                        ended_at: Some(timing_now.to_string()),
                    },
                };
                let timing_serialized = bincode::serialize(&timing)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                timings
                    .insert(timing_key.as_str(), timing_serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    /// Project NodeFailed: put_node_state, put_artifact, and put_node_timing in ONE write transaction.
    /// Reads existing timing inside the transaction for atomicity.
    async fn project_node_failed(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        rec: &NodeRecord,
        error: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let ev = ev.clone();
        let rec = rec.clone();
        let error = error.to_string();
        let timing_now = timing_now.to_string();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                // T1: the event append joins THIS transaction, so one event costs
                // one commit rather than two.
                append_event_in_txn(&write_txn, &run_id, &ev)?;
                let mut nodes = write_txn
                    .open_table(NODES_TABLE)
                    .map_err(|e| PidagError::Store(format!("Failed to open nodes table: {}", e)))?;
                let key = format!("{}\x00{}", run_id, node_id);
                let serialized = bincode::serialize(&rec)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                nodes
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

                let mut artifacts = write_txn.open_table(ARTIFACTS_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open artifacts table: {}", e))
                })?;
                let artifact_key = format!("{}\x00{}", run_id, node_id);
                let artifact_serialized = bincode::serialize(&error)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                artifacts
                    .insert(artifact_key.as_str(), artifact_serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;

                // Read-modify-write timing inside the transaction for atomicity
                let mut timings = write_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open node_timings table: {}", e))
                })?;
                let timing_key = format!("{}\x00{}", run_id, node_id);
                let timing = match timings
                    .get(timing_key.as_str())
                    .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
                {
                    Some(access) => {
                        let bytes = access.value().to_vec();
                        let t: NodeTiming = bincode::deserialize(&bytes).map_err(|e| {
                            PidagError::Store(format!("Deserialization failed: {}", e))
                        })?;
                        NodeTiming {
                            started_at: t.started_at,
                            ended_at: Some(timing_now.to_string()),
                        }
                    }
                    None => NodeTiming {
                        started_at: timing_now.to_string(),
                        ended_at: Some(timing_now.to_string()),
                    },
                };
                let timing_serialized = bincode::serialize(&timing)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                timings
                    .insert(timing_key.as_str(), timing_serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }

    /// Project NodeRetry: put_node_timing in one (batched) write transaction.
    /// Reads existing timing inside the transaction for atomicity.
    async fn project_node_retry(
        &self,
        run_id: &str,
        ev: &Event,
        node_id: &str,
        timing_now: &str,
    ) -> Result<(), PidagError> {
        let db = Arc::clone(&self.db);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let ev = ev.clone();
        let timing_now = timing_now.to_string();
        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| PidagError::Store(format!("Transaction failed: {}", e)))?;
            {
                // T1: the event append joins THIS transaction, so one event costs
                // one commit rather than two.
                append_event_in_txn(&write_txn, &run_id, &ev)?;
                let mut timings = write_txn.open_table(NODE_TIMING_TABLE).map_err(|e| {
                    PidagError::Store(format!("Failed to open node_timings table: {}", e))
                })?;
                let key = format!("{}\x00{}", run_id, node_id);
                // Read-modify-write timing: preserve started_at, clear ended_at
                let timing = match timings
                    .get(key.as_str())
                    .map_err(|e| PidagError::Store(format!("Get failed: {}", e)))?
                {
                    Some(access) => {
                        let bytes = access.value().to_vec();
                        let t: NodeTiming = bincode::deserialize(&bytes).map_err(|e| {
                            PidagError::Store(format!("Deserialization failed: {}", e))
                        })?;
                        NodeTiming {
                            started_at: t.started_at,
                            ended_at: None,
                        }
                    }
                    None => NodeTiming {
                        started_at: timing_now.to_string(),
                        ended_at: None,
                    },
                };
                let serialized = bincode::serialize(&timing)
                    .map_err(|e| PidagError::Store(format!("Serialization failed: {}", e)))?;
                timings
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| PidagError::Store(format!("Insert failed: {}", e)))?;
            }
            write_txn
                .commit()
                .map_err(|e| PidagError::Store(format!("Commit failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| PidagError::Store(format!("Task failed: {}", e)))?
    }
}

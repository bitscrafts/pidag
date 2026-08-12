use crate::scheduler::NodeStatus;
use crate::store::{NodeRecord, Store};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    DagSubmitted,
    DagDone {
        successful_nodes: usize,
        failed_nodes: usize,
    },
    NodeDispatched {
        node_id: String,
        model: String,
        attempt: usize,
    },
    NodeDone {
        node_id: String,
        model: String,
        output: String,
    },
    NodeFailed {
        node_id: String,
        error: String,
    },
    NodeBlocked {
        node_id: String,
    },
    NodeRetry {
        node_id: String,
        reason: String,
    },
    ProviderFallback {
        node_id: String,
        from_model: String,
        to_model: String,
    },
    NodeSkipped {
        node_id: String,
        reason: String,
    },
    /// Emitted when verify fails after worker claimed success. Both facts are
    /// visible: the worker's claim and the verification failure. Truncated to
    /// 8 KB per the truncation convention used for `NodeFailed`.
    NodeVerifyFailed {
        node_id: String,
        worker_claim: String,
        verify_output: String,
    },
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&mut self, event: &Event) -> Result<(), std::io::Error>;
}

pub struct JsonlSink {
    // T5: Keep the original writer and buffer lines before flushing to improve throughput.
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    // Buffer for accumulating lines before writing to the underlying writer (uses tokio::sync::Mutex for async safety)
    buffer: Arc<tokio::sync::Mutex<Vec<u8>>>,
}

impl JsonlSink {
    pub fn new(writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>) -> Self {
        Self {
            writer,
            buffer: Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(64 * 1024))), // 64 KB buffer
        }
    }

    /// Flush the buffer to the writer. Called on drain or periodically.
    pub async fn flush_buffer(&self) -> Result<(), std::io::Error> {
        let mut buffer = self.buffer.lock().await;
        if !buffer.is_empty() {
            let mut writer = self
                .writer
                .lock()
                .ok()
                .ok_or_else(|| std::io::Error::other("lock failed"))?;
            writer.write_all(&buffer)?;
            writer.flush()?;
            buffer.clear();
        }
        Ok(())
    }
}

#[async_trait]
impl EventSink for JsonlSink {
    async fn emit(&mut self, event: &Event) -> Result<(), std::io::Error> {
        let line = serde_json::to_string(&event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut buffer = self.buffer.lock().await;
        // T5: Write to buffer instead of directly to the writer. No flush per line.
        writeln!(buffer, "{}", line)?;
        // Flush the buffer if it's getting large
        if buffer.len() >= 64 * 1024 {
            drop(buffer); // Release the lock
            self.flush_buffer().await?;
        }
        Ok(())
    }
}

pub struct VecSink {
    events: Arc<Mutex<Vec<Event>>>,
}

impl Default for VecSink {
    fn default() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl VecSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clone_sink(&self) -> Self {
        Self {
            events: Arc::clone(&self.events),
        }
    }
}

impl Clone for VecSink {
    fn clone(&self) -> Self {
        self.clone_sink()
    }
}

#[async_trait]
impl EventSink for VecSink {
    async fn emit(&mut self, event: &Event) -> Result<(), std::io::Error> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
        Ok(())
    }
}

/// `CompositeSink` broadcasts one `emit` to N child sinks in order.
/// A child error is isolated (logged, does not abort the others or the run).
pub struct CompositeSink {
    children: Vec<Box<dyn EventSink>>,
}

impl CompositeSink {
    pub fn new(children: Vec<Box<dyn EventSink>>) -> Self {
        Self { children }
    }
}

#[async_trait]
impl EventSink for CompositeSink {
    async fn emit(&mut self, event: &Event) -> Result<(), std::io::Error> {
        for child in &mut self.children {
            // Isolate child errors: log them but don't abort or fail the run
            if let Err(e) = child.emit(event).await {
                eprintln!("CompositeSink child error (isolated): {}", e);
            }
        }
        Ok(())
    }
}

/// `RedbSink` persists events to a vault through the `Store` trait.
/// On each `emit`, it appends the event to the `events` table (source of truth)
/// and folds it into projections: `DagSubmitted → put_run`; `NodeDispatched → put_node_state(Running)`;
/// etc.
#[derive(Clone)]
pub struct RedbSink {
    store: Arc<dyn Store>,
    run_id: String,
    write_failures: Arc<AtomicUsize>,
}

impl RedbSink {
    pub fn new(store: Arc<dyn Store>, run_id: String) -> Self {
        Self {
            store,
            run_id,
            write_failures: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn new_with_failures(
        store: Arc<dyn Store>,
        run_id: String,
        write_failures: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            store,
            run_id,
            write_failures,
        }
    }

    pub fn write_failures(&self) -> usize {
        self.write_failures.load(Ordering::SeqCst)
    }

    pub fn write_failures_arc(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.write_failures)
    }
}

#[async_trait]
impl EventSink for RedbSink {
    async fn emit(&mut self, event: &Event) -> Result<(), std::io::Error> {
        // T1: events WITH a projection have their append folded into the
        // projection's own transaction, so they cost one commit rather than two.
        // Only events with no projection are appended on their own here.
        let has_projection = matches!(
            event,
            Event::NodeDispatched { .. }
                | Event::NodeDone { .. }
                | Event::NodeFailed { .. }
                | Event::NodeRetry { .. }
                | Event::NodeBlocked { .. }
        );
        if !has_projection && let Err(e) = self.store.append_event(&self.run_id, event).await {
            eprintln!("Failed to append event: {}", e);
            self.write_failures.fetch_add(1, Ordering::SeqCst);
        }

        // Project event into vault tables
        match event {
            Event::NodeBlocked { node_id } => {
                let now = chrono::Utc::now().to_rfc3339();
                let rec = NodeRecord {
                    node_id: node_id.clone(),
                    state: NodeStatus::Blocked,
                    model: None,
                    attempt: 0,
                    timestamp: now,
                };
                if let Err(e) = self
                    .store
                    .project_node_blocked(&self.run_id, event, node_id, &rec)
                    .await
                {
                    eprintln!("Failed to project NodeBlocked for {}: {}", node_id, e);
                    self.write_failures.fetch_add(1, Ordering::SeqCst);
                }
            }
            Event::DagSubmitted => {
                // Only create a placeholder run if one doesn't already exist.
                // `run_subcommand` pre-seeds the run with the full dag_json
                // BEFORE the scheduler starts; blindly overwriting here would
                // replace it with an empty placeholder (the dag_json="{}" bug
                // fixed 2026-08-04). Check existence first and skip if present.
                if let Ok(None) = self.store.get_run(&self.run_id).await {
                    let run = crate::store::RunMeta {
                        run_id: self.run_id.clone(),
                        dag_json: "{}".to_string(),
                        started_at: chrono::Utc::now().to_rfc3339(),
                        completed_at: None,
                        successful_nodes: 0,
                        failed_nodes: 0,
                    };
                    if let Err(e) = self.store.put_run(&run).await {
                        eprintln!("Failed to put run on DagSubmitted: {}", e);
                        self.write_failures.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            Event::NodeDispatched {
                node_id,
                model,
                attempt,
            } => {
                let now = chrono::Utc::now().to_rfc3339();
                let rec = NodeRecord {
                    node_id: node_id.clone(),
                    state: NodeStatus::Running,
                    model: Some(model.clone()),
                    attempt: *attempt,
                    timestamp: now.clone(),
                };
                // T1: single transaction. The timing read-modify-write preserves the
                // ORIGINAL started_at across retries, so a node that ran four minutes
                // with retries renders as a four-minute bar rather than the final
                // attempt's 14ms.
                if let Err(e) = self
                    .store
                    .project_node_dispatched(&self.run_id, event, node_id, &rec, &now)
                    .await
                {
                    eprintln!("Failed to project NodeDispatched for {}: {}", node_id, e);
                    self.write_failures.fetch_add(1, Ordering::SeqCst);
                }
            }
            Event::NodeDone {
                node_id,
                model: _,
                output,
            } => {
                let now = chrono::Utc::now().to_rfc3339();
                let rec = NodeRecord {
                    node_id: node_id.clone(),
                    state: NodeStatus::Done,
                    model: None,
                    attempt: 1,
                    timestamp: now.clone(),
                };
                // T1: one write transaction for state + artifact + timing. The
                // read-modify-write of timing joins that transaction, which also
                // closes a lost-update race between concurrently finishing nodes.
                if let Err(e) = self
                    .store
                    .project_node_done(&self.run_id, event, node_id, &rec, output, &now)
                    .await
                {
                    eprintln!("Failed to project NodeDone for {}: {}", node_id, e);
                    self.write_failures.fetch_add(1, Ordering::SeqCst);
                }
            }
            Event::NodeFailed { node_id, error } => {
                let now = chrono::Utc::now().to_rfc3339();
                let rec = NodeRecord {
                    node_id: node_id.clone(),
                    state: NodeStatus::Failed,
                    model: None,
                    attempt: 1,
                    timestamp: now.clone(),
                };
                // T1: single transaction. The error text is stored as the artifact so
                // `pidag show` can retrieve it.
                if let Err(e) = self
                    .store
                    .project_node_failed(&self.run_id, event, node_id, &rec, error, &now)
                    .await
                {
                    eprintln!("Failed to project NodeFailed for {}: {}", node_id, e);
                    self.write_failures.fetch_add(1, Ordering::SeqCst);
                }
            }
            Event::DagDone {
                successful_nodes,
                failed_nodes,
            } => {
                // Update run to mark completion
                if let Ok(Some(mut run)) = self.store.get_run(&self.run_id).await {
                    run.completed_at = Some(chrono::Utc::now().to_rfc3339());
                    run.successful_nodes = *successful_nodes;
                    run.failed_nodes = *failed_nodes;
                    if let Err(e) = self.store.put_run(&run).await {
                        eprintln!("Failed to put run on DagDone: {}", e);
                        self.write_failures.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            Event::NodeRetry { node_id, reason: _ } => {
                // T1: single transaction. Clears ended_at but PRESERVES started_at so
                // the Gantt bar spans the whole retry+backoff window.
                let now = chrono::Utc::now().to_rfc3339();
                if let Err(e) = self
                    .store
                    .project_node_retry(&self.run_id, event, node_id, &now)
                    .await
                {
                    eprintln!("Failed to project NodeRetry for {}: {}", node_id, e);
                    self.write_failures.fetch_add(1, Ordering::SeqCst);
                }
            }
            _ => {
                // Other events are logged but don't update projections
            }
        }

        Ok(())
    }
}

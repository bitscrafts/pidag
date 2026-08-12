//! Shared handler logic for RPC and MCP servers.
//! This module contains the core business logic for DAG submission, status,
//! results, and node retry operations that are used by both the RPC and MCP
//! server implementations.

use crate::core::Config;
use crate::core::dag::Dag;
use crate::core::event::{CompositeSink, Event, EventSink, JsonlSink, RedbSink, VecSink};
use crate::scheduler::{NodeStatus, Scheduler};
use crate::store::{NodeRecord, Store};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Shared server state for both RPC and MCP servers
pub struct ServerState {
    pub runs: Arc<Mutex<HashMap<String, RunState>>>,
    pub vault_path: PathBuf,
    pub concurrency: usize,
    pub store: Arc<dyn Store>,
    pub config: Config,
}

impl ServerState {
    pub fn new(
        vault_path: PathBuf,
        concurrency: usize,
        store: Arc<dyn Store>,
        config: Config,
    ) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            vault_path,
            concurrency,
            store,
            config,
        }
    }
}

/// Represents a JSON-RPC 2.0 request
#[derive(Debug, Clone)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    pub params: Value,
}

/// Represents the state of an active DAG run.
/// After H1, holds only cheap, Send handles (JoinHandle, Arc status cell, completed_at),
/// not an unexecuted Scheduler. The scheduler lives inside its spawned task.
pub struct RunState {
    pub task_handle: JoinHandle<()>,
    pub dag_id: String,
    pub completed_at: Arc<Mutex<Option<Instant>>>,
}

impl RunState {
    pub fn is_terminal(&self) -> bool {
        match self.completed_at.lock() {
            Ok(guard) => guard.is_some(),
            Err(_) => false,
        }
    }
}

/// Submit a DAG for execution.
/// Spawns the scheduler on a Tokio task and returns immediately with status "submitted".
/// The run executes to completion in the background, and completed_at is set when done.
pub async fn handle_dag_submit(
    state: &ServerState,
    dag_json: &Value,
) -> Result<Value, (i32, String)> {
    // Parse the DAG
    let dag: Dag = serde_json::from_value(dag_json.clone())
        .map_err(|e| (-32602, format!("Invalid DAG JSON: {}", e)))?;

    // spec-38 F5/G4: expand for_each/quorum before validating, same as the
    // `pidag run` CLI path -- the worker built below must see the expanded
    // (child) node ids, not the un-expanded parent. No-op for a DAG without
    // for_each/quorum (N1).
    let dag = dag
        .expand()
        .map_err(|e| (-32000, format!("DAG expansion failed: {}", e)))?;

    // Validate the DAG
    dag.validate()
        .map_err(|e| (-32000, format!("DAG validation failed: {}", e)))?;

    // Generate run ID
    let dag_id = format!(
        "run-{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        uuid_short()
    );

    // Create event sinks
    let redb_sink = Box::new(RedbSink::new(Arc::clone(&state.store), dag_id.clone()));
    let vec_sink = Box::new(VecSink::new());

    // Create JSONL sink for event logging
    let jsonl_file_path = state
        .vault_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".pidag"));
    let _ = std::fs::create_dir_all(&jsonl_file_path);
    let event_file_path = jsonl_file_path.join(format!("{}.events.jsonl", dag_id));

    let jsonl_sink: Box<dyn EventSink> = match std::fs::File::create(&event_file_path) {
        Ok(file) => Box::new(JsonlSink::new(Arc::new(Mutex::new(
            Box::new(file) as Box<dyn Write + Send>
        )))),
        Err(_) => Box::new(VecSink::new()),
    };

    let composite_sink = Box::new(CompositeSink::new(vec![redb_sink, vec_sink, jsonl_sink]));

    let dag_copy = dag.clone();
    // Use configured worker timeout and backend (spec-21: construct via build_worker).
    let worker_timeout = Duration::from_secs(state.config.worker.timeout_secs);
    let worker =
        crate::worker::build_worker(&dag_copy, worker_timeout, &state.config, state.concurrency)
            .map_err(|e| (-32000, format!("Failed to build worker: {}", e)))?;
    let mut scheduler = Scheduler::new(dag, worker, composite_sink, state.concurrency);

    // Shared completed_at flag for the spawned task
    let completed_at = Arc::new(Mutex::new(None::<Instant>));
    let completed_at_clone = Arc::clone(&completed_at);

    // Spawn the scheduler to run in the background (H1)
    let task_handle = tokio::spawn(async move {
        // Run the scheduler (this was the missing call!)
        // Note: allow_paid is hardcoded to false (safe-by-default). RPC/MCP clients cannot use paid models.
        // To use paid models, invoke pidag CLI directly or pass an explicit allowPaid parameter in a future version.
        let _result = scheduler.run(false).await;

        // Record completion time (H2)
        if let Ok(mut guard) = completed_at_clone.lock() {
            *guard = Some(Instant::now());
        }
    });

    // Store the run with only the handle and status, not the scheduler
    let mut runs = state
        .runs
        .lock()
        .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;
    runs.insert(
        dag_id.clone(),
        RunState {
            task_handle,
            dag_id: dag_id.clone(),
            completed_at,
        },
    );

    Ok(json!({
        "dagId": dag_id,
        "status": "submitted",
        "allowPaid": false
    }))
}

/// Get the status of a DAG run
pub async fn handle_dag_status(state: &ServerState, dag_id: &str) -> Result<Value, (i32, String)> {
    // Check if DAG exists
    {
        let runs = state
            .runs
            .lock()
            .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;

        if !runs.contains_key(dag_id) {
            return Err((-32001, "DAG not found".to_string()));
        }
    } // Lock is released here

    // Query store for real node states
    let nodes = state
        .store
        .list_nodes(dag_id)
        .await
        .map_err(|e| (-32000, format!("Failed to list nodes: {}", e)))?;

    let done_count = nodes.iter().filter(|n| n.state == NodeStatus::Done).count();
    let failed_count = nodes
        .iter()
        .filter(|n| n.state == NodeStatus::Failed)
        .count();

    let node_list: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "node_id": n.node_id,
                "state": n.state,
                "model": n.model
            })
        })
        .collect();

    Ok(json!({
        "dagId": dag_id,
        "nodes": node_list,
        "done": done_count,
        "failed": failed_count,
        "status": if failed_count > 0 { "failed" } else if done_count == nodes.len() && !nodes.is_empty() { "done" } else { "running" }
    }))
}

/// Get the results (artifacts) of a completed DAG
pub async fn handle_dag_result(state: &ServerState, dag_id: &str) -> Result<Value, (i32, String)> {
    // Check if DAG exists
    {
        let runs = state
            .runs
            .lock()
            .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;

        if !runs.contains_key(dag_id) {
            return Err((-32001, "DAG not found".to_string()));
        }
    } // Lock is released here

    // Query store for all node artifacts
    let nodes = state
        .store
        .list_nodes(dag_id)
        .await
        .map_err(|e| (-32000, format!("Failed to list nodes: {}", e)))?;

    let mut outputs = serde_json::Map::new();
    for node in nodes {
        if let Ok(Some(artifact)) = state.store.get_artifact(dag_id, &node.node_id).await {
            outputs.insert(node.node_id, Value::String(artifact));
        }
    }

    Ok(json!({ "outputs": Value::Object(outputs) }))
}

/// Retry a failed node.
/// H5: Retry is only supported for terminal runs. Active runs return an explicit error (-32002).
pub async fn handle_node_retry(
    state: &ServerState,
    dag_id: &str,
    node_id: &str,
) -> Result<Value, (i32, String)> {
    // Check if DAG exists and is terminal
    {
        let runs = state
            .runs
            .lock()
            .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;

        if let Some(run_state) = runs.get(dag_id) {
            // H5: Return error if run is still active
            if !run_state.is_terminal() {
                return Err((-32002, "Retry unsupported while run is active".to_string()));
            }
        } else {
            return Err((-32001, "DAG not found".to_string()));
        }
    } // Lock is released here

    // Mark node as Pending in the store
    let node_record = NodeRecord {
        node_id: node_id.to_string(),
        state: NodeStatus::Pending,
        model: None,
        attempt: 1,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    state
        .store
        .put_node_state(dag_id, node_id, &node_record)
        .await
        .map_err(|e| (-32000, format!("Failed to update node state: {}", e)))?;

    // Emit a NodeRetry event
    state
        .store
        .append_event(
            dag_id,
            &Event::NodeRetry {
                node_id: node_id.to_string(),
                reason: "Manual retry via RPC/MCP".to_string(),
            },
        )
        .await
        .map_err(|e| (-32000, format!("Failed to append event: {}", e)))?;

    Ok(json!({
        "state": "pending",
        "queued": true
    }))
}

/// Parse a resume token to extract dag_id and last_seq
pub fn parse_resume_token(token: &str) -> Result<(String, u64), String> {
    if !token.starts_with("resume:") {
        return Err("Invalid token format".to_string());
    }

    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() != 3 {
        return Err("Invalid token format".to_string());
    }

    let dag_id = parts[1].to_string();
    let last_seq: u64 = parts[2]
        .parse()
        .map_err(|_| "Invalid sequence number".to_string())?;

    Ok((dag_id, last_seq))
}

/// Generate a resume token for incomplete runs
pub fn generate_resume_token(dag_id: &str, last_seq: u64) -> String {
    format!("resume:{}:{}", dag_id, last_seq)
}

/// Generate a short UUID for run IDs
fn uuid_short() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = RandomState::new().build_hasher();
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO);

    hasher.write_u128(duration.as_nanos());

    format!("{:x}", hasher.finish()).chars().take(6).collect()
}

/// List all runs in the vault
pub async fn handle_list_runs(state: &ServerState) -> Result<Value, (i32, String)> {
    let runs = state
        .store
        .list_runs()
        .await
        .map_err(|e| (-32000, format!("Failed to list runs: {}", e)))?;

    let run_list: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({
                "run_id": r.run_id,
                "started_at": r.started_at,
                "completed_at": r.completed_at,
                "successful_nodes": r.successful_nodes,
                "failed_nodes": r.failed_nodes
            })
        })
        .collect();

    Ok(json!({ "runs": run_list }))
}

/// Cancel a running DAG.
/// H9: Aborts the spawned task, records terminal state, then returns success.
pub async fn handle_dag_cancel(state: &ServerState, dag_id: &str) -> Result<Value, (i32, String)> {
    let run_state = {
        let mut runs = state
            .runs
            .lock()
            .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;

        runs.remove(dag_id)
    };

    if let Some(run_state) = run_state {
        // Abort the spawned task (H9)
        run_state.task_handle.abort();

        // Record terminal state (H2)
        if let Ok(mut guard) = run_state.completed_at.lock() {
            *guard = Some(Instant::now());
        }

        Ok(json!({ "cancelled": true }))
    } else {
        Err((-32001, "DAG not found".to_string()))
    }
}

/// Handle dag.await: wait for DAG completion or timeout (H3, H4).
/// Shared by both RPC and MCP servers (N1).
/// Returns {done: true, status, done_count, failed_count} if terminal within timeout.
/// Returns {running: true, token: "resume:id:last_seq"} if still running after timeout.
pub async fn handle_dag_await(
    state: &ServerState,
    dag_id: &str,
    timeout_ms: u64,
    token: Option<&str>,
) -> Result<Value, (i32, String)> {
    // Check if DAG exists
    {
        let runs = state
            .runs
            .lock()
            .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;
        if !runs.contains_key(dag_id) {
            return Err((-32001, "DAG not found".to_string()));
        }
    }

    // Parse resume token if provided (H4)
    let resume_seq = if let Some(tok) = token {
        match parse_resume_token(tok) {
            Ok((parsed_dag_id, seq)) => {
                if parsed_dag_id != dag_id {
                    return Err((-32602, "Token dag_id mismatch".to_string()));
                }
                seq
            }
            Err(e) => return Err((-32602, format!("Malformed token: {}", e))),
        }
    } else {
        0
    };

    // Wait for completion with timeout
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    loop {
        // Check if terminal (release lock before awaiting)
        let is_terminal = {
            let runs = state
                .runs
                .lock()
                .map_err(|_| (-32000, "Failed to acquire lock on runs map".to_string()))?;
            runs.get(dag_id)
                .map(|run_state| run_state.is_terminal())
                .unwrap_or(false)
        };

        if is_terminal {
            // Terminal: return done with counts
            let nodes = state
                .store
                .list_nodes(dag_id)
                .await
                .map_err(|e| (-32000, format!("Failed to list nodes: {}", e)))?;

            let done_count = nodes.iter().filter(|n| n.state == NodeStatus::Done).count();
            let failed_count = nodes
                .iter()
                .filter(|n| n.state == NodeStatus::Failed)
                .count();
            let status = if failed_count > 0 { "failed" } else { "done" };

            return Ok(json!({
                "done": true,
                "status": status,
                "done_count": done_count,
                "failed_count": failed_count
            }));
        }

        // Check timeout
        if start.elapsed() > timeout {
            // Timeout: return running with resume token (H3)
            // Load events since resume_seq to get the last_seq
            let events = state
                .store
                .load_events_since(dag_id, resume_seq)
                .await
                .map_err(|e| (-32000, format!("Failed to load events: {}", e)))?;

            let last_seq = if events.is_empty() {
                resume_seq
            } else {
                events.last().map(|(seq, _)| *seq).unwrap_or(resume_seq)
            };

            let resume_token = generate_resume_token(dag_id, last_seq);
            return Ok(json!({
                "running": true,
                "token": resume_token
            }));
        }

        // Brief sleep before checking again
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

//! JSON-RPC 2.0 stdio server for pidag.

use super::handlers::{
    ServerState, handle_dag_await, handle_dag_cancel, handle_dag_result, handle_dag_status,
    handle_dag_submit, handle_node_retry,
};
use crate::core::Config;
use crate::core::error::PidagError;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};

/// Represents a JSON-RPC 2.0 response
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RpcResponse {
    jsonrpc: String,
    id: Value,
    result: Option<Value>,
    error: Option<RpcError>,
}

/// Represents a JSON-RPC 2.0 error
#[derive(Debug, Clone)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError { code, message }),
        }
    }

    fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        map.insert("id".to_string(), self.id.clone());

        if let Some(result) = &self.result {
            map.insert("result".to_string(), result.clone());
        } else if let Some(error) = &self.error {
            map.insert(
                "error".to_string(),
                json!({
                    "code": error.code,
                    "message": error.message,
                }),
            );
        }

        Value::Object(map)
    }
}

/// JSON-RPC 2.0 stdio server for pidag
pub struct RpcServer {
    state: Arc<ServerState>,
}

impl RpcServer {
    pub fn new(concurrency: usize, vault_path: PathBuf) -> Self {
        // Use RedbStorePool for concurrent access
        let store = Arc::new(crate::RedbStorePool::new(vault_path.clone()));

        // Load config from .pidag/config.toml if it exists
        let config_path = vault_path
            .parent()
            .map(|p| p.join("config.toml"))
            .unwrap_or_else(|| PathBuf::from(".pidag/config.toml"));
        let config = Config::load(&config_path).unwrap_or_default();

        let state = Arc::new(ServerState::new(vault_path, concurrency, store, config));
        Self { state }
    }

    /// Main server loop: read requests from stdin, dispatch, write responses to stdout
    pub async fn run(&mut self) -> Result<(), PidagError> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let stdout = tokio::io::stdout();
        let stdout = Arc::new(Mutex::new(stdout));

        // Channel for sending responses from worker tasks to the output writer
        let (tx, mut rx) = mpsc::unbounded_channel::<RpcResponse>();

        // Spawn output writer task
        let stdout_clone = Arc::clone(&stdout);
        tokio::spawn(async move {
            while let Some(response) = rx.recv().await {
                let json_str = serde_json::to_string(&response.to_json())
                    .unwrap_or_else(|_| json!({"error": "serialization failed"}).to_string());

                {
                    let mut out = stdout_clone.lock().await;
                    let _ = out.write_all(json_str.as_bytes()).await;
                    let _ = out.write_all(b"\n").await;
                    let _ = out.flush().await;
                }
            }
        });

        // Spawn cleanup task for completed runs (H6)
        let runs = Arc::clone(&self.state.runs);
        let ttl_secs = self.state.config.rpc.completed_run_ttl_secs;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let mut runs_map = runs.lock().unwrap_or_else(|e| e.into_inner());
                // Remove completed runs older than configured TTL (default 60 seconds)
                let cutoff = Instant::now() - Duration::from_secs(ttl_secs);
                runs_map.retain(|_, run_state| match run_state.completed_at.lock() {
                    Ok(guard) => match *guard {
                        Some(completed) => completed > cutoff,
                        None => true,
                    },
                    Err(_) => true,
                });
            }
        });

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    // EOF on stdin
                    eprintln!("[rpc] stdin closed, shutting down");
                    break;
                }
                Ok(_) => {
                    let state_clone = Arc::clone(&self.state);
                    let tx_clone = tx.clone();
                    let line_clone = line.clone();

                    // Spawn handler as a task for concurrent processing
                    tokio::spawn(async move {
                        let response = handle_line_async(&state_clone, &line_clone).await;
                        let _ = tx_clone.send(response);
                    });
                }
                Err(e) => {
                    eprintln!("[rpc] read error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}

async fn handle_line_async(state: &ServerState, line: &str) -> RpcResponse {
    let line = line.trim();
    if line.is_empty() {
        return RpcResponse::error(Value::Null, -32600, "Invalid Request".to_string());
    }

    let request = match parse_request(line) {
        Ok(req) => req,
        Err(msg) => return RpcResponse::error(Value::Null, -32700, msg),
    };

    eprintln!("[rpc] method={} id={}", request.method, request.id);

    match request.method.as_str() {
        "dag.submit" => match request.params.get("dag") {
            Some(dag_json) => match handle_dag_submit(state, dag_json).await {
                Ok(result) => RpcResponse::success(request.id.clone(), result),
                Err((code, msg)) => RpcResponse::error(request.id.clone(), code, msg),
            },
            None => RpcResponse::error(request.id.clone(), -32602, "Missing dag param".to_string()),
        },
        "dag.status" => match request.params.get("dagId").and_then(|v| v.as_str()) {
            Some(dag_id) => match handle_dag_status(state, dag_id).await {
                Ok(result) => RpcResponse::success(request.id.clone(), result),
                Err((code, msg)) => RpcResponse::error(request.id.clone(), code, msg),
            },
            None => RpcResponse::error(request.id.clone(), -32602, "Missing dagId".to_string()),
        },
        "dag.await" => {
            // H3, H4: Await DAG completion with timeout and optional resume token
            match request.params.get("dagId").and_then(|v| v.as_str()) {
                Some(dag_id) => {
                    let timeout_ms = request
                        .params
                        .get("timeout_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(30000); // Default 30 seconds
                    let token = request.params.get("token").and_then(|v| v.as_str());

                    match handle_dag_await(state, dag_id, timeout_ms, token).await {
                        Ok(result) => RpcResponse::success(request.id.clone(), result),
                        Err((code, msg)) => RpcResponse::error(request.id.clone(), code, msg),
                    }
                }
                None => RpcResponse::error(request.id.clone(), -32602, "Missing dagId".to_string()),
            }
        }
        "dag.result" => match request.params.get("dagId").and_then(|v| v.as_str()) {
            Some(dag_id) => match handle_dag_result(state, dag_id).await {
                Ok(result) => RpcResponse::success(request.id.clone(), result),
                Err((code, msg)) => RpcResponse::error(request.id.clone(), code, msg),
            },
            None => RpcResponse::error(request.id.clone(), -32602, "Missing dagId".to_string()),
        },
        "node.retry" => {
            match (
                request.params.get("dagId").and_then(|v| v.as_str()),
                request.params.get("nodeId").and_then(|v| v.as_str()),
            ) {
                (Some(dag_id), Some(node_id)) => {
                    match handle_node_retry(state, dag_id, node_id).await {
                        Ok(result) => RpcResponse::success(request.id.clone(), result),
                        Err((code, msg)) => RpcResponse::error(request.id.clone(), code, msg),
                    }
                }
                _ => RpcResponse::error(
                    request.id.clone(),
                    -32602,
                    "Missing dagId or nodeId".to_string(),
                ),
            }
        }
        "dag.cancel" => match request.params.get("dagId").and_then(|v| v.as_str()) {
            Some(dag_id) => match handle_dag_cancel(state, dag_id).await {
                Ok(result) => RpcResponse::success(request.id.clone(), result),
                Err((code, msg)) => RpcResponse::error(request.id.clone(), code, msg),
            },
            None => RpcResponse::error(request.id.clone(), -32602, "Missing dagId".to_string()),
        },
        "node.wait" => {
            // Stub endpoint
            RpcResponse::success(request.id.clone(), json!({"status": "not implemented"}))
        }
        "health" => RpcResponse::success(
            request.id.clone(),
            json!({
                "ok": true,
                "version": "0.1.0"
            }),
        ),
        "shutdown" => {
            eprintln!("[rpc] shutdown requested");
            std::process::exit(0);
        }
        _ => RpcResponse::error(request.id.clone(), -32601, "Method not found".to_string()),
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RpcRequest {
    jsonrpc: String,
    id: Value,
    method: String,
    params: Value,
}

fn parse_request(line: &str) -> Result<RpcRequest, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| format!("Parse error: {}", e))?;

    let obj = value.as_object().ok_or("Request must be an object")?;

    let jsonrpc = obj
        .get("jsonrpc")
        .and_then(|v| v.as_str())
        .ok_or("Missing jsonrpc")?;

    if jsonrpc != "2.0" {
        return Err("jsonrpc must be '2.0'".to_string());
    }

    let id = obj.get("id").ok_or("Missing id")?;
    let method = obj
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or("Missing method")?;
    let params = obj
        .get("params")
        .unwrap_or(&Value::Object(Default::default()))
        .clone();

    Ok(RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: id.clone(),
        method: method.to_string(),
        params,
    })
}

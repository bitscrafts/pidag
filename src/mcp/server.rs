//! MCP (Model Context Protocol) server for pidag
//! Exposes pidag operations as MCP tools over stdio transport
//! Compatible with Claude Code and other MCP clients

use crate::core::Config;
use crate::rpc::{
    ServerState, handle_dag_await, handle_dag_cancel, handle_dag_result, handle_dag_status,
    handle_dag_submit, handle_list_runs, handle_node_retry,
};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct McpServer {
    state: Arc<ServerState>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl McpServer {
    #[tool(description = "Submit a DAG for execution")]
    async fn pidag_submit_dag(&self, Parameters(input): Parameters<SubmitDagInput>) -> String {
        match handle_dag_submit(&self.state, &input.dag).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }

    #[tool(description = "Await DAG completion or timeout")]
    async fn pidag_await(&self, Parameters(input): Parameters<AwaitDagInput>) -> String {
        let timeout_ms = input.timeout_ms.unwrap_or(30000);
        let token = input.token.as_deref();
        match handle_dag_await(&self.state, &input.dag_id, timeout_ms, token).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }

    #[tool(description = "Get current DAG status and node states")]
    async fn pidag_status(&self, Parameters(input): Parameters<StatusInput>) -> String {
        match handle_dag_status(&self.state, &input.dag_id).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }

    #[tool(description = "List all runs in the vault")]
    async fn pidag_list_runs(&self, Parameters(_input): Parameters<ListRunsInput>) -> String {
        match handle_list_runs(&self.state).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }

    #[tool(description = "Cancel a running DAG")]
    async fn pidag_cancel(&self, Parameters(input): Parameters<CancelInput>) -> String {
        match handle_dag_cancel(&self.state, &input.dag_id).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }

    #[tool(description = "Get all node artifacts/outputs for a completed DAG")]
    async fn pidag_result(&self, Parameters(input): Parameters<ResultInput>) -> String {
        match handle_dag_result(&self.state, &input.dag_id).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }

    #[tool(description = "Retry a failed node")]
    async fn pidag_node_retry(&self, Parameters(input): Parameters<NodeRetryInput>) -> String {
        match handle_node_retry(&self.state, &input.dag_id, &input.node_id).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            Err((code, msg)) => {
                serde_json::to_string(&json!({"error": {"code": code, "message": msg}}))
                    .unwrap_or_else(|_| "{}".to_string())
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

// Tool input schemas
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SubmitDagInput {
    pub dag: Value,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AwaitDagInput {
    pub dag_id: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct StatusInput {
    pub dag_id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListRunsInput {}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CancelInput {
    pub dag_id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ResultInput {
    pub dag_id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct NodeRetryInput {
    pub dag_id: String,
    pub node_id: String,
}

/// Run the MCP server
pub async fn run_mcp_server(
    vault_path: PathBuf,
    concurrency: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[mcp] starting MCP stdio server");
    eprintln!("[mcp] vault: {}", vault_path.display());
    eprintln!("[mcp] concurrency: {}", concurrency);

    // Create vault directory if needed
    if let Some(parent) = vault_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create vault directory: {}", e))?;
    }

    // Open store (use RedbStorePool for concurrent access)
    let store = Arc::new(crate::RedbStorePool::new(vault_path.clone()));

    // Load config from .pidag/config.toml if it exists
    let config_path = vault_path
        .parent()
        .map(|p| p.join("config.toml"))
        .unwrap_or_else(|| PathBuf::from(".pidag/config.toml"));
    let config = Config::load(&config_path).unwrap_or_default();

    // Create server state
    let state = Arc::new(ServerState::new(vault_path, concurrency, store, config));

    // Create and run MCP server
    let mcp_server = McpServer::new(state.clone());

    // Spawn cleanup task (H6: use configured TTL)
    let runs = Arc::clone(&state.runs);
    let ttl_secs = state.config.rpc.completed_run_ttl_secs;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let mut runs_map = runs.lock().unwrap_or_else(|e| e.into_inner());
            // Remove completed runs older than configured TTL (default 60 seconds)
            let cutoff = std::time::Instant::now() - Duration::from_secs(ttl_secs);
            runs_map.retain(|_, run_state| match run_state.completed_at.lock() {
                Ok(guard) => match *guard {
                    Some(completed) => completed > cutoff,
                    None => true,
                },
                Err(_) => true,
            });
        }
    });

    let service = mcp_server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

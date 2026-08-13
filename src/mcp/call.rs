//! Worker that executes MCP (Model Context Protocol) tool calls
//!
//! Routes nodes with `mcp_call` field to MCP servers via the MCP client.

use super::client::McpClient;
use crate::core::dag::{Dag, McpCallConfig};
use crate::core::error::PidagError;
use crate::worker::{Worker, WorkerOutput};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct McpCallWorker {
    mcp_calls: HashMap<String, McpCallConfig>,
    client: Arc<Mutex<McpClient>>,
}

impl McpCallWorker {
    /// Create a new MCP call worker from a DAG
    pub fn new(dag: &Dag) -> Self {
        let mcp_calls = dag
            .nodes
            .iter()
            .filter_map(|n| {
                n.mcp_call
                    .as_ref()
                    .map(|config| (n.id.clone(), config.clone()))
            })
            .collect();

        Self {
            mcp_calls,
            client: Arc::new(Mutex::new(McpClient::new())),
        }
    }

    /// Check if a node has an MCP call configuration
    pub fn is_mcp_node(&self, node_id: &str) -> bool {
        self.mcp_calls.contains_key(node_id)
    }
}

#[async_trait]
impl Worker for McpCallWorker {
    async fn run(
        &self,
        node_id: &str,
        _prompt: &str,
        _model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        let mcp_call = self.mcp_calls.get(node_id).ok_or_else(|| {
            PidagError::Validation(format!("node {} missing mcp_call config", node_id))
        })?;

        // Get MCP client and call the tool
        let mut client = self.client.lock().await;

        // Call the tool on the MCP server
        let result = client.call_tool(&mcp_call.server, mcp_call, None).await?;

        // Convert result to string output
        let output = serde_json::to_string(&result).unwrap_or_else(|_| format!("{:?}", result));

        Ok(WorkerOutput {
            success: true,
            output,
            retryable: false,
            usage: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dag::{Dag, ModelRef, Node, RetryPolicy};

    #[test]
    fn test_mcp_call_worker_construction() {
        let dag = Dag {
            nodes: vec![],
            metadata: None,
        };
        let _worker = McpCallWorker::new(&dag);
        // Constructor should succeed
    }

    #[test]
    fn test_mcp_call_worker_identifies_mcp_nodes() {
        let mcp_call = McpCallConfig {
            server: "http://localhost:7421/mcp".to_string(),
            transport: "http".to_string(),
            tool: "search_memory".to_string(),
            arguments: serde_json::Map::new(),
        };

        let dag = Dag {
            nodes: vec![Node {
                id: "mcp-node".to_string(),
                prompt: "test".to_string(),
                depends_on: vec![],
                models: vec![ModelRef {
                    name: "mcp".to_string(),
                    paid: false,
                }],
                retry: RetryPolicy {
                    attempts: 1,
                    backoff_ms: 0,
                },
                validate: None,
                node_type: None,
                gate: None,
                timeout: None,
                mcp_call: Some(mcp_call),
                after: vec![],
                verify: None,
                verify_pre: None,

                for_each: None,
                quorum: None,
            }],
            metadata: None,
        };

        let worker = McpCallWorker::new(&dag);
        assert!(worker.is_mcp_node("mcp-node"));
        assert!(!worker.is_mcp_node("other-node"));
    }
}

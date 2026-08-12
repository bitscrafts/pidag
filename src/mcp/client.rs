//! MCP (Model Context Protocol) client for pidag nodes
//!
//! Enables nodes with `mcp_call` field to invoke tools on external MCP servers.
//! Supports both HTTP (Streamable HTTP) and stdio transports.

use crate::core::dag::McpCallConfig;
use crate::core::error::PidagError;
use serde_json::{Value, json};
use std::collections::HashMap;

/// MCP client for invoking tools on remote MCP servers
pub struct McpClient {
    /// Cached tool lists per server URL
    tools_cache: HashMap<String, Vec<ToolInfo>>,
}

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: Option<String>,
}

impl McpClient {
    pub fn new() -> Self {
        Self {
            tools_cache: HashMap::new(),
        }
    }

    /// List tools available on an MCP server
    pub async fn list_tools(&mut self, server: &str) -> Result<Vec<ToolInfo>, PidagError> {
        // Check cache first
        if let Some(tools) = self.tools_cache.get(server) {
            return Ok(tools.clone());
        }

        // Query the server (HTTP transport only for MVP)
        let tools = self.list_tools_http(server).await?;
        self.tools_cache.insert(server.to_string(), tools.clone());
        Ok(tools)
    }

    /// Call a tool on an MCP server
    pub async fn call_tool(
        &mut self,
        server: &str,
        config: &McpCallConfig,
        _node_input: Option<String>,
    ) -> Result<Value, PidagError> {
        match config.transport.as_str() {
            "http" => self.call_tool_http(server, config, _node_input).await,
            "stdio" => self.call_tool_stdio(server, config, _node_input).await,
            _ => Err(PidagError::Validation(format!(
                "unsupported MCP transport: {}",
                config.transport
            ))),
        }
    }

    /// Call tool via HTTP Streamable HTTP transport
    async fn call_tool_http(
        &mut self,
        server: &str,
        config: &McpCallConfig,
        _node_input: Option<String>,
    ) -> Result<Value, PidagError> {
        // HTTP transport: POST to server URL with MCP tool call
        // Request format: JSON-RPC 2.0 style
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": config.tool,
                "arguments": config.arguments
            }
        });

        // Use curl to POST to the server
        let output = tokio::process::Command::new("curl")
            .arg("-s")
            .arg("-X")
            .arg("POST")
            .arg(server)
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-d")
            .arg(serde_json::to_string(&request).unwrap_or_default())
            .output()
            .await
            .map_err(|e| PidagError::WorkerSpawn(format!("curl failed: {}", e)))?;

        if !output.status.success() {
            return Err(PidagError::Validation(format!(
                "MCP server returned error: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let response: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| PidagError::Parse(format!("failed to parse MCP response: {}", e)))?;

        // Extract result or error
        if let Some(result) = response.get("result") {
            Ok(result.clone())
        } else if let Some(error) = response.get("error") {
            Err(PidagError::Validation(format!(
                "MCP tool error: {}",
                error
                    .get("message")
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            )))
        } else {
            Err(PidagError::Parse(
                "no result or error in MCP response".to_string(),
            ))
        }
    }

    /// Call tool via stdio transport (spawn subprocess)
    async fn call_tool_stdio(
        &mut self,
        _server: &str,
        _config: &McpCallConfig,
        _node_input: Option<String>,
    ) -> Result<Value, PidagError> {
        // TODO: Implement stdio transport
        // 1. Spawn subprocess (path from server URL)
        // 2. Write MCP init request to stdin
        // 3. Write tool call request
        // 4. Read response from stdout
        Err(PidagError::Validation(
            "stdio transport not yet implemented".to_string(),
        ))
    }

    /// List tools via HTTP
    async fn list_tools_http(&self, server: &str) -> Result<Vec<ToolInfo>, PidagError> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        });

        let output = tokio::process::Command::new("curl")
            .arg("-s")
            .arg("-X")
            .arg("POST")
            .arg(server)
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-d")
            .arg(serde_json::to_string(&request).unwrap_or_default())
            .output()
            .await
            .map_err(|e| PidagError::WorkerSpawn(format!("curl failed: {}", e)))?;

        if !output.status.success() {
            return Err(PidagError::Validation(format!(
                "MCP server returned error: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let response: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| PidagError::Parse(format!("failed to parse MCP response: {}", e)))?;

        // Extract tools from result
        if let Some(result) = response.get("result")
            && let Some(tools_array) = result.get("tools").and_then(|v| v.as_array())
        {
            let tools = tools_array
                .iter()
                .filter_map(|tool| {
                    tool.get("name")
                        .and_then(|name| name.as_str())
                        .map(|name| ToolInfo {
                            name: name.to_string(),
                            description: tool
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(|s| s.to_string()),
                        })
                })
                .collect();
            return Ok(tools);
        }

        Err(PidagError::Parse("no tools in MCP response".to_string()))
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_client_construction() {
        let client = McpClient::new();
        assert!(client.tools_cache.is_empty());
    }
}

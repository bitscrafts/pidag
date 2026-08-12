# mcp - MCP (Model Context Protocol) Module

This module provides MCP protocol support for pidag, enabling:
- Exposure of pidag operations as MCP tools (server mode)
- Invocation of external MCP tools from DAG nodes (client mode)

## Module Structure

```
mcp/
├── mod.rs        # Module exports
├── server.rs     # McpServer - exposes pidag ops as MCP tools
├── client.rs     # McpClient - invokes tools on external MCP servers
├── call.rs       # McpCallWorker - worker for nodes with mcp_call field
└── README.md     # This file
```

## Components

### McpServer (`server.rs`)

Exposes pidag operations as MCP tools over stdio transport, compatible with
Claude Code and other MCP clients.

**Available Tools:**
- `pidag_submit_dag` - Submit a DAG for execution
- `pidag_await` - Await DAG completion or timeout
- `pidag_status` - Get current DAG status and node states
- `pidag_list_runs` - List all runs in the vault
- `pidag_cancel` - Cancel a running DAG
- `pidag_result` - Get all node artifacts/outputs for a completed DAG
- `pidag_node_retry` - Retry a failed node

### McpClient (`client.rs`)

Client for invoking tools on external MCP servers. Supports HTTP (Streamable
HTTP) transport, with stdio transport planned.

### McpCallWorker (`call.rs`)

Worker implementation that enables DAG nodes to invoke MCP tools. When a node
has an `mcp_call` field, the `TypeDispatchWorker` routes it to this worker.

**Node Configuration:**
```json
{
  "id": "search-memory",
  "prompt": "not used",
  "depends_on": [],
  "models": [{"name": "mcp", "paid": false}],
  "retry": {"attempts": 1, "backoff_ms": 0},
  "mcp_call": {
    "server": "http://localhost:7421/mcp",
    "transport": "http",
    "tool": "search_memory",
    "arguments": {"query": "test", "k": 5}
  }
}
```

## Usage

### Running the MCP Server

```bash
pidag mcp --vault .pidag/pidag.redb --concurrency 4
```

### Programmatic Usage

```rust
use pidag::mcp::{run_mcp_server, McpServer, McpClient};
use std::path::PathBuf;

// Run MCP server
run_mcp_server(PathBuf::from(".pidag/pidag.redb"), 4).await?;

// Use MCP client
let mut client = McpClient::new();
let tools = client.list_tools("http://localhost:7421/mcp").await?;
```

## Protocol

The MCP server implements the Model Context Protocol (MCP) over stdio
transport. Requests and responses follow the MCP specification with JSON-RPC
2.0 framing.

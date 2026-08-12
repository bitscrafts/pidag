# rpc - JSON-RPC 2.0 Server Module

This module provides the JSON-RPC 2.0 server for pidag, exposing DAG
operations over stdio transport.

## Module Structure

```
rpc/
├── mod.rs        # Module exports
├── server.rs     # RpcServer - JSON-RPC server over stdio
├── handlers.rs   # Shared handler logic for DAG operations
└── README.md     # This file
```

## Components

### RpcServer (`server.rs`)

JSON-RPC 2.0 server that reads requests from stdin and writes responses to
stdout. Handles concurrent request processing with a cleanup task for
completed runs.

**Supported Methods:**
- `dag.submit` - Submit a DAG for execution
- `dag.status` - Get current DAG status and node states
- `dag.await` - Await DAG completion with resume token
- `dag.result` - Get all node artifacts/outputs
- `dag.cancel` - Cancel a running DAG
- `node.retry` - Retry a failed node
- `node.wait` - Wait for a specific node
- `health` - Health check
- `shutdown` - Graceful shutdown

### ServerState (`handlers.rs`)

Shared state for both RPC and MCP servers, containing:
- Active runs map with Scheduler instances
- Vault path for persistent storage
- Concurrency settings
- Store instance

### Handler Functions (`handlers.rs`)

Core business logic for DAG operations, shared between RPC and MCP servers:
- `handle_dag_submit` - Parse, validate, and execute a DAG
- `handle_dag_status` - Query node states from store
- `handle_dag_result` - Retrieve all node artifacts
- `handle_dag_cancel` - Cancel and remove a run
- `handle_node_retry` - Mark node as pending for retry
- `handle_list_runs` - List all runs in vault

## Usage

### Running the RPC Server

```bash
pidag serve --vault .pidag/pidag.redb --concurrency 4
```

### Request Format

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "dag.submit",
  "params": {
    "dag": {
      "nodes": [...]
    }
  }
}
```

### Response Format

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "dagId": "run-20260801-123456-abc123",
    "status": "submitted"
  }
}
```

### Error Response

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "Invalid DAG JSON: ..."
  }
}
```

## Programmatic Usage

```rust
use pidag::rpc::RpcServer;
use std::path::PathBuf;

let mut server = RpcServer::new(4, PathBuf::from(".pidag/pidag.redb"));
server.run().await?;
```

## Error Codes

| Code | Meaning |
|------|---------|
| -32700 | Parse error |
| -32600 | Invalid request |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32000 | Server error |
| -32001 | DAG not found |

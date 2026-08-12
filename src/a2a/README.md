# a2a - A2A (Agent-to-Agent) Protocol Module

This module provides A2A protocol support for pidag, enabling:
- HTTP server exposing A2A protocol endpoints for external clients
- Worker for dispatching DAG nodes to A2A-compliant remote agents

## Module Structure

```
a2a/
├── mod.rs        # Module exports
├── server.rs     # A2A HTTP server with tasks/send, tasks/get endpoints
├── worker.rs     # A2aWorker - dispatches nodes to A2A remote agents
└── README.md     # This file
```

## Components

### A2A Server (`server.rs`)

HTTP server exposing A2A protocol endpoints:

**Endpoints:**
- `GET /.well-known/agent-card.json` - Agent Card discovery
- `POST /tasks/send` - Submit a DAG task for execution
- `GET /tasks/get?id=<task_id>` - Query task status

**Agent Card:**
```json
{
  "name": "pidag",
  "version": "0.1.0",
  "description": "Deterministic, resilient multi-node LLM DAG executor",
  "capabilities": ["dag_execution", "sdd_spec_processing"],
  "modalities": ["text", "json"],
  "streaming": false
}
```

### A2aWorker (`worker.rs`)

Worker that dispatches DAG `llm` nodes to any A2A-compliant remote agent
(Gemini, Claude, Hermes, Orki, etc.) via the `tasks/send` JSON-RPC protocol.

**Routing:** When a `ModelRef.name` starts with `http://` or `https://`, the
`TypeDispatchWorker` routes the node to `A2aWorker` instead of `PiPrintWorker`.

**URL Fragment for Skills:** The URL fragment (`#skill`) optionally selects an
A2A skill via the `skillId` JSON-RPC param:
- `http://localhost:8080#search` -> skill "search"
- `http://localhost:8080` -> no skill specified

**Example Node:**
```json
{
  "id": "remote-agent",
  "prompt": "Summarize this document",
  "depends_on": [],
  "models": [
    {"name": "http://hermes:7422/v1#summarize", "paid": false}
  ],
  "retry": {"attempts": 2, "backoff_ms": 5000}
}
```

## Usage

### As Part of pidag UI Server

The A2A server is typically mounted alongside the pidag UI:

```rust
use pidag::a2a::{router, A2aServerState, AgentCard};
use std::sync::Arc;

let a2a_state = Arc::new(A2aServerState {
    inner: server_state,
    agent_card: AgentCard::default(),
});
let a2a_router = router(a2a_state);
```

### Sending Nodes to A2A Agents

Simply use an HTTP URL as the model name:

```json
{
  "nodes": [
    {
      "id": "ask-hermes",
      "prompt": "What is the capital of France?",
      "models": [{"name": "http://hermes:7422/v1", "paid": false}]
    }
  ]
}
```

## Protocol

The A2A worker implements the Agent-to-Agent protocol with:
- JSON-RPC 2.0 `tasks/send` for task submission
- JSON-RPC 2.0 `tasks/get` for polling task state
- Task lifecycle: `submitted` -> `working` -> `completed`/`failed`
- Artifact extraction from response `artifacts[0].parts[0].text`

## Error Handling

HTTP errors (429, 503, quota errors) are classified as retryable via
`classify_retryable()`, enabling the scheduler to advance to the next model
in the chain or apply exponential backoff.

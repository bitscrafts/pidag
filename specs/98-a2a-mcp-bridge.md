# Spec: pidag A2A Server + MCP Client Mode

**Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
**Topic**: `claude-pi-delegation`
**Date**: 2026-08-04
**Status**: APPROVED — ready for implementation
**Priority**: P1 (MCP server stabilized, proceed with A2A+MCP bridge)

---

## Overview

Extend pidag to serve as both:
1. **A2A Server** — external A2A clients can discover pidag and send tasks
2. **MCP Client** — pidag nodes can invoke tools on external MCP servers

This enables pidag to participate in the emerging multi-agent ecosystem where
A2A handles agent-to-agent coordination and MCP handles agent-to-tool access.

**Why**: The 2026 enterprise agent stack uses both protocols:
- A2A routes tasks to the right agent
- MCP gives that agent the context/tools it needs

**References**:
- [A2A Protocol](https://a2a-protocol.org/)
- [A2A GitHub](https://github.com/google/A2A)
- [MCP Spec](https://spec.modelcontextprotocol.io/)
- [A2A + MCP Integration Analysis](https://arxiv.org/pdf/2505.03864)

---

## Requirements

### A2A Server Mode

**R1** (Agent Card): Serve `/.well-known/agent-card.json` at HTTP endpoint with:
- `name`: "pidag"
- `version`: "0.1.0"
- `description`: "Deterministic, resilient multi-node LLM DAG executor"
- `endpoint`: configurable URL
- `capabilities`: ["dag_execution", "sdd_spec_processing"]
- `modalities`: ["text", "json"]
- `auth`: optional API key or none (local-only default)

**R2** (Task Receive): Accept `tasks/send` JSON-RPC 2.0 method:
- Parse task content as DAG JSON or SDD spec path
- Create internal run via existing handlers
- Return task ID

**R3** (Task Status): Implement `tasks/get` to return task lifecycle state:
- Map pidag run states to A2A states: Pending→Submitted, Running→Working,
  Done→Completed, Failed→Failed

**R4** (Task Artifacts): Return node outputs as A2A artifacts on completion.

**R5** (Streaming): Support SSE for task status updates (optional, P3).

### MCP Client Mode

**R6** (Node MCP Calls): Add `mcp_call` field to Node schema:
```json
{
  "id": "fetch-context",
  "mcp_call": {
    "server": "http://${DEPLOY_HOST_NAME}:7421/mcp",
    "tool": "search_memory",
    "arguments": { "query": "{{input}}", "k": 5 }
  }
}
```

**R7** (MCP Transport): Support both:
- stdio (spawn child process)
- Streamable HTTP (POST to endpoint)

**R8** (Tool Discovery): Cache `tools/list` response per MCP server.

**R9** (Error Handling): Map MCP errors to node failure with retry policy.

### Bidirectional Bridge

**R10** (A2A→MCP): A2A tasks can specify target MCP server; pidag acts as bridge:
```json
{
  "task": {
    "type": "mcp_proxy",
    "server": "http://${DEPLOY_HOST_NAME}:7420/mcp",
    "tool": "store_insight",
    "arguments": { ... }
  }
}
```

**R11** (MCP→A2A): MCP tool `pidag_delegate_a2a` sends task to external A2A agent.

---

## Architecture

```
                    ┌─────────────────────────────────────┐
                    │            pidag                     │
                    │                                      │
     A2A Clients    │  ┌──────────────────────────────┐   │   MCP Servers
         │          │  │       A2A Server              │   │       │
         ▼          │  │  - Agent Card                 │   │       │
   tasks/send ────────▶│  - tasks/send → handlers.rs   │   │       │
   tasks/get  ◀───────│  - tasks/get → run status      │   │       │
                    │  └──────────────────────────────┘   │       │
                    │                │                     │       │
                    │                ▼                     │       │
                    │  ┌──────────────────────────────┐   │       │
                    │  │       Core Handlers           │   │       │
                    │  │  - handle_dag_submit          │   │       │
                    │  │  - handle_dag_status          │   │       │
                    │  └──────────────────────────────┘   │       │
                    │                │                     │       │
                    │                ▼                     │       │
     MCP Clients    │  ┌──────────────────────────────┐   │       │
         │          │  │       MCP Server (stdio)      │   │       │
         ▼          │  │  - pidag_submit_dag           │◀──┼───────┤
   tools/call ────────▶│  - pidag_await                │   │       │
   tools/list ◀───────│  - pidag_delegate_a2a (NEW)   │   │       │
                    │  └──────────────────────────────┘   │       │
                    │                │                     │       │
                    │                ▼                     │       │
                    │  ┌──────────────────────────────┐   │       │
                    │  │       MCP Client (NEW)        │   │       │
                    │  │  - Node.mcp_call execution    │───┼──────▶│
                    │  │  - Tool result → node output  │   │  tools/call
                    │  └──────────────────────────────┘   │
                    │                │                     │
                    │                ▼                     │
                    │  ┌──────────────────────────────┐   │   A2A Agents
                    │  │       A2A Client (EXISTS)     │   │       │
                    │  │  - A2aWorker                  │───┼──────▶│
                    │  │  - curl + tasks/send          │   │  tasks/send
                    │  └──────────────────────────────┘   │
                    └─────────────────────────────────────┘
```

### File Changes

| File | Change |
|------|--------|
| `src/a2a_server.rs` (NEW) | A2A HTTP server with Agent Card + task endpoints |
| `src/mcp_client.rs` (NEW) | MCP client for node mcp_call execution |
| `src/dag.rs` | Add `mcp_call` field to Node |
| `src/worker/mcp_call.rs` (NEW) | Worker that executes MCP tool calls |
| `src/mcp_server.rs` | Add `pidag_delegate_a2a` tool |
| `src/bin/pidag.rs` | Add `a2a` subcommand |
| `Cargo.toml` | Add `rmcp` client features, `axum` for A2A HTTP |

---

## TDD Contract

| # | Test name | Given | Expects |
|---|---|---|---|
| B1 | `test_agent_card_served` | GET `/.well-known/agent-card.json` | Returns valid Agent Card JSON |
| B2 | `test_a2a_tasks_send_dag` | POST `tasks/send` with DAG JSON | Returns task ID, run created |
| B3 | `test_a2a_tasks_get_status` | Submit task, GET `tasks/get` | Returns correct lifecycle state |
| B4 | `test_a2a_task_completed_artifacts` | Run completes | `tasks/get` returns artifacts |
| B5 | `test_mcp_client_tool_call` | Node with `mcp_call` field | Tool invoked, result in output |
| B6 | `test_mcp_client_error_handling` | MCP server returns error | Node fails with error message |
| B7 | `test_a2a_to_mcp_proxy` | A2A task with `mcp_proxy` type | MCP tool invoked via bridge |
| B8 | `test_pidag_delegate_a2a_tool` | MCP `pidag_delegate_a2a` call | A2A task sent to external agent |

---

## Exit Criteria

- [ ] `cargo test -p pidag 2>&1 | grep -q "0 failed"`
- [ ] `cargo clippy -p pidag -- -D warnings` clean
- [ ] All 8 tests in TDD Contract pass (B1-B8)
- [ ] `pidag a2a --port 8080` serves Agent Card at `/.well-known/agent-card.json`
- [ ] A2A `tasks/send` creates pidag run and returns task ID
- [ ] A2A `tasks/get` returns correct state mapping
- [ ] Node `mcp_call` field invokes external MCP server
- [ ] `pidag_delegate_a2a` MCP tool sends task to A2A agent
- [ ] No new deps except `axum` (A2A HTTP) — `rmcp` already present

---

## Guardrails

- Do not modify existing MCP server behavior (additive only)
- Do not change A2aWorker (existing A2A client) — this spec adds A2A SERVER
- Do not implement authentication in Phase 1 (local-only default)
- Do not implement SSE streaming (Phase 2)
- On ambiguity — stop and report to loop-engineer

---

## Implementation Notes

### Agent Card Example

```json
{
  "name": "pidag",
  "version": "0.1.0",
  "description": "Deterministic, resilient multi-node LLM DAG executor",
  "endpoint": "http://localhost:8080",
  "capabilities": ["dag_execution", "sdd_spec_processing"],
  "modalities": ["text", "json"],
  "authentication": {
    "type": "none"
  },
  "streaming": false
}
```

### A2A Task to pidag Run Mapping

| A2A Task State | pidag Run State |
|----------------|-----------------|
| submitted | Run created, nodes Pending |
| working | Any node Running |
| input-required | (not used) |
| completed | All nodes Done |
| failed | Any node Failed |
| canceled | Run removed from registry |
| rejected | Invalid DAG JSON |

### MCP Client Node Schema

```json
{
  "id": "enrich-context",
  "prompt": "not used when mcp_call present",
  "mcp_call": {
    "server": "http://${DEPLOY_HOST_NAME}:7421/mcp",
    "transport": "http",
    "tool": "search_memory",
    "arguments": {
      "query": "{{previous_node_output}}",
      "k": 10
    }
  },
  "depends_on": ["previous-node"]
}
```

# Spec: pidag Production Hardening + MCP Server Mode

**Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
**Topic**: `claude-pi-delegation`
**Date**: 2026-08-04
**Status**: SPEC READY — implementation pending
**Priority**: P0 correctness + P1 robustness

---

## Overview

Harden pidag for production use by fixing the remaining P0/P1 issues from the
2026-08-01 architectural review, and add an optional MCP server mode so Claude
Code can invoke pidag as a tool.

**Why now**: pidag is the execution engine for all SDD specs. Production
hardening ensures DAG runs are crash-safe, the RPC interface is reliable, and
the MCP server mode enables Claude Code integration without a separate proxy.

**In scope**:
- P0-3: Implement real logic for RPC stub endpoints (`dag.result`, `node.retry`, `dag.status`)
- P0-4: Implement functional resume tokens for `dag.await`
- P1-4: Make RPC server non-blocking (spawn handlers as tasks)
- P1-5: Add cleanup for completed runs in `RpcServer.runs`
- P1-6: Fix `uuid_short()` panic (use fallback on SystemTime failure)
- P2-MCP: Add MCP server mode (`pidag mcp`) exposing pidag as MCP tools

**Out of scope**: UI changes, web authentication, rate limiting, distributed
deployment. Those are separate future specs.

---

## Requirements

### Functional (P0 Correctness)

**R1** (P0-3a): `dag.result` must return actual node outputs from the store, not
`{outputs: {}}`. When called with `dagId`, return all node artifacts from the
`ARTIFACTS_TABLE` keyed by node_id.

**R2** (P0-3b): `dag.status` must return real node states, not fake status. Read
`list_nodes(dag_id)` from the store and return `{dagId, nodes: [{node_id, state, model}], done, failed, status}`.

**R3** (P0-3c): `node.retry` must actually retry the node. Mark the node as
`Pending` in the store, emit a `NodeRetry` event, and let the scheduler re-dispatch.
Return `{state: "pending", queued: true}` when successful.

**R4** (P0-4): Resume tokens must be functional. When `dag.await` times out while
still running, return a token that encodes `(dag_id, last_event_seq)`. When
`dag.await` receives a token, resume from that sequence instead of blocking from
the start. Token format: `resume:<dag_id>:<last_seq>` (base64 optional).

### Functional (P1 Robustness)

**R5** (P1-4): RPC server must handle requests concurrently. Spawn each request
handler as a `tokio::spawn` task so long-running `dag.await` calls don't block
subsequent requests.

**R6** (P1-5): Completed runs must be cleaned up. After a run reaches terminal
state (DagDone with all nodes Done/Failed/Skipped), remove it from
`RpcServer.runs` after a configurable TTL (default 60 seconds). This prevents
unbounded memory growth.

**R7** (P1-6): `uuid_short()` must not panic. Replace `unwrap()` on
`SystemTime::now().duration_since(UNIX_EPOCH)` with `unwrap_or(Duration::ZERO)`.

### Functional (P2 MCP Server Mode — stdio transport)

**R8** (MCP-mode): Add `pidag mcp` subcommand that starts an MCP server using
**stdio transport** (JSON-RPC over stdin/stdout). This REPLACES the existing
`pidag rpc` command — the new `pidag mcp` is MCP-compliant and backward-compatible
with the existing RPC methods as custom MCP tools.

**R9** (MCP-tools): Expose the following MCP tools:
- `pidag_submit_dag`: Accept `{dag: <DAG JSON>}`, return `{dagId, status: "submitted"}`
- `pidag_await`: Accept `{dagId, timeoutMs?, token?}`, return report or timeout status
- `pidag_status`: Accept `{dagId}`, return node states
- `pidag_list_runs`: No params, return list of runs
- `pidag_cancel`: Accept `{dagId}`, cancel and return confirmation
- `pidag_result`: Accept `{dagId}`, return all node artifacts
- `pidag_node_retry`: Accept `{dagId, nodeId}`, retry the node

**R10** (MCP-transport): Use stdio transport per MCP spec:
- Read newline-delimited JSON-RPC 2.0 from stdin
- Write newline-delimited JSON-RPC 2.0 to stdout
- Support MCP lifecycle methods: `initialize`, `initialized`, `shutdown`
- Support `tools/list` (returns tool schemas) and `tools/call` (invokes a tool)
- Log to stderr (not stdout — stdout is the MCP transport)

### Non-Functional

**N1**: Add `rmcp` dependency for MCP stdio server (already used in orki, proven stable).
**N2**: All changes backward-compatible — existing CLI/RPC behavior unchanged.
**N3**: No `.unwrap()/.expect()/panic!()` in production paths.
**N4**: Tests for each R# requirement.

---

## Architecture

### RPC Handler Changes (`src/rpc.rs`)

```text
Current (blocking):
  stdin line → handle_line (awaits whole request) → stdout

After (concurrent):
  stdin line → tokio::spawn(handle_line) → mpsc → stdout writer task
```

`handle_dag_result`: Query `store.list_nodes()` + `store.get_artifact()` for each
node, build actual outputs map.

`handle_dag_status`: Query `store.list_nodes()`, compute done/failed counts from
node states, return real status.

`handle_node_retry`: Verify dag exists, mark node as Pending via
`store.put_node_state()`, emit `NodeRetry` event, return success.

Resume tokens: Encode as `resume:<dag_id>:<last_seq>`. On `dag.await` with token,
parse it and call `store.load_events_since(dag_id, last_seq)` to resume where
left off.

### MCP Server (`src/mcp_server.rs` — NEW, replaces `src/rpc.rs`)

```text
pidag mcp --vault .pidag/pidag.redb
    │
    ├── stdin (JSON-RPC 2.0, newline-delimited)
    │       ├── initialize → {serverInfo, capabilities}
    │       ├── tools/list → [pidag_submit_dag, pidag_await, ...]
    │       └── tools/call → route to tool handler
    │
    ├── stdout (JSON-RPC 2.0 responses, newline-delimited)
    │
    └── Arc<dyn Store> (RedbStorePool for concurrent access)
```

The MCP server is a **drop-in replacement** for the existing `pidag rpc`:
- Same stdio transport (stdin/stdout)
- Same vault path handling
- Existing RPC methods become MCP tools (`dag.submit` → `pidag_submit_dag`)
- MCP lifecycle methods (`initialize`, `tools/list`, `tools/call`) added

Use `rmcp` crate's stdio transport with `RoleServer`. Tool handlers delegate to
the existing logic from `rpc.rs` (extract into shared `handlers.rs` module).

### File Changes

| File | Change |
|------|--------|
| `src/handlers.rs` (NEW) | Extract shared handler logic from `rpc.rs` (dag_submit, dag_await, etc.) |
| `src/mcp_server.rs` (NEW) | R8-R10: MCP stdio server with tool wrappers |
| `src/rpc.rs` | R1-R7: implement real handlers via `handlers.rs`, concurrent dispatch, cleanup |
| `src/bin/pidag.rs` | Add `mcp` subcommand (replaces `rpc` as primary interface) |
| `src/lib.rs` | Export `mcp_server` and `handlers` modules |
| `Cargo.toml` | Add `rmcp` dependency for MCP protocol |
| `tests/rpc_hardening_tests.rs` (NEW) | Tests for R1-R7 |
| `tests/mcp_server_tests.rs` (NEW) | Tests for R8-R10 (MCP protocol compliance) |

---

## TDD Contract

Tests `rust-specialist` writes BEFORE production code.

| # | Test name | Given | Expects |
|---|---|---|---|
| H1 | `test_dag_result_returns_actual_outputs` | Run with 2 done nodes + artifacts | `dag.result` returns `{outputs: {node_a: "...", node_b: "..."}}` |
| H2 | `test_dag_status_returns_real_states` | Run with 1 done, 1 failed node | `dag.status` returns correct `nodes` array, `done: 1`, `failed: 1` |
| H3 | `test_node_retry_marks_pending` | Failed node | `node.retry` returns `{state: "pending", queued: true}`, node state is Pending in store |
| H4 | `test_resume_token_continues_from_seq` | `dag.await` timeout, then resume with token | Second `dag.await` receives only events after token's seq |
| H5 | `test_concurrent_rpc_requests` | Submit 2 DAGs concurrently | Both return immediately (no blocking) |
| H6 | `test_completed_runs_cleaned_up` | Run completes, wait TTL+1s | Run removed from `runs` map |
| H7 | `test_uuid_short_no_panic` | Mock SystemTime failure | `uuid_short()` returns fallback ID, no panic |
| M1 | `test_mcp_initialize_returns_capabilities` | Send `initialize` via stdin | Response has `serverInfo`, `capabilities.tools: true` |
| M2 | `test_mcp_tools_list_returns_all_tools` | Send `tools/list` via stdin | Returns 7 tools with correct schemas |
| M3 | `test_mcp_submit_and_await` | `tools/call` with `pidag_submit_dag`, then `pidag_await` | Returns `{done: true, report: ...}` |
| M4 | `test_mcp_status_returns_node_states` | Submit DAG, `tools/call` with `pidag_status` | Returns `{nodes: [...]}` with real states |
| M5 | `test_mcp_cancel_stops_run` | Submit DAG, `tools/call` with `pidag_cancel` | Returns `{cancelled: true}` |
| M6 | `test_mcp_result_returns_artifacts` | Run DAG to completion, `tools/call` with `pidag_result` | Returns `{outputs: {node_a: "..."}}` |
| M7 | `test_mcp_node_retry_requeues` | Failed node, `tools/call` with `pidag_node_retry` | Returns `{state: "pending", queued: true}` |

---

## Exit Criteria

`loop-engineer` does not exit until every item is resolved.

- [ ] `cargo test -p pidag 2>&1 | grep -q "0 failed"`
- [ ] `cargo clippy -p pidag -- -D warnings 2>&1 && echo CLIPPY_OK | grep -q CLIPPY_OK`
- [ ] All 14 tests in the TDD Contract exist and pass (H1-H7 + M1-M7).
- [ ] `dag.result` returns actual node artifacts from the store.
- [ ] `dag.status` returns real node states with correct done/failed counts.
- [ ] `node.retry` marks the node as Pending and queues it for re-dispatch.
- [ ] Resume tokens encode `dag_id:last_seq` and `dag.await` resumes from that seq.
- [ ] Multiple concurrent RPC requests are handled without blocking.
- [ ] Completed runs are removed from `RpcServer.runs` after TTL (60s default).
- [ ] `uuid_short()` uses `unwrap_or(Duration::ZERO)` — no panic on SystemTime failure.
- [ ] `pidag mcp` starts an MCP stdio server with 7 tools (submit, await, status, list_runs, cancel, result, node_retry).
- [ ] MCP server responds to `initialize` with `capabilities.tools: true`.
- [ ] MCP `tools/list` returns all 7 tool schemas with correct inputSchema.
- [ ] MCP tools delegate to shared `handlers.rs` (no duplicate implementation).
- [ ] No `.unwrap()/.expect()/panic!()` in new production code.

---

## Guardrails

What `rust-specialist` must NOT do:

- Do not add authentication to the MCP server — that's a separate spec.
- Do not change the existing CLI subcommands (`run`, `sdd`, `ui`, `show`).
- Do not modify the Store trait or redb schema — existing schema is sufficient.
- Do not add new Cargo.toml dependencies except `rmcp` (required for MCP stdio server).
- Do not implement distributed/clustered RPC — single-process only.
- Do not use `.unwrap()/.expect()/panic!()` in `src/` production paths.
- On any ambiguity — stop and report to `loop-engineer`, do not guess.

---

## Implementation Notes

### Resume Token Format

Simple, URL-safe, no cryptographic signature (trusted single-process context):
```
resume:run-20260804-120000-abc123:42
       ^--- dag_id ---^          ^--- last_event_seq
```

### MCP stdio Protocol

```text
Client → Server (stdin):
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"claude","version":"1.0"}}}

Server → Client (stdout):
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"pidag","version":"0.1.0"},"capabilities":{"tools":{}}}}

Client → Server (stdin):
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}

Server → Client (stdout):
{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"pidag_submit_dag","description":"Submit a DAG","inputSchema":{...}},...]}}

Client → Server (stdin):
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"pidag_submit_dag","arguments":{"dag":{...}}}}

Server → Client (stdout):
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\"dagId\":\"run-...\",\"status\":\"submitted\"}"}]}}
```

### MCP Tool Schemas (all 7)

```rust
// In mcp_server.rs, define tools via rmcp macros:
#[tool(description = "Submit a DAG for execution")]
async fn pidag_submit_dag(&self, dag: Value) -> Result<CallToolResult, McpError>;

#[tool(description = "Await DAG completion or timeout")]
async fn pidag_await(&self, dag_id: String, timeout_ms: Option<u64>, token: Option<String>) -> Result<CallToolResult, McpError>;

#[tool(description = "Get current DAG status and node states")]
async fn pidag_status(&self, dag_id: String) -> Result<CallToolResult, McpError>;

#[tool(description = "List all runs in the vault")]
async fn pidag_list_runs(&self) -> Result<CallToolResult, McpError>;

#[tool(description = "Cancel a running DAG")]
async fn pidag_cancel(&self, dag_id: String) -> Result<CallToolResult, McpError>;

#[tool(description = "Get all node artifacts/outputs for a completed DAG")]
async fn pidag_result(&self, dag_id: String) -> Result<CallToolResult, McpError>;

#[tool(description = "Retry a failed node")]
async fn pidag_node_retry(&self, dag_id: String, node_id: String) -> Result<CallToolResult, McpError>;
```

### Cleanup Loop

```rust
// In McpServer, spawn a background task for run cleanup:
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        runs.lock().await.retain(|_, state| {
            !state.is_terminal() || state.completed_at.elapsed() < TTL
        });
    }
});
```

# pidag — spec-16: RPC-backed worker (persistent pi --rpc session)

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: HIGH (evolutionary upgrade from spec-15's `--session -p`)
- **Status**: SUPERSEDED-BY spec-18. Implemented in `adaeaa0`/`2363dc6` but **cannot
  work against the real `pi` binary**: it spawns `pi --rpc --mode json`, which clap
  rejects (`--rpc` and `--mode` are mutually exclusive), plus five further defects —
  see `ANALYSIS-2026-08-10-pi-sdk-realignment.md` §1. Upstream already ships the
  correct client (`pi::sdk::RpcTransportClient`); spec-18 adopts it and deletes
  `src/worker/rpc.rs`. **Do not repair this worker.**
- **Mitigation until spec-18 lands**: `PI_NO_RPC=1` (routes LLM nodes to `PiPrintWorker`)
- **Completed**: `adaeaa0` (superseded, defective)

---

## Overview

spec-15 gave the worker multi-turn context by passing `--session <path>` to `pi -p`,
but each iteration still spawns a **new process** (startup overhead + session file
I/O). pi's `--rpc` mode (src/rpc.rs, 8772 lines) is a **persistent JSON-RPC 2.0
server** over stdin/stdout: one long-lived `pi --rpc` process, prompts sent via
stdin, responses streamed via stdout, session in-memory.

This spec replaces the one-shot `PiPrintWorker` with an `RpcWorker` that:
1. Spawns ONE `pi --rpc` process per DAG run (not per node).
2. Sends `{"type":"prompt","message":"...","id":"..."}` for each LLM node.
3. Reads the streaming response until `agent_end`.
4. Extracts the assistant's final text from the `agent_end` event.
5. Keeps the session in-memory — no file I/O, no process restart between iterations.
6. Shares the session across specs via `--session <path>` (passed to the RPC process).

## Requirements

- **R1**: New `RpcWorker` implements `Worker` trait; spawns `pi --rpc` once at
  construction; reuses the process for all `run()` calls.
- **R2**: Each `run()` sends a `{"type":"prompt","message":"<prompt>","id":"<node_id>"}`
  JSON line on stdin, then reads stdout events until `agent_end`.
- **R3**: Extracts the assistant's final reply from the `agent_end` event's
  `messages` array (last assistant message's `content[0].text`).
- **R4**: Handles errors: RPC process crash, timeout, malformed JSON, missing
  `agent_end`.
- **R5**: Passes `--session <path>` to the RPC process so the session persists
  across runs AND across specs (shared session file).
- **R6**: `PI_NO_RPC=1` env var falls back to `PiPrintWorker` (spec-15 behavior).
- **R7**: The RPC worker passes `--model`, `--provider`, `--thinking` like the
  print worker.
- **R8**: The anti-loop prompt (`WORKER_ANTI_LOOP_PROMPT`) is injected via
  `--append-system-prompt` (same as spec-14).

## TDD contract

| id | given | expects |
|----|-------|---------|
| R1 | RpcWorker constructed | `pi --rpc` process spawned (alive) |
| R2 | run("node1", "model1", 1) | stdin receives JSON with type=prompt + message |
| R3 | run returns | output contains the agent_end messages text |
| R4 | RPC process exits early | WorkerOutput success=false with error |
| R5 | PI_NO_RPC=1 | TypeDispatchWorker uses PiPrintWorker (fallback) |
| R6 | --session in args | session path present in spawn args |

## Exit criteria

- [ ] `cd /projects/pidag && cargo test 2>&1 | grep -q "test result: ok"`
- [ ] `grep -q 'RpcWorker' src/worker/mod.rs`
- [ ] `grep -q '\-\-rpc' src/worker/`

## Guardrails
- Do not remove `PiPrintWorker` (fallback when `PI_NO_RPC=1`).
- Do not change the `Worker` trait signature.
- RPC worker must handle the case where `pi --rpc` is not available (fallback).
- Session file under `_tmp/`.

## Files to modify
| File | Change |
|------|--------|
| `src/worker/mod.rs` | Add `RpcWorker`, re-export |
| `src/worker/rpc.rs` | New file: RPC worker implementation |
| `src/worker/type_dispatch.rs` | Route to RpcWorker when available, else PiPrintWorker |
| `tests/*` | R1-R6 tests |

## Verification
```bash
cd /projects/pidag && cargo test && cargo clippy -- -D warnings
cargo build --release && cp target/release/pidag /root/.local/bin/pidag
# Then re-run bloodtest: implement-iter1 + implement-iter2 share one RPC session.
```
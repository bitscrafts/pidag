# pidag — spec-18: Replace the hand-rolled RPC worker with `pi::sdk::RpcTransportClient`

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: P0 (the default LLM path is broken on `main`)
- **Status**: PLAN APPROVED — implementation NOT started
- **Depends-On**: **spec-17** (contract tests) and **spec-21** (the `AgentBackend` seam)
- **Supersedes**: spec-15 (session-backed worker), spec-16 (RPC-backed worker)
- **Implementation order**: 17 → 21 → **18**
- **AMENDED 2026-08-10** (user direction: *"avoid hardwiring pi with pidag, make it
  modular, in the future we could use other coding agents"*): this spec no longer links
  `pi::sdk` into a worker. It implements **`PiBackend`, one implementation of the
  `AgentBackend` trait defined in spec-21**. The transport work below is unchanged; only
  its home moves — what was `src/worker/session.rs` becomes `src/backend/pi.rs`, and the
  lane pool sits behind the trait. Read spec-21 first.
- **Source**: `specs/ANALYSIS-2026-08-10-pi-sdk-realignment.md` §1, §3, §4

---

## Overview

`src/worker/rpc.rs` (254 lines) hand-rolls process spawning, JSON framing, session
handling and error classification for `pi`. It carries six defects (ANALYSIS §1),
starting with an invocation the real binary rejects outright — so **every LLM node
fails on the default path today**.

`pi_agent_rust` already ships the correct implementation as part of its SemVer-stable
SDK: `pi::sdk::RpcTransportClient` — request-id correlated JSON-lines framing over
`pi --mode rpc`, with 34 typed commands. Its own `RpcTransportOptions::default()` is
`args: ["--mode", "rpc"]` (`src/sdk.rs:630`), i.e. the invocation pidag got wrong.

This spec **deletes** `src/worker/rpc.rs` and routes LLM nodes through the upstream
client. It is a net code *reduction* that simultaneously fixes six defects, restores
per-node model routing, and unlocks the session-DAG work in spec-19.

### Immediate mitigation (do this before implementation starts)

Until this spec lands, the broken path must not be the default. Either export
`PI_NO_RPC=1` in the runtime environment, or invert the gate in
`type_dispatch.rs:59` so RPC is opt-**in**. This is a one-line change and is NOT
gated on the rest of this spec.

---

## Phase 0 — Dependency decision gate (MANDATORY, before any code)

Linking `pi_agent_rust` is the crux. It must be measured, not assumed.

1. Record the current clean-build baseline: `cargo clean && time cargo build --release`.
2. Add the dependency and re-measure:
   ```toml
   [dependencies]
   pi = { package = "pi_agent_rust", path = "/projects/_upstream/pi_agent_rust", default-features = false }
   ```
   `default-features = false` drops the `tui` feature (crossterm + charmed_rust); the
   `pi` **binary** requires it, the **library** does not.
3. **Decision rule** — record the measured delta in the commit message:
   - Delta ≤ +120s clean build → **proceed with Path A** (link the SDK).
   - Delta > +120s, or the build fails on feature resolution → **stop and escalate to
     the architect**. Do not silently fall back. The fallback (Path B: speak the
     documented JSON-lines protocol from `docs/rpc.md` without linking) reintroduces
     ownership of the framing and needs its own spec.

Everything below assumes Path A.

---

## Requirements

### Functional

- **R1 (dependency)**: `pidag` depends on `pi = { package = "pi_agent_rust", ... }`
  with `default-features = false`. Only the `pi::sdk` surface (plus `pi::Error` /
  `pi::PiResult`) may be referenced — all other `pi::` modules are explicitly unstable
  upstream.
- **R2 (new backend, not a new worker)**: New `src/backend/pi.rs` defines `PiBackend`
  implementing **`AgentBackend`** (spec-21), and `PiSession` implementing `AgentSession`.
  It reaches the scheduler through the generic `AgentWorker` adapter — `PiBackend` itself
  must contain **no** `Worker` impl and no scheduler knowledge.
- **R2b (capability declaration)**: `PiBackend::capabilities()` declares, honestly:
  `sessions`, `multi_turn`, `model_switch`, `thinking_levels`, `fork`, `compact`,
  `token_usage`, `cancellation` — all true, each backed by a real `RpcTransportClient`
  command. spec-21's conformance suite (R8) verifies every declared capability actually
  works, so an over-declaration fails the build.
- **R3 (one client per lane)**: `RpcTransportClient::request` takes `&mut self` and
  owns `next_request_id`, so a client is **not** shareable across concurrent nodes.
  `PiBackend` holds a pool of clients sized to the scheduler's `concurrency`, each
  lane checking one out for the duration of a node. This makes `--concurrency N`
  (`cli/sdd.rs:31`, `cli/run.rs:20`) real instead of nominal — the current design
  serialises every LLM node behind one mutex.
- **R4 (per-node model routing restored)**: Before each prompt, the lane calls
  `set_model(provider, model_id)`, deriving both from the node's model string via the
  existing `split_provider_model()` (spec-13 P13). This closes ANALYSIS §1.3, where
  `--model` was never passed and nodes silently ran on `PI_MODEL`.
- **R5 (model verification)**: After `set_model`, the worker asserts via `get_state`
  that the active model matches the requested one. A mismatch fails the node with a
  clear error rather than silently running the wrong (possibly paid) model. **This is
  the guard that makes a §1.3-class defect impossible to reintroduce silently.**
- **R6 (thinking level)**: `set_thinking_level` from `PI_THINKING` (default `low`,
  preserving checkpoint `8120e85`).
- **R7 (anti-loop prompt)**: The `WORKER_ANTI_LOOP_PROMPT` const moves into
  `RpcTransportOptions::args` as `--append-system-prompt <text>`. Same text, same
  behaviour, no inline string literal in the spawn path.
- **R8 (working directory)**: `RpcTransportOptions::cwd` is set to the DAG's
  `project_root`. Fixes the cwd-relative session/artifact paths noted in ANALYSIS §1.2.
- **R9 (error handling and lane recovery)**: A transport error, a dead child, or a
  timeout **discards that lane's client and reconnects on next use**. It must never
  restore a desynced stream (§1.6) or permanently poison the remaining nodes (§1.5).
  Errors map to `WorkerOutput { success: false, retryable: <classified> }`; retryable
  classification prefers typed SDK errors over string matching where available.
- **R10 (deletion)**: `src/worker/rpc.rs` is **deleted**, along with its `mod rpc;` and
  the `pub use rpc::RpcWorker;` re-exports in `src/worker/mod.rs` and `src/lib.rs`.
- **R11 (fallback preserved)**: `PI_NO_RPC=1` selects `PiPrintWorker` unchanged. The
  one-shot worker stays as an escape hatch; it is not deleted in this spec.
- **R12 (routing)**: LLM non-A2A nodes reach `PiBackend` through spec-21's
  `AgentWorker` + registry (`[agent] backend = "pi"`). `TypeDispatchWorker` gains no
  pi-specific branch. The A2A, shell and MCP branches are untouched.
- **R13 (no vendor leakage)**: `pi::` types stay inside `src/backend/pi.rs`. Nothing
  outside that file may name `pi::sdk`, a `PI_*` env var, or a pi CLI flag — this is
  spec-21 R5, and it is what makes a future second backend a drop-in rather than a
  refactor.

### Non-Functional

- **N1**: No regression in the full suite; spec-14's gate semantics
  (`scheduler/execute.rs`) are not touched by this spec.
- **N2**: No `unwrap()`/`expect()` in production paths — `test_no_production_unwrap`
  stays green.
- **N3**: Session/artifact writes stay under `_tmp/` or `project_root`.

---

## Architecture

```mermaid
flowchart TD
    A["TypeDispatchWorker::run"] --> B{"node_type / model"}
    B -->|shell| C["RealShellWorker"]
    B -->|mcp_call| D["McpCallWorker"]
    B -->|http(s) model| E["A2aWorker"]
    B -->|llm| F{"PI_NO_RPC set?"}
    F -->|yes| G["PiPrintWorker (legacy one-shot)"]
    F -->|no| H["PiBackend"]
    H --> I["lane pool (size = concurrency)"]
    I --> J["pi::sdk::RpcTransportClient"]
    J --> K["pi --mode rpc subprocess"]
    J -.->|"error → drop + reconnect"| I
```

### Per-node sequence

1. Acquire a lane (bounded by scheduler concurrency).
2. `set_model(provider, model_id)` from the node's model string (R4).
3. `get_state` → verify active model (R5); mismatch ⇒ fail node.
4. `set_thinking_level` (R6).
5. `prompt(node_prompt)` → `Vec<Value>` events.
6. Extract assistant text (`get_last_assistant_text` is the typed alternative to
   scraping the event array — prefer it).
7. Release lane. On any error: drop the client, mark the lane for reconnect (R9).

### Why the RPC transport rather than the in-process `AgentSessionHandle`

Three reasons, per ANALYSIS §4.1: the RPC client has the **complete** command surface
(`fork`, `steer`, `abort`, `switch_session` are RPC-only — and spec-19 needs `fork`);
it preserves **process isolation** so a wedged agent cannot take down the orchestrator
holding the redb vault and the web UI; and it is the smaller migration from pidag's
already-subprocess-shaped `Worker` trait. Revisit in-process for leaf nodes later.

---

## TDD Contract

Tests marked **[real]** use the real `pi` binary via spec-17's harness and honour
`PIDAG_REQUIRE_PI` / skip semantics. Tests marked **[unit]** are pure logic with no
subprocess. **No test in this spec may validate protocol framing against a shim** —
that is the practice that produced the defect (ANALYSIS §7).

| id | kind | given | expects |
|----|------|-------|---------|
| S1 | [unit] | `PiBackend::new(dag, timeout, concurrency=4)` | lane pool capacity 4; no process spawned at construction |
| S1b | [real] | `PiBackend::capabilities()` | every one of the eight declared capabilities passes spec-21's conformance battery (R2b); an over-declaration fails the build |
| S2 | [unit] | model string `"nvidia/z-ai/glm-5.2"` | `split_provider_model` → `("nvidia", "z-ai/glm-5.2")`; both reach the `set_model` call args |
| S3 | [unit] | model string `"deepseek-v4-flash"` (no provider) | provider `None`; `set_model` receives the configured default provider, not a fabricated one |
| S4 | [real] | `PiBackend` runs a node whose prompt is `"reply with the single word OK"` on a free model | `success == true`; output non-empty. **Only test in the suite that spends tokens** — gated behind `PIDAG_LIVE_AGENT_TESTS=1`, skipped by default |
| S5 | [real] | `set_model` to a model id that does not exist | node fails with a model-mismatch error (R5), does **not** silently run on the default model |
| S6 | [unit] | lane client returns a transport error | that lane's client is dropped; the next acquisition reconnects; other lanes unaffected (R9) |
| S7 | [unit] | two nodes dispatched concurrently with `concurrency=2` | two distinct lanes used; neither blocks the other (R3) |
| S8 | [unit] | `PI_NO_RPC=1` | `TypeDispatchWorker` routes LLM nodes to `PiPrintWorker` (R11) — serialise with the existing `ENV_LOCK` |
| S9 | [unit] | crate-wide grep | `src/worker/rpc.rs` absent; no `RpcWorker` symbol remains (R10) |
| S10 | [unit] | `RpcTransportOptions` built for a DAG with `project_root=/x` | `cwd == /x`; args contain `--append-system-prompt` (R7, R8) |

---

## Exit Criteria

```bash
cd /projects/pidag

# 1. Phase 0 decision is recorded (build-time delta in the commit message)
git log -1 --format=%B | grep -qiE 'clean build delta|build delta'

# 2. Dependency present, TUI feature off
grep -q 'package = "pi_agent_rust"' Cargo.toml
grep -q 'default-features = false' Cargo.toml

# 3. The hand-rolled worker is gone
! test -f src/worker/rpc.rs
! grep -rq "RpcWorker" src/

# 4. The SDK client is the transport
grep -q "RpcTransportClient" src/backend/pi.rs

# 5. Only the stable SDK surface is used
! grep -rE "\bpi::(?!sdk|Error|PiResult)" -P src/ || true   # informational
grep -rq "pi::sdk::" src/backend/pi.rs

# 6. Suite, lints, gate
cargo test 2>&1 | grep -q "test result: ok"
PIDAG_REQUIRE_PI=1 cargo test --test pi_contract_tests 2>&1 | grep -q "test result: ok"
cargo clippy -p pidag -- -D warnings
cargo fmt --check
bash deploy/scripts/quality-gate.sh .

# 7. Live self-test: the bloodtest fires the fix node AND records the right model
cargo build --release && cp target/release/pidag /root/.local/bin/pidag
cd _tmp/bug-a-bloodtest && pidag sdd 01-blood-test.md --run 2>&1 | tee run18.log
grep -q "implement-iter2" run18.log
grep -qi "deepseek-v4-flash" run18.log     # the configured worker model, not PI_MODEL
```

**Prose criterion (unambiguous)**: after the bloodtest run, the recorded events for
each LLM node must show the model that the DAG requested for that node. A run in which
every node reports the same default model is a failure of R4/R5 even if the DAG
completes.

---

## Guardrails

- **Only** `pi::sdk`, `pi::Error`, `pi::PiResult`. Reaching into any other `pi::`
  module is out of contract and will break without a SemVer bump.
- **Do NOT** change the `Worker` trait signature (spec-16 guardrail, still binding).
- **Do NOT** delete `PiPrintWorker` — R11 depends on it.
- **Do NOT** touch `src/scheduler/execute.rs` gate logic (spec-14). If a gate bug
  surfaces during this work, file it; do not fix it here.
- **Do NOT** validate protocol framing with a bash shim. Framing correctness is
  upstream's responsibility now; pidag's job is to pin the *invocation* (spec-17) and
  its own routing logic (S1-S3, S6-S10).
- **Do NOT** implement session forking, steering, or compaction here — spec-19/20.
  This spec is a like-for-like transport replacement plus the model-routing fix.
- Session files under `_tmp/` or `project_root`, never `/tmp/`.
- Do not modify the chromecast-tv-mirror project.

---

## Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` | Add `pi = { package = "pi_agent_rust", path = "...", default-features = false }` |
| `src/backend/pi.rs` | **NEW** — `PiBackend`, lane pool, model verification |
| `src/worker/rpc.rs` | **DELETE** (R10) |
| `src/worker/mod.rs` | Drop `mod rpc` / `RpcWorker` re-export; add `mod session` / `PiBackend`; move `WORKER_ANTI_LOOP_PROMPT` here if it is to be shared |
| `src/worker/type_dispatch.rs` | Remove the `rpc_worker` field entirely; LLM nodes route via spec-21 `AgentWorker` + registry. No pi-specific branch remains (R12) |
| `src/lib.rs` | Re-export `PiBackend`; drop `RpcWorker` |
| `tests/pi_backend_tests.rs` | **NEW** — S1-S3, S6-S10 |
| `tests/pi_contract_tests.rs` | Extend with S4, S5 (live-gated) |
| `specs/15-session-backed-worker.md`, `specs/16-rpc-backed-worker.md` | Mark `SUPERSEDED by spec-18` in the status line |

---

## Verification

```bash
cd /projects/pidag
cargo clean && time cargo build --release          # Phase 0 measurement
cargo test && PIDAG_REQUIRE_PI=1 cargo test --test pi_contract_tests
cargo clippy -p pidag -- -D warnings && cargo fmt --check
cargo build --release && cp target/release/pidag /root/.local/bin/pidag
cd _tmp/bug-a-bloodtest && pidag sdd 01-blood-test.md --run
git add -A && git commit -m "feat(pidag): spec-18 adopt pi::sdk RpcTransportClient; delete hand-rolled rpc worker (clean build delta: <N>s)"
```

## Memory

Store on completion: `pidag/specs/18-adopt-pi-sdk-rpc-transport`,
`pidag/fix/20260810-rpc-invocation-defect`,
`pidag/decision/20260810-link-pi-agent-rust-sdk` (record the Phase 0 measurement).

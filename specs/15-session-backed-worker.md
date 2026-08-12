# pidag — spec-15: Session-backed worker (multi-turn pi -p via --session)

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: HIGH (the worker-capability gap blocking dogfooding)
- **Status**: SUPERSEDED-BY spec-18. Implemented in `c0dba5a`, but built on a misread
  of `pi --session` (it *opens an existing* session file; it does not create one — see
  `ANALYSIS-2026-08-10-pi-sdk-realignment.md` §1.2). Session continuity is delivered
  correctly by `pi::sdk::RpcTransportClient` in spec-18. **Do not implement further.**
- **Completed**: `c0dba5a` (superseded)

---

## Overview

pidag's SDD worker spawns `pi -p` (print mode) for each `implement-iterN` node.
Print mode is **ephemeral by default** (`no_session = true`): each call is a
one-shot with no memory of prior calls. A "implement from scratch" prompt (6
modules + 10 tests) is too big for one completion — the worker returns partial
output or fails, and `implement-iter2` (the fix iteration) starts from scratch
with no context of what iter1 wrote.

**Fix**: pass `--session <path>` to `pi -p` so each worker call **persists its
session**. Then `implement-iter2` uses `pi -c --session <path> -p` (continue) to
resume the session with full context: what iter1 wrote, what tools it called,
what the validation failure was. This gives the worker multi-turn capability
with ZERO architecture change — just session flags.

**Evidence** (pi source, `/projects/_upstream/pi_agent_rust/src/app.rs:2227`):
`--session <path>` in print mode sets `no_session = false` → session IS persisted.
Without it, print mode stays ephemeral (line 2232).

## Requirements

- **R1**: The worker (`PiPrintWorker`) passes `--session <path>` to `pi -p`,
  where `<path>` is a per-run session file (e.g. `_tmp/.pidag-session-<run-id>.jsonl`).
- **R2**: On fix iterations (iter2/iter3), the worker passes `-c` (continue) +
  `--session <path>` so pi resumes the prior session with full context.
- **R3**: The session path is deterministic per run (derived from run-id or
  project root + node-id) so all iterations of the same run share one session.
- **R4**: The session file is cleaned up after the run completes (or left in
  `_tmp/` for debugging).
- **R5**: The `--session` flag is only added for LLM nodes (not shell nodes).
- **R6**: A `PI_NO_SESSION=1` env var disables session persistence (fallback to
  the old ephemeral one-shot behaviour) for debugging.

## TDD contract

| id | given | expects |
|----|-------|---------|
| S1 | worker spawn for implement-iter1 | cmd args include `--session <path>` and NOT `-c` |
| S2 | worker spawn for implement-iter2 | cmd args include `-c` AND `--session <path>` |
| S3 | `PI_NO_SESSION=1` env set | cmd args do NOT include `--session` or `-c` |
| S4 | session path for same run | same path across iter1/iter2/iter3 |

## Exit criteria

- [ ] `cd /projects/pidag && cargo test 2>&1 | grep -q "test result: ok"`
- [ ] `grep -q '\-\-session' src/worker/pi_print.rs`
- [ ] `grep -q 'PI_NO_SESSION' src/worker/pi_print.rs`
- [ ] `cargo clippy -- -D warnings 2>&1 | grep -q "Finished"`

## Guardrails
- Do not change the `Worker` trait or `TypeDispatchWorker` dispatch logic.
- Do not change the SDD DAG structure (nodes/edges/gates).
- The session file MUST live under `_tmp/` (intermediate artifacts).
- Do not couple to external files (system.md lesson).

## Files to modify
| File | Change |
|------|--------|
| `src/worker/pi_print.rs` | Add `--session <path>` for iter1, `-c --session <path>` for iter2/iter3; `PI_NO_SESSION` env guard |

## Verification
```bash
cd /projects/pidag && cargo test && cargo clippy -- -D warnings
cargo build --release && cp target/release/pidag /root/.local/bin/pidag
# Then re-run the bloodtest: implement-iter1 + implement-iter2 share a session.
```
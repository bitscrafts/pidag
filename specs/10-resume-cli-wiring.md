# Spec: Wire Checkpoint-Resume Into the CLI + Scheduler (Spec-08 Finish)

**Project**: `.` (container: `/projects/pidag`)
**Topic**: `claude-pi-delegation`
**Date**: 2026-08-07
**Depends-On**: 08-checkpoint-resume.md (library half already landed), 03-multi-project-workspace.md (auto driver)

---

## Overview

Spec `08-checkpoint-resume.md` shipped its **library** half (`src/sdd/resume.rs`:
`run_id_for_spec`, `load_checkpoint`, `Checkpoint`, `ResumeDecision`) plus
placeholder tests. The **integration** half never landed — `pidag sdd spec.md --run`
still generates a fresh timestamp run_id each invocation, never consults the vault,
and the scheduler begins every run with all nodes Pending. As a result:

- The autonomy driver (`pidag auto`) cannot honor its "resume unfinished across
  cron passes" requirement (design doc req #2) — it just re-runs the same spec
  from scratch each tick, wasting LLM quota on the already-completed prefix.
- The documented `--resume`/`--fresh`/`--retry-failed` flags are dead strings:
  `cli/sdd.rs` rejects them as "unknown option" (the `_ =>` arm at line 57).
- Four tests in `tests/checkpoint_resume_tests.rs` (T6-T9 + T11) are
  `HashSet::contains` stubs that assert nothing about scheduler behavior.

This spec finishes spec-08: deterministic run_id, `load_checkpoint` on startup,
`Scheduler::with_checkpoint()` that skips Done nodes and pre-decrements
dependents' in-degree, real CLI flag wiring, and the auto-driver defaulting to
`--resume`.

## Requirements

### Functional
- **R1**: `pidag sdd <spec.md> --run` derives `run_id = run_id_for_spec(spec_path, spec_content)`
  and passes `--run-id <id>` to the `pidag run` subprocess. The run_id is deterministic
  for an unchanged spec; editing the spec yields a new run_id (resume granularity = spec content).
- **R2**: `pidag run` accepts `--run-id <id>`, `--resume`, `--fresh`, `--retry-failed`.
  When `--resume` is set, it calls `load_checkpoint(store, run_id, retry_failed)` and:
  - `ResumeDecision::Fresh` -> proceed as a normal new run (using the given run_id).
  - `ResumeDecision::AlreadyDone` -> print the cached report and exit 0 (no re-execution).
  - `ResumeDecision::Resume` -> build `Scheduler::with_checkpoint(.., checkpoint)` and continue.
  - `--fresh` skips `load_checkpoint` entirely (always Fresh path, still uses the determinstic run_id so it OVERWRITES the prior run record).
- **R3**: `Scheduler::with_checkpoint(dag, worker, sink, concurrency, checkpoint)`:
  nodes in `checkpoint.completed_nodes` start in state `Done`, are NOT dispatched,
  have each dependent's in-degree pre-decremented (so a node whose only dependency
  is a completed node becomes ready immediately), and are recorded in the
  scheduler's terminal snapshot so `wait_any`/`await_dag` observers see them as terminal.
- **R4**: Nodes in `checkpoint.failed_nodes` (when `--retry-failed` is OFF) and
  `checkpoint.blocked_nodes` stay terminal Failed/Blocked; their dependents are
  cascaded Blocked (mirroring `execute.rs`'s existing Failed-dependents logic).
- **R5**: Nodes in `checkpoint.stale_running` and (when `--retry-failed` is ON)
  `checkpoint.failed_nodes` are left in `Pending` so the normal `ready` queue
  dispatches them as attempt 1.
- **R6**: `pidag auto`'s `dispatch_sdd` passes `--resume` by default (resume or
  No-op Fresh if none exists); a successful Fresh-then-resume cycle is the
  429/crash recovery path from the auto-driver design doc.
- **R7**: All existing 301 tests stay green; the 4 stub tests are promoted to
  real behavioral tests asserting dispatch counts and ready-queue contents.

### Non-Functional
- **R8**: No new dependencies. `sha2` is already present.
- **R9**: Resume startup overhead < 50ms for a 10-node DAG (existing latency test).
- **R10**: No `.unwrap()`/`.expect()`/`panic!()` in production code paths added.
- **R11**: No redb schema or `Store` trait changes (uses existing `get_run`/
  `terminal_set`/`list_nodes`/`put_node_state`).

## Architecture

```
pidag sdd spec.md --run --resume
   |  derives run_id = run_id_for_spec(abs_spec_path, spec_content)
   |  shells out:
   v
pidag run .pidag/<stem>.json --run-id <id> --resume [--retry-failed] \
        --concurrency N --vault .pidag/pidag.redb [--allow-paid]
   |
   |  store = RedbStore::open(vault)
   |  decision = load_checkpoint(store, run_id, retry_failed)   (unless --fresh)
   |  match decision:
   |     Fresh       -> Scheduler::new(..)            (normal run, id given)
   |     AlreadyDone -> print cached report, exit 0
   |     Resume      -> Scheduler::with_checkpoint(.., checkpoint)
   |  scheduler.run(allow_paid)
   v
Scheduler::execute()
   - if checkpoint present, apply at node_state/in_degree/ready init:
       completed_nodes  -> state=Done, record_terminal, dec dependents' in-degree
       terminal failed  -> state=Failed, dependents Blocked
       blocked          -> state=Blocked
       (stale_running / retryable failed -> state=Pending, dispatched normally)
```

## TDD Contract

| Test Name | Given | Expects |
|-----------|-------|---------|
| `test_run_id_deterministic` | same spec path + content | same run_id (12 hex) | *(exists)* |
| `test_run_id_changes_with_content` | path same, content differs | different run_id | *(exists)* |
| `test_checkpoint_load_empty_vault` | no prior run | `Fresh` | *(exists)* |
| `test_checkpoint_load_completed_run` | run `completed_at` set | `AlreadyDone` | *(exists)* |
| `test_checkpoint_load_partial_run` | 3 Done, no completed_at | `Resume` w/ 3 completed | *(exists)* |
| `test_scheduler_skips_done_nodes` | DAG A->C, checkpoint{A Done}, MockWorker counting | only C dispatched; A's `state` in report == "Done" | **PROMOTE** |
| `test_scheduler_decrements_indegree` | DAG A->C, checkpoint{A Done} | C's in-degree=0 -> C ready immediately; report C Done | **PROMOTE** |
| `test_scheduler_resets_stale_running` | checkpoint{B stale_running} | B re-dispatched (attempt 1), ends Done | **PROMOTE** |
| `test_retry_failed_flag_resets_failed` | checkpoint{E Failed, retry_failed=true} | E re-dispatched, ends Done | *(exists, behavior)* |
| `test_retry_failed_flag_off_skips_failed` | checkpoint{E Failed, retry_failed=false} | E stays Failed in report | **PROMOTE** |
| `test_fresh_flag_ignores_checkpoint` | `--fresh`, prior run exists | new run_id (deterministic) overwrite; Fresh decision path | **PROMOTE** |
| `test_resume_startup_latency` | 10-node checkpoint | load < 50ms | *(exists)* |
| `test_with_checkpoint_skips_completed_prefix` *(new)* | DAG A->B->C->D, checkpoint{A,B Done} | only C,D dispatched; report A-D all Done | **NEW** |

## Exit Criteria

- [ ] `cargo test -p pidag 2>&1 | grep -E "test result:" | awk '{p+=$4} END{print p}' | grep -q "^313$"`  *(301 baseline + ~12 promoted/new)*
- [ ] `cargo clippy -p pidag --lib -- -D warnings 2>&1 | tail -1 | grep -q "Finished\|empty"`
- [ ] `cargo fmt -p pidag -- --check`
- [ ] `pidag run --help 2>&1 | grep -q -- "--resume"` *(run subcommand now lists it)*
- [ ] `pidag sdd --help 2>&1 | grep -q -- "--resume"` *(already true via help text; verify still parses)*
- [ ] `pidag sdd specs/01-multiply-function.md --run --resume` does not error "unknown option: --resume"

## Guardrails
- Do NOT modify the redb schema or `Store` trait.
- Do NOT add dependencies.
- Do NOT touch `dispatch_node` retry/fallback logic (the 429 path stays byte-identical).
- No `.unwrap()`/`.expect()`/`panic!()` in added production code.
- Stay surgical: only `src/scheduler/{mod,execute}.rs`, `src/cli/{run,sdd}.rs`,
  `src/agent/run.rs`, `tests/checkpoint_resume_tests.rs`, and help text in
  `src/bin/pidag.rs`.
- Match existing style (stringly-typed states; do not introduce the NodeStatus enum here).

## Files to Modify
| File | Action | Description |
|------|--------|-------------|
| `src/scheduler/mod.rs` | MODIFY | Add `Scheduler::with_checkpoint()` storing `Option<Checkpoint>` |
| `src/scheduler/execute.rs` | MODIFY | Thread `Option<&Checkpoint>` into `execute()`; apply at init |
| `src/cli/run.rs` | MODIFY | Parse `--run-id`/`--resume`/`--fresh`/`--retry-failed`; call `load_checkpoint`; branch Fresh/AlreadyDone/Resume |
| `src/cli/sdd.rs` | MODIFY | Parse `--resume`/`--fresh`/`--retry-failed`; derive `run_id_for_spec`; pass through to `pidag run` |
| `src/agent/run.rs` | MODIFY | `dispatch_sdd` appends `--resume` |
| `src/bin/pidag.rs` | MODIFY | Help text: add `--run-id` to run usage line |
| `tests/checkpoint_resume_tests.rs` | MODIFY | Promote 4 stubs + add 1 new behavioral test |

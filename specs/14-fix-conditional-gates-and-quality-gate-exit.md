# pidag — spec-14: Honor conditional gates (fail-triggered nodes) + quality-gate exit honesty

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: HIGH (surfaced by the 2026-08-10 chromecast-tv-mirror live run
  `ed3bc4990bcd`; the SDD recovery loop failed to run ANY fix iteration)
- **Status**: DONE — implemented and committed in `2a60094` (Bug A gate semantics +
  Bug B quality-gate exit honesty + pi anti-loop prompt). Corrected 2026-08-10 during
  the spec audit; this line previously read "implementation NOT started" long after
  the work had landed. See `SPEC-AUDIT-2026-08-10.md` P1.
- **Completed**: `2a60094`

---

## Overview

The SDD driver builds a self-healing recovery chain with **conditional nodes**:

```
implement-iter1 -> quality-gate-1 -> validate-iter1 ─┤ gate:"validate-iter1:fail"
implement-iter2 (fix) ───────────────────► quality-gate-2 -> validate-iter2 ─┤ gate:"validate-iter2:fail"
implement-iter3 (final) ───────────────────────────────────────► quality-gate-3 -> validate-iter3
```

`implement-iterN+1` is declared `gate: Some("<validate-iterN>:fail")` — it is meant
to run a FIX pass **only when `validate-iterN` FAILS**, and to be **skipped** when
`validate-iterN` PASSES.

**Observed bug (live run)**: `validate-iter1` FAILED (nothing implemented yet), yet
`implement-iter2` was reported `NodeBlocked` and the DAG ended with
`failed_nodes: ["validate-baseline","validate-iter1"]` and **zero recovery
iterations executed**. The three-iteration self-healing loop never engages.

**Second independent bug**: `quality-gate` (shell) reports **PASS on real failure**
— `cargo fmt --check` and `cargo clippy -- -D warnings` failures are masked as
`passed:true`. Live evidence: `quality-gate-1` printed `"passed": true` while its
own `checks.fmt.passed==false` and `checks.test.passed==false`. Because gates fire
on the script's exit code, this corrupts the `"...:fail"` trigger signal.

**User direction** (2026-08-10): model exhaustion will NOT be exercised (paid plan),
so this is purely about optimizing/fixing pidag correctness. Plan first, implement,
then self-test by running `pidag sdd` against a purpose-built spec.

---

## Bug A — conditional `gate` is parsed but never evaluated

### Root cause (verified in `/projects/pidag` source)
- `src/core/dag.rs:28`: `Node { .. pub gate: Option<String> }`.
- `src/sdd/mod.rs:332,390`: the SDD fix nodes set `gate: Some("validate-iterN:fail")`.
- **`src/scheduler/execute.rs` never reads `node.gate`.** The failure path
  (`execute.rs:363-393`) unconditional-blocks every dependent:

```rust
} else if state.state == "Failed" {
    ...
    if let Some(deps) = dependents.get(&node_id) {
        for dependent in deps {
            let blocked_state = NodeState { state: "Blocked", .. };
            node_state[..].state = "Blocked";
            ...record_terminal(dependent, blocked);
        }
    }
}
```

So a FAILED `validate-iter1` force-blocks its dependent `implement-iter2` — the
opposite of what `gate:"validate-iter1:fail"` declares. The gate concept exists in
the model + README (`src/sdd/README.md:89-90`) but is entirely absent from the
scheduler.

### Requirements
- **A1 (fire-on-fail)**: When node `X` finishes FAILED, for each dependent `D`:
  if `D.gate == Some("X:fail")` then `D` becomes **ready to dispatch** (its fix
  pass) once all of `D`'s other deps are terminal. Otherwise `D` is Blocked.
- **A2 (skip-on-pass)**: When `X` finishes DONE, for each dependent `D`:
  if `D.gate == Some("X:fail")` then `D` is **skipped** (trigger condition false)
  and its own dependents may proceed (a skipped fix node is a no-op success).
- **A3 (all-deps-terminal)**: A gate is evaluated only when the dependent's
  in-degree reaches zero. A gated node may depend on several nodes; it fires only
  if its gate-source node ended FAILED **and** all other deps are terminal.
- **A4 (gate grammar)**: Only `Some("<id>:fail")` is recognized. Parse the `<id>`
  before `:fail` literally. Unknown/malformed gate → Legacy behavior (then the
  dependent falls back to normal dependency logic).
- **A5 (events)**: Emit coherent events — a gate-satisfied node dispatches
  (`NodeDispatched/Done`); a gate-unsatisfied node records `NodeSkipped` (new or a
  `Done` with a `skipped:true` flag), never `Failed`.
- **A6 (resume)**: `pidag sdd --resume` / checkpoints must reconcile skipped nodes
  (blocked set should not re-include a legitimately-skipped gate node).

### TDD contract
| id | dag | expects |
|----|-----|---------|
| A-skip | n1(done) ; n2 depends[n1], gate n1:fail | n2 not dispatched; recorded skipped |
| A-fire | n1(fail) ; n2 depends[n1], gate n1:fail ; n3 depends[n2] | n2 dispatched (fix), then n3 dispatched |
| A-block | n1(fail) ; n2 depends[n1], gate n2:other:fail | n2 Blocked (gate not satisfied) |
| A-multi | n2 depends[n1,n1b] ; n1 fail ; n1b done ; gate n1:fail | n2 dispatches only after both n1,n1b terminal |
| A-pass-skip-chain | n1(done) ; n2 gate n1:fail ; n3 depends[n2] | n2 skipped; n3 still dispatches |

---

## Bug B — quality-gate reports PASS on real fmt/clippy failure

### Root cause (verified in `skills/quality-gate/run.sh`)
```bash
FMT_RESULT=$(run_check fmt cargo fmt --check 2>/dev/null \
    || echo '{"passed":true,"duration_ms":0,"note":"no rustfmt"}')
CLIPPY_RESULT=$(run_check clippy cargo clippy -- -D warnings 2>/dev/null \
    || echo '{"passed":true,"duration_ms":0,"note":"no clippy"}')
```
`cargo fmt --check` / `cargo clippy -- -D warnings` exiting non-zero falls into the
`|| echo passed:true` branch → `PASSED` stays true → script exits 0 on real failure.
The intended "tolerate a missing tool" case wrongly legitimately reports genuine
failures as passes. (`check`/`test` branches already correctly report `passed:false`.)

**Note**: pidag's own `sdd` nodes call this script via `src/sdd/mod.rs`
(`quality_gate_script`), and the exit code feeds the `"...:fail"` gate signal — so
Bug B actively corrupts Bug A's trigger.

### Requirements
- **B1**: A check that RUNS and FAILS => `passed:false`, overall `passed:false`,
  non-zero exit.
- **B2**: Only a genuinely missing tool (`command -v rustfmt` / `command -v
  cargo-clippy`) may be skipped as `{"passed":true,"note":"tool not installed"}`.
  Probe presence BEFORE running; if present and fails → `passed:false`.
- **B3**: JSON summary and exit code agree (exit 0 iff `passed:true`).
- **B4**: Keep the same JSON output shape so `sdd`/UI parsing is unchanged.

### TDD contract (shell harness fixtures under `_tmp/qualgate_fixtures/`)
| id | fixture | expects |
|----|---------|---------|
| B-fmt-fail | repo where `cargo fmt --check` fails | checks.fmt.passed==false; overall passed==false; exit!=0 |
| B-clippy-fail | repo where `clippy -D warnings` fails | checks.clippy.passed==false; exit!=0 |
| B-fmt-missing | `rustfmt` not on PATH | checks.fmt.passed==true + note; overall pass OK |
| B-clean | clean repo | all passed; exit 0 |

---

## Exit criteria (shell commands returning 0)

- [ ] `cd /projects/pidag && cargo test 2>&1 | grep -q "test result: ok"`
- [ ] `grep -q 'gate' src/scheduler/execute.rs` (gate now read by scheduler)
- [ ] `bash _tmp/qualgate_fixtures/run-bad-fmt.sh 2>&1 | grep -q '"passed": false'`
- [ ] `bash _tmp/qualgate_fixtures/run-clean.sh 2>&1 | grep -q '"passed": true'`
- [ ] `pidag sdd _tmp/bug-a-bloodtest.md --run 2>&1 | grep -q 'implement-iter2'` (self-test fires fix node)

## Guardrails
- Do not change `Node::gate` storage format (`Option<String>`, `"<id>:fail"`).
- Do not regress 429-failover (`runtime-429-failover.md`) or `--thinking low`
  (checkpoint 8120e85).
- Non-goal: exhaustion testing (user: paid plan).
- `/projects/pidag` IS a git repo — commit as part of implementation.
- Do not modify the chromecast-tv-mirror crate in this work.

## Files to modify
| File | Change |
|------|--------|
| `src/scheduler/execute.rs` | Evaluate `node.gate` in failure/done propagation (A1-A3); emit skip/fire events (A5) |
| `src/core/event.rs` | Add `NodeSkipped` (or reuse Done with flag) if not present (A5) |
| `src/sdd/resume.rs` | Reconcile skipped gate nodes on resume (A6) |
| `tests/*` | A-* tests (see `tests/scheduler_tests.rs`, `tests/sdd_tests.rs`) |
| `skills/quality-gate/run.sh` | Tool-presence probe + honest pass/fail (B1-B4) |
| `_tmp/qualgate_fixtures/` | minimal cargo repos for B-* bash tests |

## Verification script
```bash
cd /projects/pidag && cargo test                    # A-* (and full suite green)
cargo build --release && cp target/release/pidag /root/.local/bin/pidag
cd _tmp && pidag sdd bug-a-bloodtest.md --run       # self-test: fix node fires
git add -A && git commit -m "spec-14: honor conditional gates; quality-gate exit honesty"
```

## Deep-plan notes / open decisions (resolve at implementation)
1. **Skip semantics**: after a gate node is skipped, its downstream dependents that
   had **only** it as a dep should become ready (they run on the "no fix needed"
   path). Confirmed by A-pass-skip-chain.
2. **In-degree bookkeeping**: the ready-push must branch on (gate-source-result) at
   the moment in-degree → 0, using the terminal state of the gate-source node. Keep
   a per-node terminal-state map (already in `node_state`/snapshot).
3. **Where to hook**: intercept at the two terminal branches (`Done` and `Failed`)
   in the `execute` loop before blanket-blocking dependents; compute per-dependent
   fire/skip/block. This is a targeted change — no redesign of the topology walk.
4. **Bug B ordering**: fix quality-gate FIRST (cheap, isolated) so its exit code is
   trustworthy; then Bug A gates key off it.
5. **Self-test via pidag**: after fixes, the highest-value verification is to run
   `pidag sdd` on a tiny throwaway spec where `validate-iter1` is forced to fail,
   and assert `implement-iter2` (the fix node) actually dispatches.

## Memory
- Store on completion: `pidag/specs/14-conditional-gates-quality-gate-exit` and
  `pidag/review/20260810-condgate-live-run-findings` (mirrors the chromecast run).

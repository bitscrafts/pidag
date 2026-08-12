# Spec: Weight-Seeded Flexible-DAG Batch Budget

**Project**: `.` (container: `/projects/pidag`)
**Topic**: `claude-pi-delegation`
**Depends-On**: spec-09 (carousel queue + `ProjectQueue::weight`), spec-10
(resume CLI wiring — provides the `pidag auto` single-task driver that this
spec tunes), the autonomy-driver design insight
(`claude-pi-delegation/refactor/pidag-auto-driver-weighted-resume-split-design`,
§6 open item "Whether `weight` should also seed the flexible-DAG batch
budget").

---

## Overview

`ProjectQueue::weight: f64` (default 1.0, spec-09) currently influences
**selection priority** in `select_from_workspace` (higher weight → picked
first). It does **not** yet change **how many specs** a project yields per
carousel batch — the daemon and `pidag auto` paths use a flat uniform `batch`
(`--batch N`, default 5) shared across all projects.

The autonomy-driver design left one lever explicitly open (§6): *"Whether
`weight` should also seed the flexible-DAG batch budget."* This spec closes
that open item with a small, surgical behavior: **a project's `weight` scales
how many of its entries the bounded carousel emits into the batch**, so a
weight-2.0 project contributes ~2× the slots per pass of a weight-1.0
project, while the global `--batch N` cap still bounds the pass.

This is a **tuning** feature, not a control-flow change — the carousel
remains round-robin, the daemon remains a single bounded pass, and resume /
single-task / split policy (spec-08, spec-10) are untouched. The scaling is
**opt-in via the existing `weight` field**, with weight 1.0 = current
behavior (no behavioral change for queues that never set `weight`).

## Requirements

### Functional

- **R1**: `carousel_bounded` accepts project weights and uses them to decide
  **per-project take counts** within the global `batch` cap. A project with
  weight `w` contributes `ceil(base_share * w)` slots per round-robin sweep,
  where `base_share` is allocated from the remaining batch budget.
- **R2**: Weights are read from `ProjectQueue::weight`. Projects whose queue
  file predates the field default to 1.0 (forward-compat already guaranteed
  by `#[serde(default = "default_weight")]`).
- **R3**: The global `--batch N` cap is **hard**: the weighted carousel never
  emits more than `N` entries total per pass, regardless of weights. If
  weights would oversubscribe, the remaining budget is distributed in
  round-robin order (fair among projects with leftover pending entries).
- **R4**: When all weights are 1.0 (the default / legacy case), the weighted
  carousel produces the **exact same ordering** as the existing
  `carousel_bounded` (no behavioral regression). This is the guardrail.
- **R5**: `run_daemon` / `check_dry_run_done` pass the project's `weight`
  into the carousel so the scale applies to the `pidag queue --daemon --batch
  N` path (current primary use site).
- **R6**: `--dry-run` output reflects the weighted order (used by tests +
  operator inspection), unchanged interface.

### Non-Functional

- **N1**: No new crate dependencies. Reuse `Vec`/arithmetic only.
- **N2**: No `unwrap`/`expect`/`panic!` in the new production code path.
- **N3**: Single-file or two-file change surface (the carousel fn + the
  daemon call site); no scheduler/CLI/selector edits.
- **N4**: Weight ≈ 0 (e.g. 0.01) must not yield **zero** slots forever — a
  nonzero-weight project must eventually get at least one slot per pass when
  pending entries exist and budget remains after higher-weight projects are
  served (anti-starvation, matches design §3(c)).
- **N5**: Backward-compat: a queue.json written without `weight` (legacy)
  decodes to weight 1.0 and behaves identically to today.

## Architecture

```
                         pidag queue --daemon --batch N
                                      │
                                      ▼
                         run_daemon(project_root, batch=N)
                                      │
              read .pidag/queue.json → ProjectQueue { weight, entries[..] }
                                      │
                                      ▼
            check_dry_run_done(project_root, batch=N, dry_run)
                                      │
                  pending = entries.filter(Pending)        (priority order)
                                      │
                                      ▼
           weighted_carousel_bounded(                 ← NEW fn (this spec)
                  projects: Vec<(f64 weight, Vec<entry>)>,
                  batch: usize
           ) -> Vec<entry>   (len ≤ N, weighted allocation, round-robin tie-break)
                                      │
                                      ▼
                  execute_entry(...) for each  (unchanged)
```

The new function lives next to `carousel_bounded` in `src/queue/execute.rs`.
It is the **only** production code change of substance; the daemon call site
switches from `pending.into_iter().take(batch)` to the new fn (single-project
case — weight comes from the one `ProjectQueue`). Multi-project callers (if
any via `--workspace`) pass `Vec<(weight, entries)>`.

### Allocation algorithm (single pass, bounded by `batch`)

```
weighted_carousel_bounded(projects: Vec<(f64, Vec<entry>)>, batch: usize):
    1. total_weight = Σ w_i  over projects with >0 pending entries
    2. remaining = batch
    3. for each project i (in round-robin order, ties broken by weight desc):
         share = max(1, round(remaining * w_i / total_weight))   if w_i > 0
         take_i = min(share, pending_i.len(), remaining)
         emit take_i entries
         remaining -= take_i
         if remaining == 0: break
    4. if remaining > 0: distribute leftovers round-robin among projects
       that still have pending entries (any weight > 0), one each, until
       budget exhausted or no pending left.   ← anti-starvation (N4)
    5. return emitted (in emission order)
```

Round-robin here means **round-robin across projects**, emitting each
project's `take_i` contiguously per sweep — same visible shape as today's
`carousel_bounded` (A-slots, B-slots, …) but with per-project slot counts
scaled by weight instead of the flat "one each" of the current impl.

## TDD Contract

| Test Name | Input | Expected Output |
|---|---|---|
| `test_weighted_carousel_unit_weights_match_flat` | two projects, weights [1.0, 1.0], 3 entries each, batch 4 | order == A/0, B/0, A/1, B/1 (identical to `carousel_bounded(projects, 4)`) |
| `test_weighted_carousel_high_weight_doubles_share` | A weight 2.0 (3 pending), B weight 1.0 (3 pending), batch 6 | A emits 4 slots, B emits 2 (2:1 ratio), len 6 |
| `test_weighted_carousel_batch_cap_is_hard` | A weight 9.0 (10 pending), B weight 1.0 (10 pending), batch 3 | total emitted len == 3, A gets all 3 (share max(1, round(3*9/10))=3), B gets 0 |
| `test_weighted_carousel_zero_weight_starves` | A weight 0.0 (5 pending), B weight 1.0 (5 pending), batch 5 | only B slots emitted, A gets 0 (w_i==0 → share 0) |
| `test_weighted_carousel_nonzero_weight_anti_starvation` | A weight 0.01 (10 pending), B weight 9.99 (1 pending), batch 5 | B takes its 1 first; A then takes ≥1 of remaining 4 (anti-starvation N4) |
| `test_weighted_carousel_handles_fewer_pending_than_share` | A weight 5.0 (1 pending), B weight 1.0 (10 pending), batch 6 | A emits 1 (only 1 available), leftover 5 distributed to B → B emits 5 |
| `test_weighted_carousel_empty_project_excluded` | A weight 5.0 (0 pending), B weight 1.0 (3 pending), batch 3 | A excluded (0 pending), B emits 3 |
| `test_run_daemon_weighted_batch_picks_more_from_heavy_project` *(integration)* | `_tmp/` two-project workspace, queue.json `{weight: 3.0}` for A, `{weight:1.0}` for B, batch 4, dry_run | emitted order has A entries ≥ B entries; len 4 |

All file-writing tests use `_tmp/...` (per workspace rule). The integration
test exercises the daemon → `check_dry_run_done` → weighted carousel path
end-to-end via `--dry-run` (no subprocess spawn, deterministic).

## Exit Criteria

- [ ] `cargo test -p pidag 2>&1 | grep -E "test result:" | awk '{p+=$4} END{print p}' | grep -qE "^3(0[2-9]|1[0-9])$"` *(302 baseline + ≥8 new weighted-carousel tests; upper bound flexible)*
- [ ] `cargo clippy -p pidag --lib -- -D warnings 2>&1 | tail -1 | grep -q "Finished\|empty"`
- [ ] `cargo fmt -p pidag -- --check`
- [ ] `cargo test -p pidag weighted_carousel 2>&1 | grep -E "test result:" | awk '{print $4}' | grep -qv "^0$"` *(the new tests run and pass)*
- [ ] `grep -n "fn weighted_carousel_bounded" src/queue/execute.rs` *(the new fn exists)*
- [ ] `grep -n "weighted_carousel_bounded" src/queue/daemon.rs` *(the daemon call site uses it)*
- [ ] `grep -n "weight" src/queue/execute.rs` *(weights flow into the carousel)*

## Guardrails

- **Do NOT** change selection priority logic in `select.rs` — that already
  uses weight. This spec is **only** about the batch budget / take counts.
- **Do NOT** add a CLI flag. The lever is the existing `weight` JSON field.
- **Do NOT** alter resume, split, or single-task policy (spec-08/spec-10).
- **Do NOT** remove or rewrite `carousel_bounded` — it stays as the
  flat-uniform reference implementation; the new fn wraps its logic with
  weighted allocation. Unit-weight parity (R4) is asserted by a test that
  calls both and compares.
- **Do NOT** introduce floats into `QueueEntry` or `SpecState` — weights
  stay on `ProjectQueue`.
- **Do NOT** add new dependencies.
- Default weights (1.0) MUST keep current behavior bit-for-bit (R4). Any
  divergence is a regression.

## Files to Modify

| File | Change |
|---|---|
| `src/queue/execute.rs` | **ADD** `pub fn weighted_carousel_bounded(projects: Vec<(f64, Vec<QueueEntry>)>, batch: usize) -> Vec<QueueEntry>` implementing the allocation algorithm above. Keep `carousel_bounded` unchanged as the unit-weight reference. |
| `src/queue/daemon.rs` | `check_dry_run_done`: after computing `pending` and reading `state.weight`, call `weighted_carousel_bounded(vec![(state.weight, pending)], batch)` instead of `pending.into_iter().take(batch).collect()`. Single-project case: the `Vec<(f64, Vec)>` has exactly one element. |
| `tests/queue_tests.rs` | **ADD** the 7 unit tests from the TDD contract (`test_weighted_carousel_*`), using the existing `_tmp/` helper pattern. Include the unit-weight-parity test that cross-checks against `carousel_bounded`. |
| `tests/queue_tests.rs` (or a new `tests/queue_weighted_daemon_tests.rs`) | **ADD** `test_run_daemon_weighted_batch_picks_more_from_heavy_project` integration test driving `run_daemon` with `dry_run=true`. |

## Implementation Notes

- Test count target is open (`exit-criteria` regex allows 302–319) because
  the exact count depends on how many unit tests land; the hard requirement
  is "≥8 new, all green."
- `round()` on `f64` is acceptable here — weights are operator-authored
  rounded values (1.0, 2.0, 3.0 typical); floating-point drift at ≤2dp is
  immaterial for "share of a small batch".
- Single-project daemon path is the only caller today; the multi-project
  signature is provided so a future `--workspace` caller wires in with no
  API change (forward-compat, matches how `ProjectQueue::weight` was added
  with `serde(default)`).

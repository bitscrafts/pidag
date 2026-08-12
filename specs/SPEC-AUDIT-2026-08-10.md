# pidag — Spec Inventory Audit (2026-08-10)

Method: every spec in `specs/` cross-checked against `git log`, the symbols present in
`src/`, and the CLI subcommand table in `src/bin/pidag.rs`. Status was **not** taken
from the spec files — they proved unreliable, which is itself finding P1.

---

## 1. Status ledger

| Spec | Verified status | Evidence |
|---|---|---|
| `01-redb-pool-fix` | **DONE** | `src/store/redb_pool.rs`, used by ui/mcp/rpc servers |
| `02-container-deployment` | **UNVERIFIED** | `deploy/` exists; all 9 exit criteria need podman on lnx. Known live defect: `PI_PROVIDER` baked into `Containerfile` (~L120) |
| `03-multi-project-workspace` | **DONE** | `src/queue/discover.rs`, `src/ui/workspace*.rs` |
| `04-workspace-frontend` | **DONE** | `src/ui_assets/index.html` routes |
| `05-spec-naming-enforcement` | **DONE** | `validate_spec_name` in `src/sdd/`, `src/cli/sdd.rs` |
| `06-spec-queue` | **DONE** | `src/queue/*`, `pidag queue` |
| `07-spec-split` | **DONE** | `src/split/*`, `pidag split` |
| `08-checkpoint-resume` | **DONE** | `ce3dc8c`, `67e6954` |
| `09-carousel-queue` | **DONE** | `src/queue/daemon.rs` |
| `10-resume-cli-wiring` | **DONE** | `67e6954` |
| `11-weight-seeded-batch-budget` | **DONE** | `f857776`, 8/8 tests |
| `12-iteration-aware-session-hardening` | **MISFILED** | Advisory; targets the *pi harness* + skills, not `pidag/src`. See P3 |
| `13-sdd-driver-fixes` | **DONE** | `0a1f9c8`, `0d08c19` |
| `14-fix-conditional-gates…` | **DONE** (status line lied) | `2a60094`; file said "implementation NOT started" |
| `15-session-backed-worker` | **SUPERSEDED** | `c0dba5a` landed, but built on the wrong `--session` semantics (ANALYSIS §1.2) |
| `16-rpc-backed-worker` | **DONE-BUT-BROKEN** | `adaeaa0` landed; cannot work against real `pi` (ANALYSIS §1) |
| `17-pi-binary-contract-tests` | **NEW — pending** | this session |
| `18-adopt-pi-sdk-rpc-transport` | **NEW — pending** | this session |
| `21-pluggable-agent-backend` | **NEW — pending** | this session; `AgentBackend` seam, blocks 18 |
| `22-rpc-mcp-server-correctness` | **NEW — pending** | this session; replaces `99-production-hardening` P0/P1 |
| `97-a2a-worker` | **DONE** | `src/a2a/worker.rs`, routed in `type_dispatch.rs` |
| `98-a2a-mcp-bridge` | **PARTIAL** | Worker + MCP client done; **A2A server mode unreachable** — see P2 |
| `92-configurable-models` | **DONE** | `ModelsConfig`, priority chain |
| `94-dag-mermaid-describe` | **DONE** | `pidag describe` |
| `99-production-hardening` | **SUPERSEDED-BY 22** (P0/P1) | P2 (MCP server) shipped as `pidag mcp`; P0/P1 corrected inventory in spec-22 §0 — see P4 |
| `95-project-overview-ui` | **DONE** | `src/ui/handlers.rs` |
| `93-runtime-429-failover` | **DONE** | `classify_retryable`, model chains |
| `91-shell-node-dispatch` | **DONE** | `RealShellWorker`, empty-models handling |
| `96-sssf-patterns-adoption` | **P1 DONE, P2-P5 NEVER SPLIT** | see P5 |
| `90-type-dispatch-worker` | **DONE** | `src/worker/type_dispatch.rs` |

**Bottom line: 20 done, 2 new, 5 with real problems.**

---

## 2. Problems found

### P1 — Spec status is not machine-readable, and is sometimes false *(process defect)*

Three different header conventions coexist (`**Status**: APPROVED`,
`- **Status**: PLAN APPROVED`, and no status at all), exit-criteria checkboxes are
almost never ticked on completion (`sssf-patterns-adoption` is the only file using
`[x]`), and at least one status was **actively wrong**: spec-14 read "implementation
NOT started" while the work was committed in `2a60094`.

Reconstructing this ledger required git archaeology. A fresh agent reading `specs/`
would draw the wrong conclusion about at least four specs — exactly the failure that
already happened this week with `HANDOFF.md`.

**Fix**: one mandatory front-matter block on every spec —
`Status: PLANNED | IN-PROGRESS | DONE | SUPERSEDED-BY <n> | ABANDONED`, plus
`Completed: <commit>`. Cheap follow-on: make `pidag queue` refuse to schedule a spec
without a parseable status.

### P2 — `98-a2a-mcp-bridge`: 278 lines of unreachable server code

`src/a2a/server.rs` exists and compiles, but there is **no `a2a` subcommand** in
`src/bin/pidag.rs` (subcommands: run, show, list, attach, sdd, split, queue, auto,
serve, mcp, ui, describe) and no reference to `a2a::server` anywhere outside its own
module. The spec's exit criterion `pidag a2a --port 8080` serves an Agent Card cannot
pass.

**Decision required, not more code**: either wire the subcommand, or delete the module.
ANALYSIS §3 flags that pi's own extension/tool system may make the whole A2A/MCP worker
layer redundant — resolve that question *before* investing in either direction.

### P3 — `12-iteration-aware-session-hardening` is in the wrong repository

It is explicitly "implemented piecemeal in the host repo, NOT in pidag/src", and its
R1/R2 request **new features from `pi_agent_rust`** (pending-edits persistence across
compaction, `pi --replay-pending-edits`). R3/R4 already shipped as skill changes.

Keeping it in `pidag/specs/` means the queue can pick it up as pidag work and never
satisfy it. **Move to the research topic docs** (`docs/claude-pi-delegation/`), or —
better, now that `_upstream/pi_agent_rust` is a real checkout — reopen R1/R2 as an
upstream contribution.

### P4 — `99-production-hardening` P0 items — **CORRECTED 2026-08-10, see spec-22**

> **This finding was wrong as first written.** It grepped for the *wire* method names
> (`node.retry`, `last_seq`) rather than the Rust function names, and concluded the
> handlers were absent. They exist. Corrected inventory (read from source, not grepped)
> is in `22-rpc-mcp-server-correctness.md` §0: R1/R2/R5/R7 are **DONE**, R3 is
> **PARTIAL**, R4 is a **STUB**, R6 is implemented but **INERT**.
>
> The real defect found while re-verifying is worse and sits underneath all of them:
> **`dag.submit` constructs a `Scheduler` and never runs it** (`src/rpc/handlers.rs:103`
> — no `.run()` call exists anywhere in `src/rpc/` or `src/mcp/`). The server accepts
> DAGs, reports `"submitted"`, and executes nothing; `completed_at` is never set, so the
> TTL sweep never collects either. Both transports are affected. Full treatment in
> spec-22.

Original text, retained for the record: *"Verified absent from `src/`: `node.retry` /
`node_retry`, resume tokens (`dag_id:last_seq`), `dag.result` returning real artifacts,
run TTL cleanup. Only `uuid_short` and the P2 MCP server mode landed."*

The spec is also **stale against the current layout**: it targets `src/rpc.rs` and
proposes `src/mcp_server.rs — NEW, replaces src/rpc.rs`, but the code has since been
refactored into `src/rpc/` and `src/mcp/` directories. The file cannot be implemented
as written.

**Fix**: rewrite the surviving P0/P1 items against the current module layout as
`spec-22 — pidag RPC/MCP server correctness` (written 2026-08-10). Do not attempt the file as-is.

### P5 — `96-sssf-patterns-adoption` P2-P5 were deferred to specs that were never written

Its own exit criterion says each of P2-P5 must get its own spec file
(`specs/envelopes.md`, `specs/gates.md`, `specs/correction-loop.md`,
`specs/writes-boundary.md`) before implementation. **None exist.** P1 (Trace UI) is
done.

Note the overlap: **P3 "Gates as First-Class Predicates" was substantially delivered by
spec-14** (fire/skip/block gate semantics in the scheduler). The sssf umbrella should
be updated to reflect that rather than leaving a phantom backlog item.

### P6 — Two naming conventions, and the tool rejects its own specs — **FIXED 2026-08-10**

Ten specs used bare names (`a2a-worker.md`, `configurable-models.md`,
`production-hardening.md`, …) while spec-05 makes `pidag sdd` **reject** anything not
matching `^[0-9]{2}-.*\.md$`. pidag could not run its own legacy specs.

**Applied**: all ten renumbered via `git mv` into a **`9x` historical block**, ordered
by actual implementation chronology:

| new | old | | new | old |
|---|---|---|---|---|
| `90-type-dispatch-worker` | `type-dispatch-worker` | | `95-project-overview-ui` | `project-overview-ui` |
| `91-shell-node-dispatch` | `shell-node-dispatch` | | `96-sssf-patterns-adoption` | `sssf-patterns-adoption` |
| `92-configurable-models` | `configurable-models` | | `97-a2a-worker` | `a2a-worker` |
| `93-runtime-429-failover` | `runtime-429-failover` | | `98-a2a-mcp-bridge` | `a2a-mcp-bridge` |
| `94-dag-mermaid-describe` | `dag-mermaid-describe` | | `99-production-hardening` | `production-hardening` |

**Why `9x` and not `19-28`**: these are the *oldest* specs (2026-08-01 → 08-05). Giving
them the highest *active* numbers would misrepresent the timeline and collide with the
17-21 roadmap. The `9x` block reads unambiguously as "historical, already landed", and
leaves `19+` free for forward work.

**Naming convention going forward**: `NN-<slug>.md` for anything runnable by
`pidag sdd`; non-spec documents (`ANALYSIS-*`, `SPEC-AUDIT-*`) deliberately carry **no**
numeric prefix so the queue never schedules them as work.

### P7 — Stale `crates/pidag/` paths — **FIXED 2026-08-10** (source refs; spec bodies remain)

Several files still referenced `crates/pidag/...` from before the move to
`/projects/pidag` — criteria and doc links that can never resolve.

**Applied**: all live references in `src/`, `tests/`, `CLAUDE.md` and the active specs
re-pathed to `specs/NN-…` (comment/doc text only — no logic touched). `HANDOFF.md` was
deliberately left alone: it is a historical narrative, and rewriting past entries would
falsify the record.

**Still open**: exit-criteria *inside* the `9x` spec bodies (e.g.
`96-sssf-patterns-adoption`'s `grep -q "pidag ui" crates/pidag/src/bin/pidag.rs`) are
unchanged. Those specs are DONE, so the stale criteria are inert — re-path them only if
a spec is reactivated.

---

### P8 — 13 pre-existing clippy errors under `--all-targets` (found 2026-08-10)

`cargo clippy --all-targets -- -D warnings` **fails** with 13 errors, all in test files:

| file | count | lints |
|---|---|---|
| `tests/split_tests.rs` | 4 | `manual_range_contains` ×2, `len_zero`, `useless_vec` |
| `tests/type_dispatch_tests.rs` | 3 | `await_holding_lock` ×3 |
| `tests/queue_tests.rs` | 2 | `ptr_arg`, `useless_vec` |
| `tests/checkpoint_resume_tests.rs` | 1 | `unwrap_or_default` |
| `tests/mcp_server_tests.rs` | 1 | `useless_vec` |
| `tests/redb_pool_fix_tests.rs` | 1 | `len_zero` |
| `tests/tdd_contract_tests.rs` | 1 | `unnecessary_map_or` |

Confirmed **pre-existing** by running clippy in a `git worktree` at `9b64d27` (before any
of today's work) — identical files, lints and line numbers. The repo's own gate
(`CLAUDE.md`) is `cargo clippy -p pidag -- -D warnings`, which passes cleanly, so this
debt has never been visible to the quality gate.

**Spec correction applied**: specs 17, 18, 21 and 22 originally specified
`cargo clippy --all-targets -- -D warnings` as an exit criterion — a bar this repo has
never met. That was an authoring error, not an implementation failure; all four now
specify the repo's actual gate. `await_holding_lock` in `type_dispatch_tests.rs` is the
only one worth a second look on merit; the rest are cosmetic. Clean them in their own
change, never as a side effect of feature work.

| Action | Rationale |
|---|---|
| **15 + 16 → SUPERSEDED-BY 18** | Both are attempts to give the worker session continuity through a misread CLI contract. spec-18 delivers the capability correctly via `RpcTransportClient`. Do not implement either further. |
| **sssf P3 (gates) → folded into spec-14** | Already delivered. Mark it done in the umbrella; do not write `specs/gates.md`. |
| **`99-production-hardening` P0/P1 → SUPERSEDED-BY 22** | Written 2026-08-10. Re-verification found the handlers exist but `dag.submit` never runs the scheduler — a worse defect than the audit first reported. |
| **a2a-mcp-bridge → blocked pending decision** | Do not extend until the pi-extension-overlap question (ANALYSIS §3) is answered. |
| **12 → move out of `pidag/specs/`** | Not pidag work; belongs upstream or in the research docs. |
| **sssf P2/P4/P5 → keep deferred, but say so** | Legitimate future work; the umbrella should carry an explicit `DEFERRED` marker so it stops reading as open backlog. |

**Nothing else should be merged.** The remaining DONE specs are coherent historical
records and are best archived, not rewritten.

---

## 4. Recommended roadmap after this audit

**Implementation order is 17 → 21 → 18 → 19 → 20.** Spec numbers are allocation order,
not execution order: spec-21 (the `AgentBackend` seam) was written after 18 but must land
before it, so the pi transport work goes in behind the abstraction rather than being
refactored out of a worker afterwards. **spec-22 is independent** of the whole pi track
and can be implemented in parallel by another agent — it touches only `src/rpc/`,
`src/mcp/` and config.

| # | Spec | State | Order |
|---|---|---|---|
| 17 | Real-binary contract tests | **written this session** | 1st |
| 21 | Pluggable `AgentBackend` seam + `MockBackend` + conformance battery | **written this session** | 2nd |
| 18 | `PiBackend` behind that seam; delete `worker/rpc.rs` | **written this session** | 3rd |
| 19 | Session-per-DAG-path via `fork`; make `--concurrency` real | planned (ANALYSIS §5.2, §5.7) | 4th |
| 20 | Token-aware budgeting (`get_state`) + `compact` on pressure | planned (ANALYSIS §5.4, §5.5) | 5th |
| 22 | RPC/MCP server correctness — the server must actually run DAGs | **written this session** | parallel |
| — | A2A/MCP overlap decision | blocks any further a2a work (P2) | — |

Immediate, ungated by any spec: set `PI_NO_RPC=1` (or invert the gate at
`type_dispatch.rs:59`) so the default LLM path stops failing.

## 5. A pattern worth naming: unverified success shapes

Three independent defects found this week share one shape — **a component reporting
success it never verified**:

| where | claim | reality |
|---|---|---|
| `skills/quality-gate/run.sh` (spec-14 Bug B) | `passed: true` | `cargo fmt --check` had failed; masked by `\|\| echo passed:true` |
| `dag.submit` (spec-22) | `status: "submitted"` | the scheduler is constructed and never run |
| `node.retry` (spec-22 H5) | `queued: true` | nothing is servicing a queue |

Each was invisible to the test suite because the tests asserted on the *reported* shape
rather than the *effect*. The generalised rule is spec-22 H8: **no handler returns a
field asserting an action occurred unless it verified the action occurred** — and, at the
test level, assert on the effect (store state, process state) rather than the response
body.

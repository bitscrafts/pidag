# SSSF Patterns Adoption — Trace UI, Envelopes, Gates, Correction, Writes

- **Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
- **Crate**: `pidag`
- **Reference**: [super-simple-software-factory](https://github.com/disler/super-simple-software-factory)
  (IndyDevDan's Python reference implementation)

---

## Overview

`super-simple-software-factory` (SSSF) is IndyDevDan's concrete
implementation of the Software Factory concept — the same concept
`docs/indydevdan-agentic-engineering` documents philosophically. SSSF and
pidag solve the **same problem** (deterministic orchestration of `pi`
coding agents) with different tradeoffs.

SSHF is a **reference implementation** pidag can learn from. This spec
plans the adoption of five SSSF patterns into pidag, in priority order:

| # | Pattern | Impact | Effort | This spec |
|---|---|---|---|---|
| P1 | **Trace UI** (web visualizer) | highest | medium | **detailed** |
| P2 | Typed envelopes | high | medium | outlined |
| P3 | Gates as first-class predicates | medium | medium | outlined |
| P4 | Correction loop (session-resumable retry) | high | high | outlined |
| P5 | Per-agent writes boundary | medium | low | outlined |

pidag is already ahead of SSSF on: DAG parallelism (diamond deps,
`ready_nodes`), ModelRef fallback chain (free → paid 429 failover),
durable redb vault (crash-tested), A2A dispatch (specced). These must NOT
regress.

---

## P1. Trace UI (detailed) — Web Visualizer

### Motivation

SSHF ships a Vue + Vite visualizer served by Bun on port 4600, backed by
a SQLite WAL trace db. It shows: sessions list, trace waterfall, per-
phase tool-call detail. Readers poll SQLite with a cursor query — no
WebSocket, no ingest endpoint. Live view and full history are the same
query at different cadence.

pidag has redb event log + terminal `render_status`. A web UI with the
same observability would make DAG runs visible mid-flight, not just
after completion. This is the highest-impact pattern to adopt.

### Architecture

```
pidag ui --port 4600 --vault .pidag/
    │
    ├── axum web server (Rust)
    │       ├── GET /              → embedded static index.html
    │       ├── GET /api/runs      → JSON: list all runs
    │       ├── GET /api/runs/:id  → JSON: run metadata + node states
    │       ├── GET /api/runs/:id/events?since=N → JSON: events after seq N
    │       └── GET /api/health    → JSON: {"status":"ok"}
    │
    ├── RedbStore (Arc<dyn Store>)
    │       └── range queries over run prefix + event seq
    │
    └── Static frontend (embedded in binary)
            ├── index.html          (sessions list + run detail SPA)
            ├── app.js               (vanilla JS, fetch() polling every 2s)
            └── style.css            (minimal, PicoCSS-class)
```

### Design decisions

**Why axum?** It is the standard async web framework in the Rust
ecosystem, maintained by Tokio, and integrates cleanly with pidag's
existing tokio runtime. Alternatives: `actix-web` (heavier), `tiny_http`
(sync, no async), hand-rolled `hyper` (too low-level). axum adds `axum`,
`tower`, `hyper` to the dep tree — all are widely-used, well-maintained
crates. This is the first new runtime dep for pidag and is justified by
the feature.

**Why vanilla JS (not Vue/Vite)?** A self-contained `pidag ui` binary
that just works with no Node/Bun toolchain is more idiomatic for a Rust
CLI. The frontend is simple enough (sessions list + trace waterfall +
polling) for vanilla JS with `fetch()`. The static files are embedded in
the binary via `include_str!` — no external assets, no build step.

**Upgrade path**: If the vanilla UI grows complex, swap `app.js` for a
Vue/Vite bundle (build step produces `dist/`, embedded via
`include_dir!`). The backend API stays the same. This is a future
decision, not a v1 concern.

**Why polling (not WebSocket)?** SSSF proved polling is sufficient. The
cursor query `events?since=N` returns only new events. Polling every 2s
gives near-real-time visibility without the complexity of WebSocket
lifecycle management. The redb WAL store supports concurrent readers
(reads never block the running writer), so polling is safe.

### Requirements

**R1**: New subcommand `pidag ui` in `bin/pidag.rs`. Flags:
- `--port <N>` (default 4600)
- `--vault <PATH>` (default `.pidag/`)
- `--host <ADDR>` (default `127.0.0.1` — local only by default)

**R2**: New module `crates/pidag/src/ui.rs` containing the axum server
setup, route handlers, and the `UiState` struct holding `Arc<dyn Store>`.

**R3**: New `Store` trait methods needed by the UI:
- `async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError>` — list
  all runs (new; currently only `get_run` exists).
- `async fn load_events_since(&self, run_id: &str, since: u64) ->
  Result<Vec<(u64, Event)>, PidagError>` — events with seq > `since`
  (new; currently `load_events` returns all with no seq).

`RedbStore` and `MockStore` both implement these. The P1-3 range-query
fix already added prefix iteration over runs; `list_runs` reuses it.

**R4**: HTTP API (all return JSON):

| Method | Path | Returns |
|---|---|---|
| GET | `/api/health` | `{"status":"ok","runs":N}` |
| GET | `/api/runs` | `[{"run_id","started_at","completed_at","successful_nodes","failed_nodes"}]` |
| GET | `/api/runs/:run_id` | `{"run":{...},"nodes":[{...}]}` |
| GET | `/api/runs/:run_id/events?since=N` | `[{"seq":N,"event":{...}}]` |
| GET | `/api/runs/:run_id/status` | `{"text":"<render_status output>"}` |

**R5**: Static frontend embedded in the binary via `include_str!`:
- `crates/pidag/src/ui_assets/index.html` — single-page app shell
- `crates/pidag/src/ui_assets/app.js` — vanilla JS: sessions list, run
  detail, trace waterfall, polling loop
- `crates/pidag/src/ui_assets/style.css` — minimal styling

The frontend is a **single-page app** with two views:
1. **Sessions list** (`/`): table of runs. Click a row → run detail.
2. **Run detail** (`#/run/:id`): header (run status, node counts),
   trace waterfall (timeline of events), node list (expandable to show
   output).

**R6**: Trace waterfall renders events as a horizontal timeline:
```
DagSubmitted ──> NodeDispatched(node1, glm-5.2) ──> NodeDone(node1) ──> NodeDispatched(node2) ──> NodeFailed(node2) ──> NodeRetry(node2) ──> NodeDone(node2) ──> DagDone
```
Each event is a colored block: `DagSubmitted` (gray), `NodeDispatched`
(blue), `NodeDone` (green), `NodeFailed` (red), `NodeRetry` (yellow),
`ProviderFallback` (orange), `NodeBlocked` (dark gray), `NodeSkipped`
(light gray). Click an event → expand details (model, output, error).

**R7**: Polling: the run detail view calls
`/api/runs/:id/events?since=<last_seq>` every 2 seconds. New events
append to the waterfall. When `DagDone` is received, polling stops and
the status badge flips to "complete" or "failed".

**R8**: `print_help()` updated to document `pidag ui`.

### TDD contract (Trace UI)

| # | Test | Description |
|---|---|---|
| T1 | `test_ui_list_runs_returns_json` | `GET /api/runs` returns `Vec<RunMeta>` as JSON from a `MockStore` with 2 runs |
| T2 | `test_ui_get_run_returns_nodes` | `GET /api/runs/:id` returns run metadata + node states |
| T3 | `test_ui_events_since_filters_by_seq` | `GET /api/runs/:id/events?since=5` returns only events with seq > 5 |
| T4 | `test_ui_health_endpoint` | `GET /api/health` returns `{"status":"ok","runs":N}` |
| T5 | `test_ui_status_endpoint` | `GET /api/runs/:id/status` returns `render_status` output as JSON |
| T6 | `test_ui_serves_index_html` | `GET /` returns the embedded `index.html` with content-type `text/html` |
| T7 | `test_list_runs_store_method` | `MockStore::list_runs` returns all inserted runs |
| T8 | `test_load_events_since_store_method` | `MockStore::load_events_since` filters by seq > N |

Tests use `axum::TestServer` or `reqwest` against a bound `127.0.0.1:0`
ephemeral port. The `MockStore` is sufficient (no redb needed for API
shape tests).

---

## P2. Typed Envelopes (outlined)

### Motivation

SSHF's `EnvelopeBase` (Pydantic) gives structured output from each
phase: `status`, `summary`, `artifacts`, `notes_for_next_agent`, plus
phase-specific fields (`changed_files`, `commit_message`). pidag passes
plain-text `String` output. A typed envelope (serde in Rust) makes node
output machine-parseable, enabling gates (P3) and automatic context
injection.

### Requirements (outlined)

**R9**: New `Envelope` struct in a new `crates/pidag/src/envelope.rs`:
```rust
pub struct Envelope {
    pub status: EnvelopeStatus,        // Success | Fail
    pub summary: String,
    pub artifacts: Vec<String>,         // file paths written
    pub notes_for_next_agent: String,
    pub raw_text: String,               // backward-compat: the full pi output
}
```

**R10**: `WorkerOutput.output` stays `String` (backward compat). The
`Envelope` is parsed from `output` by the scheduler when a node declares
`envelope: true` in its config. Nodes that don't opt in are unaffected.

**R11**: When a node has `envelope: true` and the output parses as an
`Envelope`, the scheduler injects the parent's `summary` +
`notes_for_next_agent` into the child's prompt (instead of the raw
output).

**R12**: On parse failure with `envelope: true`, the scheduler treats it
as a retryable failure (the agent didn't return structured output).
Combined with P4 (correction loop), this re-prompts the same session
asking for valid JSON.

### TDD contract (P2, outlined)

- T9: `Envelope::from_json` parses valid envelope
- T10: `Envelope::from_json` returns `None` on invalid JSON
- T11: Node with `envelope: true` and valid output → child prompt
  contains `summary` + `notes_for_next_agent`
- T12: Node with `envelope: true` and invalid output → `retryable: true`

---

## P3. Gates as First-Class Predicates (outlined)

### Motivation

SSHF's gates run **after** the agent call against the envelope's own
declarations: `artifacts_exist`, `files_non_empty`, `json_parses`,
`diff_matches_claims`, `tests_pass`. A gate is
`gate(envelope, run) -> GateReport` with one `check(item, ok, note)` per
verified thing. pidag's `validate` is a single script string — less
structured, less composable.

### Requirements (outlined)

**R13**: New `Gate` trait in a new `crates/pidag/src/gate.rs`:
```rust
pub trait Gate: Send + Sync {
    fn check(&self, envelope: &Envelope, ctx: &GateContext) -> GateReport;
}

pub struct GateReport {
    pub checks: Vec<GateCheck>,
}
pub struct GateCheck {
    pub item: String,
    pub ok: bool,
    pub note: String,
}
```

**R14**: Built-in gates: `ArtifactsExist`, `FilesNonEmpty`,
`TestsPass { command: String }`, `DiffMatchesClaims`.

**R15**: `Node` gains an optional `gates: Vec<GateConfig>` field (JSON
config, materialized into `Box<dyn Gate>` at dispatch time). Gates run
after `Worker::run` against the parsed `Envelope`. Any failed gate →
`WorkerOutput { success: false, retryable: true }`.

### TDD contract (P3, outlined)

- T13: `ArtifactsExist` gate passes when declared artifacts exist
- T14: `ArtifactsExist` gate fails when declared artifacts are missing
- T15: `TestsPass` gate runs the command and returns `ok: true` on exit 0
- T16: Node with a failing gate → `success: false, retryable: true`

---

## P4. Correction Loop — Session-Resumable Retry (outlined)

### Motivation

SSHF re-prompts the **same pi session** with the failure feedback
(parse error, gate violation) instead of cold-restarting with backoff.
The context window stays intact — the agent already learned about the
codebase, and a correction costs one message. pidag's `RetryPolicy`
cold-restarts on every attempt, throwing away context.

### Requirements (outlined)

**R16**: Change `PiPrintWorker` from `pi -p` one-shot to `pi
--session-id <uuid>` create-or-continue. The first attempt creates a
session; subsequent attempts (within the same node dispatch) continue
it.

**R17**: On `WorkerOutput { success: false }`, instead of spawning a
new `pi -p` process, the scheduler re-prompts the same session with a
correction message: "Your previous output failed because: <error>.
Please fix and return the output again." The session's context window
preserves the agent's prior reasoning.

**R18**: The correction loop is bounded by `RetryPolicy.attempts`. When
attempts are exhausted, the session is abandoned and the node fails
(existing behavior).

**R19**: The `--session-id` is stored per-node-dispatch in the redb
vault, so a crashed-and-resumed run can continue the same session.

### TDD contract (P4, outlined)

- T17: First attempt creates a pi session; second attempt continues it
- T18: Correction message includes the failure reason
- T19: Exhausted attempts → node fails (existing behavior preserved)
- T20: Resumed run continues the same pi session from the vault

---

## P5. Per-Agent Writes Boundary (outlined)

### Motivation

SSHF enforces "this agent may only write to these paths" in code,
diffing the repo before/after the agent call and rolling back
unauthorized changes. pidag has no such boundary — a `pi -p` worker can
write anywhere. This is a safety feature for production use.

### Requirements (outlined)

**R20**: `Node` gains an optional `writes: Vec<PathBuf>` field. When
present, after `Worker::run`, the scheduler diffs the repo (via `git
diff --name-only` or a file-walk) and rolls back any changes outside the
declared `writes` paths.

**R21**: Unauthorized changes cause the node to fail with
`WorkerOutput { success: false, output: "agent wrote outside declared
writes: <paths>" }`.

**R22**: When `writes` is absent (the default), no boundary is
enforced (backward compat — existing DAGs are unaffected).

### TDD contract (P5, outlined)

- T21: Node with `writes: ["src/"]` — agent writes to `src/foo.rs` →
  change is kept, node succeeds
- T22: Node with `writes: ["src/"]` — agent writes to `Cargo.toml` →
  change is rolled back, node fails
- T23: Node without `writes` — agent writes anywhere → no enforcement

---

## Exit Criteria

### P1 (Trace UI) — the immediate deliverable
- [x] `cargo test -p pidag` passes (139 existing + 8 new UI tests = 147)
- [x] `cargo clippy -p pidag --lib -- -D warnings` clean
- [x] `cargo fmt -p pidag -- --check` clean
- [x] `grep -q "pidag ui" crates/pidag/src/bin/pidag.rs`
- [x] `grep -q "list_runs" crates/pidag/src/store/mod.rs`
- [x] `grep -q "load_events_since" crates/pidag/src/store/mod.rs`
- [x] `pidag ui --port 4600` serves a sessions list at `http://127.0.0.1:4600/`
- [x] A running DAG's events appear in the waterfall within 2s (polling)
- [x] `Cargo.toml` adds `axum` (and its transitive deps)

### P2-P5 (future phases)
- [ ] Each pattern has its own spec file (`specs/envelopes.md`,
      `specs/gates.md`, `specs/correction-loop.md`, `specs/writes-boundary.md`)
      before implementation begins
- [x] No pattern regresses pidag's existing strengths (DAG parallelism,
      ModelRef fallback, redb durability, A2A dispatch)

---

## Guardrails

- Must NOT change the `Worker` trait signature (envelopes are opt-in
  via node config, not a trait change)
- Must NOT regress any existing test
- Must NOT add WebSocket (polling is sufficient, matches SSSF)
- Must NOT require Node/Bun to build or run the UI (vanilla JS,
  embedded in the binary)
- The `pidag ui` server must be local-only by default (`127.0.0.1`),
  not `0.0.0.0` (safety — the UI has no auth)
- P2-P5 must each get their own spec before implementation (this
  document is the umbrella plan; each pattern needs a focused TDD
  contract before code touches)
- The trace UI must work with `MockStore` (tests) and `RedbStore`
  (production) — no store-specific coupling in the UI layer

---

## Files to Modify (P1 only — P2-P5 get their own specs)

| File | Change |
|---|---|
| `crates/pidag/Cargo.toml` | Add `axum`, `tower`, `hyper` (web server deps) |
| `crates/pidag/src/ui.rs` | NEW — axum server, route handlers, `UiState` |
| `crates/pidag/src/ui_assets/index.html` | NEW — SPA shell (embedded via `include_str!`) |
| `crates/pidag/src/ui_assets/app.js` | NEW — vanilla JS: sessions list, run detail, polling |
| `crates/pidag/src/ui_assets/style.css` | NEW — minimal styling |
| `crates/pidag/src/store/mod.rs` | Add `list_runs` + `load_events_since` to `Store` trait |
| `crates/pidag/src/store/redb_store.rs` | Implement `list_runs` + `load_events_since` |
| `crates/pidag/src/lib.rs` | Re-export `ui` module |
| `crates/pidag/src/bin/pidag.rs` | Add `ui` match arm + `ui_subcommand`; update `print_help()` |
| `crates/pidag/tests/ui_tests.rs` | NEW — 8 tests (T1-T8) |
| `crates/pidag/HANDOFF.md` | Toggle Next Steps #9 `[ ]`→`[x]` after completion |

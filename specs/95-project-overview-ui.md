# Project Overview UI — Whole-Project View (Specs + Runs)

- **Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
- **Crate**: `pidag`

---

## Overview

The trace UI currently has two views: a sessions list (`#/` — all runs in the
vault) and a run detail (`#/run/<id>` — one DAG's Gantt timeline + event
waterfall). But a pidag **project** is bigger than any single run: it is a
collection of **phases**, each defined by a **spec** in
`crates/pidag/specs/*.md`, and each spec may produce one or more DAG runs.

The user wants to "check the whole project in the web UI" — i.e. see every
phase (spec) and its status at a glance, alongside the runs, so the web UI
becomes a project dashboard rather than just a run inspector.

This spec adds a **Project Overview** view: a top-level dashboard that
enumerates all specs in `specs/`, parses each spec's Exit Criteria progress
(checked vs total), and lists the runs in the vault. Specs and runs are
shown together so the user can see the whole project — what's specced,
what's done, what's running — in one place.

---

## Requirements

**R1**: `pidag ui` gains a `--project-root PATH` flag (default: current
working directory), analogous to the same flag on `pidag attach` and
`pidag sdd`. The project root tells the UI where `specs/*.md` live. The
flag is optional; when omitted, the UI behaves exactly as today (no specs
enumerated, Project view shows "no specs found").

**R2**: New endpoint `GET /api/project` returns a `ProjectOverview`:
```json
{
  "project_root": "/abs/path",
  "specs": [
    {
      "name": "shell-node-dispatch",
      "title": "Shell Node Dispatch — Handle Empty Models Array",
      "file": "specs/shell-node-dispatch.md",
      "exit_criteria_total": 3,
      "exit_criteria_done": 3,
      "status": "complete"
    }
  ],
  "runs": [ /* Vec<RunMeta> — same as GET /api/runs */ ]
}
```

**R3**: `SpecSummary.status` is derived from the Exit Criteria progress:
- `"complete"` when `exit_criteria_done == exit_criteria_total` and total > 0
- `"in-progress"` when `0 < exit_criteria_done < exit_criteria_total`
- `"pending"` when `exit_criteria_done == 0` and total > 0
- `"no-criteria"` when the spec has no Exit Criteria section (total == 0)

**R4**: New endpoint `GET /api/project/specs/{name}` returns the full spec
content plus its parsed metadata:
```json
{
  "name": "shell-node-dispatch",
  "title": "Shell Node Dispatch — Handle Empty Models Array",
  "file": "specs/shell-node-dispatch.md",
  "content": "<full markdown>",
  "exit_criteria_total": 3,
  "exit_criteria_done": 3
}
```
`name` is the spec file's stem (e.g. `shell-node-dispatch` for
`shell-node-dispatch.md`). Unknown name → 404.

**R5**: The Exit Criteria parser counts checkbox lines in the spec
markdown: lines matching `^\s*[-*]\s*\[(x|X| )\]\s*` are criteria; a
checked box (`[x]` or `[X]`) counts as done. The parser is case-insensitive
on the `x`. Lines inside fenced code blocks (``` ``` ```) are NOT counted
(they may contain `[ ]` in code samples).

**R6**: The frontend gains a new `#/project` route that renders the
Project Overview: a grid of spec cards (title + progress bar + status
badge) and a runs table. The spec cards link to `#/spec/<name>`.

**R7**: New frontend route `#/spec/<name>` renders a single spec: the
parsed title, the Exit Criteria checklist (rendered from the markdown with
checked/unchecked boxes shown), and the raw markdown content in a
`<pre>` block. A back-link returns to `#/project`.

**R8**: The default landing route changes from `#/` (sessions list) to
`#/project` when specs are present. When no specs are found (no
`--project-root` or empty `specs/`), the UI falls back to the sessions list
so the run-detail/timeline features still work standalone.

**R9**: `UiState` carries an `Option<PathBuf>` project-root. When `None`,
`GET /api/project` returns `specs: []` and `project_root: null` (the UI
shows a "no project root configured" hint). This keeps the UI backward-
compatible with the existing `pidag ui` invocation that has no project
context.

**R10**: No changes to `RunMeta`, `NodeRecord`, `Event`, or any bincode
schema. No new redb tables. The Project Overview is a read-only projection
of on-disk spec files + the existing `list_runs()` store method.

---

## Architecture

### Backend

```
pidag ui --project-root . --port 4600
    │
    ├── axum web server (ui.rs)
    │       ├── GET /                        → embedded index.html (SPA shell)
    │       ├── GET /api/health              → {status, runs}
    │       ├── GET /api/runs                → Vec<RunMeta>
    │       ├── GET /api/runs/:id            → {run, nodes}
    │       ├── GET /api/runs/:id/events     → Vec<{seq, event}>
    │       ├── GET /api/runs/:id/status     → {text}
    │       ├── GET /api/runs/:id/timeline   → {groups, items}        [P1 #10]
    │       ├── GET /api/runs/:id/nodes/:nid/artifact → {artifact}   [P1 #10]
    │       ├── GET /api/project             → {project_root, specs, runs}   [NEW]
    │       └── GET /api/project/specs/:name → {name, title, content, ...}    [NEW]
    │
    └── Arc<dyn Store> + Option<PathBuf> (project root)
```

`UiState` gains a `project_root: Option<PathBuf>` field. `serve()` and
`router()` / `router_for_test()` take the new field. The binary's
`ui_subcommand` parses `--project-root` and passes it through.

### Spec parsing

A new pure function in `ui.rs` (no I/O — takes a `&str` markdown, returns
parsed metadata):

```rust
struct ParsedSpec {
    title: String,              // first "# " heading, or file stem if none
    exit_criteria_total: usize, // count of [ ]/[x] checkboxes outside code fences
    exit_criteria_done: usize,  // count of [x] checkboxes outside code fences
}

fn parse_spec(markdown: &str) -> ParsedSpec
```

Code-fence tracking: a simple toggle on lines starting with ``` (three
backticks) — toggle on/off, skip checkbox counting while inside a fence.

### Endpoints

`get_project` handler:
1. If `project_root` is `None`: return `ProjectOverview { project_root: None, specs: vec![], runs: list_runs() }`.
2. Else: list `specs/*.md` under the project root (sorted by file name).
   For each, read the file, `parse_spec()`, build a `SpecSummary`.
3. `runs` comes from `store.list_runs()` (same data as `GET /api/runs`).

`get_spec` handler:
1. Resolve `project_root/specs/<name>.md`. Reject path traversal (`name`
   must match `^[A-Za-z0-9._-]+$` and contain no `..`).
2. 404 if the file doesn't exist.
3. Read + parse, return `SpecDetail`.

### Frontend

New routes (hash-based):
- `#/project` → Project Overview (spec cards grid + runs table)
- `#/spec/<name>` → single spec view (title + checklist + raw markdown)
- `#/` → redirects to `#/project` if specs exist, else sessions list
- `#/run/<id>` → unchanged (run detail + timeline)

The Project Overview is the new default landing. The sessions list moves
to `#/runs` (linked from the Project Overview's runs table heading) so the
two views coexist without removing the existing sessions functionality.

---

## TDD Contract

| # | Test | Description |
|---|---|---|
| T1 | `test_parse_spec_counts_checkboxes` | `parse_spec()` on a markdown string with 3 checked + 2 unchecked boxes (outside code fences) returns `total=5, done=3` |
| T2 | `test_parse_spec_ignores_code_fences` | Checkbox lines inside ``` ``` fences are NOT counted; `total` reflects only checkboxes outside fences |
| T3 | `test_parse_spec_extracts_title` | First `# ` heading becomes `title`; if no heading, `title` defaults to the file stem |
| T4 | `test_project_endpoint_no_root` | `GET /api/project` with `project_root: None` returns `specs: []`, `project_root: null`, and `runs` from the store |
| T5 | `test_project_endpoint_enumerates_specs` | With a temp `project_root` containing 2 spec files (one fully checked, one partial), `GET /api/project` returns 2 specs with correct `status` ("complete", "in-progress") |
| T6 | `test_spec_endpoint_returns_content` | `GET /api/project/specs/<name>` returns the full markdown `content` + parsed `title` + criteria counts |
| T7 | `test_spec_endpoint_rejects_traversal` | `GET /api/project/specs/..%2f..%2fetc%2fpasswd` → 404 (or 400); path traversal is rejected |
| T8 | `test_spec_endpoint_unknown_404` | `GET /api/project/specs/nonexistent` → 404 |

Existing tests (`ui_tests.rs`, `timeline_tests.rs`) must continue to pass.
`router_for_test` gains a `project_root` parameter (or a new
`router_for_test_with_root` helper) so the project endpoints can be tested
without a real project root.

---

## Exit Criteria

- [x] `cargo test -p pidag` passes (all existing + 8 new project-overview tests)
- [x] `cargo clippy -p pidag -- -D warnings` clean
- [x] `pidag ui --project-root .` serves `#/project` listing all specs in
      `crates/pidag/specs/` with correct status badges and progress bars
- [x] Clicking a spec card opens `#/spec/<name>` with the markdown content
      and Exit Criteria checklist
- [x] `pidag ui` (no `--project-root`) falls back to the sessions list
      (backward-compatible)
- [x] No changes to `RunMeta` / `NodeRecord` / `Event` / redb schema
- [x] Path-traversal attempts on `GET /api/project/specs/:name` are rejected

---

## Guardrails

- Must NOT change `RunMeta`, `NodeRecord`, `Event`, or any bincode schema
- Must NOT add new redb tables (this is a read-only projection of files)
- Must NOT break existing `ui_tests.rs` / `timeline_tests.rs` tests
- Must NOT use `unwrap()` / `expect()` in production paths (handlers)
- Spec file reads must be bounded (skip files > 1 MiB to avoid pathological
  inputs)
- Path traversal: `name` must match `^[A-Za-z0-9._-]+$` and resolve inside
  `project_root/specs/` — reject otherwise
- The `--project-root` flag is optional; existing `pidag ui` invocations
  keep working unchanged

---

## Files to Modify

| File | Change |
|---|---|
| `crates/pidag/src/ui.rs` | Add `project_root` to `UiState`. Add `parse_spec()`, `SpecSummary`, `SpecDetail`, `ProjectOverview` structs. Add `GET /api/project` + `GET /api/project/specs/:name` routes + handlers. Update `serve()`, `router()`, `router_for_test()` signatures. |
| `crates/pidag/src/bin/pidag.rs` | Parse `--project-root` in `ui_subcommand`, pass to `serve()`. Update `--help` text. |
| `crates/pidag/src/ui_assets/index.html` | Add `#/project` + `#/spec/<name>` + `#/runs` routes. Project Overview view (spec cards + runs table). Spec detail view. Default landing redirects to `#/project` when specs exist. |
| `crates/pidag/tests/project_overview_tests.rs` | NEW — 8 tests (T1-T8). |

### No changes to

- `crates/pidag/src/store/` (no schema changes)
- `crates/pidag/src/event.rs` (no new events)
- `crates/pidag/src/dag.rs`, `render.rs`, `scheduler/` (unaffected)
- `crates/pidag/Cargo.toml` (no new deps — `walkdir` not needed; `std::fs::read_dir` suffices for a flat `specs/` directory)

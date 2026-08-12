# ui/

Trace UI web server for `pidag` DAG runs.

## Overview

This module implements the `pidag ui` subcommand: a self-contained axum web
server that visualizes DAG runs from the redb vault. The frontend is a
vanilla-JS single-page app embedded in the binary via `include_str!` -- no
Node/Bun toolchain, no external assets.

## Architecture

```
pidag ui --port 4600 --project-root .
    │
    └── axum web server
            ├── GET /                      → embedded index.html (SPA)
            ├── GET /api/health            → {"status":"ok","runs":N}
            ├── GET /api/runs              → Vec<RunMeta>
            ├── GET /api/runs/:id          → {run, nodes}
            ├── GET /api/runs/:id/events   → Vec<{seq, event}>
            ├── GET /api/runs/:id/status   → render_status output
            ├── GET /api/runs/:id/timeline → Gantt timeline data
            ├── GET /api/project           → ProjectOverview
            └── GET /api/project/specs/:name → SpecDetail

pidag ui --workspace /projects --port 4601
    │
    └── workspace mode
            ├── GET /api/workspace                    → {projects: [...]}
            ├── GET /api/workspace/projects/:name     → ProjectOverview
            └── GET /api/workspace/projects/:name/... → scoped routes
```

## Module Structure

| File | Description |
|------|-------------|
| `mod.rs` | Router building, UiState, serve functions |
| `types.rs` | Response types, AppError |
| `handlers.rs` | Core handlers: health, runs, events, timeline |
| `project.rs` | Project Overview: spec enumeration, detail |
| `workspace_handlers.rs` | Workspace: discovery, project-attach |
| `workspace.rs` | Workspace discovery logic, ProjectInfo |
| `render.rs` | DAG rendering: mermaid flowchart, status text |
| `spec_parser.rs` | Spec markdown parsing: title, Exit Criteria |

## Workspace Mode

When started with `--workspace PATH`, the UI scans the directory for projects
and displays them as cards on the landing page (`#/`).

A directory is a project if it contains:
- `specs/*.md` files, OR
- `Cargo.toml`, OR
- `pyproject.toml`

Project cards show:
- Name (directory name)
- Spec count
- Run count
- Last run timestamp
- Status (complete/in-progress/pending)

## Routes

### Single-Project Mode (`--project-root`)

| Route | View |
|-------|------|
| `#/` | Runs list |
| `#/run/:id` | Run detail + timeline |
| `#/project` | Project overview |
| `#/project/spec/:name` | Spec detail |

### Workspace Mode (`--workspace`)

| Route | View |
|-------|------|
| `#/` | Project cards grid |
| `#/project/:name` | Project overview |
| `#/project/:name/spec/:spec` | Spec detail |
| `#/project/:name/run/:id` | Run detail |

## Safety

The server binds to `127.0.0.1` by default (local-only) because the UI has no
authentication. Use `--host 0.0.0.0` to expose on all interfaces only when you
understand the implications.

## Testing

- Integration tests: `tests/ui_tests.rs`
- Unit tests: `spec_parser.rs` (parsing tests)
- `router_for_test()` exposes the axum router for `tower::ServiceExt::oneshot`

## See Also

- [Frontend assets](../ui_assets/index.html) — Embedded SPA
- [Workspace spec](../../specs/03-multi-project-workspace.md)

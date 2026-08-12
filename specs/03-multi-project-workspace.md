# Spec: Multi-Project Workspace

## Overview

**Project**: /projects (pidag container workspace root)
**Phase**: 3 - Workspace Orchestration

Currently, `pidag ui` operates on a **single project** via `--project-root`. The user must run separate UI instances for each project (e.g., port 4602 for pidag, port 4603 for agent-memory). This is cumbersome for a workspace with multiple projects.

This spec introduces **workspace-level orchestration**: a single pidag UI instance at `http://${DEPLOY_HOST_NAME}:4601` that discovers and manages **all projects** in the workspace. Each project appears as a card on the landing page, and the user can "attach" to any project to see its specs and runs.

Additionally, this spec enforces:
1. **Numbered spec ordering** — specs must be prefixed with `01-`, `02-`, etc.
2. **One DAG per spec** — each spec produces its own named DAG (no merging)
3. **Workspace vault** — a `pidag.redb` at workspace root tracks all projects

---

## Requirements

### Functional

**R1 - Workspace Discovery**: `pidag ui --workspace PATH` (new flag) scans `PATH` for subdirectories containing either `specs/*.md` or `Cargo.toml` or `pyproject.toml`. Each discovered subdirectory is a "project".

**R2 - Project Cards**: `GET /api/workspace` returns:
```json
{
  "workspace_root": "/projects",
  "projects": [
    {
      "name": "testing",
      "path": "/projects/testing",
      "spec_count": 3,
      "run_count": 2,
      "last_run": "2026-08-05T08:18:29Z",
      "status": "complete"
    },
    {
      "name": "pidag",
      "path": "/projects/pidag",
      "spec_count": 13,
      "run_count": 0,
      "last_run": null,
      "status": "pending"
    }
  ]
}
```

**R3 - Project Attach**: `GET /api/workspace/projects/{name}` returns the same structure as current `GET /api/project` but for the specified project. The UI "attaches" to a project by fetching this endpoint.

**R4 - Workspace Vault**: A `pidag.redb` file at the workspace root (`/projects/.pidag/pidag.redb`) stores:
- Project registry (discovered projects with metadata)
- Global defaults (default model, concurrency settings)
- Cross-project run history

**R5 - Numbered Spec Enforcement**: The `pidag sdd` command validates that spec filenames follow `NN-<name>.md` pattern (e.g., `01-fibonacci.md`, `02-binary-search.md`). Specs without numeric prefix are rejected with an error message.

**R6 - One DAG Per Spec**: Each spec produces a DAG named after the spec file stem. Running `pidag sdd specs/01-fibonacci.md` creates a DAG named `01-fibonacci`, not merged with other specs. The `--run` flag executes only that single DAG.

**R7 - Landing Page Cards**: `#/` (root route) shows project cards in a grid:
- Project name (directory name)
- Spec count badge
- Run count badge
- Last run timestamp
- Overall status (derived from specs: complete/in-progress/pending)
- Click card → navigates to `#/project/{name}`

**R8 - Per-Project View**: `#/project/{name}` shows the current project overview (spec cards + runs table) but scoped to that project. Back button returns to workspace view.

**R9 - Skill Update**: Update `spec-generator` skill to:
- Auto-detect next numeric prefix by scanning existing specs
- Enforce `NN-` prefix pattern
- Generate spec with correct project path

**R10 - DAG Naming**: The DAG's `metadata.spec` field must match the spec filename stem exactly. Run IDs include the spec name for traceability: `run-YYYYMMDD-HHMMSS-{spec}-{hash}`.

### Non-Functional

**NF1**: Workspace discovery is lazy — projects are scanned on first request, then cached for 60 seconds.

**NF2**: The workspace vault is created automatically on first `pidag ui --workspace` invocation.

**NF3**: Backward compatibility — `pidag ui --project-root` continues to work for single-project mode.

---

## Architecture

```
/projects/                          # Workspace root
├── .pidag/
│   └── pidag.redb                    # Workspace vault (projects registry + defaults)
├── testing/                        # Consolidated test project
│   ├── .pidag/
│   │   └── pidag.redb                # Project vault (runs for this project)
│   ├── specs/
│   │   ├── 01-multiply-function.md
│   │   ├── 02-fibonacci.md
│   │   └── 03-binary-search.md
│   └── src/
│       └── lib.rs                  # All implementations
└── pidag/                          # pidag source (to be modified)
    ├── .pidag/
    │   └── pidag.redb
    ├── specs/
    │   ├── 01-redb-pool-fix.md
    │   ├── 02-container-deployment.md
    │   ├── 03-multi-project-workspace.md  # THIS SPEC
    │   └── ...
    └── src/
```

### Data Flow

```
pidag ui --workspace /projects --port 4601
    │
    ├── Workspace Vault: /projects/.pidag/pidag.redb
    │   ├── projects table (name, path, last_scanned)
    │   └── defaults table (model, concurrency, etc.)
    │
    ├── axum web server
    │   ├── GET /                           → index.html (SPA)
    │   ├── GET /api/workspace              → {workspace_root, projects[]}
    │   ├── GET /api/workspace/projects/:name → ProjectOverview (existing)
    │   ├── GET /api/workspace/projects/:name/specs/:spec → SpecDetail
    │   ├── GET /api/workspace/projects/:name/runs → Vec<RunMeta>
    │   ├── GET /api/workspace/projects/:name/runs/:id → RunDetail
    │   └── ... (existing endpoints scoped to project)
    │
    └── Project Vaults: /projects/{name}/.pidag/pidag.redb
        └── runs, nodes, events (per-project)
```

### Frontend Routes

| Route | View |
|-------|------|
| `#/` | Workspace overview (project cards grid) |
| `#/project/{name}` | Project overview (spec cards + runs) |
| `#/project/{name}/spec/{spec}` | Spec detail |
| `#/project/{name}/run/{id}` | Run detail + timeline |

### Spec Naming Enforcement

```rust
fn validate_spec_name(name: &str) -> Result<(), SpecError> {
    // Must match: NN-<slug>.md where NN is 01-99
    let re = Regex::new(r"^(\d{2})-[a-z0-9-]+\.md$")?;
    if !re.is_match(name) {
        return Err(SpecError::InvalidName(
            "Spec must be named NN-<slug>.md (e.g., 01-my-feature.md)".into()
        ));
    }
    Ok(())
}
```

### DAG Naming

```rust
// In sdd.rs generate_sdd_dag()
let spec_stem = spec_path.file_stem().unwrap().to_string_lossy();
// e.g., "01-fibonacci" from "specs/01-fibonacci.md"

dag.metadata.spec = spec_stem.to_string();
// Run ID: run-20260805-081631-01-fibonacci-ccf73c
```

---

## TDD Contract

| Test Name | Input | Expected Output |
|-----------|-------|-----------------|
| test_workspace_discovery | workspace with 2 project dirs (testing, pidag) | 2 projects in response |
| test_workspace_empty | empty workspace | projects: [] |
| test_workspace_filters_non_projects | dir without specs/Cargo.toml | excluded from projects |
| test_project_attach | GET /api/workspace/projects/testing | ProjectOverview for testing |
| test_project_not_found | GET /api/workspace/projects/nonexistent | 404 |
| test_spec_name_valid | "01-fibonacci.md" | Ok |
| test_spec_name_invalid_no_prefix | "fibonacci.md" | Err(InvalidName) |
| test_spec_name_invalid_bad_prefix | "1-fibonacci.md" | Err(InvalidName) |
| test_dag_name_matches_spec | sdd on 01-fibonacci.md | dag.metadata.spec == "01-fibonacci" |
| test_workspace_vault_created | first ui --workspace | .pidag/pidag.redb exists |

---

## Exit Criteria

- [ ] `cargo test -p pidag` passes (existing + 10 new workspace tests)
- [ ] `cargo clippy -p pidag -- -D warnings` clean
- [ ] `pidag ui --workspace /projects` shows project cards at `#/`
- [ ] Clicking project card navigates to `#/project/{name}` with specs
- [ ] `pidag sdd` rejects specs without `NN-` prefix
- [ ] Each spec run creates a DAG named after the spec stem
- [ ] Workspace vault created at `/projects/.pidag/pidag.redb`
- [ ] `pidag ui --project-root` still works (backward compatible)
- [ ] `spec-generator` skill creates specs with auto-incremented `NN-` prefix

---

## Guardrails

- Do NOT break existing `--project-root` single-project mode
- Do NOT merge multiple specs into a single DAG
- Do NOT allow specs without `NN-` numeric prefix
- Do NOT store run data in workspace vault (runs stay in project vaults)
- Do NOT require manual project registration (auto-discovery)
- Spec validation must fail fast with clear error message
- Project discovery must be bounded (skip dirs with > 100 specs)
- Path traversal protection on all project/spec name parameters

---

## Files to Modify

| File | Change |
|------|--------|
| `src/bin/pidag.rs` | Add `--workspace` flag, workspace mode startup |
| `src/ui.rs` | Add `WorkspaceState`, workspace endpoints, project scoping |
| `src/sdd.rs` | Validate spec naming, DAG naming from spec stem |
| `src/store/mod.rs` | Workspace vault schema (projects table, defaults) |
| `src/store/redb_store.rs` | Workspace-level store operations |
| `src/ui_assets/index.html` | Workspace view, project cards, scoped navigation |
| `tests/workspace_tests.rs` | NEW — 10 workspace tests |
| `deploy/resources/pi-skills/spec-generator/run.sh` | Auto-increment NN prefix |

### New Structs

```rust
// Workspace-level state
struct WorkspaceState {
    workspace_root: PathBuf,
    workspace_vault: Arc<dyn Store>,
    project_cache: RwLock<HashMap<String, ProjectInfo>>,
    cache_expiry: Instant,
}

struct ProjectInfo {
    name: String,
    path: PathBuf,
    spec_count: usize,
    run_count: usize,
    last_run: Option<DateTime<Utc>>,
    status: ProjectStatus,
}

enum ProjectStatus {
    Complete,
    InProgress,
    Pending,
    NoSpecs,
}
```

---

## Migration Path

1. Existing `pidag ui --project-root` continues unchanged
2. New `pidag ui --workspace` enables multi-project mode
3. Projects can be used in either mode (vaults are compatible)
4. Workspace vault is additive (no breaking changes to project vaults)

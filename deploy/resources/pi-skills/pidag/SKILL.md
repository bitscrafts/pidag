---
name: pidag
description: >
  SDD orchestration via pidag. Generate DAGs from specs, execute workflows,
  list runs, and view results. The core skill for Spec-Driven Development.
version: 1.0.0
allowed-tools: [bash]
---

# pidag

Execute pidag SDD orchestration commands.

## Usage

```bash
pi --skill pidag <command> [args...]
```

## Commands

### sdd
Generate or execute an SDD workflow from a spec file.

```bash
# Generate DAG only (dry run)
pi --skill pidag sdd specs/01-feature.md

# Execute the SDD workflow
pi --skill pidag sdd specs/01-feature.md --run

# Override model
pi --skill pidag sdd specs/01-feature.md --run --model deepseek-chat
```

### list
List all pidag runs.

```bash
pi --skill pidag list
```

### show
Show details of a specific run.

```bash
pi --skill pidag show <run-id>
```

### init
Initialize pidag for a project (creates .pidag/ directory).

```bash
pi --skill pidag init
```

## Workflow

1. Create a spec file in `specs/` using `spec-generator` skill
2. Run `pidag sdd specs/<spec>.md` to preview the DAG
3. Run `pidag sdd specs/<spec>.md --run` to execute
4. Check results with `pidag show <run-id>` or in the Web UI

## Output

Returns JSON with command results:
- `sdd`: DAG generation/execution status
- `list`: Array of runs with id, status, timestamp
- `show`: Full run details with node outputs
- `init`: Initialization confirmation

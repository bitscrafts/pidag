# sdd/

Spec-Driven Development DAG generator for the pidag orchestrator.

## Overview

Parses `spec.md` files and generates DAGs for the SDD loop:
3 iterations of implementation → quality-gate → validation.

## Files

| File | Description |
|------|-------------|
| `mod.rs` | `SddGenerator`, `validate_spec_name`, `SpecError` |

---

## SddGenerator

### Methods

```rust
// Generate DAG from file path
SddGenerator::from_spec(
    spec_path: &Path,
    project_root: &Path,
    models: &ModelsConfig,
    sdd: &SddConfig,
) -> Result<Dag, PidagError>

// Generate DAG from string content
SddGenerator::from_spec_content(
    spec_content: &str,
    project_root: &Path,
    models: &ModelsConfig,
    sdd: &SddConfig,
    spec_name: Option<String>,
    spec_file: Option<String>,
) -> Result<Dag, PidagError>
```

### Parameters

- `models: &ModelsConfig` — Model fallback chain for LLM nodes
  - Iterations 1-2: `models.free` only
  - Iteration 3: `models.free` then `models.paid`

- `sdd: &SddConfig` — Script paths for shell nodes
  - `validate_script`: Exit criteria validation
  - `quality_gate_script`: Quality checks (clippy, tests)

---

## Generated DAG Structure

10-node DAG for each spec:

```
validate-baseline ─────────────────────────────────────┐
                                                       │
implement-iter1 ──► quality-gate-1 ──► validate-iter1 ─┤
                                              │        │
                   ┌──────────────────────────┘        │
                   ▼ (gated: validate-iter1:fail)      │
implement-iter2 ──► quality-gate-2 ──► validate-iter2 ─┤
                                              │        │
                   ┌──────────────────────────┘        │
                   ▼ (gated: validate-iter2:fail)      │
implement-iter3 ──► quality-gate-3 ──► validate-iter3 ─┘
```

### Nodes

| Node | Type | Description |
|------|------|-------------|
| `validate-baseline` | shell | Check baseline before iteration 1 |
| `implement-iter1` | LLM | First implementation attempt |
| `quality-gate-1` | shell | Quality checks (clippy, tests) |
| `validate-iter1` | shell | Validate exit criteria |
| `implement-iter2` | LLM | Fix iteration 1 failures (gated) |
| `quality-gate-2` | shell | Quality checks |
| `validate-iter2` | shell | Validate exit criteria |
| `implement-iter3` | LLM | Final iteration (gated) |
| `quality-gate-3` | shell | Quality checks |
| `validate-iter3` | shell | Final validation |

### Gate Fields

- `implement-iter2` has `gate: "validate-iter1:fail"` → skips if iter1 passes
- `implement-iter3` has `gate: "validate-iter2:fail"` → skips if iter2 passes

---

## Spec Naming Validation

```rust
pub fn validate_spec_name(name: &str) -> Result<(), SpecError>
```

Specs must match pattern: `NN-<slug>.md` (e.g., `01-fibonacci.md`)

- `NN` = two-digit number (01-99)
- `<slug>` = lowercase alphanumeric with hyphens
- Must end with `.md`

Invalid examples: `fibonacci.md`, `1-test.md`, `AB-test.md`

---

## Spec Sections Parsed

| Section | Usage |
|---------|-------|
| `## TDD Contract` | Used in iter1 prompt |
| `## Architecture` | Used in iter1 prompt |
| `## Guardrails` | Used in all iteration prompts |
| `## Exit Criteria` | Passed to validation shell script |

---

## Usage

```rust
use pidag::sdd::SddGenerator;
use pidag::core::Config;

let config = Config::load(&PathBuf::from(".pidag/config.toml"))?;
let models = config.resolve_models(None);

let dag = SddGenerator::from_spec(
    &PathBuf::from("specs/01-feature.md"),
    &PathBuf::from("."),
    &models,
    &config.sdd,
)?;

// DAG metadata includes spec name
assert_eq!(dag.metadata.unwrap().get("spec"), Some(&"01-feature".to_string()));
```

---

## Checkpoint and Resume (Spec-08)

When a `pidag sdd spec.md --run` is interrupted (process crash, timeout, Ctrl+C),
the execution can resume from the last successfully completed node instead of
restarting from scratch.

### How It Works

**Run ID**: Deterministic identifier derived from `SHA-256(spec_path + spec_content)`,
truncated to 12 hex characters. Same spec = same run_id. Edited spec = new run_id.

**Checkpoint**: On startup, pidag checks the vault for an existing run with this run_id:
- If run is **completed** (`completed_at` set): Show cached report
- If run is **incomplete** (`completed_at` None): Resume from checkpoint
- If run **doesn't exist**: Start fresh

**State Restoration**:
- Done nodes are skipped; their dependents' in-degree is pre-decremented
- Running nodes (stale from crash) are reset to Pending
- Failed nodes stay terminal unless `--retry-failed` flag is passed

### CLI Flags

```bash
# Resume from an interrupted run (if exists; creates new if not)
pidag sdd spec.md --run --resume --project-root .

# Force a clean start, ignore any existing checkpoint
pidag sdd spec.md --run --fresh --project-root .

# Resume and retry nodes that failed previously
pidag sdd spec.md --run --resume --retry-failed --project-root .
```

### Module Structure

| File | Description |
|------|-------------|
| `resume.rs` | Checkpoint structures and loading logic |
| `run_id_for_spec()` | Generate deterministic run_id from spec |
| `load_checkpoint()` | Load checkpoint from vault, return ResumeDecision |
| `Checkpoint` | Terminal node states (Done/Failed/Blocked/Stale) |
| `ResumeDecision` | Enum: Fresh / AlreadyDone / Resume |

### Example: Resume from Interrupt

```bash
# Run 1: Completes 6/10 nodes, then crashes
$ timeout 30 pidag sdd spec.md --run --project-root .
# ... nodes 0-5 complete, node 6 interrupted ...

# Run 2: Resume from node 6
$ pidag sdd spec.md --run --resume --project-root .
# ... loads checkpoint, skips nodes 0-5, dispatches node 6 onwards ...
```

### Storage

Checkpoint is persisted via the existing redb vault (`.pidag/pidag.redb`).
No additional durability work needed; redb's `Durability::Immediate` fsyncs
all writes atomically.

### Performance

Checkpoint loading completes in < 50ms for a 10-node DAG (no polling, no busy loops).


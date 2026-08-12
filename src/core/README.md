# core/

Fundamental types for the pidag DAG orchestrator.

## Files

| File | Description |
|------|-------------|
| `dag.rs` | DAG structure, Node, ModelRef, validation, topological sort |
| `error.rs` | `PidagError` enum with Store, Parse, Validation variants |
| `event.rs` | Event types (RunStarted, NodeStarted, etc.) and sinks |
| `config.rs` | Configuration parsing from `.pidag/config.toml` |

---

## dag.rs — DAG Structure

### Key Types

```rust
pub struct Dag {
    pub nodes: Vec<Node>,
    pub metadata: Option<HashMap<String, String>>,
}

pub struct Node {
    pub id: String,
    pub prompt: String,
    pub depends_on: Vec<String>,
    pub models: Vec<ModelRef>,
    pub retry: Option<RetryPolicy>,
    pub node_type: Option<String>,  // "shell", "a2a", "mcp_call"
    pub gate: Option<String>,       // "node_id:fail" conditional
    pub mcp_call: Option<McpCall>,
}

pub struct ModelRef {
    pub name: String,
    pub paid: bool,
}
```

### Methods

- `Dag::validate()` — Check for cycles, missing dependencies
- `Dag::topological_order()` — Return nodes in execution order
- `Dag::ready_nodes()` — Nodes with all dependencies satisfied

---

## event.rs — Event System

### Event Types

```rust
pub enum Event {
    RunStarted { run_id: String, dag: Dag, timestamp: String },
    NodeStarted { run_id: String, node_id: String, timestamp: String },
    NodeCompleted { run_id: String, node_id: String, output: String, ... },
    NodeFailed { run_id: String, node_id: String, error: String, ... },
    RunCompleted { run_id: String, success: bool, timestamp: String },
}
```

### Event Sinks

| Sink | Description |
|------|-------------|
| `VecSink` | Collect events in memory (testing) |
| `RedbSink` | Persist to redb vault |
| `JsonlSink` | Append to JSONL file |
| `CompositeSink` | Fan-out to multiple sinks |

---

## config.rs — Configuration Management

Provides configuration loading and defaults for pidag projects.

### Types

- `Config`: Main configuration struct
- `ProjectConfig`: Project-level settings (root path)
- `WorkerConfig`: Worker defaults (model, timeout)
- `SddConfig`: SDD loop settings (max iterations, script paths)
- `ModelsConfig`: Model fallback chains (`free`, `paid`)
- `ENV_DEFAULT_MODEL`: env var name (`PIDAG_DEFAULT_MODEL`)

### Model Resolution Priority

```
1. DAG JSON node.models         (per-node, always wins)
2. CLI --model <model>
3. ENV PIDAG_DEFAULT_MODEL
4. .pidag/config.toml [models]
5. ModelsConfig::default()      (last-resort built-in fallback)
```

`Config::resolve_models(cli_model: Option<&str>)` encodes the chain.

### TOML Format

```toml
[project]
root = "/path/to/project"

[worker]
default_model = "deepseek-chat"
timeout_secs = 120

[sdd]
max_iterations = 3
validate_script = "~/.claude/skills/loop-engineer/scripts/validate-exit-criteria.sh"
quality_gate_script = "~/.claude/skills/rust-specialist/scripts/quality-gate.sh"

[models]
free = ["deepseek-chat"]
paid = ["claude-sonnet-4"]
```

### Per-iteration Model Selection

`ModelsConfig::models_for_iter(iteration)` returns the `ModelRef` list:
- Iterations 1-2: free models only
- Iteration 3+: free then paid (paid entries marked `paid: true`)

### Single Source of Truth

- Model strings: `ModelsConfig::default()` in `config.rs`
- Script paths: `SddConfig::default()` in `config.rs`

---

## error.rs — Error Types

```rust
pub enum PidagError {
    Store(String),      // Database errors
    Parse(String),      // JSON/TOML parsing
    Validation(String), // DAG validation failures
}
```

---

## Usage

```rust
use pidag::core::{Dag, Event, Config, PidagError};

// Load DAG from JSON
let dag: Dag = serde_json::from_str(&json)?;
dag.validate()?;

// Load config
let config = Config::load(&PathBuf::from(".pidag/config.toml"))?;
let models = config.resolve_models(cli_model.as_deref());
```

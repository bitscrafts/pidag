# pidag/src/

Source code for the pidag DAG orchestrator.

## Overview

pidag is a deterministic, resilient multi-node LLM DAG executor. It schedules
task graphs with offline scheduling, retry/fallback chains, and event sourcing.
All dispatch goes through a `Worker` trait, making the test suite deterministic.

## Module Structure

```
src/
├── lib.rs              # Public API exports
├── bin/                # Binary entry points
│   ├── pidag.rs        # Main CLI (93 lines)
│   └── crash_writer.rs # Test helper for crash recovery
│
├── core/               # Fundamental types
│   ├── dag.rs          # DAG, Node, ModelRef, validation
│   ├── error.rs        # PidagError enum
│   ├── event.rs        # Event types and sinks
│   └── config.rs       # Configuration parsing
│
├── cli/                # CLI subcommand implementations
│   ├── run.rs          # pidag run
│   ├── show.rs         # pidag show
│   ├── list.rs         # pidag list
│   ├── attach.rs       # pidag attach
│   ├── sdd.rs          # pidag sdd
│   ├── serve.rs        # pidag serve
│   ├── mcp.rs          # pidag mcp
│   ├── ui.rs           # pidag ui
│   └── describe.rs     # pidag describe
│
├── scheduler/          # DAG execution engine
│   ├── mod.rs          # Scheduler struct, state management
│   ├── execute.rs      # Execution loop
│   └── await_loop.rs   # Completion polling
│
├── worker/             # Worker implementations
│   ├── mod.rs          # Worker trait, re-exports
│   ├── pi_print.rs     # PiPrintWorker (subprocess)
│   ├── shell.rs        # ShellWorker (shell commands)
│   └── type_dispatch.rs # TypeDispatchWorker (routing)
│
├── store/              # Persistence layer
│   ├── mod.rs          # Store trait, MockStore
│   ├── redb_store.rs   # RedbStore (persistent lock)
│   └── redb_pool.rs    # RedbStorePool (per-operation lock)
│
├── sdd/                # Spec-Driven Development
│   └── mod.rs          # SddGenerator, validate_spec_name
│
├── ui/                 # Trace UI web server
│   ├── mod.rs          # Router, UiState, serve functions
│   ├── handlers.rs     # Core handlers (health, runs, events)
│   ├── project.rs      # Project Overview handlers
│   ├── workspace_handlers.rs # Workspace discovery handlers
│   ├── workspace.rs    # Workspace discovery logic
│   ├── render.rs       # DAG rendering (mermaid, status)
│   ├── types.rs        # Response types, AppError
│   └── spec_parser.rs  # Spec markdown parsing
│
├── ui_assets/          # Embedded frontend
│   └── index.html      # SPA (HTML + CSS + JS)
│
├── mcp/                # Model Context Protocol
│   ├── server.rs       # MCP stdio server
│   ├── client.rs       # MCP client for external servers
│   └── call.rs         # McpCallWorker for mcp_call nodes
│
├── a2a/                # Agent-to-Agent protocol
│   ├── server.rs       # A2A HTTP server
│   └── worker.rs       # A2aWorker for remote agents
│
└── rpc/                # JSON-RPC 2.0 server
    ├── server.rs       # RpcServer implementation
    └── handlers.rs     # Shared handler functions
```

---

## Public API

### Main Entry Point

```rust
pub struct Scheduler {
    // DAG, worker, event sink, concurrency limit
}

impl Scheduler {
    pub fn new(dag, worker, event_sink, concurrency) -> Self;
    pub async fn run(&mut self, allow_paid: bool) -> Result<RunReport, PidagError>;
    pub async fn await_dag(&mut self, timeout) -> AwaitOutcome;
    pub async fn resume_await(&mut self, token, timeout) -> AwaitOutcome;
    pub async fn wait_any(&mut self, timeout) -> AwaitOutcome;
}
```

### Data Structures

```rust
pub struct Dag { pub nodes: Vec<Node>, pub metadata: Option<HashMap<String, String>> }
pub struct Node { pub id, pub prompt, pub depends_on, pub models, pub retry, pub node_type, pub gate }
pub struct ModelRef { pub name: String, pub paid: bool }
pub struct RunReport { pub node_states: Vec<NodeState>, pub failed: Vec<String> }
```

### Worker Trait

```rust
#[async_trait]
pub trait Worker: Send + Sync {
    async fn run(&self, node_id: &str, model: &str, attempt: usize) -> Result<WorkerOutput, PidagError>;
}
```

### Event Sink Trait

```rust
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&mut self, event: Event) -> Result<(), std::io::Error>;
}
```

### Store Trait

```rust
#[async_trait]
pub trait Store: Send + Sync {
    async fn put_run(&self, run: &RunMeta) -> Result<(), PidagError>;
    async fn get_run(&self, run_id: &str) -> Result<Option<RunMeta>, PidagError>;
    async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError>;
    async fn append_event(&self, run_id: &str, event: &Event) -> Result<u64, PidagError>;
    async fn get_events(&self, run_id: &str) -> Result<Vec<(u64, Event)>, PidagError>;
}
```

---

## Module Documentation

| Module | README |
|--------|--------|
| bin/ | [bin/README.md](bin/README.md) |
| core/ | [core/README.md](core/README.md) |
| cli/ | [cli/README.md](cli/README.md) |
| scheduler/ | [scheduler/README.md](scheduler/README.md) |
| worker/ | [worker/README.md](worker/README.md) |
| store/ | [store/README.md](store/README.md) |
| sdd/ | [sdd/README.md](sdd/README.md) |
| ui/ | [ui/README.md](ui/README.md) |
| ui_assets/ | [ui_assets/README.md](ui_assets/README.md) |
| mcp/ | [mcp/README.md](mcp/README.md) |
| a2a/ | [a2a/README.md](a2a/README.md) |
| rpc/ | [rpc/README.md](rpc/README.md) |

---

## Behavioral Guarantees

### DAG Validation

1. All `depends_on` references must exist
2. No cycles allowed
3. Validation fails before any dispatch

### Scheduling

1. Topological ordering enforced
2. Bounded concurrency via semaphore
3. Retry lattice: `attempts × models`
4. Paid-gate: skips paid models unless allowed
5. Gate conditions: `node_id:fail` skips if predecessor passed
6. Dependency blocking: failed nodes block dependents

### Event Log

- First: `DagSubmitted`
- Last: `DagDone { successful_nodes, failed_nodes }`
- Per node: `NodeDispatched`, `NodeDone`/`NodeFailed`/`NodeBlocked`

---

## Usage Examples

### Run DAG to Completion

```rust
use pidag::{Dag, Scheduler, VecSink, DelayMockWorker};

let dag: Dag = serde_json::from_str(json)?;
dag.validate()?;

let mut scheduler = Scheduler::new(
    dag,
    Box::new(DelayMockWorker::new()),
    Box::new(VecSink::new()),
    4,
);
let report = scheduler.run(false).await?;
```

### Generate SDD DAG from Spec

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
```

### Start Trace UI

```rust
use pidag::ui;

ui::serve(4600, "127.0.0.1", vault_path, None).await?;
```

---

## Non-Functional Characteristics

- **No network I/O**: dispatch via `Worker` trait
- **No filesystem I/O** except explicit sinks/stores
- **Process spawning opt-in**: `PiPrintWorker` only
- **Deterministic scheduling**: reproducible event order
- **No panic in production paths**: all `Result`-based
- **No busy-wait**: watch channels, not polling
- **450-line file limit**: modules split when larger

---

## See Also

- [README.md](../README.md) — Crate-level documentation
- [HANDOFF.md](../HANDOFF.md) — Session continuity
- [specs/](../specs/) — SDD specifications

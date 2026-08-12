# worker — Worker Trait and Implementations

The `worker` module is the home of the [`Worker`] trait — the per-node
execution abstraction the scheduler calls — plus every concrete worker used
by [`TypeDispatchWorker`].

## Module Structure

```
src/worker/
├── mod.rs           — Worker trait, WorkerOutput, classify_retryable, DelayMockWorker, pub re-exports
├── pi_print.rs      — PiPrintWorker, parse_pi_envelope
├── shell.rs         — ShellWorker, RealShellWorker
├── a2a.rs           — A2aWorker, is_a2a_endpoint, split_endpoint_and_skill, extract_text_part
└── type_dispatch.rs — TypeDispatchWorker
```

## Workers

| Worker | File | Purpose |
|--------|------|---------|
| `PiPrintWorker` | `pi_print.rs` | Shells out to the local `pi` CLI (`pi -p --output-format json`) |
| `RealShellWorker` | `shell.rs` | Runs `bash -c` for `node_type == "shell"` nodes |
| `A2aWorker` | `a2a.rs` | Dispatches to A2A-compliant remote agents via `tasks/send` JSON-RPC over `curl` |
| `DelayMockWorker` | `mod.rs` | Deterministic test double (sleeps a scripted duration) |
| `TypeDispatchWorker` | `type_dispatch.rs` | Routes nodes to the correct worker by `node_type` + URL prefix |

## Routing Logic

`TypeDispatchWorker::run` routes each node as follows:

```
node_type == "shell"                     → RealShellWorker
node_type == "llm" / None / unknown:
    ModelRef.name starts with http://    → A2aWorker (A2A protocol)
    ModelRef.name is anything else       → PiPrintWorker (local pi CLI)
```

This makes pidag a **multi-agent orchestrator**: a single DAG can mix local
`pi` CLI nodes, `bash` shell nodes, and A2A remote-agent nodes, routed purely
by the `ModelRef.name` URL prefix — no config changes required
(backward-compatible with existing DAG JSON).

## Key Types

- **`Worker` trait** (`mod.rs`): `async fn run(&self, node_id, model, attempt) -> Result<WorkerOutput, PidagError>`
- **`WorkerOutput`** (`mod.rs`): `{ success: bool, output: String, retryable: bool }`
- **`classify_retryable`** (`mod.rs`, `pub(crate)`): Classifies HTTP 429/503/quota failures as retryable

## A2A Protocol

`A2aWorker` implements the A2A `tasks/send` JSON-RPC protocol:

1. POST a `tasks/send` body to `<endpoint>/v1/tasks/send`
2. If the response state is `"working"`, poll `tasks/get` with `result.id` every `poll_interval`
3. On `"completed"`, extract the first text `Part` from `result.artifacts[0].parts[0].text`
4. On `"failed"` or timeout, return a failure `WorkerOutput`

The URL fragment (`#skill`) optionally selects an A2A skill via the `skillId`
JSON-RPC param. See `specs/97-a2a-worker.md` for the full spec.

## Design Constraints

- No new Rust dependencies (`curl` is a system binary, `serde_json` is already a dep)
- `A2aWorker` is `Send + Sync` (required by the `Worker` trait)
- `is_a2a_endpoint` is case-sensitive on the `http://`/`https://` prefix
- `classify_retryable` is `pub(crate)` — shared between `pi_print.rs` and `a2a.rs`
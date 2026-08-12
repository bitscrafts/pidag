# Shell Node Dispatch — Handle Empty Models Array

- **Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
- **Crate**: `pidag`

---

## Overview

The `dispatch_node` function in `scheduler/execute.rs` iterates over `node.models` to
dispatch work. Shell nodes have an empty `models` array (they don't use LLM models),
which causes the loop to never execute and the worker to never be called. The node
defaults to "Failed" state without ever running.

This spec fixes the dispatch logic to handle shell nodes with empty models.

---

## Requirements

**R1**: When `node.models` is empty AND `node.node_type == Some("shell")`:
- Execute the worker once with an empty model string
- Use `retry.attempts` as the max attempts (default to 1 if not specified)

**R2**: When `node.models` is empty AND `node.node_type != Some("shell")`:
- Fail immediately with error "no models specified for LLM node"

**R3**: Existing behavior for non-empty models arrays must be preserved.

**R4**: Shell node success is determined by `output.success` (exit code 0).

---

## Architecture

The fix is in `scheduler/execute.rs::dispatch_node`:

```rust
// Current (broken):
for model in &node.models {
    // Never executes for shell nodes with empty models
}

// Fixed:
if node.models.is_empty() {
    if node.node_type.as_deref() == Some("shell") {
        // Execute worker with empty model
        for attempt in 1..=node.retry.attempts.max(1) {
            if let Ok(output) = worker.run(&node.id, "", attempt).await {
                if output.success {
                    // Success path
                }
            }
        }
    } else {
        // Fail: LLM node needs models
    }
} else {
    // Existing model iteration logic
}
```

---

## TDD Contract

| # | Test name | Given | Expected |
|---|-----------|-------|----------|
| T1 | `test_shell_node_empty_models_executes` | Shell node with `models: []` | Worker is called, node can succeed |
| T2 | `test_llm_node_empty_models_fails` | LLM node with `models: []` | Node fails with "no models" error |
| T3 | `test_shell_node_retry_works` | Shell node, `retry: {attempts: 2}`, first fails | Retries and succeeds on second |

---

## Exit Criteria

- [x] `pidag run test-dag.json` with shell nodes completes successfully
- [x] `cargo test -p pidag 2>&1 | grep "shell_node_empty_models" | grep -q "ok"`
- [x] `cargo clippy -p pidag -- -D warnings 2>&1 | grep -qv "error"`

---

## Guardrails

- Must NOT change the Worker trait
- Must NOT break existing behavior for nodes with models
- Must NOT use `unwrap()` or `expect()` in production code
- Rust edition should be 2024 (latest stable)

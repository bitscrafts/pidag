# bin/

Binary entry points for the pidag crate.

## Overview

This directory contains the executable entry points for pidag. The main CLI
binary delegates all subcommand logic to the `pidag::cli` module, keeping the
entry point minimal.

## Files

| File | Lines | Description |
|------|-------|-------------|
| `pidag.rs` | ~93 | Main CLI entry point |
| `crash_writer.rs` | ~91 | Test helper for crash recovery |

---

## pidag.rs — Main CLI Entry Point

Minimal async entry point that dispatches to `pidag::cli` subcommand handlers.

### Subcommands

| Subcommand | Handler | Description |
|------------|---------|-------------|
| `run` | `cli::run` | Execute a DAG from JSON file |
| `show` | `cli::show` | Display results of a completed run |
| `list` | `cli::list` | List all stored runs |
| `attach` | `cli::attach` | Initialize pidag in a project |
| `sdd` | `cli::sdd` | Generate SDD loop DAG from spec |
| `serve` | `cli::serve` | Start JSON-RPC 2.0 stdio server |
| `mcp` | `cli::mcp` | Start MCP stdio server |
| `ui` | `cli::ui` | Start trace UI web server |
| `describe` | `cli::describe` | Render DAG as markdown with mermaid |

### Design

The entry point follows the "thin main" pattern:
- Parse `std::env::args()` manually (no clap dependency)
- Match first argument to subcommand
- Delegate to `pidag::cli` module functions
- All error handling via `Result<(), Box<dyn Error>>`

---

## crash_writer.rs — Crash Recovery Test Helper

Test aid for validating `RedbStore` durability under abrupt termination.

### Purpose

This binary is NOT user-facing. It exists solely to support
`tests/crash_recovery_tests.rs` which validates that:
- Events committed to redb survive process crashes
- The vault remains usable after abrupt termination
- No data corruption occurs on partial writes

### How It Works

1. Opens a redb vault at the path provided via `argv[1]`
2. Writes a `RunMeta` record
3. Appends 3 events, each in its own fsync'd transaction
4. After each commit, prints `COMMITTED <n>` to stdout
5. Parks forever via `std::future::pending()`

The parent test process:
1. Spawns `crash_writer`
2. Waits for `COMMITTED 1` line
3. Sends `SIGKILL` to simulate crash
4. Reopens the vault and verifies >= 1 event survived

### Test Reference

```rust
// tests/crash_recovery_tests.rs
let crash_writer = env!("CARGO_BIN_EXE_crash_writer");
```

---

## See Also

- [cli/README.md](../cli/README.md) — Subcommand implementations
- [store/README.md](../store/README.md) — RedbStore persistence
- [tests/crash_recovery_tests.rs](../../tests/crash_recovery_tests.rs) — Durability tests

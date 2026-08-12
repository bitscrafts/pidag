# Store Module — persistent vault for pidag runs

The `store` module provides **durable, ACID persistence** for pidag DAG execution. It implements
a trait-based abstraction over multiple backends (RedbStore for production, MockStore for testing)
and defines the data model for runs, nodes, events, and artifacts.

## Overview

All run state is persisted to a local embedded database (redb-backed `.pidag/pidag.redb` file by default).
This enables:

- **Crash-durable resume**: Retrieve the terminal set from disk to resume a run after process death
- **Complete observability**: Event log is the source of truth; all projections (runs, nodes, artifacts) fold from events
- **Post-mortem analysis**: Read the vault after run completion without access to the live scheduler

## Architecture

```
Store (trait)
├── RedbStore (production: redb-backed)
│   ├── runs table: run_id → RunMeta
│   ├── nodes table: "run_id\x00node_id" → NodeRecord
│   ├── events table: "run_id\x00seq(u64)" → Event (source of truth)
│   ├── artifacts table: "run_id\x00node_id" → output text
│   ├── node_timings table: "run_id\x00node_id" → NodeTiming (start/end)
│   └── event_seq table: run_id → next-seq u64 (O(1) append)
└── MockStore (testing: in-memory)
    └── HashMap-backed storage
```

## Key Types

### `RunMeta`

Metadata for a run:
- `run_id`: unique identifier
- `dag_json`: full DAG definition
- `started_at`, `completed_at`: timestamps
- `successful_nodes`, `failed_nodes`: terminal counters

### `NodeRecord`

State and metadata for a node within a run:
- `node_id`, `state` (Pending/Running/Done/Failed)
- `model`: which model was used
- `attempt`: retry count
- `timestamp`: when this state was reached

### `NodeTiming`

Start/end timing for a single node within a run, used by the trace UI's
Gantt timeline view (P1 #10):
- `started_at`: RFC3339 timestamp, set on `NodeDispatched`
- `ended_at`: `Option<RFC3339>`, set on `NodeDone` / `NodeFailed`; `None`
  while the node is running

Stored in a dedicated `node_timings` table rather than folded into
`NodeRecord`, because `NodeRecord.timestamp` is overwritten on every state
change and would lose the dispatch time — which the timeline needs as the
bar's start. The `RedbSink` projection writes/updates `NodeTiming` on the
relevant events (`NodeDispatched`, `NodeDone`, `NodeFailed`, `NodeRetry`).

### `Event`

Events are the durable, ordered log of all state transitions during a run.
Projections (runs, nodes tables) fold from this event stream. No transition is ever dropped.

### `RedbStore`

Single-writer embedded persistence backed by redb. Fully async, supports crash-durable
resume via `terminal_set()`, and enables post-mortem reads via `load_events()`.

`RedbStore::open()` takes an **exclusive file lock** for the lifetime of the
process. This is fine for `pidag run` (or `pidag show`) in isolation, but
blocks the trace UI from reading the vault while a run is in flight.

### `RedbStorePool`

A `Store` wrapper around `RedbStore` that opens the vault **per-operation**,
releasing the exclusive lock between calls. Each method pays an `open` cost
(file open + flock + table checks — milliseconds) but holds the lock only
for the duration of a single read/write. This lets two long-lived processes
share the same vault without one blocking the other:

- The trace UI (`pidag ui`) uses `RedbStorePool` so it can keep serving
  polls while an SDD run is writing to the same vault.
- The SDD run (`pidag run`) uses `RedbStorePool` so its writes are
  short-lived locks, not a process-lifetime lock that would freeze the UI.

Trade-off: per-call `open` overhead. For the UI's low request rate (a
handful of GETs per poll) and the SDD run's event rate (a few events per
node), this is negligible. For a high-throughput writer, use `RedbStore`
directly (persistent lock).

## Usage

**Production (concurrent UI + run)**: `RedbStorePool::new(vault_path)` →
share the vault between the UI and an SDD run without blocking.

**Production (single process)**: `RedbStore::open(".pidag/pidag.redb")` →
attach to scheduler via `RedbSink` (persistent lock, lowest overhead).

**Testing**: `MockStore::new()` → in-memory vault for unit tests

## Integration

The `RedbSink` event sink (in `event.rs`) projects events into the vault via the `Store` trait.
The scheduler emits events; sinks project them to storage. This keeps the scheduler persistence-agnostic.

## Module Structure

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~335 | Store trait, MockStore implementation, re-exports |
| `redb_store.rs` | ~517 | RedbStore implementation (persistent lock) |
| `redb_pool.rs` | ~130 | RedbStorePool wrapper (per-operation lock) |

## Safety & Compliance

- No panics: all errors map to `PidagError::Store`
- No blocking: all operations are async-safe
- Concurrency: `RedbStore` holds a process-lifetime exclusive lock;
  `RedbStorePool` opens per-operation so two long-lived processes (UI + run)
  can share the vault without blocking each other
- ACID transactions: each operation is a single atomic transaction

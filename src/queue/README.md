# pidag queue — carousel priority queue with round-robin scheduling

The `queue` subsystem implements the pidag *carousel priority queue*: it scans a
project's `specs/` directory, orders specs by `NN-` prefix priority, tracks each
spec's execution state (`Pending` / `Running` / `Done` / `Failed` / `Skipped`) in
`.pidag/queue.json`, and drives execution so specs are picked up by priority and
only when they are `Pending`.

For multiple projects under a `workspace` root it uses a **round-robin carousel**
so work is fairly distributed across projects (`A/01, B/01, A/02, B/02, ...`)
rather than draining one project before the next.

## Module layout

```
src/queue/
├── mod.rs        # SpecState, QueueEntry, ProjectQueue + re-exports
├── discover.rs   # Lazy filesystem scan + NN-prefix priority parsing
├── state.rs      # Atomic JSON read/write for .pidag/queue.json
├── execute.rs    # Single-project run + round-robin carousel (interleave / bounded)
├── daemon.rs     # --daemon bounded-batch driver (cron-safe)
└── README.md     # this file
```

## Data structures

- `SpecState` — `Pending`, `Running`, `Done`, `Failed`, `Skipped` (JSON lowercased).
- `QueueEntry` — one spec row: name, path, state, NN priority, timestamps, run id, error.
- `ProjectQueue` — a project's full queue persisted to `.pidag/queue.json`.

## Key operations (all lazy)

| Function | Purpose |
|---|---|
| `discover_specs(root)` | Scan for numbered `NN-*.md` specs, sorted by priority |
| `extract_priority("42-foo")` | Parse NN prefix (`Some(42)`) or `None` if unnumbered |
| `merge_queues(cached, discovered, root)` | Preserve `Done`/known state, add new specs as `Pending` |
| `write_project_queue` / `read_project_queue` | Atomic temp+rename JSON persistence |
| `reset_all_to_pending` / `retry_failed_only` | State transitions for `--reset` / `--retry-failed` |
| `carousel_interleave` / `carousel_bounded` | Round-robin order (unbounded / bounded batch) |
| `round_robin_order(labeled)` | Round-robin with project labels |
| `run_queue` / `execute_entry` | Subprocess to `pidag sdd <spec> --run` |

## CLI

```bash
pidag queue --status [--project-root PATH]
pidag queue --run [--project-root PATH]
pidag queue --reset [--project-root PATH]
pidag queue --retry-failed [--project-root PATH]
pidag queue --workspace <root> --run           # carousel across projects
pidag queue --daemon [--workspace | --project-root] [--batch N]
```

Specs are always re-discovered from the filesystem on each invocation; the state
file only records execution status (crash recovery, idempotent re-runs skip
`Done` specs).

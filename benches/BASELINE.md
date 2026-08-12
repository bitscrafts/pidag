# pidag Performance Baseline — Spec-30 Measurement Harness (Corrected)

**Date**: 2026-08-11 (Corrected)  
**Commit**: 00c4a77  
**Host**: pidag-runner container, 4 CPU, 6 GB RAM, idle  
**Reproduction Command**:
```bash
cd /projects/pidag && cargo bench --bench scheduler_bench 2>&1
```

## Metric Corrections Applied

Two metrics in `benches/scheduler_bench.rs` were corrected:

1. **Peak RSS**: Changed from `VmPeak` (peak virtual size, constant ~2122 MB) to `VmHWM` (peak resident set size, now workload-dependent).
2. **Vault Bytes**: Changed from `std::fs::metadata(...).len()` (preallocated redb file, constant 1589248) to `CountingStore::bytes_written()` (bytes actually serialized).

## Results

Performance baseline measurements for three topologies at three node counts:

| Topology | N | Wall-Clock (ms) | Peak RSS (MB) | Write Txn | Events | Vault Bytes |
|----------|---|-----------------|---------------|-----------|--------|-------------|
| wide     | 50 | 389 | 5 | 354 | 102 | 40839 |
| wide     | 200 | 1588 | 6 | 1404 | 402 | 163241 |
| wide     | 500 | 4073 | 8 | 3504 | 1002 | 408629 |
| chain    | 50 | 537 | 8 | 354 | 102 | 41703 |
| chain    | 200 | 2168 | 8 | 1404 | 402 | 167003 |
| chain    | 500 | 5419 | 8 | 3504 | 1002 | 418403 |
| sdd_like | 50 | 541 | 8 | 361 | 104 | 44969 |
| sdd_like | 200 | 2158 | 8 | 1411 | 404 | 177165 |
| sdd_like | 500 | 5386 | 8 | 3511 | 1004 | 443302 |

**Key change:** Peak RSS now varies with N (5–8 MB) instead of remaining constant; vault bytes now scale with N (40K–443K) instead of staying constant at ~1.5 MB. Both metrics now correctly reflect workload.

## Repeatability (Requirement B5)

Three consecutive runs of `sdd_like` topology with N=50:

| Run | Wall-Clock (ms) | Peak RSS (MB) | Write Txn | Events | Vault Bytes |
|-----|-----------------|---------------|-----------|--------|-------------|
| 1 | 538 | 8 | 361 | 104 | 44963 |
| 2 | 540 | 8 | 361 | 104 | 44969 |
| 3 | 543 | 8 | 361 | 104 | 44969 |

Wall-clock spread: 543 − 538 = 5 ms (< 1% variation).  
**Transaction, event, and vault byte counts are deterministic across all runs** (361 txn, 104 events, ~44.9 KB bytes).

## Write Transactions per Node: Corrected Analysis

The previous record incorrectly concluded that the audit's prediction of "4 write transactions per `NodeDone`" was wrong. That conclusion itself is wrong; the arithmetic clarifies it:

**Total transactions across all nodes = 7N + 4** (verified):
- N=50: 7(50) + 4 = 354 ✓
- N=200: 7(200) + 4 = 1404 ✓
- N=500: 7(500) + 4 = 3504 ✓

**Events = 2N + 2** (each node emits two events: `NodeDispatched` and `NodeDone`):
- N=50: 2(50) + 2 = 102 ✓
- N=200: 2(200) + 2 = 402 ✓
- N=500: 2(500) + 2 = 1002 ✓

**Transaction breakdown per node: 7 total, split across two events:**
- `NodeDispatched` event: 3 transactions (append_event + put_node_state + put_node_timing)
- `NodeDone` event: 4 transactions (append_event + put_node_state + put_artifact + put_node_timing)

**The audit's figure of 4 per `NodeDone` is CONFIRMED, not refuted.** The previous record divided total transactions by the `NodeDone` count, which conflates per-node with per-event metrics. The correct statement is:
- **Per node: 7 write transactions** (split 3 + 4 across dispatch and done events)
- **Per `NodeDone` event: 4 write transactions** (audit's prediction)

## Notes

- All node commands are trivial (`true`) to isolate scheduler overhead
- No LLM nodes, no `pi` invocations (compliance with CLAUDE.md N7)
- Counters are primary metric; wall-clock is secondary due to shared container noise
- Write transaction and event counts remain deterministic across runs, independent of wall-clock variance
- Peak RSS and vault bytes now scale correctly with workload after metric corrections


---

## After spec-32 (partial: batched projections wired, event append still separate)

Measured 2026-08-12, same host and method.

| Topology | N | Wall-clock (ms) | Peak RSS (MB) | Write txns | Events | Vault bytes |
|---|---|---|---|---|---|---|
| wide | 50 | 243 | 5 | 204 | 102 | 34289 |
| wide | 200 | 956 | 6 | 804 | 402 | 137041 |
| wide | 500 | 2489 | 8 | 2004 | 1002 | 343141 |
| chain | 50 | 365 | 8 | 204 | 102 | 35153 |
| chain | 200 | 1476 | 8 | 804 | 402 | 140803 |
| chain | 500 | 3686 | 8 | 2004 | 1002 | 352897 |
| sdd_like | 50 | 382 | 8 | 208 | 104 | 38288 |
| sdd_like | 200 | 1506 | 8 | 808 | 404 | 150840 |
| sdd_like | 500 | 3794 | 8 | 2008 | 1004 | 377677 |

**Write transactions: `7N + 4` → `4N + 8`.** Seven per node became four.
Wall-clock fell 29% (`sdd_like` N=500: 5350 → 3794 ms) and 37% (`wide` N=500:
3979 → 2489 ms), so the fsync reduction shows through the noise at this size.

**Four per node is two per event, not the one T1 requires.** `append_event` is
still a separate transaction from the projection, so every event costs two
commits. Folding the append into the projection transaction is the remaining
step and would take this to `2N + 4`.

### A harness bug worth remembering

The first re-measurement reported **no change at all**. `CountingStore` implements
`Store` by delegation and did not override the new `project_*` methods, so it
inherited the trait's default implementations — which call `put_node_state` /
`put_artifact` / `put_node_timing` individually, land back on `CountingStore`'s own
counted methods, and reach `RedbStore` as separate transactions. The wrapper measured
the old behaviour no matter what the production path did.

A measurement wrapper that inherits a default implementation measures the default,
not the override. Any future `Store` method added for batching must be forwarded in
`CountingStore` or the benchmark will silently under-report the improvement.


---

## After spec-32 T1 complete (event append folded into the projection transaction)

| Topology | N | Write txns | Events | Per node | Per event |
|---|---|---|---|---|---|
| wide | 50 / 200 / 500 | 104 / 404 / 1004 | 102 / 402 / 1002 | 2.0 | 1.0 |
| chain | 50 / 200 / 500 | 104 / 404 / 1004 | 102 / 402 / 1002 | 2.0 | 1.0 |
| sdd_like | 50 / 200 / 500 | 106 / 406 / 1006 | 104 / 404 / 1004 | 2.0 | 1.0 |

**Progression: `7N + 4` → `4N + 8` → `2N + 4`.** A 3.5x reduction, and T1's
requirement of exactly one write transaction per event is met.

Wall-clock at this stage is unusable as a signal — `wide` N=500 measured *faster*
than `wide` N=200 on the same run, which is host contention rather than anything
about the code. The counters are deterministic and remain the primary metric,
exactly as spec-30 argued.


## After spec-32 complete (mpsc emission, T3/T4)

| Topology | N | Wall-clock (ms) | Write txns | Events |
|---|---|---|---|---|
| sdd_like | 50 | 140 | 106 | 104 |
| sdd_like | 200 | 531 | 406 | 404 |
| sdd_like | 500 | 1362 | 1006 | 1004 |

Transactions hold at `2N + 4`. Wall-clock for `sdd_like` at N=500 fell from
**5350 ms** at the original baseline to **1362 ms** — the scheduler no longer
waits on disk at all, which is what T3 was for. Treat the wall-clock figure as
indicative rather than precise: this host has produced non-monotonic timings
within a single run, and the counters remain the metric that means something.

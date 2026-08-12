# Runtime 429 Auto-Failover — Detect Exhausted Models and Advance

- **Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
- **Crate**: `pidag`
- **Priority**: CRITICAL (user-requested; completes the configurable-models work)

---

## Overview

The [configurable-models](./configurable-models.md) spec gave pidag the
**configuration plumbing** for model chains: `node.models: Vec<ModelRef>` and
`ModelsConfig { free, paid }` define ordered fallback chains. But the runtime
**never inspects worker output for rate-limit signals** and fails over to the
next model inside `node.models` only after exhausting every retry attempt —
meaning a 429 from `nvidia/z-ai/glm-5.2` still gets retried `retry.attempts`
times before reaching `nvidia/meta/llama-3.3-70b-instruct`, wasting the
caller's time on a model that is provably throttled.

This spec makes pidag **detect transient provider exhaustion (HTTP 429 / 503 /
connection resets)** in `WorkerOutput` and **immediately advance to the next
`ModelRef`** in the node's chain, skipping remaining retries for the failed
model. Shell nodes are unaffected (success/failure is the only signal).

**User request** (from HANDOFF.md "Pending Issues"):

> "pidag must know when a model is exhausted (429) and switch to the next."

**Out of scope** (per the configurable-models Guardrails): this spec is
runtime behavior ONLY. It does not change config loading, the priority chain,
or the `ModelsConfig` struct.

---

## Requirements

**R1**: `WorkerOutput` carries an error signal so the scheduler can tell
"transient/retryable provider error (advance model)" apart from "real
work failure (retry same model)". Specifically: worker implementations mark
retryable failures.

**R2**: `PiPrintWorker` classifies its output as retryable when the pi stderr /
stdout / exit-status text contains a rate-limit signal. Detected signals:
- `429`
- `Too Many Requests`
- `503`
- `Service Unavailable`
- `rate limit` / `rate_limit` / `rate-limit`
- `quota` / `quota_exceeded`
- `Connection reset` / `connection reset by peer`
- `EOF while parsing` combined with a 4xx-range string (heuristic)

Matching is **case-insensitive** on the combined stdout+stderr text the worker
already produces.

**R3**: On a `WorkerOutput { success: false, retryable: true, .. }`, the
scheduler applies **exponential backoff** before retrying the same model
(NVIDIA guidance strategy 1): `delay_ms = backoff_ms * 2^(attempt-1)`, for
attempts `1 ≤ attempt < node.retry.attempts`. Only then, if the model is
still retryable, does it advance to the next `ModelRef` in `node.models`
(NVIDIA guidance strategy 3 = fallback, R6). The remaining attempts for the
failed model are NOT consumed after the advance.

**R3-bis**: `backoff_ms == 0` means "no backoff" — a retryable failure
advances immediately to the next model. This is the default today (all existing
DAGs and tests use `backoff_ms: 0`) and **MUST stay the default** so existing
tests don't incur real sleeps. Production SDD-generated DAGs opt into backoff
by setting `backoff_ms` (e.g. 2000) — wired in a separate follow-up spec,
NOT in this one.

**R4**: On a `WorkerOutput { success: false, retryable: false, .. }`, the
scheduler keeps the existing behavior: retry up to `node.retry.attempts`,
then advance.

**R5**: `paid: true` models are still gated by `--allow-paid` (already wired
in `dispatch_node`). A retryable failure on the last non-paid model with no
`--allow-paid` does not silently fall through to paid — the node fails with a
clear "no payable model available" output.

**R6**: A `ProviderFallback` event MUST be emitted on every model advance,
**including the retryable-advance path**. The current code already does this
for the unconditional-advance path (`fallback:<node>:<from>:<to>`), so this
requirement is "make sure the retryable path goes through the same emitter".

**R6-bis**: A `Node Retry` event with `reason = "429 backoff"` MUST be
emitted on each backoff sleep before retrying the same model (so the event
log records the backoff). Existing real-failure retries keep their
`reason = "attempt failed"` string. Tests assert the reason string differs
between the two paths.

**R7**: Empty-models / shell-node behavior is unchanged. `RealShellWorker`
returns `retryable: false` (a real shell command failing is never a
rate-limit). Shell nodes with empty `models` keep working as today.

**R8**: No new public API beyond `WorkerOutput.retryable`. The `Worker` trait
signature is unchanged. Existing worker callers (`DelayMockWorker`,
`TypeDispatchWorker`, etc.) keep compiling.

---

## Architecture

### WorkerOutput grows one field

```rust
// crates/pidag/src/worker.rs
#[derive(Debug, Clone)]
pub struct WorkerOutput {
    pub success: bool,
    pub output: String,
    pub retryable: bool,   // NEW: default false; true ⇒ skip retries, advance
}
```

Every `WorkerOutput { ... }` literal in `src/` and `tests/` gets
`retryable: false,` appended. Tests that construct `WorkerOutput` directly
must opt in to the new field. This is mechanical and mechanical-only.

Workers classify their own failure reason:

```rust
pub fn classify_retryable(combined_text: &str) -> bool {
    let t = combined_text.to_ascii_lowercase();
    const SIGNALS: &[&str] = &[
        "429", "too many requests", "503", "service unavailable",
        "rate limit", "rate_limit", "rate-limit", "quota", "quota_exceeded",
        "connection reset", "connection reset by peer",
    ];
    SIGNALS.iter().any(|s| t.contains(s))
}
```

`classify_retryable` is `pub(crate)`. `PiPrintWorker` calls it on the
combined `stdout + stderr` text before returning a `WorkerOutput` whose
`success == false`. `RealShellWorker` simply sets `retryable: false`.

### dispatch_node model loop

The current loop is:

```rust
for model in &node.models {
    if model.paid && !allow_paid { continue; }
    ... emit fallback ...
    for attempt in 1..=node.retry.attempts {
        if let Ok(output) = worker.run(...).await {
            if output.success { return Done; }
            else if attempt < node.retry.attempts { emit retry; }
        }
    }
    prev_model = Some(model.name.clone());
}
```

The change is in the inner attempt loop — branch on `output.retryable`:

```rust
for attempt in 1..=node.retry.attempts {
    if let Ok(output) = worker.run(&node.id, &model.name, attempt).await {
        if output.success {
            ... // unchanged: validate, set Done, return
            return (...) ;
        }

        // ---- Retryable path (NVIDIA 429 guidance, strategy 1 + 3) ----
        // See docs/claude-pi-delegation/nvidia-429-strategy.md.
        if output.retryable {
            if attempt < node.retry.attempts && node.retry.backoff_ms > 0 {
                // Strategy 1: exponential backoff before retrying SAME model.
                // base * 2^(attempt-1): attempt 1->2 = base, 2->3 = 2×base, ...
                let delay = node.retry.backoff_ms * (1u64 << (attempt - 1));
                tokio::time::sleep(Duration::from_millis(delay)).await;
                events.push(format!("retry:{}:{}", node.id, attempt));
                continue;  // retry same model
            }
            // Either out of attempts OR backoff_ms == 0 (no backoff configured):
            // Strategy 3: fall through to next ModelRef.
            break;  // exits attempt loop → outer loop advances to next model
        }

        // ---- Real-failure path (unchanged from today: no backoff, retry same model) ----
        if attempt < node.retry.attempts {
            events.push(format!("retry:{}:{}", node.id, attempt));
        }
    }
}
```

**Rationale (from NVIDIA provider guidance — see nvidia-429-strategy.md)**:
on `429`, NVIDIA recommends *first* exponential backoff (2s/4s/8s/16s) on
the **same** model, *then* fallback to an alternative model only after
backoff is exhausted. pidag reuses the existing-but-unused
`node.retry.backoff_ms` field as the base (NVIDIA's floor is 2000 ms).

**`backoff_ms == 0` means "no backoff"**: a retryable failure advances to the
next model immediately. This preserves backwards compatibility and keeps
tests fast (existing DAGs and the toy test set `backoff_ms: 0`).

The `fallback:` event's `to_model` field is the next payable model name (or
`""` if none remains) — the scheduler's existing `handle_fallback` parsing
already extracts `parts[3]` and tolerates empty strings.

### No event-schema changes

`Event::ProviderFallback { node_id, from_model, to_model }` already exists.
`RealSink::handle_fallback` and `redb` projections are unchanged. The
`fallback:` string protocol in `dispatch_node` is reused verbatim.

---

## TDD Contract

| # | Test name | Given | Expected |
|---|-----------|-------|----------|
| T1 | `test_classify_retryable_detects_429` | text `"HTTP 429 Too Many Requests"` | `classify_retryable` returns `true` |
| T2 | `test_classify_retryable_detects_503_and_quota` | text `"quota_exceeded"` and `"503 Service Unavailable"` and `"rate_limit exceeded"` | each returns `true` |
| T3 | `test_classify_retryable_negative_on_real_failure` | text `"error: incompatible types"` | `false` |
| T4 | `test_classify_retryable_is_case_insensitive` | `"Too Many Requests"` uppercase | `true`; lowercase `"too many requests"` also `true` |
| T5 | `test_dispatch_advances_on_retryable_failure` | `backoff_ms=0` (default), node with 2 models `[A (free), B (free)]`, `retry.attempts=3`; worker returns `{success:false, retryable:true}` for A ×3 attempts (exhausted) then `{success:true}` for B | node Done with `model=B`; worker invoked on A exactly `retry.attempts` times (×3, no backoff since `backoff_ms==0`); then a `ProviderFallback` event with `from=A,to=B` is emitted; B invoked once |
| T6 | `test_dispatch_retries_on_real_failure` | node with 2 models `[A, B]`, worker returns `{success:false, retryable:false}` for A (x3 attempts), then `{success:true}` for B, `retry.attempts=3` | node Done with `model=B`; **3** retry events for A before falling to B; retry reasons equal `"attempt failed"` (NOT `"429 backoff"`) |
| T7 | `test_dispatch_retryable_does_not_reach_paid_without_allow_paid` | node with `[A (free), P (paid)]`, worker returns `{success:false, retryable:true}` for A, `--allow-paid` NOT set | node **Failed**, `ProviderFallback` events only reference `from=A` (P is skipped), final output explains payable was skipped |
| T8 | `test_dispatch_retryable_reaches_paid_with_allow_paid` | same node, `--allow-paid` set, worker returns retryable on A then success on P | node Done with `model=P`, `ProviderFallback` `from=A,to=P` emitted |
| T9 | `test_real_shell_worker_marks_not_retryable` | `RealShellWorker` runs a failing command (`exit 1`) | `WorkerOutput.retryable == false` |
| T10 | `test_pi_print_worker_marks_retryable_on_429_stderr` | `PiPrintWorker::with_command` pointed at a shim that prints `"HTTP 429"` to stderr and exits 1 | returned `WorkerOutput.retryable == true`, `success == false` |
| **T11 (backoff, `tokio::time::pause`)** | `test_dispatch_backs_off_on_retryable_with_backoff_ms` | node `[A, B]`, `retry.attempts=3`, `backoff_ms=10`; worker returns `{success:false, retryable:true}` ×2 attempts on A, then `{success:false, retryable:true}` on attempt 3 (exhausted) → advance to B → `{success:true}` | A retried **3 times** (with sleeps `10ms`, `20ms` between attempts); `Node Retry` events have `reason="429 backoff"`; then `ProviderFallback from=A,to=B`; B Done |
| T12 | `test_real_failure_retry_reason_differs_from_backoff` | real failure (`retryable:false`, `backoff_ms=10`) retries ×2 on A then advances to B | `Node Retry` events have `reason="attempt failed"` (NOT `"429 backoff"`), proving the two paths use distinguishable reasons |
| T13 | `test_backoff_then_paid_gate` | node `[A (free), P (paid)]`, `retry.attempts=3`, `backoff_ms=0`, `--allow-paid` NOT set; worker returns retryable on A ×3 | node **Failed**, P never invoked, `ProviderFallback` references only `from=A` (paid skipped) |

Tests T5–T8 use a `ScriptedWorker` (new, in the test file) that returns a
queue of `WorkerOutput`s keyed by `(node_id, model)` so the order of model
advancement is deterministic and observable.

---

## Exit Criteria

- [x] `cargo test -p pidag` passes (current 118 + T1–T10 ≥ 128 passed, no regressions)
- [x] `cargo clippy -p pidag -- -D warnings` clean
- [x] `grep -rn "retryable" crates/pidag/src/worker.rs` — the new field is set
      on every `WorkerOutput { ... }` construction site (no `..Default::default()`
      that silently leaves it `false` in a place that should be `true`)
- [x] T11 asserts retryable failures with `backoff_ms > 0` sleep `backoff_ms * 2^(attempt-1)` between retries (NVIDIA guidance strategy 1)
- [x] T5 asserts the default `backoff_ms == 0` advance-after-exhaust-attempts path (no real sleeps in tests)
- [x] T7 asserts paid models are NOT silently used when `--allow-paid` is absent
- [x] No changes to `Worker` trait method signatures
- [x] No changes to `Event` enum or `EventSink` trait
- [x] HANDOFF.md updated with this spec's verification results

---

## Guardrails

- **No 429 auto-failover at config time** — this is runtime only.
  `ModelsConfig` and `.pidag/config.toml` parsing are untouched.
- **No new dependencies**. String matching is `str::contains` (one helper).
- **No changes to the `Worker` trait** (`async fn run(...)`) — only the
  return struct grows a field.
- **Shell workers always return `retryable: false`**. A shell `exit 1` is a
  real failure, not a rate limit.
- **Backoff is backoff-on-retryable only** (NVIDIA guidance strategy 1). Real
  failures (`retryable == false`) keep the existing no-sleep retry behavior.
- **`backoff_ms == 0` means "no backoff"** (default; preserves backwards
  compatibility and keeps tests fast — `cargo test` must NOT incur real
  sleeps). Production DAGs opt into backoff via `RetryPolicy.backoff_ms`
  (e.g. 2000) — setting that default in the SDD generator is a separate
  follow-up spec (`production-dag-backoff-defaults.md`), NOT this one.
- **No runtime concurrency cap** in this spec. NVIDIA's "1 req / 2-3s,
  20-30 RPM" guidance (strategy 2) is a scheduler-wide token-bucket concern
  touching the `Semaphore` permit layer — orthogonal code path, orthogonal
  tests. Lives in its own spec (`runtime-rate-limit-throttle.md`).
- **Heuristic only** — `classify_retryable` is a substring sweep, not a
  protocol parser. False positives (a successful response that happens to
  quote "429") are benign because the caller only consults `retryable` when
  `success == false`. False negatives (a 429 phrased unusually) degrade to
  the existing retry-then-advance behavior — safe but suboptimal.

---

## Files to Modify

1. `crates/pidag/src/worker.rs` — add `WorkerOutput.retryable`, add
   `classify_retryable`, set flags in `PiPrintWorker` (`success==false` path)
   and `RealShellWorker` (always `false`); leave `DelayMockWorker` `false`.
2. `crates/pidag/src/scheduler/execute.rs` — branch on `output.retryable`
   inside `dispatch_node`'s inner attempt loop; emit `fallback:` and `break`
   to next model on retryable failure; preserve real-failure retry behavior.
3. `crates/pidag/tests/runtime_429_failover_tests.rs` — NEW. T1–T10 plus the
   `ScriptedWorker` helper.
4. Any test file that constructs `WorkerOutput { ... }` directly — append
   `retryable: false,` (mechanical; found via `grep -rn "WorkerOutput {"`).

No `config.rs`, no `sdd.rs`, no `event.rs`, no `bin/` changes required.

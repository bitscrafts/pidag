# pidag — spec-34: Typed node state and typed dispatch events

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: MEDIUM — nothing here is broken today. It is where the next bug comes from,
  and it makes phase 5 substantially simpler.
- **Status**: PLANNED
- **Source**: 2026-08-12 codebase audit — S-1, S-2.
- **Depends-On**: none strictly. Best done **before** spec-35 (index identity), which it
  simplifies; typed state removes half the reason the `try_enqueue!` macro exists.

> **Execution context: entirely inside the `pidag-runner` container**, in `/projects/pidag`.

---

## Overview

Two internal protocols are encoded as strings, and both are load-bearing.

**Node state is a `String`**, compared against literals in 66 places:

```rust
state: "Pending".to_string(),
if state.state == "Done" { ... }
matches!(node_state.get(dep).map(|s| s.state.as_str()), Some("Failed") | Some("Blocked"))
```

Five states, no type. A typo compiles and silently takes the wrong branch; adding a state
gets no exhaustiveness check; every assignment heap-allocates. Worse, the `store` layer
independently re-spells the same literals in `NodeRecord`, so scheduler and store can drift
without anything noticing.

**Dispatch events are colon-delimited strings.** `dispatch_node` returns `Vec<String>` built
with `format!("retry:{}:{}", node.id, attempt)` and `fallback:…:from:to`, which the caller
parses with `evt.find(':')`, `evt.split(':').collect()`, then indexes `parts[2]`/`parts[3]`.

Two problems. It is a private protocol between two functions in the same file that could be
an enum. And **the delimiter is not safe**: node ids come from user-authored templates, and
the gate syntax `"<node>:fail"` already puts a colon in the same namespace. A node id
containing `:` silently shifts the field indices and produces a wrong `ProviderFallback`
event rather than an error.

---

## Requirements

### Functional

- **Y1 (`NodeStatus` enum)**: replace the state `String` with

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  pub enum NodeStatus { Pending, Running, Done, Failed, Blocked }
  ```

  `Copy` matters: it removes the clone-per-transition noted in the audit as P1-5.

- **Y2 (one definition, shared)**: `NodeStatus` is used by **both** the scheduler
  (`NodeState`) and the store (`NodeRecord`). The two must not carry independent spellings.
  Serialisation must remain **wire-compatible** with the existing strings (`"Done"`,
  `"Failed"`, …) so vaults written by earlier builds still load — this is the constraint
  that makes the change safe to ship.

- **Y3 (no string comparison survives)**: no `== "Done"`, no `matches!(…, Some("Failed"))`,
  no `.state.as_str()` comparisons. A source-scanning test enforces it, because the
  literals are easy to reintroduce and the compiler cannot help once one exists.

- **Y4 (`DispatchEvent` enum)**: replace the colon protocol with

  ```rust
  enum DispatchEvent {
      Retry { attempt: usize },
      BackoffRetry { attempt: usize },
      Fallback { from: String, to: String },
  }
  ```

  `dispatch_node` returns `Vec<DispatchEvent>`; the caller matches. No parsing, no indexing.

- **Y5 (node ids reject the reserved delimiter)**: `dag.validate()` rejects a node id
  containing `:`, naming the id. The gate syntax `"<node>:fail"` reserves that character,
  and today a colon in an id corrupts gate matching silently. Cheap, static, and it closes
  the class rather than the instance.

- **Y6 (behaviour is identical)**: this is a representation change. Every existing test must
  pass **unmodified** except where a test constructs a `NodeState`/`NodeRecord` literal and
  must now name a variant instead of a string. No assertion may change.

### Non-Functional

- **N1**: **wire compatibility is mandatory** (Y2). A vault written before this change must
  still load. `tests/checkpoint_resume_tests.rs` and `crash_recovery_tests.rs` are the guard.
- **N2**: no scheduler restructuring. Index identity and the `try_enqueue!` macro are
  spec-35.
- **N3**: no new dependencies. `serde` already derives what is needed.
- **N4**: gate stays green; count may only go up.

---

## Architecture

The change is mechanical and wide. The only design decision worth stating:

**`#[serde(rename_all)]` or explicit variant renames keep the wire format identical.** The
enum must serialise to exactly the strings already in vaults — this is what lets the change
ship without a migration. Verify by loading a fixture vault written by the current build,
not by reasoning about serde's defaults.

**Why `Copy` and not just `Clone`**: the scheduler assigns state on every transition and the
audit found 2–3 clones per terminal node. A `Copy` enum makes those free and deletes the
noise rather than optimising it.

**Why the id restriction is in `validate()` and not the template layer**: hand-written
`dag.json` bypasses templates entirely, and the gate parser is in the engine. The check
belongs where the invariant is consumed.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| Y1 | `test_node_status_is_copy` | compile-level | `NodeStatus` is `Copy`; assigning does not move |
| Y2a | `test_status_serialises_to_legacy_strings` | each variant | JSON is exactly `"Pending"`, `"Running"`, `"Done"`, `"Failed"`, `"Blocked"` |
| Y2b | `test_legacy_vault_still_loads` | a vault fixture written **before** this change | loads; every node's status round-trips. **The compatibility guard** |
| Y2c | `test_scheduler_and_store_share_the_type` | source scan | `NodeRecord.state` and `NodeState.state` are both `NodeStatus` |
| Y3 | `test_no_string_state_comparisons_in_src` | source scan | no `== "Done"` / `Some("Failed")` / `.state.as_str()` comparison remains |
| Y4a | `test_dispatch_events_are_typed` | source scan | `dispatch_node` returns `Vec<DispatchEvent>`; no `split(':')` on dispatch events |
| Y4b | `test_fallback_event_carries_both_models` | a node falling back | `Fallback { from, to }` has correct values; previously `parts[2]`/`parts[3]` |
| Y5a | `test_node_id_with_colon_is_rejected` | id `"weird:node"` | `dag.validate()` errors naming the id. **Fails today** |
| Y5b | `test_gate_matching_unaffected_by_valid_ids` | normal ids with gates | unchanged behaviour |
| Y6 | `test_full_suite_unmodified_assertions` | the existing suite | passes with no assertion changed |

**Y2b is the risky one.** Generate the fixture vault from the **current** build before
changing anything, commit it, and only then start. A compatibility test written after the
change proves nothing.

---

## Exit Criteria

```bash
cd /projects/pidag

grep -q 'enum NodeStatus'   src/scheduler/mod.rs
grep -q 'enum DispatchEvent' src/scheduler/execute.rs
[ "$(grep -rc '== "Done"' src/ --include=*.rs | awk -F: '{s+=$2} END{print s+0}')" = "0" ]
! grep -rq 'state.as_str()' src/ --include=*.rs
grep -q 'colon\|:' src/core/dag.rs   # Y5 id validation present
test -f tests/fixtures/legacy_vault/   # Y2b fixture committed

bash deploy/scripts/quality-gate.sh . ; echo "GATE EXIT=$?"
env PIDAG_REQUIRE_PI=1 cargo test -j 2 --no-fail-fast 2>&1 | grep -E '^test result:'
```

**Prose criteria**:

1. The legacy-vault fixture was generated **before** any code change and is committed.
   State the commit it was generated from.
2. `Y5a` confirmed failing before the change (a colon id is accepted today).
3. Every test-file edit is a `NodeState`/`NodeRecord` **literal construction** change only.
   List them. **No assertion may change** — if one does, the representation change altered
   behaviour and is wrong.
4. Raw per-binary `^test result:` lines, unsummed.

---

## Guardrails

- **G1 — NEVER modify any file under `specs/`.** Stop and report.
- **G2 — NO WORKHORSE MAY COMMIT.**
- **G3 — NEVER modify `/projects/_upstream/`.**
- **G4 — wire compatibility is not negotiable** (N1). If keeping the existing strings is
  awkward, keep them anyway; a migration is a different spec and a much larger risk.
- **G5 — do NOT change any assertion** (Y6). Literal construction updates are expected;
  changing what a test asserts means the refactor altered behaviour.
- **G6 — do NOT restructure the scheduler**, do not touch `try_enqueue!`, do not introduce
  index-based identity. All spec-35.
- **G7 — do NOT add states.** Exactly the five that exist. A sixth is a design change with
  scheduler consequences and needs its own spec.
- **G8 — never `rm -rf` a `.pidag/` directory.** `_tmp/bug-a-bloodtest/` and
  `_tmp/interp-probe/` hold live run records; the legacy fixture must be a **copy**.
- **G9 — report raw output, never summed totals.** One line per binary, copied not retyped.
- **G10 — clippy clean at `-D warnings`.**

### Error handling expectations

- Deserialising an unknown status string is an error naming the value, not a silent default
  to `Pending` — a vault containing an unexpected state means something is wrong and
  defaulting would mask it.
- `dag.validate()` rejecting a colon id must name the offending id.

---

## Files to Modify

| File | Change |
|------|--------|
| `src/scheduler/mod.rs` | `NodeStatus`; `NodeState.state` typed |
| `src/scheduler/execute.rs` | `DispatchEvent`; all state comparisons typed |
| `src/store/mod.rs` | `NodeRecord.state` uses `NodeStatus` |
| `src/store/redb_store.rs` | serialisation, wire-compatible |
| `src/sdd/resume.rs` | checkpoint categorisation by variant |
| `src/core/dag.rs` | reject `:` in node ids (Y5) |
| `src/ui/`, `src/cli/show.rs` | render variants |
| `tests/fixtures/legacy_vault/` | **NEW** — generated pre-change (Y2b) |
| `tests/typed_state_tests.rs` | **NEW** — the TDD Contract |

## Memory

Store on completion: `workspace/specs/pidag-34-typed-state`,
`claude-pi-delegation/fix/20260812-stringly-typed-state`.

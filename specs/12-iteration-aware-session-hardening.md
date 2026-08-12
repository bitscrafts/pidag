# Spec: Iteration-Aware Session Hardening (harness-improvement, retrospec-2026-08-07)

**Project**: `.` (container: `/projects/pidag`) — **host: pi_agent_rust** (`/root/.pi/agent/`)
**Topic**: `claude-pi-delegation`
**Depends-On**: spec-11 (whose afternoon implementation supplied the
retrospective data under `claude-pi-delegation/experiment/session-slowness-rehydration-tax`).
**Scope**: This spec is **advisory + cross-cutting** — it specifies changes
across the pi agent harness (compaction, commit hooks, skill review), the
spec-generator skill (already partially applied in this session), and the
workspace authoring conventions. It is implemented piecemeal in the host
repo, NOT in pidag/src. Each numbered item below is independently
landable; order is by ROI per the retrospective.

---

## Overview

The pidag spec-11 implementation (completed this session, `f857776`)
exhibited ~6× the iterations its technical complexity warranted. The
retrospective (`claude-pi-delegation/experiment/session-slowness-rehydration-tax`)
isolated the cause: **rehydration tax**, not difficulty. Four tax sources,
in severity order:

1. **Compaction dropping in-flight state** — when the harness compacts
   mid-task, the prose summary lists "modified-files" but NOT the diff/edits
   themselves. Re-discovering "what did I already write?" cost 3–8
   iterations each (hit twice).
2. **Inconsistent spec TDD rows** — spec-11's contract had a row
   *"A weight 2.0 (3 pending) → A emits 4 slots"* — impossible, A only has
   3 entries. Implementing to that row hit impossible assertions;
   re-derivation cost ~5 iterations.
3. **Commits only at forced handoffs** — green intermediate steps went
   uncommitted and were lost on compaction; each forced handoff
   re-re-hydrated (~6 iterations).
4. **Server timeouts as interrupts** (~2 iterations each).

This spec specifies the four harness/process fixes (A–D) that close those
taxes. Item B is **already applied** in this session as a partial MVP
(template guidance + `run.sh validate` subcommand); this spec formalizes the
production version.

## Requirements

### Functional

- **R1 (A) — Pending-edits persistence across compaction**: the pi harness
  MUST persist a re-applicable delta of pending/uncommitted file edits to
  `~/.pi/agent/sessions/<session-id>/pending-edits.jsonl` (or `.redb`)
  **before** any context-compaction event. After compaction, the summary
  MUST include a `pending_edits_ref` pointer so the next turn can re-load
  the exact diff and either re-apply or diff-merge rather than re-derive.
- **R2 (A) — Replay command**: `pi --replay-pending-edits <session-id>`
  re-applies any persisted pending-edits that have not landed in the
  working tree (idempotent — skips edits whose `oldText` is already absent
  from the file, indicating the edit already landed).
- **R3 (B) — Spec TDD-row feasibility checker** (shipped this session):
  the spec-generator skill's `validate <spec>` subcommand scans each TDD
  row and warns when an `Expects` count exceeds the max count determinable
  from the `Given`. Exit 0 advisory (never fails the build). Confirmed to
  catch the canonical spec-11 bug.
- **R4 (B) — Feasibility note in template** (shipped this session): the
  `spec-template.md` now includes a "TDD-row feasibility checklist
  (MANDATORY)" block the author must re-read before saving.
- **R5 (C) — Commit-on-green default for multi-iteration tasks**: for any
  task that exceeds one tool-iteration budget, the pi agent should commit
  at every green test gate (build + test + fmt + clippy all clean), not
  only at the 80% budget handoff. This is implemented as a **behavioral
  convention note** in `~/.pi/agent/skills/handoff-generator/SKILL.md` and
  `/root/.pi/agent/packages/docs` + a `COMMIT_GREEN=1` env var the
  harness inspects to confirm the convention is active for a session.
- **R6 (D) — Spec-scope reduction rule**: a spec that touches both a NEW
  function AND rewrites an existing function in a different module MUST
  be split. The spec-generator's editorial check (validator) also warns
  when a spec's "Files to Modify" table contains >1 file in
  `src/` with a non-trivial delta vs `src/tests/`. Configurable via
  `SPEC_MAX_SRC_FILES` env var (default 2).

### Non-Functional

- **N1**: A and B must not add per-iteration latency >50ms. A is a write
  to a single jsonl/redb on the compaction event (already serialized).
- **N2**: A is mandatory and behavior-changing in the pi harness, treated
  as a separate PR in `pi_agent_rust`. B/C/D are skill/docs-only and ship
  via `/root/.pi/agent/` directly.
- **N3**: The validator is purely advisory; never auto-fails authoring.

## Architecture

```
pi session turn ──tools run──▶ [edits applied to working tree]
            │                              │
            │                              ▼
            │                     compaction event? ──no──▶ continue
            │                              │ yes
            ▼                              ▼
  ~/.pi/agent/sessions/<sid>/      summary text +
  pending-edits.jsonl              pending_edits_ref: "<sid>/pending-edits.jsonl"

  ── next turn ──▶ agent loads summary, follows pending_edits_ref,
                  diffs against disk: any edit whose oldText is still
                  present = re-apply; any whose oldText is absent =
                  assume already committed/skipped. No re-derivation.
```

The validator (B) and conventions (C, D) are static: skill + docs only.

## TDD Contract

| Test Name | Given | Expects |
|-----------|-------|---------|
| `test_validator_catches_infeasible_row` | a spec with row `A weight 2.0 (3 pending) -> A emits 4 slots` | `run.sh validate` prints `WARN: row expects count=4 but Given provides max=3` and exits 0 *(shipped + verified 2026-08-07)* |
| `test_validator_clean_on_feasible_spec` | spec-11 source spec (ranges, not single counts) | `run.sh validate` prints no WARN, exits 0 *(shipped + verified 2026-08-07)* |
| `test_pending_edits_persisted_pre_compaction` *(harness, future PR)* | simulate mid-task compaction with one uncommitted edit | `pending-edits.jsonl` exists, contains the edit's `path`+`oldText`+`newText`, summary includes the `pending_edits_ref` pointer |
| `test_replay_pending_edits_idempotent` *(harness, future PR)* | replay an edit already present in the working tree | `pi --replay-pending-edits` skips it (oldText absent) without erroring |
| `test_replay_pending_edits_reapplies_lost` *(harness, future PR)* | replay an edit whose oldText still exists (compaction dropped it) | `pi --replay-pending-edits` re-applies the edit, exit 0 |
| `test_commit_on_green_convention_documented` | `grep -q COMMIT_GREEN /root/.pi/agent/skills/handoff-generator/SKILL.md` | exits 0 *(docs land in this session's follow-up commit)* |
| `test_spec_scope_rule_documented` | `grep -q SPEC_MAX_SRC_FILES /root/.pi/agent/skills/spec-generator/SKILL.md` | exits 0 *(docs land in this session's follow-up commit)* |

## Exit Criteria

- [ ] `/root/.pi/agent/skills/spec-generator/run.sh validate <bad.md> 2>&1 | grep -q "row expects count=4 but Given provides max=3"` *(shipped 2026-08-07)*
- [ ] `/root/.pi/agent/skills/spec-generator/templates/spec-template.md | grep -q "TDD-row feasibility checklist"` *(shipped 2026-08-07)*
- [ ] `grep -q COMMIT_GREEN /root/.pi/agent/skills/handoff-generator/SKILL.md` *(docs follow-up)*
- [ ] `grep -q SPEC_MAX_SRC_FILES /root/.pi/agent/skills/spec-generator/SKILL.md` *(docs follow-up)*
- [ ] A (pending-edits persistence) — tracked as a separate PR in pi_agent_rust; not blocked by this spec. Excluded from the exit-criteria gate below once that PR lands.

## Guardrails

- **Do NOT** block this spec on item A landing in the harness. A is a
  separate PR; B/C/D ship first.
- **Do NOT** make the validator authoritative (it never fails authoring).
- **Do NOT** retroactively mark the spec-11 work as red — it landed
  functionally complete. The retrospective is forward-looking.
- **Do NOT** add new dependencies to the pidag crate.

## Files to Modify / Ship

| Item | Path | Change | Status |
|---|---|---|---|
| B-template | `/root/.pi/agent/skills/spec-generator/templates/spec-template.md` | add "TDD-row feasibility checklist" block | ✅ shipped this session |
| B-validator | `/root/.pi/agent/skills/spec-generator/run.sh` | add `validate` dispatch + feasibility checker | ✅ shipped this session |
| C-docs | `/root/.pi/agent/skills/handoff-generator/SKILL.md` | add "Commit on Green" convention note + `COMMIT_GREEN` env | TODO |
| D-docs | `/root/.pi/agent/skills/spec-generator/SKILL.md` | add "Spec-scope reduction" rule + `SPEC_MAX_SRC_FILES` model | TODO |
| A-harness | `pi_agent_rust` (host repo) | persist `pending-edits.jsonl` on compaction event + summary `pending_edits_ref` + `pi --replay-pending-edits` | separate PR, future |

## Implementation Notes

- The retrospective insight that produced this spec is stored at
  `claude-pi-delegation/experiment/session-slowness-rehydration-tax`.
- Item B's validator is a heuristic that may emit false positives; the
  spec author has final say. It is intended to catch the canonical
  "expected count > available count" failure, which was the highest-cost
  per-iteration bug observed this session.
- Item A is the only behavior change to the harness runtime and warrants a
  dedicated SDD cycle in the pi_agent_rust repo, not pidag. Filed here for
  cross-reference.

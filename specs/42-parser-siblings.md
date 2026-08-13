# pidag — spec-42: the other two copies of the section-parsing bug

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: MEDIUM — smaller blast radius than spec-41, same root cause, fix already written.
- **Status**: IMPLEMENTED and verified 2026-08-13. Gate green. P4 failed first at 5
  discrepancies (9 criteria, 12 TDD rows), then passed at zero. The counts differ from
  the 11/19 measured when this spec was written because this spec itself joined the
  corpus and its own body quotes the buggy idiom, self-tripping it.
- **Source**: 2026-08-13. Reported by the spec-41 implementation, which found the sibling
  call sites while staying inside its own scope and flagged them rather than widening it.
- **Depends-On**: spec-41 — landed. Its fixed extractor is what these two should be using.

> **Execution context: entirely inside the `pidag-runner` container**, in `/projects/pidag`.

---

## Overview

spec-41 rewrote `extract_section` as a fence-aware line scan. **Two other functions in the
same file still carry the original bug**, each with its own hand-rolled copy of the
terminator search:

- `parse_exit_criteria` (`src/split/mod.rs:56`) — `remaining.find("##")`
- `parse_tdd_tests` (`src/split/mod.rs:95`) — `remaining.find("##")`

Both truncate at the first `##` substring, so a `###` sub-heading, or a `##` line inside a
code fence, ends the section early.

**Measured across `specs/*.md`:**

| lost to the bug | count |
|---|---|
| specs affected | 3 |
| exit-criteria items dropped | **11** |
| TDD table rows dropped | **19** |

Smaller than spec-41's 37 empty sections, because Exit Criteria and TDD Contract sections
rarely *open* with a sub-heading — but the consequence is the same in kind. `split` allocates
criteria across children; criteria it cannot see are allocated to nobody and appear in no
child. Eleven silently missing exit criteria is eleven things no part is required to satisfy.

This is a duplication defect as much as a parsing one. The same three-line idiom exists in
three places; spec-41 fixed one. **The fix is to delete the other two, not to repair them.**

---

## Requirements

### Functional

- **P1 (one extractor)**: `parse_exit_criteria` and `parse_tdd_tests` obtain their section
  body from spec-41's fixed `extract_section`. The hand-rolled `find("##")` terminator search
  is **deleted** from both, not corrected in place.

- **P2 (no third copy)**: after this change, `find("##")` appears **nowhere** in
  `src/split/mod.rs`. A guard test asserts this, because the whole failure mode here is the
  idiom being copied.

- **P3 (behaviour is otherwise identical)**: both functions return what they return today for
  any input where the old and new extraction agree — which is every spec not among the three
  affected. Criteria indices, ordering, and the `- [ ]` / `- [x]` parsing are unchanged.

- **P4 (the repo is the guard)**: a test iterates `specs/*.md` and asserts that the number of
  exit-criteria items and TDD rows parsed equals the number actually present in each section.
  Following spec-41's E6: **it must fail before the change**, reporting counts close to 11 and
  19. If it does not, stop and report — the measurement in this spec would be wrong.

### Non-Functional

- **N1**: `split`'s grouping, allocation heuristics, spec-40 attribution and spec-41 extraction
  are all unchanged. Expect child specs to gain criteria they were always meant to carry.
- **N2**: no new runtime dependencies, no Markdown parser.
- **N3**: **never modify `/projects/_upstream/`.**
- **N4**: the gate stays green; the test count may only go up.
- **N5**: **the specs are not edited to suit the parser.** Same reasoning as spec-41 N6.

---

## Architecture

```mermaid
flowchart TD
    A["extract_section<br/>fence-aware, spec-41"] --> B["parse_exit_criteria"]
    A --> C["parse_tdd_tests"]
    A --> D["generate_child_spec_content"]
    E["find(\"##\") x2<br/>DELETED"] -.->|"removed"| B
```

**Key decision — delete, do not repair.** Two more correct-but-separate implementations would
be two more things to keep in sync, and the reason this spec exists is that the idiom was
copied in the first place. One extractor, three callers.

**Key decision — the guard counts, it does not sample.** P4 compares parsed counts against
actual counts per file. A test that merely checks "some criteria were parsed" is what already
passed while eleven went missing.

**What this spec is not**: it is not a change to allocation, grouping, or the `- [ ]` item
grammar. It is the removal of two duplicated bugs.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| P1a | `test_exit_criteria_survive_sub_heading` | Exit Criteria containing a `###` sub-heading with items after it | all items parsed |
| P1b | `test_tdd_rows_survive_sub_heading` | TDD Contract with a `###` sub-heading mid-section | all rows parsed |
| P1c | `test_exit_criteria_survive_fenced_hash` | a `## …` line inside a fenced block within Exit Criteria | items after it parsed |
| P2 | `test_no_bare_hash_search_remains` | source scan of `src/split/mod.rs` | `find("##")` appears zero times |
| P3 | `test_unaffected_specs_parse_identically` | specs where old and new agree | identical output |
| P4 | `test_repo_criteria_counts_match` | every `specs/*.md` | parsed count == actual count, for criteria and TDD rows. **Fails before the change** |
| N1 | existing `split_tests.rs`, `split_scope_tests.rs` | unchanged | still green |

**P4 is the acceptance test**, and P2 is what stops the bug returning a fourth time.

---

## Exit Criteria

- [ ] `! grep -q 'find("##")' src/split/mod.rs` — no copy of the idiom remains (P2)
- [ ] `grep -c 'extract_section' src/split/mod.rs` returns at least 4 — one definition, three callers (P1)
- [ ] `bash deploy/scripts/quality-gate.sh .` exits 0
- [ ] `cargo test -p pidag test_repo_criteria_counts_match -- --exact` passes, and its pre-change failure is quoted in the report (P4)
- [ ] `cargo test -p pidag test_no_bare_hash_search_remains -- --exact` passes (P2)
- [ ] `git diff --name-only -- specs/ | grep -v 42-parser-siblings` is empty — no spec edited to suit the parser (N5)
- [ ] `git diff --name-only | grep -q '_upstream'` finds nothing (N3)
- [ ] `cargo clippy -p pidag -- -D warnings` is clean (N4)
- [ ] `git hash-object tests/fixtures/legacy_vault/legacy.redb` is `cd51a399ba5dea8c415bac66c0084d4f168044c0`

**Prose criteria**:

1. **P4 quoted failing before the change**, with its counts, and passing after at zero
   discrepancies.
2. Test counts pasted raw, one `^test result:` line per binary, **unsummed**.

> **Note on this section's format.** Earlier specs in this series (36–41) wrote Exit Criteria
> as a shell block plus numbered prose. That is **not** the house format: `split`'s
> `parse_exit_criteria` and the `validate-exit-criteria.sh` validator both consume `- [ ]`
> items, as `specs/92-configurable-models.md` and `src/split/README.md` document. The
> consequence is that **none of specs 36–41 can be split or validated by pidag itself.** This
> spec uses the conforming format; the earlier ones need converting, which is tracked
> separately.

---

## Guardrails

- **G1 — do NOT hand-edit any existing file under `specs/`** (N5). If this spec looks wrong,
  **STOP and report it** — the architect amends it. This spec exists *because* the spec-41
  implementer reported these call sites instead of silently widening its own scope.
- **G2 — NO WORKHORSE MAY COMMIT.** Leave work in the tree.
- **G3 — NEVER modify `/projects/_upstream/`.**
- **G4 — do NOT write a corrected copy of the terminator search** (P1). Call `extract_section`.
  Three correct implementations is the same defect as three incorrect ones, one refactor later.
- **G5 — do NOT weaken P4** to make it pass: no skipping files, no allowlist, no "at least one
  item" softening. It must count.
- **G6 — do NOT change allocation, grouping, the `- [ ]` grammar, or spec-40/41 behaviour** (N1).
- **G7 — do NOT regenerate any pinned fixture.**
- **G8 — never `rm -rf` a `.pidag/` directory.** Move it aside with `mv`.
- **G9 — report raw output, never summed totals.**
- **G10 — no hardcoded absolute paths.**

### Error handling expectations

- `parse_exit_criteria` returns `Result` and errors when the section is absent; keep that.
  `parse_tdd_tests` returns an empty vector for an absent section; keep that too. The
  asymmetry is pre-existing and out of scope.

---

## Files to Modify

| File | Change |
|------|--------|
| `src/split/mod.rs` | both functions call `extract_section`; delete both `find("##")` copies (P1, P2) |
| `tests/split_scope_tests.rs` | the TDD Contract above (P1–P4) |

**Not modified**: `specs/` (by hand), `deploy/`, `/projects/_upstream/`, allocation and
grouping, spec-40 attribution, spec-41 extraction.

## Memory

Store on completion: `workspace/specs/pidag-42-parser-siblings`,
`claude-pi-delegation/fix/20260813-parser-siblings`.

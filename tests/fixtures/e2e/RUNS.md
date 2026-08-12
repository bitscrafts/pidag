# End-to-end run log — 2026-08-12

Every live run performed while proving pidag works as a product. Recorded because
each one overturned something that testing alone had reported as fine.

Format: what was run, what it proved, and what it changed.

---

## 1. Bloodtest, before spec-29

**Ran**: `pidag sdd 01-blood-test.md --run` on `_tmp/bug-a-bloodtest/`.

**Result**: full recovery sequence — `validate-iter1` failed, the gate fired,
`implement-iter2` dispatched and wrote `FIXED.txt`, `implement-iter3` skipped.
Reported as green.

**What it actually showed**: reading the model's own reply revealed it had been
dispatched with the literal text `{{validate-iter1.output}}` and had recovered the
failure by *reading the event log itself*. The loop repaired **blind**. The run
succeeded despite the feature, not because of it.

**Changed**: spec-29 written — runtime output interpolation.

---

## 2. Bloodtest, after spec-29 landed (commit `276a112`)

**Result**: identical event sequence, still green — and the placeholder was **still
literal**. 477 tests passed while the feature did nothing.

**Why the suite missed it**: the acceptance test called `interpolate_outputs`
directly. The unit worked; the seam did not.

---

## 3. Interpolation probe — the isolating experiment

**Ran**: a two-node DAG, zero tokens.
`b.prompt = "echo SAW: {{a.output}}"`, `b.depends_on = ["a"]`.

**Result**: `b` executed `echo SAW: {{a.output}}` — the **literal**.

**The diagnostic that cracked it**: a surviving literal means the interpolation
result was *discarded*; an empty substitution would have meant the lookup found
nothing. Those point at completely different code. `Worker::run` took only
`node_id`, and every worker held its own prompt snapshot captured at construction.

**Changed**: spec-29 I10 — the prompt moved into the `Worker` trait signature and
all seven implementations lost their snapshot fields.

---

## 4. Interpolation probe, after I10 (commit `a202d8e`)

**Result**: `b ✓ Done  SAW: HELLO-FROM-A`. Proven end to end, zero tokens.

---

## 5. Bloodtest, after I10

**Result**: literal `{{validate-iter1.output}}` occurrences in the run log went
**2 → 0**. `implement-iter2`'s reply named the specific failing criterion
(`test -f FIXED.txt`) — content that exists only in `validate-iter1`'s output.

**Proved**: the loop repairs *with sight*.

---

## 6. csvstats — first real software

**Ran**: `01-csv-stats.md`, a 45-line spec with 4 objective exit criteria, in a
fresh git repo.

**Result**:

```
NodeFailed     validate-baseline     <- correct: no code yet
NodeDone       implement-iter1       <- 1762B csvstats.py + 2460B tests
NodeDone       quality-gate-1
NodeDone       validate-iter1        <- exit criteria PASS
NodeDone       implement-iter2       <- SKIPPED (no NodeDispatched)
NodeDone       implement-iter3       <- SKIPPED
```

Verified independently: 4 unit tests pass; output matches the spec exactly
(`age,2,10.0,20.0,15.0`); non-numeric columns skipped; missing file exits 1 with
errno on stderr.

**Significance**: the first run to exercise the **success** branch. The bloodtest
only ever exercised the failure branch. Neither alone covers the gate.

---

## 7. logtool split, before the split fix

**Ran**: `02-logtool-split.md`, 15 criteria → `--auto` produced 3 children at 100%
coverage, `--validate` PASS.

**Result**: running **part1 alone built all six files** of the three-module system,
all three test suites passing. Parts 2 and 3 had nothing left to do.

**Cause**: `split` partitioned only the Exit Criteria; Architecture and Requirements
were copied wholesale, so every child still described the whole system. Running all
three costs ~3x the tokens for one system; running one makes the split pointless.

**Changed**: audit finding U-2.

---

## 8. logtool split, after the fix (`28ae45a`, `2863fc1`)

**The first fix exposed a deeper defect.** Adding a `Scope` section made it list
*whole shell commands* as artifacts, which pointed at `extract_mentioned_modules`:
it matched only `.rs` files and treated an entire backtick span as a module name.
Criteria are written as `` `test -f logfilter.py` ``, so a Python spec yielded
either nothing or the whole command — and `group_by_module` keys on
`mentioned_modules[0]`, so the distribution was arbitrary.

Every unit test for that function used Rust filenames in unbackticked prose — the
one shape where the broken implementation worked.

**Result after fixing**: grouping is module-coherent, one module plus its test per
part, each part self-contained:

| part | owns |
|---|---|
| part1 | `logfilter.py`, `test_logfilter.py` |
| part2 | `logreport.py`, `test_logreport.py` |
| part3 | `logparse.py`, `test_logparse.py` |

**Still imperfect**: a scoped run produced **4 files where Scope named 2** — down
from 6, not solved. The Architecture section is still copied verbatim and merely
annotated as wider than the part, so the child says "build only `logfilter.py`"
while showing an Architecture describing three modules. **When a spec instructs
narrowly but describes broadly, the description wins.** The remaining fix is to
narrow the Architecture prose itself.

---

## What the runs established that the test suite did not

| claim | suite said | a run said |
|---|---|---|
| spec-26 templates work | 3 static gates green | could not parse at all |
| spec-29 interpolation works | 477 tests green | placeholder passed through untouched |
| spec-32 batching works | tests green | harness measured the old path |
| `split` decomposes work | coverage 100%, validate PASS | one child built everything |
| module grouping is coherent | unit tests green | grouping was arbitrary on any non-Rust spec |

Five cases, one pattern: **a check that looks rigorous while testing the wrong
surface.** The generalisation — an acceptance test must assert on what the
*consumer* received, not on what the producing function returned.

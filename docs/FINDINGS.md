# Findings

Defects found while hardening pidag, and the pattern behind them. Recorded because the
pattern generalises well beyond this codebase.

## The pattern: a check that looks rigorous while testing the wrong surface

Six times in one session, a green test suite coexisted with a broken feature. Every time,
the check exercised the *unit* and never the *seam*.

| what was claimed | the suite said | a run said |
|---|---|---|
| workflow templates load | three static gates green | **could not parse at all** — TOML has no `null` type |
| output interpolation works | 477 tests green | **placeholder reached the model verbatim** |
| transaction batching works | tests green | **the harness measured the old path** |
| `split` decomposes work | 100% coverage, validate PASS | **one child built the entire system** |
| module grouping is coherent | unit tests green | **arbitrary on any non-Rust spec** |
| every store method is offloaded | guard test green | **the guard false-positived on the one method it protected** |

The generalisation, and the rule now applied here:

> **An acceptance test must assert on what the *consumer* received, not on what the
> producing function returned.**

And its corollary: when a check counts occurrences of a code pattern, verify the count
against reality before trusting it. One exit criterion matched 2 of 12 discards because
rustfmt wrapped the other ten across two lines.

## Defects worth knowing about

### verify measured a state, not a delta

Every implement node verified with `git diff --quiet && exit 1 || exit 0` — which passes
**iff the working tree is dirty**. Its purpose was to catch a worker claiming success while
changing nothing.

On resume that predicate is *already satisfied*: a crashed node leaves exactly the dirt it
tests for. A resumed node that did nothing verified green, was marked `Done`, and satisfied
the gate. **The guarantee that existed to detect empty work was void precisely in the
scenario recovery exists for, and it failed open.**

Fixed with `verify_pre`: a token captured before dispatch, compared after. The predicate now
measures change *across the node*.

A second round was needed. The first token hashed `git status --porcelain`, which encodes
file names and states but **not contents** — so editing an already-dirty file produced
identical output and failed verification despite real work. Two false negatives, both in the
resume-and-repair case. The counterweight test could not catch it because it created a *new*
file, which does alter porcelain output.

### The interpolated prompt never reached the worker

`{{node.output}}` interpolation ran at dispatch and its result was **discarded**. `Worker::run`
took only `node_id`, and every worker held a snapshot of the prompts captured at
construction, looking its command up by id.

The diagnostic that cracked it generalises: **a surviving literal placeholder means the
result was discarded; an empty substitution means the lookup found nothing.** Those point at
completely different code.

### Blocked was never persisted

`RedbSink::emit` had no arm for `Event::NodeBlocked`, and `terminal_set` filtered to
`Done | Failed`. Together they made `load_checkpoint`'s `"Blocked"` branch **dead code** — it
could not execute under any input. On resume, a node blocked by a failed dependency was read
back as `Pending` and re-dispatched.

Found while generating a test fixture, not by any test.

### The pool's capacity limit did nothing

`if pool.created < pool.capacity { pool.created += 1; }` — and then a client was created
regardless of the answer. `created` was never decremented. The cap never blocked; it only
stopped counting. Any sustained error pattern became an unbounded subprocess spawn loop.

### The gate-skip cascade mis-dispatched at depth two

The cascade released a skipped node's dependents without checking whether *they* were gated
on it — so a second-level repair node whose source had not failed was **dispatched and ran**.
Not a stall; wrong work.

### `pidag` resolved itself through PATH

Five sites used `Command::new("pidag")`. A freshly built binary handed the real work to
whichever copy was installed, so a rebuild could be "verified" while the old code ran. It
concealed two separate changes before being found.

### `split` divided the checklist, not the work

`--auto` partitioned Exit Criteria but copied `Architecture` and `TDD Contract` wholesale, so
every child still described the whole system. Running one child built everything; running all
three cost three times the tokens for one system.

The root cause sat a level deeper: the module extractor matched only `.rs` files and treated
an entire backtick span as a module name. Since criteria are written as `` `test -f x.py` ``,
a Python spec yielded either nothing or the whole shell command — and the grouping heuristic
keys on that. **Every unit test for it used Rust filenames in unbackticked prose, the one
input shape where the broken implementation worked.**

Partially fixed: grouping is now module-coherent, but a scoped run still over-builds, because
the Architecture section is annotated as wider than the part rather than narrowed to it.
**When a spec instructs narrowly but describes broadly, the description wins.**

## Where the failures actually came from

Scored against the [MAST taxonomy](ARCHITECTURE.md#sources) — 42% specification ambiguity,
37% coordination, 21% verification — one session of work reproduced the published
distribution closely, and **the specification errors were the orchestrating layer's**, not
the workers': a requirement premised on a misreading of a dependency's API, an exit criterion
that matched a fraction of its targets, a lock that would have deadlocked by design, a
requirement placed in a module with no access to what it needed.

That is the ICML 2026 finding reproduced in miniature: failures originate in the orchestrator.
It is why the next work is in the specification and verification layers rather than the
engine.

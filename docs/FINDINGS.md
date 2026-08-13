# Findings

Defects found while hardening pidag, and the pattern behind them. Recorded because the
pattern generalises well beyond this codebase.

## The pattern: a check that looks rigorous while testing the wrong surface

Eight times in one session, a green test suite coexisted with a broken feature. Every time,
the check exercised the *unit* and never the *seam*.

| what was claimed | the suite said | a run said |
|---|---|---|
| workflow templates load | three static gates green | **could not parse at all** — TOML has no `null` type |
| output interpolation works | 477 tests green | **placeholder reached the model verbatim** |
| transaction batching works | tests green | **the harness measured the old path** |
| `split` decomposes work | 100% coverage, validate PASS | **one child built the entire system** |
| module grouping is coherent | unit tests green | **arbitrary on any non-Rust spec** |
| every store method is offloaded | guard test green | **the guard false-positived on the one method it protected** |
| vault wire format is compatible | the compat guard passed | **no pre-change vault could be opened at all** |
| `split` produces complete child specs | all `split` tests green | **a third of every spec silently dropped** |

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

### The compatibility guard was regenerated by the change it was guarding

spec-34 changed two serialized fields from `String` to a `NodeStatus` enum. The store uses
**bincode**, which is not self-describing: a `String` is a length prefix plus bytes, an enum
is a variant index. Old records therefore decode as
`invalid value: integer 6, expected variant index 0 <= i < 5` — `6` being the byte length of
`"Failed"` read as a variant index. Every vault written before that change became unopenable,
and the vault is the only record of a run.

The change shipped carrying `#[serde(rename_all = "PascalCase")]` and the comment *"wire-format
compatibility to existing strings"*. The attribute is **inert under bincode**, which never
emits names at all. Alongside it came `NodeStatus::parse`, commented *"for deserialization
from legacy data"* — dead code. The migration it was written for was never built, and nothing
noticed the promise had no implementation behind it.

What makes this the sharpest instance of the pattern: spec-34 **did** require a compatibility
guard, and one was built correctly — a real pre-change vault, captured in its own commit
expressly so the guard would mean something. But the generator that produced it was a plain
`#[tokio::test]` with no `#[ignore]`, so it ran on every `cargo test` and rewrote the fixture
using the *current* build. The refactor commit swept the regenerated fixture in alongside the
change it was meant to police. From that moment the test read back what the build under test
had just written.

> **A fixture that the test suite can regenerate is not a fixture.** The producer and the
> consumer had become the same build, so the guard could not fail no matter what the format
> did. Pin the artifact's hash, or the guard silently becomes decorative.

Restoring the genuine blob made it fail on the first run. Fixed by spec-36: vaults now carry
an explicit schema version, and a v1 vault migrates on open inside a single write transaction
with the version stamped last, so it only reads as v2 once every record has actually been
converted.

There is a tail to this one. spec-36 as written also required migrating the `events` table,
on the premise that spec-34 had changed an `Event` variant's `state` field too. It had not —
the `state:` assignments spec-34 touched in `event.rs` are inside the sink's handler, where
it builds `NodeRecord` values for the *nodes* table, and no `Event` variant carries a `state`
field at all. The implementing agent checked this against the real fixture, found every event
decoded cleanly, **reported the discrepancy instead of quietly working around it**, and built
the frozen mirror type anyway because the spec said so. It was then withdrawn and removed.

That is the orchestrator failing the same way twice in one spec: the defect was a false
compatibility claim, and the fix for it shipped with a false claim of its own. It also shows
the guardrail working — a spec that forbids the workhorse from editing requirements gets the
disagreement surfaced rather than absorbed.

### `split` lost a third of every spec to a nine-line parser

`extract_section` ended a section at `remaining.find("##")` — a *substring* search. A
`### Functional` sub-heading therefore terminated the section, so a `## Requirements` whose
body opens with one extracted as **empty**. That is the house style of every spec from 21
onward. Two further defects sat in the same nine lines: the terminator also matched inside
fenced code blocks, and the start was unanchored, so a `### Requirements` sub-heading or a
prose mention of `` `## Architecture` `` could win over the real heading.

Measured across 227 sections in `specs/`: **37 extracted empty despite having content, 39 lost
more than 10%, 151 were intact.** One third of all spec content, silently dropped. The largest
single case was 7,942 characters of Requirements.

`split` writes child specs from these extractions. A child missing its Requirements is not a
degraded brief — it is a brief with the contract removed, handed to an implementer with no way
to know. Every `split` test passed throughout, because their fixtures are simple specs with no
`###` sub-headings: **the one input shape where the broken implementation works.** The same
sentence was already written in this file about the module extractor, one section above.

It surfaced only because spec-40 added Architecture attribution and its implementer reported
that the feature "never fires visibly" on real specs — noticing that a *new* feature was
inert, and looking for why, rather than shipping it green.

> **When a component is fed real data in production and fixtures in tests, the fixtures are a
> statement about what you imagined, not about what arrives.** Point the test at the real
> corpus.

Fixed by spec-41, whose regression guard iterates `specs/*.md` rather than fixtures.

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

---
name: spec-author
description: >
  Write a spec that a cheaper model can implement correctly without judgement
  calls. Use before delegating any non-trivial change via pi-delegate. Covers
  the six mandatory sections, verified premises, and exit criteria that a
  machine can check.
---

# spec-author

The spec IS the contract. It carries the judgement so the workhorse does not
have to supply any.

This matters more when the implementer is a cheaper model: **every ambiguity you
leave is a decision it will make for you.** Published failure analysis puts 42%
of multi-agent failures on specification ambiguity, and in practice those errors
belong to the orchestrator, not the worker.

## Verify every premise before writing it

Do not write "X currently does Y" unless you have read X. Requirements have had
to be withdrawn mid-flight for being premised on a misreading — an API assumed
synchronous that was async, a field assumed serialized that was not, a lock that
would have deadlocked by design.

Read the code the spec talks about. Grep for the call sites. Then write.

## The six sections

1. **Overview** — the why, and the defect or driver in concrete terms.
2. **Requirements** — numbered, each independently checkable. Functional and
   non-functional separated.
3. **Architecture** — modules, data flow, and **key decisions with rationale**.
   State what was rejected and why; that is what stops a reimplementation.
4. **TDD Contract** — one row per behaviour: id, test name, given, expects.
   Name the **acceptance test** explicitly, and say which requirement is the one
   a plausible implementation gets wrong.
5. **Exit Criteria** — `- [ ]` checkbox items, each a runnable command in
   backticks. This grammar is what `validate-exit-criteria` and `split` consume;
   prose criteria cannot be machine-checked.
6. **Guardrails** — what the implementer must NOT do, and the error-handling
   expectations. Say why, briefly: a rule with a reason survives paraphrase.

## Make the dangerous requirement obvious

For each spec, ask: *which requirement will a plausible implementation satisfy
in appearance only?* Name it, and give it a test that fails on the naive version.

Where it matters, require the implementer to **write the wrong version first,
watch the test fail, then fix it** — and to paste both outputs. That is the only
proof the test can detect the defect at all.

## Exit criteria must be machine-checkable

Every requirement needs a criterion, and no criterion may be subjective. "Code
looks good" is not a criterion. Prefer a shell command with an exit status.

Include the negative checks — no hardcoded absolute paths, no forbidden
dependency, the pinned fixture hash unchanged.

## Guardrails that have proven necessary

Carry these forward unless there is a reason not to: never edit the spec; never
commit; never weaken or delete a test; never regenerate a pinned fixture; report
a wrong premise rather than coding around it; raw output, never summed totals.

## Length is not the goal

A spec is long enough when a competent implementer needs no judgement calls.
Beyond that, more words dilute the binding parts.

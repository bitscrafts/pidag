---
name: pi-delegate
description: >
  Delegate implementation to `pi` (deepseek) instead of a Claude subagent, keeping
  Claude as planner and reviewer. Use when implementing a spec, fixing a failing
  gate, or any multi-file code change where the reasoning is bounded by a written
  contract. Saves ~80-90% of implementation tokens.
---

# pi-delegate

Claude plans and verifies. `pi` implements. The split exists because
implementation reasoning is the expensive part and the cheapest competent model
that passes the gate is the right one to do it — one session spent 1.9M tokens
on Claude subagents for work `pi` could have done.

## The two layers

- **This skill** tells *Claude* when and how to delegate.
- **`~/.pi/agent/skills/rust-specialist/`** tells *pi* how to implement.

They are separate files for separate readers. Do not merge them.

## The one thing that will silently break

**Invoking `pi` from a non-interactive shell does not apply the user's shell
alias**, so `~/.pi/agent/system.md` is absent and pi runs with **no operating
rules whatsoever**. Verified 2026-08-14: the bare binary answers NO to "must you
update HANDOFF.md"; the same call with `--append-system-prompt` answers YES.

Never call `pi` directly for real work. Always go through
`deploy/scripts/pi-workhorse.sh`, which composes the guardrails explicitly.

Second trap: `system.md` commandment 1 instructs the agent to **git commit before
modifying files**, which violates this project's *no workhorse may ever commit*
rule. The harness appends project overrides **after** the commandments and marks
them as winning. If you compose a prompt by hand, you must do the same.

## The loop

```
1. Claude writes the spec           (expensive, valuable, stays with Claude)
2. pi-workhorse.sh implement <spec> (deepseek does the work)
3. pi-workhorse.sh gate             (objective — exit code, not opinion)
4. fail? pi-workhorse.sh repair     (continues the session, keeps context)
     still failing after 2 rounds → escalate the model, do not keep retrying
5. pi-workhorse.sh review <spec>    (read-only pi, second pair of eyes)
6. Claude reads the load-bearing diff, then commits
```

Roles are separated **by tool grant, not by instruction**: the reviewer gets
`read,grep,find,ls` and structurally cannot edit. A reviewer that cannot write is
worth more than a reviewer told not to.

## Token discipline — this is what makes it cheap

- **Verify by running checks, not by reading code.** The gate returns an exit
  code and ~50 `test result:` lines: ~2k tokens to know the suite passed. Reading
  an 800-line diff to reach the same conclusion costs 20x.
- **Still read the load-bearing diff before committing.** Not the mechanical bulk
  — the parts where the spec's judgement lives. Skipping this is how a tree gets
  committed uncompiling.
- **Demand terse replies.** The harness caps them; do not undo that.
- Never paste a whole file back to pi. It has `read`.

## Escalation

Default `deepseek-v4-flash`. Escalate on: two failed gate rounds, a reported
spec defect needing judgement, or anything touching wire formats, migrations or
concurrency. Pass the model as the third argument.

`pi --list-models` shows what is configured. Escalating is cheaper than three
more rounds of a model that cannot see the problem.

## When NOT to delegate

- Writing or amending a spec — that is the contract, and it is Claude's job.
- Deciding whether a reported premise is actually wrong — requires reading the
  surrounding code with judgement.
- Anything where the failure mode is silent (wire formats, fixtures, migrations).
  Delegate the typing, keep the verification.
- Committing. Ever.

## What to watch for

The most valuable output of a workhorse is **a report that the spec is wrong**.
Five requirements in this project were withdrawn or corrected that way. When pi
says a premise is bad, verify it directly rather than believing or dismissing it
— one agent's claim that the container had no model credentials was false and got
repeated into three documents before anyone checked.

Corollary: an agent's incidental claim about its environment is not a
measurement.

---
name: pi-delegate
description: >
  Delegate implementation to `pi` instead of a Claude subagent, keeping Claude as
  planner and reviewer. Use when implementing a spec, fixing a failing quality
  gate, or any multi-file change bounded by a written contract. Cuts
  implementation token cost by roughly an order of magnitude.
---

# pi-delegate

Claude plans and verifies. `pi` implements.

The split exists because implementation reasoning is the expensive part, and the
cheapest competent model that passes the gate is the right one to do it. One
session spent **1.9M tokens** on Claude subagents for work `pi` could have done.
Which model that is belongs to pi's configuration, not to this skill.

## The layers, and what each one is for

| layer | file | reader | encodes |
|---|---|---|---|
| orchestration | `claude/skills/pi-delegate/SKILL.md` | Claude | when and how to delegate |
| spec authoring | `claude/skills/spec-author/SKILL.md` | Claude | how to write a contract worth implementing |
| operating rules | `pi/DIRECTIVES.md` | pi | the hard rules, every invocation |
| implement role | `pi/skills/implementer/SKILL.md` | pi | TDD order, failure modes |
| review role | `pi/skills/reviewer/SKILL.md` | pi | read-only architecture review |
| harness | `bin/pi-workhorse.sh` | shell | tool grants, sessions, escalation |

They are separate files for separate readers. Do not merge them.

## Guardrails travel with the harness

`pi` auto-loads `AGENTS.md` / `CLAUDE.md` from its working directory, and does
**not** auto-load `~/.pi/agent/system.md` (that arrives only via an interactive
shell alias, which does not expand in a non-interactive shell).

**Do not rely on either.** `CLAUDE.md` carries *solution* detail — what this
project is — and may be absent, stale, or written for a different audience.
Operating rules live in `pi/DIRECTIVES.md` and are passed explicitly by the
harness on every call. That is what makes this bundle portable.

Never invoke `pi` directly for real work. Go through the harness.

## The loop

```
1. Claude writes the spec              → see spec-author skill
2. pi-workhorse.sh implement <spec>    → pi does the work
3. pi-workhorse.sh gate                → objective: exit code, not opinion
4. fail? pi-workhorse.sh repair <spec> → same session, keeps context
     after ORCH_MAX_REPAIR_ROUNDS      → repair <spec> <root> --escalate
5. pi-workhorse.sh review <spec>       → read-only pi, architecture review
6. pi-workhorse.sh validate <spec>     → exit criteria
7. pi-workhorse.sh handoff             → HANDOFF.md updated?
8. Claude reads the load-bearing diff, then commits
```

Roles are separated **by tool grant, not by instruction**: the reviewer gets
`read,grep,find,ls` and structurally cannot edit.

`gate`, `validate` and `handoff` invoke **no model at all**. They are exit codes.

## Step 8 is not optional

Read the load-bearing diff before committing — the parts where the spec's
judgement lives: error paths, ordering, persistence, wire formats, anything the
spec called a key decision. Verify the architecture built matches the one
specified.

Not the mechanical bulk. Mechanical churn is what the gate is for. But a green
gate has repeatedly coexisted with a broken feature, and every one of those was
visible in the diff to someone who read it.

## Token discipline

- **Verify by running checks, not by reading code.** The gate is an exit code
  plus ~50 `test result:` lines — a couple of thousand tokens to know the suite
  passed. Reading an 800-line diff for the same conclusion costs 20×.
- **Read the load-bearing parts anyway.** The two rules above are in tension;
  resolve it by *selecting* what to read, not by skipping it.
- **Demand terse replies.** The harness caps them; do not undo that. Prompt bulk
  costs latency as well as tokens — an oversized system prompt once timed a
  trivial query out at two minutes.
- Never paste a file back to pi. It has `read`.

## Model selection and escalation

The harness hardcodes no model: omitting `--model` uses pi's own configuration
(`settings.json`, `PI_MODEL`, `PI_PROVIDER`).

Pass `--escalate` as the model argument to switch to `ORCH_ESCALATION_MODEL` from
`config.env` (default `glm-5.2:cloud` via `ollama-cloud`). Edit `config.env` as
better models appear — that is the one place the choice lives.

Escalate on: `ORCH_MAX_REPAIR_ROUNDS` failed gate rounds, a reported spec defect
needing judgement, or anything touching wire formats, migrations or concurrency.
Escalating is cheaper than three more rounds of a model that cannot see the
problem.

## When NOT to delegate

- Writing or amending a spec. That is the contract and it is Claude's job.
- Judging whether a reported premise is actually wrong — read the code yourself.
- Anything whose failure mode is silent: wire formats, fixtures, migrations.
  Delegate the typing; keep the verification.
- Committing. Ever.

## What to watch for

The most valuable output of a workhorse is **a report that the spec is wrong**.
Several requirements have been withdrawn or corrected that way. When pi says a
premise is bad, **verify it directly** rather than believing or dismissing it —
one agent's claim that its container had no model credentials was false and got
repeated into three documents before anyone checked.

An agent's incidental claim about its environment is not a measurement.

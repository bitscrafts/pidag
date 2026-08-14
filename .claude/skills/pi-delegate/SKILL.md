---
name: pi-delegate
description: >
  Delegate implementation to `pi` instead of a Claude subagent, keeping
  Claude as planner and reviewer. Use when implementing a spec, fixing a failing
  gate, or any multi-file code change where the reasoning is bounded by a written
  contract. Saves ~80-90% of implementation tokens.
---

# pi-delegate

Claude plans and verifies. `pi` implements. The split exists because
implementation reasoning is the expensive part, and the cheapest competent model
that passes the gate is the right one to do it — one session spent 1.9M tokens on
Claude subagents for work `pi` could have done. Which model that is belongs to
pi's own configuration, not to this skill.

## The two layers

- **This skill** tells *Claude* when and how to delegate.
- **`~/.pi/agent/skills/rust-specialist/`** tells *pi* how to implement.

They are separate files for separate readers. Do not merge them.

## How guardrails actually reach pi

**pi auto-loads `AGENTS.md` / `CLAUDE.md` from its working directory.** It does
**not** auto-load `~/.pi/agent/system.md` — that arrives only through the user's
interactive shell alias, which does not expand in a non-interactive shell.

Verified 2026-08-14, both directions: `pi -p` run bare in this repo answers NO to
"is a workhorse ever allowed to run git commit" (stated only in `CLAUDE.md`),
while the same binary answers NO to "must you update HANDOFF.md" until
`--append-system-prompt` is passed explicitly.

Three consequences, and they are the whole design:

1. **`CLAUDE.md` is the guardrail channel.** Worker-facing rules must live there
   to take effect. This is why the project's hard rules belong in the main Claude
   file rather than in a spec or a wrapper.
2. **Working directory is load-bearing.** The harness `cd`s to the project root
   before invoking. Run pi from anywhere else and the rules silently vanish.
3. **`system.md` is deliberately not appended.** Its commandment 1 orders a git
   commit before editing, which this project forbids outright; its HANDOFF.md and
   memory commandments belong to the orchestrator. Appending it would mean
   shipping a conflict and then overriding it — and the extra bulk pushed a
   trivial query past a 2-minute timeout on the configured flash model.

pidag's own workers take the same channel: `PiPrintWorker` runs `pi` with cwd at
the project root, so they inherit `CLAUDE.md` too, plus a small anti-loop prompt
of their own.

## The loop

```
1. Claude writes the spec           (expensive, valuable, stays with Claude)
2. pi-workhorse.sh implement <spec> (pi's configured model does the work)
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
- **Demand terse replies.** The harness caps them; do not undo that. Prompt bulk
  costs latency as well as tokens — an oversized system prompt timed a trivial
  query out at two minutes.
- Never paste a whole file back to pi. It has `read`.

## Model selection

**The harness hardcodes no model.** Omitting `--model` lets pi use whatever its
own configuration defines — `settings.json` (`provider`, `model`) or the
`PI_MODEL` / `PI_PROVIDER` environment. That is the default and it should stay
the default: a model baked into the harness silently contradicts pi's config the
moment that config changes.

Pass a model as the **third argument** only to escalate. Escalate on: two failed
gate rounds, a reported spec defect needing judgement, or anything touching wire
formats, migrations or concurrency. `pi --list-models` shows what is available.
Escalating is cheaper than three more rounds of a model that cannot see the
problem.

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

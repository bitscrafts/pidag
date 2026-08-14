# pi-orchestration

**Claude Code plans and verifies. `pi` implements.**

A portable, self-contained bundle for running spec-driven development where the
expensive model writes the contract and reviews the result, and a cheaper model
does the typing. One session spent **1.9M tokens** on Claude subagents for work
`pi` could have done under a written spec.

Nothing here depends on the host's `CLAUDE.md`, `AGENTS.md`, or shell aliases.
Drop it on a new machine, run `install.sh`, and the loop works.

---

## Install

```bash
./install.sh                       # ~/.pi-orchestration, ~/.claude/skills, ~/.pi/agent/skills
./install.sh --orch-home /opt/orch # or place it wherever

export PATH="$PATH:$HOME/.pi-orchestration/bin"
export ORCH_HOME="$HOME/.pi-orchestration"
```

Idempotent: existing files are backed up with a `.bak-<timestamp>` suffix rather
than overwritten.

**Requires**: `pi` on `PATH` (or `PI_BIN` set), provider credentials configured
for pi, and a quality-gate script in the target project at
`deploy/scripts/quality-gate.sh` (or pi's `quality-gate` skill installed).

---

## The layers

Six files, six readers. They are deliberately not merged.

| layer | file | read by | encodes |
|---|---|---|---|
| **orchestration** | `claude/skills/pi-delegate/SKILL.md` | Claude | when and how to delegate; the loop; token discipline |
| **spec authoring** | `claude/skills/spec-author/SKILL.md` | Claude | how to write a contract a cheap model can implement without judgement |
| **operating rules** | `pi/DIRECTIVES.md` | pi, every call | never commit, never edit specs, report wrong premises, no weakened tests |
| **implement role** | `pi/skills/implementer/SKILL.md` | pi | TDD order and the usual failure modes |
| **review role** | `pi/skills/reviewer/SKILL.md` | pi | read-only architecture review against the spec |
| **harness** | `bin/pi-workhorse.sh` | shell | tool grants, sessions, escalation, gate/validate/handoff |

Plus `config.env` (tunables) and `templates/` (spec and HANDOFF skeletons).

---

## Why a directives file instead of `CLAUDE.md`

`pi` auto-loads `AGENTS.md` / `CLAUDE.md` from its working directory, and does
**not** auto-load `~/.pi/agent/system.md` — that arrives only through an
interactive shell alias, which does not expand in a non-interactive shell. A
programmatic call therefore gets **no operating rules at all** unless you pass
them.

But relying on the auto-load is wrong for a different reason: `CLAUDE.md` carries
*solution* detail — what a particular project is and how it is built. It is
project-specific, it changes, and on a new machine it may be absent or written
for a different audience.

So operating rules live in `pi/DIRECTIVES.md` and are passed explicitly on every
invocation. **The rules travel with the harness; the solution detail stays with
the project.** That separation is what makes the bundle portable.

---

## The loop

```
1. Claude writes the spec              → spec-author skill, templates/spec-template.md
2. pi-workhorse.sh implement <spec>    → pi implements, tests first
3. pi-workhorse.sh gate                → objective: an exit code, not an opinion
4. fail? pi-workhorse.sh repair <spec> → same session, context retained
     after N rounds:  repair <spec> <root> --escalate
5. pi-workhorse.sh review <spec>       → read-only pi, architecture review
6. pi-workhorse.sh validate <spec>     → exit criteria
7. pi-workhorse.sh handoff             → HANDOFF.md updated?
8. Claude reads the load-bearing diff, then commits
```

**Roles are separated by tool grant, not by instruction.** The reviewer gets
`read,grep,find,ls` and structurally cannot edit. A reviewer that cannot write is
worth more than a reviewer told not to.

**`gate`, `validate` and `handoff` invoke no model.** They are exit codes.

**Step 8 is not optional.** A green gate has repeatedly coexisted with a broken
feature, and every one of those was visible in the diff to someone who read it.
Read the parts where the spec's judgement lives — error paths, ordering,
persistence, wire formats — not the mechanical bulk.

---

## Configuration

`config.env`, all overridable by environment variable:

| setting | default | meaning |
|---|---|---|
| `ORCH_MODEL` | *(empty)* | normal work. Empty = whatever pi is configured to use. Leave it empty. |
| `ORCH_ESCALATION_MODEL` | `glm-5.2:cloud` | used with `--escalate` |
| `ORCH_ESCALATION_PROVIDER` | `ollama-cloud` | provider for the above |
| `ORCH_MAX_REPAIR_ROUNDS` | `2` | repair attempts before escalating |
| `ORCH_MAX_TOOL_ITERATIONS` | `60` | bound on the agent's tool loop |
| `ORCH_REQUEST_TIMEOUT` | `600` | seconds per turn |

**No model is hardcoded in the harness.** Omitting `--model` uses pi's own
configuration; pinning one here would silently contradict it the moment it
changes. Escalation is the exception, and `config.env` is the single place that
choice lives — edit it as better models appear.

---

## Enforced conventions

**`specs/`** — every non-trivial change starts with a spec. The workhorse may
read them and may never edit them: a worker that can edit the spec can make any
failure vanish by rewriting the requirement.

**`HANDOFF.md`** — the implementation diary, checked by `pi-workhorse.sh
handoff`. Read before starting, updated before finishing. A blocker recorded
there is worth more than a blocker remembered.

**`_tmp/`** — all scratch and test artefacts, never `/tmp/`, never an absolute
path. Absolute paths pass locally and fail on a fresh checkout.

---

## What this bundle assumes you have learned

The rules in `DIRECTIVES.md` are not generic hygiene. Each one is a defect that
shipped:

- a compatibility guard **regenerated by the very commit it was guarding**, so it
  could not fail
- a third of every spec silently dropped by a nine-line parser, because its tests
  used fixtures and production used real files
- a budget breach that marked the run complete, so resume refused to continue it
- five requirements withdrawn because a worker **reported a bad premise** instead
  of coding around it — the most valuable output those runs produced

The recurring shape: **a green suite beside a broken feature**, because the check
exercised the unit and never the seam. Hence: assert on what the consumer
received, verify by running the gate, and read the load-bearing diff anyway.

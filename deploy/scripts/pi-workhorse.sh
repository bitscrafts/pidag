#!/usr/bin/env bash
#
# pi-workhorse.sh — drive `pi` as the implementer while Claude Code orchestrates.
#
# Why this exists: invoking `pi` from a non-interactive shell does NOT pick up
# the user's shell alias, so `~/.pi/agent/system.md` is silently absent and the
# agent runs with no operating rules at all. Verified 2026-08-14: the bare
# binary answers NO to "must you update HANDOFF.md", the same call with
# --append-system-prompt answers YES. Every invocation here composes the
# guardrails explicitly.
#
# Roles are separated by TOOL GRANT, not by instruction:
#   implement  read,write,edit,bash,grep,find,ls   — can change the tree
#   review     read,grep,find,ls                   — structurally CANNOT edit
#   gate       (no model)                          — the objective check
#   validate   (no model)                          — exit-criteria check
#
# A reviewer that cannot write is worth more than a reviewer told not to.
#
# Usage:
#   pi-workhorse.sh implement <spec_path> [project_root] [model]
#   pi-workhorse.sh repair    <spec_path> [project_root] [model]   # continues the session
#   pi-workhorse.sh review    <spec_path> [project_root] [model]
#   pi-workhorse.sh gate      [project_root]
#   pi-workhorse.sh validate  <spec_path> [project_root]
#
set -uo pipefail

PI="${PI_BIN:-/root/.local/bin/pi}"
CMD="${1:-}"
[ -z "$CMD" ] && { echo "usage: $0 {implement|repair|review|gate|validate} ..." >&2; exit 2; }
shift

DEFAULT_MODEL="${PI_MODEL:-deepseek-v4-flash}"
MAX_ITER="${PI_MAX_TOOL_ITERATIONS:-60}"

# ---------------------------------------------------------------- guardrails
# pi AUTO-LOADS AGENTS.md / CLAUDE.md from the working directory — verified
# 2026-08-14: `pi -p` run bare in this repo correctly answers NO to "is a
# workhorse ever allowed to run git commit", which is stated only in CLAUDE.md.
# That is why `cd "$ROOT"` below is load-bearing, not tidiness: invoke from
# anywhere else and the project rules silently do not load.
#
# `~/.pi/agent/system.md` is NOT auto-loaded, and is deliberately NOT appended
# here. Its commandment 1 orders the agent to git commit before editing, which
# this project forbids outright; its HANDOFF.md and memory-contract commandments
# belong to the orchestrator, not to a bounded spec implementation. Appending it
# would mean shipping a conflict and then overriding it. Cheaper and safer to
# let CLAUDE.md be the single channel — and much shorter, which matters: a
# bloated system prompt pushed a trivial query past a 2-minute timeout on
# deepseek-v4-flash.
#
# Consequence: worker-facing rules MUST live in CLAUDE.md to take effect.
role_preamble() {
    cat <<'GUARD'
You are a workhorse under an orchestrator. The project's CLAUDE.md rules are
already in your context and are binding — in particular: never git commit, never
edit anything under specs/, never touch /projects/_upstream/.

If the spec is wrong, incomplete or self-contradictory, STOP and say so in one
line rather than coding around it. Those reports are the most valuable thing you
produce.

Be terse. Your reply is read by an orchestrator paying per token: no preamble,
no restating the task, no narrating what you are about to do.
GUARD
}

# ------------------------------------------------------------------ helpers
session_dir_for() {  # one session per spec, so repair keeps context
    local root="$1" spec="$2"
    local d="$root/_tmp/pi-sessions/$(basename "$spec" .md)"
    mkdir -p "$d"; echo "$d"
}

run_gate() {
    local root="$1"
    if [ -x "$root/deploy/scripts/quality-gate.sh" ]; then
        bash "$root/deploy/scripts/quality-gate.sh" "$root"
    else
        echo "no quality-gate.sh at $root/deploy/scripts/ — cannot verify" >&2
        return 127
    fi
}

run_validate() {
    local spec="$1" root="${2:-.}"
    local v
    for v in "$HOME/.pi/agent/skills/validate-exit-criteria/run.sh" \
             /usr/local/scripts/validate-exit-criteria.sh; do
        [ -x "$v" ] && { "$v" "$spec" "$root"; return $?; }
    done
    echo "validate-exit-criteria not found — exit criteria UNVERIFIED" >&2
    return 127
}

# -------------------------------------------------------------------- roles
case "$CMD" in
  implement|repair|review)
    SPEC="${1:?spec_path required}"; ROOT="${2:-$(dirname "$(dirname "$SPEC")")}"
    MODEL="${3:-$DEFAULT_MODEL}"
    [ -f "$SPEC" ] || { echo "no such spec: $SPEC" >&2; exit 2; }
    SD="$(session_dir_for "$ROOT" "$SPEC")"

    case "$CMD" in
      implement)
        TOOLS="read,write,edit,bash,grep,find,ls"
        CONT=""
        TASK="Implement the spec at $SPEC in project root $ROOT, following it exactly.
Write the tests from its TDD Contract FIRST, then the production code.
Verify with: bash $ROOT/deploy/scripts/quality-gate.sh $ROOT
Do not stop until that gate exits 0, or until you hit something the spec gets wrong.
Reply in at most 25 lines: files changed, the gate's four stage results, every
'test result:' line verbatim, and anything in the spec you found wrong."
        ;;
      repair)
        TOOLS="read,write,edit,bash,grep,find,ls"
        CONT="-c"
        TASK="${PI_REPAIR_MSG:-The gate is still failing. Read the failure above, fix the cause, and re-run the gate. Reply in at most 15 lines.}"
        ;;
      review)
        # Read-only by tool grant. It cannot edit even if it decides to.
        TOOLS="read,grep,find,ls"
        CONT=""
        TASK="Review the working-tree changes in $ROOT against the spec at $SPEC.
You have READ-ONLY tools by design; do not attempt to modify anything.
Check specifically: does each Requirement have a corresponding change; was any
existing test weakened or deleted to make things pass; does any new check assert
on what the CONSUMER received rather than on what the producing function
returned; are there hardcoded absolute paths.
Reply in at most 20 lines: PASS or FAIL on the first line, then findings, most
serious first. Say FAIL if any requirement is unimplemented or any test was weakened."
        ;;
    esac

    # cd is load-bearing: pi auto-loads CLAUDE.md from the working directory,
    # and that is the entire guardrail channel. Invoked from elsewhere, the
    # project rules silently do not reach the model.
    cd "$ROOT" || { echo "cannot cd to $ROOT" >&2; exit 2; }

    exec "$PI" -p $CONT \
        --session-dir "$SD" \
        --model "$MODEL" \
        --tools "$TOOLS" \
        --max-tool-iterations "$MAX_ITER" \
        --append-system-prompt "$(role_preamble)" \
        "$TASK"
    ;;

  gate)     run_gate "${1:-.}" ;;
  validate) run_validate "${1:?spec_path required}" "${2:-.}" ;;
  *) echo "unknown command: $CMD" >&2; exit 2 ;;
esac

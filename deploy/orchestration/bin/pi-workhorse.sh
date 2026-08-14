#!/usr/bin/env bash
#
# pi-workhorse.sh — drive `pi` as the implementer while Claude Code orchestrates.
#
# Part of the pi-orchestration bundle. Self-contained: it depends on this
# bundle's DIRECTIVES.md and config.env, NOT on the host's CLAUDE.md or
# AGENTS.md. Those carry solution detail and may be absent or written for a
# different audience; operating rules travel with the harness.
#
# Roles are separated by TOOL GRANT, not by instruction:
#   implement  read,write,edit,bash,grep,find,ls   — can change the tree
#   repair     read,write,edit,bash,grep,find,ls   — continues the same session
#   review     read,grep,find,ls                   — structurally CANNOT edit
#   gate       (no model)                          — the objective check
#   validate   (no model)                          — exit-criteria check
#   handoff    (no model)                          — checks HANDOFF.md was updated
#
# A reviewer that cannot write is worth more than a reviewer told not to.
#
# Usage:
#   pi-workhorse.sh implement <spec> [root] [model]
#   pi-workhorse.sh repair    <spec> [root] [model]     # --escalate for the big model
#   pi-workhorse.sh review    <spec> [root] [model]
#   pi-workhorse.sh gate      [root]
#   pi-workhorse.sh validate  <spec> [root]
#   pi-workhorse.sh handoff   [root]
#
# Model: none is hardcoded. Omitting it uses pi's own configuration. Pass
# `--escalate` as the model argument to use config.env's ORCH_ESCALATION_MODEL.
#
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORCH_HOME_DEFAULT="$(dirname "$HERE")"
: "${ORCH_HOME:=$ORCH_HOME_DEFAULT}"
[ -f "$ORCH_HOME/config.env" ] && . "$ORCH_HOME/config.env"

PI="${PI_BIN:-$(command -v pi || echo /root/.local/bin/pi)}"
DIRECTIVES="$ORCH_HOME/pi/DIRECTIVES.md"

CMD="${1:-}"
[ -z "$CMD" ] && { sed -n '20,29p' "$0" >&2; exit 2; }
shift

die() { echo "pi-workhorse: $*" >&2; exit 2; }
[ -x "$PI" ] || die "pi binary not found (set PI_BIN)"

# ------------------------------------------------------------------ helpers
session_dir_for() {   # one session per spec, so repair keeps context
    local root="$1"
    local spec="$2"
    local d="$root/_tmp/pi-sessions/$(basename "$spec" .md)"
    mkdir -p "$d"; echo "$d"
}

run_gate() {
    local root="${1:-.}"
    for g in "$root/deploy/scripts/quality-gate.sh" \
             "$HOME/.pi/agent/skills/quality-gate/run.sh"; do
        [ -x "$g" ] && { bash "$g" "$root"; return $?; }
    done
    die "no quality gate found — cannot verify; add deploy/scripts/quality-gate.sh"
}

run_validate() {
    local spec="$1"
    local root="${2:-.}"
    for v in "$HOME/.pi/agent/skills/validate-exit-criteria/run.sh" \
             /usr/local/scripts/validate-exit-criteria.sh; do
        [ -x "$v" ] && { "$v" "$spec" "$root"; return $?; }
    done
    echo "validate-exit-criteria not found — EXIT CRITERIA UNVERIFIED" >&2
    return 127
}

# HANDOFF.md is the implementation diary. Enforced, not suggested: a task is
# not complete until it records what was done and what comes next.
check_handoff() {
    local root="${1:-.}"
    local h="$root/HANDOFF.md"
    [ -f "$h" ] || { echo "HANDOFF MISSING: $h does not exist" >&2; return 1; }
    if [ -n "$(find "$h" -mmin +240 2>/dev/null)" ]; then
        echo "HANDOFF STALE: $h not modified in the last 4 hours" >&2; return 1
    fi
    echo "HANDOFF OK: $h updated recently"
}

# -------------------------------------------------------------------- roles
case "$CMD" in
  implement|repair|review)
    SPEC="${1:-}"; [ -n "$SPEC" ] || die "spec path required"
    ROOT="${2:-$(cd "$(dirname "$SPEC")/.." && pwd)}"
    MODEL="${3:-${ORCH_MODEL:-}}"
    [ -f "$SPEC" ] || die "no such spec: $SPEC"
    [ -d "$ROOT" ] || die "no such project root: $ROOT"
    [ -f "$DIRECTIVES" ] || die "DIRECTIVES.md missing at $DIRECTIVES — bundle is incomplete"

    PROVIDER=""
    if [ "$MODEL" = "--escalate" ]; then
        MODEL="$ORCH_ESCALATION_MODEL"; PROVIDER="$ORCH_ESCALATION_PROVIDER"
        echo "pi-workhorse: escalating to $MODEL (${PROVIDER:-default provider})" >&2
    fi

    SD="$(session_dir_for "$ROOT" "$SPEC")"
    SPEC_REL="${SPEC#$ROOT/}"

    case "$CMD" in
      implement)
        TOOLS="read,write,edit,bash,grep,find,ls"; CONT=""
        TASK="Implement the spec at $SPEC_REL, following it exactly.

Write the tests from its TDD Contract FIRST, then the production code.
Verify by running the project quality gate; do not stop until it exits 0, or
until you hit something the spec gets wrong.

Then update HANDOFF.md: what you did, the outcome, and what comes next.

Reply in at most 25 lines: files changed, the gate's stage results, every
'test result:' line verbatim, and anything in the spec you found wrong."
        ;;
      repair)
        TOOLS="read,write,edit,bash,grep,find,ls"; CONT="-c"
        TASK="${ORCH_REPAIR_MSG:-The gate is still failing. Read the failure, fix the cause rather than the symptom, and re-run the gate. If the spec is what is wrong, say so instead of working around it. Reply in at most 15 lines.}"
        ;;
      review)
        # Read-only by tool grant: it cannot edit even if it decides to.
        TOOLS="read,grep,find,ls"; CONT=""
        TASK="Review the working-tree changes against the spec at $SPEC_REL.
You have READ-ONLY tools by design. Do not attempt to modify anything.

Read the load-bearing code, not just the diff stat: the parts where the spec's
judgement lives — error paths, ordering, persistence, wire formats, anything the
spec called out as a key decision. Check the ARCHITECTURE the spec describes is
actually what was built, not merely that tests pass.

Specifically: does every Requirement have a corresponding change; was any
existing test weakened, skipped or deleted; does each new check assert on what
the CONSUMER received rather than on what the producing function returned; any
hardcoded absolute paths; any regenerated fixture.

Reply in at most 20 lines: PASS or FAIL on line 1, then findings worst-first.
FAIL if any requirement is unimplemented or any test was weakened."
        ;;
    esac

    cd "$ROOT" || die "cannot cd to $ROOT"

    ARGS=(-p)
    [ -n "$CONT" ] && ARGS+=("$CONT")
    ARGS+=(--session-dir "$SD" --tools "$TOOLS")
    ARGS+=(--max-tool-iterations "${ORCH_MAX_TOOL_ITERATIONS:-60}")
    [ -n "$MODEL" ]    && ARGS+=(--model "$MODEL")
    [ -n "$PROVIDER" ] && ARGS+=(--provider "$PROVIDER")
    ARGS+=(--append-system-prompt "$(cat "$DIRECTIVES")")

    exec "$PI" "${ARGS[@]}" "$TASK"
    ;;

  gate)     run_gate "${1:-.}" ;;
  validate) [ -n "${1:-}" ] || die "spec path required"; run_validate "$1" "${2:-.}" ;;
  handoff)  check_handoff "${1:-.}" ;;
  *) die "unknown command: $CMD" ;;
esac

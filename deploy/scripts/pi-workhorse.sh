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
# Composed in precedence order: the global commandments first, then the
# project overrides. The overrides come LAST and say so explicitly, because
# commandment 1 of system.md tells the agent to git commit and this project
# forbids any workhorse from committing at all.
build_guardrails() {
    local root="$1"
    [ -f "$HOME/.pi/agent/system.md" ] && cat "$HOME/.pi/agent/system.md"
    cat <<'GUARD'

# PROJECT OVERRIDES — these WIN over anything above, including the numbered
# Global Workflow Commandments. Where they conflict, these are correct.

1. **NEVER run `git commit`, `git add`, `git stash`, `git checkout`, `git restore`
   or `git push`.** This OVERRIDES the "Pre-modification Commits" commandment
   above, which does not apply to you. Leave all work in the working tree. The
   orchestrator commits, only after reading the diff. Eleven commits once had to
   be rewritten because this was not enforced.
2. **NEVER modify any file under `specs/`.** Specs are the contract being
   implemented, not an artefact of the implementation. A workhorse that can edit
   the spec can make any failure vanish by rewriting the requirement — the same
   failure mode as editing a test to fit the code. If a spec looks wrong,
   incomplete or self-contradictory, STOP and say so in one line. Those reports
   are the most valuable thing you produce: five requirements in this project
   were withdrawn or corrected because a workhorse reported a bad premise
   instead of coding around it.
3. **NEVER touch `/projects/_upstream/`.** Read-only reference checkout on the
   user's own fork and active branch.
4. **NEVER `rm -rf` a `.pidag/` directory.** It is the only record of a run.
   Move it aside: `mv .pidag .pidag.prev-$(date +%H%M%S)`.
5. **Never regenerate a pinned test fixture**, and never run an `#[ignore]`d
   generator with `--ignored`. A fixture the suite can regenerate is not a
   fixture.
6. **Test artefacts go in `_tmp/`**, never `/tmp/`. Use paths relative to the
   project root or `env!("CARGO_MANIFEST_DIR")` — never a hardcoded absolute
   path, which breaks CI on a fresh checkout.
7. **Report raw output; never state a summed total.** Paste every
   `^test result:` line verbatim. The orchestrator does the arithmetic.
8. **Be terse.** Your reply is read by an orchestrator paying per token. No
   preamble, no restating the task, no narrating what you are about to do.
   Report: what changed, the gate result, and anything you found wrong.
GUARD
    if [ -f "$root/CLAUDE.md" ]; then
        echo
        echo "# PROJECT RULES (from $root/CLAUDE.md)"
        cat "$root/CLAUDE.md"
    fi
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

    exec "$PI" -p $CONT \
        --session-dir "$SD" \
        --model "$MODEL" \
        --tools "$TOOLS" \
        --max-tool-iterations "$MAX_ITER" \
        --append-system-prompt "$(build_guardrails "$ROOT")" \
        "$TASK"
    ;;

  gate)     run_gate "${1:-.}" ;;
  validate) run_validate "${1:?spec_path required}" "${2:-.}" ;;
  *) echo "unknown command: $CMD" >&2; exit 2 ;;
esac

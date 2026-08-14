#!/usr/bin/env bash
#
# install.sh — install the pi-orchestration bundle on a fresh machine.
#
#   ./install.sh [--orch-home DIR] [--claude-dir DIR] [--pi-skills DIR]
#
# Defaults:
#   bundle      -> ~/.pi-orchestration
#   claude skills -> ~/.claude/skills
#   pi skills     -> ~/.pi/agent/skills
#
# Idempotent. Existing files are backed up with a .bak-<timestamp> suffix
# rather than overwritten, because clobbering a customised skill silently is
# exactly the kind of thing this bundle exists to prevent.
#
set -euo pipefail
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORCH_HOME="$HOME/.pi-orchestration"
CLAUDE_SKILLS="$HOME/.claude/skills"
PI_SKILLS="$HOME/.pi/agent/skills"
STAMP="$(date +%Y%m%d-%H%M%S)"

while [ $# -gt 0 ]; do
  case "$1" in
    --orch-home)  ORCH_HOME="$2"; shift 2 ;;
    --claude-dir) CLAUDE_SKILLS="$2"; shift 2 ;;
    --pi-skills)  PI_SKILLS="$2"; shift 2 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

place() {  # place <src> <dst>
  local s="$1" d="$2"
  mkdir -p "$(dirname "$d")"
  if [ -e "$d" ] && ! cmp -s "$s" "$d"; then
    mv "$d" "$d.bak-$STAMP"
    echo "  backed up existing -> $(basename "$d").bak-$STAMP"
  fi
  cp "$s" "$d"
  echo "  $d"
}

echo "installing pi-orchestration"
echo "bundle -> $ORCH_HOME"
mkdir -p "$ORCH_HOME"
cp -r "$SRC/bin" "$SRC/pi" "$SRC/templates" "$ORCH_HOME/"
[ -e "$ORCH_HOME/config.env" ] && ! cmp -s "$SRC/config.env" "$ORCH_HOME/config.env" \
  && { mv "$ORCH_HOME/config.env" "$ORCH_HOME/config.env.bak-$STAMP"; echo "  kept your config.env as .bak-$STAMP"; }
cp "$SRC/config.env" "$ORCH_HOME/config.env"
chmod +x "$ORCH_HOME/bin/pi-workhorse.sh"

echo "claude skills -> $CLAUDE_SKILLS"
for d in "$SRC"/claude/skills/*/; do
  place "$d/SKILL.md" "$CLAUDE_SKILLS/$(basename "$d")/SKILL.md"
done

echo "pi skills -> $PI_SKILLS"
for d in "$SRC"/pi/skills/*/; do
  place "$d/SKILL.md" "$PI_SKILLS/$(basename "$d")/SKILL.md"
done

echo
echo "done. Verify with:"
echo "  $ORCH_HOME/bin/pi-workhorse.sh gate <project_root>"
echo
echo "Add to your shell profile so the harness is on PATH:"
echo "  export PATH=\"\$PATH:$ORCH_HOME/bin\""
echo "  export ORCH_HOME=\"$ORCH_HOME\""
echo
echo "Escalation model is set in $ORCH_HOME/config.env (currently:"
echo "  $(grep -o 'ORCH_ESCALATION_MODEL:=[^}]*' "$ORCH_HOME/config.env" | cut -d= -f2))"

#!/usr/bin/env bash
#
# handoff-generator — Generate HANDOFF.md from template.
#
# Usage:
#   run.sh [<project_root>]
#
set -euo pipefail

SKILL_DIR="$(dirname "$0")"
TEMPLATE="$SKILL_DIR/templates/HANDOFF-template.md"
PROJECT_ROOT="${1:-.}"
HANDOFF_FILE="$PROJECT_ROOT/HANDOFF.md"
PROJECT_NAME=$(basename "$(cd "$PROJECT_ROOT" && pwd)")
TIMESTAMP=$(date "+%Y-%m-%d %H:%M")
SESSION_ID="run-$(date '+%Y%m%d-%H%M%S')-$$"

# Check if HANDOFF.md exists
if [ -f "$HANDOFF_FILE" ]; then
    # Update timestamp only
    sed -i "s/\*\*Last Updated\*\*:.*/\*\*Last Updated\*\*: $TIMESTAMP/" "$HANDOFF_FILE"
    echo "{\"updated\": \"$HANDOFF_FILE\", \"timestamp\": \"$TIMESTAMP\"}"
    exit 0
fi

# Create new HANDOFF.md from template
if [ -f "$TEMPLATE" ]; then
    sed -e "s/\[Project Name\]/$PROJECT_NAME/g" \
        -e "s/\[YYYY-MM-DD HH:MM\]/$TIMESTAMP/g" \
        -e "s/\[run-YYYYMMDD-HHMMSS-xxxxxx\]/$SESSION_ID/g" \
        "$TEMPLATE" > "$HANDOFF_FILE"
else
    # Fallback if template not found
    cat > "$HANDOFF_FILE" << EOF
# HANDOFF — ${PROJECT_NAME}

**Status**: YELLOW
**Last Updated**: ${TIMESTAMP}
**Session**: ${SESSION_ID}

## Current Phase

Phase 1: [Description]
- [ ] Step 1

## What Was Done

1. **[Task 1]**: [Brief description]

## Next Steps

1. [Immediate next action]

## Memory Keys

| Key | Description |
|-----|-------------|
| \`project/category/slug\` | [What it stores] |
EOF
fi

echo "{\"created\": \"$HANDOFF_FILE\", \"project\": \"$PROJECT_NAME\"}"

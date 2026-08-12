#!/usr/bin/env bash
#
# spec-generator — Generate new spec.md from template.
#
# Usage:
#   run.sh <feature_name> [<project_root>]
#
set -euo pipefail

SKILL_DIR="$(dirname "$0")"
TEMPLATE="$SKILL_DIR/templates/spec-template.md"
FEATURE_NAME="${1:-}"
PROJECT_ROOT="${2:-.}"

if [ -z "$FEATURE_NAME" ]; then
    echo '{"error": "feature_name required"}' >&2
    exit 1
fi

# Ensure specs directory exists
mkdir -p "$PROJECT_ROOT/specs"

# Find next spec number
LAST_NUM=$(ls "$PROJECT_ROOT/specs/" 2>/dev/null | grep -E '^[0-9]{2}-' | sort -r | head -1 | grep -oE '^[0-9]+' || echo "00")
NEXT_NUM=$(printf "%02d" $((10#$LAST_NUM + 1)))

SPEC_FILE="$PROJECT_ROOT/specs/${NEXT_NUM}-${FEATURE_NAME}.md"
ABS_PROJECT=$(cd "$PROJECT_ROOT" && pwd)

# Create from template
if [ -f "$TEMPLATE" ]; then
    sed -e "s/\[Feature Name\]/$FEATURE_NAME/g" \
        -e "s|\[absolute path to project root\]|$ABS_PROJECT|g" \
        -e "s/\[N\]/1/g" \
        "$TEMPLATE" > "$SPEC_FILE"
else
    # Fallback if template not found
    cat > "$SPEC_FILE" << EOF
# Spec: ${FEATURE_NAME}

## Overview

[One paragraph description of what this feature does and why it matters.]

**Project**: ${ABS_PROJECT}
**Phase**: 1 - [Phase title]

## Requirements

### Functional

1. **[Requirement Name]** - [Description of expected behavior]

### Non-Functional

1. [Constraint or quality attribute]

## Architecture

\`\`\`
[ASCII diagram or file tree showing structure]
\`\`\`

## TDD Contract

| Test Name | Input | Expected Output |
|-----------|-------|-----------------|
| test_name_1 | [input] | [expected output] |

## Exit Criteria

- [ ] \`cargo build --manifest-path ${ABS_PROJECT}/Cargo.toml\`
- [ ] \`cargo test --manifest-path ${ABS_PROJECT}/Cargo.toml\`

## Guardrails

- Do NOT [forbidden action]
EOF
fi

echo "{\"created\": \"$SPEC_FILE\", \"number\": \"$NEXT_NUM\", \"feature\": \"$FEATURE_NAME\"}"

#!/usr/bin/env bash
#
# skill-builder — Create and validate pi skills.
#
# Usage:
#   run.sh create <skill_name> [description]
#   run.sh validate <skill_path>
#   run.sh list
#
set -euo pipefail

SKILLS_DIR="/root/.pi/agent/skills"
COMMAND="${1:-}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required (create, validate, list)"}' >&2
    exit 1
fi

shift

case "$COMMAND" in
    create)
        SKILL_NAME="${1:-}"
        DESCRIPTION="${2:-A new pi skill.}"

        if [ -z "$SKILL_NAME" ]; then
            echo '{"error": "skill_name required"}' >&2
            exit 1
        fi

        # Validate name format
        if ! echo "$SKILL_NAME" | grep -qE '^[a-z][a-z0-9-]*$'; then
            echo '{"error": "skill_name must be lowercase with hyphens only"}' >&2
            exit 1
        fi

        SKILL_PATH="$SKILLS_DIR/$SKILL_NAME"

        if [ -d "$SKILL_PATH" ]; then
            echo "{\"error\": \"skill already exists: $SKILL_NAME\"}" >&2
            exit 1
        fi

        # Create structure
        mkdir -p "$SKILL_PATH"/{scripts,templates,resources}

        # Create SKILL.md
        cat > "$SKILL_PATH/SKILL.md" << EOF
---
name: $SKILL_NAME
description: >
  $DESCRIPTION
version: 1.0.0
allowed-tools: [bash]
---

# $SKILL_NAME

## Usage

\`\`\`bash
pi --skill $SKILL_NAME <command> [args...]
\`\`\`

## Commands

### example
Example command description.

\`\`\`bash
pi --skill $SKILL_NAME example arg1
\`\`\`

## Output

Returns JSON with command results.
EOF

        # Create run.sh
        cat > "$SKILL_PATH/run.sh" << 'RUNEOF'
#!/usr/bin/env bash
#
# SKILL_NAME_PLACEHOLDER — Skill description.
#
# Usage:
#   run.sh <command> [args...]
#
set -euo pipefail

COMMAND="${1:-}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required"}' >&2
    exit 1
fi

shift

case "$COMMAND" in
    example)
        echo '{"result": "example output"}'
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac
RUNEOF

        # Replace placeholder
        sed -i "s/SKILL_NAME_PLACEHOLDER/$SKILL_NAME/g" "$SKILL_PATH/run.sh"
        chmod +x "$SKILL_PATH/run.sh"

        echo "{\"created\": \"$SKILL_PATH\", \"files\": [\"SKILL.md\", \"run.sh\", \"scripts/\", \"templates/\", \"resources/\"]}"
        ;;

    validate)
        SKILL_PATH="${1:-}"

        if [ -z "$SKILL_PATH" ]; then
            echo '{"error": "skill_path required"}' >&2
            exit 1
        fi

        if [ ! -d "$SKILL_PATH" ]; then
            echo "{\"error\": \"not a directory: $SKILL_PATH\"}" >&2
            exit 1
        fi

        ERRORS=()
        WARNINGS=()

        # Check SKILL.md exists
        if [ ! -f "$SKILL_PATH/SKILL.md" ]; then
            ERRORS+=("SKILL.md missing")
        else
            # Check frontmatter
            if ! head -1 "$SKILL_PATH/SKILL.md" | grep -q "^---"; then
                ERRORS+=("SKILL.md missing frontmatter")
            fi

            # Check name field
            if ! grep -q "^name:" "$SKILL_PATH/SKILL.md"; then
                ERRORS+=("SKILL.md missing name field")
            fi

            # Check description field
            if ! grep -q "^description:" "$SKILL_PATH/SKILL.md"; then
                ERRORS+=("SKILL.md missing description field")
            fi

            # Check line count
            LINES=$(wc -l < "$SKILL_PATH/SKILL.md")
            if [ "$LINES" -gt 500 ]; then
                WARNINGS+=("SKILL.md exceeds 500 lines ($LINES)")
            fi
        fi

        # Check run.sh exists and is executable
        if [ ! -f "$SKILL_PATH/run.sh" ]; then
            ERRORS+=("run.sh missing")
        elif [ ! -x "$SKILL_PATH/run.sh" ]; then
            WARNINGS+=("run.sh not executable")
        fi

        # Output result
        if [ ${#ERRORS[@]} -eq 0 ]; then
            VALID="true"
        else
            VALID="false"
        fi

        # Build JSON array
        ERROR_JSON="[]"
        if [ ${#ERRORS[@]} -gt 0 ]; then
            ERROR_JSON=$(printf '%s\n' "${ERRORS[@]}" | jq -R . | jq -s .)
        fi

        WARN_JSON="[]"
        if [ ${#WARNINGS[@]} -gt 0 ]; then
            WARN_JSON=$(printf '%s\n' "${WARNINGS[@]}" | jq -R . | jq -s .)
        fi

        echo "{\"valid\": $VALID, \"errors\": $ERROR_JSON, \"warnings\": $WARN_JSON}"
        ;;

    list)
        echo '{"skills": ['
        FIRST=true
        for skill in "$SKILLS_DIR"/*/; do
            if [ -d "$skill" ]; then
                NAME=$(basename "$skill")
                DESC=""
                if [ -f "$skill/SKILL.md" ]; then
                    DESC=$(grep -A1 "^description:" "$skill/SKILL.md" 2>/dev/null | tail -1 | sed 's/^  //' | head -c 100)
                fi
                if [ "$FIRST" = true ]; then
                    FIRST=false
                else
                    echo ","
                fi
                echo -n "{\"name\": \"$NAME\", \"description\": \"$DESC\"}"
            fi
        done
        echo ']}'
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac

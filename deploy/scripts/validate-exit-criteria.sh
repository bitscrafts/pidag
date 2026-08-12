#!/usr/bin/env bash
#
# validate-exit-criteria.sh — Parse spec.md and validate exit criteria.
#
# Usage:
#   validate-exit-criteria.sh <spec_path> [<project_root>]
#
# The script reads the spec file, extracts exit criteria from the
# "## Exit Criteria" section, and runs each shell command (lines in backticks).
# Outputs PASS/FAIL count for SDD validation.
#
# Example:
#   validate-exit-criteria.sh /projects/my-app/specs/01-api.md /projects/my-app
#
set -uo pipefail

SPEC_PATH="${1:-}"
PROJECT_ROOT="${2:-.}"

if [ -z "$SPEC_PATH" ] || [ ! -f "$SPEC_PATH" ]; then
    echo "error: spec file not found: $SPEC_PATH" >&2
    echo "usage: validate-exit-criteria.sh <spec_path> [<project_root>]" >&2
    exit 1
fi

cd "$PROJECT_ROOT" || exit 1

PASS=0
FAIL=0
TOTAL=0

echo "=== Validating Exit Criteria ==="
echo "Spec: $SPEC_PATH"
echo "Project: $PROJECT_ROOT"
echo ""

# Extract exit criteria section and process each line
in_section=false
while IFS= read -r line; do
    # Detect start of Exit Criteria section
    if [[ "$line" =~ ^##[[:space:]]+Exit[[:space:]]+Criteria ]]; then
        in_section=true
        continue
    fi

    # Detect end of section (next ## heading)
    if $in_section && [[ "$line" =~ ^##[[:space:]] ]]; then
        break
    fi

    # Skip if not in section
    $in_section || continue

    # Skip empty lines and non-checkbox lines
    [[ "$line" =~ ^\-[[:space:]]\[[[:space:]x]\] ]] || continue

    ((TOTAL++))

    # Extract command from backticks (e.g., `cargo test ...`)
    if [[ "$line" =~ \`([^\`]+)\` ]]; then
        cmd="${BASH_REMATCH[1]}"
        printf "%-60s " "$cmd"

        # Run the command
        if eval "$cmd" > /dev/null 2>&1; then
            echo "PASS"
            ((PASS++))
        else
            echo "FAIL"
            ((FAIL++))
        fi
    else
        # Prose criterion — skip (requires manual validation)
        criterion="${line#*] }"
        printf "%-60s " "[PROSE] ${criterion:0:50}..."
        echo "SKIP"
    fi
done < "$SPEC_PATH"

echo ""
echo "=== Results ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "TOTAL: $TOTAL"

if [ "$FAIL" -gt 0 ]; then
    echo "EXIT CRITERIA: NOT MET"
    exit 1
fi

if [ "$PASS" -eq 0 ]; then
    echo "EXIT CRITERIA: NONE FOUND"
    exit 2
fi

echo "EXIT CRITERIA: ALL MET"
exit 0

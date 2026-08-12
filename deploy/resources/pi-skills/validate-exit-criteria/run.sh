#!/usr/bin/env bash
#
# validate-exit-criteria — Parse spec.md and validate exit criteria.
#
# Usage:
#   run.sh <spec_path> [<project_root>]
#
set -uo pipefail

SPEC_PATH="${1:-}"
PROJECT_ROOT="${2:-.}"

if [ -z "$SPEC_PATH" ] || [ ! -f "$SPEC_PATH" ]; then
    echo '{"error": "spec file not found", "path": "'"$SPEC_PATH"'"}' >&2
    exit 2
fi

cd "$PROJECT_ROOT" || exit 2

PASS=0
FAIL=0
TOTAL=0
CRITERIA_JSON="["

# Extract exit criteria section and process each line
in_section=false
first=true
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

    # Extract command from backticks
    if [[ "$line" =~ \`([^\`]+)\` ]]; then
        cmd="${BASH_REMATCH[1]}"

        # Run the command
        if output=$(bash -c "$cmd" 2>&1); then
            ((PASS++))
            status="true"
            error=""
        else
            ((FAIL++))
            status="false"
            error=$(echo "$output" | head -1 | sed 's/"/\\"/g')
        fi

        # Add to JSON array
        if ! $first; then
            CRITERIA_JSON+=","
        fi
        first=false

        cmd_escaped=$(echo "$cmd" | sed 's/"/\\"/g')
        if [ -z "$error" ]; then
            CRITERIA_JSON+='{"command":"'"$cmd_escaped"'","passed":'"$status"'}'
        else
            CRITERIA_JSON+='{"command":"'"$cmd_escaped"'","passed":'"$status"',"error":"'"$error"'"}'
        fi
    fi
done < "$SPEC_PATH"

CRITERIA_JSON+="]"

# Determine if all criteria met
if [ "$TOTAL" -eq 0 ]; then
    MET="false"
elif [ "$FAIL" -eq 0 ]; then
    MET="true"
else
    MET="false"
fi

# Output JSON result
cat <<EOF
{
  "pass": $PASS,
  "fail": $FAIL,
  "total": $TOTAL,
  "met": $MET,
  "criteria": $CRITERIA_JSON
}
EOF

# Exit code based on criteria met
if [ "$MET" = "true" ]; then
    exit 0
else
    exit 1
fi

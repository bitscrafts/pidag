#!/usr/bin/env bash
#
# quality-gate — Run Rust quality checks.
#
# Usage:
#   run.sh [<project_root>]
#
set -uo pipefail

PROJECT_ROOT="${1:-.}"
cd "$PROJECT_ROOT" || exit 1

PASSED=true

run_check() {
    local name="$1"
    shift
    local start_ms=$(date +%s%3N)

    if output=$("$@" 2>&1); then
        local end_ms=$(date +%s%3N)
        local duration=$((end_ms - start_ms))
        echo '{"passed":true,"duration_ms":'"$duration"'}'
    else
        local end_ms=$(date +%s%3N)
        local duration=$((end_ms - start_ms))
        PASSED=false
        local error=$(echo "$output" | head -3 | tr '\n' ' ' | sed 's/"/\\"/g')
        echo '{"passed":false,"duration_ms":'"$duration"',"error":"'"$error"'"}'
    fi
}

# Run checks
FMT_RESULT=$(run_check fmt cargo fmt --check 2>/dev/null || echo '{"passed":true,"duration_ms":0,"note":"no rustfmt"}')
CHECK_RESULT=$(run_check check cargo check 2>/dev/null || echo '{"passed":false,"error":"cargo check failed"}')
CLIPPY_RESULT=$(run_check clippy cargo clippy -- -D warnings 2>/dev/null || echo '{"passed":true,"duration_ms":0,"note":"no clippy"}')
TEST_RESULT=$(run_check test cargo test 2>/dev/null || echo '{"passed":false,"error":"cargo test failed"}')

# Output JSON
cat <<EOF
{
  "passed": $PASSED,
  "checks": {
    "fmt": $FMT_RESULT,
    "check": $CHECK_RESULT,
    "clippy": $CLIPPY_RESULT,
    "test": $TEST_RESULT
  }
}
EOF

if [ "$PASSED" = "true" ]; then
    exit 0
else
    exit 1
fi

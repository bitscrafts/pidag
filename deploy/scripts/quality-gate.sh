#!/usr/bin/env bash
#
# quality-gate.sh — Run quality checks on a Rust project.
#
# Usage:
#   quality-gate.sh [<project_root>]
#
# Runs: cargo fmt --check, cargo check, cargo clippy -D warnings, cargo test.
# Prints PASS/FAIL per check and exits non-zero if any FAIL.
#
# For standalone workspaces (like agent-memory), add --workspace flag.
#
set -uo pipefail

PROJECT_ROOT="${1:-.}"

if [ ! -d "$PROJECT_ROOT" ]; then
    echo "error: project root not found: $PROJECT_ROOT" >&2
    exit 1
fi

cd "$PROJECT_ROOT" || exit 1

PASS=0
FAIL=0

echo "=== Quality Gate ==="
echo "Project: $PROJECT_ROOT"
echo ""

run_check() {
    local label="$1"; shift
    printf '%-40s ' "$label"
    if "$@" > /dev/null 2>&1; then
        echo "PASS"
        ((PASS++))
    else
        echo "FAIL"
        ((FAIL++))
    fi
}

# Detect if this is a workspace (has workspace members in Cargo.toml)
if grep -q '^\[workspace\]' Cargo.toml 2>/dev/null; then
    WORKSPACE_FLAG="--workspace"
else
    WORKSPACE_FLAG=""
fi

run_check "cargo fmt --check"      cargo fmt --check $WORKSPACE_FLAG
run_check "cargo check"            cargo check $WORKSPACE_FLAG
run_check "cargo clippy -D warnings" cargo clippy $WORKSPACE_FLAG -- -D warnings
run_check "cargo test"             env PIDAG_REQUIRE_PI=1 cargo test -j 2 $WORKSPACE_FLAG

echo ""
echo "=== Results ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"

if [ "$FAIL" -gt 0 ]; then
    echo "QUALITY GATE: FAILED"
    exit 1
fi
echo "QUALITY GATE: PASSED"
exit 0

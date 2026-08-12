#!/usr/bin/env bash
#
# rust-specialist — Rust TDD implementation and quality checks.
#
# Usage:
#   run.sh <command> [args...]
#
# Commands:
#   implement <spec_path> [project_root]  - Implement with TDD
#   review [project_root]                 - Code review / quality gate
#   analyze [project_root]                - Deep codebase analysis
#
set -euo pipefail

COMMAND="${1:-}"
PROJECT_ROOT="${2:-.}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required (implement, review, analyze)"}' >&2
    exit 1
fi

shift

case "$COMMAND" in
    implement)
        SPEC_PATH="${1:-}"
        PROJECT_ROOT="${2:-.}"

        if [ -z "$SPEC_PATH" ]; then
            echo '{"error": "spec_path required"}' >&2
            exit 1
        fi

        # Run quality gate first to establish baseline
        echo '{"phase": "quality-gate-baseline"}'
        /root/.pi/agent/skills/quality-gate/run.sh "$PROJECT_ROOT" || true

        echo "{\"phase\": \"implementing\", \"spec\": \"$SPEC_PATH\"}"
        ;;

    review)
        PROJECT_ROOT="${1:-.}"
        /root/.pi/agent/skills/quality-gate/run.sh "$PROJECT_ROOT"
        ;;

    analyze)
        PROJECT_ROOT="${1:-.}"

        echo '{"phase": "analysis"}'

        # File count and sizes
        echo '{"step": "file-analysis"}'
        find "$PROJECT_ROOT/src" -name "*.rs" -type f 2>/dev/null | while read -r file; do
            lines=$(wc -l < "$file")
            if [ "$lines" -gt 450 ]; then
                echo "{\"warning\": \"large_file\", \"file\": \"$file\", \"lines\": $lines}"
            fi
        done

        # Check for unwraps
        echo '{"step": "unwrap-check"}'
        grep -rn "\.unwrap()" "$PROJECT_ROOT/src" 2>/dev/null | grep -v "_test" | head -20 || true

        # Run quality gate
        /root/.pi/agent/skills/quality-gate/run.sh "$PROJECT_ROOT"
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac

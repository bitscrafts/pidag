#!/usr/bin/env bash
#
# python-specialist — Python development using UV exclusively.
#
# Usage:
#   run.sh <command> [args...]
#
# Commands:
#   run <script.py>           - Run script via UV
#   add <package>             - Add dependency
#   init [project_name]       - Initialize UV project
#   sync                      - Sync dependencies
#   venv                      - Create/manage venv
#
set -euo pipefail

COMMAND="${1:-}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required (run, add, init, sync, venv)"}' >&2
    exit 1
fi

shift

case "$COMMAND" in
    run)
        if [ $# -eq 0 ]; then
            echo '{"error": "script or module required"}' >&2
            exit 1
        fi
        uv run "$@"
        ;;

    add)
        if [ $# -eq 0 ]; then
            echo '{"error": "package name required"}' >&2
            exit 1
        fi
        uv add "$@"
        echo "{\"added\": \"$*\"}"
        ;;

    init)
        PROJECT_NAME="${1:-}"
        if [ -n "$PROJECT_NAME" ]; then
            uv init "$PROJECT_NAME"
            echo "{\"created\": \"$PROJECT_NAME\"}"
        else
            uv init
            echo '{"created": "."}'
        fi
        ;;

    sync)
        uv sync
        echo '{"synced": true}'
        ;;

    venv)
        uv venv
        echo '{"venv": ".venv"}'
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac

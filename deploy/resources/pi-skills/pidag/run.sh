#!/usr/bin/env bash
#
# pidag skill — Execute pidag SDD orchestration commands.
#
# Usage:
#   run.sh <command> [args...]
#
# Commands:
#   sdd <spec> [--run] [--model <model>]  - Generate/execute SDD workflow
#   list                                   - List all runs
#   show <run-id>                          - Show run details
#   init                                   - Initialize pidag for project
#
set -euo pipefail

COMMAND="${1:-}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required (sdd, list, show, init)"}' >&2
    exit 1
fi

shift

case "$COMMAND" in
    sdd)
        # Pass all remaining args to pidag sdd
        if [ $# -eq 0 ]; then
            echo '{"error": "spec file required"}' >&2
            exit 1
        fi
        pidag sdd "$@"
        ;;

    list)
        pidag list "$@"
        ;;

    show)
        if [ $# -eq 0 ]; then
            echo '{"error": "run-id required"}' >&2
            exit 1
        fi
        pidag show "$@"
        ;;

    init)
        pidag attach
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac

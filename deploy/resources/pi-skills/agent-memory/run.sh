#!/usr/bin/env bash
#
# agent-memory — Interface to workspace persistent memory.
#
# Usage:
#   run.sh <command> [args...]
#
# Commands:
#   health                                    - Check service health
#   search <topic> <query> [k]                - Search memory
#   store <topic> <key> <content> [importance] - Store insight
#   store-global <key> <content> [importance]  - Store global insight
#
set -euo pipefail

# Use lnx IP for container-to-container communication
# host.containers.internal doesn't work reliably between separate podman networks
MEMORY_URL="${AGENT_MEMORY_URL:-http://${MEMORY_HOST}:7420}"
COMMAND="${1:-}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required (health, search, store, store-global)"}' >&2
    exit 1
fi

shift

case "$COMMAND" in
    health)
        # Test with a valid endpoint - search with empty query
        if curl -sf "$MEMORY_URL/v1/memory/workspace/search?q=health&k=1" > /dev/null 2>&1; then
            echo '{"status": "ok", "url": "'"$MEMORY_URL"'"}'
        else
            echo '{"status": "down", "url": "'"$MEMORY_URL"'"}' >&2
            exit 1
        fi
        ;;

    search)
        TOPIC="${1:-}"
        QUERY="${2:-}"
        K="${3:-5}"

        if [ -z "$TOPIC" ] || [ -z "$QUERY" ]; then
            echo '{"error": "topic and query required"}' >&2
            exit 1
        fi

        ENCODED_QUERY=$(echo -n "$QUERY" | jq -sRr @uri)
        curl -sf "$MEMORY_URL/v1/memory/$TOPIC/search?q=$ENCODED_QUERY&k=$K" | jq .
        ;;

    store)
        TOPIC="${1:-}"
        KEY="${2:-}"
        CONTENT="${3:-}"
        IMPORTANCE="${4:-0.7}"

        if [ -z "$TOPIC" ] || [ -z "$KEY" ] || [ -z "$CONTENT" ]; then
            echo '{"error": "topic, key, and content required"}' >&2
            exit 1
        fi

        curl -sf -X POST "$MEMORY_URL/v1/memory/$TOPIC/insights" \
            -H 'Content-Type: application/json' \
            -d "$(jq -n \
                --arg key "$KEY" \
                --arg content "$CONTENT" \
                --arg topic "$TOPIC" \
                --argjson importance "$IMPORTANCE" \
                '{
                    key: $key,
                    content: $content,
                    scope: {type: "topic", topic: $topic},
                    tags: ["pidag"],
                    importance: $importance
                }')" | jq .
        ;;

    store-global)
        KEY="${1:-}"
        CONTENT="${2:-}"
        IMPORTANCE="${3:-0.8}"

        if [ -z "$KEY" ] || [ -z "$CONTENT" ]; then
            echo '{"error": "key and content required"}' >&2
            exit 1
        fi

        curl -sf -X POST "$MEMORY_URL/v1/memory/workspace/insights" \
            -H 'Content-Type: application/json' \
            -d "$(jq -n \
                --arg key "$KEY" \
                --arg content "$CONTENT" \
                --argjson importance "$IMPORTANCE" \
                '{
                    key: $key,
                    content: $content,
                    scope: {type: "global"},
                    tags: ["pidag", "sdd"],
                    importance: $importance
                }')" | jq .
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac

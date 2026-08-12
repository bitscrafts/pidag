#!/usr/bin/env bash
#
# doc-organizer — SSOT documentation organization.
#
# Usage:
#   run.sh <command> [project_root]
#
# Commands:
#   discover   - Find all markdown files
#   analyze    - Analyze for duplicates
#   organize   - Create docs/ structure
#   index      - Generate INDEX.md and TOC.md
#
set -euo pipefail

COMMAND="${1:-}"
PROJECT_ROOT="${2:-.}"

if [ -z "$COMMAND" ]; then
    echo '{"error": "command required (discover, analyze, organize, index)"}' >&2
    exit 1
fi

case "$COMMAND" in
    discover)
        echo '{"phase": "discovery"}'
        find "$PROJECT_ROOT" -name "*.md" -type f \
            ! -path "*/.git/*" \
            ! -path "*/node_modules/*" \
            ! -path "*/_tmp/*" \
            ! -path "*/target/*" \
            2>/dev/null | while read -r file; do
            lines=$(wc -l < "$file" 2>/dev/null || echo 0)
            echo "{\"file\": \"$file\", \"lines\": $lines}"
        done
        ;;

    analyze)
        echo '{"phase": "analysis"}'

        # Find potential duplicates by heading
        echo '{"step": "heading-analysis"}'
        find "$PROJECT_ROOT" -name "*.md" -type f \
            ! -path "*/.git/*" \
            ! -path "*/node_modules/*" \
            2>/dev/null | xargs grep -h "^# " 2>/dev/null | sort | uniq -c | sort -rn | head -20

        # Large files
        echo '{"step": "large-files"}'
        find "$PROJECT_ROOT" -name "*.md" -type f \
            ! -path "*/.git/*" \
            2>/dev/null | while read -r file; do
            lines=$(wc -l < "$file")
            if [ "$lines" -gt 500 ]; then
                echo "{\"large_file\": \"$file\", \"lines\": $lines}"
            fi
        done
        ;;

    organize)
        DOCS_DIR="$PROJECT_ROOT/docs"
        echo "{\"phase\": \"organizing\", \"target\": \"$DOCS_DIR\"}"

        # Create structure
        mkdir -p "$DOCS_DIR"/{01-overview,02-guides,03-reference,04-development,05-operations,99-archive}

        # Create section READMEs if missing
        for dir in "$DOCS_DIR"/*/; do
            if [ ! -f "$dir/README.md" ]; then
                section=$(basename "$dir" | sed 's/^[0-9]*-//')
                echo "# ${section^}" > "$dir/README.md"
                echo "" >> "$dir/README.md"
                echo "Documentation for ${section}." >> "$dir/README.md"
            fi
        done

        echo '{"organized": true}'
        ;;

    index)
        DOCS_DIR="$PROJECT_ROOT/docs"

        if [ ! -d "$DOCS_DIR" ]; then
            echo '{"error": "docs/ directory not found, run organize first"}' >&2
            exit 1
        fi

        # Generate INDEX.md
        INDEX_FILE="$DOCS_DIR/INDEX.md"
        echo "# Documentation Index" > "$INDEX_FILE"
        echo "" >> "$INDEX_FILE"
        echo "Quick access to all documentation." >> "$INDEX_FILE"
        echo "" >> "$INDEX_FILE"
        echo "## Sections" >> "$INDEX_FILE"
        echo "" >> "$INDEX_FILE"

        for dir in "$DOCS_DIR"/*/; do
            section=$(basename "$dir")
            echo "- [$section]($section/README.md)" >> "$INDEX_FILE"
        done

        # Generate TOC.md
        TOC_FILE="$DOCS_DIR/TOC.md"
        echo "# Table of Contents" > "$TOC_FILE"
        echo "" >> "$TOC_FILE"

        find "$DOCS_DIR" -name "*.md" -type f | sort | while read -r file; do
            rel_path="${file#$DOCS_DIR/}"
            title=$(head -1 "$file" | sed 's/^# //')
            echo "- [$title]($rel_path)" >> "$TOC_FILE"
        done

        echo "{\"created\": [\"$INDEX_FILE\", \"$TOC_FILE\"]}"
        ;;

    *)
        echo "{\"error\": \"unknown command: $COMMAND\"}" >&2
        exit 1
        ;;
esac

#!/bin/bash
#
# entrypoint.sh — Container entrypoint for pidag-runner.
#
# Creates necessary symlinks after volume mounts and starts the main process.
#
set -e

# Create the .pi symlink in the mounted projects volume
# This provides project-local access to pi configuration
ln -sf /root/.pi /projects/.pi

# Create .local/bin for locally built binaries (overrides image binaries via PATH)
mkdir -p /projects/.local/bin

# Copy AGENTS.md if it doesn't exist in projects (first run)
if [ ! -f /projects/AGENTS.md ]; then
    cp /usr/local/share/AGENTS.md /projects/AGENTS.md 2>/dev/null || true
fi

# Copy pidag source to /projects/pidag for development (if not exists)
# Maintains structure: /projects/pidag/{Cargo.toml,src/,specs/,...}
if [ -d /opt/pidag-src/src ] && [ ! -d /projects/pidag/src ]; then
    echo "Copying pidag source to /projects/pidag..."
    mkdir -p /projects/pidag
    cp -r /opt/pidag-src/Cargo.toml /projects/pidag/
    cp -r /opt/pidag-src/src /projects/pidag/
    [ -d /opt/pidag-src/specs ] && cp -r /opt/pidag-src/specs /projects/pidag/
    [ -d /opt/pidag-src/tests ] && cp -r /opt/pidag-src/tests /projects/pidag/
    echo "pidag source available at /projects/pidag (cargo build ready)"
fi

# Execute the main command
exec "$@"

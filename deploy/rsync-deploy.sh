#!/usr/bin/env bash
#
# rsync-deploy.sh — Sync pidag and pi_agent_rust to lnx and build container.
#
# Usage:
#   ./rsync-deploy.sh [--no-build]
#
# This script:
# 1. Creates the remote directory structure
# 2. rsync's pidag source (with standalone Cargo.toml patch)
# 3. rsync's pi_agent_rust source
# 4. rsync's deploy files (Containerfile, compose, scripts)
# 5. Builds the container on lnx (unless --no-build)
#
set -euo pipefail

REMOTE_HOST="${DEPLOY_HOST}"
REMOTE_DIR="/podman/PROJECTS/pidag-container"
LOCAL_PIDAG="$(dirname "$0")/.."
LOCAL_PI="${WORKSPACE}/_upstream/pi_agent_rust"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

BUILD=true
if [[ "${1:-}" == "--no-build" ]]; then
    BUILD=false
fi

echo "=== pidag Container Deployment ==="
echo "Remote: $REMOTE_HOST:$REMOTE_DIR"
echo ""

# Create remote directory structure
echo "Creating remote directories..."
ssh "$REMOTE_HOST" "mkdir -p $REMOTE_DIR/{pidag,pi_agent_rust,scripts,projects}"

# Sync pidag source (excluding Cargo.toml which we'll patch)
echo ""
echo "Syncing pidag source..."
rsync -avz --delete \
    --exclude='target/' \
    --exclude='.git/' \
    --exclude='_tmp/' \
    --exclude='Cargo.toml' \
    "$LOCAL_PIDAG/" "$REMOTE_HOST:$REMOTE_DIR/pidag/"

# Generate standalone Cargo.toml (replace workspace inheritance with explicit values)
echo "Patching Cargo.toml for standalone build..."
sed -e 's/version\.workspace = true/version = "0.1.0"/' \
    -e 's/edition\.workspace = true/edition = "2024"/' \
    -e 's/authors\.workspace = true/authors = ["${DEPLOY_USER}"]/' \
    -e 's/license\.workspace = true/license = "MIT"/' \
    -e 's/repository\.workspace = true/repository = "https:\/\/github.com\/${DEPLOY_USER}\/on-research"/' \
    "$LOCAL_PIDAG/Cargo.toml" | ssh "$REMOTE_HOST" "cat > $REMOTE_DIR/pidag/Cargo.toml"

# Sync pi_agent_rust source
echo ""
echo "Syncing pi_agent_rust source..."
rsync -avz --delete \
    --exclude='target/' \
    --exclude='.git/' \
    --exclude='_tmp/' \
    --exclude='.beads/' \
    --exclude='examples/' \
    "$LOCAL_PI/" "$REMOTE_HOST:$REMOTE_DIR/pi_agent_rust/"

# Sync deploy files
echo ""
echo "Syncing deploy files..."
rsync -avz \
    "$SCRIPT_DIR/Containerfile" \
    "$SCRIPT_DIR/podman-compose.yml" \
    "$SCRIPT_DIR/Makefile" \
    "$REMOTE_HOST:$REMOTE_DIR/"

rsync -avz "$SCRIPT_DIR/scripts/" "$REMOTE_HOST:$REMOTE_DIR/scripts/"
rsync -avz "$SCRIPT_DIR/resources/" "$REMOTE_HOST:$REMOTE_DIR/resources/"

# Ensure scripts are executable
ssh "$REMOTE_HOST" "chmod +x $REMOTE_DIR/scripts/*.sh"

# Build container if requested
if $BUILD; then
    echo ""
    echo "Building container on lnx..."
    ssh "$REMOTE_HOST" "cd $REMOTE_DIR && podman-compose build"
    echo ""
    echo "=== Build complete ==="
    echo "Start with: ssh $REMOTE_HOST 'cd $REMOTE_DIR && podman-compose up -d'"
    echo "Web UI at: http://${DEPLOY_HOST_NAME}:4601"
else
    echo ""
    echo "=== Sync complete (no build) ==="
    echo "Build with: ssh $REMOTE_HOST 'cd $REMOTE_DIR && podman-compose build'"
fi

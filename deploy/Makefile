# pidag Container Deployment Makefile
#
# Targets:
#   build   - Build the container image on lnx
#   deploy  - rsync source to lnx and build
#   up      - Start the container
#   down    - Stop the container
#   logs    - View container logs
#   shell   - Open a shell in the container
#   run     - Run a DAG file
#   sdd     - Generate SDD DAG from a spec

REMOTE_HOST := ${DEPLOY_HOST}
REMOTE_DIR := /podman/PROJECTS/pidag-container
LOCAL_PIDAG := ..
LOCAL_PI := ${WORKSPACE}/_upstream/pi_agent_rust

.PHONY: help build deploy up down logs shell run sdd clean

help:
	@echo "pidag Container Deployment"
	@echo ""
	@echo "Targets:"
	@echo "  deploy  - Sync source to lnx and build container"
	@echo "  build   - Build container on lnx (after manual sync)"
	@echo "  up      - Start the container"
	@echo "  down    - Stop the container"
	@echo "  logs    - View container logs"
	@echo "  shell   - Open a shell in the container"
	@echo "  run     - Run a DAG: make run DAG=/projects/test/dag.json"
	@echo "  sdd     - Generate SDD: make sdd SPEC=/projects/test/specs/01-api.md"
	@echo "  clean   - Remove container and image"

# Sync source files to lnx and build
deploy:
	@echo "=== Syncing pidag source ==="
	rsync -avz --delete \
		--exclude='target/' \
		--exclude='.git/' \
		--exclude='_tmp/' \
		$(LOCAL_PIDAG)/ $(REMOTE_HOST):$(REMOTE_DIR)/pidag/
	@echo ""
	@echo "=== Syncing pi_agent_rust source ==="
	rsync -avz --delete \
		--exclude='target/' \
		--exclude='.git/' \
		--exclude='_tmp/' \
		--exclude='.beads/' \
		$(LOCAL_PI)/ $(REMOTE_HOST):$(REMOTE_DIR)/pi_agent_rust/
	@echo ""
	@echo "=== Syncing deploy files ==="
	rsync -avz \
		Containerfile \
		podman-compose.yml \
		scripts/ \
		$(REMOTE_HOST):$(REMOTE_DIR)/
	rsync -avz scripts/ $(REMOTE_HOST):$(REMOTE_DIR)/scripts/
	@echo ""
	@echo "=== Building container on lnx ==="
	ssh $(REMOTE_HOST) "cd $(REMOTE_DIR) && podman-compose build"
	@echo ""
	@echo "Deploy complete. Run 'make up' to start the container."

# Build on lnx (assumes source already synced)
build:
	ssh $(REMOTE_HOST) "cd $(REMOTE_DIR) && podman-compose build"

# Start container
up:
	ssh $(REMOTE_HOST) "cd $(REMOTE_DIR) && podman-compose up -d"
	@echo ""
	@echo "Container started. Web UI at http://${DEPLOY_HOST_NAME}:4601"

# Stop container
down:
	ssh $(REMOTE_HOST) "cd $(REMOTE_DIR) && podman-compose down"

# View logs
logs:
	ssh $(REMOTE_HOST) "cd $(REMOTE_DIR) && podman-compose logs -f"

# Open shell in container
shell:
	ssh -t $(REMOTE_HOST) "podman exec -it pidag-runner /bin/bash"

# Run a DAG file
run:
ifndef DAG
	@echo "Usage: make run DAG=/projects/path/to/dag.json"
	@exit 1
endif
	ssh $(REMOTE_HOST) "podman exec pidag-runner pidag run $(DAG)"

# Generate SDD from spec
sdd:
ifndef SPEC
	@echo "Usage: make sdd SPEC=/projects/path/to/spec.md"
	@exit 1
endif
	ssh $(REMOTE_HOST) "podman exec pidag-runner pidag sdd $(SPEC)"

# Clean up container and image
clean:
	ssh $(REMOTE_HOST) "cd $(REMOTE_DIR) && podman-compose down --rmi local"

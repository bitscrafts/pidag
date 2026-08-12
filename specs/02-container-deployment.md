# Spec: pidag Container Deployment — Isolated Orchestration on lnx

**Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
**Topic**: `claude-pi-delegation`
**Author**: Fable (Principal Architect)
**Date**: 2026-08-04
**Status**: APPROVED

---

## Overview

Deploy pidag + pi_agent_rust as an isolated container on lnx server. The container
provides a complete spec-to-implementation environment: open a `pi` instance,
describe an app, generate formatted spec → DAG → implementation (or multiple specs
in a `specs/` folder). Uses a `projects/` folder for isolation between projects.

**Why**: Running pidag in a container on lnx:
- Isolates the orchestration environment from the Mac development machine
- Uses lnx's resources (faster network to NVIDIA/Anthropic APIs)
- Enables unattended SDD runs while the Mac sleeps
- Provides reproducible builds via Rust slim image
- Matches the agent-memory container deployment pattern

---

## Requirements

### Container Build
- R1: Multi-stage Containerfile using `rust:1.88-slim` builder, `debian:bookworm-slim` runtime
- R2: Build both `pidag` (from crates/pidag) and `pi` (from pi_agent_rust upstream)
- R3: Include supporting scripts: `validate-exit-criteria.sh`, `quality-gate.sh`
- R4: Include pi skills directory (pi_agent_rust/skills if exists, or create minimal)
- R5: Runtime image < 200MB (Rust slim + binaries only, no toolchain)

### Runtime
- R6: Projects mounted at `/projects` (bind-mount from host)
- R7: Each project has structure: `specs/`, `.pidag/`, `src/` (or language-specific)
- R8: Web UI accessible at port 4601 (pidag ui)
- R9: Environment variables for API keys: `NVIDIA_API_KEY`, `ANTHROPIC_API_KEY`
- R10: agent-memory reachable at `http://host.containers.internal:7420`

### Deployment
- R11: rsync deployment script: Mac → lnx:/podman/PROJECTS/pidag-container/
- R12: podman-compose.yml for service definition
- R13: Makefile or scripts for common operations (build, deploy, run)

---

## Architecture

```mermaid
graph TB
    subgraph "Mac (Dev Machine)"
        M1[crates/pidag source]
        M2[pi_agent_rust source]
        M3[rsync-deploy.sh]
    end

    subgraph "lnx Server"
        subgraph "pidag Container"
            C1[pidag binary]
            C2[pi binary]
            C3[validate-exit-criteria.sh]
            C4[quality-gate.sh]
            C5[/projects mount]
        end

        D1[podman-compose]
        D2[agent-memory container]
    end

    M1 -->|rsync| D1
    M2 -->|rsync| D1
    C1 -->|spawns| C2
    C2 -->|LLM calls| E1[NVIDIA API]
    C2 -->|LLM calls| E2[Anthropic API]
    C1 -->|memory| D2
```

**Directory structure on lnx:**

```
/podman/PROJECTS/pidag-container/
├── Containerfile
├── podman-compose.yml
├── Makefile
├── pidag/                    # rsync'd from crates/pidag
│   └── ...
├── pi_agent_rust/            # rsync'd from _upstream/pi_agent_rust
│   └── ...
├── scripts/
│   ├── validate-exit-criteria.sh
│   └── quality-gate.sh
└── projects/                 # persistent project storage
    ├── project-a/
    │   ├── specs/
    │   │   └── 01-feature.md
    │   ├── .pidag/
    │   │   ├── config.toml
    │   │   └── pidag.redb
    │   └── src/
    └── project-b/
        └── ...
```

**Key decisions and rationale:**

- **Two-binary build**: Both `pidag` and `pi` compiled from source in the same
  builder stage. This ensures version compatibility and a single deploy artifact.

- **projects/ mount**: Projects persist across container restarts. Each project
  is self-contained with its own specs, vault, and source.

- **Skills minimal**: Initially ship with core skills only. Add more via bind-mount
  or volume as needed.

- **No SSH in container**: All interaction via `podman exec` or pidag web UI.
  Simpler security model.

---

## TDD Contract

Tests in `tests/container_deployment_tests.rs` (run on Mac, verify artifacts):

| Test name | Given | Expects |
|---|---|---|
| `test_containerfile_syntax` | Containerfile | `podman build --dry-run` succeeds |
| `test_compose_syntax` | podman-compose.yml | `podman-compose config` succeeds |
| `test_scripts_present` | scripts/ directory | validate-exit-criteria.sh, quality-gate.sh exist |
| `test_project_structure` | sample project | specs/, .pidag/, src/ directories created |

Integration tests (run inside container after deploy):

| Test name | Given | Expects |
|---|---|---|
| `test_pidag_runs` | `pidag --version` | exits 0, prints version |
| `test_pi_runs` | `pi --version` | exits 0, prints version |
| `test_agent_memory_reachable` | curl host.containers.internal:7420 | 200 or 404 (service exists) |
| `test_web_ui_serves` | curl localhost:4601 | 200, contains "pidag trace" |

---

## Exit Criteria

- [ ] `podman build -t pidag-runner .` succeeds on lnx
- [ ] `podman run --rm pidag-runner pidag --version` prints version
- [ ] `podman run --rm pidag-runner pi --version` prints version
- [ ] Image size < 200MB (`podman images pidag-runner --format "{{.Size}}"`)
- [ ] `podman-compose up -d` starts container with web UI on port 4601
- [ ] Web UI accessible from Mac at `http://${DEPLOY_HOST_NAME}:4601`
- [ ] Test project can run: `podman exec pidag-runner pidag run /projects/test/dag.json`
- [ ] validate-exit-criteria.sh works inside container
- [ ] agent-memory reachable from container via host.containers.internal

---

## Guardrails

- Do not include Rust toolchain in runtime image (builder only)
- Do not hardcode API keys in Containerfile (use env vars)
- Do not include .git directories in rsync
- Do not modify pi_agent_rust source (use as-is)
- Do not add network calls during container build (offline-friendly)
- Projects folder must be a bind-mount, not baked into image

---

## Files to Create

| File | Purpose |
|---|---|
| `deploy/Containerfile` | Multi-stage build for pidag + pi |
| `deploy/podman-compose.yml` | Service definition with ports, volumes, env |
| `deploy/Makefile` | build, deploy, run, shell targets |
| `deploy/rsync-deploy.sh` | Sync source to lnx, trigger rebuild |
| `deploy/scripts/validate-exit-criteria.sh` | Copy from skills or create |
| `deploy/scripts/quality-gate.sh` | Copy from skills or create |
| `deploy/projects/.gitkeep` | Placeholder for projects mount |
| `deploy/README.md` | Deployment instructions |

---

## Containerfile Skeleton

```dockerfile
# syntax=docker/dockerfile:1
# pidag + pi_agent_rust container for isolated SDD orchestration

# ---------------------------------------------------------------------------
# Stage 1: Build pidag and pi
# ---------------------------------------------------------------------------
FROM rust:1.88-slim AS builder

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev g++ && rm -rf /var/lib/apt/lists/*

# Copy pidag workspace
COPY pidag/Cargo.toml pidag/Cargo.lock ./pidag/
COPY pidag/src ./pidag/src/
COPY pidag/tests ./pidag/tests/

# Copy pi_agent_rust
COPY pi_agent_rust/Cargo.toml pi_agent_rust/Cargo.lock ./pi_agent_rust/
COPY pi_agent_rust/src ./pi_agent_rust/src/
COPY pi_agent_rust/build.rs ./pi_agent_rust/
COPY pi_agent_rust/docs ./pi_agent_rust/docs/
COPY pi_agent_rust/CHANGELOG.md ./pi_agent_rust/

# Build pidag
WORKDIR /build/pidag
RUN cargo build --release

# Build pi
WORKDIR /build/pi_agent_rust
RUN cargo build --release

# ---------------------------------------------------------------------------
# Stage 2: Runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 curl jq && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/pidag/target/release/pidag /usr/local/bin/
COPY --from=builder /build/pi_agent_rust/target/release/pi /usr/local/bin/
COPY scripts/ /usr/local/scripts/

RUN mkdir -p /projects
VOLUME ["/projects"]

ENV PATH="/usr/local/scripts:$PATH"
WORKDIR /projects

EXPOSE 4601

# Default: run pidag ui on all interfaces
CMD ["pidag", "ui", "--host", "0.0.0.0", "--port", "4601"]
```

---

## podman-compose.yml Skeleton

```yaml
version: "3.8"

services:
  pidag:
    build:
      context: .
      dockerfile: Containerfile
    image: pidag-runner:latest
    container_name: pidag-runner
    ports:
      - "4601:4601"
    volumes:
      - ./projects:/projects
    environment:
      - NVIDIA_API_KEY=${NVIDIA_API_KEY}
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
      - AGENT_MEMORY_URL=http://host.containers.internal:7420
    extra_hosts:
      - "host.containers.internal:host-gateway"
    restart: unless-stopped
```

---

## Workflow After Deployment

```bash
# 1. Create a new project
podman exec pidag-runner mkdir -p /projects/my-app/{specs,.pidag,src}

# 2. Write a spec (or use pi to generate one)
podman exec -it pidag-runner pi -p "write a spec for a REST API"

# 3. Generate SDD DAG from spec
podman exec pidag-runner pidag sdd /projects/my-app/specs/01-api.md

# 4. Run the DAG
podman exec pidag-runner pidag run /projects/my-app/.pidag/temp_sdd.json

# 5. Monitor via web UI
open http://${DEPLOY_HOST_NAME}:4601
```

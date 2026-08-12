# AGENTS.md — pidag SDD Orchestration Guide

Pi is the agent. Claude Code is NOT available in this container.

---

## CRITICAL: Memory + Commits are MANDATORY

**EVERY task MUST follow this protocol. No exceptions.**

### Task Start
```bash
# 1. Verify memory is up
pi --skill agent-memory health

# 2. Search for prior work (avoid re-deriving)
pi --skill agent-memory search <topic> "<what you're about to do>"

# 3. Commit current state before changes
git add -A && git commit -m "wip: starting <task-description>"
```

### Task End
```bash
# 1. Store findings in memory (MANDATORY - task NOT complete until this succeeds)
pi --skill agent-memory store <topic> <category>/<slug> "<finding with key details>" <importance>

# 2. Commit completed work
git add -A && git commit -m "feat(<topic>): <what was done>"
```

**A task is NOT complete until:**
1. `agent-memory store` returns success
2. `git commit` succeeds

**If memory is down:** Start it first. Never skip the memory update.

---

## Quick Start

```bash
# 1. Create spec
pi --skill spec-generator my-feature /projects/my-app

# 2. Generate DAG
pi --skill pidag sdd specs/01-my-feature.md

# 3. Execute
pi --skill pidag sdd specs/01-my-feature.md --run

# 4. Review at http://${DEPLOY_HOST_NAME}:4601

# 5. Store findings (MANDATORY)
pi --skill agent-memory store claude-pi-delegation fix/my-fix "Fixed issue X" 0.7
```

## Skills Reference

All skills in `/root/.pi/agent/skills/` (symlinked at `/projects/.pi/agent/skills/`).

| Skill | Purpose | Key Commands |
|-------|---------|--------------|
| `agent-memory` | **MANDATORY** memory storage | `health`, `search`, `store`, `store-global` |
| `pidag` | SDD orchestration | `sdd`, `list`, `show`, `init` |
| `rust-specialist` | Rust TDD + quality gate | `implement`, `review`, `analyze` |
| `python-specialist` | Python via UV | `run`, `add`, `init`, `sync` |
| `doc-organizer` | SSOT documentation | `discover`, `analyze`, `organize`, `index` |
| `skill-builder` | Create new skills | `create`, `validate`, `list` |
| `spec-generator` | Create numbered specs | `<feature> [project]` |
| `handoff-generator` | Session handoff | `[project]` |
| `validate-exit-criteria` | Check spec criteria | `<spec> [project]` |
| `quality-gate` | Rust quality checks | `[project]` |

## Memory Reference

See **CRITICAL** section at top. Memory commands:

```bash
# Health check
pi --skill agent-memory health

# Search (ALWAYS do this before starting work)
pi --skill agent-memory search <topic> "<query>"

# Store topic-scoped insight
pi --skill agent-memory store <topic> <category>/<slug> "<content>" <importance>

# Store global insight (cross-topic patterns)
pi --skill agent-memory store-global <key> "<content>" <importance>
```

### Key Naming Convention
- `<topic>/fix/<slug>` — bug fixes
- `<topic>/experiment/<slug>` — experimental results
- `<topic>/phase<N>/<slug>` — phase completions
- `workspace/specs/<feature>` — specs (global scope)

### Importance Scale
- `0.9` — Critical finding, architecture decision
- `0.7` — Standard finding, fix documentation
- `0.5` — Minor finding
- `0.3` — Uncertain, may be archived by decay

## Model Hierarchy

### Planners (free first)
- `nvidia:zhipuai/glm-5-plus` (GLM 5.2)
- `moonshotai:kimi-k3` (Kimi K3)

### Workers (fast)
- `deepseek:deepseek-chat` (default)
- `nvidia:deepseek-ai/deepseek-v3.2`
- `google:gemini-2.5-flash`

### Avoid
- gpt-3.5-turbo, gemini-1.0-pro, llama-2-*, mistral-7b

## Directory Structure

```
/projects/<project>/
├── .pidag/              # pidag database (auto-created)
├── _tmp/                # Test artifacts (MANDATORY for tests)
├── specs/               # Numbered specs (01-, 02-)
├── src/                 # Implementation
├── target/              # Rust build output (gitignored)
├── HANDOFF.md           # Session continuity
└── AGENTS.md            # Project-specific (optional)
```

## Test Artifacts (_tmp/) - MANDATORY

**ALL test artifacts MUST go in `_tmp/` folder. No exceptions.**

```bash
# CORRECT: Use _tmp/ for test files
mkdir -p _tmp/test-workspace
cargo test  # tests should write to _tmp/

# WRONG: Never use /tmp or system directories
# /tmp/test-file  <- FORBIDDEN
```

Rules:
- Tests creating files/directories MUST use `_tmp/` as base
- Never write to `/tmp/` or other system directories
- `_tmp/` is gitignored - safe to delete between runs
- Integration tests use `_tmp/` for workspaces and outputs

## Spec Format

```markdown
# Spec: [Feature Name]

## Overview
**Project**: /projects/<name>
**Phase**: N - [title]

## Requirements
### Functional
### Non-Functional

## Architecture

## TDD Contract
| Test Name | Input | Expected Output |

## Exit Criteria
- [ ] `shell command returning 0`

## Guardrails
- Do NOT [forbidden]
```

## Python via UV

**Always use UV. Never pip/conda.**

```bash
pi --skill python-specialist run script.py
pi --skill python-specialist add requests
```

## Configuration

Settings: `/projects/.pi/agent/settings.json`
Models: `/projects/.pi/agent/models.json`

## API Keys

Set in container `.env`:
- `DEEPSEEK_API_KEY`
- `NVIDIA_API_KEY`
- `GEMINI_API_KEY`
- `OPENAI_API_KEY`
- `KIMI_API_KEY`

## Web UI

http://${DEPLOY_HOST_NAME}:4601

---

## Self-Upgrading pidag (CRITICAL)

**NEVER overwrite the running pidag binary directly!**

The pidag binary at `/usr/local/bin/pidag` is running the UI and orchestrator.
Overwriting it mid-run can corrupt state or lose access to the orchestrator.

### Local Binary Override (`/projects/.local/bin/`)

**Preferred method** - binaries in `/projects/.local/bin/` override image binaries:

```bash
# Build and install to local bin (no container restart needed!)
cd /projects/pidag
cargo build --release
cp target/release/pidag /projects/.local/bin/

# Verify (should use local version)
which pidag  # /projects/.local/bin/pidag
pidag --version
```

This works because `/projects/.local/bin` is FIRST in PATH.

### Alternative: Direct Binary Swap

For replacing the image binary (requires container restart):

```bash
# 1. Build to staging
cd /projects/pidag
cargo build --release
cp target/release/pidag /tmp/pidag-staged

# 2. Test staged binary
/tmp/pidag-staged --version

# 3. From HOST - stop, swap, restart
ssh lnx "cd /podman/PROJECTS/pidag-container && podman-compose down"
ssh lnx "podman cp /tmp/pidag-staged pidag-runner:/usr/local/bin/pidag"
ssh lnx "cd /podman/PROJECTS/pidag-container && podman-compose up -d"
```

### Rollback

```bash
# Remove local override (falls back to image binary)
rm /projects/.local/bin/pidag

# Or restore backup
cp /usr/local/bin/pidag.backup /usr/local/bin/pidag
```

### Development Workflow (recommended)

1. Edit source in `/projects/pidag/src/`
2. Build: `cargo build --release`
3. Install to local: `cp target/release/pidag /projects/.local/bin/`
4. Test immediately (no restart needed)

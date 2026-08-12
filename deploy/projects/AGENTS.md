# AGENTS.md — pidag SDD Orchestration Guide

This document describes the Spec-Driven Development (SDD) workflow using
pidag and pi (pi_agent_rust) for projects running in this container.

**Key Design**: Pi is the agent. Claude Code is NOT available in this container.

## Workflow Overview

```
Spec → DAG → Implementation → Validation → Commit
```

1. **Write Spec**: Create `specs/<NN>-<feature>.md` using spec-generator skill
2. **Generate DAG**: `pidag sdd specs/<NN>-<feature>.md`
3. **Execute DAG**: `pidag sdd specs/<NN>-<feature>.md --run`
4. **Review Results**: Check HANDOFF.md and pidag UI at http://${DEPLOY_HOST_NAME}:4601
5. **Commit Changes**: `git add && git commit`

## Directory Structure

```
/projects/<project-name>/
├── .pidag/
│   ├── config.toml       # pidag configuration
│   └── pidag.redb          # Run history database
├── specs/
│   └── 01-<feature>.md   # Spec files (numbered, use 01-, 02-, etc.)
├── src/                   # Implementation code
├── HANDOFF.md            # Session handoff document
└── AGENTS.md             # Project-specific agents file
```

## Pi Configuration

Pi configuration lives in `/root/.pi/agent/` with symlink at `/projects/.pi`:

```
/root/.pi/agent/
├── settings.json         # Provider, model, tools
├── models.json           # Planner/worker model hierarchy
└── skills/               # Pi skills
    ├── pidag/
    ├── validate-exit-criteria/
    ├── quality-gate/
    ├── spec-generator/
    └── handoff-generator/
```

**Edit settings directly**: `nvim /projects/.pi/agent/settings.json`

## Model Hierarchy

### Planners (try free first)
Used for architecture and planning tasks:
- `nvidia:zhipuai/glm-5-plus` (GLM 5.2)
- `moonshotai:kimi-k3` (Kimi K3)

### Workers (fast implementation)
Used for code generation:
- `nvidia:deepseek-ai/deepseek-v3.2` (DeepSeek V4 flash)
- `google:gemini-2.5-flash` (Gemini 3.6 flash)
- `deepseek:deepseek-chat` (Direct DeepSeek API)

### Avoid Older Models
- gpt-3.5-turbo
- gemini-1.0-pro
- llama-2-*
- mistral-7b

## Pi Skills

### pidag
Execute pidag SDD orchestration commands:
```bash
/root/.pi/agent/skills/pidag/run.sh sdd specs/01-feature.md       # Generate DAG
/root/.pi/agent/skills/pidag/run.sh sdd specs/01-feature.md --run # Execute
/root/.pi/agent/skills/pidag/run.sh list                          # List runs
/root/.pi/agent/skills/pidag/run.sh show <run-id>                 # Show run
/root/.pi/agent/skills/pidag/run.sh init                          # Initialize
```

### validate-exit-criteria
```bash
/root/.pi/agent/skills/validate-exit-criteria/run.sh <spec_path> [project_root]
```

### quality-gate
```bash
/root/.pi/agent/skills/quality-gate/run.sh [project_root]
```

### spec-generator
```bash
/root/.pi/agent/skills/spec-generator/run.sh <feature_name> [project_root]
```

### handoff-generator
```bash
/root/.pi/agent/skills/handoff-generator/run.sh [project_root]
```

## Project Configuration

### Initialize Project
```bash
cd /projects/<project-name>
pidag attach
```

### pidag Config (.pidag/config.toml)
```toml
[project]
root = "."

[worker]
default_model = "deepseek-chat"
timeout_secs = 300

[sdd]
max_iterations = 3
validate_script = "~/.claude/skills/loop-engineer/scripts/validate-exit-criteria.sh"
quality_gate_script = "~/.claude/skills/rust-specialist/scripts/quality-gate.sh"

[models]
free = ["deepseek-chat"]
paid = ["deepseek-chat"]
```

## Commands

### Generate Spec from Template
```bash
/root/.pi/agent/skills/spec-generator/run.sh my-feature /projects/my-app
# Creates: /projects/my-app/specs/01-my-feature.md
```

### Generate DAG from Spec
```bash
pidag sdd specs/01-feature.md
```

### Execute SDD Workflow
```bash
pidag sdd specs/01-feature.md --run
```

### Override Model
```bash
pidag sdd specs/01-feature.md --run --model deepseek-chat
```

### View Run Results
```bash
pidag show <run-id>
```

### List All Runs
```bash
pidag list
```

### Web UI
Access at http://${DEPLOY_HOST_NAME}:4601 (future: http://${DEPLOY_HOST_NAME}:4601/#/<project-name>)

## Python Scripts

**All Python scripts must run via UV**.

```bash
# CORRECT
uv run script.py
uv run python -m module

# WRONG
python script.py
```

### UV Commands
```bash
uv add package-name    # Add dependency
uv run script.py       # Run script
uv venv               # Create venv
uv sync               # Sync deps
```

## HANDOFF Protocol

After each session, update HANDOFF.md:
```bash
/root/.pi/agent/skills/handoff-generator/run.sh /projects/my-app
```

Contents:
- Current phase and status (GREEN/YELLOW/RED)
- What was done
- What failed / needs attention
- Key files modified
- Next steps
- Memory keys for agent-memory

## Spec File Format

Specs must be numbered (01-, 02-, etc.) and contain:

```markdown
# Spec: [Feature Name]

## Overview
[Description]
**Project**: /projects/<project-name>
**Phase**: N - [Phase title]

## Requirements
### Functional
### Non-Functional

## Architecture
[ASCII diagram]

## TDD Contract
| Test Name | Input | Expected Output |

## Exit Criteria
- [ ] `shell command that returns 0 on success`

## Guardrails
- Do NOT [forbidden]
```

## Troubleshooting

### Exit Criteria Not Found
Use checkbox format:
```markdown
- [ ] `command here`
```

### API Key Errors
Check environment: `env | grep API_KEY`

### LLM Node Fails
Test pi directly:
```bash
pi -p --model deepseek-chat "test"
```

### Timeout Issues
Increase timeout in `/projects/.pi/agent/settings.json`:
```json
{"requestTimeoutSecs": 600}
```

## API Keys

Configure in container environment or .env:
- `NVIDIA_API_KEY` — NVIDIA NIM models
- `GEMINI_API_KEY` — Google Gemini
- `OPENAI_API_KEY` — OpenAI
- `DEEPSEEK_API_KEY` — DeepSeek
- `KIMI_API_KEY` — Moonshot Kimi

## Future Enhancements

- **Project URLs**: http://${DEPLOY_HOST_NAME}:4601/#/hello-world (avoid port per project)
- **Cron Jobs**: Priority queue with round-robin for multiple projects
- **Remote Planning**: Pi session with project context for planning

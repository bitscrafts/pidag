---
name: agent-memory
description: >
  MANDATORY memory backend interface. Use for storing findings, searching prior work,
  and enforcing memory updates at task completion. Every task MUST store insights.
version: 1.0.0
allowed-tools: [bash]
---

# agent-memory

Interface to the workspace persistent memory backend.

## CRITICAL: Memory is MANDATORY

**Every task that produces a finding, benchmark, or decision MUST store it in agent-memory.**
A task is NOT complete until the store_insight call returns successfully.

## Usage

```bash
pi --skill agent-memory <command> [args...]
```

## Commands

### health
Check if agent-memory is running.

```bash
pi --skill agent-memory health
```

### search
Search memory before implementing (avoid re-deriving known results).

```bash
pi --skill agent-memory search <topic> <query> [k]
```

### store
Store an insight after task completion.

```bash
pi --skill agent-memory store <topic> <key> <content> [importance]
```

### store-global
Store a cross-topic insight (visible to all topics).

```bash
pi --skill agent-memory store-global <key> <content> [importance]
```

## Key Naming Convention

```
<topic-slug>/<category>/<identifier>
```

Examples:
- `claude-pi-delegation/fix/timeout-handling`
- `claude-pi-delegation/experiment/model-comparison`
- `workspace/specs/pidag-mcp-server`

## Standard Tags

`benchmark`, `paper`, `architecture`, `performance`, `rust`, `deployment`, `cross-topic`, `sdd`, `spec`

## Importance Scores

- **0.9**: Critical findings, production issues
- **0.8**: Specs, architectural decisions
- **0.7**: Implementation findings
- **0.5**: Experiments, observations
- **< 0.4**: Uncertain items (will decay)

## Endpoint

```
http://host.containers.internal:7420
```

Or via AGENT_MEMORY_URL environment variable.

## Task-End Checklist

1. Run `agent-memory health` to verify service is up
2. Search memory to avoid duplicates
3. Store insight with stable key, proper scope, tags, importance
4. For experiments: include key numbers in content
5. Only report task complete AFTER store succeeds

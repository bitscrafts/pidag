---
name: handoff-generator
description: >
  Generate or update HANDOFF.md for session continuity. Use at session end
  to document progress, issues, and next steps for the next agent.
version: 1.0.0
allowed-tools: [bash]
---

# handoff-generator

Generate or update HANDOFF.md for session continuity.

## Usage

```bash
pi --skill handoff-generator [project_root]
```

## Parameters

- `project_root`: Project root directory (default: current directory)

## Output

Creates or updates `HANDOFF.md` with:
- Current status (GREEN/YELLOW/RED)
- Last updated timestamp
- Current phase
- Completed work
- Pending issues
- Next steps

## Template Sections

1. **Status** - Overall health indicator
2. **Current Phase** - What phase we're in with checklist
3. **What Was Done** - Completed tasks
4. **What Failed** - Issues requiring attention
5. **Key Files Modified** - Changed files table
6. **Next Steps** - Immediate actions
7. **Memory Keys** - agent-memory references

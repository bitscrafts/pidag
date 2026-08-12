---
name: skill-builder
description: >
  Create new pi skills with proper folder structure. Generates SKILL.md with
  frontmatter, run.sh script, and optional scripts/, templates/, resources/ folders.
version: 1.0.0
allowed-tools: [bash]
---

# skill-builder

Create new pi skills with proper folder structure.

## Usage

```bash
pi --skill skill-builder create <skill_name> [description]
```

## Skill Folder Structure

```
skills/<skill-name>/
├── SKILL.md              # Required: frontmatter + instructions
├── run.sh                # Required: entry point script
├── scripts/              # Optional: helper scripts
│   └── *.sh
├── templates/            # Optional: file templates
│   └── *.md
├── resources/            # Optional: task-specific docs
│   └── *.md
└── assets/               # Optional: reusable outputs
    └── *
```

## Required Files

### SKILL.md
```markdown
---
name: skill-name
description: >
  Clear description stating what the skill does and when to use it.
  Max 1024 characters.
version: 1.0.0
allowed-tools: [bash]
---

# skill-name

Full instructions for the skill.
```

### run.sh
```bash
#!/usr/bin/env bash
set -euo pipefail

COMMAND="${1:-}"
# Handle commands...
```

## Frontmatter Rules

- `name`: Required. Lowercase with hyphens. Must match folder name.
- `description`: Required. Max 1024 chars. State what + when to use.
- `version`: Recommended. Semantic version.
- `allowed-tools`: Optional. Restrict available tools.

## Best Practices

1. Keep SKILL.md under 500 lines
2. Scripts should be self-contained with error messages
3. Use JSON output from scripts for machine parsing
4. Include usage examples
5. Document all commands and parameters

## Commands

### create
Create a new skill with all required files.

```bash
pi --skill skill-builder create my-new-skill "Does something useful"
```

### validate
Validate an existing skill structure.

```bash
pi --skill skill-builder validate <skill_path>
```

### list
List all available skills.

```bash
pi --skill skill-builder list
```

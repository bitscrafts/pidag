---
name: spec-generator
description: >
  Generate numbered spec.md files from template. Use when starting a new feature
  to create a properly formatted SDD specification with all required sections.
version: 1.0.0
allowed-tools: [bash]
---

# spec-generator

Generate a new spec.md file from template.

## Usage

```bash
pi --skill spec-generator <feature_name> [project_root]
```

## Parameters

- `feature_name`: Name of the feature (e.g., "user-auth", "api-endpoints")
- `project_root`: Project root directory (default: current directory)

## Output

Creates `specs/NN-<feature_name>.md` with:
- Auto-numbered prefix (01, 02, etc.)
- All required sections pre-filled with placeholders
- Project path set correctly

## Template Sections

1. **Overview** - What and why
2. **Requirements** - Functional and non-functional
3. **Architecture** - Structure diagram
4. **TDD Contract** - Test cases table
5. **Exit Criteria** - Shell commands in checkbox format
6. **Guardrails** - Forbidden actions

## Example

```bash
pi --skill spec-generator user-authentication /projects/my-app
# Creates: /projects/my-app/specs/01-user-authentication.md
```

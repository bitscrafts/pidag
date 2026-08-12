---
name: doc-organizer
description: >
  Organize documentation using SSOT (Single Source of Truth) methodology.
  Consolidates, deduplicates, and hierarchically structures markdown files.
version: 1.0.0
allowed-tools: [bash, read, write]
---

# doc-organizer

Organize project documentation using SSOT (Single Source of Truth) methodology.

## Usage

```bash
pi --skill doc-organizer <command> [args...]
```

## Commands

### discover
Find all markdown documentation in a project.

```bash
pi --skill doc-organizer discover [project_root]
```

### analyze
Analyze for duplicates and overlaps.

```bash
pi --skill doc-organizer analyze [project_root]
```

### organize
Create organized docs/ structure.

```bash
pi --skill doc-organizer organize [project_root]
```

### index
Generate master index and TOC.

```bash
pi --skill doc-organizer index [project_root]
```

## SSOT Principles

1. **Single Source of Truth** — Each info exists in ONE place
2. **Hierarchical Organization** — General to specific
3. **Cross-References** — Link, don't duplicate
4. **Preserve Originals** — Never delete README.md or CLAUDE.md

## Standard Hierarchy

```
docs/
├── INDEX.md              # Master index
├── TOC.md                # Table of contents
├── 01-overview/          # High-level docs
├── 02-guides/            # How-to guides
├── 03-reference/         # Technical reference
├── 04-development/       # Developer docs
├── 05-operations/        # Operational docs
└── 99-archive/           # Deprecated docs
```

## Output

Returns JSON status with files processed and actions taken.

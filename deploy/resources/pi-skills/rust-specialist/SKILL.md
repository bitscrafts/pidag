---
name: rust-specialist
description: >
  Rust architect implementing features with strict TDD (red-green-refactor) and
  quality gates. Use for implementing specs, code review, and codebase analysis.
version: 1.0.0
allowed-tools: [bash, read, edit, write]
---

# rust-specialist

Rust architect implementing features with strict TDD and quality gates.

## Usage

```bash
pi --skill rust-specialist <command> [args...]
```

## Commands

### implement
Implement a feature following TDD (test-first).

```bash
pi --skill rust-specialist implement <spec_path> [project_root]
```

### review
Run quality gate and code review.

```bash
pi --skill rust-specialist review [project_root]
```

### analyze
Deep codebase analysis with findings report.

```bash
pi --skill rust-specialist analyze [project_root]
```

## TDD Workflow

1. **RED**: Write failing test first (must fail initially)
2. **GREEN**: Implement minimum code to pass
3. **REFACTOR**: Clean up while keeping tests green
4. **GATE**: Run quality gate before completion

## Quality Gate Checks

- `cargo check` — Compilation
- `cargo clippy -- -D warnings` — Lints
- `cargo fmt --check` — Formatting
- `cargo test` — All tests pass
- File size limits (< 450 lines)
- No `.unwrap()` in production code

## Output Report

```
## rust-specialist report

Status: PASS | FAIL
Files changed: [list]
Tests written: [list]
Quality gate: PASSED | FAILED
Memory stored: [keys or "none"]
Unresolved: [issues or "none"]
```

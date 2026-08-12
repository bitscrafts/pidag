---
name: quality-gate
description: >
  Run Rust quality checks (fmt, check, clippy, test). Use before commits and
  at the end of TDD cycles to ensure code quality meets project standards.
version: 1.0.0
allowed-tools: [bash]
---

# quality-gate

Run Rust quality checks: cargo fmt, check, clippy, and test.

## Usage

```bash
pi --skill quality-gate [project_root]
```

## Parameters

- `project_root`: Project root directory (default: current directory)

## Checks Performed

1. `cargo fmt --check` - Format validation
2. `cargo check` - Compilation check
3. `cargo clippy -- -D warnings` - Lint check
4. `cargo test` - Test suite

## Output

Returns JSON with check results:

```json
{
  "passed": true,
  "checks": {
    "fmt": {"passed": true, "duration_ms": 123},
    "check": {"passed": true, "duration_ms": 456},
    "clippy": {"passed": true, "duration_ms": 789},
    "test": {"passed": true, "duration_ms": 1234, "tests": 5}
  }
}
```

## Exit Codes

- 0: All checks passed
- 1: One or more checks failed

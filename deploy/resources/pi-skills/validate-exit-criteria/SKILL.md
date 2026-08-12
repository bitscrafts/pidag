---
name: validate-exit-criteria
description: >
  Validate spec exit criteria by running each shell command. Use to check if
  implementation satisfies all requirements defined in the spec file.
version: 1.0.0
allowed-tools: [bash]
---

# validate-exit-criteria

Validate exit criteria from a spec.md file by running each shell command.

## Usage

```bash
pi --skill validate-exit-criteria <spec_path> [project_root]
```

## Parameters

- `spec_path`: Path to the spec.md file containing Exit Criteria section
- `project_root`: Project root directory (default: current directory)

## Exit Criteria Format

The spec must have an `## Exit Criteria` section with checkbox items:

```markdown
## Exit Criteria

- [ ] `cargo build --manifest-path /projects/my-app/Cargo.toml`
- [ ] `cargo test --manifest-path /projects/my-app/Cargo.toml`
- [ ] `grep -q "pub fn my_function" /projects/my-app/src/lib.rs`
```

## Output

Returns JSON with pass/fail counts:

```json
{
  "pass": 3,
  "fail": 2,
  "total": 5,
  "met": false,
  "criteria": [
    {"command": "cargo build ...", "passed": true},
    {"command": "cargo test ...", "passed": false, "error": "test failed"}
  ]
}
```

## Exit Codes

- 0: All criteria met
- 1: One or more criteria failed
- 2: Spec file not found or invalid format

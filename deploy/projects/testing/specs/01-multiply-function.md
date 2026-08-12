# Spec: multiply-function

## Overview

Add a multiply function to the library that multiplies two integers and handles overflow gracefully using checked arithmetic.

**Project**: /projects/testing
**Phase**: 1 - Core Arithmetic

## Requirements

### Functional

1. **Multiply** - Function `multiply(a: i32, b: i32) -> Option<i32>` returns `Some(product)` or `None` on overflow
2. **Zero handling** - Multiplying by zero always returns `Some(0)`

### Non-Functional

1. No panics on overflow - use checked_mul
2. All public functions must have doc comments

## Architecture

```
src/
  lib.rs      # Add multiply() below existing add()
```

## TDD Contract

| Test Name | Input | Expected Output |
|-----------|-------|-----------------|
| test_multiply_positive | multiply(3, 4) | Some(12) |
| test_multiply_zero | multiply(5, 0) | Some(0) |
| test_multiply_negative | multiply(-2, 3) | Some(-6) |
| test_multiply_overflow | multiply(i32::MAX, 2) | None |

## Exit Criteria

- [ ] `cargo build --manifest-path /projects/testing/Cargo.toml`
- [ ] `cargo test --manifest-path /projects/testing/Cargo.toml`
- [ ] `cargo clippy --manifest-path /projects/testing/Cargo.toml -- -D warnings`
- [ ] `grep -q "pub fn multiply" /projects/testing/src/lib.rs`

## Guardrails

- Do NOT use unwrap() or expect() - return Option instead
- Do NOT modify the existing add() function
- Keep implementation minimal - no unnecessary abstractions

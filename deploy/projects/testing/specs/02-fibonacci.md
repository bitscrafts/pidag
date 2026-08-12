# Spec: fibonacci-function

## Overview

Implement a fibonacci function that calculates the nth Fibonacci number efficiently using iteration (not recursion) to avoid stack overflow.

**Project**: /projects/testing
**Phase**: 1 - Core Algorithm

## Requirements

### Functional

1. **Fibonacci** - Function `fib(n: u64) -> u64` returns the nth Fibonacci number
2. **Base cases** - fib(0) = 0, fib(1) = 1
3. **Iteration** - Must use iterative approach, not recursion

### Non-Functional

1. O(n) time complexity
2. O(1) space complexity (no arrays/vectors)
3. All public functions must have doc comments

## Architecture

```
src/
  lib.rs      # Add fib() function
```

## TDD Contract

| Test Name | Input | Expected Output |
|-----------|-------|-----------------|
| test_fib_zero | fib(0) | 0 |
| test_fib_one | fib(1) | 1 |
| test_fib_ten | fib(10) | 55 |
| test_fib_twenty | fib(20) | 6765 |

## Exit Criteria

- [ ] `cargo build --manifest-path /projects/testing/Cargo.toml`
- [ ] `cargo test --manifest-path /projects/testing/Cargo.toml`
- [ ] `cargo clippy --manifest-path /projects/testing/Cargo.toml -- -D warnings`
- [ ] `grep -q "pub fn fib" /projects/testing/src/lib.rs`

## Guardrails

- Do NOT use recursion
- Do NOT use Vec or arrays for storing intermediate values
- Do NOT use external crates
- Keep implementation minimal

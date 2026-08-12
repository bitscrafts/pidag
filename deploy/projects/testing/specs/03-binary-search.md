# Spec: binary-search

## Overview

Implement a binary search function that finds an element in a sorted slice and returns its index.

**Project**: /projects/testing
**Phase**: 2 - Search Algorithms

## Requirements

### Functional

1. **Binary Search** - Function `binary_search(arr: &[i32], target: i32) -> Option<usize>`
2. **Found** - Returns `Some(index)` if target exists in array
3. **Not Found** - Returns `None` if target does not exist
4. **Sorted Input** - Assumes input is sorted in ascending order

### Non-Functional

1. O(log n) time complexity
2. Must use iterative approach (not recursion)
3. Handle empty arrays gracefully

## Architecture

```
src/
  lib.rs      # Add binary_search() function below fib()
```

## TDD Contract

| Test Name | Input | Expected Output |
|-----------|-------|-----------------|
| test_bs_found_middle | binary_search(&[1,3,5,7,9], 5) | Some(2) |
| test_bs_found_first | binary_search(&[1,3,5,7,9], 1) | Some(0) |
| test_bs_found_last | binary_search(&[1,3,5,7,9], 9) | Some(4) |
| test_bs_not_found | binary_search(&[1,3,5,7,9], 4) | None |
| test_bs_empty | binary_search(&[], 5) | None |

## Exit Criteria

- [ ] `cargo build --manifest-path /projects/testing/Cargo.toml`
- [ ] `cargo test --manifest-path /projects/testing/Cargo.toml`
- [ ] `cargo clippy --manifest-path /projects/testing/Cargo.toml -- -D warnings`
- [ ] `grep -q "pub fn binary_search" /projects/testing/src/lib.rs`

## Guardrails

- Do NOT use recursion
- Do NOT use the standard library binary_search method
- Do NOT modify existing fib() function
- Handle integer overflow in midpoint calculation: use `left + (right - left) / 2`

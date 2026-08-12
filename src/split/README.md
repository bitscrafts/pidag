# split — Spec Splitting and Coverage Validation

Enables splitting large specs (>7 exit criteria, >10 tests, >5 files) into smaller,
manageable child specs while maintaining **100% coverage** of all original criteria.

## Critical Invariant

**NO exit criteria may be orphaned in the split.** Every criterion from the parent spec
must map to exactly one child spec. This is enforced by a coverage report
(`.pidag/split-coverage.json`) that is validated after every split.

## Module Overview

### mod.rs — Core Split Logic

Data structures and high-level split operations:

- **`ExitCriterion`** — Single criterion with index, text, shell-command flag, and mentioned modules
- **`parse_exit_criteria()`** — Extract all `- [ ]` items from spec's "## Exit Criteria" section
- **`parse_tdd_tests()`** — Extract test names from "## TDD Contract" section
- **`auto_determine_split_count()`** — Heuristic: target 5-7 criteria per child
- **`split_into_n_parts()`** — Allocate criteria indices to N groups using heuristics
- **`generate_child_spec_content()`** — Rewrite spec with only assigned criteria + metadata

### heuristics.rs — Grouping and Association

Heuristics for keeping related criteria together:

- **`extract_mentioned_modules()`** — Find `.rs` files and module names in criterion text
- **`find_tdd_tests()`** — Match TDD tests to criteria by function name
- **`group_by_module()`** — Assign criteria to parts, balancing by module membership

### coverage.rs — Report Generation and Validation

Persistence and validation of split coverage:

- **`SplitCoverage`** — Serializable report with parent, children, and coverage stats
- **`ChildCoverage`** — Single child's criteria indices and associated tests
- **`CoverageStats`** — Counters and orphaned-criteria list
- **`build_coverage_report()`** — Create report from child allocations
- **`write_coverage_json()`** — Atomic write to `.pidag/split-coverage.json`
- **`validate_coverage()`** — Read report and check for orphaned criteria

## Usage Example

```rust
use pidag::split::*;

// Parse parent spec
let spec = std::fs::read_to_string("specs/01-large-feature.md")?;
let criteria = parse_exit_criteria(&spec)?;
let tests = parse_tdd_tests(&spec)?;

// Determine split count
let n_parts = auto_determine_split_count(criteria.len());

// Allocate criteria to parts
let parts = split_into_n_parts(&criteria, &tests, n_parts)?;

// Generate child specs
for (part_num, indices) in parts.iter().enumerate() {
    let child_content = generate_child_spec_content(
        &spec,
        "01-large-feature",
        part_num + 1,
        parts.len(),
        indices,
        &criteria,
    )?;
    
    let child_path = format!("specs/{}", child_spec_filename("01-large-feature", part_num + 1));
    std::fs::write(&child_path, child_content)?;
}

// Build and validate coverage
let children = parts.iter().enumerate()
    .map(|(i, indices)| {
        (
            child_spec_filename("01-large-feature", i + 1),
            indices.clone(),
            Vec::new(),
        )
    })
    .collect::<Vec<_>>();

let coverage = build_coverage_report("specs/01-large-feature.md", &children, criteria.len());
write_coverage_json(&coverage, Path::new(".pidag/split-coverage.json"))?;

if !coverage.is_complete() {
    return Err("Coverage incomplete!".into());
}
```

## Design Principles

1. **Determinism** — Same input always produces same output (sort indices before heuristics)
2. **No Data Loss** — Every criterion appears in exactly one child
3. **Atomic Writes** — File updates use temp+rename pattern
4. **Heuristic Grouping** — Module/test matching keeps related criteria together
5. **Transparent Coverage** — JSON report enables external validation

## Integration with Queue

After splitting:

```bash
$ pidag split specs/01-big-feature.md --auto
Created:
  specs/01-big-feature-part1.md (5 criteria)
  specs/01-big-feature-part2.md (5 criteria)
  specs/01-big-feature-part3.md (5 criteria)

Coverage: 15/15 (100%)

$ pidag queue --run
Executing specs in order:
  [1/3] 01-big-feature-part1.md ... Done
  [2/3] 01-big-feature-part2.md ... Done
  [3/3] 01-big-feature-part3.md ... Done
```

The queue automatically detects split specs (via `.pidag/split-coverage.json`) and
skips the parent spec (since children cover all criteria).

## Testing

See `tests/split_tests.rs` for 12 TDD contract tests:

- Parsing exit criteria and tests
- Splitting into fixed/auto counts
- Coverage validation
- Metadata generation
- Determinism guarantees
- Heuristic grouping
- Error handling (empty children, orphaned criteria)

//! TDD tests for spec splitting and coverage validation (spec-07).
//!
//! 12 tests covering parsing, splitting, coverage validation, and metadata generation.

use pidag::split::*;
use std::fs;
use std::path::Path;

fn setup_test_dir() -> String {
    let dir = "_tmp/test-split";
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).unwrap();
    fs::create_dir_all(format!("{}/specs", dir)).unwrap();
    dir.to_string()
}

fn teardown_test_dir(dir: &str) {
    let _ = fs::remove_dir_all(dir);
}

// Test 1: Parse exit criteria from spec
#[test]
fn test_parse_exit_criteria() {
    let spec = r#"
## Exit Criteria

- [ ] First criterion
- [ ] Second criterion
- [ ] Third criterion
- [ ] Fourth criterion
- [ ] Fifth criterion

## Next Section
"#;

    let criteria = parse_exit_criteria(spec).unwrap();
    assert_eq!(criteria.len(), 5);
    assert_eq!(criteria[0].index, 0);
    assert_eq!(criteria[0].text, "First criterion");
    assert_eq!(criteria[4].text, "Fifth criterion");
}

// Test 2: Distinguish shell commands from prose
#[test]
fn test_parse_exit_criteria_shell_vs_prose() {
    let spec = r#"
## Exit Criteria

- [ ] `cargo test` passes
- [ ] Regular prose criterion
- [ ] `bash /root/script.sh .`
- [ ] Another text criterion

## Next Section
"#;

    let criteria = parse_exit_criteria(spec).unwrap();
    assert_eq!(criteria.len(), 4);
    assert!(criteria[0].is_shell_command);
    assert!(!criteria[1].is_shell_command);
    assert!(criteria[2].is_shell_command);
    assert!(!criteria[3].is_shell_command);
}

// Test 3: Split into N parts (fixed)
#[test]
fn test_split_into_n_parts() {
    let mut criteria = Vec::new();
    for i in 0..9 {
        criteria.push(ExitCriterion::new(i, format!("Criterion {}", i + 1)));
    }

    let parts = split_into_n_parts(&criteria, &[], 3).unwrap();
    assert_eq!(parts.len(), 3);
    // Each part should have 3 criteria (balanced)
    let total: usize = parts.iter().map(|p| p.len()).sum();
    assert_eq!(total, 9);
}

// Test 4: Auto-determine split count
#[test]
fn test_split_auto_determines_n() {
    // 15 criteria should split into ~2-3 parts
    let n = auto_determine_split_count(15);
    assert!(n >= 2 && n <= 3);

    // 6 criteria should not split
    assert_eq!(auto_determine_split_count(6), 1);

    // 18 criteria should split into ~3
    let n18 = auto_determine_split_count(18);
    assert!(n18 >= 2 && n18 <= 4);
}

// Test 5: Preserve all criteria in split
#[test]
fn test_split_preserves_all_criteria() {
    let test_dir = setup_test_dir();

    let spec = r#"
## Overview
Test spec with 10 criteria.

## Exit Criteria

- [ ] Criterion 1
- [ ] Criterion 2
- [ ] Criterion 3
- [ ] Criterion 4
- [ ] Criterion 5
- [ ] Criterion 6
- [ ] Criterion 7
- [ ] Criterion 8
- [ ] Criterion 9
- [ ] Criterion 10

## Guardrails
Keep parent spec unchanged.
"#;

    let criteria = parse_exit_criteria(spec).unwrap();
    assert_eq!(criteria.len(), 10);

    let parts = split_into_n_parts(&criteria, &[], 2).unwrap();
    // parts already contains indices (Vec<Vec<usize>>)
    let child_specs: Vec<(String, Vec<usize>, Vec<String>)> = vec![
        ("part1.md".to_string(), parts[0].clone(), vec![]),
        ("part2.md".to_string(), parts[1].clone(), vec![]),
    ];

    let coverage = build_coverage_report("parent.md", &child_specs, 10);

    assert_eq!(coverage.coverage.total_parent_criteria, 10);
    assert_eq!(coverage.coverage.covered_criteria, 10);
    assert!(coverage.coverage.orphaned_criteria.is_empty());
    assert!(coverage.is_complete());

    teardown_test_dir(&test_dir);
}

// Test 6: Validation fails on orphaned criteria
#[test]
fn test_split_fails_on_orphan() {
    let test_dir = setup_test_dir();

    // Create a coverage report with orphaned criteria
    let children = vec![("part1.md".to_string(), vec![0, 1], vec![])];

    let coverage = build_coverage_report("parent.md", &children, 5); // 5 total but only 2 covered
    assert!(!coverage.is_complete());
    assert_eq!(coverage.coverage.orphaned_criteria.len(), 3);

    teardown_test_dir(&test_dir);
}

// Test 7: Child spec includes parent metadata
#[test]
fn test_child_spec_metadata() {
    let parent_spec = r#"# Original Title

## Overview
Test overview.

## Exit Criteria

- [ ] First criterion

## Guardrails
Keep it clean.
"#;

    let criteria = parse_exit_criteria(parent_spec).unwrap();
    let indices = vec![0];

    let child_content = generate_child_spec_content(
        parent_spec,
        "01-test-spec",
        1,
        3,
        &indices,
        &criteria,
        std::slice::from_ref(&indices),
    )
    .unwrap();

    assert!(child_content.contains("**Parent-Spec**: `01-test-spec.md`"));
    assert!(child_content.contains("**Part**: 1 of 3"));
    assert!(child_content.contains("**Covers**: Exit criteria 1"));
}

// Test 8: Coverage report format (JSON)
#[test]
fn test_coverage_report_format() {
    let test_dir = setup_test_dir();

    let children = vec![
        (
            "part1.md".to_string(),
            vec![0, 1],
            vec!["test_foo".to_string()],
        ),
        (
            "part2.md".to_string(),
            vec![2, 3],
            vec!["test_bar".to_string()],
        ),
    ];

    let coverage = build_coverage_report("parent.md", &children, 4);

    // Write to file
    let coverage_path = Path::new(&test_dir).join("split-coverage.json");
    write_coverage_json(&coverage, &coverage_path).unwrap();

    // Verify file exists and contains expected JSON
    assert!(coverage_path.exists());
    let content = fs::read_to_string(&coverage_path).unwrap();
    assert!(content.contains("\"covered_criteria\""));
    assert!(content.contains("\"orphaned_criteria\""));
    assert!(content.contains("\"parent_spec\""));

    teardown_test_dir(&test_dir);
}

// Test 9: Splitting is deterministic
#[test]
fn test_split_deterministic() {
    let mut criteria = Vec::new();
    for i in 0..12 {
        criteria.push(ExitCriterion::new(i, format!("Criterion {}", i + 1)));
    }

    // Split twice with same input
    let parts1 = split_into_n_parts(&criteria, &[], 2).unwrap();
    let parts2 = split_into_n_parts(&criteria, &[], 2).unwrap();

    // Both splits should produce identical results
    assert_eq!(parts1.len(), parts2.len());
    for (p1, p2) in parts1.iter().zip(parts2.iter()) {
        assert_eq!(p1, p2);
    }
}

// Test 10: Abort if would create empty child
#[test]
fn test_split_aborts_empty_child() {
    let criteria = vec![
        ExitCriterion::new(0, "First".to_string()),
        ExitCriterion::new(1, "Second".to_string()),
    ];

    // Try to split 2 criteria into 5 parts (would create empty children)
    let result = split_into_n_parts(&criteria, &[], 5);
    assert!(result.is_err());
}

// Test 11: Module grouping heuristic
#[test]
fn test_module_grouping_heuristic() {
    let criteria = vec![
        ExitCriterion {
            index: 0,
            text: "Implement auth.rs module".to_string(),
            is_shell_command: false,
            mentioned_modules: vec!["auth.rs".to_string()],
        },
        ExitCriterion {
            index: 1,
            text: "Validate in auth.rs".to_string(),
            is_shell_command: false,
            mentioned_modules: vec!["auth.rs".to_string()],
        },
        ExitCriterion {
            index: 2,
            text: "Implement db.rs layer".to_string(),
            is_shell_command: false,
            mentioned_modules: vec!["db.rs".to_string()],
        },
    ];

    let groups = group_by_module(&criteria, 2).unwrap();
    assert_eq!(groups.len(), 2);

    // Criteria 0 and 1 (both auth.rs) should be together
    let part1 = &groups[0];
    let part2 = &groups[1];

    assert!(
        (part1.contains(&0) && part1.contains(&1)) || (part2.contains(&0) && part2.contains(&1))
    );
}

// Test 12: Test association heuristic
#[test]
fn test_test_association() {
    let criteria = vec![ExitCriterion::new(
        0,
        "Implement user_auth function for validation".to_string(),
    )];

    let tests = vec![
        "test_user_auth_create".to_string(),
        "test_user_auth_validate".to_string(),
        "test_other_module".to_string(),
    ];

    let related = find_tdd_tests(&criteria[0], &tests);

    // Should find tests mentioning "user_auth"
    assert!(related.len() >= 1);
    assert!(
        related.contains(&"test_user_auth_create".to_string())
            || related.contains(&"test_user_auth_validate".to_string())
    );
}

// Bonus: Test child spec filename generation
#[test]
fn test_child_spec_filename_generation() {
    assert_eq!(child_spec_filename("01-auth", 1), "01-auth-part1.md");
    assert_eq!(child_spec_filename("01-auth", 2), "01-auth-part2.md");
    assert_eq!(child_spec_filename("02-api", 1), "02-api-part1.md");
}

// Bonus: Test parsing TDD contract section
#[test]
fn test_parse_tdd_tests() {
    let spec = r#"
## TDD Contract

| Test name | Given | Expects |
|---|---|---|
| test_auth_create | User data | User object |
| test_auth_validate | Invalid token | Error |
| test_db_query | SQL string | Results |

## Exit Criteria
"#;

    let tests = parse_tdd_tests(spec).unwrap();
    assert!(!tests.is_empty());
    assert!(tests.iter().any(|t| t.contains("test_")));
}

// Regression: a child spec's TDD Contract must be filtered to only the tests
// relevant to that part's criteria, not the whole parent contract.
#[test]
fn test_child_tdd_contract_filtered() {
    let parent_spec = r#"# Auth

## Overview
Auth module.

## Requirements
- R1 auth

## TDD Contract

| Test name | Given | Expects |
|---|---|---|
| `test_auth_create` | user | id |
| `test_auth_validate` | token | ok |
| `test_db_query` | sql | rows |

## Exit Criteria

- [ ] `cargo test auth` — implement auth_create
- [ ] `cargo test db` — implement db_query

## Guardrails
Keep clean.
"#;

    let criteria = parse_exit_criteria(parent_spec).unwrap();
    assert_eq!(criteria.len(), 2);

    // Split into 2 parts. Criterion 0 (auth) vs criterion 1 (db).
    let parts = split_into_n_parts(&criteria, &[], 2).unwrap();

    // Build each child and inspect its TDD Contract.
    for (part_num, indices) in parts.iter().enumerate() {
        let child = generate_child_spec_content(
            parent_spec,
            "01-auth",
            part_num + 1,
            parts.len(),
            indices,
            &criteria,
            &parts,
        )
        .unwrap();

        // Extract the TDD Contract section from the child.
        let start = child
            .find("## TDD Contract")
            .expect("child has TDD section");
        let rest = &child[start..];
        let end = rest
            .find("## Exit Criteria")
            .expect("child has exit criteria");
        let tdd = &rest[..end];

        // The child must NOT contain every parent test — only its relevant ones.
        let has_auth_create = tdd.contains("test_auth_create");
        let has_db_query = tdd.contains("test_db_query");

        // Criterion index into this part determines which tests belong.
        let is_auth_part = indices.contains(&0);
        let is_db_part = indices.contains(&1);

        if is_auth_part {
            assert!(has_auth_create, "auth part keeps auth tests");
            assert!(!has_db_query, "auth part drops db tests");
        } else if is_db_part {
            assert!(has_db_query, "db part keeps db tests");
            assert!(!has_auth_create, "db part drops auth tests");
        }
    }
}
// names in backticks (e.g. `test_foo`), which the old parser missed because the
// pipe column no longer started with "test_".
#[test]
fn test_parse_tdd_tests_backticked() {
    let spec = r#"
## TDD Contract

| Test name | Given | Expects |
|---|---|---|
| `test_spec_state_serde_round_trip` | `SpecState::Pending` | deserializes to `Pending` |
| `test_carousel_interleave` | A[01,02], B[01,02] | A/01,B/01,A/02,B/02 |
| `test_carousel_bounded_batch` | batch 3 | A/01,B/01,A/02 |
| `test_state_merge_preserves_done` | cached Done | Done preserved |
| `test_discover_specs_ignores_unnumbered` | readme + 01-a | only 01-a |

## Exit Criteria

- [ ] `cargo test` passes
"#;

    let tests = parse_tdd_tests(spec).unwrap();
    assert_eq!(tests.len(), 5, "all 5 backticked test names parsed");
    assert!(tests.contains(&"test_spec_state_serde_round_trip".to_string()));
    assert!(tests.contains(&"test_carousel_interleave".to_string()));
    assert!(tests.contains(&"test_carousel_bounded_batch".to_string()));
    assert!(tests.contains(&"test_state_merge_preserves_done".to_string()));
    assert!(tests.contains(&"test_discover_specs_ignores_unnumbered".to_string()));
    // No duplicates.
    let mut uniq = tests.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), 5, "no duplicate test names");
}

// Enhancement 3: order-preserving split. Assigns criteria to parts as
// contiguous ranges (part1 = first chunk, part2 = next, ...) so that a
// criterion `k+1` depending on `k` lands in the same-or-later part, unlike the
// module-grouped split which can scatter criteria out of order.
#[test]
fn test_split_into_n_parts_ordered() {
    let mut criteria = Vec::new();
    for i in 0..8 {
        criteria.push(ExitCriterion::new(i, format!("Criterion {}", i + 1)));
    }

    let parts = split_into_n_parts_ordered(&criteria, 2).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0],
        vec![0, 1, 2, 3],
        "part1 is the first contiguous chunk"
    );
    assert_eq!(
        parts[1],
        vec![4, 5, 6, 7],
        "part2 is the next contiguous chunk"
    );

    // Order preserved: every part's indices are strictly increasing, and parts
    // cover all criteria exactly once.
    let mut seen: Vec<usize> = parts.iter().flatten().copied().collect();
    seen.sort();
    assert_eq!(
        seen,
        (0..8).collect::<Vec<usize>>(),
        "all criteria covered once"
    );

    // Uneven counts still produce non-empty, sequential parts.
    let parts3 = split_into_n_parts_ordered(&criteria, 3).unwrap();
    assert_eq!(parts3.len(), 3);
    assert_eq!(parts3[0], vec![0, 1, 2]);
    assert_eq!(parts3[1], vec![3, 4, 5]);
    assert_eq!(parts3[2], vec![6, 7]);

    // Too many parts -> error (would create empty children).
    assert!(split_into_n_parts_ordered(&criteria, 9).is_err());
}

// The ordered split keeps related tests with their criteria and must never
// produce an orphaned criterion.
#[test]
fn test_ordered_split_coverage_is_complete() {
    use pidag::split::*;
    let mut criteria = Vec::new();
    for i in 0..10 {
        criteria.push(ExitCriterion::new(i, format!("feature_{}_impl", i)));
    }

    let parts = split_into_n_parts_ordered(&criteria, 2).unwrap();
    assert_eq!(parts.len(), 2);

    let child_specs: Vec<(String, Vec<usize>, Vec<String>)> = parts
        .iter()
        .enumerate()
        .map(|(i, idx)| (format!("part{}.md", i + 1), idx.clone(), vec![]))
        .collect();

    let coverage = build_coverage_report("parent.md", &child_specs, 10);
    assert!(
        coverage.is_complete(),
        "ordered split must cover 100% of criteria"
    );
    assert!(coverage.coverage.orphaned_criteria.is_empty());
}

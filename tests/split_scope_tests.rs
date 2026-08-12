//! Audit U-2: `split` divided the checklist while duplicating the work.
//!
//! Measured 2026-08-12 on a 15-criteria spec: `--auto` produced three children,
//! and running part1 ALONE built all six files of the three-module system, all
//! tests passing, leaving parts 2 and 3 with nothing to do. Cause: Architecture
//! and Requirements were copied wholesale into every child, so each child still
//! described the entire system. Run all three and you pay three times the tokens
//! for one system; run one and the split served no purpose.

use pidag::split::{generate_child_spec_content, parse_exit_criteria, split_into_n_parts};

const PARENT: &str = r#"# Spec: three modules

## Overview
Three independent modules.

## Architecture
Three files: `alpha.py`, `beta.py`, `gamma.py`. One test file per module.

## Exit Criteria

- [ ] `test -f alpha.py`
- [ ] `test -f beta.py`
- [ ] `test -f gamma.py`
- [ ] `python3 -c "import alpha"`
- [ ] `python3 -c "import beta"`
- [ ] `python3 -c "import gamma"`
- [ ] `python3 -m unittest test_alpha`
- [ ] `python3 -m unittest test_beta`

## Guardrails
- Standard library only.
"#;

fn children() -> Vec<String> {
    let criteria = parse_exit_criteria(PARENT).expect("criteria parse");
    let groups = split_into_n_parts(&criteria, &[], 2).expect("split");
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            generate_child_spec_content(PARENT, "01-three", i + 1, 2, g, &criteria, &groups)
                .expect("child generated")
        })
        .collect()
}

/// U-2's core defect: a child must state what it owns. Without this the child
/// still described the whole system and the implementer built all of it.
#[test]
fn test_child_declares_its_own_scope() {
    for (i, c) in children().iter().enumerate() {
        assert!(
            c.contains("## Scope (Part"),
            "part{} has no Scope section; it still describes the whole system",
            i + 1
        );
    }
}

/// The Architecture is retained as context, so it must be explicitly marked as
/// wider than this part -- otherwise it reads as an instruction to build it all.
#[test]
fn test_architecture_is_marked_as_wider_than_the_part() {
    for c in children() {
        if c.contains("## Architecture") {
            assert!(
                c.contains("describes the FULL system"),
                "Architecture copied verbatim with no scope note -- this is exactly \
                 what made part1 build every module"
            );
        }
    }
}

/// The guardrail is what reaches the worker prompt, so the rule has to live there
/// and not only in a section the template may not forward.
#[test]
fn test_scope_rule_reaches_the_guardrails() {
    for c in children() {
        assert!(
            c.contains("Do NOT create artifacts owned by other parts"),
            "scope rule missing from Guardrails, which is the section the sdd \
             template forwards into the worker prompt"
        );
    }
}

/// Children are not interchangeable; ordering must be explicit.
#[test]
fn test_children_declare_ordering() {
    for (i, c) in children().iter().enumerate() {
        assert!(
            c.contains("**Depends-On-Parts**"),
            "part{} declares no ordering; nothing distinguishes run-me-first \
             from run-me-last",
            i + 1
        );
    }
}

/// The module extractor was Rust-only and treated a whole backtick span as a
/// module name. Exit Criteria are normally written as `` `test -f x.py` ``, so
/// on any non-Rust spec it produced either nothing or the entire shell command
/// — which is why the Scope section first listed commands instead of files, and
/// why `group_by_module` had nothing coherent to group by.
#[test]
fn test_module_extraction_is_language_agnostic() {
    use pidag::split::extract_mentioned_modules;

    let m = extract_mentioned_modules("`test -f logfilter.py`");
    assert_eq!(
        m,
        vec!["logfilter.py".to_string()],
        "should find the file inside the backticked command, not the command"
    );

    for (text, want) in [
        ("`test -f logparse.py`", "logparse.py"),
        ("Implement auth.rs module", "auth.rs"),
        ("update server.go and main.ts", "server.go"),
    ] {
        assert!(
            extract_mentioned_modules(text).contains(&want.to_string()),
            "{text:?} should yield {want:?}"
        );
    }
}

/// A whole shell command must never be reported as an artifact.
#[test]
fn test_scope_lists_files_not_commands() {
    for c in children() {
        if let Some(scope) = c.split("## Scope").nth(1) {
            let body = scope.split("## ").next().unwrap_or("");
            for line in body.lines().filter(|l| l.trim_start().starts_with("- `")) {
                assert!(
                    !line.contains("python3 ") && !line.contains("test -f"),
                    "Scope lists a shell command as an artifact: {line}"
                );
            }
        }
    }
}

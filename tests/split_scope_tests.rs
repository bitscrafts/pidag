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

// ---------------------------------------------------------------------------
// spec-40: narrow the Architecture section per split child.
//
// The Scope-plus-trailing-note design above (audit U-2) was honest and did not
// work: docs/FINDINGS.md records that running one child of a three-part split
// built the entire system anyway. A full-system Architecture read first forms
// the implementer's mental model, and a caveat at the end does not undo it.
// This spec reorders (Scope first) and attributes (inline, by part number)
// instead of filtering the Architecture -- filtering by keyword would drop
// design rationale that is genuine context even for the part that owns it.
//
// Deterministic 3-part fixture: part 1 owns `alpha.py`, part 2 owns
// `beta.py`, part 3 owns `gamma.py`. Explicit groups (not
// `split_into_n_parts`, whose module-grouping heuristic iterates a HashMap
// and is not order-stable) so each test knows exactly which part owns what.
// ---------------------------------------------------------------------------

const PARENT3: &str = r#"# Spec: three owned files, one orphan

## Architecture

Line one: `alpha.py` is the entry point.
Line two: `beta.py` handles validation.
Line three: `gamma.py` handles output.
Line four: `delta.py` is unowned scratch code that no criterion mentions.
Line five: pure prose about testing strategy, naming no file at all.

## Exit Criteria

- [ ] `test -f alpha.py`
- [ ] `test -f beta.py`
- [ ] `test -f gamma.py`

## Guardrails
- Standard library only.
"#;

/// This part's criteria, grouped by index: part 1 -> alpha.py (idx 0),
/// part 2 -> beta.py (idx 1), part 3 -> gamma.py (idx 2).
fn three_part_groups() -> Vec<Vec<usize>> {
    vec![vec![0], vec![1], vec![2]]
}

/// The child's Architecture section body, isolated from the heading itself
/// (which may carry a `(full system — ...)` suffix), the trailing `> **Note**`
/// paragraph the generator also appends, and from Scope/TDD/Exit Criteria --
/// so assertions about a specific line can't accidentally match those instead.
fn architecture_body(child: &str) -> &str {
    let after_heading = child
        .split("## Architecture")
        .nth(1)
        .expect("child has an Architecture section");
    let section = after_heading.split("\n## ").next().unwrap_or("");
    // Skip past the rest of the heading line itself (e.g. the
    // `(full system — context, not a build list)` suffix, or nothing).
    let body_start = section.find('\n').map(|i| i + 1).unwrap_or(0);
    let body = &section[body_start..];
    // Drop the generator's trailing "> **Note**: ..." paragraph, if present.
    match body.find("\n> **Note**") {
        Some(idx) => body[..idx].trim(),
        None => body.trim(),
    }
}

/// The child's Scope section body, isolated the same way.
fn scope_body(child: &str) -> &str {
    child
        .split("## Scope")
        .nth(1)
        .expect("child has a Scope section")
        .split("\n## ")
        .next()
        .unwrap_or("")
}

/// Remove every `**[Part N]**` / `**[Parts N, M]**` marker this generator
/// adds, leaving the line as the parent wrote it.
fn strip_markers(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(idx) = rest.find(" **[Part") {
        out.push_str(&rest[..idx]);
        rest = &rest[idx..];
        match rest.find("]**") {
            Some(end) => rest = &rest[end + 3..],
            None => break, // malformed marker; leave the rest untouched
        }
    }
    out.push_str(rest);
    out
}

/// S1 (Scope comes first): the reader must meet the narrow instruction before
/// the broad description, so `## Scope` must byte-precede `## Architecture`.
#[test]
fn test_scope_precedes_architecture() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let scope_at = child.find("## Scope").expect("Scope section present");
    let architecture_at = child
        .find("## Architecture")
        .expect("Architecture section present");
    assert!(
        scope_at < architecture_at,
        "Scope (offset {scope_at}) must precede Architecture (offset {architecture_at})"
    );
}

/// S2: a reader skimming headings must not mistake the Architecture for this
/// part's brief.
#[test]
fn test_architecture_heading_marks_context() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 2, 3, &groups[1], &criteria, &groups)
            .expect("child generated");

    assert!(
        child.contains("context, not a build list"),
        "Architecture heading must mark itself as context, not a build list: {child}"
    );
}

/// S3a, the acceptance test: attribution must land at the artifact, inline,
/// not only in a trailing note -- a trailing note already existed and already
/// failed (docs/FINDINGS.md).
#[test]
fn test_other_part_artifact_is_annotated_inline() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    // Part 1's child: `beta.py` is owned by part 2, a different part.
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let body = architecture_body(&child);
    let line = body
        .lines()
        .find(|l| l.contains("beta.py"))
        .expect("beta.py mentioned in Architecture");
    assert!(
        line.contains("**[Part 2]**"),
        "line naming another part's artifact must carry its attribution: {line:?}"
    );
}

/// S3b: this part's own artifact must not carry a marker -- it isn't "someone
/// else's", there's nothing to attribute.
#[test]
fn test_own_artifact_is_not_annotated() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let body = architecture_body(&child);
    let line = body
        .lines()
        .find(|l| l.contains("alpha.py"))
        .expect("alpha.py mentioned in Architecture");
    assert!(
        !line.contains("[Part"),
        "this part's own artifact must not be marked as someone else's: {line:?}"
    );
}

/// S4: "belongs to another part" is not actionable; the Scope section must
/// name each other part and what it owns.
#[test]
fn test_scope_names_other_parts_artifacts() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let body = scope_body(&child);
    assert!(
        body.contains("Part 2 owns:") && body.contains("beta.py"),
        "Scope must name part 2's artifact: {body}"
    );
    assert!(
        body.contains("Part 3 owns:") && body.contains("gamma.py"),
        "Scope must name part 3's artifact: {body}"
    );
}

/// S5: annotating in place means rewriting lines, and a line-rewriting bug
/// that silently drops content would be worse than the problem being fixed.
/// Every parent Architecture line must survive, markers aside.
#[test]
fn test_no_architecture_line_is_lost() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let original = PARENT3
        .split("## Architecture")
        .nth(1)
        .unwrap()
        .split("\n## ")
        .next()
        .unwrap()
        .trim();
    let reconstructed = strip_markers(architecture_body(&child));
    assert_eq!(
        reconstructed.trim(),
        original,
        "every Architecture line must survive annotation, unchanged apart from markers"
    );
}

/// S6: with a single part there is no other part to attribute to, so no
/// scaffolding is added -- the Architecture is copied exactly as it always
/// was.
#[test]
fn test_single_part_is_unannotated() {
    const PARENT_SINGLE: &str = r#"# Spec: single part

## Architecture

Only one file: `solo.py`. Nothing else to attribute.

## Exit Criteria

- [ ] `test -f solo.py`

## Guardrails
- Keep it minimal.
"#;

    let criteria = parse_exit_criteria(PARENT_SINGLE).expect("criteria parse");
    let groups = vec![vec![0]];
    let child = generate_child_spec_content(
        PARENT_SINGLE,
        "01-solo",
        1,
        1,
        &groups[0],
        &criteria,
        &groups,
    )
    .expect("child generated");

    assert!(
        !child.contains("[Part"),
        "a single-part split has no other part to attribute to: {child}"
    );
    assert!(
        !child.contains("context, not a build list"),
        "single-part split must not acquire split scaffolding (G7): {child}"
    );

    let original = PARENT_SINGLE
        .split("## Architecture")
        .nth(1)
        .unwrap()
        .split("\n## ")
        .next()
        .unwrap()
        .trim();
    let got = architecture_body(&child).trim();
    assert_eq!(
        got, original,
        "Architecture must be byte-identical to the parent's when total_parts == 1"
    );
}

/// S7a: an artifact no criterion mentions is a gap in the parent spec, not
/// something to arbitrate. Do not guess -- leave the line alone.
#[test]
fn test_unowned_artifact_is_not_annotated() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let body = architecture_body(&child);
    let line = body
        .lines()
        .find(|l| l.contains("delta.py"))
        .expect("delta.py mentioned in Architecture");
    assert!(
        !line.contains("[Part"),
        "an artifact nobody owns must be copied unchanged: {line:?}"
    );
    assert!(
        line.contains("Line four: `delta.py` is unowned scratch code that no criterion mentions."),
        "unowned-artifact line must be copied verbatim: {line:?}"
    );
}

/// S7b: a line naming no artifact at all must be copied unchanged.
#[test]
fn test_prose_line_is_not_annotated() {
    let criteria = parse_exit_criteria(PARENT3).expect("criteria parse");
    let groups = three_part_groups();
    let child =
        generate_child_spec_content(PARENT3, "01-three", 1, 3, &groups[0], &criteria, &groups)
            .expect("child generated");

    let body = architecture_body(&child);
    let line = body
        .lines()
        .find(|l| l.contains("pure prose"))
        .expect("prose-only line present");
    assert!(
        !line.contains("[Part"),
        "a prose line must stay unmarked: {line:?}"
    );
    assert_eq!(
        line.trim(),
        "Line five: pure prose about testing strategy, naming no file at all.",
        "prose line must be copied verbatim"
    );
}

//! Audit U-2: `split` divided the checklist while duplicating the work.
//!
//! Measured 2026-08-12 on a 15-criteria spec: `--auto` produced three children,
//! and running part1 ALONE built all six files of the three-module system, all
//! tests passing, leaving parts 2 and 3 with nothing to do. Cause: Architecture
//! and Requirements were copied wholesale into every child, so each child still
//! described the entire system. Run all three and you pay three times the tokens
//! for one system; run one and the split served no purpose.

use pidag::split::{generate_child_spec_content, parse_exit_criteria, split_into_n_parts};
use std::fs;
use std::path::Path;

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

// ---------------------------------------------------------------------------
// spec-41: `extract_section` loses a third of every spec.
//
// `extract_section` ended a section at `remaining.find("##")` -- a substring
// search, so a `### Functional` sub-heading (house style since spec-21)
// terminated a `## Requirements` section before its body even began, and the
// terminator matched inside fenced code blocks too. The start was also
// unanchored, so a `### Requirements` sub-heading or a prose mention of
// `` `## Architecture` `` could win over the real heading. Measured across
// specs/*.md: 37 sections extracted as empty despite having content, 39 more
// lost over 10%. `split` writes child specs from this output, so a child was
// handed a brief with its Requirements silently removed.
// ---------------------------------------------------------------------------

use pidag::split::extract_section;

/// The five headings `generate_child_spec_content` actually calls
/// `extract_section` with -- the complete real-usage surface, and the same
/// set spec-41's own measurement (37 empty / 39 truncated / 151 intact = 227)
/// is computed over.
const CANONICAL_SECTIONS: &[&str] = &[
    "## Overview",
    "## Requirements",
    "## Architecture",
    "## Guardrails",
    "## TDD Contract",
];

/// Ground truth for "does this heading have non-empty content", computed
/// independently of `extract_section` -- fence-aware, anchored to a
/// line-exact heading match, terminated at the next `^#{1,2}\s` heading
/// outside a fence. If this used `extract_section` itself the test would be
/// tautological: a bug that breaks extraction would also break the
/// "ground truth" the same way, and the two would always agree.
fn ground_truth_has_content(spec: &str, title: &str) -> bool {
    let mut in_fence = false;
    let mut fence_marker = "";
    let mut body: Option<Vec<&str>> = None;

    for line in spec.lines() {
        let trimmed = line.trim_start();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");

        if is_fence {
            if !in_fence {
                in_fence = true;
                fence_marker = &trimmed[..3];
            } else if trimmed.starts_with(fence_marker) {
                in_fence = false;
            }
            if let Some(b) = body.as_mut() {
                b.push(line);
            }
            continue;
        }

        if in_fence {
            if let Some(b) = body.as_mut() {
                b.push(line);
            }
            continue;
        }

        match &mut body {
            None => {
                if line.trim_end() == title {
                    body = Some(Vec::new());
                }
            }
            Some(b) => {
                let hashes = line.bytes().take_while(|&c| c == b'#').count();
                let is_heading = (1..=2).contains(&hashes)
                    && line
                        .as_bytes()
                        .get(hashes)
                        .is_some_and(u8::is_ascii_whitespace);
                if is_heading {
                    break;
                }
                b.push(line);
            }
        }
    }

    body.map(|b| !b.join("\n").trim().is_empty())
        .unwrap_or(false)
}

/// E6, the repo-wide regression guard: for every real spec in this repo, for
/// every canonical section heading present with non-empty content, extraction
/// must return non-empty. This is the check that would have caught the
/// defect -- the fixture-only unit tests passed throughout because their
/// fixtures are simple specs with no `###` sub-headings, the one input shape
/// where the broken implementation happened to work.
///
/// Must fail on the current (pre-fix) implementation, reporting a count in
/// the dozens (spec-41 measured 37).
#[test]
fn test_no_spec_in_repo_extracts_empty() {
    let specs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("specs");
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    let mut paths: Vec<_> = fs::read_dir(&specs_dir)
        .expect("specs/ directory readable")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no specs/*.md found at {}; E6 has nothing to check",
        specs_dir.display()
    );

    for path in &paths {
        let spec = fs::read_to_string(path).expect("spec readable");
        for &title in CANONICAL_SECTIONS {
            let present = spec.lines().any(|l| l.trim_end() == title);
            if !present {
                continue;
            }
            if !ground_truth_has_content(&spec, title) {
                continue; // E5: legitimately empty (heading immediately followed by another)
            }
            checked += 1;
            let got = extract_section(&spec, title);
            if got.trim().is_empty() {
                failures.push(format!(
                    "{}: {title:?} has content but extract_section returned empty",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} sections with real content extracted empty:\n{}",
        failures.len(),
        checked,
        failures.join("\n")
    );
}

/// E1a: a prose mention of the heading text (even inline-code-quoted) before
/// the real heading must not be mistaken for the start of the section.
#[test]
fn test_prose_mention_does_not_start_a_section() {
    let spec = r#"# Spec

## Overview

This section describes the emission order and mentions `## Architecture`
in a bullet, purely as prose:

- the code emits the `## Architecture` marker before Requirements

## Architecture

Real architecture content here.

## Guardrails
"#;
    let got = extract_section(spec, "## Architecture");
    assert_eq!(got, "Real architecture content here.");
}

/// E1b: a `### Requirements` sub-heading appearing before the real
/// `## Requirements` heading must not win.
#[test]
fn test_sub_heading_does_not_start_a_section() {
    let spec = r#"# Spec

## Overview

See ### Requirements below for the sub-heading style used elsewhere.

## Requirements

The real requirements body.

## Guardrails
"#;
    let got = extract_section(spec, "## Requirements");
    assert_eq!(got, "The real requirements body.");
}

/// E2a, the acceptance test: `## Requirements` followed by `### Functional`
/// and `### Non-Functional` sub-headings must extract with BOTH sub-sections
/// intact -- the exact shape of the 37 empty extractions.
#[test]
fn test_sub_headings_are_kept_in_the_body() {
    let spec = r#"# Spec

## Requirements

### Functional

- **F1**: does a thing.
- **F2**: does another thing.

### Non-Functional

- **N1**: is fast.

## Guardrails
"#;
    let got = extract_section(spec, "## Requirements");
    assert!(
        got.contains("### Functional") && got.contains("F1") && got.contains("F2"),
        "Functional sub-section missing: {got:?}"
    );
    assert!(
        got.contains("### Non-Functional") && got.contains("N1"),
        "Non-Functional sub-section missing: {got:?}"
    );
}

/// E2b: two consecutive `##` sections -- the first body stops at the second
/// heading.
#[test]
fn test_body_ends_at_next_h2() {
    let spec = "# Spec\n\n## Overview\n\nOverview body.\n\n## Architecture\n\nArch body.\n";
    let got = extract_section(spec, "## Overview");
    assert_eq!(got, "Overview body.");
}

/// E2c: `## X` followed by `# Y` -- the body stops at the `#` heading too.
#[test]
fn test_body_ends_at_h1() {
    let spec = "## X\n\nX body.\n\n# Y\n\nY body.\n";
    let got = extract_section(spec, "## X");
    assert_eq!(got, "X body.");
}

/// E3a: a `## Exit Criteria` line inside a ``` fenced block must not
/// terminate the section -- the body continues past it.
#[test]
fn test_fenced_heading_does_not_terminate() {
    let spec = r#"## Overview

Some text before the fence.

```bash
## Exit Criteria
echo "this looks like a heading but is code"
```

More text after the fence, still part of Overview.

## Architecture

Arch body.
"#;
    let got = extract_section(spec, "## Overview");
    assert!(
        got.contains("## Exit Criteria") && got.contains("echo"),
        "fenced content dropped: {got:?}"
    );
    assert!(
        got.contains("More text after the fence"),
        "extraction terminated early inside/at the fence: {got:?}"
    );
    assert!(
        !got.contains("Arch body"),
        "extraction ran past the real terminator: {got:?}"
    );
}

/// E3b: the same, with a `~~~` fence.
#[test]
fn test_tilde_fence_is_honoured() {
    let spec = r#"## Overview

~~~bash
## Exit Criteria
echo "still code"
~~~

Trailing overview text.

## Architecture

Arch body.
"#;
    let got = extract_section(spec, "## Overview");
    assert!(
        got.contains("## Exit Criteria") && got.contains("Trailing overview text"),
        "tilde-fenced content dropped or section ended early: {got:?}"
    );
    assert!(!got.contains("Arch body"));
}

/// E4: a spec with no such heading yields an empty string, not an error.
#[test]
fn test_missing_section_is_empty() {
    let spec = "# Spec\n\n## Overview\n\nSome content.\n";
    let got = extract_section(spec, "## Nonexistent Section");
    assert_eq!(got, "");
}

/// E5: a heading immediately followed by the next heading yields an empty
/// body -- "absent" and "present but empty" are not distinguished.
#[test]
fn test_empty_section_is_empty() {
    let spec = "## Requirements\n## Guardrails\n\nGuardrail body.\n";
    let got = extract_section(spec, "## Requirements");
    assert_eq!(got, "");
}

/// Error-handling decision (spec-41): an unterminated fence must not swallow
/// the rest of the file. Here `## Overview` opens a ``` fence that is never
/// closed anywhere in the document. The chosen behaviour treats that
/// unpaired marker as ordinary text rather than as a fence-open, so scanning
/// continues normally and a later, real heading (`## Architecture`) is still
/// found and still terminates the section it is searched for.
#[test]
fn test_unterminated_fence_does_not_swallow_the_rest_of_the_file() {
    let spec = r#"## Overview

```bash
echo "this fence is never closed"

## Architecture

Real architecture content, after the unterminated fence.

## Guardrails

Guardrail body.
"#;
    let arch = extract_section(spec, "## Architecture");
    assert!(
        arch.contains("Real architecture content"),
        "a real heading after an unterminated fence must still be found: {arch:?}"
    );
    let guardrails = extract_section(spec, "## Guardrails");
    assert!(
        guardrails.contains("Guardrail body"),
        "extraction must not panic or hang on malformed input, and later \
         sections must remain reachable: {guardrails:?}"
    );
}

//! Spec splitting and coverage validation module.
//!
//! Enables splitting large specs (>7 exit criteria, >10 tests, >5 files) into
//! smaller child specs while maintaining 100% coverage of all original criteria.
//!
//! Critical invariant: NO exit criteria may be orphaned in the split.

pub mod coverage;
pub mod heuristics;

use crate::core::error::PidagError;
use std::path::Path;

pub use coverage::{SplitCoverage, build_coverage_report, validate_coverage, write_coverage_json};
pub use heuristics::{extract_mentioned_modules, find_tdd_tests, group_by_module};

/// A single exit criterion extracted from a spec.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ExitCriterion {
    /// Index in the original list (0-based)
    pub index: usize,
    /// Full criterion text
    pub text: String,
    /// True if criterion is a shell command (backtick-wrapped)
    pub is_shell_command: bool,
    /// Modules or files mentioned in the criterion text
    pub mentioned_modules: Vec<String>,
}

impl ExitCriterion {
    /// Create a new exit criterion.
    pub fn new(index: usize, text: String) -> Self {
        let is_shell_command = text.contains('`');
        let mentioned_modules = extract_mentioned_modules(&text);
        Self {
            index,
            text,
            is_shell_command,
            mentioned_modules,
        }
    }
}

/// Parse exit criteria from a spec string.
///
/// Looks for "## Exit Criteria" section and extracts `- [ ]` checkbox items.
/// Each item becomes an ExitCriterion. Returns error if section not found.
pub fn parse_exit_criteria(spec: &str) -> Result<Vec<ExitCriterion>, PidagError> {
    let start = spec
        .find("## Exit Criteria")
        .ok_or_else(|| PidagError::Parse("Exit Criteria section not found".to_string()))?;

    // Find next section (starts with ##) or end of spec
    let remaining = &spec[start + 16..]; // len("## Exit Criteria") = 16
    let end = remaining
        .find("##")
        .map(|i| i + start + 16)
        .unwrap_or(spec.len());

    let criteria_section = &spec[start..end];

    // Extract checkbox items: `- [ ] <text>`
    let mut criteria = Vec::new();
    let mut index = 0;

    for line in criteria_section.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("- [ ]") {
            // Extract text after "- [ ]"
            let text = rest.trim().to_string();
            if !text.is_empty() {
                criteria.push(ExitCriterion::new(index, text));
                index += 1;
            }
        }
    }

    if criteria.is_empty() {
        return Err(PidagError::Parse(
            "No exit criteria found (expected `- [ ]` items)".to_string(),
        ));
    }

    Ok(criteria)
}

/// Parse TDD Contract section and extract test names.
///
/// Looks for "## TDD Contract" section and extracts test function names
/// (usually in a table). Returns empty vector if section not found.
pub fn parse_tdd_tests(spec: &str) -> Result<Vec<String>, PidagError> {
    if let Some(start) = spec.find("## TDD Contract") {
        let remaining = &spec[start + 15..];
        let end = remaining
            .find("##")
            .map(|i| i + start + 15)
            .unwrap_or(spec.len());

        let tdd_section = &spec[start..end];

        // Extract test names from table rows: `| test_name |`
        let mut tests = Vec::new();
        for line in tdd_section.lines() {
            if line.contains("test_") && line.contains('|') {
                // Simple extraction: look for `test_*` pattern in pipes
                if let Some(test_name) = extract_test_name_from_line(line)
                    && !tests.contains(&test_name)
                {
                    tests.push(test_name);
                }
            }
        }
        Ok(tests)
    } else {
        Ok(Vec::new())
    }
}

/// Extract test name from a line (e.g., table row with `| test_foo |`)
fn extract_test_name_from_line(line: &str) -> Option<String> {
    // Look for pattern: test_<identifier>
    for part in line.split('|') {
        // SDD specs typically wrap test names in backticks (e.g. `test_foo_bar`),
        // so strip surrounding backticks before matching.
        let trimmed = part.trim().trim_matches('`');
        if trimmed.starts_with("test_") {
            // Extract until whitespace or special char
            let end_pos = trimmed
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(trimmed.len());
            let test_name = trimmed[..end_pos].to_string();
            if !test_name.is_empty() {
                return Some(test_name);
            }
        }
    }
    None
}

/// Collect the union of TDD tests associated with the given part's criteria.
///
/// Uses `find_tdd_tests` per criterion in this part, de-duplicated, preserving
/// order of first appearance.
fn collect_relevant_tests(
    criteria_indices: &[usize],
    parent_criteria: &[ExitCriterion],
    parent_content: &str,
) -> Vec<String> {
    let all_tests = parse_tdd_tests(parent_content).unwrap_or_default();
    let mut relevant: Vec<String> = Vec::new();

    for idx in criteria_indices {
        if let Some(criterion) = parent_criteria.iter().find(|c| c.index == *idx) {
            for test in find_tdd_tests(criterion, &all_tests) {
                if !relevant.contains(&test) {
                    relevant.push(test);
                }
            }
        }
    }

    relevant
}

/// Keep only TDD Contract table rows whose test name is in `keep`.
///
/// Non-data rows (header `| Test name ... |`, separator `|---|`, prose) are
/// preserved. A data row is kept iff its extracted test name appears in
/// `keep`. Relevant tests with no table row are appended as minimal rows so no
/// associated test is lost from the child spec.
fn filter_tdd_table(tdd_section: &str, keep: &[String]) -> String {
    let mut out = String::new();
    let mut seen: Vec<String> = Vec::new();

    for raw in tdd_section.lines() {
        let line = raw.trim();
        if line.starts_with('|') {
            if let Some(name) = extract_test_name_from_line(line) {
                if keep.contains(&name) && !seen.contains(&name) {
                    out.push_str(raw);
                    out.push('\n');
                    seen.push(name);
                }
                continue;
            }
            // Header or separator row -> preserve.
            out.push_str(raw);
            out.push('\n');
        } else {
            // Non-table line (prose before/after) -> preserve.
            out.push_str(raw);
            out.push('\n');
        }
    }

    // Append any relevant tests that had no table row, so none are lost.
    for name in keep {
        if !seen.contains(name) {
            out.push_str(&format!("| `{}` | (test only) |\n", name));
        }
    }

    out
}

/// Determine optimal number of child specs based on exit criteria count.
///
/// Target: 5-7 criteria per child spec. Returns at least 1.
pub fn auto_determine_split_count(total_criteria: usize) -> usize {
    const TARGET_MAX: usize = 7;
    const TARGET_AVG: usize = 6;

    if total_criteria <= TARGET_MAX {
        return 1; // Fits in one spec
    }

    // Calculate number of parts: round up to nearest part that gives us ~6 per part
    total_criteria.div_ceil(TARGET_AVG)
}

/// Split criteria into N roughly-equal groups.
///
/// Uses heuristics (module grouping, test association) to keep related
/// criteria together. Returns error if would create any empty groups.
pub fn split_into_n_parts(
    criteria: &[ExitCriterion],
    _tests: &[String],
    n_parts: usize,
) -> Result<Vec<Vec<usize>>, PidagError> {
    if n_parts == 0 {
        return Err(PidagError::Parse("n_parts must be > 0".to_string()));
    }

    if n_parts > criteria.len() {
        return Err(PidagError::Parse(format!(
            "Cannot split {} criteria into {} parts (would create empty children)",
            criteria.len(),
            n_parts
        )));
    }

    if n_parts == 1 {
        return Ok(vec![criteria.iter().map(|c| c.index).collect()]);
    }

    // Use heuristics to group related criteria
    let groupings = group_by_module(criteria, n_parts)?;
    Ok(groupings)
}

/// Split criteria into N parts while preserving their original (dependency)
/// order.
///
/// Unlike `split_into_n_parts` (which rebalances by module and can scatter a
/// later criterion into an earlier part), this assigns criteria in sequence:
/// part 1 gets the first chunk, part 2 the next, and so on. Parts are therefore
/// contiguous ranges that respect the parent's order — so a criterion `k+1` that
/// builds on criterion `k` always lands in the same-or-later part.
///
/// Returns an error if the split would create any empty part.
pub fn split_into_n_parts_ordered(
    criteria: &[ExitCriterion],
    n_parts: usize,
) -> Result<Vec<Vec<usize>>, PidagError> {
    if n_parts == 0 {
        return Err(PidagError::Parse("n_parts must be > 0".to_string()));
    }

    if n_parts > criteria.len() {
        return Err(PidagError::Parse(format!(
            "Cannot split {} criteria into {} parts (would create empty children)",
            criteria.len(),
            n_parts
        )));
    }

    if n_parts == 1 {
        return Ok(vec![criteria.iter().map(|c| c.index).collect()]);
    }

    // Sequential chunking: earlier parts may absorb the rounding remainder so
    // every part is non-empty and as balanced as possible.
    let base = criteria.len() / n_parts;
    let remainder = criteria.len() % n_parts;

    let mut parts: Vec<Vec<usize>> = Vec::with_capacity(n_parts);
    let mut offset = 0;
    for p in 0..n_parts {
        let chunk = base + if p < remainder { 1 } else { 0 };
        let slice = criteria[offset..offset + chunk]
            .iter()
            .map(|c| c.index)
            .collect();
        parts.push(slice);
        offset += chunk;
    }

    Ok(parts)
}

/// Generate child spec filename from parent and part number.
///
/// E.g., `01-auth.md` with part 1 of 3 → `01-auth-part1.md`
pub fn child_spec_filename(parent_stem: &str, part_num: usize) -> String {
    // Extract numeric prefix (e.g., "01" from "01-auth")
    let (prefix, slug) = if let Some(pos) = parent_stem.find('-') {
        (&parent_stem[..pos], &parent_stem[pos + 1..])
    } else {
        ("01", parent_stem)
    };

    format!("{}-{}-part{}.md", prefix, slug, part_num)
}

/// Artifacts a child part owns, derived from the criteria assigned to it.
///
/// `ExitCriterion::mentioned_modules` already carries the files/modules named by
/// each criterion; this collects them for one part, deduplicated and sorted.
fn owned_artifacts(criteria_indices: &[usize], parent_criteria: &[ExitCriterion]) -> Vec<String> {
    let mut owned: Vec<String> = Vec::new();
    for idx in criteria_indices {
        if let Some(c) = parent_criteria.iter().find(|c| c.index == *idx) {
            for m in &c.mentioned_modules {
                if !owned.contains(m) {
                    owned.push(m.clone());
                }
            }
        }
    }
    owned.sort();
    owned
}

/// Parts, other than this one, that own an artifact this part's criteria mention.
///
/// The split distributes criteria without module coherence, so a part can assert
/// behaviour of a module whose file another part is responsible for creating.
/// Recording that as explicit metadata is what makes the children orderable;
/// previously nothing distinguished "run me first" from "run me last".
fn depends_on_parts(
    my_indices: &[usize],
    all_groups: &[Vec<usize>],
    parent_criteria: &[ExitCriterion],
    my_part_num: usize,
) -> Vec<usize> {
    let mine = owned_artifacts(my_indices, parent_criteria);
    let mut deps: Vec<usize> = Vec::new();
    for (i, group) in all_groups.iter().enumerate() {
        let part_num = i + 1;
        if part_num == my_part_num {
            continue;
        }
        let theirs = owned_artifacts(group, parent_criteria);
        // Does any artifact I mention get CREATED by that part? Creation is
        // signalled by a `test -f <artifact>` style criterion owned there.
        let creates = theirs.iter().any(|a| {
            mine.contains(a)
                && group.iter().any(|gi| {
                    parent_criteria
                        .iter()
                        .find(|c| c.index == *gi)
                        .map(|c| c.text.contains("test -f") && c.text.contains(a.as_str()))
                        .unwrap_or(false)
                })
        });
        if creates && !deps.contains(&part_num) {
            deps.push(part_num);
        }
    }
    deps.sort();
    deps
}

/// Every artifact any part owns, mapped to the part number(s) that own it.
///
/// Built from `all_groups` (every part's criteria assignment), so ownership is
/// computable for all parts, not only the one being rendered. An artifact
/// mentioned by more than one part's criteria keeps every owning part number —
/// that is a grouping ambiguity in the parent, surfaced rather than arbitrated
/// (see "Error handling expectations" in spec-40).
fn artifact_owners(
    all_groups: &[Vec<usize>],
    parent_criteria: &[ExitCriterion],
) -> Vec<(String, Vec<usize>)> {
    let mut owners: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, group) in all_groups.iter().enumerate() {
        let part_num = i + 1;
        for artifact in owned_artifacts(group, parent_criteria) {
            match owners.iter_mut().find(|(a, _)| *a == artifact) {
                Some((_, parts)) => {
                    if !parts.contains(&part_num) {
                        parts.push(part_num);
                    }
                }
                None => owners.push((artifact, vec![part_num])),
            }
        }
    }
    owners
}

/// The first occurrence of `token` in `line`, bounded so it cannot match inside
/// a longer path-like token (e.g. `store.rs` must not match inside
/// `redb_store.rs`). Uses the same character class as [`extract_mentioned_modules`].
fn find_bounded_occurrence(line: &str, token: &str) -> Option<(usize, usize)> {
    fn is_module_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '.' || c == '-'
    }

    let mut search_from = 0;
    while search_from < line.len() {
        let rel = line[search_from..].find(token)?;
        let start = search_from + rel;
        let end = start + token.len();
        let before_ok = line[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_module_char(c));
        let after_ok = line[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_module_char(c));
        if before_ok && after_ok {
            return Some((start, end));
        }
        search_from = start + 1;
    }
    None
}

/// Annotate one Architecture line in place with the part number(s) of any
/// artifact it names that a **different** part owns (S3). An artifact this
/// part owns too, or that no part owns, is left untouched (S3b, S7) — S7
/// specifically because a wrong attribution is worse than none: it would tell
/// an implementer to skip something nobody is building.
fn annotate_line(line: &str, my_part_num: usize, owners: &[(String, Vec<usize>)]) -> String {
    let mut marks: Vec<(usize, usize, String)> = Vec::new();
    for token in extract_mentioned_modules(line) {
        let Some((_, parts)) = owners.iter().find(|(a, _)| *a == token) else {
            continue; // S7a: nobody owns it -- do not guess.
        };
        let mut others: Vec<usize> = parts
            .iter()
            .copied()
            .filter(|p| *p != my_part_num)
            .collect();
        if others.is_empty() {
            continue; // S3b: only this part owns it.
        }
        others.sort_unstable();
        let Some((start, end)) = find_bounded_occurrence(line, &token) else {
            continue;
        };
        let marker = if others.len() == 1 {
            format!(" **[Part {}]**", others[0])
        } else {
            let list = others
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(" **[Parts {}]**", list)
        };
        marks.push((start, end, marker));
    }

    if marks.is_empty() {
        return line.to_string();
    }
    marks.sort_by_key(|(start, ..)| *start);

    let mut out = String::new();
    let mut last = 0usize;
    for (start, end, marker) in marks {
        if start < last {
            continue; // overlapping match on this line; keep the earlier one.
        }
        // Land the marker outside any closing backtick/quote/paren so it reads
        // as `` `path.rs` **[Part 2]** `` rather than splitting the token.
        let mut insert_at = end;
        while let Some(c) = line[insert_at..].chars().next() {
            if matches!(c, '`' | '\'' | '"' | ')') {
                insert_at += c.len_utf8();
            } else {
                break;
            }
        }
        out.push_str(&line[last..insert_at]);
        out.push_str(&marker);
        last = insert_at;
    }
    out.push_str(&line[last..]);
    out
}

/// Annotate every line of the parent's Architecture with cross-part ownership.
///
/// Every line is preserved (S5) — this rewrites lines in place, it never drops
/// one. Only lines naming an artifact owned by a different part gain a marker.
fn annotate_architecture(
    architecture: &str,
    my_part_num: usize,
    owners: &[(String, Vec<usize>)],
) -> String {
    architecture
        .lines()
        .map(|line| annotate_line(line, my_part_num, owners))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate child spec content with metadata and rewritten Exit Criteria.
///
/// Includes Parent-Spec metadata, only the criteria assigned to this child, and
/// a Scope section naming the artifacts this part owns. The Scope section exists
/// because without it every child still described the whole system: a 15-criteria
/// spec split into three parts had part1 build all six files on its own, leaving
/// parts 2 and 3 with nothing to do (audit U-2, measured 2026-08-12).
///
/// The Architecture is retained verbatim as context (spec-40): deleting design
/// rationale would silently drop the reasoning that prevents a wrong
/// implementation. What changes is *ordering* — Scope is emitted first, so the
/// narrow instruction is read before the broad description — and *attribution*:
/// every artifact the Architecture names that a different part owns is marked
/// in place with that part's number. A trailing note alone already existed and
/// already failed (docs/FINDINGS.md: "split divided the checklist, not the work").
pub fn generate_child_spec_content(
    parent_content: &str,
    parent_stem: &str,
    part_num: usize,
    total_parts: usize,
    criteria_indices: &[usize],
    parent_criteria: &[ExitCriterion],
    all_groups: &[Vec<usize>],
) -> Result<String, PidagError> {
    // Extract parent's TDD Contract and other sections
    let tdd_section = extract_section(parent_content, "## TDD Contract");
    let requirements = extract_section(parent_content, "## Requirements");
    let guardrails = extract_section(parent_content, "## Guardrails");

    // Find parent's overview
    let overview = extract_section(parent_content, "## Overview");

    // Build child spec
    let mut child = String::new();

    // Title with parent reference
    if let Some(title_end) = parent_content.find('\n') {
        let parent_title = &parent_content[..title_end];
        child.push_str(&format!(
            "{} — Part {}/{}\n\n",
            parent_title, part_num, total_parts
        ));
    }

    // Parent-Spec metadata
    child.push_str(&format!("**Parent-Spec**: `{}.md`\n", parent_stem));
    child.push_str(&format!("**Part**: {} of {}\n", part_num, total_parts));
    child.push_str(&format!(
        "**Covers**: Exit criteria {}\n",
        criteria_indices
            .iter()
            .map(|i| (i + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    // Ordering between children. The split distributes criteria without module
    // coherence, so a part can assert behaviour of a module another part creates.
    // Without this the children look interchangeable and are not.
    let deps = depends_on_parts(criteria_indices, all_groups, parent_criteria, part_num);
    if deps.is_empty() {
        child.push_str("**Depends-On-Parts**: none — this part is self-contained\n\n");
    } else {
        child.push_str(&format!(
            "**Depends-On-Parts**: {} — run those first\n\n",
            deps.iter()
                .map(|d| format!("part{}", d))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // Copy Overview
    if !overview.is_empty() {
        child.push_str("## Overview\n\n");
        child.push_str(&overview);
        child.push_str("\n\n");
    }

    // Copy Requirements
    if !requirements.is_empty() {
        child.push_str("## Requirements\n\n");
        child.push_str(&requirements);
        child.push_str("\n\n");
    }

    // Scope FIRST (S1): the narrow instruction must be read before the broad
    // Architecture description, not after it. A full-system Architecture read
    // first forms the implementer's mental model and a trailing caveat does not
    // undo it -- that ordering, not a missing note, is what made one child build
    // the entire system (docs/FINDINGS.md).
    let owned = owned_artifacts(criteria_indices, parent_criteria);
    child.push_str(&format!(
        "## Scope (Part {} of {})\n\n",
        part_num, total_parts
    ));
    if owned.is_empty() {
        child.push_str(
            "This part's criteria name no specific artifact. Satisfy the Exit\n\
             Criteria below and create nothing else.\n\n",
        );
    } else {
        child.push_str("Create or modify **only** these artifacts:\n\n");
        for a in &owned {
            child.push_str(&format!("- `{}`\n", a));
        }
        child.push_str(
            "\nAnything else in the Architecture belongs to another part. Creating it\n\
             here duplicates that part's work and makes the split pointless.\n\n",
        );
    }

    // S4: name the other parts' artifacts explicitly, grouped by part number.
    // "Anything else belongs to another part" is not actionable; a name and a
    // number are. There is nothing to list when there is no other part
    // (total_parts == 1, S6/G7) -- no split scaffolding is added in that case.
    if total_parts > 1 {
        for (i, group) in all_groups.iter().enumerate() {
            let other_part_num = i + 1;
            if other_part_num == part_num {
                continue;
            }
            let theirs = owned_artifacts(group, parent_criteria);
            if theirs.is_empty() {
                continue;
            }
            child.push_str(&format!("Part {} owns:\n", other_part_num));
            for a in &theirs {
                child.push_str(&format!("- `{}`\n", a));
            }
            child.push('\n');
        }
    }

    // Copy Architecture. It is retained verbatim because it is genuine context --
    // deleting design rationale would silently drop the reasoning that prevents
    // a wrong implementation (S5). For a real split (total_parts > 1), every
    // mention of an artifact owned by a DIFFERENT part is annotated in place
    // with that part's number (S3), so the broad description no longer reads as
    // an unattributed build list. With total_parts == 1 there is no other part
    // to attribute to, so nothing is added (S6): copied exactly as it always has
    // been.
    let architecture = extract_section(parent_content, "## Architecture");
    if !architecture.is_empty() {
        if total_parts > 1 {
            child.push_str("## Architecture (full system — context, not a build list)\n\n");
            let owners = artifact_owners(all_groups, parent_criteria);
            child.push_str(&annotate_architecture(&architecture, part_num, &owners));
        } else {
            child.push_str("## Architecture\n\n");
            child.push_str(&architecture);
        }
        child.push_str("\n\n");
        child.push_str(
            "> **Note**: the Architecture above describes the FULL system, including\n\
             > artifacts owned by other parts. Build only what Scope lists.\n\n",
        );
    }

    // Copy TDD Contract if present. Filter to only the tests relevant to this
    // part (keep related tests with their criteria; drop unrelated table rows).
    if !tdd_section.is_empty() {
        let relevant = collect_relevant_tests(criteria_indices, parent_criteria, parent_content);
        let filtered = filter_tdd_table(&tdd_section, &relevant);
        child.push_str("## TDD Contract\n\n");
        child.push_str(&filtered);
        child.push_str("\n\n");
    }

    // Generate Exit Criteria for this part
    child.push_str("## Exit Criteria\n\n");
    for idx in criteria_indices {
        if let Some(criterion) = parent_criteria.iter().find(|c| c.index == *idx) {
            child.push_str(&format!("- [ ] {}\n", criterion.text));
        }
    }
    child.push('\n');

    // Copy Guardrails, appending the scope rule. Guardrails reach the worker
    // prompt via the sdd template, so this is where the instruction actually lands.
    child.push_str("## Guardrails\n\n");
    if !guardrails.is_empty() {
        child.push_str(&guardrails);
        child.push('\n');
    }
    child.push_str(
        "- **Do NOT create artifacts owned by other parts.** Only what Scope lists.\n\
         The Architecture section describes the full system; building all of it here\n\
         duplicates other parts' work and defeats the split.\n",
    );

    Ok(child)
}

/// True if `line` is a level-1 or level-2 Markdown heading (`^#{1,2}\s`).
///
/// This is the fixed terminator rule (E2): it does not depend on the level of
/// the section being extracted. `###` and deeper are content, never a
/// terminator -- that is the whole defect, stated as a rule.
fn is_h1_or_h2(line: &str) -> bool {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    (1..=2).contains(&hashes)
        && line
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace)
}

/// Extract a section from spec (e.g., "## Overview") returning the body
/// between that heading and the next line matching `^#{1,2}\s`.
///
/// A hand-written, fence-aware line scan -- not a Markdown parser (N2). Three
/// rules, each fixing one of the three independent ways the previous
/// nine-line substring-search version was wrong (spec-41):
///
/// - **Anchored start** (E1): the section starts only at a line whose entire
///   trimmed content equals `section_title`. A prose mention, an inline-code
///   quotation, or a deeper heading such as `### Requirements` does not
///   match -- only a line that IS the heading, verbatim, does.
/// - **Same-or-higher-level terminator, `###`+ is content** (E2): the body
///   ends at the next `^#{1,2}\s` line. A `### Functional` sub-heading is
///   part of the body, not a terminator -- this is the house style of every
///   spec from 21 onward, and the shape of most of the 37 empty extractions.
/// - **Fence-aware** (E3): lines inside a fenced code block (``` or ~~~,
///   opened and closed at line start, info string permitted) are never
///   treated as a heading, for the start or the terminator, so a `##` inside
///   a code sample can't end the section early.
///
/// Unterminated fence (error handling, spec-41): a fence marker with no
/// matching close anywhere in the document is deliberately NOT treated as
/// opening a fence at all -- it is ordinary text, and scanning continues
/// normally past it. The alternative (toggle "in a fence" and never toggle
/// back) would let one malformed marker anywhere in the file swallow every
/// later heading, including the one being searched for, turning a local typo
/// into total data loss for the rest of the document. Markers are paired by
/// type (``` with ```, ~~~ with ~~~) in file order; if a type's count is odd,
/// its last, unpaired occurrence is the one dropped. See
/// `test_unterminated_fence_does_not_swallow_the_rest_of_the_file`.
pub fn extract_section(spec: &str, section_title: &str) -> String {
    let title = section_title.trim_end();
    let lines: Vec<&str> = spec.lines().collect();

    let backtick_total = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("```"))
        .count();
    let tilde_total = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("~~~"))
        .count();
    let backtick_limit = backtick_total - (backtick_total % 2);
    let tilde_limit = tilde_total - (tilde_total % 2);

    let mut backtick_seen = 0usize;
    let mut tilde_seen = 0usize;
    let mut in_fence = false;
    let mut body: Option<Vec<&str>> = None;

    for line in lines {
        let trimmed = line.trim_start();
        let is_backtick = trimmed.starts_with("```");
        let is_tilde = trimmed.starts_with("~~~");

        let toggles_fence = if is_backtick {
            backtick_seen += 1;
            backtick_seen <= backtick_limit
        } else if is_tilde {
            tilde_seen += 1;
            tilde_seen <= tilde_limit
        } else {
            false
        };

        if toggles_fence {
            in_fence = !in_fence;
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
                if is_h1_or_h2(line) {
                    break;
                }
                b.push(line);
            }
        }
    }

    body.map(|b| b.join("\n").trim().to_string())
        .unwrap_or_default()
}

/// A child spec produced by [`split_for_auto`]: its path and the spec file name.
#[derive(Debug, Clone)]
pub struct AutoSplitResult {
    /// Absolute or as-passed child spec file path (under the parent `specs/` dir).
    pub child_path: std::path::PathBuf,
    /// Child spec filename, e.g. `01-auth-part1.md`.
    pub child_name: String,
}

/// Split a parent spec for autonomous/single-process runs.
///
/// Applies the size heuristic ([`auto_determine_split_count`]) to the parent's
/// exit-criteria count. When the spec is small enough to fit one run, returns
/// `Ok(None)` and emits nothing. When it is large (> 7 criteria), it writes the
/// child spec files (order-preserving contiguous chunks, so earlier criteria
/// stay dependencies of later ones) alongside the parent, writes the split
/// coverage report, and returns `Some(children)` in dependency order.
///
/// `project_root` locates `.pidag/` for the coverage report. The parent spec
/// path is used as-is for reading and for the coverage report's parent label.
pub fn split_for_auto(
    parent_spec_path: &Path,
    project_root: &Path,
) -> Result<Option<Vec<AutoSplitResult>>, PidagError> {
    let parent_stem = parent_spec_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| PidagError::Parse("invalid parent spec filename".to_string()))?
        .to_string();
    let parent_dir = parent_spec_path.parent().unwrap_or_else(|| Path::new("."));

    let spec = std::fs::read_to_string(parent_spec_path)
        .map_err(|e| PidagError::Parse(format!("failed to read parent spec: {e}")))?;
    let criteria = parse_exit_criteria(&spec)?;
    let tests = parse_tdd_tests(&spec)?;

    let n = auto_determine_split_count(criteria.len());
    if n <= 1 || criteria.len() <= 7 {
        return Ok(None); // fits in one run — no split
    }

    // Order-preserving split so auto runs the children in dependency order.
    let parts = split_into_n_parts_ordered(&criteria, n)
        .map_err(|e| PidagError::Parse(format!("auto split failed: {e}")))?;

    let mut child_specs: Vec<(String, Vec<usize>, Vec<String>)> = Vec::new();
    let mut children: Vec<AutoSplitResult> = Vec::new();

    for (part_num, indices) in parts.iter().enumerate() {
        let part_num_1 = part_num + 1;
        let child_name = child_spec_filename(&parent_stem, part_num_1);
        let child_path = parent_dir.join(&child_name);
        let child_content = generate_child_spec_content(
            &spec,
            &parent_stem,
            part_num_1,
            n,
            indices,
            &criteria,
            &parts,
        )?;

        // Atomic write.
        let temp_path = child_path.with_extension("md.tmp");
        std::fs::write(&temp_path, &child_content)
            .map_err(|e| PidagError::Parse(format!("failed to write child temp: {e}")))?;
        std::fs::rename(&temp_path, &child_path)
            .map_err(|e| PidagError::Parse(format!("failed to rename child spec: {e}")))?;

        let part_tests = criteria
            .iter()
            .filter(|c| indices.contains(&c.index))
            .flat_map(|c| find_tdd_tests(c, &tests))
            .fold(Vec::new(), |mut acc: Vec<String>, t| {
                if !acc.contains(&t) {
                    acc.push(t);
                }
                acc
            });

        child_specs.push((child_name.clone(), indices.clone(), part_tests));
        children.push(AutoSplitResult {
            child_path,
            child_name: child_name.clone(),
        });
    }

    // Build + validate coverage (invariant: no orphaned criteria).
    let coverage = build_coverage_report(
        &format!("specs/{}.md", parent_stem),
        &child_specs,
        criteria.len(),
    );
    if !coverage.is_complete() {
        return Err(PidagError::Parse(format!(
            "split coverage incomplete: {} orphaned",
            coverage.coverage.orphaned_criteria.len()
        )));
    }

    let coverage_dir = project_root.join(".pidag");
    write_coverage_json(&coverage, &coverage_dir.join("split-coverage.json"))?;

    Ok(Some(children))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_exit_criteria() {
        let spec = r#"
## Exit Criteria

- [ ] First criterion
- [ ] Second criterion
- [ ] Third criterion

## Next Section
"#;
        let criteria = parse_exit_criteria(spec).unwrap();
        assert_eq!(criteria.len(), 3);
        assert_eq!(criteria[0].index, 0);
        assert_eq!(criteria[0].text, "First criterion");
        assert_eq!(criteria[1].index, 1);
    }

    #[test]
    fn test_parse_exit_criteria_shell_command() {
        let spec = r#"
## Exit Criteria

- [ ] `cargo test` passes
- [ ] Regular text criterion
"#;
        let criteria = parse_exit_criteria(spec).unwrap();
        assert_eq!(criteria.len(), 2);
        assert!(criteria[0].is_shell_command);
        assert!(!criteria[1].is_shell_command);
    }

    #[test]
    fn test_auto_determine_split_count() {
        assert_eq!(auto_determine_split_count(6), 1);
        assert_eq!(auto_determine_split_count(7), 1);
        assert_eq!(auto_determine_split_count(12), 2);
        assert_eq!(auto_determine_split_count(15), 3);
    }

    #[test]
    fn test_child_spec_filename() {
        assert_eq!(child_spec_filename("01-auth", 1), "01-auth-part1.md");
        assert_eq!(child_spec_filename("01-auth", 2), "01-auth-part2.md");
        assert_eq!(child_spec_filename("02-api", 1), "02-api-part1.md");
    }

    #[test]
    fn test_split_into_n_parts_too_many() {
        let criteria = vec![
            ExitCriterion::new(0, "First".to_string()),
            ExitCriterion::new(1, "Second".to_string()),
        ];
        let result = split_into_n_parts(&criteria, &[], 5);
        assert!(result.is_err());
    }
}

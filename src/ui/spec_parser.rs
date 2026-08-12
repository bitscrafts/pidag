//! Spec markdown parsing utilities for the trace UI.
//!
//! This module provides functions to parse spec markdown files, extracting
//! titles and Exit Criteria progress for the Project Overview dashboard.

/// Parsed metadata from a spec markdown file. Pure function -- no I/O -- so it
/// can be unit-tested without touching the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSpec {
    pub title: String,              // first "# " heading, or "" if none
    pub exit_criteria_total: usize, // checkboxes outside code fences
    pub exit_criteria_done: usize,  // checked boxes outside code fences
}

/// Parse a spec markdown string into title + Exit Criteria progress.
///
/// - The title is the first line starting with `# ` (after trimming). If no
///   such line exists, `title` is empty and the caller should fall back to
///   the file stem.
/// - Exit Criteria checkboxes are lines matching `^\s*[-*]\s*\[(x|X| )\]\s*`
///   OUTSIDE fenced code blocks. A fenced block is toggled by a line that
///   starts with three backticks (```` ``` ```). Checkboxes inside code
///   fences are ignored (they may be code samples, not real criteria).
pub fn parse_spec(markdown: &str) -> ParsedSpec {
    let mut title = String::new();
    let mut total = 0usize;
    let mut done = 0usize;
    let mut in_fence = false;

    for raw in markdown.lines() {
        let line = raw.trim_start();

        // Toggle code-fence state on lines starting with ```.
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }

        // First "# " heading becomes the title.
        if title.is_empty() && line.starts_with("# ") {
            title = line[2..].trim().to_string();
            // Don't `continue` -- a heading line won't match the checkbox
            // pattern below, so falling through is harmless.
        }

        // Checkbox: `^\s*[-*]\s*\[(x|X| )\]\s*...`
        // Match `- [ ]`, `- [x]`, `* [X]`, etc. Collapsed via let-chains
        // (the codebase already uses them in redb_store.rs) to keep the
        // match logic flat and clippy-clean.
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            let rest = rest.trim_start();
            if let Some(after_box) = rest.strip_prefix('[')
                && let Some(marker_char) = after_box.chars().next()
                && let Some(inside) = after_box.strip_prefix(marker_char)
                && inside.starts_with(']')
            {
                // It's a checkbox line. Count it.
                total += 1;
                if marker_char == 'x' || marker_char == 'X' {
                    done += 1;
                }
            }
        }
    }

    ParsedSpec {
        title,
        exit_criteria_total: total,
        exit_criteria_done: done,
    }
}

/// Derive the status badge from Exit Criteria progress (R3).
pub fn spec_status(total: usize, done: usize) -> &'static str {
    if total == 0 {
        "no-criteria"
    } else if done == total {
        "complete"
    } else if done == 0 {
        "pending"
    } else {
        "in-progress"
    }
}

/// Reject spec names that could escape the `specs/` directory. The name
/// must match `^[A-Za-z0-9._-]+$` (no path separators, no `..`). This is
/// defense-in-depth on top of the path resolution check in the handler.
pub fn is_safe_spec_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && name != "."
        && name != ".."
}

#[cfg(test)]
mod tests {
    use super::{parse_spec, spec_status};

    fn md(title: &str, body: &str) -> String {
        format!("# {}\n\n{}", title, body)
    }

    // T1: counts checked + unchecked boxes outside code fences
    #[test]
    fn test_parse_spec_counts_checkboxes() {
        let m = md(
            "Sample",
            "## Exit Criteria\n\n- [x] one\n- [x] two\n- [ ] three\n- [ ] four\n- [x] five\n",
        );
        let p = parse_spec(&m);
        assert_eq!(p.title, "Sample");
        assert_eq!(p.exit_criteria_total, 5, "total should count all 5 boxes");
        assert_eq!(p.exit_criteria_done, 3, "done should count 3 checked boxes");
        assert_eq!(spec_status(5, 3), "in-progress");
        assert_eq!(spec_status(5, 5), "complete");
        assert_eq!(spec_status(5, 0), "pending");
        assert_eq!(spec_status(0, 0), "no-criteria");
    }

    // T2: checkbox lines inside code fences are NOT counted
    #[test]
    fn test_parse_spec_ignores_code_fences() {
        let m = md(
            "Fenced",
            "## Exit Criteria\n\n- [x] real\n\n```\n- [x] fake\n- [ ] fake\n```\n\n- [ ] real2\n",
        );
        let p = parse_spec(&m);
        assert_eq!(
            p.exit_criteria_total, 2,
            "only checkboxes outside fences count, got {}",
            p.exit_criteria_total
        );
        assert_eq!(p.exit_criteria_done, 1, "one checked outside fences");
    }

    // T3: first "# " heading becomes title; no heading -> empty (caller falls back to stem)
    #[test]
    fn test_parse_spec_extracts_title() {
        let with_title = "# My Spec\n\nbody";
        assert_eq!(parse_spec(with_title).title, "My Spec");

        let no_title = "body without a heading";
        assert_eq!(parse_spec(no_title).title, "", "no heading -> empty title");
    }

    // Extra: case-insensitive [X] counts as done
    #[test]
    fn test_parse_spec_uppercase_x_counts() {
        let m = md("C", "- [X] upper\n- [x] lower\n- [ ] none\n");
        let p = parse_spec(&m);
        assert_eq!(p.exit_criteria_total, 3);
        assert_eq!(p.exit_criteria_done, 2, "[X] and [x] both count as done");
    }

    // Extra: asterisk bullets and leading whitespace
    #[test]
    fn test_parse_spec_asterisk_and_indent() {
        let m = md("A", "  * [x] a\n  * [ ] b\n    - [x] c\n");
        let p = parse_spec(&m);
        assert_eq!(
            p.exit_criteria_total, 3,
            "asterisk + indented bullets count"
        );
        assert_eq!(p.exit_criteria_done, 2);
    }
}

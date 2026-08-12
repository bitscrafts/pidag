//! Splitting heuristics: module grouping and test association.

use super::ExitCriterion;
use crate::core::error::PidagError;
use std::collections::HashMap;

/// Extract module/file names mentioned in criterion text.
///
/// Recognises any `name.ext` token for a known source extension, wherever it
/// appears — including inside a backtick-wrapped shell command, which is how
/// Exit Criteria are normally written.
///
/// Two defects this replaces, both found on 2026-08-12 by splitting a Python
/// spec (audit U-2):
///
/// 1. It matched **only `.rs`**, so every non-Rust spec produced no modules at
///    all and `group_by_module` — which keys on `mentioned_modules[0]` — had
///    nothing to group by, making the distribution effectively arbitrary.
/// 2. It treated an entire backtick span as a module name. Criteria are usually
///    written as `` `test -f logfilter.py` ``, so the "module" became the whole
///    shell command, and the Scope section listed commands instead of files.
pub fn extract_mentioned_modules(text: &str) -> Vec<String> {
    const SOURCE_EXTENSIONS: &[&str] = &[
        "rs", "py", "js", "ts", "go", "rb", "java", "c", "h", "cpp", "hpp", "sh", "toml", "json",
        "yaml", "yml", "md", "sql",
    ];

    let mut modules: Vec<String> = Vec::new();

    // Split on anything that cannot appear in a path-like token. Backticks,
    // quotes, parentheses and semicolons are all separators, so a file name
    // embedded in a shell command is found on its own rather than swallowed
    // along with the command around it.
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.' && c != '-') {
        let candidate = word.trim_matches('.');
        if candidate.len() < 3 {
            continue;
        }
        let Some((stem, ext)) = candidate.rsplit_once('.') else {
            continue;
        };
        if stem.is_empty() || !SOURCE_EXTENSIONS.contains(&ext) {
            continue;
        }
        let module = candidate.to_string();
        if !modules.contains(&module) {
            modules.push(module);
        }
    }

    modules
}

/// Find TDD tests related to a criterion by function name matching.
///
/// Extracts snake_case identifiers from criterion text and matches them
/// against test names.
pub fn find_tdd_tests(criterion: &ExitCriterion, all_tests: &[String]) -> Vec<String> {
    let mut related_tests = Vec::new();

    // Extract identifiers from criterion text (snake_case, alphanumeric + underscore)
    let identifiers = extract_identifiers(&criterion.text);

    for test in all_tests {
        // Check if test name contains any of the criterion's identifiers
        for id in &identifiers {
            if test.contains(id) {
                related_tests.push(test.clone());
                break; // Don't add same test twice
            }
        }
    }

    related_tests
}

/// Generic tokens that should never be treated as a meaningful identifier for
/// test association (they match every test name and cause false positives).
const STOP_WORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "from",
    "it",
    "in",
    "to",
    "of",
    "a",
    "an",
    "is",
    "are",
    "be",
    "as",
    "on",
    "at",
    "by",
    "or",
    "not",
    "when",
    "must",
    "should",
    "will",
    "has",
    "have",
    "via",
    "using",
    "shell",
    // build/test tooling that appears in exit-criteria commands
    "test",
    "tests",
    "cargo",
    "rustc",
    "clippy",
    "fmt",
    "bash",
    "run",
    "go",
    "make",
    "gradle",
    "mvn",
    "npm",
    "passes",
    "pass",
    "check",
    "checks",
    "command",
    "cmd",
    "implement",
    "implementation",
    "expected",
    "output",
    "returns",
    "require",
    "requires",
    "build",
    "compile",
];

/// Extract strong snake_case identifiers from text.
///
/// Prefers identifiers containing an underscore (the strongest signal for a
/// function/test name), and skips generic stop-words and single-character
/// tokens to avoid false-positive associations across parts.
fn extract_identifiers(text: &str) -> Vec<String> {
    let mut identifiers: Vec<String> = Vec::new();
    let not_present =
        |identifiers: &Vec<String>, word: &str| !identifiers.iter().any(|i| i == word);

    // Split on non-identifier characters
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.is_empty() || STOP_WORDS.contains(&word) {
            continue;
        }
        // Strong signal: underscore-containing snake_case token.
        if word.contains('_') {
            if not_present(&identifiers, word) {
                identifiers.push(word.to_string());
            }
            continue;
        }
        // Weak signal: a standalone lowercase word of meaningful length is only
        // kept if it looks like a domain term (all lowercase, len >= 4) — but
        // generic tooling words are already filtered by STOP_WORDS above.
        if word.chars().all(|c| c.is_lowercase())
            && word.len() >= 4
            && not_present(&identifiers, word)
        {
            identifiers.push(word.to_string());
        }
    }

    identifiers
}

/// Group criteria by module/theme, trying to balance group sizes.
///
/// Returns a vector of criterion index groups. Each group is a vector of
/// indices that should go together in a child spec.
pub fn group_by_module(
    criteria: &[ExitCriterion],
    n_parts: usize,
) -> Result<Vec<Vec<usize>>, PidagError> {
    if n_parts == 0 || n_parts > criteria.len() {
        return Err(PidagError::Parse(format!(
            "Invalid n_parts: {} (have {} criteria)",
            n_parts,
            criteria.len()
        )));
    }

    if n_parts == 1 {
        return Ok(vec![criteria.iter().map(|c| c.index).collect()]);
    }

    // Build module → indices mapping
    let mut module_groups: HashMap<String, Vec<usize>> = HashMap::new();
    let mut unassigned = Vec::new();

    for criterion in criteria {
        if criterion.mentioned_modules.is_empty() {
            unassigned.push(criterion.index);
        } else {
            // Assign to first mentioned module
            let module = &criterion.mentioned_modules[0];
            module_groups
                .entry(module.clone())
                .or_default()
                .push(criterion.index);
        }
    }

    // Distribute groups across parts, balancing sizes
    let mut parts: Vec<Vec<usize>> = vec![Vec::new(); n_parts];
    let mut part_sizes = vec![0; n_parts];

    // First, distribute module groups
    let mut sorted_modules: Vec<_> = module_groups.iter().collect();
    sorted_modules.sort_by_key(|(_, indices)| std::cmp::Reverse(indices.len()));

    for (_, indices) in sorted_modules {
        // Find the smallest part and add this group
        let min_part = part_sizes
            .iter()
            .enumerate()
            .min_by_key(|(_, size)| *size)
            .map(|(i, _)| i)
            .unwrap_or(0);

        for idx in indices {
            parts[min_part].push(*idx);
            part_sizes[min_part] += 1;
        }
    }

    // Then distribute unassigned criteria
    for idx in unassigned {
        let min_part = part_sizes
            .iter()
            .enumerate()
            .min_by_key(|(_, size)| *size)
            .map(|(i, _)| i)
            .unwrap_or(0);

        parts[min_part].push(idx);
        part_sizes[min_part] += 1;
    }

    // Sort indices within each part for determinism
    for part in &mut parts {
        part.sort();
    }

    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_mentioned_modules() {
        let text = "Implement auth.rs module with db.rs integration";
        let modules = extract_mentioned_modules(text);
        assert!(modules.contains(&"auth.rs".to_string()));
        assert!(modules.contains(&"db.rs".to_string()));
    }

    #[test]
    fn test_extract_identifiers() {
        let text = "The user_auth_handler must validate_token";
        let ids = extract_identifiers(text);
        assert!(ids.contains(&"user_auth_handler".to_string()));
        assert!(ids.contains(&"validate_token".to_string()));
    }

    #[test]
    fn test_find_tdd_tests() {
        let criterion = ExitCriterion::new(0, "Implement user_auth function".to_string());
        let tests = vec![
            "test_user_auth_create".to_string(),
            "test_user_auth_validate".to_string(),
            "test_other".to_string(),
        ];

        let related = find_tdd_tests(&criterion, &tests);
        assert_eq!(related.len(), 2);
        assert!(related.contains(&"test_user_auth_create".to_string()));
    }

    #[test]
    fn test_group_by_module() {
        let criteria = vec![
            ExitCriterion {
                index: 0,
                text: "auth.rs implementation".to_string(),
                is_shell_command: false,
                mentioned_modules: vec!["auth.rs".to_string()],
            },
            ExitCriterion {
                index: 1,
                text: "auth.rs validation".to_string(),
                is_shell_command: false,
                mentioned_modules: vec!["auth.rs".to_string()],
            },
            ExitCriterion {
                index: 2,
                text: "db.rs layer".to_string(),
                is_shell_command: false,
                mentioned_modules: vec!["db.rs".to_string()],
            },
        ];

        let groups = group_by_module(&criteria, 2).unwrap();
        assert_eq!(groups.len(), 2);
        // Criteria 0,1 should be together; 2 separate
        assert!(groups[0].contains(&0) && groups[0].contains(&1));
        assert!(groups[1].contains(&2));
    }
}

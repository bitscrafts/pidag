//! Coverage report generation and validation.

use crate::core::error::PidagError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Coverage statistics for a split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageStats {
    pub total_parent_criteria: usize,
    pub covered_criteria: usize,
    pub orphaned_criteria: Vec<usize>,
}

/// Child spec coverage information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildCoverage {
    pub spec: String,
    pub exit_criteria_indices: Vec<usize>,
    pub tdd_tests: Vec<String>,
}

/// Complete coverage report for a split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitCoverage {
    pub parent_spec: String,
    pub split_date: String,
    pub children: Vec<ChildCoverage>,
    pub coverage: CoverageStats,
}

impl SplitCoverage {
    /// Check if coverage is 100% (no orphaned criteria).
    pub fn is_complete(&self) -> bool {
        self.coverage.orphaned_criteria.is_empty()
            && self.coverage.covered_criteria == self.coverage.total_parent_criteria
    }
}

/// Build a coverage report from split information.
///
/// Maps each criterion index to the child spec that contains it.
pub fn build_coverage_report(
    parent_spec_path: &str,
    child_specs: &[(String, Vec<usize>, Vec<String>)],
    total_criteria: usize,
) -> SplitCoverage {
    let mut covered_set = std::collections::HashSet::new();
    let mut children = Vec::new();

    for (spec_name, indices, tests) in child_specs {
        for idx in indices {
            covered_set.insert(*idx);
        }
        children.push(ChildCoverage {
            spec: spec_name.clone(),
            exit_criteria_indices: indices.clone(),
            tdd_tests: tests.clone(),
        });
    }

    let mut orphaned = Vec::new();
    for i in 0..total_criteria {
        if !covered_set.contains(&i) {
            orphaned.push(i);
        }
    }

    let covered_criteria = covered_set.len();
    orphaned.sort();

    SplitCoverage {
        parent_spec: parent_spec_path.to_string(),
        split_date: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        children,
        coverage: CoverageStats {
            total_parent_criteria: total_criteria,
            covered_criteria,
            orphaned_criteria: orphaned,
        },
    }
}

/// Write coverage report to JSON file with atomic temp+rename.
///
/// Writes to a temporary file first, then atomically renames to the target.
pub fn write_coverage_json(coverage: &SplitCoverage, output_path: &Path) -> Result<(), PidagError> {
    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PidagError::Parse(format!("Failed to create directory: {}", e)))?;
    }

    // Serialize to JSON
    let json = serde_json::to_string_pretty(coverage)
        .map_err(|e| PidagError::Parse(format!("Failed to serialize coverage: {}", e)))?;

    // Write to temp file
    let temp_path = output_path.with_extension("json.tmp");
    std::fs::write(&temp_path, json)
        .map_err(|e| PidagError::Parse(format!("Failed to write temp file: {}", e)))?;

    // Atomic rename
    std::fs::rename(&temp_path, output_path)
        .map_err(|e| PidagError::Parse(format!("Failed to rename coverage file: {}", e)))?;

    Ok(())
}

/// Read and validate an existing coverage report.
///
/// Checks that all parent criteria are covered (no orphans).
/// Returns error if any criteria are orphaned.
pub fn validate_coverage(coverage_path: &Path) -> Result<SplitCoverage, PidagError> {
    let content = std::fs::read_to_string(coverage_path)
        .map_err(|e| PidagError::Parse(format!("Failed to read coverage file: {}", e)))?;

    let coverage: SplitCoverage = serde_json::from_str(&content)
        .map_err(|e| PidagError::Parse(format!("Failed to parse coverage JSON: {}", e)))?;

    if !coverage.is_complete() {
        return Err(PidagError::Parse(format!(
            "Coverage validation failed: {} orphaned criteria {:?}",
            coverage.coverage.orphaned_criteria.len(),
            coverage.coverage.orphaned_criteria
        )));
    }

    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_coverage_report() {
        let children = vec![
            ("part1.md".to_string(), vec![0, 1, 2], vec![]),
            ("part2.md".to_string(), vec![3, 4, 5], vec![]),
        ];

        let report = build_coverage_report("parent.md", &children, 6);
        assert_eq!(report.coverage.total_parent_criteria, 6);
        assert_eq!(report.coverage.covered_criteria, 6);
        assert!(report.coverage.orphaned_criteria.is_empty());
        assert!(report.is_complete());
    }

    #[test]
    fn test_build_coverage_orphaned() {
        let children = vec![("part1.md".to_string(), vec![0, 1], vec![])];

        let report = build_coverage_report("parent.md", &children, 5);
        assert_eq!(report.coverage.total_parent_criteria, 5);
        assert_eq!(report.coverage.covered_criteria, 2);
        assert_eq!(report.coverage.orphaned_criteria, vec![2, 3, 4]);
        assert!(!report.is_complete());
    }
}

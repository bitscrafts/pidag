//! Spec-splitting abstraction for the autonomous driver.
//!
//! Free-tier model exhaustion (HTTP 429) hits on LONG tasks, so the driver
//! keeps each SDD sub-run small. Before dispatching an `Implement` target it
//! asks a [`SpecSplitter`] whether the spec is large enough to warrant being
//! split into child specs, and if so gets the ordered child paths to run one by
//! one.
//!
//! The split behaviour is behind a trait so the driver is decoupled from the
//! concrete splitter (and so it can be unit-tested with a fake). The default
//! implementation is [`crate::split::split_for_auto`].

use std::fmt;
use std::path::{Path, PathBuf};

/// A child spec produced by a [`SpecSplitter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitChild {
    /// Path to the child spec file.
    pub path: PathBuf,
    /// Child spec filename, e.g. `01-auth-part1.md`.
    pub name: String,
}

/// Strategy for deciding whether a spec should be split before running and, if
/// so, producing the ordered child specs in precedence/dependency order.
///
/// Implementers are `Send + Sync` so a splitter can be shared across async
/// driver passes. `box_clone` enables cloning a boxed splitter, letting it be
/// stored in `AutoOptions` (which derives `Clone`).
pub trait SpecSplitter: Send + Sync {
    /// Inspect `parent_spec_path` under `project_root`. Returns `Ok(None)` when
    /// the spec fits in one run; `Ok(Some(children))` when it was split; `Err`
    /// on an unrecoverable split failure.
    fn split_if_large(
        &self,
        parent_spec_path: &Path,
        project_root: &Path,
    ) -> Result<Option<Vec<SplitChild>>, String>;

    /// Clone this splitter as a boxed trait object.
    fn box_clone(&self) -> Box<dyn SpecSplitter>;
}

impl Clone for Box<dyn SpecSplitter> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

impl fmt::Debug for dyn SpecSplitter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpecSplitter")
    }
}

/// The production splitter: delegates to `crate::split::split_for_auto`, which
/// applies the size heuristic (auto-determination by exit-criteria count) and
/// writes child specs + the coverage report.
#[derive(Debug, Default, Clone)]
pub struct AutoSplitter;

impl AutoSplitter {
    /// Box a fresh default splitter (convenience for callers building options).
    pub fn boxed() -> Box<dyn SpecSplitter> {
        Box::new(AutoSplitter)
    }
}

impl SpecSplitter for AutoSplitter {
    fn split_if_large(
        &self,
        parent_spec_path: &Path,
        project_root: &Path,
    ) -> Result<Option<Vec<SplitChild>>, String> {
        match crate::split::split_for_auto(parent_spec_path, project_root) {
            Ok(None) => Ok(None),
            Ok(Some(results)) => Ok(Some(
                results
                    .into_iter()
                    .map(|r| SplitChild {
                        path: r.child_path,
                        name: r.child_name,
                    })
                    .collect(),
            )),
            Err(e) => Err(format!("auto-split failed: {e}")),
        }
    }

    fn box_clone(&self) -> Box<dyn SpecSplitter> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_project() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static C: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "pidag-splitter-{}-{}",
            std::process::id(),
            C.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("specs")).unwrap();
        d
    }

    /// Build a realistic spec body with `criteria` exit-criteria items and the
    /// mandatory TDD contract section.
    fn large_spec(criteria: usize) -> String {
        let mut s = String::from(
            "# Spec: Big Feature\n\n## Overview\nBig.\n\n## Requirements\n\n### Functional\n\n## TDD Contract\n",
        );
        for i in 0..criteria {
            s.push_str(&format!(
                "| `test_{i:02}_feature` | input {i} | output {i} |\n"
            ));
        }
        s.push_str("\n## Exit Criteria\n");
        for i in 0..criteria {
            s.push_str(&format!("- [ ] `cargo test test_{i:02}_feature` passes\n"));
        }
        s.push('\n');
        s
    }

    #[test]
    fn small_spec_is_not_split() {
        let project = tmp_project();
        let spec_path = project.join("specs").join("01-feature.md");
        std::fs::write(&spec_path, large_spec(5)).unwrap(); // 5 criteria -> fits

        let split = AutoSplitter.split_if_large(&spec_path, &project).unwrap();
        assert!(split.is_none(), "small spec should not be split");

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn large_spec_is_split_into_children_and_covers_all() {
        let project = tmp_project();
        let spec_path = project.join("specs").join("03-large.md");
        std::fs::write(&spec_path, large_spec(12)).unwrap(); // 12 criteria -> split

        let children = AutoSplitter
            .split_if_large(&spec_path, &project)
            .unwrap()
            .expect("large spec should be split");
        assert!(
            children.len() >= 2,
            "12 criteria should split into at least 2 children, got {}",
            children.len()
        );

        // Every child file must actually exist on disk.
        for c in &children {
            assert!(c.path.exists(), "child spec {} should exist", c.name);
            assert!(c.name.ends_with(".md"));
        }

        // Coverage report written to <project>/.pidag/split-coverage.json.
        let cov = project.join(".pidag").join("split-coverage.json");
        assert!(cov.exists(), "coverage report should be written");
        let parsed =
            crate::split::validate_coverage(&cov).expect("coverage report should parse + validate");
        assert!(parsed.is_complete(), "no orphaned criteria allowed");

        let _ = std::fs::remove_dir_all(&project);
    }
}

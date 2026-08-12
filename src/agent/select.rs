//! Project and spec selection for the autonomous driver.
//!
//! Priority order for a single drive pass:
//! 1. A resume candidate — an entry left `Running` by a crash, or `Failed` and
//!    therefore retryable (resume across run boundaries, 429-safe).
//! 2. The next `Pending` spec by priority.
//! 3. A handoff-nominated spec (if still pending), else a handoff work item.
//!
//! Across a workspace, projects are ordered by explicit `weight` (higher first)
//! then by pending workload, so heavy-weight projects get more turns without
//! starving lighter ones.

use crate::agent::handoff;
use crate::queue::{
    SpecState, discover_projects, discover_specs, merge_queues, read_project_queue,
};
use std::path::{Path, PathBuf};

/// A selected project to work on next.
pub struct SelectedProject {
    pub root: PathBuf,
    pub spec_file: String,
}

/// Merge cached queue state into the discovered spec list so selection sees the
/// true `Running`/`Failed`/`Done` state (not the filesystem's default Pending).
fn merged_specs(project_root: &Path) -> Vec<crate::queue::QueueEntry> {
    let discovered = discover_specs(project_root);
    if discovered.is_empty() {
        return discovered;
    }
    match read_project_queue(project_root) {
        Ok(Some(cached)) => merge_queues(&cached, &discovered, project_root).entries,
        _ => discovered,
    }
}

/// Choose the next project + spec from a workspace root.
///
/// Projects are considered by explicit weight (descending) then by pending
/// workload (descending), so a weighted project yields more turns. The caller's
/// `handoff_read` lets us honour a handoff-nominated target.
pub fn select_from_workspace(
    workspace: &Path,
    handoff_read: &dyn Fn(&Path) -> Option<String>,
) -> Result<Option<SelectedProject>, String> {
    let projects = discover_projects(workspace).map_err(|e| e.to_string())?;

    // Rank projects: weight desc, then pending-workload desc.
    let mut ranked: Vec<(PathBuf, f64, usize)> = projects
        .iter()
        .map(|p| {
            let weight = read_project_queue(p)
                .ok()
                .flatten()
                .map(|q| q.weight)
                .unwrap_or(1.0);
            let workload = merged_specs(p)
                .iter()
                .filter(|e| {
                    matches!(
                        e.state,
                        SpecState::Pending | SpecState::Running | SpecState::Failed
                    )
                })
                .count();
            (p.clone(), weight, workload)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.2.cmp(&a.2))
    });

    for (root, _, _) in ranked {
        if let Some(spec) = select_spec_for_project(&root, handoff_read)? {
            return Ok(Some(SelectedProject {
                root,
                spec_file: spec,
            }));
        }
    }
    Ok(None)
}

/// Choose the next spec for a single project:
/// 1. A resume candidate (`Running` after a crash, or `Failed` to retry).
/// 2. The handoff-nominated spec, if still pending.
/// 3. The next fresh `Pending` spec by priority.
///
/// Returns `Ok(None)` when nothing is pending.
pub fn select_spec_for_project(
    project_root: &Path,
    handoff_read: &dyn Fn(&Path) -> Option<String>,
) -> Result<Option<String>, String> {
    let specs = merged_specs(project_root);
    if specs.is_empty() {
        return Ok(None);
    }

    // 1. Resume/retry: Running (crashed) or Failed entries first.
    if let Some(r) = specs
        .iter()
        .filter(|e| matches!(e.state, SpecState::Running | SpecState::Failed))
        .map(|e| e.spec_file.clone())
        .next()
    {
        return Ok(Some(r));
    }

    // 2. Handoff-nominated spec, if still discoverable as pending.
    let handoff = handoff_read(project_root);
    if let Some(nominated) = handoff::spec_nominated_by_handoff(handoff.as_deref()) {
        let stem = nominated
            .trim_start_matches("specs/")
            .trim_end_matches(".md");
        if specs
            .iter()
            .any(|e| e.spec_name == stem && e.state == SpecState::Pending)
        {
            return Ok(Some(nominated));
        }
    }

    // 3. Fresh pending by priority.
    let next = specs
        .iter()
        .filter(|e| e.state == SpecState::Pending)
        .map(|e| e.spec_file.clone())
        .next();
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot_fs(specs: &[&str]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pidag-sel-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for s in specs {
            std::fs::write(dir.join(s), "# spec").unwrap();
        }
        dir
    }

    #[test]
    fn picks_first_pending_by_priority() {
        let dir = boot_fs(&["01-a.md", "02-b.md", "03-c.md"]);
        // No handoff file -> no nomination, no queue state -> all pending.
        let got = select_spec_for_project(&dir, &|_| None).unwrap();
        assert_eq!(got.as_deref(), Some("specs/01-a.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn none_when_no_specs() {
        let dir = boot_fs(&[]);
        assert_eq!(select_spec_for_project(&dir, &|_| None).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_prefers_cached_running() {
        use crate::queue::write_project_queue;
        let dir = boot_fs(&["01-a.md", "02-b.md"]);
        // Only 02-b in cached state; mark it Running (crashed mid-run).
        let q = crate::queue::ProjectQueue {
            project_root: dir.to_str().unwrap().to_string(),
            entries: vec![crate::queue::QueueEntry {
                spec_name: "02-b".to_string(),
                spec_file: "specs/02-b.md".to_string(),
                state: SpecState::Running,
                priority: 2,
                last_run_at: None,
                run_id: None,
                error: None,
            }],
            updated_at: crate::queue::now_iso(),
            weight: 1.0,
        };
        write_project_queue(&dir, &q).unwrap();
        let got = select_spec_for_project(&dir, &|_| None).unwrap();
        assert_eq!(
            got.as_deref(),
            Some("specs/02-b.md"),
            "resume Running first"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

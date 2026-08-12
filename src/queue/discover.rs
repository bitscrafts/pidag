//! Lazy spec discovery + NN-prefix priority parsing for the queue.
//!
//! Specs are always scanned from the filesystem on each invocation (never
//! cached in state) so the queue reflects the current `specs/` directory
//! (spec R11).

use super::{QueueEntry, SpecState};
use crate::core::error::PidagError;
use std::path::{Path, PathBuf};

/// Parse an NN-prefix priority from a spec filename stem.
///
/// `"42-foo"` -> `Some(42)`; `"readme"` -> `None`.
pub fn extract_priority(name: &str) -> Option<u8> {
    let stem = name.trim_end_matches(".md");
    let (prefix, _) = stem.split_once('-')?;
    prefix.parse::<u8>().ok().filter(|n| *n >= 1)
}

/// Discover numbered specs from a project, returning entries sorted by
/// priority (NN prefix). Unnumbered files (e.g. `readme.md`) are ignored.
///
/// Scans both the given directory and (if present) its `specs/` subdirectory,
/// de-duplicating by file name. Real pidag projects keep specs in `specs/`;
/// TDD tests write specs directly in the tmp dir. Spec paths are labelled
/// `specs/<file>` for consistency with the queue state file format.
pub fn discover_specs(project_root: &Path) -> Vec<QueueEntry> {
    let mut entries = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let scan = |dir: &Path,
                entries: &mut Vec<QueueEntry>,
                seen: &mut std::collections::HashSet<String>| {
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        for item in read_dir.flatten() {
            let path = item.path();
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let Some(priority) = extract_priority(&file_name) else {
                continue; // unnumbered -> ignore
            };
            if seen.contains(&file_name) {
                continue;
            }
            seen.insert(file_name.clone());
            entries.push(QueueEntry {
                spec_name: file_name.trim_end_matches(".md").to_string(),
                spec_file: format!("specs/{}", file_name),
                state: SpecState::Pending,
                priority,
                last_run_at: None,
                run_id: None,
                error: None,
            });
        }
    };

    scan(project_root, &mut entries, &mut seen);
    let specs_sub = project_root.join("specs");
    if specs_sub.is_dir() {
        scan(&specs_sub, &mut entries, &mut seen);
    }

    entries.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.spec_file.cmp(&b.spec_file))
    });
    entries
}

/// Discover sub-projects under a workspace root. A sub-project is any directory
/// containing a `specs/` subdirectory. Returns sorted project paths.
pub fn discover_projects(workspace_root: &Path) -> Result<Vec<PathBuf>, PidagError> {
    let read_dir = std::fs::read_dir(workspace_root)
        .map_err(|e| PidagError::Parse(format!("failed to read workspace {e:?}: {e}")))?;
    let mut projects = Vec::new();
    for item in read_dir.flatten() {
        let p = item.path();
        if p.is_dir() && p.join("specs").is_dir() {
            projects.push(p);
        }
    }
    projects.sort();
    Ok(projects)
}

/// Render the queue status table for a project root.
pub fn render_status_table(entries: &[QueueEntry]) -> String {
    let mut out = String::from("PRIORITY  STATE     SPEC\n");
    for e in entries {
        out.push_str(&format!(
            "{:>8}  {:<9} {}\n",
            e.priority,
            format!("{:?}", e.state).to_lowercase(),
            e.spec_file
        ));
    }
    out
}

/// Run before executing a spec: if enabled, snapshot the current good state
/// file to a `.bak` so a crash during the run can be recovered.
pub fn backup_queue_if_needed(project_root: &Path) {
    let state_path = super::state::state_file_path(project_root);
    if state_path.exists() {
        let bak = state_path.with_extension("json.bak");
        let _ = std::fs::copy(&state_path, &bak);
    }
}

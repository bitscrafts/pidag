//! State persistence for the pidag queue.
//!
//! Reads/writes `.pidag/queue.json` with atomic temp+rename semantics so a
//! crash mid-write never leaves a partially-written state file. State only
//! tracks execution status; the spec list is always re-discovered lazily from
//! the filesystem (spec R9, R11).

use super::{ProjectQueue, QueueEntry, SpecState};
use crate::core::error::PidagError;
use std::path::{Path, PathBuf};

/// The state file path for a project root: `<root>/.pidag/queue.json`.
pub fn state_file_path(project_root: &Path) -> PathBuf {
    project_root.join(".pidag").join("queue.json")
}

/// Write a project queue atomically (temp file + rename).
pub fn write_project_queue(project_root: &Path, queue: &ProjectQueue) -> Result<(), PidagError> {
    let path = state_file_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PidagError::Parse(format!("failed to create state dir: {e}")))?;
    }

    let json = serde_json::to_string_pretty(queue)
        .map_err(|e| PidagError::Parse(format!("failed to serialize queue: {e}")))?;

    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)
        .map_err(|e| PidagError::Parse(format!("failed to write temp state file: {e}")))?;
    std::fs::rename(&temp, &path)
        .map_err(|e| PidagError::Parse(format!("failed to rename state file: {e}")))?;
    Ok(())
}

/// Read a project queue from disk, returning `Ok(None)` when no state file
/// exists (fresh project).
pub fn read_project_queue(project_root: &Path) -> Result<Option<ProjectQueue>, PidagError> {
    let path = state_file_path(project_root);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| PidagError::Parse(format!("failed to read state file: {e}")))?;
    let queue: ProjectQueue = serde_json::from_str(&content)
        .map_err(|e| PidagError::Parse(format!("failed to parse state file: {e}")))?;
    Ok(Some(queue))
}

/// Merge a freshly-discovered list of specs with cached state, preserving the
/// status of specs that are already known (e.g. `Done`) and adding new specs
/// as `Pending`.
pub fn merge_queues(
    cached: &ProjectQueue,
    discovered: &[QueueEntry],
    project_root: &Path,
) -> ProjectQueue {
    let cached_by_file: std::collections::HashMap<&str, &QueueEntry> = cached
        .entries
        .iter()
        .map(|e| (e.spec_file.as_str(), e))
        .collect();

    let mut entries: Vec<QueueEntry> = discovered
        .iter()
        .map(|d| {
            if let Some(prev) = cached_by_file.get(d.spec_file.as_str()) {
                // Preserve the known status (use the cached structural fields).
                let mut e = d.clone();
                e.state = prev.state;
                e.last_run_at = prev.last_run_at.clone();
                e.run_id = prev.run_id.clone();
                e.error = prev.error.clone();
                e
            } else {
                d.clone()
            }
        })
        .collect();

    // Keep deterministic ordering by priority then spec file.
    entries.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.spec_file.cmp(&b.spec_file))
    });

    let updated_at = crate::queue::now_iso();
    ProjectQueue {
        project_root: project_root.to_string_lossy().to_string(),
        entries,
        updated_at,
        // Preserve the cached scheduling weight so re-discover/merge does not
        // stampede a configured weight back to the 1.0 default.
        weight: cached.weight,
    }
}

/// Reset every entry to `Pending` (a full re-run). Preserves structural fields
/// (spec_file, priority, spec_name) but clears execution status and errors.
pub fn reset_all_to_pending(queue: &mut ProjectQueue) {
    for e in &mut queue.entries {
        e.state = SpecState::Pending;
        e.last_run_at = None;
        e.run_id = None;
        e.error = None;
    }
    queue.updated_at = crate::queue::now_iso();
}

/// Re-queue only `Failed` entries as `Pending`.
pub fn retry_failed_only(queue: &mut ProjectQueue) {
    for e in &mut queue.entries {
        if e.state == SpecState::Failed {
            e.state = SpecState::Pending;
            e.error = None;
        }
    }
    queue.updated_at = crate::queue::now_iso();
}

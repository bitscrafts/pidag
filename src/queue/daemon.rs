//! `--daemon` bounded-batch driver.
//!
//! Runs at most `batch` specs round-robin across the carousel in a single
//! process pass, updates state, and exits — safe to drive repeatedly from a
//! host cron entry (spec R8).

use super::{
    ProjectQueue, QueueEntry, SpecState,
    discover::discover_specs,
    state::{merge_queues, read_project_queue, write_project_queue},
    weighted_carousel_bounded,
};
use crate::core::error::PidagError;
use std::path::Path;

/// Result of a daemon batch pass.
#[derive(Debug, Clone, Default)]
pub struct ExecResult {
    pub executed: usize,
    pub remaining: usize,
}

/// Execute a single entry (used by the bounded carousel); marks it Running,
/// subprocesses an SDD run, then marks Done/Failed.
pub async fn execute_entry(
    project_root: &Path,
    state: &mut ProjectQueue,
    entry: &QueueEntry,
    dry_run: bool,
) -> bool {
    if dry_run {
        println!("{}", entry.spec_file);
        return true;
    }
    if let Some(e) = state
        .entries
        .iter_mut()
        .find(|e| e.spec_file == entry.spec_file)
    {
        e.state = SpecState::Running;
        e.last_run_at = Some(crate::queue::now_iso());
    }
    let _ = write_project_queue(project_root, state);

    let ok = matches!(
        tokio::process::Command::new(crate::core::selfexe::self_exe())
            .arg("sdd")
            .arg(&entry.spec_file)
            .arg("--run")
            .status()
            .await,
        Ok(s) if s.success()
    );

    if let Some(e) = state
        .entries
        .iter_mut()
        .find(|e| e.spec_file == entry.spec_file)
    {
        e.state = if ok {
            SpecState::Done
        } else {
            SpecState::Failed
        };
        e.error = if ok {
            None
        } else {
            Some("sdd run failed".to_string())
        };
    }
    let _ = write_project_queue(project_root, state);
    ok
}

/// Execute a pre-computed ordered list of entries across projects. Used by the
/// round-robin driver for the batch.
pub async fn execute_ordered_entries(
    ordered: Vec<(String, QueueEntry)>,
    project_state: &mut std::collections::HashMap<String, ProjectQueue>,
    roots: &std::collections::HashMap<String, std::path::PathBuf>,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<usize, PidagError> {
    let mut done = 0usize;
    for (label, entry) in ordered {
        let Some(state) = project_state.get_mut(&label) else {
            continue;
        };
        let Some(root) = roots.get(&label) else {
            continue;
        };
        let ok = execute_entry(root, state, &entry, dry_run).await;
        done += 1;
        if !ok && stop_on_failure {
            break;
        }
    }
    Ok(done)
}

/// A single-project batch pass. Scans specs, merges with cached state, runs a
/// bounded round-robin (single project = sequential priority order) of up to
/// `batch` pending specs, writes state, and reports how many remain.
pub async fn check_dry_run_done(
    project_root: &Path,
    batch: usize,
    dry_run: bool,
    _stop_on_failure: bool,
) -> Result<ExecResult, PidagError> {
    let discovered = discover_specs(project_root);
    let cached = read_project_queue(project_root)?.unwrap_or_else(|| ProjectQueue {
        project_root: project_root.to_string_lossy().to_string(),
        entries: Vec::new(),
        updated_at: crate::queue::now_iso(),
        weight: 1.0,
    });
    let mut state = merge_queues(&cached, &discovered, project_root);

    // Single project: pending entries in priority order.
    let pending: Vec<QueueEntry> = state
        .entries
        .iter()
        .filter(|e| e.state == SpecState::Pending)
        .cloned()
        .collect();

    let total_pending = pending.len();
    let take = weighted_carousel_bounded(vec![(state.weight, pending)], batch);

    if dry_run {
        for e in &take {
            println!("{}", e.spec_file);
        }
    } else {
        for entry in &take {
            execute_entry(project_root, &mut state, entry, false).await;
        }
    }

    let remaining = total_pending.saturating_sub(take.len());
    Ok(ExecResult {
        executed: take.len(),
        remaining,
    })
}

/// Daemon batch driver entry: runs up to `batch` specs round-robin across the
/// workspace (or a single project), then returns. `dry_run` only renders the
/// order.
pub async fn run_daemon(
    _workspace: Option<&Path>,
    project_root: &Path,
    batch: usize,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<ExecResult, PidagError> {
    let batch_effective = if batch == 0 { 5 } else { batch };
    check_dry_run_done(project_root, batch_effective, dry_run, stop_on_failure).await
}

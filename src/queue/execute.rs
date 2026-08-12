//! Queue execution: single-project run + multi-project carousel interleave.
//!
//! The carousel produces a round-robin order across projects' pending specs
//! (A/01, B/01, A/02, B/02, ...). Execution of each spec subprocesses to
//! `pidag sdd <spec.md> --run` (isolation + backward compat, spec R2/R12).

use super::{ProjectQueue, QueueEntry, SpecState};
use crate::core::error::PidagError;
use std::path::Path;

/// Outcome of a single-queue `--run` pass.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Round-robin interleave over a list of per-project pending-entry vectors.
///
/// Each inner vector is assumed to be pre-sorted by priority. Projects with no
/// pending entries are skipped without stalling the carousel.
pub fn carousel_interleave(mut projects: Vec<Vec<QueueEntry>>) -> Vec<QueueEntry> {
    let mut order = Vec::new();
    let mut any_left = true;
    while any_left {
        any_left = false;
        for project in projects.iter_mut() {
            if !project.is_empty() {
                order.push(project.remove(0));
                any_left = true;
            }
        }
    }
    order
}

/// Round-robin interleave, stopping once the batch limit is reached.
pub fn carousel_bounded(mut projects: Vec<Vec<QueueEntry>>, batch: usize) -> Vec<QueueEntry> {
    let mut order = Vec::new();
    let mut any_left = true;
    while any_left && order.len() < batch {
        any_left = false;
        for project in projects.iter_mut() {
            if order.len() >= batch {
                break;
            }
            if !project.is_empty() {
                order.push(project.remove(0));
                any_left = true;
            }
        }
    }
    order
}

/// Weighted round-robin interleave, bounded by `batch`, where each project's
/// per-sweep slot count is scaled by its `weight` (spec-11).
///
/// `projects` is `Vec<(weight, pending_entries)>` with each inner vector
/// pre-sorted by priority. The global `batch` is a HARD cap: the returned
/// order never exceeds `batch` entries. Projects with weight 0.0 or no
/// pending entries are skipped during the weighted sweep. When all weights
/// are `1.0` the emitted order matches [`carousel_bounded`] (parity tested).
///
/// Allocation (single sweep):
/// - For each project in round-robin order, share = max(1, round(remaining *
///   w_i / total_weight)) when w_i > 0; take min(share, pending.len(),
///   remaining) contiguous entries.
/// - After the weighted sweep, if budget remains, distribute leftovers one
///   per still-pending project (anti-starvation for low-weight projects).
pub fn weighted_carousel_bounded(
    mut projects: Vec<(f64, Vec<QueueEntry>)>,
    batch: usize,
) -> Vec<QueueEntry> {
    let mut order = Vec::new();
    if batch == 0 {
        return order;
    }

    // Drain each project's pending entries; keep only weight>0 non-empty ones
    // for the weighted sweep (a weight-0.0 project starves by design).
    let mut weighted: Vec<(f64, Vec<QueueEntry>)> = projects
        .iter_mut()
        .map(|(w, entries)| (*w, std::mem::take(entries)))
        .filter(|(w, entries)| *w > 0.0 && !entries.is_empty())
        .collect();

    let total_weight: f64 = weighted.iter().map(|(w, _)| w).sum();
    if total_weight <= 0.0 {
        return order;
    }

    // R4 byte-for-byte parity with carousel_bounded when ALL weights are 1.0:
    // collapse to the flat 1-each round-robin so legacy/untuned queues see
    // bit-identical ordering (no behavioral regression guardrail).
    if weighted.iter().all(|(w, _)| (*w - 1.0).abs() < 1e-9) {
        let flat: Vec<Vec<QueueEntry>> = weighted.into_iter().map(|(_, e)| e).collect();
        return carousel_bounded(flat, batch);
    }

    // Track each project's initial pending count (before the sweep drains
    // it) so the anti-starvation tail can prefer the LEAST-served project
    // (lowest taken/initial ratio) instead of blindly rotating A-first, which
    // would otherwise give the leftover slot to the first-listed project even
    // when a lighter-weight project is under-served (spec-11 fairness TODO).
    let initial_pending: Vec<usize> = weighted.iter().map(|(_, e)| e.len()).collect();
    let mut taken: Vec<usize> = vec![0; weighted.len()];
    let mut remaining = batch;

    // Weighted sweep: one block per project sized by weighted share.
    for (i, (weight, entries)) in weighted.iter_mut().enumerate() {
        if remaining == 0 {
            break;
        }
        let share = ((remaining as f64) * (*weight) / total_weight).round() as usize;
        let share = share.max(1).min(remaining);
        let take = share.min(entries.len());
        if take == 0 {
            continue;
        }
        let drained: Vec<QueueEntry> = entries.drain(..take).collect();
        for e in drained {
            order.push(e);
        }
        taken[i] += take;
        remaining -= take;
        if remaining == 0 {
            break;
        }
    }

    // Anti-starvation tail: distribute leftovers one slot at a time to the
    // LEAST-served project that still has pending entries, where served-ratio
    // = taken[i] / initial_pending[i] (ties broken by lower weight, then by
    // index order). This makes the leftover go to the under-served light
    // project; if all served-ratios are equal it falls back to round-robin.
    while remaining > 0 {
        // Pick the index of the least-served project with pending entries.
        let mut best: Option<usize> = None;
        let mut best_ratio = f64::INFINITY;
        for (i, (weight, entries)) in weighted.iter().enumerate() {
            if entries.is_empty() {
                continue;
            }
            let init = initial_pending[i].max(1) as f64;
            let ratio = (taken[i] as f64) / init;
            // Tie-break: lower weight first (give the slot to the project that
            // needs it more), then lower index for determinism.
            let better = match best {
                None => true,
                Some(b) => {
                    let bw = weighted[b].0;
                    ratio < best_ratio || (ratio == best_ratio && *weight < bw)
                }
            };
            if better {
                best = Some(i);
                best_ratio = ratio;
            }
        }
        let Some(i) = best else { break };
        order.push(weighted[i].1.remove(0));
        taken[i] += 1;
        remaining -= 1;
    }

    order
}

/// Ordered list of pending specs for round-robin. Takes `Vec<(label, Vec<entry>)>`.
pub fn round_robin_order(projects: Vec<(String, Vec<QueueEntry>)>) -> Vec<(String, QueueEntry)> {
    let mut labeled: Vec<Vec<(String, QueueEntry)>> = projects
        .into_iter()
        .map(|(label, entries)| {
            entries
                .into_iter()
                .map(|e| (label.clone(), e))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut order = Vec::new();
    let mut any_left = true;
    while any_left {
        any_left = false;
        for project in labeled.iter_mut() {
            if !project.is_empty() {
                let (label, entry) = project.remove(0);
                order.push((label, entry));
                any_left = true;
            }
        }
    }
    order
}

/// Execute the entries already returned by the carousel, subprocessing each to
/// `pidag sdd <spec.md> --run` and updating the queue state. `dry_run` renders
/// the order without spawning SDD runs.
pub async fn run_queue(
    project_root: &Path,
    entries: &[QueueEntry],
    state: &mut ProjectQueue,
    dry_run: bool,
    stop_on_failure: bool,
) -> Result<RunOutcome, PidagError> {
    let mut outcome = RunOutcome::default();

    for entry in entries {
        if state
            .entries
            .iter()
            .any(|e| e.spec_file == entry.spec_file && e.state == SpecState::Done)
        {
            outcome.skipped += 1;
            continue;
        }

        if dry_run {
            println!("{}", entry.spec_file);
            continue;
        }

        // Mark running.
        if let Some(e) = state
            .entries
            .iter_mut()
            .find(|e| e.spec_file == entry.spec_file)
        {
            e.state = SpecState::Running;
            e.last_run_at = Some(crate::queue::now_iso());
        }
        super::state::write_project_queue(project_root, state)?;

        let status = run_sdd(project_root, entry).await;
        let ok = status;

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
        super::state::write_project_queue(project_root, state)?;

        if ok {
            outcome.completed += 1;
        } else {
            outcome.failed += 1;
            if stop_on_failure {
                break;
            }
        }
    }

    Ok(outcome)
}

/// Subprocess to `pidag sdd <spec.md> --run`. Returns true on success.
async fn run_sdd(_project_root: &Path, entry: &QueueEntry) -> bool {
    let status = tokio::process::Command::new(crate::core::selfexe::self_exe())
        .arg("sdd")
        .arg(&entry.spec_file)
        .arg("--run")
        .status()
        .await;
    match status {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}

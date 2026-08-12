//! Checkpoint and resume logic for interrupted SDD runs (Spec-08).
//!
//! When `pidag sdd spec.md --run` is interrupted, this module enables resuming
//! from the last successfully completed node rather than restarting from scratch.
//!
//! Key concepts:
//! - **Run ID**: Derived from hash(spec_path + spec_content) for deterministic
//!   association. Same spec = same run_id = resume enabled. Edited spec = new run_id.
//! - **Checkpoint**: Loaded from vault on startup; contains terminal node states.
//! - **ResumeDecision**: Guides startup behavior (Fresh/AlreadyDone/Resume).

use crate::core::error::PidagError;
use crate::scheduler::{NodeStatus, RunReport};
use crate::store::Store;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Checkpoint state loaded from vault on resume (Spec-08).
///
/// Canonical home is `crate::scheduler::Checkpoint` (avoids a
/// `sdd -> scheduler -> sdd` module cycle). Re-exported here so existing
/// `pidag::sdd::Checkpoint` import paths keep working. Contains node states
/// from a prior (incomplete) run, used to skip pre-completed nodes.
pub use crate::scheduler::Checkpoint;

/// Resume decision made at startup.
#[derive(Debug, Clone)]
pub enum ResumeDecision {
    /// No prior run found; start fresh.
    Fresh { run_id: String },
    /// Prior run completed; show cached report.
    AlreadyDone { run_id: String, report: RunReport },
    /// Prior run incomplete; resume from checkpoint.
    Resume { checkpoint: Checkpoint },
}

/// Derive run ID from spec file path + spec content hash (SHA-256, truncated to 12 hex).
///
/// Returns a 12-character hex string that deterministically represents this spec.
/// Same spec path + content = same run_id. Edited spec = new run_id (intentional).
///
/// # Arguments
///
/// * `spec_path` - Path to the spec file (used in hash for determinism)
/// * `spec_content` - Full content of the spec file
///
/// # Returns
///
/// A 12-character hex string (run_id)
pub fn run_id_for_spec(spec_path: &Path, spec_content: &str) -> String {
    // Combine spec path and content for deterministic hashing
    let spec_str = format!("{}{}", spec_path.display(), spec_content);

    // Compute SHA-256 hash
    let mut hasher = Sha256::new();
    hasher.update(spec_str.as_bytes());
    let digest = hasher.finalize();

    // Convert to hex and truncate to 12 chars
    let hex = format!("{:x}", digest);
    hex[..12].to_string()
}

/// Load checkpoint from vault based on run_id.
///
/// Checks if a prior run exists for this run_id and:
/// - If completed_at is set: return AlreadyDone
/// - If completed_at is None: return Resume with checkpoint
/// - If no prior run: return Fresh
///
/// # Arguments
///
/// * `store` - Persistent store (redb vault)
/// * `run_id` - Run identifier derived from spec hash
/// * `retry_failed` - If true, reset Failed nodes to Pending for retry
///
/// # Returns
///
/// ResumeDecision guiding startup behavior
pub async fn load_checkpoint(
    store: &dyn Store,
    run_id: &str,
    retry_failed: bool,
) -> Result<ResumeDecision, PidagError> {
    // Check if run exists in vault
    let run_opt = store.get_run(run_id).await?;

    let run = match run_opt {
        Some(r) => r,
        None => {
            // No prior run; start fresh
            return Ok(ResumeDecision::Fresh {
                run_id: run_id.to_string(),
            });
        }
    };

    // If run is already completed, return AlreadyDone
    if run.completed_at.is_some() {
        // Generate a RunReport from the terminal set
        let terminal_set = store.terminal_set(run_id).await?;
        let node_states = terminal_set
            .iter()
            .map(|(node_id, state)| crate::scheduler::NodeState {
                node_id: node_id.clone(),
                state: NodeStatus::parse(state).unwrap_or(NodeStatus::Pending),
                model: None,
                attempts: 1,
                output: None,
            })
            .collect();

        let report = RunReport {
            node_states,
            failed: terminal_set
                .iter()
                .filter_map(|(id, state)| {
                    if state == "Failed" {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            store_write_failures: 0,
        };

        return Ok(ResumeDecision::AlreadyDone {
            run_id: run_id.to_string(),
            report,
        });
    }

    // Run is incomplete; build checkpoint from terminal set and node list
    let terminal_set = store.terminal_set(run_id).await?;
    let all_nodes = store.list_nodes(run_id).await?;

    let mut completed_nodes = HashSet::new();
    let mut failed_nodes = HashSet::new();
    let mut blocked_nodes = HashSet::new();
    let mut stale_running = HashSet::new();
    let mut outputs: HashMap<String, String> = HashMap::new();

    // Categorize nodes based on state and hydrate outputs (I7)
    for (node_id, state) in terminal_set {
        match state.as_str() {
            "Done" => {
                completed_nodes.insert(node_id.clone());
                // Hydrate output from vault for interpolation (I7)
                if let Ok(Some(output)) = store.get_artifact(run_id, &node_id).await {
                    outputs.insert(node_id, output);
                }
            }
            "Failed" => {
                if retry_failed {
                    // With --retry-failed, reset Failed to Pending (will be retried)
                    // But still track it for reference
                } else {
                    // Keep it in failed_nodes to mark as terminal
                    failed_nodes.insert(node_id.clone());
                    // Hydrate output from vault for interpolation (I7, I2)
                    if let Ok(Some(output)) = store.get_artifact(run_id, &node_id).await {
                        outputs.insert(node_id, output);
                    }
                }
            }
            "Blocked" => {
                blocked_nodes.insert(node_id.clone());
                // Hydrate output from vault for interpolation (I7)
                if let Ok(Some(output)) = store.get_artifact(run_id, &node_id).await {
                    outputs.insert(node_id, output);
                }
            }
            _ => {}
        }
    }

    // Check for stale Running nodes (crashed while running)
    for node in all_nodes {
        if node.state == NodeStatus::Running {
            stale_running.insert(node.node_id);
        }
    }

    Ok(ResumeDecision::Resume {
        checkpoint: Checkpoint {
            run_id: run_id.to_string(),
            completed_nodes,
            failed_nodes,
            blocked_nodes,
            stale_running,
            outputs,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_id_is_deterministic() {
        let path = Path::new("/test/spec.md");
        let content = "test content";

        let id1 = run_id_for_spec(path, content);
        let id2 = run_id_for_spec(path, content);

        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 12);
    }

    #[test]
    fn test_run_id_differs_for_different_content() {
        let path = Path::new("/test/spec.md");

        let id1 = run_id_for_spec(path, "content 1");
        let id2 = run_id_for_spec(path, "content 2");

        assert_ne!(id1, id2);
    }

    #[test]
    fn test_run_id_differs_for_different_path() {
        let content = "test";

        let id1 = run_id_for_spec(Path::new("/test1/spec.md"), content);
        let id2 = run_id_for_spec(Path::new("/test2/spec.md"), content);

        assert_ne!(id1, id2);
    }
}

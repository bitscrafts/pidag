//! pidag queue — carousel priority queue with round-robin scheduling.
//!
//! Implements the single-project queue (`specs/` NN-priority scan, state
//! persistence for crash recovery) and the multi-project round-robin carousel
//! (`--workspace`). State persists to `.pidag/queue.json` with atomic
//! temp+rename writes.
//!
//! ## Data flow
//!
//! ```text
//! discover_specs() -> Vec<QueueEntry>           (lazy scan, NN priority)
//!        |
//!        v
//! merge_queues(cached, discovered) -> ProjectQueue   (preserve Done)
//!        |
//!        v
//! write_project_queue() / read_project_queue()       (atomic JSON)
//!        |
//!        v
//! carousel_interleave() / carousel_bounded()         (round-robin order)
//! ```

pub mod daemon;
pub mod discover;
pub mod execute;
pub mod state;

pub use daemon::{
    ExecResult, check_dry_run_done, execute_entry, execute_ordered_entries, run_daemon,
};
pub use discover::{
    backup_queue_if_needed, discover_projects, discover_specs, extract_priority,
    render_status_table,
};
pub use execute::{
    RunOutcome, carousel_bounded, carousel_interleave, round_robin_order, run_queue,
    weighted_carousel_bounded,
};
pub use state::{
    merge_queues, read_project_queue, reset_all_to_pending, retry_failed_only, state_file_path,
    write_project_queue,
};

use serde::{Deserialize, Serialize};

/// Execution state of a single spec in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpecState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

/// A single spec tracked by the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    /// Spec name without extension, e.g. `"01-fibonacci"`.
    pub spec_name: String,
    /// Spec file path relative to project root, e.g. `"specs/01-fibonacci.md"`.
    pub spec_file: String,
    /// Current execution state.
    pub state: SpecState,
    /// NN-prefix priority (1..=99).
    pub priority: u8,
    /// RFC3339 timestamp of the last run, if any.
    pub last_run_at: Option<String>,
    /// Detached run id of the last scheduled execution, if any.
    pub run_id: Option<String>,
    /// Last error message, if in a failed state.
    pub error: Option<String>,
}

/// Persistent project queue state stored at `.pidag/queue.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectQueue {
    /// Absolute, normalized project root.
    pub project_root: String,
    /// Queue entries, lazily refreshed from the filesystem each invocation.
    pub entries: Vec<QueueEntry>,
    /// RFC3339 timestamp of the last state write.
    pub updated_at: String,
    /// Relative scheduling weight (default 1.0). Higher-weight projects yield
    /// more turns per carousel batch. Unknown in older queue files -> 1.0.
    #[serde(default = "default_weight")]
    pub weight: f64,
}

/// Default project weight used when a queue file has no `weight` field
/// (forward-compat for queues written before weighted scheduling existed).
fn default_weight() -> f64 {
    1.0
}

/// RFC3339 timestamp used to stamp state writes.
pub(crate) fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

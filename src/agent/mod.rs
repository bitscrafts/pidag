//! Autonomous spec driver (`pidag auto`).
//!
//! Self-improvement / research loop driven by the flexible DAG and the
//! project's `HANDOFF.md`:
//!
//! 1. Pick a project from a list (workspace discovery).
//! 2. Read `HANDOFF.md` to learn the current phase + next steps.
//! 3. Choose a target: a pending spec to implement, OR a research/improvement
//!    direction named in the handoff (the flexible DAG expresses any of these).
//! 4. Commit-before-modify: snapshot the working tree so any change is
//!    rollbackable.
//! 5. Dispatch a `pi` agent to accomplish the target; it commits its changes
//!    and updates `HANDOFF.md` when done.
//! 6. Record the outcome in the queue state and exit — safe to drive repeatedly
//!    from a host cron entry.

pub mod git;
pub mod handoff;
pub mod lock;
pub mod run;
pub mod select;
pub mod splitter;

pub use lock::PidLock;
pub use run::{AutoOptions, AutoOutcome, auto_drive};
pub use splitter::{AutoSplitter, SpecSplitter, SplitChild};

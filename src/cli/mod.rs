//! pidag CLI command implementations
//!
//! This module organizes all subcommand logic. Each command is implemented
//! in its own module (run.rs, show.rs, etc.) and re-exported here for use
//! by the binary.

pub mod agent;
pub mod attach;
pub mod describe;
pub mod list;
pub mod mcp;
pub mod queue;
pub mod run;
pub mod sdd;
pub mod serve;
pub mod show;
pub mod ui;
pub mod workflows;

pub use self::agent::auto;
pub use self::attach::attach;
pub use self::describe::describe;
pub use self::list::list;
pub use self::mcp::mcp;
pub use self::queue::queue;
pub use self::run::run;
pub use self::sdd::sdd;
pub use self::serve::serve;
pub use self::show::show;
pub use self::ui::ui;
pub use self::workflows::workflows;
pub use self::workflows::{WorkflowsCommand, parse_args as workflows_parse_args};

pub mod split;
pub use self::split::split;

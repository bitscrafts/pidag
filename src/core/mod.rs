//! Core DAG types and error handling.

pub mod config;
pub mod dag;
pub mod error;
pub mod event;
pub mod selfexe;

// Re-export public types for convenient access
pub use config::{Config, ModelsConfig, SddConfig};
pub use dag::{Dag, McpCallConfig, ModelRef, Node, QuorumConfig, RetryPolicy, Verify};
pub use error::PidagError;
pub use event::{CompositeSink, Event, EventSink, JsonlSink, RedbSink, VecSink};

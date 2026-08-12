use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum PidagError {
    #[error("JSON parse error: {0}")]
    Parse(String),

    #[error("DAG contains a cycle")]
    Cycle,

    #[error("Unknown dependency in DAG")]
    UnknownDependency,

    #[error("No eligible model for this node")]
    NoEligibleModel,

    #[error("Validation failed: {0}")]
    Validation(String),

    #[error("Worker failed")]
    WorkerFailed,

    #[error("Worker spawn failed: {0}")]
    WorkerSpawn(String),

    #[error("Worker timeout")]
    Timeout,

    #[error("Store error: {0}")]
    Store(String),
}

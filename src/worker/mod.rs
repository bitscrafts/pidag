//! Worker trait and implementations for pidag.
//!
//! This module is the home of the [`Worker`] trait — the per-node execution
//! abstraction the scheduler calls — plus every concrete worker used by
//! [`TypeDispatchWorker`]:
//!
//! - [`PiPrintWorker`] shells out to the local `pi` CLI (`pi -p`).
//! - [`RealShellWorker`] runs `bash -c` for `node_type == "shell"` nodes.
//! - [`A2aWorker`] dispatches to any A2A-compliant remote agent via the
//!   `tasks/send` JSON-RPC protocol over `curl`.
//! - [`McpCallWorker`] dispatches to MCP servers via the MCP client.
//! - [`DelayMockWorker`] is the deterministic test double.
//! - [`AgentWorker`] adapts an `AgentBackend` (spec-21) to the worker trait.
//!
//! `TypeDispatchWorker` routes each node to the right worker based on its
//! `node_type` (shell vs. llm) and, for llm nodes, whether a backend is
//! configured (spec-21), the `ModelRef.name` URL prefix (`http://`/`https://`),
//! or `pi -p` via [`PiPrintWorker`].

pub mod agent;
pub mod pi_print;
mod shell;
mod type_dispatch;

// Re-export A2A worker from the a2a module
pub use crate::a2a::worker::{A2aWorker, is_a2a_endpoint};
// Re-export MCP call worker from the mcp module
pub use self::agent::AgentWorker;
pub use self::pi_print::{PiPrintWorker, parse_pi_envelope};
pub use self::shell::{RealShellWorker, ShellWorker};
pub use self::type_dispatch::TypeDispatchWorker;
pub use crate::mcp::call::McpCallWorker;

use crate::backend::PiBackend;
use crate::core::Config;
use crate::core::dag::Dag;
use crate::core::error::PidagError;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct WorkerOutput {
    pub success: bool,
    pub output: String,
    /// True => this failure is a transient provider signal (HTTP 429 / 503 /
    /// `quota_exceeded`, connection reset, ...). On `success == false &&
    /// retryable`, the scheduler skips remaining retries for this model and
    /// advances to the next `ModelRef` in the node's chain. Defaults to
    /// `false`; only `PiPrintWorker` and `A2aWorker` may set it `true`. See
    /// [`classify_retryable`].
    pub retryable: bool,
}

#[async_trait]
pub trait Worker: Send + Sync {
    async fn run(
        &self,
        node_id: &str,
        prompt: &str,
        model: &str,
        attempt: usize,
    ) -> Result<WorkerOutput, PidagError>;
}

/// Classify a worker failure as transient/retryable (HTTP 429 / 503 /
/// `quota_exceeded`, rate-limit strings, connection reset).
///
/// Used by [`PiPrintWorker`](crate::PiPrintWorker) and [`A2aWorker`] on their
/// `success == false` paths so the scheduler can distinguish "this model is
/// exhausted" from "this work failed" — and apply exponential backoff (NVIDIA
/// guidance strategy 1) before either retrying the same model or advancing to
/// the next `ModelRef` (strategy 3).
///
/// Matching is **case-insensitive** on the combined stdout+stderr text. See
/// `specs/93-runtime-429-failover.md` R2 and
/// `docs/claude-pi-delegation/nvidia-429-strategy.md`.
///
/// False positives (a successful response that happens to contain "429") are
/// benign — callers only consult the return value when
/// `WorkerOutput.success == false`.
pub(crate) fn classify_retryable(combined_text: &str) -> bool {
    let t = combined_text.to_ascii_lowercase();
    const SIGNALS: &[&str] = &[
        "429",
        "too many requests",
        "503",
        "service unavailable",
        "rate limit",
        "rate_limit",
        "rate-limit",
        "quota",
        "quota_exceeded",
        "connection reset",
        "connection reset by peer",
    ];
    SIGNALS.iter().any(|s| t.contains(s))
}

/// Deterministic, offline test double: sleeps a scripted duration (per node
/// id) before returning a canned success. Used to simulate slow nodes for
/// `await_dag`/`wait_any`/resume/timeout tests without any real process or
/// network dependency.
#[derive(Clone, Default)]
pub struct DelayMockWorker {
    delays: Arc<Mutex<HashMap<String, Duration>>>,
}

impl DelayMockWorker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script node `node_id` to sleep `delay` before returning success.
    pub fn set_delay(&self, node_id: &str, delay: Duration) {
        if let Ok(mut delays) = self.delays.lock() {
            delays.insert(node_id.to_string(), delay);
        }
    }
}

#[async_trait]
impl Worker for DelayMockWorker {
    async fn run(
        &self,
        node_id: &str,
        _prompt: &str,
        _model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        let delay = self
            .delays
            .lock()
            .ok()
            .and_then(|delays| delays.get(node_id).copied())
            .unwrap_or(Duration::from_millis(0));

        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        Ok(WorkerOutput {
            success: true,
            output: format!("output from {node_id}"),
            retryable: false,
        })
    }
}

/// Build a `TypeDispatchWorker` with optional backend routing.
///
/// This helper constructs a worker with the following logic:
/// 1. Resolve the backend name from env (via `PIDAG_AGENT_BACKEND`) or config.
/// 2. If a backend name is resolved:
///    - Match against registered backend names ("mock", "pi").
///    - If unknown, return an error listing registered names (spec-21 R6).
///    - Otherwise, construct the appropriate backend and attach it via `with_backend()`.
/// 3. If NO backend is configured, return a plain `TypeDispatchWorker::new()` (spec-21 N1),
///    preserving the default routing behavior and ensuring all existing tests pass.
///
/// # Arguments
/// - `dag`: The DAG to execute
/// - `timeout`: Worker timeout for RPC operations
/// - `config`: Configuration containing the optional `[agent] backend` name and project root
/// - `concurrency`: Scheduler concurrency, used to size the `PiBackend` client pool
///
/// # Returns
/// A boxed `TypeDispatchWorker` configured with a backend if one is selected, or a plain
/// worker using default routing (A2aWorker/PiPrintWorker).
pub fn build_worker(
    dag: &Dag,
    timeout: Duration,
    config: &Config,
    concurrency: usize,
) -> Result<Box<dyn Worker>, PidagError> {
    // Resolve the effective backend name (env override or config)
    let backend_name =
        crate::backend::registry::resolve_backend_name(config.agent.backend.as_deref());

    let worker = if let Some(name) = backend_name {
        // Match against registered backend names and construct the appropriate backend
        let backend: Arc<dyn crate::backend::AgentBackend> = match name.as_str() {
            "mock" => Arc::new(crate::backend::MockBackend::new()),
            "pi" => Arc::new(PiBackend::new(
                config.project.root.clone(),
                timeout,
                concurrency,
                config.worker.provider.clone(),
            )),
            _ => {
                // Unknown backend: list registered names (spec-21 R6)
                return Err(PidagError::Validation(format!(
                    "unknown backend '{}'; registered backends: mock, pi",
                    name
                )));
            }
        };

        // Construct TypeDispatchWorker with the backend attached
        let base_worker = TypeDispatchWorker::new(dag, timeout);
        base_worker.with_backend(backend, dag, timeout)
    } else {
        // No backend configured: use default routing (spec-21 N1)
        TypeDispatchWorker::new(dag, timeout)
    };

    Ok(Box::new(worker))
}

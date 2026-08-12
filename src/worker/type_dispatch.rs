//! [`TypeDispatchWorker`] — routes each DAG node to the correct worker
//! based on its `node_type` field and (for llm nodes) the `ModelRef.name`
//! URL prefix.

use super::agent::AgentWorker;
use super::pi_print::PiPrintWorker;
use super::shell::RealShellWorker;
use super::{Worker, WorkerOutput};
use crate::a2a::worker::{A2aWorker, is_a2a_endpoint};
use crate::core::dag::Dag;
use crate::core::error::PidagError;
use crate::mcp::call::McpCallWorker;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Type-dispatching worker: routes nodes to the correct worker based on
/// their `node_type` field and MCP call presence in the DAG.
///
/// - Node has `mcp_call` field => delegates to [`McpCallWorker`]
/// - `node_type == Some("shell")` => delegates to [`RealShellWorker`]
/// - `node_type == Some("llm")` or `node_type == None` or any unknown value:
///   - If an agent backend is configured => [`AgentWorker`] (routes through backend via spec-21)
///   - `ModelRef.name` starts with `http://`/`https://` => [`A2aWorker`]
///   - otherwise => [`PiPrintWorker`] (or configured backend if available)
///
/// This makes pidag a **multi-agent orchestrator**: a single DAG can mix
/// local `pi` CLI nodes, `bash` shell nodes, A2A remote-agent nodes, and
/// MCP tool-calling nodes, routed purely by the configuration — no
/// additional configuration changes required (backward-compatible with
/// existing DAG JSON).
pub struct TypeDispatchWorker {
    node_types: HashMap<String, Option<String>>,
    pi_worker: PiPrintWorker,
    shell_worker: RealShellWorker,
    a2a_worker: A2aWorker,
    mcp_worker: McpCallWorker,
    /// Optional agent worker used for LLM nodes when a backend is configured (spec-21).
    /// If present, LLM nodes route through this instead of the default routing.
    agent_worker: Option<Arc<AgentWorker>>,
}

impl TypeDispatchWorker {
    /// Create a new type-dispatching worker from a DAG.
    ///
    /// The `A2aWorker` is configured with a 2s default poll interval (per
    /// spec R4) and the same `timeout` as the pi/shell workers.
    ///
    /// This constructor maintains full backward compatibility: LLM nodes
    /// route to A2aWorker/RpcWorker/PiPrintWorker (no agent backend).
    pub fn new(dag: &Dag, timeout: Duration) -> Self {
        let node_types = dag
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.node_type.clone()))
            .collect();
        Self {
            node_types,
            pi_worker: PiPrintWorker::new(dag, timeout),
            shell_worker: RealShellWorker::new(dag, timeout),
            a2a_worker: A2aWorker::new(dag, timeout, Duration::from_secs(2)),
            mcp_worker: McpCallWorker::new(dag),
            agent_worker: None,
        }
    }

    /// Create with custom command override for the pi worker (testing).
    ///
    /// The `A2aWorker` is constructed with the real `curl` binary (no shim),
    /// so tests that only exercise the pi path never hit the A2A branch.
    pub fn with_pi_command(
        dag: &Dag,
        timeout: Duration,
        program: String,
        extra_args: Vec<String>,
    ) -> Self {
        let node_types = dag
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.node_type.clone()))
            .collect();
        Self {
            node_types,
            pi_worker: PiPrintWorker::with_command(timeout, program, extra_args),
            shell_worker: RealShellWorker::new(dag, timeout),
            a2a_worker: A2aWorker::new(dag, timeout, Duration::from_secs(2)),
            mcp_worker: McpCallWorker::new(dag),
            agent_worker: None,
        }
    }

    /// Create with custom command overrides for BOTH the pi worker and the
    /// A2A worker (testing). This lets tests shim both the pi CLI and the
    /// curl binary with harmless shell commands, keeping the suite fully
    /// offline and deterministic.
    ///
    /// - `pi_program` + `pi_args` replace the `pi` binary.
    /// - `a2a_program` + `a2a_args` replace the `curl` binary.
    /// - `poll_interval` controls the A2A poll loop interval.
    pub fn with_pi_and_a2a_command(
        dag: &Dag,
        timeout: Duration,
        pi_program: String,
        pi_args: Vec<String>,
        a2a_program: String,
        a2a_args: Vec<String>,
        poll_interval: Duration,
    ) -> Self {
        let node_types = dag
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.node_type.clone()))
            .collect();
        Self {
            node_types,
            pi_worker: PiPrintWorker::with_command(timeout, pi_program, pi_args),
            shell_worker: RealShellWorker::new(dag, timeout),
            a2a_worker: A2aWorker::with_command(timeout, poll_interval, a2a_program, a2a_args),
            mcp_worker: McpCallWorker::new(dag),
            agent_worker: None,
        }
    }

    /// Set the agent backend for this worker (optional).
    /// If set, LLM nodes will route through the backend instead of the default routing.
    pub fn with_backend(
        mut self,
        backend: Arc<dyn crate::backend::AgentBackend>,
        dag: &Dag,
        timeout: Duration,
    ) -> Self {
        self.agent_worker = Some(Arc::new(AgentWorker::new(backend, dag, timeout)));
        self
    }
}

#[async_trait]
impl Worker for TypeDispatchWorker {
    async fn run(
        &self,
        node_id: &str,
        prompt: &str,
        model: &str,
        attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        // Check MCP call first (highest priority)
        if self.mcp_worker.is_mcp_node(node_id) {
            return self.mcp_worker.run(node_id, prompt, model, attempt).await;
        }

        let node_type = self
            .node_types
            .get(node_id)
            .and_then(|opt| opt.as_ref())
            .map(String::as_str);

        match node_type {
            Some("shell") => self.shell_worker.run(node_id, prompt, model, attempt).await,
            // "llm", None, or any unknown value: route by URL prefix (or agent backend if configured).
            _ => {
                // If an agent backend is configured, route LLM nodes through it.
                if let Some(agent) = &self.agent_worker {
                    agent.run(node_id, prompt, model, attempt).await
                } else if is_a2a_endpoint(model) {
                    self.a2a_worker.run(node_id, prompt, model, attempt).await
                } else {
                    self.pi_worker.run(node_id, prompt, model, attempt).await
                }
            }
        }
    }
}

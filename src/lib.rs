//! pidag - Deterministic, resilient multi-node LLM DAG executor

pub mod a2a;
pub mod agent;
pub mod backend;
pub mod cli;
pub mod core;
pub mod mcp;
pub mod queue;
pub mod rpc;
pub mod scheduler;
pub mod sdd;
pub mod split;
pub mod store;
pub mod ui;
pub mod worker;
pub mod workflow;

// Re-export core types and modules for backward compatibility
pub use core::config;
pub use core::selfexe::self_exe;
pub use core::{
    CompositeSink, Config, Dag, Event, EventSink, JsonlSink, McpCallConfig, ModelRef, ModelsConfig,
    Node, PidagError, RedbSink, RetryPolicy, SddConfig, VecSink, Verify,
}; // Re-export the config module for backward compatibility (e.g., config::ENV_DEFAULT_MODEL)

// Re-export scheduler and store types
pub use scheduler::NodeStatus;

// Re-export backend types for backward compatibility
pub use backend::mock::MockCapabilities;
pub use backend::{
    AgentBackend, AgentCapabilities, AgentReply, AgentSession, BackendRegistry, MockBackend,
    PiBackend, SessionSpec, ThinkingLevel, TokenUsage,
};

/// Split a model string into `(provider, model)`.
///
/// Accepts the forms `provider/model` (slash) and `provider:model` (colon).
/// A bare model name (no separator) yields `(None, name)` — used by the worker
/// to decide whether to pass `--provider` to `pi` (spec-13, P13).
///
/// Examples:
/// - `"google/gemini-3.6-flash"`  -> `(Some("google"), "gemini-3.6-flash")`
/// - `"nvidia:z-ai/glm-5.2"`      -> `(Some("nvidia"), "z-ai/glm-5.2")`
/// - `"deepseek-chat"`             -> `(None, "deepseek-chat")`
pub fn split_provider_model(s: &str) -> (Option<&str>, String) {
    let s = s.trim();
    // Colon form `provider:model` — the whole remainder (which may itself
    // contain a `/`, e.g. `nvidia:z-ai/glm-5.2`) is the model. Check colon
    // FIRST so a model path with `/` isn't split incorrectly.
    if let Some((p, m)) = s.split_once(':') {
        return (Some(p.trim()), m.trim().to_string());
    }
    if let Some((p, m)) = s.split_once('/') {
        return (Some(p.trim()), m.trim().to_string());
    }
    (None, s.to_string())
}

// Re-export UI types
pub use agent::AutoOptions;
pub use ui::{render::render_dag_mermaid, render::render_status};

pub use rpc::RpcServer;
pub use scheduler::{AwaitOutcome, NodeState, ResumeToken, RunReport, Scheduler};
pub use sdd::{
    Checkpoint, ResumeDecision, SddGenerator, SpecError, load_checkpoint, run_id_for_spec,
    validate_spec_name,
};
pub use store::{MockStore, NodeRecord, NodeTiming, RedbStore, RedbStorePool, RunMeta, Store};
pub use worker::pi_print::resolve_pi_binary;
pub use worker::{
    A2aWorker, AgentWorker, DelayMockWorker, PiPrintWorker, RealShellWorker, ShellWorker,
    TypeDispatchWorker, Worker, WorkerOutput, is_a2a_endpoint,
};

// Re-export MCP module items for backward compatibility
pub use mcp::{McpServer, run_mcp_server};

// Re-export A2A module items for backward compatibility
pub use a2a::{
    A2aServerState, AgentCard, AuthConfig, router as a2a_router,
    router_for_test as a2a_router_for_test,
};

// Re-export workflow module items
pub use workflow::{TemplateContext, WorkflowEngine};

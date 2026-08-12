//! Agent backend abstraction for pluggable LLM orchestration.
//!
//! This module defines the trait-based seam that allows pidag to orchestrate
//! multiple coding agents without being hardwired to any particular one.
//! The abstraction is vendor-neutral: all vocabulary is pidag's own, and
//! backends are selected purely by name via config + env override.

pub mod capabilities;
pub mod mock;
pub mod pi;
pub mod registry;

pub use capabilities::AgentCapabilities;
pub use mock::MockBackend;
pub use pi::{PiBackend, split_provider_model};
pub use registry::BackendRegistry;

pub use crate::core::dag::ModelRef;
use crate::core::error::PidagError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Thinking/reasoning level, independent of any backend's internal representation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ThinkingLevel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// Session specification: context for opening a new backend session.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub model: ModelRef,
    pub thinking_level: ThinkingLevel,
}

/// Token usage snapshot reported by a backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// Single turn in a backend event stream (optional, declared by capability).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// Text token from the model.
    TextDelta(String),
    /// Tool call initiated.
    ToolCall {
        tool_id: String,
        tool_name: String,
        arguments: String,
    },
    /// Tool result received.
    ToolResult { tool_id: String, result: String },
}

/// Reply from a backend session prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReply {
    /// The plain text response; carries no backend-specific JSON.
    pub text: String,
    /// Event stream if the backend supports `tool_events` capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<AgentEvent>>,
    /// Token usage if the backend supports `token_usage` capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

/// Error returned when a capability is unsupported by the backend.
#[derive(Debug, Clone)]
pub struct Unsupported;

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "capability unsupported by backend")
    }
}

impl std::error::Error for Unsupported {}

/// A live conversation session with a backend.
///
/// Mandatory methods:
/// - `prompt()` — send a prompt and get a reply
/// - `close()` — close the session
///
/// Optional methods (each gated by a declared capability, default impl returns Unsupported):
/// - `set_model()` — change the model mid-session (gated by `model_switch`)
/// - `set_thinking()` — set reasoning level (gated by `thinking_levels`)
/// - `fork()` — fork to a new independent branch (gated by `fork`)
/// - `compact()` — compact internal state (gated by `compact`)
/// - `usage()` — get accumulated token usage (gated by `token_usage`)
/// - `abort()` — cancel any pending operation (gated by `cancellation`)
#[async_trait]
pub trait AgentSession: Send + Sync {
    /// Send a prompt to the session and receive a reply.
    async fn prompt(&mut self, text: &str) -> Result<AgentReply, PidagError>;

    /// Close the session and release any held resources.
    async fn close(&mut self) -> Result<(), PidagError>;

    /// Change the model for this session (optional, gated by `model_switch`).
    /// Default implementation returns `Unsupported`.
    async fn set_model(
        &mut self,
        _model: &ModelRef,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(Unsupported))
    }

    /// Set the thinking/reasoning level (optional, gated by `thinking_levels`).
    /// Default implementation returns `Unsupported`.
    async fn set_thinking(
        &mut self,
        _level: ThinkingLevel,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(Unsupported))
    }

    /// Fork this session into a new independent branch (optional, gated by `fork`).
    /// Returns a new session that shares the prefix but can diverge.
    /// Default implementation returns `Unsupported`.
    async fn fork(
        &mut self,
    ) -> Result<Box<dyn AgentSession>, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(Unsupported))
    }

    /// Compact internal state (e.g., summarize context) (optional, gated by `compact`).
    /// Default implementation returns `Unsupported`.
    async fn compact(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(Unsupported))
    }

    /// Get accumulated token usage (optional, gated by `token_usage`).
    /// Default implementation returns `Unsupported`.
    async fn usage(&self) -> Result<TokenUsage, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(Unsupported))
    }

    /// Abort any pending operation (optional, gated by `cancellation`).
    /// Default implementation returns `Unsupported`.
    async fn abort(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(Unsupported))
    }
}

/// A backend (vendor) that can open sessions to orchestrate LLM work.
/// Backends are selected by name via config and env override (spec-92 priority chain).
#[async_trait]
pub trait AgentBackend: Send + Sync + std::fmt::Debug {
    /// The name of this backend (e.g., "pi", "mock", "claude-code").
    /// Used for config-based backend selection and logging.
    fn name(&self) -> &str;

    /// Declare the capabilities this backend actually implements.
    /// pidag uses this to degrade gracefully when a capability is missing.
    fn capabilities(&self) -> AgentCapabilities;

    /// Open a new session with the specified model and thinking level.
    /// Returns a boxed trait object that implements the session protocol.
    async fn open_session(&self, spec: SessionSpec) -> Result<Box<dyn AgentSession>, PidagError>;
}

//! MockBackend: an in-process, deterministic backend for testing.
//!
//! MockBackend is a second implementation that proves the abstraction is not
//! pi-shaped. It supports configurable capabilities and simple prompt echoing,
//! making it suitable for conformance tests and for validating R4 (graceful
//! degradation) without any subprocess or network dependency.

use crate::backend::{
    AgentBackend, AgentCapabilities, AgentReply, AgentSession, ModelRef, SessionSpec, ThinkingLevel,
};
use crate::core::error::PidagError;
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex;

/// Configuration for MockBackend capabilities.
#[derive(Debug, Clone, Default)]
pub struct MockCapabilities {
    pub sessions: bool,
    pub multi_turn: bool,
    pub model_switch: bool,
    pub thinking_levels: bool,
    pub fork: bool,
    pub compact: bool,
    pub token_usage: bool,
    pub cancellation: bool,
    pub tool_events: bool,
}

/// MockBackend: deterministic, in-process backend for testing.
#[derive(Debug)]
pub struct MockBackend {
    capabilities: MockCapabilities,
    /// If true, prompt() will fail with a transport error for testing error handling.
    fail_on_prompt: bool,
    /// spec-39: usage to attach to every `AgentReply` this backend's
    /// sessions produce. `None` means the reply carries no usage at all --
    /// which is a legitimate, distinct-from-`token_usage: false` scenario
    /// (B4b: a backend that *claims* the capability but fails to report on
    /// a given call). Defaults to `None` so every pre-spec-39 test (none of
    /// which cares about usage) is unaffected.
    usage_per_call: Option<crate::backend::TokenUsage>,
}

impl MockBackend {
    /// Create a new MockBackend with zero capabilities (minimal viable abstraction).
    pub fn new() -> Self {
        Self {
            capabilities: MockCapabilities::default(),
            fail_on_prompt: false,
            usage_per_call: None,
        }
    }

    /// Create a MockBackend with custom capabilities.
    pub fn with_capabilities(capabilities: MockCapabilities) -> Self {
        Self {
            capabilities,
            fail_on_prompt: false,
            usage_per_call: None,
        }
    }

    /// Set the capabilities of this backend.
    pub fn set_capabilities(&mut self, capabilities: MockCapabilities) {
        self.capabilities = capabilities;
    }

    /// Configure this backend to fail on prompt (for testing error handling).
    pub fn with_prompt_failure(mut self) -> Self {
        self.fail_on_prompt = true;
        self
    }

    /// spec-39: attach a fixed `TokenUsage` to every reply this backend's
    /// sessions produce (B1a, B3). Test-only knob -- a real backend reports
    /// whatever usage the provider actually returned, not a constant.
    pub fn with_usage_per_call(mut self, usage: crate::backend::TokenUsage) -> Self {
        self.usage_per_call = Some(usage);
        self
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentBackend for MockBackend {
    fn name(&self) -> &str {
        "mock"
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            sessions: self.capabilities.sessions,
            multi_turn: self.capabilities.multi_turn,
            model_switch: self.capabilities.model_switch,
            thinking_levels: self.capabilities.thinking_levels,
            fork: self.capabilities.fork,
            compact: self.capabilities.compact,
            token_usage: self.capabilities.token_usage,
            cancellation: self.capabilities.cancellation,
            tool_events: self.capabilities.tool_events,
        }
    }

    async fn open_session(&self, spec: SessionSpec) -> Result<Box<dyn AgentSession>, PidagError> {
        Ok(Box::new(MockSession {
            model: spec.model,
            thinking_level: spec.thinking_level,
            capabilities: self.capabilities.clone(),
            turn_count: Arc::new(Mutex::new(0)),
            closed: Arc::new(Mutex::new(false)),
            fail_on_prompt: self.fail_on_prompt,
            usage_per_call: self.usage_per_call.clone(),
        }))
    }
}

/// A mock session: stores model/thinking state and echoes prompts.
#[derive(Debug)]
pub struct MockSession {
    model: ModelRef,
    thinking_level: ThinkingLevel,
    capabilities: MockCapabilities,
    turn_count: Arc<Mutex<usize>>,
    closed: Arc<Mutex<bool>>,
    fail_on_prompt: bool,
    /// spec-39: see `MockBackend::usage_per_call`.
    usage_per_call: Option<crate::backend::TokenUsage>,
}

#[async_trait]
impl AgentSession for MockSession {
    async fn prompt(&mut self, text: &str) -> Result<AgentReply, PidagError> {
        if let Ok(closed) = self.closed.lock()
            && *closed
        {
            return Err(PidagError::WorkerFailed);
        }

        // If configured to fail, simulate a transport error
        if self.fail_on_prompt {
            return Err(PidagError::Parse(
                "transport error: 429 too many requests".to_string(),
            ));
        }

        // Increment turn count
        if let Ok(mut count) = self.turn_count.lock() {
            *count += 1;
        }

        // Echo the prompt with model and thinking level info
        let reply_text = format!(
            "MockBackend reply to '{}' (model: {}, thinking: {})",
            text,
            self.model.name,
            self.thinking_level.as_str()
        );

        Ok(AgentReply {
            text: reply_text,
            events: None,
            // spec-39 B1a/B3: attach the configured per-call usage, if any.
            // A backend that declares `token_usage` in its capabilities but
            // was NOT configured with `with_usage_per_call` still returns
            // `None` here -- exercising B4b (a capable backend that fails
            // to report on a given call) rather than papering over it.
            usage: self.usage_per_call.clone(),
        })
    }

    async fn close(&mut self) -> Result<(), PidagError> {
        if let Ok(mut closed) = self.closed.lock() {
            *closed = true;
        }
        Ok(())
    }

    async fn set_model(
        &mut self,
        model: &ModelRef,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.capabilities.model_switch {
            return Err(Box::new(crate::backend::Unsupported));
        }
        self.model = model.clone();
        Ok(())
    }

    async fn set_thinking(
        &mut self,
        level: ThinkingLevel,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.capabilities.thinking_levels {
            return Err(Box::new(crate::backend::Unsupported));
        }
        self.thinking_level = level;
        Ok(())
    }

    async fn fork(
        &mut self,
    ) -> Result<Box<dyn AgentSession>, Box<dyn std::error::Error + Send + Sync>> {
        if !self.capabilities.fork {
            return Err(Box::new(crate::backend::Unsupported));
        }
        Ok(Box::new(MockSession {
            model: self.model.clone(),
            thinking_level: self.thinking_level,
            capabilities: self.capabilities.clone(),
            turn_count: Arc::new(Mutex::new(0)),
            closed: Arc::new(Mutex::new(false)),
            fail_on_prompt: self.fail_on_prompt,
            usage_per_call: self.usage_per_call.clone(),
        }))
    }

    async fn compact(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.capabilities.compact {
            return Err(Box::new(crate::backend::Unsupported));
        }
        Ok(())
    }

    async fn usage(
        &self,
    ) -> Result<crate::backend::TokenUsage, Box<dyn std::error::Error + Send + Sync>> {
        if !self.capabilities.token_usage {
            return Err(Box::new(crate::backend::Unsupported));
        }
        Ok(crate::backend::TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
        })
    }

    async fn abort(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.capabilities.cancellation {
            return Err(Box::new(crate::backend::Unsupported));
        }
        Ok(())
    }
}

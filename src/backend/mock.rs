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
}

impl MockBackend {
    /// Create a new MockBackend with zero capabilities (minimal viable abstraction).
    pub fn new() -> Self {
        Self {
            capabilities: MockCapabilities::default(),
            fail_on_prompt: false,
        }
    }

    /// Create a MockBackend with custom capabilities.
    pub fn with_capabilities(capabilities: MockCapabilities) -> Self {
        Self {
            capabilities,
            fail_on_prompt: false,
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
            usage: None,
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

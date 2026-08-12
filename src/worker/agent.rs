//! [`AgentWorker`] — adapts an [`AgentBackend`] to the [`Worker`] trait.
//!
//! This worker bridges the gap between pidag's per-call `Worker` interface
//! and the session-oriented `AgentBackend` trait. It acquires a session,
//! prompts it with the node's text, and maps the reply to `WorkerOutput`.
//! Failures are returned as `Ok(WorkerOutput { success: false, .. })` with
//! proper retryability classification, following pidag's model-advancement
//! semantics (see spec-93, R2).
//!
//! R4 Graceful Degradation (spec-21):
//! - Consults backend capabilities before calling optional session methods.
//! - Only calls `set_thinking()` if `thinking_levels` capability is true.
//! - Degrades by skipping unsupported operations, not by panicking or silently
//!   no-oping: if `thinking_levels=false`, omits the `set_thinking` call
//!   entirely and proceeds with the backend's default thinking level.

use crate::backend::{AgentBackend, SessionSpec, ThinkingLevel};
use crate::core::dag::Dag;
use crate::core::error::PidagError;
use crate::worker::{Worker, WorkerOutput, classify_retryable};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Worker that routes LLM nodes through an `AgentBackend`.
pub struct AgentWorker {
    backend: Arc<dyn AgentBackend>,
    dag: Arc<Dag>,
    _timeout: Duration,
}

impl AgentWorker {
    /// Create a new agent worker backed by the given backend.
    pub fn new(backend: Arc<dyn AgentBackend>, dag: &Dag, timeout: Duration) -> Self {
        Self {
            backend,
            dag: Arc::new(dag.clone()),
            _timeout: timeout,
        }
    }
}

#[async_trait]
impl Worker for AgentWorker {
    async fn run(
        &self,
        node_id: &str,
        prompt: &str,
        model: &str,
        _attempt: usize,
    ) -> Result<WorkerOutput, PidagError> {
        // Find the node in the DAG to get its model list and paid flag.
        // The prompt is passed directly by the scheduler (already interpolated).
        let node = self
            .dag
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .ok_or_else(|| PidagError::Validation(format!("node {} not found in DAG", node_id)))?;

        // Look up the paid flag from the node's model list (money safety).
        // If the model is not in the node's list, error rather than defaulting to false.
        let model_ref = node
            .models
            .iter()
            .find(|m| m.name == model)
            .cloned()
            .ok_or_else(|| {
                PidagError::Validation(format!(
                    "node {}: model {} not in models list",
                    node_id, model
                ))
            })?;

        // Consult backend capabilities for R4 degradation (spec-21, R4).
        let caps = self.backend.capabilities();

        // Open a session with the specified model and default thinking level.
        let spec = SessionSpec {
            model: model_ref,
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = match self.backend.open_session(spec).await {
            Ok(s) => s,
            Err(e) => {
                // Session open failed — return as a soft failure with retryability classification
                let error_text = e.to_string();
                return Ok(WorkerOutput {
                    success: false,
                    output: format!("Failed to open session: {}", error_text),
                    retryable: classify_retryable(&error_text),
                });
            }
        };

        // R4: Apply thinking level if backend declares the capability.
        // If `thinking_levels=false`, skip this call entirely (graceful degradation).
        if caps.thinking_levels {
            // Backend declared thinking_levels capability; attempt to set it.
            // Ignore errors: backend may support the capability but reject the specific level.
            let _ = session.set_thinking(ThinkingLevel::Low).await;
        }

        // Send the prompt and get the reply.
        let reply = match session.prompt(prompt).await {
            Ok(r) => r,
            Err(e) => {
                // Prompt failed — return as a soft failure with retryability classification
                let error_text = e.to_string();
                let _ = session.close().await; // best-effort cleanup
                return Ok(WorkerOutput {
                    success: false,
                    output: format!("Prompt failed: {}", error_text),
                    retryable: classify_retryable(&error_text),
                });
            }
        };

        // Close the session (best-effort; ignore errors).
        let _ = session.close().await;

        // Map the successful reply to WorkerOutput.
        Ok(WorkerOutput {
            success: true,
            output: reply.text,
            retryable: false,
        })
    }
}

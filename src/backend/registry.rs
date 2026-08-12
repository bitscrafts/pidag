//! Backend registry: name-based backend selection with config + env override.
//!
//! Backends are selected at startup based on:
//! 1. `PIDAG_AGENT_BACKEND` env var (if set) — wins over config
//! 2. `[agent] backend` in config TOML
//! 3. No backend configured (normal case) — LLM nodes route to A2aWorker/PiPrintWorker as today
//!
//! An unknown backend name is a startup error listing the registered names.

use crate::backend::{AgentBackend, MockBackend};
use crate::core::error::PidagError;
use std::collections::HashMap;

/// Registry of available backends, indexed by name.
pub struct BackendRegistry {
    backends: HashMap<String, Box<dyn AgentBackend>>,
}

impl BackendRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    /// Register a backend by name. Any backend with the same name is replaced.
    pub fn register(&mut self, backend: Box<dyn AgentBackend>) {
        let name = backend.name().to_string();
        self.backends.insert(name, backend);
    }

    /// Look up a backend by name. Returns None if not found.
    pub fn get(&self, name: &str) -> Option<&dyn AgentBackend> {
        self.backends.get(name).map(|b| b.as_ref())
    }

    /// List the names of all registered backends.
    pub fn names(&self) -> Vec<&str> {
        self.backends.keys().map(|k| k.as_str()).collect()
    }

    /// Check if a backend is registered by name.
    pub fn contains(&self, name: &str) -> bool {
        self.backends.contains_key(name)
    }

    /// Resolve a backend by name, returning an error with registered names
    /// if the backend is not found.
    pub fn resolve(&self, name: &str) -> Result<&dyn AgentBackend, PidagError> {
        self.get(name).ok_or_else(|| {
            let registered = self.names();
            PidagError::Validation(format!(
                "unknown backend '{}'; registered backends: {}",
                name,
                registered.join(", ")
            ))
        })
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        // Register the MockBackend as the default.
        // Additional backends (pi, claude-code, etc.) are registered
        // when they become available.
        registry.register(Box::new(MockBackend::new()));
        registry
    }
}

/// Resolve the effective backend name from env (takes priority) or config.
/// Returns None if neither is set.
pub fn resolve_backend_name(config_backend_name: Option<&str>) -> Option<String> {
    // Env override takes priority (PIDAG_AGENT_BACKEND)
    if let Ok(env_name) = std::env::var("PIDAG_AGENT_BACKEND")
        && !env_name.is_empty()
    {
        return Some(env_name);
    }
    // Fall back to config-specified backend
    config_backend_name.map(|s| s.to_string())
}

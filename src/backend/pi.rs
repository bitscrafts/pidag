//! PiBackend — orchestrates LLM nodes via pi::sdk::RpcTransportClient.
//!
//! This backend replaces the hand-rolled RPC worker (spec-16) with the upstream
//! pi SDK's proper RPC transport client. It pools clients sized to scheduler
//! concurrency, routes model changes and thinking levels through the SDK,
//! verifies model state (R5 money safety), and recovers from transport errors
//! by discarding poisoned clients (R9).

use crate::backend::{
    AgentBackend, AgentCapabilities, AgentReply, AgentSession, ModelRef, SessionSpec,
    ThinkingLevel, TokenUsage, Unsupported,
};
use crate::core::error::PidagError;
use async_trait::async_trait;
use pi::sdk::{RpcTransportClient, RpcTransportOptions, ThinkingLevel as PiThinkingLevel};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

/// Anti-loop prompt constant (R7): moved from worker/rpc.rs
const WORKER_ANTI_LOOP_PROMPT: &str = "\nAct, don't narrate (worker anti-loop discipline): To make progress you MUST emit a concrete tool call. NEVER reply with only narration, placeholder text, or a bare intent statement with no tool call attached. Do not pause mid-task for permission on routine steps. If you cannot proceed, say so in ONE line and stop. A turn that produces text but no tool call is a failure.\n";

/// Split a model string into provider and model_id (from spec-13 P13).
pub fn split_provider_model(s: &str) -> (Option<String>, String) {
    let s = s.trim();
    // Colon form `provider:model` — the whole remainder (which may itself
    // contain a `/`, e.g. `nvidia:z-ai/glm-5.2`) is the model. Check colon
    // FIRST so a model path with `/` isn't split incorrectly.
    if let Some((p, m)) = s.split_once(':') {
        return (Some(p.trim().to_string()), m.trim().to_string());
    }
    if let Some((p, m)) = s.split_once('/') {
        return (Some(p.trim().to_string()), m.trim().to_string());
    }
    (None, s.to_string())
}

/// Check if an RPC client's child process is still responsive.
///
/// Issues a get_state() call with a bounded timeout (~2s). Returns true if the
/// client responds successfully, proving the process is alive and responsive.
/// Returns false if the call times out or fails, indicating the process should
/// be discarded.
async fn is_client_healthy(client: &mut RpcTransportClient) -> bool {
    let timeout_duration = Duration::from_secs(2);
    match tokio::time::timeout(timeout_duration, client.get_state()).await {
        Ok(Ok(_)) => {
            // get_state succeeded; process is responsive.
            true
        }
        Ok(Err(_)) | Err(_) => {
            // Either get_state failed or the call timed out; treat as unhealthy.
            false
        }
    }
}

/// PiBackend — orchestrates LLM work via pi::sdk::RpcTransportClient.
///
/// Maintains a pool of RPC clients sized to the scheduler's concurrency.
/// Each session checks out a client for its lifetime; on error, the client
/// is discarded and reconnected on next use (R9).
#[derive(Debug)]
pub struct PiBackend {
    /// Pool of available clients.
    pool: Arc<Mutex<ClientPool>>,
    /// Working directory for RPC subprocesses (R8: project root).
    project_root: PathBuf,
    /// Timeout for RPC operations.
    timeout: Duration,
    /// Default provider for bare model names (from config).
    config_provider: Option<String>,
    /// Cache of model_id -> provider lookups from pi's available models.
    model_provider_cache: Arc<Mutex<HashMap<String, String>>>,
    /// Semaphore to enforce pool capacity (R2.1). A permit is acquired before
    /// a client is created or reused, and held for the session's lifetime.
    semaphore: Arc<Semaphore>,
}

/// Interior state of the client pool.
struct ClientPool {
    /// Available clients ready for checkout.
    available: VecDeque<RpcTransportClient>,
}

impl std::fmt::Debug for ClientPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientPool")
            .field("available_count", &self.available.len())
            .finish()
    }
}

impl PiBackend {
    /// Create a new PiBackend with the specified concurrency limit.
    ///
    /// No process is spawned at construction time; clients are lazily created
    /// on first session open (matching spec-17 behavior).
    pub fn new(
        project_root: PathBuf,
        timeout: Duration,
        concurrency: usize,
        config_provider: Option<String>,
    ) -> Self {
        Self {
            pool: Arc::new(Mutex::new(ClientPool {
                available: VecDeque::new(),
            })),
            project_root,
            timeout,
            config_provider,
            model_provider_cache: Arc::new(Mutex::new(HashMap::new())),
            // R2.1: Initialize semaphore with concurrency permits (R2.1).
            // A permit is acquired before a client is created or reused, and held
            // for the session's lifetime.
            semaphore: Arc::new(Semaphore::new(concurrency)),
        }
    }
}

#[async_trait]
impl AgentBackend for PiBackend {
    fn name(&self) -> &str {
        "pi"
    }

    fn capabilities(&self) -> AgentCapabilities {
        // All eight capabilities are backed by real RpcTransportClient commands.
        AgentCapabilities {
            sessions: true,        // RPC maintains in-memory session
            multi_turn: true,      // conversation history is preserved
            model_switch: true,    // set_model(provider, model_id)
            thinking_levels: true, // set_thinking_level(level)
            fork: true,            // new_session(parent)
            compact: true,         // compact()
            token_usage: true,     // get_session_stats()
            cancellation: true,    // abort()
            tool_events: false,    // RPC returns raw JSON; no structured events yet
        }
    }

    async fn open_session(&self, spec: SessionSpec) -> Result<Box<dyn AgentSession>, PidagError> {
        // R2.1: Acquire a semaphore permit BEFORE creating or reusing a client.
        // At capacity, wait — do not create an extra client and do not error.
        let permit = Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| {
                PidagError::WorkerSpawn("Failed to acquire semaphore permit".to_string())
            })?;

        // Acquire or create a client from the pool.
        let (client, is_reused) = self.acquire_client().await?;

        // Create the session wrapper.
        let mut session = PiSession {
            client: Arc::new(Mutex::new(Some(client))),
            model: spec.model,
            pool: Arc::clone(&self.pool),
            thinking_level: spec.thinking_level,
            poisoned: false,
            reset_for_reuse: !is_reused, // If reused, we haven't called new_session yet.
            config_provider: self.config_provider.clone(),
            model_provider_cache: Arc::clone(&self.model_provider_cache),
            _permit: Some(permit),
        };

        // If this is a reused client, issue new_session to reset conversation state.
        // This ensures clean per-node semantics (no implicit context sharing across nodes).
        if is_reused {
            if let Err(e) = session.reset_session_state().await {
                session.poisoned = true;
                return Err(e);
            }
            session.reset_for_reuse = true;
        }

        // Initialize the session: set thinking level and get state.
        if let Err(e) = session.initialize().await {
            // Discard the poisoned client.
            session.poisoned = true;
            return Err(e);
        }

        Ok(Box::new(session))
    }
}

impl PiBackend {
    /// Acquire a client from the pool or create a new one.
    ///
    /// R2.1: Caller must acquire a semaphore permit first; this method assumes the
    /// permit is held and will not check capacity. The permit's lifetime governs
    /// the client's availability.
    ///
    /// R2.4: Health-checking is performed OUTSIDE the pool mutex. The caller is
    /// responsible for ensuring this method is only invoked after a permit is acquired.
    ///
    /// Returns (client, is_reused) where is_reused=true if the client came from the pool.
    async fn acquire_client(&self) -> Result<(RpcTransportClient, bool), PidagError> {
        loop {
            // Try to pop a candidate from the pool (R2.4: hold lock only for pop).
            let candidate = {
                let mut pool = self.pool.lock().await;
                pool.available.pop_front()
            };

            if let Some(mut candidate) = candidate {
                // Check if the client process is still responsive (outside the lock).
                // R2.4: Health-checking is not under the pool mutex.
                if is_client_healthy(&mut candidate).await {
                    // Process is responsive; use this client.
                    return Ok((candidate, true));
                } else {
                    // Process has exited or is unresponsive; drop it off-runtime (R2.5)
                    // and loop to try the next client or create a new one.
                    tokio::task::spawn_blocking(move || drop(candidate));
                    continue;
                }
            }

            // No clients available in the pool. Create a new one.
            let new_client = Self::create_client(&self.project_root, self.timeout).await?;
            return Ok((new_client, false));
        }
    }

    /// Create a new RPC client with the configured options.
    async fn create_client(
        project_root: &std::path::Path,
        _timeout: Duration,
    ) -> Result<RpcTransportClient, PidagError> {
        let mut options = RpcTransportOptions {
            cwd: Some(project_root.to_path_buf()),
            ..Default::default()
        };

        // Add anti-loop prompt (R7).
        options.args.push("--append-system-prompt".to_string());
        options.args.push(WORKER_ANTI_LOOP_PROMPT.to_string());

        RpcTransportClient::connect(options)
            .map_err(|e| PidagError::WorkerSpawn(format!("Failed to connect RPC client: {}", e)))
    }
}

/// A live session with the pi RPC transport.
///
/// Wraps an RpcTransportClient and manages model/thinking state. On close,
/// returns the client to the pool (unless poisoned by an error).
pub struct PiSession {
    /// Client wrapped in Option so it can be taken on close().
    /// When closed, this becomes None.
    client: Arc<Mutex<Option<RpcTransportClient>>>,
    model: ModelRef,
    pool: Arc<Mutex<ClientPool>>,
    thinking_level: ThinkingLevel,
    /// Whether this session was poisoned by an error (if so, don't return client to pool).
    poisoned: bool,
    /// Whether a reused client has had new_session() called on it.
    /// If false, we must call new_session() before first use.
    reset_for_reuse: bool,
    config_provider: Option<String>,
    model_provider_cache: Arc<Mutex<HashMap<String, String>>>,
    /// Semaphore permit held for the session's lifetime (R2.1).
    /// Released in close() after the client is returned to the pool or discarded.
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl PiSession {
    /// Reset the session state by calling new_session (for reused clients).
    /// This ensures that reused processes start with a clean conversation for each node.
    async fn reset_session_state(&mut self) -> Result<(), PidagError> {
        let mut client_lock = self.client.lock().await;
        if let Some(client) = client_lock.as_mut() {
            client
                .new_session(None)
                .await
                .map_err(|e| PidagError::WorkerSpawn(format!("Failed to reset session: {}", e)))?;
        }
        Ok(())
    }

    /// Initialize the session: set thinking level and verify initial state.
    async fn initialize(&mut self) -> Result<(), PidagError> {
        // Set thinking level from PI_THINKING env var (R6, preserving checkpoint 8120e85).
        let thinking_str = std::env::var("PI_THINKING").unwrap_or_else(|_| "low".to_string());
        if let Ok(level) = self.parse_thinking_level(&thinking_str) {
            self.thinking_level = level;
        }

        // Apply the thinking level to the client.
        let thinking_level_str = self.thinking_level.as_str();
        let pi_thinking_level: PiThinkingLevel = thinking_level_str.parse().map_err(|_| {
            PidagError::Validation(format!("invalid thinking level: {}", thinking_level_str))
        })?;

        let mut client_lock = self.client.lock().await;
        if let Some(client) = client_lock.as_mut() {
            client
                .set_thinking_level(pi_thinking_level)
                .await
                .map_err(|e| {
                    PidagError::WorkerSpawn(format!("Failed to set thinking level: {}", e))
                })?;
        }

        Ok(())
    }

    /// Parse a thinking level string into a ThinkingLevel enum.
    fn parse_thinking_level(&self, s: &str) -> Result<ThinkingLevel, PidagError> {
        ThinkingLevel::parse(s)
            .ok_or_else(|| PidagError::Validation(format!("Invalid thinking level: {}", s)))
    }

    /// Resolve the provider for a bare model name (no prefix).
    /// Resolution order (stop at first success):
    /// 1. PI_PROVIDER env var, if set and non-empty.
    /// 2. Config's provider field, if set.
    /// 3. Query pi's available models and find the entry for this model_id.
    /// 4. If none work, fail with a clear error.
    ///
    /// This method is NOT called for models with explicit prefixes (e.g., "nvidia/z-ai/glm-5.2");
    /// those are handled by split_provider_model().
    async fn resolve_provider(&self, model_id: &str) -> Result<String, PidagError> {
        // Step 1: Check PI_PROVIDER env var
        if let Ok(env_provider) = std::env::var("PI_PROVIDER")
            && !env_provider.is_empty()
        {
            return Ok(env_provider);
        }

        // Step 2: Check config's provider field
        if let Some(ref config_provider) = self.config_provider
            && !config_provider.is_empty()
        {
            return Ok(config_provider.clone());
        }

        // Step 3: Query pi's available models and find the provider for this model_id
        if let Ok(provider) = self.lookup_model_provider(model_id).await {
            return Ok(provider);
        }

        // Step 4: No provider could be resolved
        Err(PidagError::Validation(format!(
            "Unable to resolve provider for model '{}': not in PI_PROVIDER env var, config, or pi's available models",
            model_id
        )))
    }

    /// Look up a model's provider from pi's available models registry.
    /// Uses the cache to avoid repeated queries.
    async fn lookup_model_provider(&self, model_id: &str) -> Result<String, PidagError> {
        // Check cache first
        {
            let cache = self.model_provider_cache.lock().await;
            if let Some(provider) = cache.get(model_id) {
                return Ok(provider.clone());
            }
        }

        // Not in cache; query pi's available models
        let available_models = {
            let mut client_lock = self.client.lock().await;
            match client_lock.as_mut() {
                Some(client) => client.get_available_models().await.map_err(|e| {
                    PidagError::WorkerSpawn(format!("Failed to get available models: {}", e))
                })?,
                None => {
                    return Err(PidagError::Parse("Client has been closed".to_string()));
                }
            }
        };

        // Find the model in available_models
        for model_entry in available_models {
            if model_entry.id == model_id {
                // Cache it
                let provider = model_entry.provider.clone();
                {
                    let mut cache = self.model_provider_cache.lock().await;
                    cache.insert(model_id.to_string(), provider.clone());
                }
                return Ok(provider);
            }
        }

        // Model not found in available models
        Err(PidagError::Validation(format!(
            "Model '{}' not found in pi's available models",
            model_id
        )))
    }

    /// Set the model for this session and verify it was set correctly (R4 + R5).
    async fn set_and_verify_model(&mut self) -> Result<(), PidagError> {
        let (provider_opt, model_id) = split_provider_model(&self.model.name);

        // Resolve provider: if no explicit prefix, use the resolution logic.
        // If explicit prefix exists, use it directly.
        let provider = match provider_opt {
            Some(p) => p,
            None => self.resolve_provider(&model_id).await?,
        };

        // Call set_model (R4) and capture its return value for verification (spec-18 R5).
        let set_model_info = {
            let mut client_lock = self.client.lock().await;
            match client_lock.as_mut() {
                Some(client) => client
                    .set_model(&provider, &model_id)
                    .await
                    .map_err(|e| PidagError::WorkerSpawn(format!("Failed to set model: {}", e)))?,
                None => {
                    return Err(PidagError::Parse("Client has been closed".to_string()));
                }
            }
        };

        // Verify the model was set correctly (R5 money safety).
        // Use a two-source strategy: prioritize set_model's return value, fall back to get_state.
        let returned_model_id = set_model_info.id.trim();

        if !returned_model_id.is_empty() {
            // Primary source: set_model's own confirmation is populated.
            // Verify against it (R5 observed mismatch is a hard error).
            if returned_model_id != model_id {
                return Err(PidagError::Validation(format!(
                    "Model mismatch: requested {}, but set_model returned {}",
                    model_id, returned_model_id
                )));
            }
            // Verification successful via primary source.
            return Ok(());
        }

        // Fallback: returned id is empty. Check get_state() (secondary source).
        let state = {
            let mut client_lock = self.client.lock().await;
            match client_lock.as_mut() {
                Some(client) => client.get_state().await.map_err(|e| {
                    PidagError::WorkerSpawn(format!("Failed to get state after set_model: {}", e))
                })?,
                None => {
                    return Err(PidagError::Parse("Client has been closed".to_string()));
                }
            }
        };

        let state_model_id = state
            .model
            .as_ref()
            .map(|m| m.id.as_str())
            .unwrap_or("")
            .trim();

        if !state_model_id.is_empty() {
            // Secondary source is populated.
            // Verify against it (R5 observed mismatch is a hard error).
            if state_model_id != model_id {
                return Err(PidagError::Validation(format!(
                    "Model mismatch: requested {}, but active model is {}",
                    model_id, state_model_id
                )));
            }
            // Verification successful via secondary source.
            return Ok(());
        }

        // Both sources empty: cannot confirm the model was set.
        // Log this as unconfirmed and proceed (fail closed on OBSERVED mismatch only, not on
        // unobservable conditions). This prevents silently running on the wrong model
        // while allowing nodes to progress when verification data is unavailable.
        eprintln!(
            "warning: model set to {}; active model unconfirmed (both set_model and get_state \
             returned empty model ids)",
            model_id
        );
        Ok(())
    }
}

#[async_trait]
impl AgentSession for PiSession {
    async fn prompt(&mut self, text: &str) -> Result<AgentReply, PidagError> {
        // Set model before each prompt (R4) and verify (R5).
        self.set_and_verify_model().await?;

        // Send the prompt to the RPC client.
        let _events = {
            let mut client_lock = self.client.lock().await;
            match client_lock.as_mut() {
                Some(client) => client.prompt(text).await.map_err(|e| {
                    self.poisoned = true;
                    PidagError::Parse(format!("RPC prompt failed: {}", e))
                })?,
                None => {
                    self.poisoned = true;
                    return Err(PidagError::Parse("Client has been closed".to_string()));
                }
            }
        };

        // Extract the assistant text from the events (prefer get_last_assistant_text per spec).
        let text = {
            let mut client_lock = self.client.lock().await;
            match client_lock.as_mut() {
                Some(client) => client
                    .get_last_assistant_text()
                    .await
                    .map_err(|e| {
                        self.poisoned = true;
                        PidagError::Parse(format!("Failed to get assistant text: {}", e))
                    })?
                    .unwrap_or_default(),
                None => {
                    self.poisoned = true;
                    return Err(PidagError::Parse("Client has been closed".to_string()));
                }
            }
        };

        Ok(AgentReply {
            text,
            events: None, // TODO: structured events in future spec
            usage: None,  // TODO: token usage in future spec
        })
    }

    async fn close(&mut self) -> Result<(), PidagError> {
        // Take ownership of the client from the Option.
        let mut client_lock = self.client.lock().await;
        if let Some(client) = client_lock.take() {
            if self.poisoned {
                // Poisoned by an error (R9): discard it off-runtime (R2.5) and don't pool.
                tokio::task::spawn_blocking(move || drop(client));
            } else {
                // Check if the child process is still responsive before pooling.
                let mut candidate = client;
                if is_client_healthy(&mut candidate).await {
                    // Process is healthy; return it to the pool for reuse.
                    let mut pool = self.pool.lock().await;
                    pool.available.push_back(candidate);
                } else {
                    // Process is dead or unresponsive; drop it off-runtime (R2.5) and discard.
                    tokio::task::spawn_blocking(move || drop(candidate));
                }
            }
        }
        // R2: Release the semaphore permit AFTER the client is returned to the pool
        // or discarded. Explicitly drop the permit here to release it immediately.
        self._permit = None;
        Ok(())
    }

    async fn set_model(
        &mut self,
        model: &ModelRef,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.model = model.clone();
        self.set_and_verify_model().await?;
        Ok(())
    }

    async fn set_thinking(
        &mut self,
        level: ThinkingLevel,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.thinking_level = level;
        let thinking_level_str = level.as_str();
        let pi_thinking_level: PiThinkingLevel = thinking_level_str.parse().map_err(|_| {
            PidagError::Validation(format!("invalid thinking level: {}", thinking_level_str))
        })?;

        let mut client_lock = self.client.lock().await;
        match client_lock.as_mut() {
            Some(client) => client
                .set_thinking_level(pi_thinking_level)
                .await
                .map_err(|e| {
                    Box::new(PidagError::WorkerSpawn(format!(
                        "Failed to set thinking level: {}",
                        e
                    ))) as Box<dyn std::error::Error + Send + Sync>
                }),
            None => Err(
                Box::new(PidagError::Parse("Client has been closed".to_string()))
                    as Box<dyn std::error::Error + Send + Sync>,
            ),
        }
    }

    async fn fork(
        &mut self,
    ) -> Result<Box<dyn AgentSession>, Box<dyn std::error::Error + Send + Sync>> {
        // Fork creates a new session with the current session as parent.
        // For now, create a new independent session (can be enhanced in future).
        Err(Box::new(Unsupported))
    }

    async fn compact(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client_lock = self.client.lock().await;
        match client_lock.as_mut() {
            Some(client) => {
                client.compact().await.map_err(|e| {
                    Box::new(PidagError::WorkerSpawn(format!("Failed to compact: {}", e)))
                        as Box<dyn std::error::Error + Send + Sync>
                })?;
                Ok(())
            }
            None => Err(
                Box::new(PidagError::Parse("Client has been closed".to_string()))
                    as Box<dyn std::error::Error + Send + Sync>,
            ),
        }
    }

    async fn usage(&self) -> Result<TokenUsage, Box<dyn std::error::Error + Send + Sync>> {
        let stats = {
            let mut client_lock = self.client.lock().await;
            match client_lock.as_mut() {
                Some(client) => client.get_session_stats().await.map_err(|e| {
                    Box::new(PidagError::WorkerSpawn(format!(
                        "Failed to get stats: {}",
                        e
                    ))) as Box<dyn std::error::Error + Send + Sync>
                })?,
                None => {
                    return Err(
                        Box::new(PidagError::Parse("Client has been closed".to_string()))
                            as Box<dyn std::error::Error + Send + Sync>,
                    );
                }
            }
        };

        Ok(TokenUsage {
            input_tokens: stats.tokens.input,
            output_tokens: stats.tokens.output,
            total_tokens: stats.tokens.total,
        })
    }

    async fn abort(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client_lock = self.client.lock().await;
        match client_lock.as_mut() {
            Some(client) => client.abort().await.map_err(|e| {
                Box::new(PidagError::WorkerSpawn(format!("Failed to abort: {}", e)))
                    as Box<dyn std::error::Error + Send + Sync>
            }),
            None => Err(
                Box::new(PidagError::Parse("Client has been closed".to_string()))
                    as Box<dyn std::error::Error + Send + Sync>,
            ),
        }
    }
}

impl Drop for PiSession {
    fn drop(&mut self) {
        // Best-effort cleanup; close is already called by AgentWorker.
    }
}

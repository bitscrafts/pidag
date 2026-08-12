//! Agent backend capability declarations.
//!
//! `AgentCapabilities` is an honest feature declaration that every backend
//! must expose. pidag uses this to degrade gracefully when a capability is
//! missing (R4), without requiring each backend to implement fallback logic.

use serde::{Deserialize, Serialize};

/// Capability declaration for an agent backend.
/// Each boolean indicates whether the backend actually implements that feature.
///
/// pidag asks for what it wants and gracefully degrades when a capability
/// is absent, using the fallbacks listed in the spec. A backend MUST NOT
/// declare a capability it doesn't implement — the conformance suite enforces this.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AgentCapabilities {
    /// Can hold multiple turns in a single session (multi-turn conversation).
    /// False => pidag opens one session per node, carrying prior context inline.
    pub sessions: bool,

    /// Implies `sessions`; can make multiple prompts in the same session.
    /// False => similar to sessions=false but for explicit multi-turn tracking.
    pub multi_turn: bool,

    /// Can switch models mid-session via `set_model()`.
    /// False => close session, open new one with target model (R4).
    pub model_switch: bool,

    /// Can set thinking/reasoning level on the session.
    pub thinking_levels: bool,

    /// Can fork a session into independent branches.
    /// False => open independent sessions per DAG branch (R4).
    pub fork: bool,

    /// Can compact internal state (e.g., summarize context).
    /// False => fall back to the spec-splitting heuristic (R4).
    pub compact: bool,

    /// Can report token usage (input + output).
    /// False => fall back to the >7-criteria heuristic (spec-07) (R4).
    pub token_usage: bool,

    /// Can cancel pending operations mid-prompt.
    /// False => rely on timeout + drop (R4).
    pub cancellation: bool,

    /// Can emit structured events (tool calls, streaming, etc).
    /// False => treat reply as opaque text (R4).
    pub tool_events: bool,
}

impl AgentCapabilities {
    /// Create a zero-capability backend (the minimum viable abstraction).
    /// Used for testing R4 (graceful degradation).
    pub fn zero() -> Self {
        Self {
            sessions: false,
            multi_turn: false,
            model_switch: false,
            thinking_levels: false,
            fork: false,
            compact: false,
            token_usage: false,
            cancellation: false,
            tool_events: false,
        }
    }

    /// Create a full-capability backend (design target for rich agents).
    pub fn full() -> Self {
        Self {
            sessions: true,
            multi_turn: true,
            model_switch: true,
            thinking_levels: true,
            fork: true,
            compact: true,
            token_usage: true,
            cancellation: true,
            tool_events: true,
        }
    }
}

impl Default for AgentCapabilities {
    fn default() -> Self {
        Self::zero()
    }
}

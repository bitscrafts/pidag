//! A2A (Agent-to-Agent) protocol module for pidag.
//!
//! This module provides:
//! - `A2aServer`: HTTP server exposing A2A protocol endpoints
//! - `A2aWorker`: Worker that dispatches DAG nodes to A2A-compliant remote agents

mod server;
pub mod worker;

pub use server::{A2aServerState, AgentCard, AuthConfig, router, router_for_test};
pub use worker::{A2aWorker, is_a2a_endpoint};

//! RPC (Remote Procedure Call) module for pidag.
//!
//! This module provides:
//! - `RpcServer`: JSON-RPC 2.0 server over stdio
//! - Handler functions for DAG operations (submit, status, result, cancel, etc.)
//! - Shared server state for RPC and MCP servers

mod handlers;
mod server;

pub use handlers::{
    RpcRequest, RunState, ServerState, generate_resume_token, handle_dag_await, handle_dag_cancel,
    handle_dag_result, handle_dag_status, handle_dag_submit, handle_list_runs, handle_node_retry,
    parse_resume_token,
};
pub use server::RpcServer;

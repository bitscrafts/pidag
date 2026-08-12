//! MCP (Model Context Protocol) module for pidag.
//!
//! This module provides:
//! - `McpServer`: Exposes pidag operations as MCP tools over stdio transport
//! - `McpClient`: Client for invoking tools on external MCP servers
//! - `McpCallWorker`: Worker that executes MCP tool calls for DAG nodes

pub mod call;
mod client;
mod server;

pub use call::McpCallWorker;
pub use client::{McpClient, ToolInfo};
pub use server::{McpServer, run_mcp_server};

//! MCP for the Orchestrator (ADR-0021).
//!
//! - [`McpClient`]: talks to an MCP server over stdio or Streamable HTTP.
//! - [`McpManager`] / [`McpTools`]: the user's servers (`mcp.json`), whose
//!   tools the AIs get as `mcp.<server>.<tool>`, behind the autonomy gate
//!   and audited like every other tool.
//! - [`serve`]: the Orchestrator's own tools over MCP, for the AIs that run
//!   as CLI programs (Claude Code, Codex, Gemini CLI).

mod client;
pub mod config;
mod manager;
pub mod serve;

pub use client::{CallOutcome, Expand, McpClient, McpError, RemoteTool, PROTOCOL_VERSION};
pub use config::{ServerConfig, Transport};
pub use manager::{McpManager, McpTools, ServerStatus, ServerView, ToolView};

//! Model Context Protocol (MCP) client, proxy, and service discovery.
//!
//! Implements the MCP specification over JSON-RPC 2.0 / stdio, enabling
//! Argentor to connect to external tool servers, discover their capabilities,
//! and expose them as skills within the agent framework.
//!
//! # Main types
//!
//! - [`McpClient`] — JSON-RPC 2.0 client for communicating with MCP servers.
//! - [`McpSkill`] — Wraps an MCP tool as an Argentor [`Skill`](argentor_skills::Skill).
//! - [`McpProxy`] — Transparent proxy that multiplexes requests to multiple MCP servers.
//! - [`McpServerManager`] — Manages lifecycle and health of connected MCP servers.

/// MCP JSON-RPC client.
pub mod client;
/// Centralized API credential vault with rotation, quotas, and provider grouping.
pub mod credential_vault;
/// Tool discovery over MCP.
pub mod discovery;
/// In-process MCP server — define tools without spawning a subprocess.
pub mod in_process;
/// MCP server lifecycle manager.
pub mod manager;
/// JSON-RPC 2.0 protocol types.
pub mod protocol;
/// MCP proxy for multi-server multiplexing.
pub mod proxy;
/// Multi-proxy coordination with routing, circuit breaker, and failover.
pub mod proxy_orchestrator;
/// MCP server — exposes Argentor skills as MCP tools.
pub mod server;
/// MCP tool-to-skill adapter.
pub mod skill;
/// Per-provider token pool with rate limiting and tier priority.
pub mod token_pool;

pub use client::McpClient;
pub use credential_vault::CredentialVault;
pub use discovery::ToolDiscovery;
pub use in_process::InProcessMcpServer;
pub use manager::{McpServerConfig, McpServerManager, McpServerStatus};
pub use proxy::McpProxy;
pub use proxy_orchestrator::ProxyOrchestrator;
pub use server::McpServer;
pub use skill::McpSkill;
pub use token_pool::{PoolHealth, PoolStats, SelectionStrategy, TokenPool, TokenTier};

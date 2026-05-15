//! MCP (Model Context Protocol) client setup.
//!
//! Demonstrates how to connect to an MCP server subprocess, discover its tools,
//! and wrap them as Argentor skills via `McpSkill`.
//!
//! This example shows the API surface — the actual connection to an MCP server
//! requires a real MCP-compatible subprocess. Run with a real server binary:
//!
//! ```bash
//! cargo run --example mcp_client
//! ```
//!
//! The example deliberately skips the live connection so it compiles and runs
//! without any external dependencies.

use argentor_mcp::McpServerConfig;

#[tokio::main]
async fn main() {
    // --- Option A: connect to a live MCP server subprocess ---
    //
    // Uncomment this block once you have an MCP server binary available:
    //
    // let (client, tools) = McpClient::connect(
    //     "npx",                          // command to spawn
    //     &["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    //     &[],                            // extra env vars
    // )
    // .await
    // .expect("failed to connect to MCP server");
    //
    // println!("Discovered {} tools from MCP server:", tools.len());
    // for tool in &tools {
    //     println!("  - {}: {}", tool.name, tool.description);
    // }
    //
    // // Wrap each MCP tool as an Argentor skill and register it.
    // let mut registry = SkillRegistry::new();
    // for tool in tools {
    //     let skill = McpSkill::new(Arc::new(client.clone()), tool);
    //     registry.register(Arc::new(skill));
    // }

    // --- Option B: show the server manager API (no subprocess spawned) ---

    println!("MCP client API demo (no live connection):");

    // McpServerConfig describes an external MCP server.
    let _config = McpServerConfig {
        command: "npx".into(),
        args: vec![
            "-y".into(),
            "@modelcontextprotocol/server-filesystem".into(),
            "/tmp".into(),
        ],
        env: std::collections::HashMap::new(),
        auto_reconnect: true,
        health_check_interval_secs: 60,
    };

    println!("  McpServerConfig ready (would spawn: npx -y @modelcontextprotocol/server-filesystem /tmp)");
    println!("  McpSkill wraps any MCP tool as an Argentor Skill trait object.");
    println!("  McpServerManager handles lifecycle, health checks, and reconnection.");
    println!("\nConnect a real server with McpClient::connect(command, args, env).await");
}

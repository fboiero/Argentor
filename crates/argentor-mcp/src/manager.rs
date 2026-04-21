use crate::client::McpClient;
use crate::protocol::McpToolDef;
use crate::skill::McpSkill;
use argentor_core::{ArgentorError, ArgentorResult};
use argentor_skills::SkillRegistry;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    /// Command to launch the MCP server process.
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set for the server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Enable auto-reconnect on failure (default: true).
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    /// Health check interval in seconds (default: 60). Set to 0 to disable.
    #[serde(default = "default_health_interval")]
    pub health_check_interval_secs: u64,
}

fn default_true() -> bool {
    true
}
fn default_health_interval() -> u64 {
    60
}

/// Status of a managed MCP server.
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    /// Command used to launch the server.
    pub command: String,
    /// Whether the server is currently connected.
    pub connected: bool,
    /// Number of tools exposed by this server.
    pub tool_count: usize,
    /// UTC timestamp of when the connection was established.
    pub connected_at: Option<DateTime<Utc>>,
    /// UTC timestamp of the last health check.
    pub last_health_check: Option<DateTime<Utc>>,
    /// Number of times the server has been reconnected.
    pub reconnect_count: usize,
}

/// Internal state for a managed server.
struct ManagedServer {
    config: McpServerConfig,
    client: Arc<McpClient>,
    tool_names: Vec<String>,
    connected_at: DateTime<Utc>,
    last_health_check: Option<DateTime<Utc>>,
    reconnect_count: usize,
}

/// Manages multiple MCP server connections with health checks and reconnection.
pub struct McpServerManager {
    servers: RwLock<HashMap<String, ManagedServer>>,
}

impl McpServerManager {
    /// Create a new, empty server manager.
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
        }
    }

    /// Connect to all configured MCP servers and register their tools.
    /// Returns a list of errors for servers that failed to connect.
    pub async fn connect_all(
        &self,
        configs: &[McpServerConfig],
        registry: &SkillRegistry,
    ) -> Vec<ArgentorError> {
        let mut errors = Vec::new();

        for config in configs {
            match self.connect_server(config, registry).await {
                Ok(tool_count) => {
                    info!(
                        server = %config.command,
                        tools = tool_count,
                        "MCP server connected"
                    );
                }
                Err(e) => {
                    warn!(
                        server = %config.command,
                        error = %e,
                        "Failed to connect MCP server"
                    );
                    errors.push(e);
                }
            }
        }

        errors
    }

    /// Connect to a single MCP server and register its tools.
    async fn connect_server(
        &self,
        config: &McpServerConfig,
        registry: &SkillRegistry,
    ) -> ArgentorResult<usize> {
        let args: Vec<&str> = config
            .args
            .iter()
            .map(std::string::String::as_str)
            .collect();
        let env: Vec<(&str, &str)> = config
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let (client, tools) = McpClient::connect(&config.command, &args, &env).await?;
        let client = Arc::new(client);
        let tool_count = tools.len();

        let tool_names = Self::register_tools(&tools, &client, registry);

        let mut servers = self.servers.write().await;
        servers.insert(
            config.command.clone(),
            ManagedServer {
                config: config.clone(),
                client,
                tool_names,
                connected_at: Utc::now(),
                last_health_check: None,
                reconnect_count: 0,
            },
        );

        Ok(tool_count)
    }

    /// Register MCP tools as skills in the registry.
    fn register_tools(
        tools: &[McpToolDef],
        client: &Arc<McpClient>,
        registry: &SkillRegistry,
    ) -> Vec<String> {
        let mut names = Vec::new();
        for tool in tools {
            let skill = McpSkill::new(tool, client.clone());
            let name = format!("mcp_{}_{}", client.server_name(), tool.name)
                .replace(['/', '\\', ' '], "_");
            names.push(name);
            registry.register(Arc::new(skill));
        }
        names
    }

    /// Run a health check on all managed servers.
    /// Attempts to reconnect failed servers if auto_reconnect is enabled.
    pub async fn health_check(&self) {
        let server_keys: Vec<String> = {
            let servers = self.servers.read().await;
            servers.keys().cloned().collect()
        };

        for key in server_keys {
            let needs_reconnect = {
                let mut servers = self.servers.write().await;
                if let Some(server) = servers.get_mut(&key) {
                    server.last_health_check = Some(Utc::now());
                    match server.client.health_check().await {
                        Ok(()) => false,
                        Err(e) => {
                            warn!(
                                server = %key,
                                error = %e,
                                "MCP health check failed"
                            );
                            server.config.auto_reconnect
                        }
                    }
                } else {
                    false
                }
            };

            if needs_reconnect {
                self.reconnect(&key).await;
            }
        }
    }

    /// Attempt to reconnect a server with exponential backoff.
    async fn reconnect(&self, server_key: &str) {
        let config = {
            let servers = self.servers.read().await;
            match servers.get(server_key) {
                Some(s) => s.config.clone(),
                None => return,
            }
        };

        info!(server = %server_key, "Attempting MCP server reconnection");

        match reconnect_with_backoff(&config, 5).await {
            Ok((client, tools)) => {
                let client = Arc::new(client);
                let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
                let tool_count = tools.len();

                let mut servers = self.servers.write().await;
                if let Some(server) = servers.get_mut(server_key) {
                    server.client = client;
                    server.tool_names = tool_names;
                    server.connected_at = Utc::now();
                    server.reconnect_count += 1;
                    info!(
                        server = %server_key,
                        tools = tool_count,
                        reconnects = server.reconnect_count,
                        "MCP server reconnected"
                    );
                }
            }
            Err(e) => {
                error!(
                    server = %server_key,
                    error = %e,
                    "MCP server reconnection failed after retries"
                );
            }
        }
    }

    /// Start a background health check loop.
    pub fn start_health_loop(self: Arc<Self>, interval: Duration) {
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            loop {
                timer.tick().await;
                self.health_check().await;
            }
        });
    }

    /// Get the status of all managed servers.
    pub async fn status(&self) -> Vec<McpServerStatus> {
        let servers = self.servers.read().await;
        servers
            .values()
            .map(|s| McpServerStatus {
                command: s.config.command.clone(),
                connected: true, // If it's in the map, it was connected at some point
                tool_count: s.tool_names.len(),
                connected_at: Some(s.connected_at),
                last_health_check: s.last_health_check,
                reconnect_count: s.reconnect_count,
            })
            .collect()
    }

    /// Get the number of managed servers.
    pub async fn server_count(&self) -> usize {
        self.servers.read().await.len()
    }
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Reconnect to an MCP server with exponential backoff.
async fn reconnect_with_backoff(
    config: &McpServerConfig,
    max_retries: u32,
) -> ArgentorResult<(McpClient, Vec<McpToolDef>)> {
    let mut delay = Duration::from_secs(1);

    for attempt in 1..=max_retries {
        let args: Vec<&str> = config
            .args
            .iter()
            .map(std::string::String::as_str)
            .collect();
        let env: Vec<(&str, &str)> = config
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        match McpClient::connect(&config.command, &args, &env).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                warn!(
                    attempt = attempt,
                    max_retries = max_retries,
                    delay_secs = delay.as_secs(),
                    error = %e,
                    "MCP reconnect failed, retrying..."
                );
                if attempt < max_retries {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    Err(ArgentorError::Skill(format!(
        "Failed to reconnect to MCP server '{}' after {} retries",
        config.command, max_retries
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config: McpServerConfig = serde_json::from_str(r#"{"command":"test"}"#).unwrap();
        assert!(config.auto_reconnect);
        assert_eq!(config.health_check_interval_secs, 60);
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
    }

    #[test]
    fn test_config_custom_values() {
        let config: McpServerConfig = serde_json::from_str(
            r#"{"command":"server","args":["--db","test"],"auto_reconnect":false,"health_check_interval_secs":30}"#,
        )
        .unwrap();
        assert!(!config.auto_reconnect);
        assert_eq!(config.health_check_interval_secs, 30);
        assert_eq!(config.args, vec!["--db", "test"]);
    }

    #[test]
    fn test_server_status_serialization() {
        let status = McpServerStatus {
            command: "test-server".to_string(),
            connected: true,
            tool_count: 5,
            connected_at: Some(Utc::now()),
            last_health_check: None,
            reconnect_count: 0,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("test-server"));
        assert!(json.contains("tool_count"));
    }

    #[tokio::test]
    async fn test_manager_empty() {
        let mgr = McpServerManager::new();
        assert_eq!(mgr.server_count().await, 0);
        let status = mgr.status().await;
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn test_connect_nonexistent_server() {
        let mgr = McpServerManager::new();
        let config = McpServerConfig {
            command: "/nonexistent/mcp-server".to_string(),
            args: vec![],
            env: HashMap::new(),
            auto_reconnect: false,
            health_check_interval_secs: 0,
        };
        let registry = SkillRegistry::new();
        let errors = mgr.connect_all(&[config], &registry).await;
        assert_eq!(errors.len(), 1);
        assert_eq!(mgr.server_count().await, 0);
    }
}

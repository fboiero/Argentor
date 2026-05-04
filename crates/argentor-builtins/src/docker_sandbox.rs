//! Docker sandbox for executing commands in isolated containers.
//!
//! This module provides a sandboxed execution environment using Docker containers.
//! The `DockerSandbox` manages the lifecycle of a container, and `DockerShellSkill`
//! exposes command execution as a skill that agents can invoke.
//!
//! The actual Docker integration requires the `docker` feature flag (which pulls in
//! the `bollard` crate). Configuration structs and validation helpers are always
//! available.

use argentor_core::{ArgentorError, ArgentorResult};
use serde::{Deserialize, Serialize};

#[cfg(feature = "docker")]
use {
    argentor_core::{ToolCall, ToolResult},
    argentor_security::Capability,
    argentor_skills::skill::{Skill, SkillDescriptor},
    async_trait::async_trait,
    bollard::{
        container::{
            Config as ContainerConfig, CreateContainerOptions, LogOutput, RemoveContainerOptions,
            StartContainerOptions, StopContainerOptions,
        },
        exec::{CreateExecOptions, StartExecResults},
        Docker,
    },
    futures_util::StreamExt,
    std::sync::Arc,
    tracing::{debug, error, info},
};

// ---------------------------------------------------------------------------
// Configuration (always available)
// ---------------------------------------------------------------------------

/// Configuration for the Docker sandbox environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerSandboxConfig {
    /// Docker image to use (default: "ubuntu:22.04").
    #[serde(default = "default_image")]
    pub image: String,

    /// Memory limit in megabytes (default: 512).
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: u64,

    /// CPU core limit (default: 1.0).
    #[serde(default = "default_cpu_limit")]
    pub cpu_limit: f64,

    /// Command execution timeout in seconds (default: 30).
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Whether networking is enabled inside the container (default: false).
    #[serde(default)]
    pub network_enabled: bool,

    /// Working directory inside the container (default: "/workspace").
    #[serde(default = "default_working_dir")]
    pub working_dir: String,
}

fn default_image() -> String {
    "ubuntu:22.04".to_string()
}

fn default_memory_limit_mb() -> u64 {
    512
}

fn default_cpu_limit() -> f64 {
    1.0
}

fn default_timeout_secs() -> u64 {
    30
}

fn default_working_dir() -> String {
    "/workspace".to_string()
}

impl Default for DockerSandboxConfig {
    fn default() -> Self {
        Self {
            image: default_image(),
            memory_limit_mb: default_memory_limit_mb(),
            cpu_limit: default_cpu_limit(),
            timeout_secs: default_timeout_secs(),
            network_enabled: false,
            working_dir: default_working_dir(),
        }
    }
}

// ---------------------------------------------------------------------------
// Execution result (always available)
// ---------------------------------------------------------------------------

/// Result of executing a command inside the Docker sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    /// Process exit code (0 means success).
    pub exit_code: i64,
    /// Standard output captured from the command.
    pub stdout: String,
    /// Standard error captured from the command.
    pub stderr: String,
}

// ---------------------------------------------------------------------------
// Command sanitisation (always available)
// ---------------------------------------------------------------------------

/// Basic validation of a command string before execution.
///
/// Rejects empty commands and commands containing null bytes.
pub fn sanitize_command(cmd: &str) -> ArgentorResult<String> {
    if cmd.trim().is_empty() {
        return Err(ArgentorError::Skill(
            "Docker sandbox: empty command rejected".to_string(),
        ));
    }

    if cmd.contains('\0') {
        return Err(ArgentorError::Skill(
            "Docker sandbox: command contains null bytes".to_string(),
        ));
    }

    Ok(cmd.to_string())
}

// ---------------------------------------------------------------------------
// Docker sandbox (requires `docker` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "docker")]
pub struct DockerSandbox {
    pub config: DockerSandboxConfig,
    client: Docker,
    container_id: Option<String>,
}

#[cfg(feature = "docker")]
impl DockerSandbox {
    /// Create a new `DockerSandbox` using the given configuration.
    ///
    /// Connects to the local Docker daemon (via the default socket).
    pub async fn new(config: DockerSandboxConfig) -> ArgentorResult<Self> {
        let client = Docker::connect_with_local_defaults().map_err(|e| {
            ArgentorError::Skill(format!("Failed to connect to Docker daemon: {e}"))
        })?;

        // Quick health check — ping the daemon.
        client
            .ping()
            .await
            .map_err(|e| ArgentorError::Skill(format!("Docker daemon ping failed: {e}")))?;

        info!(image = %config.image, "Docker sandbox created");

        Ok(Self {
            config,
            client,
            container_id: None,
        })
    }

    /// Ensure that the backing container exists and is running.
    ///
    /// If the container has not been created yet it will be created and started.
    pub async fn ensure_container(&mut self) -> ArgentorResult<()> {
        if self.container_id.is_some() {
            return Ok(());
        }

        let memory_bytes = (self.config.memory_limit_mb * 1024 * 1024) as i64;
        // Docker CPU quota: period=100_000 µs, quota = period * cpu_limit
        let cpu_quota = (100_000.0 * self.config.cpu_limit) as i64;

        let host_config = bollard::models::HostConfig {
            memory: Some(memory_bytes),
            cpu_quota: Some(cpu_quota),
            cpu_period: Some(100_000),
            network_mode: if self.config.network_enabled {
                None
            } else {
                Some("none".to_string())
            },
            ..Default::default()
        };

        let container_config = ContainerConfig {
            image: Some(self.config.image.clone()),
            working_dir: Some(self.config.working_dir.clone()),
            tty: Some(true),
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            host_config: Some(host_config),
            ..Default::default()
        };

        let container = self
            .client
            .create_container(
                Some(CreateContainerOptions::<String> {
                    ..Default::default()
                }),
                container_config,
            )
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to create container: {e}")))?;

        let id = container.id.clone();

        self.client
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to start container: {e}")))?;

        info!(container_id = %id, "Docker container started");
        self.container_id = Some(id);

        Ok(())
    }

    /// Execute a command inside the running container.
    ///
    /// The command is run via `sh -c` so shell features (pipes, redirects, etc.)
    /// are available.
    pub async fn exec(&mut self, command: &str) -> ArgentorResult<ExecResult> {
        let command = sanitize_command(command)?;
        self.ensure_container().await?;

        // Safety: ensure_container() was just called above and succeeded,
        // which guarantees container_id is Some. If the invariant is somehow
        // broken we surface a clear error instead of panicking.
        let container_id = self.container_id.as_ref().ok_or_else(|| {
            ArgentorError::Skill(
                "Docker sandbox: container_id missing after ensure_container".to_string(),
            )
        })?;

        let exec_opts = CreateExecOptions {
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            cmd: Some(vec!["sh".to_string(), "-c".to_string(), command.clone()]),
            working_dir: Some(self.config.working_dir.clone()),
            ..Default::default()
        };

        let exec_created = self
            .client
            .create_exec(container_id, exec_opts)
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to create exec: {e}")))?;

        let start_result = self
            .client
            .start_exec(&exec_created.id, None)
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to start exec: {e}")))?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        if let StartExecResults::Attached { mut output, .. } = start_result {
            let deadline = tokio::time::Instant::now()
                + std::time::Duration::from_secs(self.config.timeout_secs);

            loop {
                let chunk = tokio::time::timeout_at(deadline, output.next()).await;
                match chunk {
                    Ok(Some(Ok(log))) => match log {
                        LogOutput::StdOut { message } => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        LogOutput::StdErr { message } => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        _ => {}
                    },
                    Ok(Some(Err(e))) => {
                        error!(error = %e, "Error reading exec output");
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        return Ok(ExecResult {
                            exit_code: -1,
                            stdout,
                            stderr: format!(
                                "Command timed out after {}s",
                                self.config.timeout_secs
                            ),
                        });
                    }
                }
            }
        }

        // Retrieve the exit code.
        let inspect = self
            .client
            .inspect_exec(&exec_created.id)
            .await
            .map_err(|e| ArgentorError::Skill(format!("Failed to inspect exec: {e}")))?;

        let exit_code = inspect.exit_code.unwrap_or(-1);

        debug!(
            exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "Exec finished"
        );

        Ok(ExecResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    /// Stop and remove the backing container, releasing all resources.
    pub async fn cleanup(&mut self) -> ArgentorResult<()> {
        if let Some(id) = self.container_id.take() {
            info!(container_id = %id, "Cleaning up Docker container");

            // Best-effort stop.
            let _ = self
                .client
                .stop_container(&id, Some(StopContainerOptions { t: 5 }))
                .await;

            // Force remove.
            self.client
                .remove_container(
                    &id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|e| ArgentorError::Skill(format!("Failed to remove container: {e}")))?;

            info!(container_id = %id, "Docker container removed");
        }

        Ok(())
    }
}

#[cfg(feature = "docker")]
impl Drop for DockerSandbox {
    fn drop(&mut self) {
        if let Some(id) = &self.container_id {
            // Cannot await in Drop — log a warning so users remember to call cleanup().
            tracing::warn!(
                container_id = %id,
                "DockerSandbox dropped without cleanup — container may still be running"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DockerShellSkill (requires `docker` feature)
// ---------------------------------------------------------------------------

/// A Skill that executes commands inside a Docker sandbox container.
#[cfg(feature = "docker")]
pub struct DockerShellSkill {
    descriptor: SkillDescriptor,
    sandbox: Arc<tokio::sync::Mutex<DockerSandbox>>,
}

#[cfg(feature = "docker")]
impl DockerShellSkill {
    /// Create a new `DockerShellSkill` wrapping the given sandbox.
    pub fn new(sandbox: Arc<tokio::sync::Mutex<DockerSandbox>>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "docker_shell".to_string(),
                description: "Execute a command inside a sandboxed Docker container".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The command to execute inside the Docker container"
                        }
                    },
                    "required": ["command"]
                }),
                required_capabilities: vec![Capability::ShellExec {
                    allowed_commands: vec!["*".into()],
                }],
                requires_approval: true,
            },
            sandbox,
        }
    }
}

#[cfg(feature = "docker")]
#[async_trait]
impl Skill for DockerShellSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let command = call.arguments["command"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if command.is_empty() {
            return Ok(ToolResult::error(&call.id, "Empty command"));
        }

        let mut sandbox = self.sandbox.lock().await;
        match sandbox.exec(&command).await {
            Ok(result) => {
                let response = serde_json::json!({
                    "exit_code": result.exit_code,
                    "stdout": result.stdout,
                    "stderr": result.stderr,
                });

                if result.exit_code == 0 {
                    Ok(ToolResult::success(&call.id, response.to_string()))
                } else {
                    Ok(ToolResult::error(&call.id, response.to_string()))
                }
            }
            Err(e) => Ok(ToolResult::error(
                &call.id,
                format!("Docker exec failed: {e}"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let config = DockerSandboxConfig::default();
        assert_eq!(config.image, "ubuntu:22.04");
        assert_eq!(config.memory_limit_mb, 512);
        assert!((config.cpu_limit - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.timeout_secs, 30);
        assert!(!config.network_enabled);
        assert_eq!(config.working_dir, "/workspace");
    }

    #[test]
    fn test_sanitize_command_rejects_empty() {
        let result = sanitize_command("");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty command"), "Error was: {err}");
    }

    #[test]
    fn test_sanitize_command_rejects_whitespace_only() {
        let result = sanitize_command("   ");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_command_rejects_null_bytes() {
        let result = sanitize_command("echo \0hello");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("null bytes"), "Error was: {err}");
    }

    #[test]
    fn test_sanitize_command_accepts_valid() {
        let result = sanitize_command("echo hello");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "echo hello");
    }

    #[test]
    fn test_exec_result_serialization_roundtrip() {
        let result = ExecResult {
            exit_code: 0,
            stdout: "hello world".to_string(),
            stderr: String::new(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ExecResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.exit_code, result.exit_code);
        assert_eq!(deserialized.stdout, result.stdout);
        assert_eq!(deserialized.stderr, result.stderr);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = DockerSandboxConfig {
            image: "python:3.12".to_string(),
            memory_limit_mb: 1024,
            cpu_limit: 2.0,
            timeout_secs: 60,
            network_enabled: true,
            working_dir: "/app".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DockerSandboxConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.image, "python:3.12");
        assert_eq!(deserialized.memory_limit_mb, 1024);
        assert!((deserialized.cpu_limit - 2.0).abs() < f64::EPSILON);
        assert_eq!(deserialized.timeout_secs, 60);
        assert!(deserialized.network_enabled);
        assert_eq!(deserialized.working_dir, "/app");
    }

    #[test]
    fn test_config_deserialize_with_defaults() {
        let json = r#"{}"#;
        let config: DockerSandboxConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.image, "ubuntu:22.04");
        assert_eq!(config.memory_limit_mb, 512);
        assert!(!config.network_enabled);
    }

    // -----------------------------------------------------------------------
    // Docker-dependent tests (integration) — require a running Docker daemon
    // -----------------------------------------------------------------------

    #[cfg(feature = "docker")]
    mod docker_integration {
        use super::super::*;

        #[test]
        fn test_docker_shell_skill_descriptor() {
            use std::sync::Arc;
            use tokio::sync::Mutex;

            // We need a sandbox instance; create a dummy runtime to build one.
            // Since we only inspect the descriptor we just check the struct directly.
            let rt = tokio::runtime::Runtime::new().unwrap();
            let skill_descriptor_name = "docker_shell";

            // Build the skill — this requires a live Docker daemon for DockerSandbox::new,
            // so we construct the skill manually to test just the descriptor.
            let descriptor = SkillDescriptor {
                name: "docker_shell".to_string(),
                description: "Execute a command inside a sandboxed Docker container".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The command to execute inside the Docker container"
                        }
                    },
                    "required": ["command"]
                }),
                required_capabilities: vec![Capability::ShellExec {
                    allowed_commands: vec!["*".into()],
                }],
            };

            assert_eq!(descriptor.name, skill_descriptor_name);
            assert!(descriptor.description.contains("sandboxed Docker"));
            assert_eq!(descriptor.required_capabilities.len(), 1);

            // Validate JSON schema structure
            let props = &descriptor.parameters_schema["properties"];
            assert!(props["command"]["type"].as_str() == Some("string"));
            let required = descriptor.parameters_schema["required"].as_array().unwrap();
            assert!(required.contains(&serde_json::json!("command")));
        }
    }
}

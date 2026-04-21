//! NDJSON protocol for headless agent communication.
//!
//! Enables SDK wrapping: external processes spawn `argentor` and communicate
//! via JSON Lines over stdin/stdout, like Claude Agent SDK wraps Claude Code.
//!
//! ## Message Types
//!
//! **Inbound (SDK -> Agent):**
//! - `init` -- initialize session with config
//! - `query` -- send user prompt
//! - `permission_response` -- respond to permission request
//! - `abort` -- cancel current operation
//!
//! **Outbound (Agent -> SDK):**
//! - `system` -- session lifecycle events
//! - `assistant` -- agent text output
//! - `tool_use` -- agent wants to call a tool
//! - `tool_result` -- tool execution result
//! - `permission_request` -- asking for permission
//! - `stream` -- partial streaming token
//! - `result` -- final output with metadata
//! - `error` -- error occurred
//! - `guardrail` -- guardrail violation report

use argentor_core::ArgentorResult;
use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════
// Inbound messages (SDK -> Agent)
// ═══════════════════════════════════════════════════

/// Messages sent from an external SDK to the Argentor agent process.
///
/// Each variant maps to a JSON object with a `"type"` discriminator field.
/// The SDK writes one JSON object per line (NDJSON) to the agent's stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InboundMessage {
    /// Initialize a new (or resume an existing) agent session.
    #[serde(rename = "init")]
    Init {
        /// Optional session ID to resume. `None` starts a new session.
        session_id: Option<String>,
        /// LLM provider name (e.g. `"claude"`, `"openai"`, `"groq"`).
        provider: String,
        /// Model identifier (e.g. `"claude-sonnet-4-20250514"`).
        model: String,
        /// API key for the provider.
        api_key: String,
        /// Optional system prompt override.
        system_prompt: Option<String>,
        /// Maximum agentic loop turns.
        max_turns: Option<u32>,
        /// Sampling temperature.
        temperature: Option<f32>,
        /// Tool names to enable, or `"builtins"` for all built-in tools.
        tools: Option<Vec<String>>,
        /// Permission mode: `"default"`, `"strict"`, `"permissive"`, `"plan"`.
        permission_mode: Option<String>,
        /// MCP server configurations to connect to.
        mcp_servers: Option<Vec<McpServerConfig>>,
        /// Working directory for file operations.
        working_directory: Option<String>,
    },
    /// Send a user prompt to the agent.
    #[serde(rename = "query")]
    Query {
        /// The user's prompt text.
        prompt: String,
        /// Whether to include streaming token events. Default: `false`.
        #[serde(default)]
        include_streaming: bool,
    },
    /// Respond to a permission request from the agent.
    #[serde(rename = "permission_response")]
    PermissionResponse {
        /// The request ID from the corresponding `permission_request`.
        request_id: String,
        /// Whether the permission is granted.
        allowed: bool,
        /// Optional reason for the decision.
        reason: Option<String>,
    },
    /// Abort the current operation.
    #[serde(rename = "abort")]
    Abort {
        /// Optional reason for aborting.
        reason: Option<String>,
    },
}

/// Configuration for an MCP server that the agent should connect to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Human-readable name for the MCP server.
    pub name: String,
    /// Command to spawn the MCP server process.
    pub command: String,
    /// Arguments to pass to the command.
    pub args: Vec<String>,
}

// ═══════════════════════════════════════════════════
// Outbound messages (Agent -> SDK)
// ═══════════════════════════════════════════════════

/// Messages sent from the Argentor agent to the wrapping SDK.
///
/// Each variant maps to a JSON object with a `"type"` discriminator field.
/// The agent writes one JSON object per line (NDJSON) to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutboundMessage {
    /// Session lifecycle event.
    #[serde(rename = "system")]
    System {
        /// The lifecycle event that occurred.
        event: SystemEvent,
        /// Session identifier.
        session_id: String,
        /// ISO 8601 timestamp.
        timestamp: String,
    },
    /// Agent text output (complete assistant turn).
    #[serde(rename = "assistant")]
    Assistant {
        /// The assistant's text response.
        text: String,
        /// Current turn number.
        turn: u32,
    },
    /// The agent wants to invoke a tool.
    #[serde(rename = "tool_use")]
    ToolUse {
        /// Tool name.
        name: String,
        /// JSON arguments for the tool.
        arguments: serde_json::Value,
        /// Unique call identifier.
        call_id: String,
        /// Current turn number.
        turn: u32,
    },
    /// Result from a tool execution.
    #[serde(rename = "tool_result")]
    ToolResult {
        /// Tool name.
        name: String,
        /// Call identifier matching the `tool_use`.
        call_id: String,
        /// Textual output from the tool.
        content: String,
        /// Whether the tool execution errored.
        is_error: bool,
        /// Wall-clock execution time in milliseconds.
        duration_ms: u64,
    },
    /// The agent is requesting permission to perform an action.
    #[serde(rename = "permission_request")]
    PermissionRequest {
        /// Unique request identifier (SDK must echo this back).
        request_id: String,
        /// Name of the tool requiring permission.
        tool_name: String,
        /// Arguments the tool would receive.
        arguments: serde_json::Value,
        /// Risk classification: `"low"`, `"medium"`, `"high"`, `"critical"`.
        risk_level: String,
    },
    /// Partial streaming token (only sent when `include_streaming` is true).
    #[serde(rename = "stream")]
    Stream {
        /// The text fragment.
        text: String,
        /// Monotonically increasing token index within this response.
        token_index: u64,
    },
    /// Final result with metadata, sent when the agent completes a query.
    #[serde(rename = "result")]
    Result {
        /// The final output text.
        output: String,
        /// Session identifier.
        session_id: String,
        /// Total turns used.
        turns: u32,
        /// Input tokens consumed.
        tokens_in: u64,
        /// Output tokens generated.
        tokens_out: u64,
        /// Estimated cost in USD.
        cost_usd: f64,
        /// Total wall-clock time in milliseconds.
        duration_ms: u64,
        /// Names of tools that were called during this query.
        tools_called: Vec<String>,
    },
    /// An error occurred.
    #[serde(rename = "error")]
    Error {
        /// Human-readable error message.
        message: String,
        /// Whether the session can continue after this error.
        recoverable: bool,
        /// Optional machine-readable error code.
        code: Option<String>,
    },
    /// Guardrail violation report.
    #[serde(rename = "guardrail")]
    Guardrail {
        /// Guardrail phase: `"input"`, `"output"`, `"tool_result"`.
        phase: String,
        /// List of violation descriptions.
        violations: Vec<String>,
        /// Whether the request was blocked.
        blocked: bool,
    },
}

/// Session lifecycle events emitted as `system` messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    /// A new session has started.
    #[serde(rename = "session_start")]
    SessionStart,
    /// An existing session has been resumed.
    #[serde(rename = "session_resume")]
    SessionResume,
    /// The session has ended.
    #[serde(rename = "session_end")]
    SessionEnd,
    /// The context window was compacted (older messages pruned).
    #[serde(rename = "context_compaction")]
    ContextCompaction,
    /// A new agentic loop turn has started.
    #[serde(rename = "turn_start")]
    TurnStart {
        /// The turn number (0-based).
        turn: u32,
    },
    /// A permission decision was acknowledged.
    #[serde(rename = "permission_decision")]
    PermissionDecision {
        /// The original request ID.
        request_id: String,
        /// Whether the action was allowed.
        allowed: bool,
        /// Optional reason for the decision.
        reason: Option<String>,
    },
}

// ═══════════════════════════════════════════════════
// Codec: encode / decode
// ═══════════════════════════════════════════════════

/// Encode an outbound message as a single NDJSON line (JSON + `\n`).
pub fn encode_message(msg: &OutboundMessage) -> ArgentorResult<String> {
    let json = serde_json::to_string(msg)?;
    Ok(format!("{json}\n"))
}

/// Decode an inbound message from a single NDJSON line.
///
/// Leading/trailing whitespace is trimmed before parsing.
pub fn decode_message(line: &str) -> ArgentorResult<InboundMessage> {
    let msg: InboundMessage = serde_json::from_str(line.trim())?;
    Ok(msg)
}

/// Encode an inbound message as NDJSON (useful for SDK-side serialization).
pub fn encode_inbound(msg: &InboundMessage) -> ArgentorResult<String> {
    let json = serde_json::to_string(msg)?;
    Ok(format!("{json}\n"))
}

/// Decode an outbound message from a single NDJSON line (useful for SDK-side parsing).
pub fn decode_outbound(line: &str) -> ArgentorResult<OutboundMessage> {
    let msg: OutboundMessage = serde_json::from_str(line.trim())?;
    Ok(msg)
}

// ═══════════════════════════════════════════════════
// Protocol handler
// ═══════════════════════════════════════════════════

use crate::backends::LlmBackend;
use crate::config::{LlmProvider, ModelConfig};
use crate::runner::AgentRunner;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Protocol handler that processes inbound NDJSON messages and yields outbound messages.
///
/// This is the core bridge between the NDJSON transport and the Argentor agent engine.
/// An SDK spawns `argentor` in headless mode, then sends [`InboundMessage`]s and
/// receives [`OutboundMessage`]s over stdin/stdout.
///
/// # Lifecycle
///
/// 1. SDK sends `init` -> handler creates `AgentRunner` + `Session`, returns `system{session_start}`
/// 2. SDK sends `query` -> handler runs the agentic loop, yields tool_use/tool_result/assistant/result
/// 3. SDK sends `abort` -> handler cancels and returns `system{session_end}`
pub struct ProtocolHandler {
    session: Option<Session>,
    runner: Option<AgentRunner>,
    config_received: bool,
    session_id_str: String,
    turn_counter: u32,
    tools_called: Vec<String>,
}

impl ProtocolHandler {
    /// Create a new protocol handler (uninitialized).
    ///
    /// The handler must receive an `Init` message before it can process queries.
    pub fn new() -> Self {
        Self {
            session: None,
            runner: None,
            config_received: false,
            session_id_str: String::new(),
            turn_counter: 0,
            tools_called: Vec::new(),
        }
    }

    /// Create a protocol handler with a pre-built backend (for testing).
    pub fn from_backend(backend: Box<dyn LlmBackend>, max_turns: u32) -> Self {
        let skills = Arc::new(SkillRegistry::new());
        let permissions = PermissionSet::new();
        let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-audit")));
        let runner = AgentRunner::from_backend(backend, skills, permissions, audit, max_turns);
        let session = Session::new();
        let session_id_str = session.id.to_string();

        Self {
            session: Some(session),
            runner: Some(runner),
            config_received: true,
            session_id_str,
            turn_counter: 0,
            tools_called: Vec::new(),
        }
    }

    /// Check if the handler has been initialized (received an `init` message).
    pub fn is_initialized(&self) -> bool {
        self.config_received && self.runner.is_some()
    }

    /// Get the current session ID (empty string if not initialized).
    pub fn session_id(&self) -> &str {
        &self.session_id_str
    }

    /// Process an inbound message and return zero or more outbound messages.
    pub async fn handle(&mut self, msg: InboundMessage) -> Vec<OutboundMessage> {
        match msg {
            InboundMessage::Init {
                session_id,
                provider,
                model,
                api_key,
                system_prompt,
                max_turns,
                temperature,
                tools: _tools,
                permission_mode: _permission_mode,
                mcp_servers: _mcp_servers,
                working_directory: _working_directory,
            } => self.handle_init(
                session_id,
                provider,
                model,
                api_key,
                system_prompt,
                max_turns,
                temperature,
            ),
            InboundMessage::Query {
                prompt,
                include_streaming: _include_streaming,
            } => self.handle_query(prompt).await,
            InboundMessage::PermissionResponse {
                request_id,
                allowed,
                reason,
            } => self.handle_permission_response(request_id, allowed, reason),
            InboundMessage::Abort { reason } => self.handle_abort(reason),
        }
    }

    /// Handle an `init` message: set up the agent runner and session.
    #[allow(clippy::too_many_arguments)]
    fn handle_init(
        &mut self,
        session_id: Option<String>,
        provider: String,
        model: String,
        api_key: String,
        system_prompt: Option<String>,
        max_turns: Option<u32>,
        temperature: Option<f32>,
    ) -> Vec<OutboundMessage> {
        let llm_provider = match provider.to_lowercase().as_str() {
            "claude" | "anthropic" => LlmProvider::Claude,
            "openai" | "gpt" => LlmProvider::OpenAi,
            "openrouter" => LlmProvider::OpenRouter,
            "groq" => LlmProvider::Groq,
            "claude_code" | "claudecode" => LlmProvider::ClaudeCode,
            "gemini" | "google" => LlmProvider::Gemini,
            "ollama" => LlmProvider::Ollama,
            "mistral" => LlmProvider::Mistral,
            "xai" | "grok" => LlmProvider::XAi,
            "azure" | "azure_openai" => LlmProvider::AzureOpenAi,
            "cerebras" => LlmProvider::Cerebras,
            "together" => LlmProvider::Together,
            "deepseek" => LlmProvider::DeepSeek,
            "vllm" => LlmProvider::VLlm,
            "fireworks" => LlmProvider::Fireworks,
            "huggingface" | "hugging_face" | "hf" => LlmProvider::HuggingFace,
            "cohere" => LlmProvider::Cohere,
            "bedrock" | "aws_bedrock" | "aws-bedrock" => LlmProvider::Bedrock,
            "replicate" => LlmProvider::Replicate,
            _ => {
                return vec![OutboundMessage::Error {
                    message: format!("Unknown provider: {provider}"),
                    recoverable: false,
                    code: Some("UNKNOWN_PROVIDER".to_string()),
                }];
            }
        };

        let config = ModelConfig {
            provider: llm_provider,
            model_id: model,
            api_key,
            api_base_url: None,
            temperature: temperature.unwrap_or(0.7),
            max_tokens: 4096,
            max_turns: max_turns.unwrap_or(20),
            max_context_tokens: 200_000,
            fallback_models: vec![],
            retry_policy: None,
        };

        let skills = Arc::new(SkillRegistry::new());
        let permissions = PermissionSet::new();
        let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-audit")));

        let mut runner = AgentRunner::new(config, skills, permissions, audit);
        if let Some(prompt) = system_prompt {
            runner = runner.with_system_prompt(prompt);
        }

        let (session, event) = if let Some(sid) = session_id {
            // Attempt to resume an existing session
            let mut s = Session::new();
            // Store the requested session ID in metadata for tracking
            s.metadata.insert(
                "original_session_id".to_string(),
                serde_json::Value::String(sid),
            );
            (s, SystemEvent::SessionResume)
        } else {
            (Session::new(), SystemEvent::SessionStart)
        };

        self.session_id_str = session.id.to_string();
        self.session = Some(session);
        self.runner = Some(runner);
        self.config_received = true;
        self.turn_counter = 0;
        self.tools_called.clear();

        let timestamp = chrono::Utc::now().to_rfc3339();

        vec![OutboundMessage::System {
            event,
            session_id: self.session_id_str.clone(),
            timestamp,
        }]
    }

    /// Handle a `query` message: run the agentic loop and collect results.
    async fn handle_query(&mut self, prompt: String) -> Vec<OutboundMessage> {
        if !self.is_initialized() {
            return vec![OutboundMessage::Error {
                message: "Handler not initialized. Send an 'init' message first.".to_string(),
                recoverable: true,
                code: Some("NOT_INITIALIZED".to_string()),
            }];
        }

        // Safety: early return above guarantees runner and session are Some
        #[allow(clippy::unwrap_used)]
        let runner = self.runner.as_ref().unwrap();
        #[allow(clippy::unwrap_used)]
        let session = self.session.as_mut().unwrap();

        let start = Instant::now();
        self.turn_counter += 1;

        let mut messages = Vec::new();

        // Emit turn_start
        messages.push(OutboundMessage::System {
            event: SystemEvent::TurnStart {
                turn: self.turn_counter,
            },
            session_id: self.session_id_str.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

        // Run the agentic loop
        match runner.run(session, &prompt).await {
            Ok(output) => {
                let duration_ms = start.elapsed().as_millis() as u64;

                messages.push(OutboundMessage::Assistant {
                    text: output.clone(),
                    turn: self.turn_counter,
                });

                // Estimate tokens from output length (1 token ≈ 4 chars)
                let prompt_len = prompt.len() as u64;
                let output_len = output.len() as u64;
                let est_tokens_in = prompt_len / 4 + 100; // prompt + overhead
                let est_tokens_out = output_len / 4;
                let est_cost = (est_tokens_in + est_tokens_out) as f64 * 0.003 / 1000.0;

                messages.push(OutboundMessage::Result {
                    output,
                    session_id: self.session_id_str.clone(),
                    turns: self.turn_counter,
                    tokens_in: est_tokens_in,
                    tokens_out: est_tokens_out,
                    cost_usd: est_cost,
                    duration_ms,
                    tools_called: self.tools_called.clone(),
                });
            }
            Err(e) => {
                messages.push(OutboundMessage::Error {
                    message: e.to_string(),
                    recoverable: true,
                    code: None,
                });
            }
        }

        messages
    }

    /// Handle a `permission_response` message.
    fn handle_permission_response(
        &mut self,
        request_id: String,
        allowed: bool,
        reason: Option<String>,
    ) -> Vec<OutboundMessage> {
        if !self.is_initialized() {
            return vec![OutboundMessage::Error {
                message: "Handler not initialized.".to_string(),
                recoverable: true,
                code: Some("NOT_INITIALIZED".to_string()),
            }];
        }

        // Log the permission decision (actual unblocking would require async channels)
        let action = if allowed { "granted" } else { "denied" };
        let reason_text = reason.unwrap_or_default();
        tracing::info!(
            request_id = %request_id,
            action = %action,
            reason = %reason_text,
            "Permission response received"
        );

        // Acknowledge the decision back to the SDK
        vec![OutboundMessage::System {
            event: SystemEvent::PermissionDecision {
                request_id,
                allowed,
                reason: if reason_text.is_empty() {
                    None
                } else {
                    Some(reason_text)
                },
            },
            session_id: self.session_id_str.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }]
    }

    /// Handle an `abort` message: end the session.
    fn handle_abort(&mut self, reason: Option<String>) -> Vec<OutboundMessage> {
        let reason_text = reason.unwrap_or_else(|| "User abort".to_string());
        tracing::info!(reason = %reason_text, "Session aborted");

        let timestamp = chrono::Utc::now().to_rfc3339();

        let msg = OutboundMessage::System {
            event: SystemEvent::SessionEnd,
            session_id: self.session_id_str.clone(),
            timestamp,
        };

        // Clean up
        self.session = None;
        self.runner = None;
        self.config_received = false;

        vec![msg]
    }
}

impl Default for ProtocolHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::backends::LlmBackend;
    use crate::llm::LlmResponse;
    use crate::stream::StreamEvent;
    use argentor_core::{ArgentorResult, Message};
    use argentor_skills::SkillDescriptor;
    use async_trait::async_trait;
    use tokio::sync::mpsc;

    // -- Mock backend for protocol tests ------------------------------------

    struct FixedBackend {
        response: String,
    }

    impl FixedBackend {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[async_trait]
    impl LlmBackend for FixedBackend {
        async fn chat(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            Ok(LlmResponse::Done(self.response.clone()))
        }

        fn provider_name(&self) -> &str {
            "fixed-test"
        }

        async fn chat_stream(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<(
            mpsc::Receiver<StreamEvent>,
            tokio::task::JoinHandle<ArgentorResult<LlmResponse>>,
        )> {
            let (tx, rx) = mpsc::channel(1);
            let resp = self.response.clone();
            let handle = tokio::spawn(async move {
                let _ = tx.send(StreamEvent::Done).await;
                Ok(LlmResponse::Done(resp))
            });
            Ok((rx, handle))
        }
    }

    // ═══════════════════════════════════════════════════
    // Inbound message serialization / deserialization
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_inbound_init_serialize_roundtrip() {
        let msg = InboundMessage::Init {
            session_id: None,
            provider: "claude".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key: "sk-test-123".to_string(),
            system_prompt: Some("You are helpful".to_string()),
            max_turns: Some(10),
            temperature: Some(0.5),
            tools: Some(vec!["builtins".to_string()]),
            permission_mode: Some("default".to_string()),
            mcp_servers: None,
            working_directory: Some("/tmp".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: InboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            InboundMessage::Init {
                provider, model, ..
            } => {
                assert_eq!(provider, "claude");
                assert_eq!(model, "claude-sonnet-4-20250514");
            }
            _ => panic!("Expected Init variant"),
        }
    }

    #[test]
    fn test_inbound_init_minimal() {
        let json = r#"{"type":"init","provider":"openai","model":"gpt-4o","api_key":"key"}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::Init {
                session_id,
                provider,
                model,
                system_prompt,
                max_turns,
                temperature,
                tools,
                permission_mode,
                mcp_servers,
                working_directory,
                ..
            } => {
                assert_eq!(provider, "openai");
                assert_eq!(model, "gpt-4o");
                assert!(session_id.is_none());
                assert!(system_prompt.is_none());
                assert!(max_turns.is_none());
                assert!(temperature.is_none());
                assert!(tools.is_none());
                assert!(permission_mode.is_none());
                assert!(mcp_servers.is_none());
                assert!(working_directory.is_none());
            }
            _ => panic!("Expected Init variant"),
        }
    }

    #[test]
    fn test_inbound_query_serialize_roundtrip() {
        let msg = InboundMessage::Query {
            prompt: "What is 2 + 2?".to_string(),
            include_streaming: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: InboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            InboundMessage::Query {
                prompt,
                include_streaming,
            } => {
                assert_eq!(prompt, "What is 2 + 2?");
                assert!(include_streaming);
            }
            _ => panic!("Expected Query variant"),
        }
    }

    #[test]
    fn test_inbound_query_default_streaming() {
        let json = r#"{"type":"query","prompt":"Hello"}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::Query {
                include_streaming, ..
            } => {
                assert!(!include_streaming);
            }
            _ => panic!("Expected Query variant"),
        }
    }

    #[test]
    fn test_inbound_permission_response_serialize_roundtrip() {
        let msg = InboundMessage::PermissionResponse {
            request_id: "req-42".to_string(),
            allowed: true,
            reason: Some("User approved".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: InboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            InboundMessage::PermissionResponse {
                request_id,
                allowed,
                reason,
            } => {
                assert_eq!(request_id, "req-42");
                assert!(allowed);
                assert_eq!(reason.unwrap(), "User approved");
            }
            _ => panic!("Expected PermissionResponse variant"),
        }
    }

    #[test]
    fn test_inbound_permission_response_denied() {
        let json =
            r#"{"type":"permission_response","request_id":"r1","allowed":false,"reason":null}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::PermissionResponse {
                allowed, reason, ..
            } => {
                assert!(!allowed);
                assert!(reason.is_none());
            }
            _ => panic!("Expected PermissionResponse variant"),
        }
    }

    #[test]
    fn test_inbound_abort_serialize_roundtrip() {
        let msg = InboundMessage::Abort {
            reason: Some("timeout".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: InboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            InboundMessage::Abort { reason } => {
                assert_eq!(reason.unwrap(), "timeout");
            }
            _ => panic!("Expected Abort variant"),
        }
    }

    #[test]
    fn test_inbound_abort_no_reason() {
        let json = r#"{"type":"abort","reason":null}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::Abort { reason } => {
                assert!(reason.is_none());
            }
            _ => panic!("Expected Abort variant"),
        }
    }

    // ═══════════════════════════════════════════════════
    // Outbound message serialization / deserialization
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_outbound_system_serialize_roundtrip() {
        let msg = OutboundMessage::System {
            event: SystemEvent::SessionStart,
            session_id: "sess-1".to_string(),
            timestamp: "2026-04-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"system"#));
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::System {
                event, session_id, ..
            } => {
                assert!(matches!(event, SystemEvent::SessionStart));
                assert_eq!(session_id, "sess-1");
            }
            _ => panic!("Expected System variant"),
        }
    }

    #[test]
    fn test_outbound_assistant_serialize_roundtrip() {
        let msg = OutboundMessage::Assistant {
            text: "The answer is 4.".to_string(),
            turn: 1,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::Assistant { text, turn } => {
                assert_eq!(text, "The answer is 4.");
                assert_eq!(turn, 1);
            }
            _ => panic!("Expected Assistant variant"),
        }
    }

    #[test]
    fn test_outbound_tool_use_serialize_roundtrip() {
        let msg = OutboundMessage::ToolUse {
            name: "calculator".to_string(),
            arguments: serde_json::json!({"expression": "2+2"}),
            call_id: "call-1".to_string(),
            turn: 1,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::ToolUse {
                name,
                arguments,
                call_id,
                turn,
            } => {
                assert_eq!(name, "calculator");
                assert_eq!(arguments["expression"], "2+2");
                assert_eq!(call_id, "call-1");
                assert_eq!(turn, 1);
            }
            _ => panic!("Expected ToolUse variant"),
        }
    }

    #[test]
    fn test_outbound_tool_result_serialize_roundtrip() {
        let msg = OutboundMessage::ToolResult {
            name: "calculator".to_string(),
            call_id: "call-1".to_string(),
            content: "4".to_string(),
            is_error: false,
            duration_ms: 12,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::ToolResult {
                name,
                content,
                is_error,
                duration_ms,
                ..
            } => {
                assert_eq!(name, "calculator");
                assert_eq!(content, "4");
                assert!(!is_error);
                assert_eq!(duration_ms, 12);
            }
            _ => panic!("Expected ToolResult variant"),
        }
    }

    #[test]
    fn test_outbound_permission_request_serialize_roundtrip() {
        let msg = OutboundMessage::PermissionRequest {
            request_id: "perm-1".to_string(),
            tool_name: "file_write".to_string(),
            arguments: serde_json::json!({"path": "/etc/passwd"}),
            risk_level: "critical".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::PermissionRequest {
                request_id,
                tool_name,
                risk_level,
                ..
            } => {
                assert_eq!(request_id, "perm-1");
                assert_eq!(tool_name, "file_write");
                assert_eq!(risk_level, "critical");
            }
            _ => panic!("Expected PermissionRequest variant"),
        }
    }

    #[test]
    fn test_outbound_stream_serialize_roundtrip() {
        let msg = OutboundMessage::Stream {
            text: "Hello".to_string(),
            token_index: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::Stream { text, token_index } => {
                assert_eq!(text, "Hello");
                assert_eq!(token_index, 42);
            }
            _ => panic!("Expected Stream variant"),
        }
    }

    #[test]
    fn test_outbound_result_serialize_roundtrip() {
        let msg = OutboundMessage::Result {
            output: "Done!".to_string(),
            session_id: "sess-1".to_string(),
            turns: 3,
            tokens_in: 100,
            tokens_out: 50,
            cost_usd: 0.003,
            duration_ms: 1500,
            tools_called: vec!["echo".to_string(), "time".to_string()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::Result {
                output,
                turns,
                tokens_in,
                tokens_out,
                cost_usd,
                tools_called,
                ..
            } => {
                assert_eq!(output, "Done!");
                assert_eq!(turns, 3);
                assert_eq!(tokens_in, 100);
                assert_eq!(tokens_out, 50);
                assert!((cost_usd - 0.003).abs() < f64::EPSILON);
                assert_eq!(tools_called.len(), 2);
            }
            _ => panic!("Expected Result variant"),
        }
    }

    #[test]
    fn test_outbound_error_serialize_roundtrip() {
        let msg = OutboundMessage::Error {
            message: "Rate limit exceeded".to_string(),
            recoverable: true,
            code: Some("RATE_LIMIT".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::Error {
                message,
                recoverable,
                code,
            } => {
                assert_eq!(message, "Rate limit exceeded");
                assert!(recoverable);
                assert_eq!(code.unwrap(), "RATE_LIMIT");
            }
            _ => panic!("Expected Error variant"),
        }
    }

    #[test]
    fn test_outbound_error_no_code() {
        let msg = OutboundMessage::Error {
            message: "Unknown".to_string(),
            recoverable: false,
            code: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""code":null"#));
    }

    #[test]
    fn test_outbound_guardrail_serialize_roundtrip() {
        let msg = OutboundMessage::Guardrail {
            phase: "input".to_string(),
            violations: vec!["PII detected: email".to_string()],
            blocked: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::Guardrail {
                phase,
                violations,
                blocked,
            } => {
                assert_eq!(phase, "input");
                assert_eq!(violations.len(), 1);
                assert!(blocked);
            }
            _ => panic!("Expected Guardrail variant"),
        }
    }

    // ═══════════════════════════════════════════════════
    // SystemEvent serialization
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_system_event_session_start() {
        let event = SystemEvent::SessionStart;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#""session_start""#);
    }

    #[test]
    fn test_system_event_session_resume() {
        let event = SystemEvent::SessionResume;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#""session_resume""#);
    }

    #[test]
    fn test_system_event_session_end() {
        let event = SystemEvent::SessionEnd;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#""session_end""#);
    }

    #[test]
    fn test_system_event_context_compaction() {
        let event = SystemEvent::ContextCompaction;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#""context_compaction""#);
    }

    #[test]
    fn test_system_event_turn_start() {
        let event = SystemEvent::TurnStart { turn: 5 };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SystemEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            SystemEvent::TurnStart { turn } => assert_eq!(turn, 5),
            _ => panic!("Expected TurnStart"),
        }
    }

    // ═══════════════════════════════════════════════════
    // McpServerConfig
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_mcp_server_config_serialize_roundtrip() {
        let config = McpServerConfig {
            name: "filesystem".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "/tmp".to_string(),
            ],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "filesystem");
        assert_eq!(parsed.command, "npx");
        assert_eq!(parsed.args.len(), 3);
    }

    #[test]
    fn test_mcp_server_config_in_init() {
        let json = r#"{
            "type": "init",
            "provider": "claude",
            "model": "claude-sonnet-4-20250514",
            "api_key": "key",
            "mcp_servers": [
                {"name": "fs", "command": "npx", "args": ["-y", "fs-server"]}
            ]
        }"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::Init { mcp_servers, .. } => {
                let servers = mcp_servers.unwrap();
                assert_eq!(servers.len(), 1);
                assert_eq!(servers[0].name, "fs");
            }
            _ => panic!("Expected Init"),
        }
    }

    // ═══════════════════════════════════════════════════
    // encode_message / decode_message
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_encode_message_ends_with_newline() {
        let msg = OutboundMessage::Assistant {
            text: "Hi".to_string(),
            turn: 1,
        };
        let encoded = encode_message(&msg).unwrap();
        assert!(encoded.ends_with('\n'));
        assert_eq!(encoded.matches('\n').count(), 1);
    }

    #[test]
    fn test_encode_message_is_valid_json() {
        let msg = OutboundMessage::Error {
            message: "oops".to_string(),
            recoverable: false,
            code: None,
        };
        let encoded = encode_message(&msg).unwrap();
        let trimmed = encoded.trim();
        let _: serde_json::Value = serde_json::from_str(trimmed).unwrap();
    }

    #[test]
    fn test_decode_message_trims_whitespace() {
        let json = r#"  {"type":"query","prompt":"hello"}  "#;
        let msg = decode_message(json).unwrap();
        match msg {
            InboundMessage::Query { prompt, .. } => {
                assert_eq!(prompt, "hello");
            }
            _ => panic!("Expected Query"),
        }
    }

    #[test]
    fn test_decode_message_trims_newline() {
        let json = "{\"type\":\"abort\",\"reason\":null}\n";
        let msg = decode_message(json).unwrap();
        assert!(matches!(msg, InboundMessage::Abort { .. }));
    }

    #[test]
    fn test_decode_message_invalid_json_returns_error() {
        let result = decode_message("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_message_unknown_type_returns_error() {
        let result = decode_message(r#"{"type":"unknown_xyz","data":1}"#);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════
    // Round-trip encode/decode
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_roundtrip_inbound_encode_decode() {
        let msg = InboundMessage::Query {
            prompt: "What's the weather?".to_string(),
            include_streaming: false,
        };
        let encoded = encode_inbound(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        match decoded {
            InboundMessage::Query { prompt, .. } => {
                assert_eq!(prompt, "What's the weather?");
            }
            _ => panic!("Expected Query"),
        }
    }

    #[test]
    fn test_roundtrip_outbound_encode_decode() {
        let msg = OutboundMessage::Result {
            output: "Result text".to_string(),
            session_id: "s-123".to_string(),
            turns: 2,
            tokens_in: 50,
            tokens_out: 25,
            cost_usd: 0.001,
            duration_ms: 800,
            tools_called: vec!["echo".to_string()],
        };
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_outbound(&encoded).unwrap();
        match decoded {
            OutboundMessage::Result { output, turns, .. } => {
                assert_eq!(output, "Result text");
                assert_eq!(turns, 2);
            }
            _ => panic!("Expected Result"),
        }
    }

    // ═══════════════════════════════════════════════════
    // ProtocolHandler tests
    // ═══════════════════════════════════════════════════

    #[test]
    fn test_handler_new_is_not_initialized() {
        let handler = ProtocolHandler::new();
        assert!(!handler.is_initialized());
        assert!(handler.session_id().is_empty());
    }

    #[test]
    fn test_handler_default_is_not_initialized() {
        let handler = ProtocolHandler::default();
        assert!(!handler.is_initialized());
    }

    #[tokio::test]
    async fn test_handler_init_creates_session() {
        let mut handler = ProtocolHandler::new();
        let msgs = handler
            .handle(InboundMessage::Init {
                session_id: None,
                provider: "claude".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                api_key: "test-key".to_string(),
                system_prompt: None,
                max_turns: None,
                temperature: None,
                tools: None,
                permission_mode: None,
                mcp_servers: None,
                working_directory: None,
            })
            .await;

        assert_eq!(msgs.len(), 1);
        assert!(handler.is_initialized());
        assert!(!handler.session_id().is_empty());

        match &msgs[0] {
            OutboundMessage::System { event, .. } => {
                assert!(matches!(event, SystemEvent::SessionStart));
            }
            _ => panic!("Expected System message"),
        }
    }

    #[tokio::test]
    async fn test_handler_init_with_session_resume() {
        let mut handler = ProtocolHandler::new();
        let msgs = handler
            .handle(InboundMessage::Init {
                session_id: Some("existing-session".to_string()),
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                api_key: "key".to_string(),
                system_prompt: None,
                max_turns: None,
                temperature: None,
                tools: None,
                permission_mode: None,
                mcp_servers: None,
                working_directory: None,
            })
            .await;

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            OutboundMessage::System { event, .. } => {
                assert!(matches!(event, SystemEvent::SessionResume));
            }
            _ => panic!("Expected System message with SessionResume"),
        }
    }

    #[tokio::test]
    async fn test_handler_init_unknown_provider() {
        let mut handler = ProtocolHandler::new();
        let msgs = handler
            .handle(InboundMessage::Init {
                session_id: None,
                provider: "nonexistent_provider".to_string(),
                model: "m".to_string(),
                api_key: "k".to_string(),
                system_prompt: None,
                max_turns: None,
                temperature: None,
                tools: None,
                permission_mode: None,
                mcp_servers: None,
                working_directory: None,
            })
            .await;

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            OutboundMessage::Error { code, .. } => {
                assert_eq!(code.as_deref(), Some("UNKNOWN_PROVIDER"));
            }
            _ => panic!("Expected Error"),
        }
        assert!(!handler.is_initialized());
    }

    #[tokio::test]
    async fn test_handler_query_without_init_returns_error() {
        let mut handler = ProtocolHandler::new();
        let msgs = handler
            .handle(InboundMessage::Query {
                prompt: "hi".to_string(),
                include_streaming: false,
            })
            .await;

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            OutboundMessage::Error { code, .. } => {
                assert_eq!(code.as_deref(), Some("NOT_INITIALIZED"));
            }
            _ => panic!("Expected Error"),
        }
    }

    #[tokio::test]
    async fn test_handler_query_with_mock_backend() {
        let backend = Box::new(FixedBackend::new("Mock response"));
        let mut handler = ProtocolHandler::from_backend(backend, 5);

        assert!(handler.is_initialized());

        let msgs = handler
            .handle(InboundMessage::Query {
                prompt: "Hello, world!".to_string(),
                include_streaming: false,
            })
            .await;

        // Should get: system(turn_start), assistant, result
        assert!(msgs.len() >= 2);

        // First should be turn_start
        match &msgs[0] {
            OutboundMessage::System { event, .. } => {
                assert!(matches!(event, SystemEvent::TurnStart { turn: 1 }));
            }
            _ => panic!("Expected System(TurnStart), got {:?}", msgs[0]),
        }

        // Should contain an assistant message
        let has_assistant = msgs
            .iter()
            .any(|m| matches!(m, OutboundMessage::Assistant { .. }));
        assert!(has_assistant, "Expected at least one Assistant message");

        // Should contain a result message
        let has_result = msgs
            .iter()
            .any(|m| matches!(m, OutboundMessage::Result { .. }));
        assert!(has_result, "Expected a Result message");
    }

    #[tokio::test]
    async fn test_handler_abort_returns_session_end() {
        let mut handler = ProtocolHandler::new();
        // Init first
        handler
            .handle(InboundMessage::Init {
                session_id: None,
                provider: "ollama".to_string(),
                model: "llama3".to_string(),
                api_key: "".to_string(),
                system_prompt: None,
                max_turns: None,
                temperature: None,
                tools: None,
                permission_mode: None,
                mcp_servers: None,
                working_directory: None,
            })
            .await;

        assert!(handler.is_initialized());

        let msgs = handler
            .handle(InboundMessage::Abort {
                reason: Some("User cancelled".to_string()),
            })
            .await;

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            OutboundMessage::System { event, .. } => {
                assert!(matches!(event, SystemEvent::SessionEnd));
            }
            _ => panic!("Expected System(SessionEnd)"),
        }

        // Handler should no longer be initialized
        assert!(!handler.is_initialized());
    }

    #[tokio::test]
    async fn test_handler_permission_response_without_init() {
        let mut handler = ProtocolHandler::new();
        let msgs = handler
            .handle(InboundMessage::PermissionResponse {
                request_id: "r1".to_string(),
                allowed: true,
                reason: None,
            })
            .await;

        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            OutboundMessage::Error { code, .. } => {
                assert_eq!(code.as_deref(), Some("NOT_INITIALIZED"));
            }
            _ => panic!("Expected Error"),
        }
    }

    #[tokio::test]
    async fn test_handler_multiple_queries() {
        let backend = Box::new(FixedBackend::new("Response"));
        let mut handler = ProtocolHandler::from_backend(backend, 5);

        // First query
        let msgs1 = handler
            .handle(InboundMessage::Query {
                prompt: "First".to_string(),
                include_streaming: false,
            })
            .await;
        assert!(!msgs1.is_empty());

        // Second query
        let msgs2 = handler
            .handle(InboundMessage::Query {
                prompt: "Second".to_string(),
                include_streaming: false,
            })
            .await;
        assert!(!msgs2.is_empty());

        // Turn counter should advance
        let turn_events: Vec<_> = msgs2
            .iter()
            .filter_map(|m| match m {
                OutboundMessage::System {
                    event: SystemEvent::TurnStart { turn },
                    ..
                } => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(turn_events, vec![2]);
    }

    #[tokio::test]
    async fn test_handler_result_contains_session_id() {
        let backend = Box::new(FixedBackend::new("OK"));
        let mut handler = ProtocolHandler::from_backend(backend, 5);
        let session_id = handler.session_id().to_string();

        let msgs = handler
            .handle(InboundMessage::Query {
                prompt: "test".to_string(),
                include_streaming: false,
            })
            .await;

        let result_msg = msgs
            .iter()
            .find(|m| matches!(m, OutboundMessage::Result { .. }))
            .expect("Expected Result message");

        match result_msg {
            OutboundMessage::Result {
                session_id: sid, ..
            } => {
                assert_eq!(*sid, session_id);
            }
            _ => unreachable!(),
        }
    }
}

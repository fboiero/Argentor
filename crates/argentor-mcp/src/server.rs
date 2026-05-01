//! MCP Server — exposes Argentor skills as MCP tools over JSON-RPC 2.0 / stdio.
//!
//! This allows external clients (Claude Code, Cursor, etc.) to connect to
//! Argentor and use its registered skills via the Model Context Protocol.

use crate::protocol::McpToolDef;
use argentor_core::{ArgentorError, ArgentorResult, ToolCall};
use argentor_security::PermissionSet;
use argentor_skills::{SkillDescriptor, SkillRegistry};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Server-side JSON-RPC types
// ---------------------------------------------------------------------------

/// Incoming JSON-RPC 2.0 request (deserialization-friendly).
///
/// Separate from [`crate::protocol::JsonRpcRequest`] which is `Serialize`-only
/// (uses `&'static str` for `jsonrpc`).
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingRequest {
    /// Protocol version (expected `"2.0"`).
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Request id. `None` for notifications.
    pub id: Option<u64>,
    /// RPC method name.
    pub method: String,
    /// Optional method parameters.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Outgoing JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize)]
pub struct OutgoingResponse {
    /// Protocol version (always `"2.0"`).
    pub jsonrpc: &'static str,
    /// Request id this response corresponds to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// Successful result payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OutgoingError>,
}

/// Outgoing JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize)]
pub struct OutgoingError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl OutgoingResponse {
    /// Build a success response.
    pub(crate) fn success(id: u64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response.
    pub(crate) fn error(id: Option<u64>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(OutgoingError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// JSON-RPC standard error codes
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const INVALID_REQUEST: i64 = -32600;
const PARSE_ERROR: i64 = -32700;

// Application-level error codes
const TOOL_NOT_FOUND: i64 = -32001;
const PERMISSION_DENIED: i64 = -32002;

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

/// MCP Server that exposes Argentor's registered skills as MCP tools.
///
/// Implements the server side of the Model Context Protocol over JSON-RPC 2.0
/// stdio, allowing external clients to discover and invoke skills.
pub struct McpServer {
    name: String,
    version: String,
    skills: Arc<SkillRegistry>,
    permissions: PermissionSet,
}

impl McpServer {
    /// Create a new MCP server.
    ///
    /// # Arguments
    /// - `name` — server name reported in `initialize` response
    /// - `version` — server version reported in `initialize` response
    /// - `skills` — the skill registry whose skills will be exposed as MCP tools
    /// - `permissions` — permission set used to gate tool execution
    pub fn new(
        name: &str,
        version: &str,
        skills: Arc<SkillRegistry>,
        permissions: PermissionSet,
    ) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            skills,
            permissions,
        }
    }

    /// Run the MCP server over stdio (JSON-RPC line protocol).
    ///
    /// Reads newline-delimited JSON-RPC messages from stdin and writes
    /// responses to stdout.  Runs until stdin is closed.
    pub async fn run_stdio(&self) -> ArgentorResult<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        info!(server = %self.name, "MCP server started on stdio");

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    info!("MCP server: stdin closed, shutting down");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Parse the incoming request
                    let request = match serde_json::from_str::<IncomingRequest>(trimmed) {
                        Ok(req) => req,
                        Err(e) => {
                            debug!(error = %e, line = %trimmed, "Failed to parse JSON-RPC request");
                            let resp = OutgoingResponse::error(
                                None,
                                PARSE_ERROR,
                                format!("Parse error: {e}"),
                            );
                            let msg = serde_json::to_string(&resp).unwrap_or_default();
                            let _ = stdout.write_all(msg.as_bytes()).await;
                            let _ = stdout.write_all(b"\n").await;
                            let _ = stdout.flush().await;
                            continue;
                        }
                    };

                    // Handle the request
                    if let Some(response) = self.handle_request(request).await {
                        let msg = serde_json::to_string(&response).map_err(|e| {
                            ArgentorError::Skill(format!("Failed to serialize response: {e}"))
                        })?;
                        stdout.write_all(msg.as_bytes()).await.map_err(|e| {
                            ArgentorError::Skill(format!("Failed to write to stdout: {e}"))
                        })?;
                        stdout.write_all(b"\n").await.map_err(|e| {
                            ArgentorError::Skill(format!("Failed to write newline: {e}"))
                        })?;
                        stdout.flush().await.map_err(|e| {
                            ArgentorError::Skill(format!("Failed to flush stdout: {e}"))
                        })?;
                    }
                    // Notifications (no id) that don't need a response return None
                }
                Err(e) => {
                    error!(error = %e, "Error reading stdin");
                    return Err(ArgentorError::Skill(format!(
                        "Failed to read from stdin: {e}"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Dispatch an incoming JSON-RPC request to the appropriate handler.
    ///
    /// Returns `None` for notifications that require no response (e.g.
    /// `notifications/initialized`).
    pub async fn handle_request(&self, request: IncomingRequest) -> Option<OutgoingResponse> {
        debug!(method = %request.method, id = ?request.id, "Handling MCP request");

        match request.method.as_str() {
            "initialize" => Some(self.handle_initialize(request)),
            "notifications/initialized" => {
                info!("MCP client acknowledged initialization");
                None // Notification — no response
            }
            "ping" => Some(self.handle_ping(request)),
            "tools/list" => Some(self.handle_tools_list(request)),
            "tools/call" => Some(self.handle_tools_call(request).await),
            _ => {
                warn!(method = %request.method, "Unknown MCP method");
                match request.id {
                    Some(id) => Some(OutgoingResponse::error(
                        Some(id),
                        METHOD_NOT_FOUND,
                        format!("Method not found: {}", request.method),
                    )),
                    None => None, // Unknown notification — silently ignore
                }
            }
        }
    }

    /// Handle `initialize` — return server capabilities.
    fn handle_initialize(&self, request: IncomingRequest) -> OutgoingResponse {
        let id = match request.id {
            Some(id) => id,
            None => {
                return OutgoingResponse::error(
                    None,
                    INVALID_REQUEST,
                    "initialize must be a request (must include id)",
                );
            }
        };

        info!(server = %self.name, version = %self.version, "Handling MCP initialize");

        OutgoingResponse::success(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": self.name,
                    "version": self.version
                }
            }),
        )
    }

    /// Handle `ping` — respond with empty result (pong).
    fn handle_ping(&self, request: IncomingRequest) -> OutgoingResponse {
        let id = match request.id {
            Some(id) => id,
            None => {
                return OutgoingResponse::error(
                    None,
                    INVALID_REQUEST,
                    "ping must be a request (must include id)",
                );
            }
        };

        debug!("Handling MCP ping");
        OutgoingResponse::success(id, serde_json::json!({}))
    }

    /// Handle `tools/list` — return all skills as MCP tool descriptors.
    fn handle_tools_list(&self, request: IncomingRequest) -> OutgoingResponse {
        let id = match request.id {
            Some(id) => id,
            None => {
                return OutgoingResponse::error(
                    None,
                    INVALID_REQUEST,
                    "tools/list must be a request (must include id)",
                );
            }
        };

        let descriptors = self.skills.list_descriptors();
        let tools: Vec<McpToolDef> = descriptors.iter().map(Self::skill_to_mcp_tool).collect();

        OutgoingResponse::success(id, serde_json::json!({ "tools": tools }))
    }

    /// Handle `tools/call` — execute a skill and return the result.
    async fn handle_tools_call(&self, request: IncomingRequest) -> OutgoingResponse {
        let id = match request.id {
            Some(id) => id,
            None => {
                return OutgoingResponse::error(
                    None,
                    INVALID_REQUEST,
                    "tools/call must be a request (must include id)",
                );
            }
        };

        let params = match request.params {
            Some(p) => p,
            None => {
                return OutgoingResponse::error(
                    Some(id),
                    INVALID_PARAMS,
                    "tools/call requires params with 'name' and 'arguments'",
                );
            }
        };

        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(name) => name.to_string(),
            None => {
                return OutgoingResponse::error(
                    Some(id),
                    INVALID_PARAMS,
                    "Missing 'name' in tools/call params",
                );
            }
        };

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        // Build an internal ToolCall
        let call = ToolCall {
            id: format!("mcp-{id}"),
            name: tool_name.clone(),
            arguments,
        };

        info!(tool = %tool_name, "Executing MCP tool call");

        // Execute through the skill registry (handles permission checks)
        match self.skills.execute(call, &self.permissions).await {
            Ok(result) => {
                let content = serde_json::json!([{
                    "type": "text",
                    "text": result.content
                }]);

                OutgoingResponse::success(
                    id,
                    serde_json::json!({
                        "content": content,
                        "isError": result.is_error
                    }),
                )
            }
            Err(e) => {
                let error_msg = e.to_string();
                // Check if it's a "not found" error
                if error_msg.contains("Unknown skill") {
                    OutgoingResponse::error(
                        Some(id),
                        TOOL_NOT_FOUND,
                        format!("Tool not found: {tool_name}"),
                    )
                } else if error_msg.contains("Permission denied") {
                    OutgoingResponse::error(
                        Some(id),
                        PERMISSION_DENIED,
                        format!("Permission denied: {tool_name}"),
                    )
                } else {
                    OutgoingResponse::error(
                        Some(id),
                        INTERNAL_ERROR,
                        format!("Tool execution failed: {error_msg}"),
                    )
                }
            }
        }
    }

    /// Convert a [`SkillDescriptor`] to an MCP [`McpToolDef`].
    pub fn skill_to_mcp_tool(descriptor: &SkillDescriptor) -> McpToolDef {
        McpToolDef {
            name: descriptor.name.clone(),
            description: descriptor.description.clone(),
            input_schema: descriptor.parameters_schema.clone(),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_core::{ArgentorResult, ToolCall, ToolResult};
    use argentor_security::{Capability, PermissionSet};
    use argentor_skills::{Skill, SkillDescriptor, SkillRegistry};
    use async_trait::async_trait;

    // -----------------------------------------------------------------------
    // Test skill implementations
    // -----------------------------------------------------------------------

    struct EchoSkill {
        descriptor: SkillDescriptor,
    }

    impl EchoSkill {
        fn new() -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: "echo".to_string(),
                    description: "Echo the input back".to_string(),
                    parameters_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "message": { "type": "string", "description": "Message to echo" }
                        },
                        "required": ["message"]
                    }),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
            }
        }
    }

    #[async_trait]
    impl Skill for EchoSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }

        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            let msg = call
                .arguments
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(empty)");
            Ok(ToolResult::success(&call.id, msg))
        }
    }

    struct RestrictedSkill {
        descriptor: SkillDescriptor,
    }

    impl RestrictedSkill {
        fn new() -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: "restricted_tool".to_string(),
                    description: "A tool that requires FileRead capability".to_string(),
                    parameters_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                    required_capabilities: vec![Capability::FileRead {
                        allowed_paths: vec!["/tmp".to_string()],
                    }],
                    requires_approval: false,
                },
            }
        }
    }

    #[async_trait]
    impl Skill for RestrictedSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }

        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            Ok(ToolResult::success(&call.id, "restricted output"))
        }
    }

    // -----------------------------------------------------------------------
    // Helper: build a server with test skills
    // -----------------------------------------------------------------------

    fn make_test_server() -> McpServer {
        let registry = SkillRegistry::new();
        registry.register(Arc::new(EchoSkill::new()));
        registry.register(Arc::new(RestrictedSkill::new()));
        McpServer::new(
            "test-server",
            "0.1.0",
            Arc::new(registry),
            PermissionSet::new(),
        )
    }

    fn make_request(
        id: Option<u64>,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> IncomingRequest {
        IncomingRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_initialize_handshake() {
        let server = make_test_server();
        let req = make_request(
            Some(1),
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            })),
        );

        let resp = server.handle_request(req).await.unwrap();
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "test-server");
        assert_eq!(result["serverInfo"]["version"], "0.1.0");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn test_tools_list_returns_all_skills() {
        let server = make_test_server();
        let req = make_request(Some(2), "tools/list", None);

        let resp = server.handle_request(req).await.unwrap();
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"restricted_tool"));
    }

    #[tokio::test]
    async fn test_tool_call_executes_skill() {
        let server = make_test_server();
        let req = make_request(
            Some(3),
            "tools/call",
            Some(serde_json::json!({
                "name": "echo",
                "arguments": { "message": "hello world" }
            })),
        );

        let resp = server.handle_request(req).await.unwrap();
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "hello world");
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn test_tool_call_unknown_tool_returns_error() {
        let server = make_test_server();
        let req = make_request(
            Some(4),
            "tools/call",
            Some(serde_json::json!({
                "name": "nonexistent_tool",
                "arguments": {}
            })),
        );

        let resp = server.handle_request(req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, TOOL_NOT_FOUND);
        assert!(err.message.contains("nonexistent_tool"));
    }

    #[tokio::test]
    async fn test_ping_pong() {
        let server = make_test_server();
        let req = make_request(Some(5), "ping", None);

        let resp = server.handle_request(req).await.unwrap();
        assert!(resp.error.is_none());
        assert_eq!(resp.id, Some(5));
        // Pong is an empty result object
        assert_eq!(resp.result.unwrap(), serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_invalid_method_returns_error() {
        let server = make_test_server();
        let req = make_request(Some(6), "invalid/method", None);

        let resp = server.handle_request(req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("invalid/method"));
    }

    #[tokio::test]
    async fn test_permission_denied_on_tool_call() {
        // Server with empty permissions — restricted_tool requires FileRead
        let server = make_test_server();
        let req = make_request(
            Some(7),
            "tools/call",
            Some(serde_json::json!({
                "name": "restricted_tool",
                "arguments": {}
            })),
        );

        let resp = server.handle_request(req).await.unwrap();
        assert!(
            resp.error.is_none(),
            "Permission denied is returned as a tool result, not a JSON-RPC error"
        );

        // The registry returns a ToolResult with is_error=true for permission denied
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let content = result["content"].as_array().unwrap();
        assert!(content[0]["text"]
            .as_str()
            .unwrap()
            .contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_skill_descriptor_to_mcp_tool_conversion() {
        let descriptor = SkillDescriptor {
            name: "shell".to_string(),
            description: "Execute shell commands".to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to execute" }
                },
                "required": ["command"]
            }),
            required_capabilities: vec![Capability::ShellExec {
                allowed_commands: vec!["ls".to_string()],
            }],
            requires_approval: false,
        };

        let tool = McpServer::skill_to_mcp_tool(&descriptor);
        assert_eq!(tool.name, "shell");
        assert_eq!(tool.description, "Execute shell commands");
        assert_eq!(tool.input_schema["type"], "object");
        assert_eq!(tool.input_schema["properties"]["command"]["type"], "string");
        assert_eq!(tool.input_schema["required"][0], "command");
    }

    #[tokio::test]
    async fn test_notifications_initialized_returns_none() {
        let server = make_test_server();
        let req = make_request(None, "notifications/initialized", None);

        let resp = server.handle_request(req).await;
        assert!(resp.is_none(), "Notification should not produce a response");
    }

    #[tokio::test]
    async fn test_tools_call_missing_params() {
        let server = make_test_server();
        let req = make_request(Some(8), "tools/call", None);

        let resp = server.handle_request(req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_tools_call_missing_name_in_params() {
        let server = make_test_server();
        let req = make_request(
            Some(9),
            "tools/call",
            Some(serde_json::json!({ "arguments": {} })),
        );

        let resp = server.handle_request(req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("name"));
    }

    #[test]
    fn test_outgoing_response_serialization() {
        let resp = OutgoingResponse::success(42, serde_json::json!({"key": "value"}));
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["result"]["key"], "value");
        // Error should be absent (not null)
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn test_outgoing_error_serialization() {
        let resp = OutgoingResponse::error(Some(99), -32600, "Invalid request");
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 99);
        assert_eq!(parsed["error"]["code"], -32600);
        assert_eq!(parsed["error"]["message"], "Invalid request");
        // Result should be absent
        assert!(parsed.get("result").is_none());
    }

    #[test]
    fn test_incoming_request_deserialization() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#;
        let req: IncomingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(1));
        assert_eq!(req.method, "initialize");
        assert!(req.params.is_some());
    }

    #[test]
    fn test_incoming_notification_deserialization() {
        let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: IncomingRequest = serde_json::from_str(json).unwrap();
        assert!(req.id.is_none());
        assert_eq!(req.method, "notifications/initialized");
    }
}

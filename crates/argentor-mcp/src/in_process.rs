//! In-process MCP server — define MCP tools without spawning a subprocess.
//!
//! Tools run in the same process as the agent, avoiding IPC overhead.
//! Useful for custom tools that need access to application state.
//!
//! # Example
//!
//! ```rust,no_run
//! use argentor_mcp::in_process::InProcessMcpServer;
//! use argentor_skills::SkillRegistry;
//! use serde_json::json;
//!
//! let server = InProcessMcpServer::new("my-tools", "1.0.0")
//!     .add_tool(
//!         "get_weather",
//!         "Get current weather for a city",
//!         json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
//!         |args| {
//!             let city = args["city"].as_str().unwrap_or("Unknown");
//!             Ok(format!("Weather in {city}: Sunny, 22°C"))
//!         },
//!     )
//!     .add_async_tool(
//!         "fetch_data",
//!         "Fetch data from an API",
//!         json!({"type": "object", "properties": {"url": {"type": "string"}}}),
//!         |args| Box::pin(async move {
//!             let url = args["url"].as_str().unwrap_or("none");
//!             Ok(format!("Fetched data from {url}"))
//!         }),
//!     );
//!
//! // Register all tools as Argentor skills
//! let registry = SkillRegistry::new();
//! server.register_skills(&registry);
//! ```

use crate::protocol::McpToolDef;
use crate::server::{IncomingRequest, OutgoingResponse};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info, warn};

// JSON-RPC standard error codes (matching server.rs)
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const INVALID_REQUEST: i64 = -32600;

// Application-level error codes
const TOOL_NOT_FOUND: i64 = -32001;

// ---------------------------------------------------------------------------
// Tool handler types
// ---------------------------------------------------------------------------

/// A synchronous tool handler: receives a reference to JSON arguments and returns a string.
type SyncHandler = Box<dyn Fn(&serde_json::Value) -> ArgentorResult<String> + Send + Sync>;

/// An asynchronous tool handler: receives owned JSON arguments and returns a boxed future.
type AsyncHandler = Box<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = ArgentorResult<String>> + Send>>
        + Send
        + Sync,
>;

/// The type of handler backing an in-process tool.
enum ToolHandlerType {
    /// Synchronous handler — receives a reference to the arguments.
    Sync(SyncHandler),
    /// Asynchronous handler — receives owned arguments and returns a boxed future.
    Async(AsyncHandler),
}

/// A single tool registered in an [`InProcessMcpServer`].
struct InProcessTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    handler: ToolHandlerType,
}

// ---------------------------------------------------------------------------
// InProcessMcpServer
// ---------------------------------------------------------------------------

/// In-process MCP server that hosts tools without spawning a subprocess.
///
/// Each tool is a Rust closure (sync or async) that receives JSON arguments
/// and returns a string result. The server can:
///
/// - List tools in MCP format via [`list_tools()`](Self::list_tools)
/// - Execute a tool by name via [`call_tool()`](Self::call_tool)
/// - Handle raw JSON-RPC requests via [`handle_request()`](Self::handle_request)
/// - Register all tools as Argentor skills via [`register_skills()`](Self::register_skills)
pub struct InProcessMcpServer {
    name: String,
    version: String,
    tools: Vec<InProcessTool>,
}

impl InProcessMcpServer {
    /// Create a new in-process MCP server with the given name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            tools: Vec::new(),
        }
    }

    /// Add a synchronous tool.
    ///
    /// The handler receives a reference to the JSON arguments and must return
    /// a string result or an error.
    pub fn add_tool<F>(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        handler: F,
    ) -> Self
    where
        F: Fn(&serde_json::Value) -> ArgentorResult<String> + Send + Sync + 'static,
    {
        self.tools.push(InProcessTool {
            name: name.into(),
            description: description.into(),
            input_schema,
            handler: ToolHandlerType::Sync(Box::new(handler)),
        });
        self
    }

    /// Add an asynchronous tool.
    ///
    /// The handler receives owned JSON arguments and must return a future that
    /// resolves to a string result or an error.
    pub fn add_async_tool<F, Fut>(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        handler: F,
    ) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ArgentorResult<String>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        self.tools.push(InProcessTool {
            name: name.into(),
            description: description.into(),
            input_schema,
            handler: ToolHandlerType::Async(Box::new(move |args| {
                let h = Arc::clone(&handler);
                Box::pin(async move { h(args).await })
            })),
        });
        self
    }

    /// List all tool definitions in MCP format.
    pub fn list_tools(&self) -> Vec<McpToolDef> {
        self.tools
            .iter()
            .map(|t| McpToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect()
    }

    /// Execute a tool by name with the given arguments.
    ///
    /// Returns an error if the tool is not found or the handler fails.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> ArgentorResult<String> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| ArgentorError::Skill(format!("Unknown tool: {name}")))?;

        debug!(tool = %name, "Executing in-process MCP tool");

        match &tool.handler {
            ToolHandlerType::Sync(f) => f(arguments),
            ToolHandlerType::Async(f) => f(arguments.clone()).await,
        }
    }

    /// Handle a JSON-RPC request (tools/list, tools/call, initialize, ping).
    ///
    /// Returns `None` for notifications that require no response.
    pub async fn handle_request(&self, request: &IncomingRequest) -> Option<OutgoingResponse> {
        debug!(method = %request.method, id = ?request.id, "Handling in-process MCP request");

        match request.method.as_str() {
            "initialize" => Some(self.handle_initialize(request)),
            "notifications/initialized" => {
                info!("In-process MCP client acknowledged initialization");
                None
            }
            "ping" => Some(self.handle_ping(request)),
            "tools/list" => Some(self.handle_tools_list(request)),
            "tools/call" => Some(self.handle_tools_call(request).await),
            _ => {
                warn!(method = %request.method, "Unknown in-process MCP method");
                request.id.map(|id| {
                    OutgoingResponse::error(
                        Some(id),
                        METHOD_NOT_FOUND,
                        format!("Method not found: {}", request.method),
                    )
                })
            }
        }
    }

    /// Handle `initialize` — return server capabilities.
    fn handle_initialize(&self, request: &IncomingRequest) -> OutgoingResponse {
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

        info!(
            server = %self.name,
            version = %self.version,
            "Handling in-process MCP initialize"
        );

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
    fn handle_ping(&self, request: &IncomingRequest) -> OutgoingResponse {
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

        debug!("Handling in-process MCP ping");
        OutgoingResponse::success(id, serde_json::json!({}))
    }

    /// Handle `tools/list` — return all in-process tool definitions.
    fn handle_tools_list(&self, request: &IncomingRequest) -> OutgoingResponse {
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

        let tools = self.list_tools();
        OutgoingResponse::success(id, serde_json::json!({ "tools": tools }))
    }

    /// Handle `tools/call` — execute an in-process tool and return the result.
    async fn handle_tools_call(&self, request: &IncomingRequest) -> OutgoingResponse {
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

        let params = match &request.params {
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
            Some(name) => name,
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

        match self.call_tool(tool_name, &arguments).await {
            Ok(text) => {
                let content = serde_json::json!([{
                    "type": "text",
                    "text": text
                }]);

                OutgoingResponse::success(
                    id,
                    serde_json::json!({
                        "content": content,
                        "isError": false
                    }),
                )
            }
            Err(e) => {
                let error_msg = e.to_string();
                if error_msg.contains("Unknown tool") {
                    OutgoingResponse::error(
                        Some(id),
                        TOOL_NOT_FOUND,
                        format!("Tool not found: {tool_name}"),
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

    /// Convert all tools to Argentor Skills and register them in the given registry.
    ///
    /// Tool names follow the convention `mcp__{server_name}__{tool_name}`,
    /// matching the naming used by [`McpSkill`](crate::skill::McpSkill).
    pub fn register_skills(self, registry: &SkillRegistry) {
        // Collect tool metadata before moving self into Arc.
        let tool_meta: Vec<(String, String, serde_json::Value)> = self
            .tools
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    t.description.clone(),
                    t.input_schema.clone(),
                )
            })
            .collect();

        let server = Arc::new(self);

        for (tool_name, description, input_schema) in tool_meta {
            let prefixed_name = format!("mcp__{}_{}", server.name, tool_name)
                .replace(|c: char| !c.is_alphanumeric() && c != '_', "_");

            let skill = InProcessMcpSkill {
                descriptor: SkillDescriptor {
                    name: prefixed_name.clone(),
                    description: format!("[MCP:{}] {}", server.name, description),
                    parameters_schema: input_schema,
                    required_capabilities: vec![],
                },
                server: Arc::clone(&server),
                tool_name: tool_name.clone(),
            };

            info!(
                skill = %prefixed_name,
                server = %server.name,
                tool = %tool_name,
                "Registered in-process MCP tool as skill"
            );

            registry.register(Arc::new(skill));
        }
    }

    /// Get the number of tools registered in this server.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Get the server name and version.
    pub fn server_info(&self) -> (&str, &str) {
        (&self.name, &self.version)
    }
}

// ---------------------------------------------------------------------------
// InProcessMcpSkill — wraps an in-process tool as an Argentor Skill
// ---------------------------------------------------------------------------

/// Wrapper that presents an [`InProcessMcpServer`] tool as an Argentor [`Skill`].
///
/// Delegates execution to the server's [`call_tool()`](InProcessMcpServer::call_tool) method.
struct InProcessMcpSkill {
    descriptor: SkillDescriptor,
    server: Arc<InProcessMcpServer>,
    tool_name: String,
}

#[async_trait]
impl Skill for InProcessMcpSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        match self
            .server
            .call_tool(&self.tool_name, &call.arguments)
            .await
        {
            Ok(text) => Ok(ToolResult::success(&call.id, text)),
            Err(e) => Ok(ToolResult::error(&call.id, e.to_string())),
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
    use argentor_security::PermissionSet;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_test_server() -> InProcessMcpServer {
        InProcessMcpServer::new("test-server", "0.1.0")
            .add_tool(
                "greet",
                "Greet a person by name",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Person's name" }
                    },
                    "required": ["name"]
                }),
                |args| {
                    let name = args["name"].as_str().unwrap_or("World");
                    Ok(format!("Hello, {name}!"))
                },
            )
            .add_tool(
                "add",
                "Add two numbers",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }),
                |args| {
                    let a = args["a"].as_f64().unwrap_or(0.0);
                    let b = args["b"].as_f64().unwrap_or(0.0);
                    Ok(format!("{}", a + b))
                },
            )
    }

    fn make_async_server() -> InProcessMcpServer {
        InProcessMcpServer::new("async-server", "0.2.0").add_async_tool(
            "delayed_echo",
            "Echo after a short delay",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            }),
            |args| {
                Box::pin(async move {
                    let msg = args["message"].as_str().unwrap_or("(empty)");
                    Ok(format!("Async echo: {msg}"))
                })
            },
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
    // Construction and accessors
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_server_is_empty() {
        let server = InProcessMcpServer::new("empty", "1.0.0");
        assert_eq!(server.tool_count(), 0);
        assert!(server.list_tools().is_empty());
    }

    #[test]
    fn test_server_info() {
        let server = InProcessMcpServer::new("my-server", "2.3.4");
        let (name, version) = server.server_info();
        assert_eq!(name, "my-server");
        assert_eq!(version, "2.3.4");
    }

    #[test]
    fn test_tool_count_after_adding_tools() {
        let server = make_test_server();
        assert_eq!(server.tool_count(), 2);
    }

    #[test]
    fn test_add_tool_returns_self_for_chaining() {
        let server = InProcessMcpServer::new("chain", "1.0.0")
            .add_tool("a", "Tool A", serde_json::json!({}), |_| Ok("a".into()))
            .add_tool("b", "Tool B", serde_json::json!({}), |_| Ok("b".into()))
            .add_tool("c", "Tool C", serde_json::json!({}), |_| Ok("c".into()));
        assert_eq!(server.tool_count(), 3);
    }

    // -----------------------------------------------------------------------
    // list_tools
    // -----------------------------------------------------------------------

    #[test]
    fn test_list_tools_returns_mcp_format() {
        let server = make_test_server();
        let tools = server.list_tools();

        assert_eq!(tools.len(), 2);

        let greet = tools.iter().find(|t| t.name == "greet").unwrap();
        assert_eq!(greet.description, "Greet a person by name");
        assert_eq!(greet.input_schema["type"], "object");
        assert!(greet.input_schema["properties"]["name"].is_object());

        let add = tools.iter().find(|t| t.name == "add").unwrap();
        assert_eq!(add.description, "Add two numbers");
        assert_eq!(add.input_schema["required"][0], "a");
        assert_eq!(add.input_schema["required"][1], "b");
    }

    #[test]
    fn test_list_tools_empty_server() {
        let server = InProcessMcpServer::new("empty", "1.0.0");
        assert!(server.list_tools().is_empty());
    }

    // -----------------------------------------------------------------------
    // call_tool (sync)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_call_sync_tool_greet() {
        let server = make_test_server();
        let result = server
            .call_tool("greet", &serde_json::json!({"name": "Alice"}))
            .await
            .unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[tokio::test]
    async fn test_call_sync_tool_add() {
        let server = make_test_server();
        let result = server
            .call_tool("add", &serde_json::json!({"a": 3, "b": 7}))
            .await
            .unwrap();
        assert_eq!(result, "10");
    }

    #[tokio::test]
    async fn test_call_unknown_tool_returns_error() {
        let server = make_test_server();
        let err = server
            .call_tool("nonexistent", &serde_json::json!({}))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown tool"));
        assert!(msg.contains("nonexistent"));
    }

    #[tokio::test]
    async fn test_call_tool_with_default_args() {
        let server = make_test_server();
        // "name" not provided — handler defaults to "World"
        let result = server
            .call_tool("greet", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result, "Hello, World!");
    }

    // -----------------------------------------------------------------------
    // call_tool (async)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_call_async_tool() {
        let server = make_async_server();
        let result = server
            .call_tool("delayed_echo", &serde_json::json!({"message": "ping"}))
            .await
            .unwrap();
        assert_eq!(result, "Async echo: ping");
    }

    #[tokio::test]
    async fn test_call_async_tool_default_message() {
        let server = make_async_server();
        let result = server
            .call_tool("delayed_echo", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result, "Async echo: (empty)");
    }

    // -----------------------------------------------------------------------
    // Mixed sync + async on same server
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mixed_sync_and_async_tools() {
        let server = InProcessMcpServer::new("mixed", "1.0.0")
            .add_tool("sync_tool", "A sync tool", serde_json::json!({}), |_| {
                Ok("sync result".to_string())
            })
            .add_async_tool("async_tool", "An async tool", serde_json::json!({}), |_| {
                Box::pin(async { Ok("async result".to_string()) })
            });

        assert_eq!(server.tool_count(), 2);

        let sync_result = server
            .call_tool("sync_tool", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(sync_result, "sync result");

        let async_result = server
            .call_tool("async_tool", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(async_result, "async result");
    }

    // -----------------------------------------------------------------------
    // Error propagation from handler
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_sync_handler_error_propagates() {
        let server = InProcessMcpServer::new("err", "1.0.0").add_tool(
            "fail",
            "Always fails",
            serde_json::json!({}),
            |_| Err(ArgentorError::Skill("handler exploded".to_string())),
        );

        let err = server
            .call_tool("fail", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("handler exploded"));
    }

    #[tokio::test]
    async fn test_async_handler_error_propagates() {
        let server = InProcessMcpServer::new("err", "1.0.0").add_async_tool(
            "async_fail",
            "Always fails async",
            serde_json::json!({}),
            |_| Box::pin(async { Err(ArgentorError::Skill("async handler exploded".to_string())) }),
        );

        let err = server
            .call_tool("async_fail", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("async handler exploded"));
    }

    // -----------------------------------------------------------------------
    // handle_request: JSON-RPC dispatch
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_request_initialize() {
        let server = make_test_server();
        let req = make_request(
            Some(1),
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            })),
        );

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "test-server");
        assert_eq!(result["serverInfo"]["version"], "0.1.0");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn test_handle_request_ping() {
        let server = make_test_server();
        let req = make_request(Some(42), "ping", None);

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_none());
        assert_eq!(resp.id, Some(42));
        assert_eq!(resp.result.unwrap(), serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_handle_request_tools_list() {
        let server = make_test_server();
        let req = make_request(Some(2), "tools/list", None);

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"add"));
    }

    #[tokio::test]
    async fn test_handle_request_tools_call() {
        let server = make_test_server();
        let req = make_request(
            Some(3),
            "tools/call",
            Some(serde_json::json!({
                "name": "greet",
                "arguments": { "name": "Bob" }
            })),
        );

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello, Bob!");
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn test_handle_request_tools_call_unknown_tool() {
        let server = make_test_server();
        let req = make_request(
            Some(4),
            "tools/call",
            Some(serde_json::json!({
                "name": "nonexistent",
                "arguments": {}
            })),
        );

        let resp = server.handle_request(&req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, TOOL_NOT_FOUND);
        assert!(err.message.contains("nonexistent"));
    }

    #[tokio::test]
    async fn test_handle_request_tools_call_missing_params() {
        let server = make_test_server();
        let req = make_request(Some(5), "tools/call", None);

        let resp = server.handle_request(&req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_request_tools_call_missing_name() {
        let server = make_test_server();
        let req = make_request(
            Some(6),
            "tools/call",
            Some(serde_json::json!({ "arguments": {} })),
        );

        let resp = server.handle_request(&req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("name"));
    }

    #[tokio::test]
    async fn test_handle_request_unknown_method() {
        let server = make_test_server();
        let req = make_request(Some(7), "resources/list", None);

        let resp = server.handle_request(&req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("resources/list"));
    }

    #[tokio::test]
    async fn test_handle_request_notification_returns_none() {
        let server = make_test_server();
        let req = make_request(None, "notifications/initialized", None);

        let resp = server.handle_request(&req).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn test_handle_request_tools_call_handler_error() {
        let server = InProcessMcpServer::new("err", "1.0.0").add_tool(
            "boom",
            "Always explodes",
            serde_json::json!({}),
            |_| Err(ArgentorError::Skill("boom!".to_string())),
        );

        let req = make_request(
            Some(8),
            "tools/call",
            Some(serde_json::json!({
                "name": "boom",
                "arguments": {}
            })),
        );

        let resp = server.handle_request(&req).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, INTERNAL_ERROR);
        assert!(err.message.contains("boom!"));
    }

    // -----------------------------------------------------------------------
    // register_skills: integration with SkillRegistry
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_skills_adds_to_registry() {
        let server = make_test_server();
        let registry = SkillRegistry::new();
        assert_eq!(registry.skill_count(), 0);

        server.register_skills(&registry);
        assert_eq!(registry.skill_count(), 2);
    }

    #[test]
    fn test_registered_skill_names_follow_convention() {
        let server = make_test_server();
        let registry = SkillRegistry::new();
        server.register_skills(&registry);

        // Names should be mcp__{server_name}_{tool_name}
        assert!(registry.get("mcp__test_server_greet").is_some());
        assert!(registry.get("mcp__test_server_add").is_some());
    }

    #[test]
    fn test_registered_skill_descriptors() {
        let server = make_test_server();
        let registry = SkillRegistry::new();
        server.register_skills(&registry);

        let skill = registry.get("mcp__test_server_greet").unwrap();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "mcp__test_server_greet");
        assert!(desc.description.contains("[MCP:test-server]"));
        assert!(desc.description.contains("Greet a person by name"));
        assert_eq!(desc.parameters_schema["type"], "object");
        assert!(desc.required_capabilities.is_empty());
    }

    #[tokio::test]
    async fn test_execute_registered_skill_via_registry() {
        let server = make_test_server();
        let registry = SkillRegistry::new();
        server.register_skills(&registry);

        let perms = PermissionSet::new();
        let call = ToolCall {
            id: "call_1".to_string(),
            name: "mcp__test_server_greet".to_string(),
            arguments: serde_json::json!({"name": "Carlos"}),
        };

        let result = registry.execute(call, &perms).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "Hello, Carlos!");
    }

    #[tokio::test]
    async fn test_execute_registered_async_skill_via_registry() {
        let server = make_async_server();
        let registry = SkillRegistry::new();
        server.register_skills(&registry);

        let perms = PermissionSet::new();
        let call = ToolCall {
            id: "call_2".to_string(),
            name: "mcp__async_server_delayed_echo".to_string(),
            arguments: serde_json::json!({"message": "hello async"}),
        };

        let result = registry.execute(call, &perms).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "Async echo: hello async");
    }

    #[tokio::test]
    async fn test_registered_skill_handler_error_returns_error_tool_result() {
        let server = InProcessMcpServer::new("err-server", "1.0.0").add_tool(
            "fail",
            "Always fails",
            serde_json::json!({}),
            |_| Err(ArgentorError::Skill("intentional failure".to_string())),
        );

        let registry = SkillRegistry::new();
        server.register_skills(&registry);

        let perms = PermissionSet::new();
        let call = ToolCall {
            id: "call_3".to_string(),
            name: "mcp__err_server_fail".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = registry.execute(call, &perms).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("intentional failure"));
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tools_call_without_arguments_uses_empty_object() {
        let server = InProcessMcpServer::new("test", "1.0.0").add_tool(
            "no_args",
            "Takes no arguments",
            serde_json::json!({"type": "object", "properties": {}}),
            |args| {
                // Verify we got an empty object, not null
                assert!(args.is_object());
                Ok("ok".to_string())
            },
        );

        let req = make_request(
            Some(1),
            "tools/call",
            Some(serde_json::json!({ "name": "no_args" })),
        );

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "ok");
    }

    #[test]
    fn test_special_characters_in_server_name_sanitized() {
        let server = InProcessMcpServer::new("my-special.server", "1.0.0").add_tool(
            "tool-with-dashes",
            "A tool",
            serde_json::json!({}),
            |_| Ok("ok".into()),
        );

        let registry = SkillRegistry::new();
        server.register_skills(&registry);

        // Dashes and dots should be replaced with underscores
        assert!(registry
            .get("mcp__my_special_server_tool_with_dashes")
            .is_some());
    }

    #[tokio::test]
    async fn test_handle_request_initialize_without_id_returns_error() {
        let server = make_test_server();
        let req = make_request(None, "initialize", None);

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, INVALID_REQUEST);
    }

    #[tokio::test]
    async fn test_handle_request_tools_list_without_id_returns_error() {
        let server = make_test_server();
        let req = make_request(None, "tools/list", None);

        let resp = server.handle_request(&req).await.unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, INVALID_REQUEST);
    }

    #[tokio::test]
    async fn test_unknown_notification_returns_none() {
        let server = make_test_server();
        let req = make_request(None, "unknown/notification", None);

        let resp = server.handle_request(&req).await;
        assert!(resp.is_none());
    }
}

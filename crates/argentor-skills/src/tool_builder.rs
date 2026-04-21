//! Fluent builder for defining skills without boilerplate.
//!
//! Inspired by the Claude Agent SDK's `@tool` decorator, `ToolBuilder` lets you
//! create a fully-functional [`Skill`] in a few chained calls instead of manually
//! implementing the trait and writing JSON Schema by hand.
//!
//! # Example
//!
//! ```rust
//! use argentor_skills::tool_builder::ToolBuilder;
//!
//! let greeting = ToolBuilder::new("greet")
//!     .description("Greet a user by name")
//!     .param("name", "string", "The user's name", true)
//!     .param("greeting", "string", "Custom greeting", false)
//!     .handler(|args| {
//!         let name = args["name"].as_str().unwrap_or("World");
//!         let greeting = args["greeting"].as_str().unwrap_or("Hello");
//!         Ok(format!("{greeting}, {name}!"))
//!     })
//!     .build();
//! ```

use crate::skill::{Skill, SkillDescriptor};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use async_trait::async_trait;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Definition of a single parameter for the built tool.
#[derive(Debug, Clone)]
struct ParamDef {
    /// Parameter name (used as JSON key).
    name: String,
    /// JSON Schema type: `"string"`, `"number"`, `"boolean"`, `"object"`, `"array"`.
    type_name: String,
    /// Human-readable description shown to the LLM.
    description: String,
    /// Whether the parameter is required.
    required: bool,
}

/// Type alias for a synchronous tool handler function.
type SyncHandlerFn = dyn Fn(&serde_json::Value) -> ArgentorResult<String> + Send + Sync;

/// Type alias for an asynchronous tool handler function.
type AsyncHandlerFn = dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = ArgentorResult<String>> + Send>>
    + Send
    + Sync;

/// The runtime handler stored inside a [`BuiltTool`].
enum ToolHandler {
    /// Synchronous handler — called with a reference to the arguments.
    Sync(Box<SyncHandlerFn>),
    /// Asynchronous handler — called with owned arguments, returns a pinned future.
    Async(Box<AsyncHandlerFn>),
}

/// Fluent builder for creating [`Skill`] implementations without boilerplate.
///
/// Chain `.param()`, `.description()`, `.capability()`, and either `.handler()` or
/// `.async_handler()`, then call `.build()` to obtain an `Arc<dyn Skill>`.
pub struct ToolBuilder {
    name: String,
    description: String,
    params: Vec<ParamDef>,
    capabilities: Vec<Capability>,
    handler: Option<Box<SyncHandlerFn>>,
    async_handler: Option<Box<AsyncHandlerFn>>,
}

impl ToolBuilder {
    /// Start building a new tool with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            params: Vec::new(),
            capabilities: Vec::new(),
            handler: None,
            async_handler: None,
        }
    }

    /// Set the human-readable description shown to the LLM.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Add a parameter definition.
    ///
    /// `type_name` should be one of: `"string"`, `"number"`, `"boolean"`, `"object"`, `"array"`.
    pub fn param(mut self, name: &str, type_name: &str, description: &str, required: bool) -> Self {
        self.params.push(ParamDef {
            name: name.to_string(),
            type_name: type_name.to_string(),
            description: description.to_string(),
            required,
        });
        self
    }

    /// Declare a capability this tool requires (e.g., `Capability::NetworkAccess`).
    pub fn capability(mut self, cap: Capability) -> Self {
        self.capabilities.push(cap);
        self
    }

    /// Set a **synchronous** handler that receives the arguments JSON.
    ///
    /// The handler returns `Ok(String)` on success or an `ArgentorError` on failure.
    pub fn handler<F>(mut self, f: F) -> Self
    where
        F: Fn(&serde_json::Value) -> ArgentorResult<String> + Send + Sync + 'static,
    {
        self.handler = Some(Box::new(f));
        self
    }

    /// Set an **asynchronous** handler that receives owned arguments JSON.
    ///
    /// The handler returns a `Future<Output = ArgentorResult<String>>`.
    pub fn async_handler<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ArgentorResult<String>> + Send + 'static,
    {
        self.async_handler = Some(Box::new(move |args| Box::pin(f(args))));
        self
    }

    /// Build the tool into an `Arc<dyn Skill>`.
    ///
    /// # Panics
    ///
    /// Returns an error wrapped in the `Arc` if neither `handler` nor `async_handler`
    /// was set — but to keep the API ergonomic the actual check happens here so
    /// callers get a clear message.
    pub fn build(self) -> Arc<dyn Skill> {
        let handler = if let Some(sync_h) = self.handler {
            ToolHandler::Sync(sync_h)
        } else if let Some(async_h) = self.async_handler {
            ToolHandler::Async(async_h)
        } else {
            // Return a tool that always errors — this lets callers detect the
            // mistake at runtime without panicking during build.
            return Arc::new(ErrorTool {
                descriptor: SkillDescriptor {
                    name: self.name.clone(),
                    description: self.description,
                    parameters_schema: json!({}),
                    required_capabilities: self.capabilities,
                },
                message: format!(
                    "Tool '{}' was built without a handler — call .handler() or .async_handler() before .build()",
                    self.name
                ),
            });
        };

        // Build JSON Schema from params
        let mut properties = serde_json::Map::new();
        let mut required: Vec<String> = Vec::new();

        for p in &self.params {
            let mut prop = serde_json::Map::new();
            prop.insert("type".to_string(), json!(p.type_name));
            prop.insert("description".to_string(), json!(p.description));
            properties.insert(p.name.clone(), serde_json::Value::Object(prop));
            if p.required {
                required.push(p.name.clone());
            }
        }

        let parameters_schema = json!({
            "type": "object",
            "properties": properties,
            "required": required,
        });

        Arc::new(BuiltTool {
            descriptor: SkillDescriptor {
                name: self.name,
                description: self.description,
                parameters_schema,
                required_capabilities: self.capabilities,
            },
            handler,
        })
    }

    /// Try to build, returning `Err` if no handler was set.
    ///
    /// This is the fallible alternative to [`build()`](Self::build) for callers
    /// that prefer explicit error handling over the always-errors sentinel.
    pub fn try_build(self) -> ArgentorResult<Arc<dyn Skill>> {
        let name = self.name.clone();
        let has_handler = self.handler.is_some() || self.async_handler.is_some();
        if !has_handler {
            return Err(ArgentorError::Skill(format!(
                "Tool '{name}' has no handler — call .handler() or .async_handler() before .build()"
            )));
        }
        Ok(self.build())
    }
}

// ---------------------------------------------------------------------------
// BuiltTool — the Skill impl produced by ToolBuilder
// ---------------------------------------------------------------------------

/// A fully-configured skill built by [`ToolBuilder`].
struct BuiltTool {
    descriptor: SkillDescriptor,
    handler: ToolHandler,
}

#[async_trait]
impl Skill for BuiltTool {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let result = match &self.handler {
            ToolHandler::Sync(f) => f(&call.arguments)?,
            ToolHandler::Async(f) => f(call.arguments.clone()).await?,
        };

        Ok(ToolResult::success(call.id, result))
    }
}

// ---------------------------------------------------------------------------
// ErrorTool — sentinel for missing handler
// ---------------------------------------------------------------------------

/// Sentinel skill returned when `ToolBuilder::build()` is called without a handler.
struct ErrorTool {
    descriptor: SkillDescriptor,
    message: String,
}

#[async_trait]
impl Skill for ErrorTool {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        Ok(ToolResult::error(call.id, &self.message))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use argentor_core::ToolCall;
    use serde_json::json;

    fn make_call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test-1".to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    // ---- Basic construction ------------------------------------------------

    #[test]
    fn test_basic_build_name_and_description() {
        let tool = ToolBuilder::new("echo")
            .description("Echo the input")
            .handler(|args| Ok(args.to_string()))
            .build();

        assert_eq!(tool.descriptor().name, "echo");
        assert_eq!(tool.descriptor().description, "Echo the input");
    }

    #[test]
    fn test_params_generate_json_schema() {
        let tool = ToolBuilder::new("greet")
            .description("Greet someone")
            .param("name", "string", "The name", true)
            .param("age", "number", "Age", false)
            .handler(|_| Ok("hi".to_string()))
            .build();

        let schema = &tool.descriptor().parameters_schema;
        assert_eq!(schema["type"], "object");

        let props = &schema["properties"];
        assert_eq!(props["name"]["type"], "string");
        assert_eq!(props["name"]["description"], "The name");
        assert_eq!(props["age"]["type"], "number");

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "name");
    }

    #[test]
    fn test_multiple_required_params() {
        let tool = ToolBuilder::new("t")
            .param("a", "string", "A", true)
            .param("b", "number", "B", true)
            .param("c", "boolean", "C", false)
            .handler(|_| Ok("ok".to_string()))
            .build();

        let required = tool.descriptor().parameters_schema["required"]
            .as_array()
            .unwrap();
        assert_eq!(required.len(), 2);
        assert!(required.contains(&json!("a")));
        assert!(required.contains(&json!("b")));
    }

    #[test]
    fn test_no_params_empty_schema() {
        let tool = ToolBuilder::new("noop")
            .handler(|_| Ok("done".to_string()))
            .build();

        let schema = &tool.descriptor().parameters_schema;
        assert_eq!(schema["properties"], json!({}));
        let required = schema["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn test_capabilities_stored() {
        let tool = ToolBuilder::new("net")
            .capability(Capability::NetworkAccess {
                allowed_hosts: vec!["*".to_string()],
            })
            .capability(Capability::FileRead {
                allowed_paths: vec!["/tmp".to_string()],
            })
            .handler(|_| Ok("ok".to_string()))
            .build();

        assert_eq!(tool.descriptor().required_capabilities.len(), 2);
    }

    // ---- Sync handler ------------------------------------------------------

    #[tokio::test]
    async fn test_sync_handler_executes() {
        let tool = ToolBuilder::new("greet")
            .param("name", "string", "Name", true)
            .handler(|args| {
                let name = args["name"].as_str().unwrap_or("World");
                Ok(format!("Hello, {name}!"))
            })
            .build();

        let call = make_call("greet", json!({"name": "Alice"}));
        let result = tool.execute(call).await.unwrap();
        assert_eq!(result.content, "Hello, Alice!");
        assert!(!result.is_error);
        assert_eq!(result.call_id, "test-1");
    }

    #[tokio::test]
    async fn test_sync_handler_default_values() {
        let tool = ToolBuilder::new("greet")
            .handler(|args| {
                let name = args["name"].as_str().unwrap_or("World");
                Ok(format!("Hi, {name}!"))
            })
            .build();

        let call = make_call("greet", json!({}));
        let result = tool.execute(call).await.unwrap();
        assert_eq!(result.content, "Hi, World!");
    }

    #[tokio::test]
    async fn test_sync_handler_error() {
        let tool = ToolBuilder::new("fail")
            .handler(|_| Err(ArgentorError::Skill("boom".to_string())))
            .build();

        let call = make_call("fail", json!({}));
        let err = tool.execute(call).await.unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    // ---- Async handler -----------------------------------------------------

    #[tokio::test]
    async fn test_async_handler_executes() {
        let tool = ToolBuilder::new("async_greet")
            .param("name", "string", "Name", true)
            .async_handler(|args| async move {
                let name = args["name"].as_str().unwrap_or("World").to_string();
                Ok(format!("Async hello, {name}!"))
            })
            .build();

        let call = make_call("async_greet", json!({"name": "Bob"}));
        let result = tool.execute(call).await.unwrap();
        assert_eq!(result.content, "Async hello, Bob!");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_async_handler_error() {
        let tool = ToolBuilder::new("async_fail")
            .async_handler(|_| async move { Err(ArgentorError::Skill("async boom".to_string())) })
            .build();

        let call = make_call("async_fail", json!({}));
        let err = tool.execute(call).await.unwrap_err();
        assert!(err.to_string().contains("async boom"));
    }

    // ---- Missing handler ---------------------------------------------------

    #[tokio::test]
    async fn test_missing_handler_build_returns_error_tool() {
        let tool = ToolBuilder::new("no_handler").description("Oops").build();

        let call = make_call("no_handler", json!({}));
        let result = tool.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("without a handler"));
    }

    #[test]
    fn test_try_build_missing_handler_returns_err() {
        let result = ToolBuilder::new("no_handler").try_build();
        assert!(result.is_err());
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(msg.contains("no handler"));
    }

    #[test]
    fn test_try_build_with_handler_returns_ok() {
        let result = ToolBuilder::new("ok")
            .handler(|_| Ok("fine".to_string()))
            .try_build();
        assert!(result.is_ok());
    }

    // ---- All param types ---------------------------------------------------

    #[tokio::test]
    async fn test_all_param_types_in_schema() {
        let tool = ToolBuilder::new("multi")
            .param("s", "string", "A string", false)
            .param("n", "number", "A number", false)
            .param("b", "boolean", "A bool", false)
            .param("o", "object", "An object", false)
            .param("a", "array", "An array", false)
            .handler(|_| Ok("ok".to_string()))
            .build();

        let props = &tool.descriptor().parameters_schema["properties"];
        assert_eq!(props["s"]["type"], "string");
        assert_eq!(props["n"]["type"], "number");
        assert_eq!(props["b"]["type"], "boolean");
        assert_eq!(props["o"]["type"], "object");
        assert_eq!(props["a"]["type"], "array");
    }

    // ---- Complex handler logic ---------------------------------------------

    #[tokio::test]
    async fn test_handler_with_computation() {
        let tool = ToolBuilder::new("add")
            .param("a", "number", "First number", true)
            .param("b", "number", "Second number", true)
            .handler(|args| {
                let a = args["a"].as_f64().unwrap_or(0.0);
                let b = args["b"].as_f64().unwrap_or(0.0);
                Ok(format!("{}", a + b))
            })
            .build();

        let call = make_call("add", json!({"a": 10.5, "b": 20.5}));
        let result = tool.execute(call).await.unwrap();
        assert_eq!(result.content, "31");
    }

    #[tokio::test]
    async fn test_handler_preserves_call_id() {
        let tool = ToolBuilder::new("id_check")
            .handler(|_| Ok("ok".to_string()))
            .build();

        let call = ToolCall {
            id: "unique-42".to_string(),
            name: "id_check".to_string(),
            arguments: json!({}),
        };
        let result = tool.execute(call).await.unwrap();
        assert_eq!(result.call_id, "unique-42");
    }
}

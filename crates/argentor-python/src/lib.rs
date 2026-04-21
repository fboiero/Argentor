//! Argentor Python bindings -- expose core agent framework to Python via PyO3.
//!
//! Install: `pip install maturin && cd crates/argentor-python && maturin develop`
//! Usage:   `import argentor`

#![allow(clippy::unwrap_used, clippy::expect_used)]
// PyO3 conversions and lock access require unwrap/expect in many places.

use pyo3::prelude::*;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Sub-modules: dynamic Python tool loading + LangChain shim
// ---------------------------------------------------------------------------

pub mod dynamic_load;
pub mod langchain_compat;

pub use dynamic_load::{
    discover_python_tools, load_langchain_tool, PythonToolConfig, PythonToolSkill,
};
pub use langchain_compat::{LangChainAdapter, LangChainCategory};

// ---------------------------------------------------------------------------
// PyMessage
// ---------------------------------------------------------------------------

/// A single message in a conversation session.
#[pyclass(name = "Message")]
#[derive(Clone)]
pub struct PyMessage {
    /// Role of the message author (`"user"`, `"assistant"`, `"system"`, `"tool"`).
    #[pyo3(get)]
    role: String,
    /// Textual content of the message.
    #[pyo3(get)]
    content: String,
    /// ISO-8601 UTC timestamp.
    #[pyo3(get)]
    timestamp: String,
}

#[pymethods]
impl PyMessage {
    fn __repr__(&self) -> String {
        format!("Message(role='{}', content='{}...')", self.role, truncate(&self.content, 40))
    }
}

impl From<&argentor_core::Message> for PyMessage {
    fn from(m: &argentor_core::Message) -> Self {
        let role = match m.role {
            argentor_core::Role::User => "user",
            argentor_core::Role::Assistant => "assistant",
            argentor_core::Role::System => "system",
            argentor_core::Role::Tool => "tool",
        };
        Self {
            role: role.to_string(),
            content: m.content.clone(),
            timestamp: m.timestamp.to_rfc3339(),
        }
    }
}

// ---------------------------------------------------------------------------
// PyToolResult
// ---------------------------------------------------------------------------

/// Result returned by a skill execution.
#[pyclass(name = "ToolResult")]
#[derive(Clone)]
pub struct PyToolResult {
    /// ID of the tool call this result corresponds to.
    #[pyo3(get)]
    call_id: String,
    /// Textual output from the tool.
    #[pyo3(get)]
    content: String,
    /// Whether the execution ended in an error.
    #[pyo3(get)]
    is_error: bool,
}

#[pymethods]
impl PyToolResult {
    fn __repr__(&self) -> String {
        format!(
            "ToolResult(call_id='{}', is_error={}, content='{}...')",
            self.call_id,
            self.is_error,
            truncate(&self.content, 60)
        )
    }
}

impl From<argentor_core::ToolResult> for PyToolResult {
    fn from(r: argentor_core::ToolResult) -> Self {
        Self {
            call_id: r.call_id,
            content: r.content,
            is_error: r.is_error,
        }
    }
}

// ---------------------------------------------------------------------------
// PySession
// ---------------------------------------------------------------------------

/// A conversation session that groups messages and tracks metadata.
#[pyclass(name = "Session")]
pub struct PySession {
    inner: argentor_session::Session,
}

#[pymethods]
impl PySession {
    /// Create a new session with a fresh UUID and empty message history.
    #[new]
    fn new() -> Self {
        Self {
            inner: argentor_session::Session::new(),
        }
    }

    /// Unique session ID (UUID v4).
    #[getter]
    fn id(&self) -> String {
        self.inner.id.to_string()
    }

    /// Number of messages in the session.
    fn message_count(&self) -> usize {
        self.inner.message_count()
    }

    /// Return all messages in the session.
    fn messages(&self) -> Vec<PyMessage> {
        self.inner.messages.iter().map(PyMessage::from).collect()
    }

    /// Add a user message to the session.
    fn add_user_message(&mut self, content: &str) {
        let msg = argentor_core::Message::user(content, self.inner.id);
        self.inner.add_message(msg);
    }

    /// Add an assistant message to the session.
    fn add_assistant_message(&mut self, content: &str) {
        let msg = argentor_core::Message::assistant(content, self.inner.id);
        self.inner.add_message(msg);
    }

    /// Add a system message to the session.
    fn add_system_message(&mut self, content: &str) {
        let msg = argentor_core::Message::system(content, self.inner.id);
        self.inner.add_message(msg);
    }

    /// ISO-8601 timestamp of when the session was created.
    #[getter]
    fn created_at(&self) -> String {
        self.inner.created_at.to_rfc3339()
    }

    /// ISO-8601 timestamp of the last modification.
    #[getter]
    fn updated_at(&self) -> String {
        self.inner.updated_at.to_rfc3339()
    }

    fn __repr__(&self) -> String {
        format!(
            "Session(id='{}', messages={})",
            self.inner.id,
            self.inner.message_count()
        )
    }
}

// ---------------------------------------------------------------------------
// PySkillRegistry
// ---------------------------------------------------------------------------

/// Central registry for discovering and invoking Argentor skills from Python.
///
/// Creates a registry pre-loaded with all built-in utility skills (calculator,
/// hash, json_query, text_transform, etc.). Skills that require external
/// resources (shell, file I/O, browser) are also registered but subject to
/// the internal permission system.
#[pyclass(name = "SkillRegistry")]
pub struct PySkillRegistry {
    inner: Arc<argentor_skills::SkillRegistry>,
    permissions: argentor_security::PermissionSet,
    rt: tokio::runtime::Runtime,
}

#[pymethods]
impl PySkillRegistry {
    /// Create a new registry with all built-in skills registered.
    #[new]
    fn new() -> PyResult<Self> {
        let registry = argentor_skills::SkillRegistry::new();
        argentor_builtins::register_builtins(&registry);

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to create tokio runtime: {e}")))?;

        // Build a permissive permission set for Python usage.
        // In production, callers should configure this more restrictively.
        let mut permissions = argentor_security::PermissionSet::new();
        permissions.grant(argentor_security::Capability::FileRead {
            allowed_paths: vec!["/".to_string()],
        });
        permissions.grant(argentor_security::Capability::FileWrite {
            allowed_paths: vec!["/".to_string()],
        });
        permissions.grant(argentor_security::Capability::NetworkAccess {
            allowed_hosts: vec!["*".to_string()],
        });
        permissions.grant(argentor_security::Capability::ShellExec {
            allowed_commands: vec!["*".to_string()],
        });
        permissions.grant(argentor_security::Capability::EnvRead {
            allowed_vars: vec!["*".to_string()],
        });
        permissions.grant(argentor_security::Capability::DatabaseQuery);
        permissions.grant(argentor_security::Capability::BrowserAccess {
            allowed_domains: vec!["*".to_string()],
        });

        Ok(Self {
            inner: Arc::new(registry),
            permissions,
            rt,
        })
    }

    /// List names of all registered skills.
    fn list_skills(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .inner
            .list_descriptors()
            .iter()
            .map(|d| d.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Number of registered skills.
    fn skill_count(&self) -> usize {
        self.inner.skill_count()
    }

    /// Execute a skill by name with JSON-encoded arguments.
    ///
    /// Returns a `ToolResult` with the output or error.
    ///
    /// Example:
    /// ```python
    /// result = registry.execute("calculator", '{"operation": "add", "a": 2, "b": 3}')
    /// print(result.content)  # "5"
    /// ```
    fn execute(&self, name: &str, arguments_json: &str) -> PyResult<PyToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments_json).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid JSON arguments: {e}"))
        })?;

        let call = argentor_core::ToolCall {
            id: format!("py_{}", uuid_v4_string()),
            name: name.to_string(),
            arguments: args,
        };

        let permissions = self.permissions.clone();
        let registry = Arc::clone(&self.inner);

        let result = self
            .rt
            .block_on(async move { registry.execute(call, &permissions).await })
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Skill execution failed: {e}")))?;

        Ok(PyToolResult::from(result))
    }

    fn __repr__(&self) -> String {
        format!("SkillRegistry(skills={})", self.inner.skill_count())
    }
}

// ---------------------------------------------------------------------------
// PyGuardrailResult
// ---------------------------------------------------------------------------

/// Result of running the guardrail pipeline on a text input.
#[pyclass(name = "GuardrailResult")]
#[derive(Clone)]
pub struct PyGuardrailResult {
    /// `True` when no block-severity violations were found.
    #[pyo3(get)]
    passed: bool,
    /// List of violation descriptions.
    #[pyo3(get)]
    violations: Vec<String>,
    /// Sanitized text with PII redacted (if applicable).
    #[pyo3(get)]
    sanitized_text: Option<String>,
    /// Processing time in milliseconds.
    #[pyo3(get)]
    processing_time_ms: u64,
}

#[pymethods]
impl PyGuardrailResult {
    fn __repr__(&self) -> String {
        format!(
            "GuardrailResult(passed={}, violations={}, time_ms={})",
            self.passed,
            self.violations.len(),
            self.processing_time_ms
        )
    }
}

// ---------------------------------------------------------------------------
// PyGuardrailEngine
// ---------------------------------------------------------------------------

/// Production-grade guardrail engine for filtering, validating, and sanitizing
/// LLM inputs and outputs. Pre-loaded with PII detection, prompt-injection
/// prevention, toxicity filtering, and length limits.
#[pyclass(name = "GuardrailEngine")]
pub struct PyGuardrailEngine {
    inner: argentor_agent::guardrails::GuardrailEngine,
}

#[pymethods]
impl PyGuardrailEngine {
    /// Create a new engine with default rules (PII, prompt injection, toxicity, length).
    #[new]
    fn new() -> Self {
        Self {
            inner: argentor_agent::guardrails::GuardrailEngine::new(),
        }
    }

    /// Validate text before sending it to an LLM.
    fn check_input(&self, text: &str) -> PyGuardrailResult {
        convert_guardrail_result(self.inner.check_input(text))
    }

    /// Validate text after receiving it from an LLM.
    fn check_output(&self, text: &str) -> PyGuardrailResult {
        convert_guardrail_result(self.inner.check_output(text, None))
    }

    /// Redact PII from text, returning `(sanitized_text, pii_matches_json)`.
    #[staticmethod]
    fn redact_pii(text: &str) -> (String, String) {
        let (sanitized, matches) = argentor_agent::guardrails::redact_pii(text);
        let matches_json: Vec<serde_json::Value> = matches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "kind": m.kind,
                    "span": [m.span.0, m.span.1],
                    "original": m.original,
                })
            })
            .collect();
        let json_str = serde_json::to_string(&matches_json).unwrap_or_default();
        (sanitized, json_str)
    }

    fn __repr__(&self) -> String {
        "GuardrailEngine(rules=default)".to_string()
    }
}

fn convert_guardrail_result(r: argentor_agent::guardrails::GuardrailResult) -> PyGuardrailResult {
    PyGuardrailResult {
        passed: r.passed,
        violations: r
            .violations
            .iter()
            .map(|v| format!("[{}] {}: {}", severity_str(&v.severity), v.rule_name, v.message))
            .collect(),
        sanitized_text: r.sanitized_text,
        processing_time_ms: r.processing_time_ms,
    }
}

fn severity_str(s: &argentor_agent::guardrails::RuleSeverity) -> &'static str {
    match s {
        argentor_agent::guardrails::RuleSeverity::Block => "BLOCK",
        argentor_agent::guardrails::RuleSeverity::Warn => "WARN",
        argentor_agent::guardrails::RuleSeverity::Log => "LOG",
    }
}

// ---------------------------------------------------------------------------
// PyCalculator -- direct access to the calculator skill
// ---------------------------------------------------------------------------

/// Direct Python interface to the calculator skill.
///
/// Example:
/// ```python
/// calc = argentor.Calculator()
/// print(calc.evaluate("2 + 3 * 4"))   # "14"
/// print(calc.execute_json('{"operation": "sqrt", "value": 144}'))
/// ```
#[pyclass(name = "Calculator")]
pub struct PyCalculator {
    skill: Arc<argentor_builtins::CalculatorSkill>,
    rt: tokio::runtime::Runtime,
}

#[pymethods]
impl PyCalculator {
    #[new]
    fn new() -> PyResult<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("{e}")))?;
        Ok(Self {
            skill: Arc::new(argentor_builtins::CalculatorSkill::new()),
            rt,
        })
    }

    /// Evaluate a math expression string (e.g. `"2 + 3 * 4"`).
    fn evaluate(&self, expression: &str) -> PyResult<String> {
        self.execute_json(&format!(
            r#"{{"operation": "evaluate", "expression": "{}"}}"#,
            expression.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    }

    /// Execute the calculator with a raw JSON arguments string.
    fn execute_json(&self, arguments_json: &str) -> PyResult<String> {
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), arguments_json)?;
        Ok(result.content)
    }

    fn __repr__(&self) -> String {
        "Calculator()".to_string()
    }
}

// ---------------------------------------------------------------------------
// PyJsonQuery -- direct access to the JSON query skill
// ---------------------------------------------------------------------------

/// Direct Python interface to the JSON query/manipulation skill.
///
/// Example:
/// ```python
/// jq = argentor.JsonQuery()
/// result = jq.execute_json('{"operation": "get", "data": {"a": {"b": 1}}, "path": "a.b"}')
/// ```
#[pyclass(name = "JsonQuery")]
pub struct PyJsonQuery {
    skill: Arc<argentor_builtins::JsonQuerySkill>,
    rt: tokio::runtime::Runtime,
}

#[pymethods]
impl PyJsonQuery {
    #[new]
    fn new() -> PyResult<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("{e}")))?;
        Ok(Self {
            skill: Arc::new(argentor_builtins::JsonQuerySkill::new()),
            rt,
        })
    }

    /// Execute a JSON query operation with raw JSON arguments.
    fn execute_json(&self, arguments_json: &str) -> PyResult<String> {
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), arguments_json)?;
        Ok(result.content)
    }

    /// Get a value at a dot-notation path from a JSON string.
    fn get(&self, data_json: &str, path: &str) -> PyResult<String> {
        let data: serde_json::Value = serde_json::from_str(data_json).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid JSON: {e}"))
        })?;
        let args = serde_json::json!({
            "operation": "get",
            "data": data,
            "path": path,
        });
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), &args.to_string())?;
        Ok(result.content)
    }

    /// List keys of a JSON object.
    fn keys(&self, data_json: &str) -> PyResult<String> {
        let data: serde_json::Value = serde_json::from_str(data_json).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid JSON: {e}"))
        })?;
        let args = serde_json::json!({
            "operation": "keys",
            "data": data,
        });
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), &args.to_string())?;
        Ok(result.content)
    }

    fn __repr__(&self) -> String {
        "JsonQuery()".to_string()
    }
}

// ---------------------------------------------------------------------------
// PyHashTool -- direct access to the hash skill
// ---------------------------------------------------------------------------

/// Direct Python interface to the cryptographic hashing skill.
///
/// Example:
/// ```python
/// h = argentor.HashTool()
/// print(h.sha256("hello world"))
/// print(h.sha512("hello world"))
/// ```
#[pyclass(name = "HashTool")]
pub struct PyHashTool {
    skill: Arc<argentor_builtins::HashSkill>,
    rt: tokio::runtime::Runtime,
}

#[pymethods]
impl PyHashTool {
    #[new]
    fn new() -> PyResult<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("{e}")))?;
        Ok(Self {
            skill: Arc::new(argentor_builtins::HashSkill::new()),
            rt,
        })
    }

    /// Compute SHA-256 hash of the input string.
    fn sha256(&self, input: &str) -> PyResult<String> {
        let args = serde_json::json!({
            "operation": "sha256",
            "input": input,
        });
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), &args.to_string())?;
        Ok(result.content)
    }

    /// Compute SHA-512 hash of the input string.
    fn sha512(&self, input: &str) -> PyResult<String> {
        let args = serde_json::json!({
            "operation": "sha512",
            "input": input,
        });
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), &args.to_string())?;
        Ok(result.content)
    }

    /// Compute HMAC-SHA256 of the input with the given key.
    fn hmac_sha256(&self, input: &str, key: &str) -> PyResult<String> {
        let args = serde_json::json!({
            "operation": "hmac_sha256",
            "input": input,
            "key": key,
        });
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), &args.to_string())?;
        Ok(result.content)
    }

    /// Execute the hash tool with raw JSON arguments.
    fn execute_json(&self, arguments_json: &str) -> PyResult<String> {
        let result = execute_skill_json(&self.rt, self.skill.as_ref(), arguments_json)?;
        Ok(result.content)
    }

    fn __repr__(&self) -> String {
        "HashTool()".to_string()
    }
}

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

/// Return the Argentor framework version.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Return a sorted list of all available built-in skill names.
#[pyfunction]
fn available_skills() -> Vec<String> {
    let registry = argentor_skills::SkillRegistry::new();
    argentor_builtins::register_builtins(&registry);
    let mut names: Vec<String> = registry
        .list_descriptors()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Python module definition
// ---------------------------------------------------------------------------

/// Argentor Python module -- Rust-powered AI agent framework.
#[pymodule]
fn argentor(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Classes
    m.add_class::<PySession>()?;
    m.add_class::<PyMessage>()?;
    m.add_class::<PyToolResult>()?;
    m.add_class::<PySkillRegistry>()?;
    m.add_class::<PyGuardrailEngine>()?;
    m.add_class::<PyGuardrailResult>()?;
    m.add_class::<PyCalculator>()?;
    m.add_class::<PyJsonQuery>()?;
    m.add_class::<PyHashTool>()?;

    // Module-level functions
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(available_skills, m)?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Execute any `Skill` impl with JSON arguments via a shared tokio runtime.
fn execute_skill_json(
    rt: &tokio::runtime::Runtime,
    skill: &dyn argentor_skills::Skill,
    arguments_json: &str,
) -> PyResult<argentor_core::ToolResult> {
    let args: serde_json::Value = serde_json::from_str(arguments_json).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("Invalid JSON arguments: {e}"))
    })?;

    let call = argentor_core::ToolCall {
        id: format!("py_{}", uuid_v4_string()),
        name: skill.descriptor().name.clone(),
        arguments: args,
    };

    // SAFETY: We must move ownership for the async block. Clone the skill descriptor
    // to build the error message, but we cannot move `skill` (it's a reference).
    // Instead we use block_on which keeps the borrow alive.
    let result = rt
        .block_on(skill.execute(call))
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Skill error: {e}")))?;

    if result.is_error {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
            "Skill returned error: {}",
            result.content
        )));
    }

    Ok(result)
}

/// Generate a UUID v4 string without pulling in the uuid crate directly.
fn uuid_v4_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple pseudo-unique ID; real UUID generation happens in argentor-core.
    format!("{nanos:032x}")
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

//! Universal high-level API for running agent queries.
//!
//! Inspired by Claude Agent SDK's `query()` but model-agnostic.
//! Works with all 14 LLM providers supported by Argentor.
//!
//! # Example
//!
//! ```no_run
//! use argentor_agent::query::{query, QueryEvent, QueryOptions};
//!
//! # async fn demo() -> argentor_core::ArgentorResult<()> {
//! let mut events = query("What files are in /tmp?", QueryOptions::claude("your-key")).await?;
//! while let Some(event) = events.recv().await {
//!     match event {
//!         QueryEvent::Text { text } => print!("{text}"),
//!         QueryEvent::ToolCall { name, .. } => println!("[calling {name}]"),
//!         QueryEvent::Done { output, .. } => println!("\n{output}"),
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```

use crate::backends::LlmBackend;
use crate::config::{LlmProvider, ModelConfig};
use crate::runner::AgentRunner;
use crate::stream::StreamEvent;
use argentor_core::ArgentorResult;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// QueryEvent
// ---------------------------------------------------------------------------

/// Events yielded during query execution.
///
/// These provide a unified view of agent progress regardless of the underlying
/// LLM provider. Consumers can pattern-match on variants to render UI, log
/// progress, or collect the final output.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryEvent {
    /// Agent started processing.
    Started {
        /// The session identifier for this query.
        session_id: String,
    },
    /// Thinking/reasoning text from the model.
    Thinking {
        /// Reasoning fragment.
        text: String,
    },
    /// Partial output text (streaming token).
    Text {
        /// The text fragment.
        text: String,
    },
    /// Agent is calling a tool.
    ToolCall {
        /// Tool name.
        name: String,
        /// Tool call arguments as JSON.
        arguments: serde_json::Value,
        /// Provider-assigned call identifier.
        call_id: String,
    },
    /// Tool returned a result.
    ToolResult {
        /// Tool name.
        name: String,
        /// Result content.
        content: String,
        /// Whether the tool reported an error.
        is_error: bool,
    },
    /// Agent completed with final output.
    Done {
        /// The final text output from the agent.
        output: String,
        /// Number of agentic-loop turns executed.
        turns: u32,
        /// Estimated tokens consumed (best-effort).
        tokens_used: u64,
        /// Estimated cost in USD (best-effort, 0.0 when unknown).
        cost_usd: f64,
    },
    /// An error occurred during query execution.
    Error {
        /// Error description.
        message: String,
    },
    /// Turn boundary (new loop iteration).
    Turn {
        /// The 1-based turn number.
        number: u32,
    },
}

// ---------------------------------------------------------------------------
// ToolConfig
// ---------------------------------------------------------------------------

/// How tools (skills) should be configured for a query.
#[derive(Clone)]
pub enum ToolConfig {
    /// Register default builtins (requires `argentor-builtins` at call site).
    Builtins,
    /// Use only the named tools from a registry.
    Only(Vec<String>),
    /// No tools available to the agent.
    None,
    /// Provide a fully custom skill registry.
    Custom(Arc<SkillRegistry>),
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self::None
    }
}

impl std::fmt::Debug for ToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtins => write!(f, "Builtins"),
            Self::Only(names) => f.debug_tuple("Only").field(names).finish(),
            Self::None => write!(f, "None"),
            Self::Custom(_) => write!(f, "Custom(SkillRegistry)"),
        }
    }
}

// ---------------------------------------------------------------------------
// QueryOptions
// ---------------------------------------------------------------------------

/// Configuration for a query invocation.
///
/// Use the convenience constructors ([`QueryOptions::claude`],
/// [`QueryOptions::openai`], etc.) for quick setup, then chain builder methods
/// to customize.
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// LLM provider to use.
    pub provider: LlmProvider,
    /// Model identifier (e.g., `"claude-sonnet-4-20250514"`).
    pub model: String,
    /// API key (ignored for local providers like Ollama).
    pub api_key: String,
    /// Override the default API base URL.
    pub api_base_url: Option<String>,
    /// System prompt given to the agent.
    pub system_prompt: Option<String>,
    /// Maximum agentic-loop turns before stopping.
    pub max_turns: u32,
    /// Sampling temperature.
    pub temperature: f32,
    /// Maximum tokens per LLM response.
    pub max_tokens: u32,
    /// Tool/skill configuration.
    pub tools: ToolConfig,
    /// Whether to enable default guardrails (PII, prompt injection, etc.).
    pub guardrails: bool,
    /// Resume an existing session by ID (not yet wired to persistent stores).
    pub session_id: Option<String>,
    /// Permission mode hint: `"default"`, `"strict"`, `"permissive"`,
    /// `"plan"`, or `"readonly"`.
    pub permission_mode: Option<String>,
}

impl QueryOptions {
    // -- Quick-setup constructors ------------------------------------------

    /// Quick setup for Anthropic Claude.
    pub fn claude(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::Claude, "claude-sonnet-4-20250514", api_key)
    }

    /// Quick setup for OpenAI.
    pub fn openai(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::OpenAi, "gpt-4o", api_key)
    }

    /// Quick setup for Google Gemini.
    pub fn gemini(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::Gemini, "gemini-2.0-flash", api_key)
    }

    /// Quick setup for local Ollama (no API key required).
    pub fn ollama(model: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::Ollama, model, "")
    }

    /// Quick setup for OpenRouter.
    pub fn openrouter(api_key: impl Into<String>) -> Self {
        Self::with_provider(
            LlmProvider::OpenRouter,
            "anthropic/claude-sonnet-4",
            api_key,
        )
    }

    /// Quick setup for Groq.
    pub fn groq(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::Groq, "llama-3.3-70b-versatile", api_key)
    }

    /// Quick setup for Mistral.
    pub fn mistral(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::Mistral, "mistral-large-latest", api_key)
    }

    /// Quick setup for xAI (Grok).
    pub fn xai(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::XAi, "grok-2", api_key)
    }

    /// Quick setup for DeepSeek.
    pub fn deepseek(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::DeepSeek, "deepseek-chat", api_key)
    }

    /// Quick setup for Together AI.
    pub fn together(api_key: impl Into<String>) -> Self {
        Self::with_provider(
            LlmProvider::Together,
            "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
            api_key,
        )
    }

    /// Quick setup for Cerebras.
    pub fn cerebras(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::Cerebras, "llama3.1-70b", api_key)
    }

    /// Quick setup for Azure OpenAI.
    pub fn azure_openai(api_key: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::AzureOpenAi, "gpt-4o", api_key)
    }

    /// Quick setup for vLLM (local, no API key required).
    pub fn vllm(model: impl Into<String>) -> Self {
        Self::with_provider(LlmProvider::VLlm, model, "")
    }

    /// Quick setup for Claude Code (local CLI, no API key required).
    pub fn claude_code() -> Self {
        Self::with_provider(LlmProvider::ClaudeCode, "claude-code", "")
    }

    /// Generic constructor for any provider.
    pub fn with_provider(
        provider: LlmProvider,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            api_key: api_key.into(),
            api_base_url: None,
            system_prompt: None,
            max_turns: 20,
            temperature: 0.7,
            max_tokens: 4096,
            tools: ToolConfig::None,
            guardrails: false,
            session_id: None,
            permission_mode: None,
        }
    }

    // -- Builder methods ---------------------------------------------------

    /// Set a custom system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum number of agentic-loop turns.
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.max_turns = turns;
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    /// Set the maximum tokens per LLM response.
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    /// Configure which tools are available to the agent.
    pub fn tools(mut self, config: ToolConfig) -> Self {
        self.tools = config;
        self
    }

    /// Enable default guardrails (PII detection, prompt injection, toxicity).
    pub fn with_guardrails(mut self) -> Self {
        self.guardrails = true;
        self
    }

    /// Resume an existing session by its ID.
    pub fn resume_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the permission mode (`"default"`, `"strict"`, `"permissive"`,
    /// `"plan"`, or `"readonly"`).
    pub fn permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.permission_mode = Some(mode.into());
        self
    }

    /// Override the API base URL.
    pub fn api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = Some(url.into());
        self
    }

    // -- Internal helpers --------------------------------------------------

    /// Build a [`ModelConfig`] from these options.
    fn to_model_config(&self) -> ModelConfig {
        ModelConfig {
            provider: self.provider.clone(),
            model_id: self.model.clone(),
            api_key: self.api_key.clone(),
            api_base_url: self.api_base_url.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            max_turns: self.max_turns,
            max_context_tokens: 200_000,
            fallback_models: vec![],
            retry_policy: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Core query functions
// ---------------------------------------------------------------------------

/// Run a query and receive events via an unbounded channel.
///
/// Spawns a background task that drives the agent loop. Returns a receiver
/// that yields [`QueryEvent`]s as the agent progresses.
///
/// # Errors
///
/// Returns an error if the agent runner cannot be constructed.
pub async fn query(
    prompt: &str,
    options: QueryOptions,
) -> ArgentorResult<mpsc::UnboundedReceiver<QueryEvent>> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let prompt = prompt.to_owned();

    // Build skill registry
    let skills: Arc<SkillRegistry> = match &options.tools {
        ToolConfig::Custom(registry) => Arc::clone(registry),
        _ => Arc::new(SkillRegistry::new()),
    };

    // Build audit log (temp directory, ephemeral)
    let audit_dir = std::env::temp_dir().join(format!("argentor-query-{}", uuid::Uuid::new_v4()));
    let audit = Arc::new(AuditLog::new(audit_dir));

    // Build permissions
    let permissions = PermissionSet::new();

    // Build model config
    let config = options.to_model_config();

    // Build agent runner
    let mut agent = AgentRunner::new(config, skills, permissions, audit);

    // Apply system prompt
    if let Some(sp) = &options.system_prompt {
        agent = agent.with_system_prompt(sp.clone());
    }

    // Apply guardrails
    if options.guardrails {
        agent = agent.with_default_guardrails();
    }

    // Create session
    let mut session = Session::new();
    let session_id = session.id.to_string();

    // Notify: started
    let _ = event_tx.send(QueryEvent::Started {
        session_id: session_id.clone(),
    });

    // Spawn the agent loop in a background task
    tokio::spawn(async move {
        // Use streaming path: forward StreamEvents as QueryEvents
        let (stream_tx, mut stream_rx) = mpsc::unbounded_channel::<StreamEvent>();

        // We run the streaming loop in a spawned task and bridge events
        let result = agent.run_streaming(&mut session, &prompt, stream_tx).await;

        // Note: run_streaming sends StreamEvents to stream_tx synchronously
        // within its own loop, so by the time it returns, all stream events
        // have already been sent. We drain any remaining events first.
        while let Ok(stream_event) = stream_rx.try_recv() {
            let query_event = stream_event_to_query_event(stream_event);
            let _ = event_tx.send(query_event);
        }

        match result {
            Ok(output) => {
                let _ = event_tx.send(QueryEvent::Done {
                    output,
                    turns: 0, // best-effort; detailed tracking is a future enhancement
                    tokens_used: 0,
                    cost_usd: 0.0,
                });
            }
            Err(e) => {
                let _ = event_tx.send(QueryEvent::Error {
                    message: e.to_string(),
                });
            }
        }
    });

    Ok(event_rx)
}

/// Run a query using a custom [`LlmBackend`] (useful for testing with mocks).
///
/// Behaves identically to [`query`] but allows injecting a backend directly
/// instead of constructing one from provider configuration.
pub async fn query_with_backend(
    prompt: &str,
    backend: Box<dyn LlmBackend>,
    options: QueryOptions,
) -> ArgentorResult<mpsc::UnboundedReceiver<QueryEvent>> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let prompt = prompt.to_owned();

    // Build skill registry
    let skills: Arc<SkillRegistry> = match &options.tools {
        ToolConfig::Custom(registry) => Arc::clone(registry),
        _ => Arc::new(SkillRegistry::new()),
    };

    // Build audit log (temp directory, ephemeral)
    let audit_dir = std::env::temp_dir().join(format!("argentor-query-{}", uuid::Uuid::new_v4()));
    let audit = Arc::new(AuditLog::new(audit_dir));

    // Build permissions
    let permissions = PermissionSet::new();

    // Build agent runner from backend
    let mut agent =
        AgentRunner::from_backend(backend, skills, permissions, audit, options.max_turns);

    // Apply system prompt
    if let Some(sp) = &options.system_prompt {
        agent = agent.with_system_prompt(sp.clone());
    }

    // Apply guardrails
    if options.guardrails {
        agent = agent.with_default_guardrails();
    }

    // Create session
    let mut session = Session::new();
    let session_id = session.id.to_string();

    // Notify: started
    let _ = event_tx.send(QueryEvent::Started {
        session_id: session_id.clone(),
    });

    // Spawn the agent loop
    tokio::spawn(async move {
        let result = agent.run(&mut session, &prompt).await;

        match result {
            Ok(output) => {
                let _ = event_tx.send(QueryEvent::Done {
                    output,
                    turns: 0,
                    tokens_used: 0,
                    cost_usd: 0.0,
                });
            }
            Err(e) => {
                let _ = event_tx.send(QueryEvent::Error {
                    message: e.to_string(),
                });
            }
        }
    });

    Ok(event_rx)
}

/// Run a query and return just the final output string.
///
/// This is the simplest possible API — it blocks until the agent finishes and
/// returns the final response text.
///
/// # Errors
///
/// Returns an error if the agent fails to produce a response.
pub async fn query_simple(prompt: &str, options: QueryOptions) -> ArgentorResult<String> {
    let mut rx = query(prompt, options).await?;
    let mut final_output = None;
    let mut last_error = None;

    while let Some(event) = rx.recv().await {
        match event {
            QueryEvent::Done { output, .. } => {
                final_output = Some(output);
            }
            QueryEvent::Error { message } => {
                last_error = Some(message);
            }
            _ => {}
        }
    }

    if let Some(output) = final_output {
        Ok(output)
    } else if let Some(err) = last_error {
        Err(argentor_core::ArgentorError::Agent(err))
    } else {
        Err(argentor_core::ArgentorError::Agent(
            "Query completed without producing output".to_string(),
        ))
    }
}

/// Run a query and return just the final output, using a custom backend.
///
/// Convenience wrapper around [`query_with_backend`] that collects events
/// and returns the final text.
pub async fn query_simple_with_backend(
    prompt: &str,
    backend: Box<dyn LlmBackend>,
    options: QueryOptions,
) -> ArgentorResult<String> {
    let mut rx = query_with_backend(prompt, backend, options).await?;
    let mut final_output = None;
    let mut last_error = None;

    while let Some(event) = rx.recv().await {
        match event {
            QueryEvent::Done { output, .. } => {
                final_output = Some(output);
            }
            QueryEvent::Error { message } => {
                last_error = Some(message);
            }
            _ => {}
        }
    }

    if let Some(output) = final_output {
        Ok(output)
    } else if let Some(err) = last_error {
        Err(argentor_core::ArgentorError::Agent(err))
    } else {
        Err(argentor_core::ArgentorError::Agent(
            "Query completed without producing output".to_string(),
        ))
    }
}

/// Run a query with a callback invoked for each event.
///
/// Returns the final output string once the agent finishes.
///
/// # Errors
///
/// Returns an error if the agent fails to produce a response.
pub async fn query_with_callback<F>(
    prompt: &str,
    options: QueryOptions,
    callback: F,
) -> ArgentorResult<String>
where
    F: Fn(QueryEvent) + Send + 'static,
{
    let mut rx = query(prompt, options).await?;
    let mut final_output = None;
    let mut last_error = None;

    while let Some(event) = rx.recv().await {
        match &event {
            QueryEvent::Done { output, .. } => {
                final_output = Some(output.clone());
            }
            QueryEvent::Error { message } => {
                last_error = Some(message.clone());
            }
            _ => {}
        }
        callback(event);
    }

    if let Some(output) = final_output {
        Ok(output)
    } else if let Some(err) = last_error {
        Err(argentor_core::ArgentorError::Agent(err))
    } else {
        Err(argentor_core::ArgentorError::Agent(
            "Query completed without producing output".to_string(),
        ))
    }
}

/// Run a query with a callback, using a custom backend.
pub async fn query_with_callback_and_backend<F>(
    prompt: &str,
    backend: Box<dyn LlmBackend>,
    options: QueryOptions,
    callback: F,
) -> ArgentorResult<String>
where
    F: Fn(QueryEvent) + Send + 'static,
{
    let mut rx = query_with_backend(prompt, backend, options).await?;
    let mut final_output = None;
    let mut last_error = None;

    while let Some(event) = rx.recv().await {
        match &event {
            QueryEvent::Done { output, .. } => {
                final_output = Some(output.clone());
            }
            QueryEvent::Error { message } => {
                last_error = Some(message.clone());
            }
            _ => {}
        }
        callback(event);
    }

    if let Some(output) = final_output {
        Ok(output)
    } else if let Some(err) = last_error {
        Err(argentor_core::ArgentorError::Agent(err))
    } else {
        Err(argentor_core::ArgentorError::Agent(
            "Query completed without producing output".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Convenience one-liners
// ---------------------------------------------------------------------------

/// Ask Claude a question (simplest possible API).
///
/// ```no_run
/// # async fn demo() -> argentor_core::ArgentorResult<()> {
/// let answer = argentor_agent::query::ask_claude("What is 2+2?", "sk-ant-...").await?;
/// println!("{answer}");
/// # Ok(())
/// # }
/// ```
pub async fn ask_claude(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::claude(api_key)).await
}

/// Ask OpenAI a question.
pub async fn ask_openai(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::openai(api_key)).await
}

/// Ask Google Gemini a question.
pub async fn ask_gemini(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::gemini(api_key)).await
}

/// Ask a local Ollama model a question (no API key required).
pub async fn ask_ollama(prompt: &str, model: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::ollama(model)).await
}

/// Ask via OpenRouter.
pub async fn ask_openrouter(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::openrouter(api_key)).await
}

/// Ask Groq a question.
pub async fn ask_groq(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::groq(api_key)).await
}

/// Ask Mistral a question.
pub async fn ask_mistral(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::mistral(api_key)).await
}

/// Ask DeepSeek a question.
pub async fn ask_deepseek(prompt: &str, api_key: &str) -> ArgentorResult<String> {
    query_simple(prompt, QueryOptions::deepseek(api_key)).await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert a low-level [`StreamEvent`] into a [`QueryEvent`].
fn stream_event_to_query_event(event: StreamEvent) -> QueryEvent {
    match event {
        StreamEvent::TextDelta { text } => QueryEvent::Text { text },
        StreamEvent::ToolCallStart { id, name } => QueryEvent::ToolCall {
            name,
            arguments: serde_json::Value::Null,
            call_id: id,
        },
        StreamEvent::ToolCallDelta {
            id: _,
            arguments_delta,
        } => QueryEvent::Text {
            text: arguments_delta,
        },
        StreamEvent::ToolCallEnd { id: _ } => QueryEvent::ToolResult {
            name: String::new(),
            content: String::new(),
            is_error: false,
        },
        StreamEvent::Done => QueryEvent::Turn { number: 0 },
        StreamEvent::Error { message } => QueryEvent::Error { message },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::LlmBackend;
    use crate::llm::LlmResponse;
    use argentor_core::{ArgentorResult, Message};
    use argentor_skills::SkillDescriptor;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};

    // -- Mock backend that returns a fixed response -------------------------

    struct MockBackend {
        response: String,
    }

    impl MockBackend {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[async_trait]
    impl LlmBackend for MockBackend {
        async fn chat(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            Ok(LlmResponse::Done(self.response.clone()))
        }

        fn provider_name(&self) -> &str {
            "mock"
        }

        async fn chat_stream(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<(
            tokio::sync::mpsc::Receiver<StreamEvent>,
            tokio::task::JoinHandle<ArgentorResult<LlmResponse>>,
        )> {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let response = self.response.clone();
            let handle = tokio::spawn(async move {
                let _ = tx
                    .send(StreamEvent::TextDelta {
                        text: response.clone(),
                    })
                    .await;
                let _ = tx.send(StreamEvent::Done).await;
                Ok(LlmResponse::Done(response))
            });
            Ok((rx, handle))
        }
    }

    // -- Mock backend that counts calls ------------------------------------

    struct CountingBackend {
        response: String,
        call_count: Arc<AtomicU32>,
    }

    impl CountingBackend {
        fn new(response: impl Into<String>) -> (Self, Arc<AtomicU32>) {
            let count = Arc::new(AtomicU32::new(0));
            (
                Self {
                    response: response.into(),
                    call_count: Arc::clone(&count),
                },
                count,
            )
        }
    }

    #[async_trait]
    impl LlmBackend for CountingBackend {
        async fn chat(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse::Done(self.response.clone()))
        }

        fn provider_name(&self) -> &str {
            "counting-mock"
        }

        async fn chat_stream(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<(
            tokio::sync::mpsc::Receiver<StreamEvent>,
            tokio::task::JoinHandle<ArgentorResult<LlmResponse>>,
        )> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let response = self.response.clone();
            let handle = tokio::spawn(async move {
                let _ = tx.send(StreamEvent::Done).await;
                Ok(LlmResponse::Done(response))
            });
            Ok((rx, handle))
        }
    }

    // -- Mock backend that returns an error --------------------------------

    struct ErrorBackend {
        message: String,
    }

    impl ErrorBackend {
        fn new(message: impl Into<String>) -> Self {
            Self {
                message: message.into(),
            }
        }
    }

    #[async_trait]
    impl LlmBackend for ErrorBackend {
        async fn chat(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            Err(argentor_core::ArgentorError::Agent(self.message.clone()))
        }

        fn provider_name(&self) -> &str {
            "error-mock"
        }

        async fn chat_stream(
            &self,
            _system_prompt: Option<&str>,
            _messages: &[Message],
            _tools: &[SkillDescriptor],
        ) -> ArgentorResult<(
            tokio::sync::mpsc::Receiver<StreamEvent>,
            tokio::task::JoinHandle<ArgentorResult<LlmResponse>>,
        )> {
            Err(argentor_core::ArgentorError::Agent(self.message.clone()))
        }
    }

    // ======================================================================
    // Tests
    // ======================================================================

    // -- QueryOptions construction -----------------------------------------

    #[test]
    fn test_query_options_claude() {
        let opts = QueryOptions::claude("sk-ant-test");
        assert!(matches!(opts.provider, LlmProvider::Claude));
        assert_eq!(opts.model, "claude-sonnet-4-20250514");
        assert_eq!(opts.api_key, "sk-ant-test");
        assert_eq!(opts.max_turns, 20);
        assert_eq!(opts.temperature, 0.7);
        assert_eq!(opts.max_tokens, 4096);
        assert!(!opts.guardrails);
        assert!(opts.session_id.is_none());
        assert!(opts.system_prompt.is_none());
    }

    #[test]
    fn test_query_options_openai() {
        let opts = QueryOptions::openai("sk-test");
        assert!(matches!(opts.provider, LlmProvider::OpenAi));
        assert_eq!(opts.model, "gpt-4o");
        assert_eq!(opts.api_key, "sk-test");
    }

    #[test]
    fn test_query_options_gemini() {
        let opts = QueryOptions::gemini("gemini-key");
        assert!(matches!(opts.provider, LlmProvider::Gemini));
        assert_eq!(opts.model, "gemini-2.0-flash");
        assert_eq!(opts.api_key, "gemini-key");
    }

    #[test]
    fn test_query_options_ollama() {
        let opts = QueryOptions::ollama("llama3");
        assert!(matches!(opts.provider, LlmProvider::Ollama));
        assert_eq!(opts.model, "llama3");
        assert_eq!(opts.api_key, "");
    }

    #[test]
    fn test_query_options_openrouter() {
        let opts = QueryOptions::openrouter("or-key");
        assert!(matches!(opts.provider, LlmProvider::OpenRouter));
        assert_eq!(opts.model, "anthropic/claude-sonnet-4");
    }

    #[test]
    fn test_query_options_groq() {
        let opts = QueryOptions::groq("groq-key");
        assert!(matches!(opts.provider, LlmProvider::Groq));
        assert_eq!(opts.model, "llama-3.3-70b-versatile");
    }

    #[test]
    fn test_query_options_mistral() {
        let opts = QueryOptions::mistral("mistral-key");
        assert!(matches!(opts.provider, LlmProvider::Mistral));
        assert_eq!(opts.model, "mistral-large-latest");
    }

    #[test]
    fn test_query_options_xai() {
        let opts = QueryOptions::xai("xai-key");
        assert!(matches!(opts.provider, LlmProvider::XAi));
        assert_eq!(opts.model, "grok-2");
    }

    #[test]
    fn test_query_options_deepseek() {
        let opts = QueryOptions::deepseek("ds-key");
        assert!(matches!(opts.provider, LlmProvider::DeepSeek));
        assert_eq!(opts.model, "deepseek-chat");
    }

    #[test]
    fn test_query_options_together() {
        let opts = QueryOptions::together("tog-key");
        assert!(matches!(opts.provider, LlmProvider::Together));
    }

    #[test]
    fn test_query_options_cerebras() {
        let opts = QueryOptions::cerebras("cb-key");
        assert!(matches!(opts.provider, LlmProvider::Cerebras));
    }

    #[test]
    fn test_query_options_azure_openai() {
        let opts = QueryOptions::azure_openai("az-key");
        assert!(matches!(opts.provider, LlmProvider::AzureOpenAi));
        assert_eq!(opts.model, "gpt-4o");
    }

    #[test]
    fn test_query_options_vllm() {
        let opts = QueryOptions::vllm("my-model");
        assert!(matches!(opts.provider, LlmProvider::VLlm));
        assert_eq!(opts.model, "my-model");
        assert_eq!(opts.api_key, "");
    }

    #[test]
    fn test_query_options_claude_code() {
        let opts = QueryOptions::claude_code();
        assert!(matches!(opts.provider, LlmProvider::ClaudeCode));
    }

    // -- Builder methods ---------------------------------------------------

    #[test]
    fn test_builder_system_prompt() {
        let opts = QueryOptions::claude("key").system_prompt("You are a pirate.");
        assert_eq!(opts.system_prompt.as_deref(), Some("You are a pirate."));
    }

    #[test]
    fn test_builder_max_turns() {
        let opts = QueryOptions::claude("key").max_turns(5);
        assert_eq!(opts.max_turns, 5);
    }

    #[test]
    fn test_builder_temperature() {
        let opts = QueryOptions::claude("key").temperature(0.2);
        assert!((opts.temperature - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_builder_max_tokens() {
        let opts = QueryOptions::claude("key").max_tokens(8192);
        assert_eq!(opts.max_tokens, 8192);
    }

    #[test]
    fn test_builder_tools_none() {
        let opts = QueryOptions::claude("key").tools(ToolConfig::None);
        assert!(matches!(opts.tools, ToolConfig::None));
    }

    #[test]
    fn test_builder_tools_builtins() {
        let opts = QueryOptions::claude("key").tools(ToolConfig::Builtins);
        assert!(matches!(opts.tools, ToolConfig::Builtins));
    }

    #[test]
    fn test_builder_tools_only() {
        let opts =
            QueryOptions::claude("key").tools(ToolConfig::Only(vec!["echo".into(), "time".into()]));
        if let ToolConfig::Only(names) = &opts.tools {
            assert_eq!(names.len(), 2);
            assert_eq!(names[0], "echo");
            assert_eq!(names[1], "time");
        } else {
            panic!("Expected ToolConfig::Only");
        }
    }

    #[test]
    fn test_builder_tools_custom() {
        let registry = Arc::new(SkillRegistry::new());
        let opts = QueryOptions::claude("key").tools(ToolConfig::Custom(registry));
        assert!(matches!(opts.tools, ToolConfig::Custom(_)));
    }

    #[test]
    fn test_builder_guardrails() {
        let opts = QueryOptions::claude("key").with_guardrails();
        assert!(opts.guardrails);
    }

    #[test]
    fn test_builder_resume_session() {
        let opts = QueryOptions::claude("key").resume_session("sess-123");
        assert_eq!(opts.session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn test_builder_permission_mode() {
        let opts = QueryOptions::claude("key").permission_mode("strict");
        assert_eq!(opts.permission_mode.as_deref(), Some("strict"));
    }

    #[test]
    fn test_builder_api_base_url() {
        let opts = QueryOptions::claude("key").api_base_url("http://localhost:8080");
        assert_eq!(opts.api_base_url.as_deref(), Some("http://localhost:8080"));
    }

    #[test]
    fn test_builder_chaining() {
        let opts = QueryOptions::openai("key")
            .system_prompt("Be helpful")
            .max_turns(3)
            .temperature(0.5)
            .max_tokens(1024)
            .with_guardrails()
            .permission_mode("plan")
            .resume_session("abc-123");

        assert!(matches!(opts.provider, LlmProvider::OpenAi));
        assert_eq!(opts.system_prompt.as_deref(), Some("Be helpful"));
        assert_eq!(opts.max_turns, 3);
        assert!((opts.temperature - 0.5).abs() < f32::EPSILON);
        assert_eq!(opts.max_tokens, 1024);
        assert!(opts.guardrails);
        assert_eq!(opts.permission_mode.as_deref(), Some("plan"));
        assert_eq!(opts.session_id.as_deref(), Some("abc-123"));
    }

    // -- to_model_config ---------------------------------------------------

    #[test]
    fn test_to_model_config() {
        let opts = QueryOptions::claude("my-key")
            .max_turns(5)
            .temperature(0.3)
            .max_tokens(2048)
            .api_base_url("http://custom:9999");

        let config = opts.to_model_config();
        assert!(matches!(config.provider, LlmProvider::Claude));
        assert_eq!(config.model_id, "claude-sonnet-4-20250514");
        assert_eq!(config.api_key, "my-key");
        assert_eq!(config.max_turns, 5);
        assert!((config.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.max_tokens, 2048);
        assert_eq!(config.api_base_url.as_deref(), Some("http://custom:9999"));
        assert!(config.fallback_models.is_empty());
        assert!(config.retry_policy.is_none());
    }

    // -- QueryEvent serialization ------------------------------------------

    #[test]
    fn test_query_event_started_serialization() {
        let event = QueryEvent::Started {
            session_id: "sess-1".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"started\""));
        assert!(json.contains("\"session_id\":\"sess-1\""));
    }

    #[test]
    fn test_query_event_text_serialization() {
        let event = QueryEvent::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"text\""));
    }

    #[test]
    fn test_query_event_thinking_serialization() {
        let event = QueryEvent::Thinking {
            text: "reasoning...".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("reasoning..."));
    }

    #[test]
    fn test_query_event_tool_call_serialization() {
        let event = QueryEvent::ToolCall {
            name: "shell".into(),
            arguments: serde_json::json!({"command": "ls"}),
            call_id: "call-1".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        assert!(json.contains("\"name\":\"shell\""));
        assert!(json.contains("\"call_id\":\"call-1\""));
    }

    #[test]
    fn test_query_event_tool_result_serialization() {
        let event = QueryEvent::ToolResult {
            name: "shell".into(),
            content: "file.txt".into(),
            is_error: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_result\""));
        assert!(json.contains("\"is_error\":false"));
    }

    #[test]
    fn test_query_event_done_serialization() {
        let event = QueryEvent::Done {
            output: "The answer is 42".into(),
            turns: 3,
            tokens_used: 1500,
            cost_usd: 0.015,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"done\""));
        assert!(json.contains("\"turns\":3"));
        assert!(json.contains("\"tokens_used\":1500"));
        assert!(json.contains("0.015"));
    }

    #[test]
    fn test_query_event_error_serialization() {
        let event = QueryEvent::Error {
            message: "timeout".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("timeout"));
    }

    #[test]
    fn test_query_event_turn_serialization() {
        let event = QueryEvent::Turn { number: 2 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"turn\""));
        assert!(json.contains("\"number\":2"));
    }

    // -- query_simple with mock backend ------------------------------------

    #[tokio::test]
    async fn test_query_simple_with_mock_backend() {
        let backend = Box::new(MockBackend::new("Hello from mock!"));
        let opts = QueryOptions::claude("test-key");

        let result = query_simple_with_backend("Say hello", backend, opts).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello from mock!");
    }

    #[tokio::test]
    async fn test_query_simple_with_error_backend() {
        let backend = Box::new(ErrorBackend::new("LLM unavailable"));
        let opts = QueryOptions::claude("test-key");

        let result = query_simple_with_backend("Say hello", backend, opts).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("LLM unavailable"), "Got: {err}");
    }

    // -- query with events channel -----------------------------------------

    #[tokio::test]
    async fn test_query_with_backend_events() {
        let backend = Box::new(MockBackend::new("Event response"));
        let opts = QueryOptions::openai("test-key");

        let mut rx = query_with_backend("Test", backend, opts)
            .await
            .expect("query should succeed");

        let mut got_started = false;
        let mut got_done = false;
        let mut done_output = String::new();

        while let Some(event) = rx.recv().await {
            match event {
                QueryEvent::Started { .. } => got_started = true,
                QueryEvent::Done { output, .. } => {
                    got_done = true;
                    done_output = output;
                }
                _ => {}
            }
        }

        assert!(got_started, "Should have received Started event");
        assert!(got_done, "Should have received Done event");
        assert_eq!(done_output, "Event response");
    }

    #[tokio::test]
    async fn test_query_with_backend_error_events() {
        let backend = Box::new(ErrorBackend::new("Simulated failure"));
        let opts = QueryOptions::claude("key");

        let mut rx = query_with_backend("Test", backend, opts)
            .await
            .expect("query should succeed");

        let mut got_error = false;
        let mut error_msg = String::new();

        while let Some(event) = rx.recv().await {
            if let QueryEvent::Error { message } = event {
                got_error = true;
                error_msg = message;
            }
        }

        assert!(got_error, "Should have received Error event");
        assert!(
            error_msg.contains("Simulated failure"),
            "Error message mismatch: {error_msg}"
        );
    }

    // -- query_with_callback -----------------------------------------------

    #[tokio::test]
    async fn test_query_with_callback_and_backend_success() {
        let backend = Box::new(MockBackend::new("Callback response"));
        let opts = QueryOptions::claude("key");

        let events_collected = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events_collected);

        let result = query_with_callback_and_backend("Test", backend, opts, move |event| {
            events_clone.lock().unwrap().push(format!("{:?}", event));
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Callback response");

        let events = events_collected.lock().unwrap();
        assert!(
            events.len() >= 2,
            "Should have at least Started + Done events, got: {}",
            events.len()
        );
    }

    // -- ToolConfig --------------------------------------------------------

    #[test]
    fn test_tool_config_default_is_none() {
        let config = ToolConfig::default();
        assert!(matches!(config, ToolConfig::None));
    }

    #[test]
    fn test_tool_config_debug() {
        assert_eq!(format!("{:?}", ToolConfig::Builtins), "Builtins");
        assert_eq!(format!("{:?}", ToolConfig::None), "None");
        assert!(format!("{:?}", ToolConfig::Only(vec!["a".into()])).contains("Only"));
        let reg = Arc::new(SkillRegistry::new());
        assert!(format!("{:?}", ToolConfig::Custom(reg)).contains("Custom"));
    }

    // -- stream_event_to_query_event --------------------------------------

    #[test]
    fn test_stream_event_text_delta_conversion() {
        let se = StreamEvent::TextDelta {
            text: "chunk".into(),
        };
        let qe = stream_event_to_query_event(se);
        if let QueryEvent::Text { text: t } = qe {
            assert_eq!(t, "chunk");
        } else {
            panic!("Expected QueryEvent::Text");
        }
    }

    #[test]
    fn test_stream_event_tool_call_start_conversion() {
        let se = StreamEvent::ToolCallStart {
            id: "tc-1".into(),
            name: "echo".into(),
        };
        let qe = stream_event_to_query_event(se);
        if let QueryEvent::ToolCall { name, call_id, .. } = qe {
            assert_eq!(name, "echo");
            assert_eq!(call_id, "tc-1");
        } else {
            panic!("Expected QueryEvent::ToolCall");
        }
    }

    #[test]
    fn test_stream_event_error_conversion() {
        let se = StreamEvent::Error {
            message: "fail".into(),
        };
        let qe = stream_event_to_query_event(se);
        if let QueryEvent::Error { message } = qe {
            assert_eq!(message, "fail");
        } else {
            panic!("Expected QueryEvent::Error");
        }
    }

    #[test]
    fn test_stream_event_done_conversion() {
        let se = StreamEvent::Done;
        let qe = stream_event_to_query_event(se);
        assert!(matches!(qe, QueryEvent::Turn { .. }));
    }

    // -- Counting backend (ensures agent calls LLM exactly once) -----------

    #[tokio::test]
    async fn test_counting_backend_single_call() {
        let (backend, count) = CountingBackend::new("One call");
        let opts = QueryOptions::claude("key");

        let result = query_simple_with_backend("Hi", Box::new(backend), opts).await;
        assert!(result.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    // -- Default values verification --------------------------------------

    #[test]
    fn test_default_values_claude() {
        let opts = QueryOptions::claude("k");
        assert_eq!(opts.max_turns, 20);
        assert_eq!(opts.max_tokens, 4096);
        assert!((opts.temperature - 0.7).abs() < f32::EPSILON);
        assert!(!opts.guardrails);
        assert!(opts.api_base_url.is_none());
        assert!(opts.system_prompt.is_none());
        assert!(opts.session_id.is_none());
        assert!(opts.permission_mode.is_none());
        assert!(matches!(opts.tools, ToolConfig::None));
    }

    #[test]
    fn test_default_values_ollama() {
        let opts = QueryOptions::ollama("phi3");
        assert_eq!(opts.api_key, "");
        assert_eq!(opts.max_turns, 20);
    }

    // -- with_provider generic constructor --------------------------------

    #[test]
    fn test_with_provider_generic() {
        let opts = QueryOptions::with_provider(LlmProvider::Mistral, "mistral-medium", "mist-key");
        assert!(matches!(opts.provider, LlmProvider::Mistral));
        assert_eq!(opts.model, "mistral-medium");
        assert_eq!(opts.api_key, "mist-key");
    }
}

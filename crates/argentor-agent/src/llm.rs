use crate::backends::bedrock::BedrockBackend;
use crate::backends::claude::ClaudeBackend;
use crate::backends::claude_code::ClaudeCodeBackend;
use crate::backends::cohere::CohereBackend;
use crate::backends::gemini::GeminiBackend;
use crate::backends::openai::OpenAiBackend;
use crate::backends::replicate::ReplicateBackend;
use crate::backends::LlmBackend;
use crate::config::{LlmProvider, ModelConfig};
use crate::failover::FailoverBackend;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorResult, Message, ToolCall};
use argentor_skills::SkillDescriptor;
use tokio::sync::mpsc;

/// Response from the LLM — either text content or a tool call request.
#[derive(Debug, Clone)]
pub enum LlmResponse {
    /// Pure text response.
    Text(String),
    /// Response requesting tool invocations.
    ToolUse {
        /// Optional text content accompanying the tool calls.
        content: Option<String>,
        /// Tool calls the model wants to execute.
        tool_calls: Vec<ToolCall>,
    },
    /// Final response indicating the conversation turn is complete.
    Done(String),
}

/// LLM client that dispatches to the correct provider backend.
///
/// Uses the `LlmBackend` trait to abstract away provider-specific API differences.
/// To add a new provider: implement `LlmBackend` in `backends/` and wire it here.
pub struct LlmClient {
    backend: Box<dyn LlmBackend>,
}

impl LlmClient {
    /// Create a new LLM client from the given model configuration.
    pub fn new(config: ModelConfig) -> Self {
        let fallback_models = config.fallback_models.clone();
        let retry_policy = config.retry_policy.clone();

        let make_backend = |cfg: ModelConfig| -> Box<dyn LlmBackend> {
            match cfg.provider {
                LlmProvider::Claude => Box::new(ClaudeBackend::new(cfg)),
                LlmProvider::Gemini => Box::new(GeminiBackend::new(cfg)),
                LlmProvider::ClaudeCode => Box::new(ClaudeCodeBackend::new(cfg)),
                LlmProvider::Cohere => Box::new(CohereBackend::new(cfg)),
                LlmProvider::Bedrock => Box::new(BedrockBackend::new(cfg)),
                LlmProvider::Replicate => Box::new(ReplicateBackend::new(cfg)),
                // All OpenAI-compatible providers
                LlmProvider::OpenAi
                | LlmProvider::OpenRouter
                | LlmProvider::Groq
                | LlmProvider::Ollama
                | LlmProvider::Mistral
                | LlmProvider::XAi
                | LlmProvider::AzureOpenAi
                | LlmProvider::Cerebras
                | LlmProvider::Together
                | LlmProvider::DeepSeek
                | LlmProvider::VLlm
                | LlmProvider::Fireworks
                | LlmProvider::HuggingFace => Box::new(OpenAiBackend::new(cfg)),
            }
        };

        let backend: Box<dyn LlmBackend> = if fallback_models.is_empty() {
            make_backend(config)
        } else {
            let policy = retry_policy.unwrap_or_default();
            let mut backends: Vec<Box<dyn LlmBackend>> = Vec::new();
            backends.push(make_backend(config));
            for fallback in fallback_models {
                backends.push(make_backend(fallback));
            }
            Box::new(FailoverBackend::new(backends, policy))
        };

        Self { backend }
    }

    /// Create from a pre-built backend (for custom/external providers).
    pub fn from_backend(backend: Box<dyn LlmBackend>) -> Self {
        Self { backend }
    }

    /// Return a short name identifying the backend provider (for metrics/circuit breaker).
    pub fn provider_name(&self) -> &str {
        self.backend.provider_name()
    }

    /// Non-streaming chat completion.
    pub async fn chat(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        self.backend.chat(system_prompt, messages, tools).await
    }

    /// Streaming chat completion.
    ///
    /// Returns an `mpsc::Receiver<StreamEvent>` that yields events as the LLM
    /// generates its response, plus the final aggregated `LlmResponse` when done.
    pub async fn chat_stream(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        tokio::task::JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        self.backend
            .chat_stream(system_prompt, messages, tools)
            .await
    }
}

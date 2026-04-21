use crate::failover::RetryPolicy;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Default Ollama host, overridable via `OLLAMA_HOST` env var.
static OLLAMA_HOST: LazyLock<String> = LazyLock::new(|| {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string())
});

/// Default vLLM host, overridable via `VLLM_HOST` env var.
static VLLM_HOST: LazyLock<String> = LazyLock::new(|| {
    std::env::var("VLLM_HOST").unwrap_or_else(|_| "http://localhost:8000".to_string())
});

/// Supported LLM provider backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    /// Anthropic Claude API.
    Claude,
    /// OpenAI API (GPT-4, etc.).
    OpenAi,
    /// OpenRouter multi-provider proxy.
    OpenRouter,
    /// Groq cloud inference — OpenAI-compatible API, free tier with rate limits.
    Groq,
    /// Use the local `claude` CLI in headless mode (-p --output-format json).
    /// No API key needed — uses the user's existing Claude Code session/subscription.
    ClaudeCode,
    /// Google Gemini — native REST API (not OpenAI-compatible).
    Gemini,
    /// Local Ollama — OpenAI-compatible API at localhost:11434.
    Ollama,
    /// Mistral AI — OpenAI-compatible API.
    Mistral,
    /// xAI (Grok) — OpenAI-compatible API.
    #[serde(alias = "xai")]
    XAi,
    /// Azure OpenAI — OpenAI-compatible but with different auth and URL scheme.
    #[serde(alias = "azure_openai", alias = "azure")]
    AzureOpenAi,
    /// Cerebras — OpenAI-compatible API for fast inference.
    Cerebras,
    /// Together AI — OpenAI-compatible API for open-source models.
    Together,
    /// DeepSeek — OpenAI-compatible API.
    DeepSeek,
    /// vLLM — OpenAI-compatible API for self-hosted inference.
    #[serde(alias = "vllm")]
    VLlm,
    /// Fireworks AI — OpenAI-compatible API for open-weights models.
    Fireworks,
    /// Hugging Face Inference API — OpenAI-compatible endpoint.
    #[serde(alias = "hugging_face", alias = "huggingface")]
    HuggingFace,
    /// Cohere — native `v2/chat` REST API (not OpenAI-compatible).
    Cohere,
    /// AWS Bedrock — SigV4-signed requests. Ships as a stub; real path
    /// requires the `aws-sdk-bedrock` crate behind an `aws-bedrock` feature.
    #[serde(alias = "aws_bedrock", alias = "aws-bedrock")]
    Bedrock,
    /// Replicate — async prediction polling API.
    Replicate,
}

/// Configuration for an LLM model used by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Which LLM provider to use.
    pub provider: LlmProvider,
    /// Model identifier (e.g., `"claude-sonnet-4-20250514"`, `"gpt-4o"`).
    pub model_id: String,
    /// API key for authentication (ignored for local providers).
    pub api_key: String,
    /// Override the default API base URL for this provider.
    pub api_base_url: Option<String>,
    /// Sampling temperature (0.0 = deterministic, 1.0 = creative). Default: 0.7.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum tokens to generate per response. Default: 4096.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Maximum agentic loop turns before stopping. Default: 20.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// Total context window size for the model in tokens.
    ///
    /// Used by the adaptive compaction trigger to compute the percentage
    /// threshold at which to summarise conversation history.
    /// Defaults to 200 000 (Claude's context window).
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: u64,
    /// Fallback model configs tried in order if the primary fails.
    #[serde(default)]
    pub fallback_models: Vec<ModelConfig>,
    /// Retry policy for transient errors.
    #[serde(default)]
    pub retry_policy: Option<RetryPolicy>,
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_max_turns() -> u32 {
    20
}

fn default_max_context_tokens() -> u64 {
    200_000
}

impl ModelConfig {
    /// Returns `true` if this config has a usable API key.
    ///
    /// Providers that don't need a key (ClaudeCode, Ollama, VLlm) always return `true`.
    pub fn is_available(&self) -> bool {
        match self.provider {
            // Local providers — no API key required.
            LlmProvider::ClaudeCode | LlmProvider::Ollama | LlmProvider::VLlm => true,
            // Bedrock uses AWS credentials from the environment, not api_key.
            LlmProvider::Bedrock => true,
            _ => !self.api_key.is_empty(),
        }
    }

    /// Validate the configuration and return a list of issues (empty = valid).
    ///
    /// Does not panic — callers can log warnings or refuse to start depending on
    /// the severity of the issues.
    pub fn validate_config(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !self.is_available() {
            issues.push(format!(
                "Provider {:?} for model '{}' has no API key configured",
                self.provider, self.model_id,
            ));
        }
        if self.max_tokens == 0 {
            issues.push("max_tokens is 0".to_string());
        }
        if self.max_turns == 0 {
            issues.push("max_turns is 0".to_string());
        }
        for (i, fb) in self.fallback_models.iter().enumerate() {
            for issue in fb.validate_config() {
                issues.push(format!("fallback[{i}]: {issue}"));
            }
        }
        issues
    }

    /// Return the provider's API base URL (custom or default).
    pub fn base_url(&self) -> &str {
        if let Some(url) = &self.api_base_url {
            url
        } else {
            match self.provider {
                LlmProvider::Claude => "https://api.anthropic.com",
                LlmProvider::OpenAi => "https://api.openai.com",
                LlmProvider::OpenRouter => "https://openrouter.ai/api",
                LlmProvider::Groq => "https://api.groq.com/openai",
                LlmProvider::ClaudeCode => "local://claude-cli",
                LlmProvider::Gemini => "https://generativelanguage.googleapis.com",
                LlmProvider::Ollama => &OLLAMA_HOST,
                LlmProvider::Mistral => "https://api.mistral.ai",
                LlmProvider::XAi => "https://api.x.ai",
                LlmProvider::AzureOpenAi => "https://models.inference.ai.azure.com",
                LlmProvider::Cerebras => "https://api.cerebras.ai",
                LlmProvider::Together => "https://api.together.xyz",
                LlmProvider::DeepSeek => "https://api.deepseek.com",
                LlmProvider::VLlm => &VLLM_HOST,
                LlmProvider::Fireworks => "https://api.fireworks.ai/inference",
                LlmProvider::HuggingFace => "https://api-inference.huggingface.co",
                LlmProvider::Cohere => "https://api.cohere.com",
                LlmProvider::Bedrock => "https://bedrock-runtime.us-east-1.amazonaws.com",
                LlmProvider::Replicate => "https://api.replicate.com",
            }
        }
    }
}

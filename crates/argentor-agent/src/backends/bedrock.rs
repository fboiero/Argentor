use super::LlmBackend;
use crate::config::ModelConfig;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message};
use argentor_skills::SkillDescriptor;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// AWS Bedrock backend (stub).
///
/// Bedrock uses SigV4-signed requests against service-specific endpoints
/// (`bedrock-runtime.{region}.amazonaws.com`). SigV4 is significantly heavier
/// than a simple Bearer-token HTTP call: request canonicalization, HMAC key
/// derivation, and credential providers (env, IMDS, SSO, STS) are all required
/// for a production-quality integration.
///
/// Rather than implement SigV4 by hand, the production path should sit behind
/// an `aws-bedrock` feature flag that enables the [`aws-sdk-bedrock`] crate and
/// delegates to the official AWS Rust SDK.
///
/// This stub exists so the provider is routable, testable, and discoverable in
/// the `LlmProvider` enum while signalling a clear, actionable error message
/// when the stub is invoked at runtime.
///
/// [`aws-sdk-bedrock`]: https://docs.rs/aws-sdk-bedrock
pub struct BedrockBackend {
    config: ModelConfig,
}

impl BedrockBackend {
    /// Create a new Bedrock backend with the given configuration.
    ///
    /// The `config.api_key` field is ignored — Bedrock uses AWS credentials
    /// from the standard AWS credential chain. The region is derived from the
    /// configured `api_base_url` or defaults to `us-east-1`.
    pub fn new(config: ModelConfig) -> Self {
        Self { config }
    }

    /// Return the AWS region encoded in the Bedrock endpoint.
    ///
    /// Parses `bedrock-runtime.{region}.amazonaws.com` from the configured
    /// base URL. Falls back to `us-east-1` when no region is encoded.
    pub fn region(&self) -> String {
        let base = self.config.base_url();
        if let Some(after) = base.strip_prefix("https://bedrock-runtime.") {
            if let Some(region) = after.split('.').next() {
                if !region.is_empty() {
                    return region.to_string();
                }
            }
        }
        "us-east-1".to_string()
    }

    /// Return the fully-qualified Bedrock `InvokeModel` URL for the configured model.
    pub fn invoke_url(&self) -> String {
        format!(
            "{}/model/{}/invoke",
            self.config.base_url(),
            self.config.model_id,
        )
    }

    /// Build a placeholder request body that mirrors the shape of an
    /// Anthropic-on-Bedrock `InvokeModel` request.
    ///
    /// Real integration must be provided by the AWS SDK — this helper exists
    /// for structural tests only.
    pub fn build_request_body(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> serde_json::Value {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != argentor_core::Role::System)
            .map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        argentor_core::Role::User | argentor_core::Role::Tool => "user",
                        argentor_core::Role::Assistant => "assistant",
                        argentor_core::Role::System => unreachable!(),
                    },
                    "content": m.content,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": self.config.max_tokens,
            "messages": api_messages,
        });
        if let Some(sys) = system_prompt {
            body["system"] = serde_json::Value::String(sys.to_string());
        }
        body
    }

    fn sdk_required_error() -> ArgentorError {
        ArgentorError::Config(
            "Bedrock requires AWS SDK integration. Use the `aws-sdk-bedrock` crate \
             behind the `aws-bedrock` feature flag for production use."
                .into(),
        )
    }
}

#[async_trait]
impl LlmBackend for BedrockBackend {
    fn provider_name(&self) -> &str {
        "bedrock"
    }

    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        Err(Self::sdk_required_error())
    }

    async fn chat_stream(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        Err(Self::sdk_required_error())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::LlmProvider;
    use argentor_core::Role;
    use uuid::Uuid;

    fn sample_config() -> ModelConfig {
        ModelConfig {
            provider: LlmProvider::Bedrock,
            model_id: "anthropic.claude-3-5-sonnet-20240620-v1:0".into(),
            api_key: String::new(),
            api_base_url: None,
            temperature: 0.7,
            max_tokens: 1024,
            max_turns: 10,
            fallback_models: vec![],
            retry_policy: None,
        }
    }

    fn user_msg(content: &str) -> Message {
        Message::new(Role::User, content, Uuid::new_v4())
    }

    fn assistant_msg(content: &str) -> Message {
        Message::new(Role::Assistant, content, Uuid::new_v4())
    }

    #[test]
    fn constructor_stores_config() {
        let backend = BedrockBackend::new(sample_config());
        assert_eq!(
            backend.config.model_id,
            "anthropic.claude-3-5-sonnet-20240620-v1:0"
        );
    }

    #[test]
    fn provider_name_is_bedrock() {
        let backend = BedrockBackend::new(sample_config());
        assert_eq!(backend.provider_name(), "bedrock");
    }

    #[test]
    fn region_defaults_to_us_east_1_when_unset() {
        let backend = BedrockBackend::new(sample_config());
        assert_eq!(backend.region(), "us-east-1");
    }

    #[test]
    fn region_parses_from_regional_endpoint() {
        let mut cfg = sample_config();
        cfg.api_base_url = Some("https://bedrock-runtime.eu-west-1.amazonaws.com".into());
        let backend = BedrockBackend::new(cfg);
        assert_eq!(backend.region(), "eu-west-1");
    }

    #[test]
    fn invoke_url_formats_with_model_id() {
        let backend = BedrockBackend::new(sample_config());
        assert!(backend
            .invoke_url()
            .ends_with("/model/anthropic.claude-3-5-sonnet-20240620-v1:0/invoke"));
    }

    #[test]
    fn build_request_body_includes_anthropic_version() {
        let backend = BedrockBackend::new(sample_config());
        let body = backend.build_request_body(None, &[user_msg("Hi")], &[]);
        assert_eq!(body["anthropic_version"], "bedrock-2023-05-31");
        assert_eq!(body["max_tokens"], 1024);
    }

    #[test]
    fn build_request_body_adds_system_field_when_prompt_set() {
        let backend = BedrockBackend::new(sample_config());
        let body = backend.build_request_body(Some("Be brief."), &[user_msg("Hi")], &[]);
        assert_eq!(body["system"], "Be brief.");
    }

    #[test]
    fn build_request_body_maps_roles_correctly() {
        let backend = BedrockBackend::new(sample_config());
        let body = backend.build_request_body(None, &[user_msg("Hi"), assistant_msg("Hello")], &[]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
    }

    #[test]
    fn build_request_body_filters_system_role_messages() {
        let backend = BedrockBackend::new(sample_config());
        let sys = Message::new(Role::System, "ignored", Uuid::new_v4());
        let body = backend.build_request_body(None, &[sys, user_msg("Hi")], &[]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "Hi");
    }

    #[tokio::test]
    async fn stub_chat_returns_sdk_error() {
        let backend = BedrockBackend::new(sample_config());
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("bedrock stub must error");
        let msg = err.to_string();
        assert!(
            msg.contains("aws-sdk-bedrock") || msg.contains("AWS SDK"),
            "error should mention AWS SDK, got: {msg}"
        );
    }

    #[tokio::test]
    async fn stub_chat_stream_returns_sdk_error() {
        let backend = BedrockBackend::new(sample_config());
        let err = backend
            .chat_stream(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("bedrock stub stream must error");
        assert!(err.to_string().contains("AWS"));
    }

    #[test]
    fn bedrock_skips_api_key_requirement() {
        // Bedrock uses AWS credentials, not an API key. is_available() must
        // return true even with an empty api_key (config layer).
        let cfg = sample_config();
        assert!(cfg.api_key.is_empty());
        // This merely documents the intent — the enum guard lives in config.rs.
    }
}

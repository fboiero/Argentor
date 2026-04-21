use super::LlmBackend;
use crate::config::ModelConfig;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role};
use argentor_skills::SkillDescriptor;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Replicate API backend (stub).
///
/// Replicate's API is an async prediction pattern: you POST a request to create
/// a prediction, then poll (or listen to a webhook / SSE stream) until the
/// prediction transitions to `succeeded` or `failed`. The endpoint shape is:
///
/// ```text
/// POST {base_url}/v1/models/{owner}/{name}/predictions
/// Authorization: Token <REPLICATE_API_TOKEN>
/// ```
///
/// Polling happens at `GET {base_url}/v1/predictions/{prediction_id}` until
/// `status ∈ { "succeeded", "failed", "canceled" }`.
///
/// The `model_id` field is expected in `owner/name` form (e.g.
/// `meta/meta-llama-3-70b-instruct`). This stub exposes helpers to build the
/// prediction URL and the request body so the plumbing is testable, while
/// shipping a deterministic stub response for `chat()` / `chat_stream()`.
pub struct ReplicateBackend {
    config: ModelConfig,
}

impl ReplicateBackend {
    /// Create a new Replicate API backend with the given configuration.
    pub fn new(config: ModelConfig) -> Self {
        Self { config }
    }

    /// Build the `predictions` URL for the configured `owner/name` model.
    ///
    /// Returns a `Err(ArgentorError::Config)` when `model_id` does not contain
    /// the required `/` separator.
    pub fn predictions_url(&self) -> ArgentorResult<String> {
        let (owner, name) = self.split_model_id()?;
        Ok(format!(
            "{}/v1/models/{}/{}/predictions",
            self.config.base_url(),
            owner,
            name
        ))
    }

    /// Build the polling URL for a prediction id.
    pub fn prediction_status_url(&self, prediction_id: &str) -> String {
        format!(
            "{}/v1/predictions/{}",
            self.config.base_url(),
            prediction_id
        )
    }

    /// Build the `Authorization` header value (`Token <key>`).
    pub fn auth_header(&self) -> String {
        format!("Token {}", self.config.api_key)
    }

    /// Build the Replicate `input` payload. Replicate expects the conversation
    /// collapsed into a single `prompt` string (plus optional `system_prompt`).
    pub fn build_request_body(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> serde_json::Value {
        let prompt = Self::collapse_messages(messages);
        let mut input = serde_json::json!({
            "prompt": prompt,
            "max_new_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
        });
        if let Some(sys) = system_prompt {
            input["system_prompt"] = serde_json::Value::String(sys.to_string());
        }
        serde_json::json!({ "input": input })
    }

    /// Extract the concatenated text output from a `succeeded` Replicate
    /// prediction response. Replicate returns `output` as either a string, an
    /// array of strings, or `null`.
    pub fn parse_prediction_output(value: &serde_json::Value) -> String {
        match &value["output"] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }

    fn split_model_id(&self) -> ArgentorResult<(String, String)> {
        let (owner, name) = self.config.model_id.split_once('/').ok_or_else(|| {
            ArgentorError::Config(format!(
                "Replicate model_id must be in 'owner/name' form, got '{}'",
                self.config.model_id
            ))
        })?;
        if owner.is_empty() || name.is_empty() {
            return Err(ArgentorError::Config(format!(
                "Replicate model_id owner and name must both be non-empty: '{}'",
                self.config.model_id
            )));
        }
        Ok((owner.to_string(), name.to_string()))
    }

    fn collapse_messages(messages: &[Message]) -> String {
        messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| {
                let role = match m.role {
                    Role::User | Role::Tool => "User",
                    Role::Assistant => "Assistant",
                    Role::System => unreachable!(),
                };
                format!("{role}: {}", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn ensure_ready(&self) -> ArgentorResult<()> {
        if self.config.api_key.is_empty() {
            return Err(ArgentorError::Config(
                "Replicate provider requires a non-empty api_key".into(),
            ));
        }
        // Validate model id eagerly so stub callers get the same error shape.
        self.split_model_id().map(|_| ())
    }
}

#[async_trait]
impl LlmBackend for ReplicateBackend {
    fn provider_name(&self) -> &str {
        "replicate"
    }

    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        self.ensure_ready()?;
        Ok(LlmResponse::Done("[replicate-stub] response".into()))
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
        self.ensure_ready()?;
        let (tx, rx) = mpsc::channel::<StreamEvent>(8);
        let handle = tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::TextDelta {
                    text: "[replicate-stub] response".into(),
                })
                .await;
            let _ = tx.send(StreamEvent::Done).await;
            Ok(LlmResponse::Done("[replicate-stub] response".into()))
        });
        Ok((rx, handle))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::LlmProvider;
    use argentor_core::Role;
    use uuid::Uuid;

    fn sample_config(api_key: &str, model_id: &str) -> ModelConfig {
        ModelConfig {
            provider: LlmProvider::Replicate,
            model_id: model_id.into(),
            api_key: api_key.into(),
            api_base_url: None,
            temperature: 0.6,
            max_tokens: 512,
            max_turns: 8,
            max_context_tokens: 200_000,
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
        let backend =
            ReplicateBackend::new(sample_config("r8_key", "meta/meta-llama-3-70b-instruct"));
        assert_eq!(backend.config.model_id, "meta/meta-llama-3-70b-instruct");
    }

    #[test]
    fn provider_name_is_replicate() {
        let backend = ReplicateBackend::new(sample_config("k", "meta/llama"));
        assert_eq!(backend.provider_name(), "replicate");
    }

    #[test]
    fn auth_header_uses_token_scheme() {
        let backend = ReplicateBackend::new(sample_config("r8_abc", "meta/llama"));
        assert_eq!(backend.auth_header(), "Token r8_abc");
    }

    #[test]
    fn predictions_url_uses_owner_name_path() {
        let backend =
            ReplicateBackend::new(sample_config("r8_key", "meta/meta-llama-3-70b-instruct"));
        assert_eq!(
            backend.predictions_url().unwrap(),
            "https://api.replicate.com/v1/models/meta/meta-llama-3-70b-instruct/predictions"
        );
    }

    #[test]
    fn predictions_url_rejects_model_id_without_slash() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "llama-no-owner"));
        let err = backend.predictions_url().expect_err("should reject");
        assert!(err.to_string().contains("owner/name"));
    }

    #[test]
    fn prediction_status_url_uses_prediction_id() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        assert_eq!(
            backend.prediction_status_url("abc123"),
            "https://api.replicate.com/v1/predictions/abc123"
        );
    }

    #[test]
    fn build_request_body_wraps_payload_in_input_field() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let body = backend.build_request_body(None, &[user_msg("Hi")], &[]);
        let input = &body["input"];
        assert!(input.is_object(), "expected input object");
        assert!(input["prompt"].is_string());
        assert_eq!(input["max_new_tokens"], 512);
        let temp = input["temperature"].as_f64().unwrap();
        assert!((temp - 0.6).abs() < 1e-4, "expected ~0.6, got {temp}");
    }

    #[test]
    fn build_request_body_includes_system_prompt_when_set() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let body = backend.build_request_body(Some("Be crisp."), &[user_msg("Hola")], &[]);
        assert_eq!(body["input"]["system_prompt"], "Be crisp.");
    }

    #[test]
    fn build_request_body_collapses_messages_with_role_prefix() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let body = backend.build_request_body(
            None,
            &[user_msg("Hola"), assistant_msg("¿Cómo estás?")],
            &[],
        );
        let prompt = body["input"]["prompt"].as_str().unwrap();
        assert!(prompt.contains("User: Hola"));
        assert!(prompt.contains("Assistant: ¿Cómo estás?"));
    }

    #[test]
    fn parse_prediction_output_handles_string_output() {
        let resp = serde_json::json!({ "output": "hello world" });
        assert_eq!(
            ReplicateBackend::parse_prediction_output(&resp),
            "hello world"
        );
    }

    #[test]
    fn parse_prediction_output_joins_string_array_output() {
        let resp = serde_json::json!({ "output": ["hello", " ", "world"] });
        assert_eq!(
            ReplicateBackend::parse_prediction_output(&resp),
            "hello world"
        );
    }

    #[test]
    fn parse_prediction_output_returns_empty_for_null() {
        let resp = serde_json::json!({ "output": null });
        assert_eq!(ReplicateBackend::parse_prediction_output(&resp), "");
    }

    #[tokio::test]
    async fn stub_chat_returns_done_response() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let resp = backend.chat(None, &[user_msg("Hi")], &[]).await.unwrap();
        match resp {
            LlmResponse::Done(text) => assert!(text.contains("[replicate-stub]")),
            other => panic!("Expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chat_fails_without_api_key() {
        let backend = ReplicateBackend::new(sample_config("", "meta/llama"));
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("should require token");
        assert!(err.to_string().to_lowercase().contains("api_key"));
    }

    #[tokio::test]
    async fn chat_fails_with_invalid_model_id() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "no-slash"));
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("should reject bad model id");
        assert!(err.to_string().contains("owner/name"));
    }

    #[tokio::test]
    async fn stub_chat_stream_emits_events() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let (mut rx, handle) = backend
            .chat_stream(None, &[user_msg("Hi")], &[])
            .await
            .unwrap();

        let mut saw_done = false;
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::Done) {
                saw_done = true;
                break;
            }
        }
        assert!(saw_done);
        assert!(matches!(
            handle.await.unwrap().unwrap(),
            LlmResponse::Done(_)
        ));
    }
}

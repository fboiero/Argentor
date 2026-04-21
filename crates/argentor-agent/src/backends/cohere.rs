use super::LlmBackend;
use crate::config::ModelConfig;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role};
use argentor_skills::SkillDescriptor;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Cohere API backend (stub).
///
/// Implements the Cohere `v2/chat` REST API at `https://api.cohere.com/v2/chat`.
/// Auth header: `Authorization: bearer <API_KEY>`.
///
/// Request format (non-OpenAI):
/// ```json
/// {
///   "model": "command-r-08-2024",
///   "messages": [
///     { "role": "system", "content": "..." },
///     { "role": "user", "content": "..." }
///   ],
///   "temperature": 0.7,
///   "max_tokens": 4096
/// }
/// ```
///
/// NOTE: this backend ships as a stub — real HTTP integration lives behind a
/// `cohere-http` feature flag that is not enabled yet. Stub responses are
/// deterministic for tests and make routing/plumbing verifiable.
pub struct CohereBackend {
    config: ModelConfig,
}

impl CohereBackend {
    /// Create a new Cohere API backend with the given configuration.
    pub fn new(config: ModelConfig) -> Self {
        Self { config }
    }

    /// Build the Cohere request body from the Argentor message shape.
    ///
    /// Exposed for tests — real HTTP sending is out of scope for the stub.
    pub fn build_request_body(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> serde_json::Value {
        let mut api_messages: Vec<serde_json::Value> = Vec::new();

        if let Some(sys) = system_prompt {
            api_messages.push(serde_json::json!({
                "role": "system",
                "content": sys,
            }));
        }

        for m in messages {
            if m.role == Role::System {
                continue;
            }
            api_messages.push(serde_json::json!({
                "role": match m.role {
                    Role::User | Role::Tool => "user",
                    Role::Assistant => "assistant",
                    Role::System => unreachable!(),
                },
                "content": m.content,
            }));
        }

        let mut body = serde_json::json!({
            "model": self.config.model_id,
            "messages": api_messages,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
        });

        if !tools.is_empty() {
            let tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters_schema,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        body
    }

    /// Return the default Cohere chat endpoint: `{base_url}/v2/chat`.
    pub fn chat_url(&self) -> String {
        format!("{}/v2/chat", self.config.base_url())
    }

    /// Build the Authorization header value (`bearer <key>`).
    pub fn auth_header(&self) -> String {
        format!("bearer {}", self.config.api_key)
    }

    fn ensure_api_key(&self) -> ArgentorResult<()> {
        if self.config.api_key.is_empty() {
            return Err(ArgentorError::Config(
                "Cohere provider requires a non-empty api_key".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl LlmBackend for CohereBackend {
    fn provider_name(&self) -> &str {
        "cohere"
    }

    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        self.ensure_api_key()?;
        Ok(LlmResponse::Done("[cohere-stub] response".into()))
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
        self.ensure_api_key()?;
        let (tx, rx) = mpsc::channel::<StreamEvent>(8);
        let handle = tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::TextDelta {
                    text: "[cohere-stub] response".into(),
                })
                .await;
            let _ = tx.send(StreamEvent::Done).await;
            Ok(LlmResponse::Done("[cohere-stub] response".into()))
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

    fn sample_config(api_key: &str) -> ModelConfig {
        ModelConfig {
            provider: LlmProvider::Cohere,
            model_id: "command-r-08-2024".into(),
            api_key: api_key.into(),
            api_base_url: None,
            temperature: 0.5,
            max_tokens: 256,
            max_turns: 5,
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

    fn sample_tool() -> SkillDescriptor {
        SkillDescriptor {
            name: "get_weather".into(),
            description: "Fetch weather".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                }
            }),
            required_capabilities: vec![],
        }
    }

    #[test]
    fn constructor_stores_config() {
        let backend = CohereBackend::new(sample_config("key-1"));
        assert_eq!(backend.config.model_id, "command-r-08-2024");
    }

    #[test]
    fn provider_name_is_cohere() {
        let backend = CohereBackend::new(sample_config("key-1"));
        assert_eq!(backend.provider_name(), "cohere");
    }

    #[test]
    fn default_chat_url_uses_v2_chat() {
        let backend = CohereBackend::new(sample_config("key-1"));
        assert_eq!(backend.chat_url(), "https://api.cohere.com/v2/chat");
    }

    #[test]
    fn custom_base_url_is_honored() {
        let mut cfg = sample_config("key-1");
        cfg.api_base_url = Some("https://example.test".into());
        let backend = CohereBackend::new(cfg);
        assert_eq!(backend.chat_url(), "https://example.test/v2/chat");
    }

    #[test]
    fn auth_header_uses_lowercase_bearer() {
        let backend = CohereBackend::new(sample_config("secret-key"));
        assert_eq!(backend.auth_header(), "bearer secret-key");
    }

    #[test]
    fn build_request_body_includes_model_and_params() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let body = backend.build_request_body(None, &[user_msg("Hi")], &[]);
        assert_eq!(body["model"], "command-r-08-2024");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 256);
    }

    #[test]
    fn build_request_body_prepends_system_prompt_as_message() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let body = backend.build_request_body(Some("Be concise."), &[user_msg("Hola")], &[]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "Be concise.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hola");
    }

    #[test]
    fn build_request_body_maps_roles_correctly() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let body = backend.build_request_body(None, &[user_msg("Hi"), assistant_msg("Hello")], &[]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
    }

    #[test]
    fn build_request_body_filters_system_role_from_messages() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let sys = Message::new(Role::System, "ignored", Uuid::new_v4());
        let body = backend.build_request_body(None, &[sys, user_msg("Hi")], &[]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "Hi");
    }

    #[test]
    fn build_request_body_includes_tools_when_provided() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let body = backend.build_request_body(None, &[user_msg("Hi")], &[sample_tool()]);
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn build_request_body_omits_tools_when_empty() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let body = backend.build_request_body(None, &[user_msg("Hi")], &[]);
        assert!(body.get("tools").is_none());
    }

    #[tokio::test]
    async fn stub_chat_returns_done_response() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let resp = backend.chat(None, &[user_msg("Hi")], &[]).await.unwrap();
        match resp {
            LlmResponse::Done(text) => assert!(text.contains("[cohere-stub]")),
            other => panic!("Expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chat_fails_without_api_key() {
        let backend = CohereBackend::new(sample_config(""));
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("should require api key");
        assert!(err.to_string().to_lowercase().contains("api_key"));
    }

    #[tokio::test]
    async fn stub_chat_stream_emits_done_and_text_events() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let (mut rx, handle) = backend
            .chat_stream(None, &[user_msg("Hi")], &[])
            .await
            .unwrap();

        let mut saw_text = false;
        let mut saw_done = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::TextDelta { text } => {
                    saw_text = saw_text || text.contains("[cohere-stub]");
                }
                StreamEvent::Done => {
                    saw_done = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_text, "expected a text delta from the stub");
        assert!(saw_done, "expected a Done event from the stub");

        let final_resp = handle.await.unwrap().unwrap();
        assert!(matches!(final_resp, LlmResponse::Done(_)));
    }
}

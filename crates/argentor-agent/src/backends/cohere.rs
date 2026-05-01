// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2024 Argentor contributors
//
// Cohere v2 chat backend.
//
// Without the `cohere` feature flag this module compiles to a stub that
// returns a clear error.  Enable the real implementation with:
//
//   cargo build --features cohere
//
// API reference: https://docs.cohere.com/reference/chat
// Endpoint: POST https://api.cohere.com/v2/chat
// Auth: Authorization: bearer <API_KEY>

use super::LlmBackend;
use crate::config::ModelConfig;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role};
use argentor_skills::SkillDescriptor;
#[cfg(not(feature = "cohere"))]
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Cohere API backend.
///
/// Implements the Cohere `v2/chat` REST API at `https://api.cohere.com/v2/chat`.
/// Auth header: `Authorization: bearer <API_KEY>`.
///
/// Request format:
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
/// Enable real HTTP with `--features cohere`. Without that flag, the backend
/// is a lightweight stub that returns a clear, actionable error message.
pub struct CohereBackend {
    config: ModelConfig,
    #[cfg(feature = "cohere")]
    http: reqwest::Client,
}

impl CohereBackend {
    /// Create a new Cohere API backend with the given configuration.
    pub fn new(config: ModelConfig) -> Self {
        Self {
            #[cfg(feature = "cohere")]
            http: reqwest::Client::new(),
            config,
        }
    }

    /// Build the Cohere request body from the Argentor message shape.
    ///
    /// Exposed for tests — always compiled regardless of feature flag.
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

// ---------------------------------------------------------------------------
// Real implementation (feature = "cohere")
// Uses reqwest with SSE streaming support.
// ---------------------------------------------------------------------------

#[cfg(feature = "cohere")]
mod real {
    use super::*;
    use futures_util::StreamExt;

    /// Parse a Cohere v2 non-streaming chat response into an `LlmResponse`.
    pub fn parse_cohere_response(body: &serde_json::Value) -> ArgentorResult<LlmResponse> {
        // Cohere v2: { "message": { "content": [{ "type": "text", "text": "..." }] } }
        let content_arr = body
            .pointer("/message/content")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ArgentorError::Agent(format!("Unexpected Cohere response shape: {}", body))
            })?;

        let text: String = content_arr
            .iter()
            .filter_map(|block| {
                if block["type"].as_str() == Some("text") {
                    block["text"].as_str().map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        let finish_reason = body
            .pointer("/finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("COMPLETE");

        if finish_reason == "COMPLETE" || finish_reason == "MAX_TOKENS" {
            Ok(LlmResponse::Done(text))
        } else {
            Ok(LlmResponse::Text(text))
        }
    }

    #[async_trait::async_trait]
    impl LlmBackend for super::CohereBackend {
        fn provider_name(&self) -> &str {
            "cohere"
        }

        async fn chat(
            &self,
            system_prompt: Option<&str>,
            messages: &[Message],
            tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            self.ensure_api_key()?;

            let url = self.chat_url();
            let body = self.build_request_body(system_prompt, messages, tools);

            let resp = self
                .http
                .post(&url)
                .header("Authorization", self.auth_header())
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            let status = resp.status();
            let resp_body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            if !status.is_success() {
                return Err(ArgentorError::Http(format!(
                    "Cohere API error {status}: {resp_body}"
                )));
            }

            parse_cohere_response(&resp_body)
        }

        async fn chat_stream(
            &self,
            system_prompt: Option<&str>,
            messages: &[Message],
            tools: &[SkillDescriptor],
        ) -> ArgentorResult<(
            mpsc::Receiver<StreamEvent>,
            JoinHandle<ArgentorResult<LlmResponse>>,
        )> {
            self.ensure_api_key()?;

            let url = self.chat_url();
            let mut body = self.build_request_body(system_prompt, messages, tools);
            body["stream"] = serde_json::json!(true);

            let resp = self
                .http
                .post(&url)
                .header("Authorization", self.auth_header())
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                return Err(ArgentorError::Http(format!(
                    "Cohere API error {status}: {error_body}"
                )));
            }

            let (tx, rx) = mpsc::channel::<StreamEvent>(256);
            let byte_stream = resp.bytes_stream();

            let handle = tokio::spawn(async move {
                let mut stream = byte_stream;
                let mut buffer = String::new();
                let mut full_text = String::new();
                let mut finish_reason = String::from("COMPLETE");

                while let Some(chunk_result) = stream.next().await {
                    let chunk = match chunk_result {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            let _ = tx
                                .send(StreamEvent::Error {
                                    message: format!("Stream read error: {e}"),
                                })
                                .await;
                            return Err(ArgentorError::Http(format!("Stream read error: {e}")));
                        }
                    };

                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer = buffer[line_end + 1..].to_string();

                        if line.is_empty() || line.starts_with(':') {
                            continue;
                        }

                        // Cohere SSE: "data: <json>"
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                let _ = tx.send(StreamEvent::Done).await;
                                continue;
                            }

                            let event: serde_json::Value = match serde_json::from_str(data) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };

                            let event_type = event["type"].as_str().unwrap_or("");

                            match event_type {
                                // Cohere v2 streaming event types
                                "content-delta" => {
                                    if let Some(text) = event
                                        .pointer("/delta/message/content/text")
                                        .and_then(|v| v.as_str())
                                    {
                                        full_text.push_str(text);
                                        let _ = tx
                                            .send(StreamEvent::TextDelta {
                                                text: text.to_string(),
                                            })
                                            .await;
                                    }
                                }
                                "message-end" => {
                                    if let Some(fr) = event["delta"]["finish_reason"].as_str() {
                                        finish_reason = fr.to_string();
                                    }
                                    let _ = tx.send(StreamEvent::Done).await;
                                }
                                _ => {}
                            }
                        }
                    }
                }

                if finish_reason == "COMPLETE" || finish_reason == "MAX_TOKENS" {
                    Ok(LlmResponse::Done(full_text))
                } else {
                    Ok(LlmResponse::Text(full_text))
                }
            });

            Ok((rx, handle))
        }
    }
}

// ---------------------------------------------------------------------------
// Stub implementation (no feature = "cohere")
// ---------------------------------------------------------------------------

#[cfg(not(feature = "cohere"))]
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
        Err(ArgentorError::Config(
            "Cohere backend requires the `cohere` feature flag. \
             Recompile with `--features cohere`."
                .into(),
        ))
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
        Err(ArgentorError::Config(
            "Cohere backend requires the `cohere` feature flag. \
             Recompile with `--features cohere`."
                .into(),
        ))
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
            requires_approval: false,
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
    async fn chat_fails_without_api_key() {
        let backend = CohereBackend::new(sample_config(""));
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("should require api key");
        assert!(err.to_string().to_lowercase().contains("api_key"));
    }

    #[cfg(not(feature = "cohere"))]
    #[tokio::test]
    async fn stub_chat_returns_feature_flag_error() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("stub should return error without cohere feature");
        assert!(
            err.to_string().contains("cohere"),
            "error should mention the feature flag"
        );
    }

    #[cfg(not(feature = "cohere"))]
    #[tokio::test]
    async fn stub_chat_stream_returns_feature_flag_error() {
        let backend = CohereBackend::new(sample_config("key-1"));
        let err = backend
            .chat_stream(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("stub should return error without cohere feature");
        assert!(
            err.to_string().contains("cohere"),
            "error should mention the feature flag"
        );
    }

    /// Integration test — requires a real COHERE_API_KEY in the environment.
    #[cfg(feature = "cohere")]
    #[tokio::test]
    #[ignore = "requires real COHERE_API_KEY"]
    async fn integration_chat_real_api() {
        let api_key = std::env::var("COHERE_API_KEY").expect("COHERE_API_KEY must be set");
        let mut cfg = sample_config(&api_key);
        cfg.model_id = "command-r-plus".into();
        let backend = CohereBackend::new(cfg);
        let resp = backend
            .chat(None, &[user_msg("Say hello in one word.")], &[])
            .await
            .expect("real API call should succeed");
        match resp {
            LlmResponse::Done(text) | LlmResponse::Text(text) => {
                assert!(!text.is_empty(), "response text must not be empty");
            }
            other => panic!("Unexpected response: {other:?}"),
        }
    }

    /// Integration test — requires a real COHERE_API_KEY in the environment.
    #[cfg(feature = "cohere")]
    #[tokio::test]
    #[ignore = "requires real COHERE_API_KEY"]
    async fn integration_chat_stream_real_api() {
        use futures_util::StreamExt as _;
        let api_key = std::env::var("COHERE_API_KEY").expect("COHERE_API_KEY must be set");
        let mut cfg = sample_config(&api_key);
        cfg.model_id = "command-r-plus".into();
        let backend = CohereBackend::new(cfg);
        let (mut rx, handle) = backend
            .chat_stream(None, &[user_msg("Say hello in one word.")], &[])
            .await
            .expect("stream should start");

        let mut saw_text = false;
        let mut saw_done = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::TextDelta { text } if !text.is_empty() => saw_text = true,
                StreamEvent::Done => {
                    saw_done = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_text, "expected at least one text delta");
        assert!(saw_done, "expected Done event");
        let final_resp = handle.await.unwrap().expect("join handle should succeed");
        assert!(matches!(
            final_resp,
            LlmResponse::Done(_) | LlmResponse::Text(_)
        ));
    }

    /// Unit test for response parsing logic (feature-gated, no HTTP).
    #[cfg(feature = "cohere")]
    #[test]
    fn parse_cohere_response_extracts_text() {
        let body = serde_json::json!({
            "message": {
                "content": [
                    { "type": "text", "text": "Hello, world!" }
                ]
            },
            "finish_reason": "COMPLETE"
        });
        let resp = real::parse_cohere_response(&body).unwrap();
        match resp {
            LlmResponse::Done(text) => assert_eq!(text, "Hello, world!"),
            other => panic!("Expected Done, got {other:?}"),
        }
    }

    #[cfg(feature = "cohere")]
    #[test]
    fn parse_cohere_response_joins_multiple_text_blocks() {
        let body = serde_json::json!({
            "message": {
                "content": [
                    { "type": "text", "text": "Hello" },
                    { "type": "text", "text": " world" }
                ]
            },
            "finish_reason": "COMPLETE"
        });
        let resp = real::parse_cohere_response(&body).unwrap();
        match resp {
            LlmResponse::Done(text) => assert_eq!(text, "Hello world"),
            other => panic!("Expected Done, got {other:?}"),
        }
    }

    #[cfg(feature = "cohere")]
    #[test]
    fn parse_cohere_response_returns_error_on_bad_shape() {
        let body = serde_json::json!({ "unexpected": "shape" });
        assert!(real::parse_cohere_response(&body).is_err());
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2024 Argentor contributors
//
// Replicate async-prediction backend.
//
// Without the `replicate` feature flag this module compiles to a stub that
// returns a clear error.  Enable the real implementation with:
//
//   cargo build --features replicate
//
// API reference: https://replicate.com/docs/reference/http
// Endpoint: POST https://api.replicate.com/v1/models/{owner}/{name}/predictions
// Auth: Authorization: Token <REPLICATE_API_TOKEN>
//
// Replicate's API is async: POST creates a prediction, then poll GET until
// status is "succeeded" or "failed".  Exponential backoff is used for polling.

use super::LlmBackend;
use crate::config::ModelConfig;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role};
use argentor_skills::SkillDescriptor;
#[cfg(not(feature = "replicate"))]
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Replicate API backend.
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
/// `meta/meta-llama-3-70b-instruct`).
///
/// Enable real HTTP with `--features replicate`. Without that flag, the backend
/// is a lightweight stub that returns a clear, actionable error message.
pub struct ReplicateBackend {
    config: ModelConfig,
    #[cfg(feature = "replicate")]
    http: reqwest::Client,
}

impl ReplicateBackend {
    /// Create a new Replicate API backend with the given configuration.
    pub fn new(config: ModelConfig) -> Self {
        Self {
            #[cfg(feature = "replicate")]
            http: reqwest::Client::new(),
            config,
        }
    }

    /// Build the `predictions` URL for the configured `owner/name` model.
    ///
    /// Returns an `Err(ArgentorError::Config)` when `model_id` does not contain
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
        // Validate model id eagerly so callers get the same error shape.
        self.split_model_id().map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Real implementation (feature = "replicate")
// Uses reqwest with exponential-backoff polling.
// ---------------------------------------------------------------------------

#[cfg(feature = "replicate")]
mod real {
    use super::*;
    use std::time::Duration;

    const MAX_POLL_ATTEMPTS: u32 = 30;
    const INITIAL_BACKOFF_MS: u64 = 500;
    const MAX_BACKOFF_MS: u64 = 10_000;

    /// Poll a Replicate prediction until it reaches a terminal state.
    ///
    /// Returns the final prediction JSON on success, or an error if the
    /// prediction failed/was canceled, or the poll limit was exhausted.
    pub async fn poll_prediction(
        http: &reqwest::Client,
        status_url: &str,
        auth_header: &str,
    ) -> ArgentorResult<serde_json::Value> {
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        for attempt in 0..MAX_POLL_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }

            let resp = http
                .get(status_url)
                .header("Authorization", auth_header)
                .send()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            let status_code = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            if !status_code.is_success() {
                return Err(ArgentorError::Http(format!(
                    "Replicate poll error {status_code}: {body}"
                )));
            }

            let prediction_status = body["status"].as_str().unwrap_or("unknown");
            match prediction_status {
                "succeeded" => return Ok(body),
                "failed" | "canceled" => {
                    let error_msg = body["error"]
                        .as_str()
                        .unwrap_or(prediction_status)
                        .to_string();
                    return Err(ArgentorError::Http(format!(
                        "Replicate prediction {prediction_status}: {error_msg}"
                    )));
                }
                // "starting" | "processing" — keep polling
                _ => {}
            }
        }

        Err(ArgentorError::Http(format!(
            "Replicate prediction did not complete after {MAX_POLL_ATTEMPTS} poll attempts"
        )))
    }

    #[async_trait::async_trait]
    impl LlmBackend for super::ReplicateBackend {
        fn provider_name(&self) -> &str {
            "replicate"
        }

        async fn chat(
            &self,
            system_prompt: Option<&str>,
            messages: &[Message],
            tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            self.ensure_ready()?;

            let predictions_url = self.predictions_url()?;
            let auth = self.auth_header();
            let body = self.build_request_body(system_prompt, messages, tools);

            // 1. Create prediction
            let create_resp = self
                .http
                .post(&predictions_url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            let create_status = create_resp.status();
            let create_body: serde_json::Value = create_resp
                .json()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            if !create_status.is_success() {
                return Err(ArgentorError::Http(format!(
                    "Replicate create prediction error {create_status}: {create_body}"
                )));
            }

            let prediction_id = create_body["id"].as_str().ok_or_else(|| {
                ArgentorError::Http("Replicate: no prediction id in response".into())
            })?;

            // 2. Poll until done
            let status_url = self.prediction_status_url(prediction_id);
            let final_body = poll_prediction(&self.http, &status_url, &auth).await?;

            let text = Self::parse_prediction_output(&final_body);
            Ok(LlmResponse::Done(text))
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
            self.ensure_ready()?;

            let predictions_url = self.predictions_url()?;
            let auth = self.auth_header();
            let mut body = self.build_request_body(system_prompt, messages, tools);
            // Request Replicate SSE streaming when supported
            body["stream"] = serde_json::json!(true);

            // 1. Create prediction (with stream=true header)
            let create_resp = self
                .http
                .post(&predictions_url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .header("Prefer", "wait")
                .json(&body)
                .send()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            let create_status = create_resp.status();
            let create_body: serde_json::Value = create_resp
                .json()
                .await
                .map_err(|e| ArgentorError::Http(e.to_string()))?;

            if !create_status.is_success() {
                return Err(ArgentorError::Http(format!(
                    "Replicate create prediction error {create_status}: {create_body}"
                )));
            }

            let prediction_id = create_body["id"]
                .as_str()
                .ok_or_else(|| {
                    ArgentorError::Http("Replicate: no prediction id in response".into())
                })?
                .to_string();

            let status_url = self.prediction_status_url(&prediction_id);
            let http = self.http.clone();

            let (tx, rx) = mpsc::channel::<StreamEvent>(256);

            let handle = tokio::spawn(async move {
                // Poll until done, emitting simulated token events from each partial output.
                let final_body = match poll_prediction(&http, &status_url, &auth).await {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx
                            .send(StreamEvent::Error {
                                message: e.to_string(),
                            })
                            .await;
                        return Err(e);
                    }
                };

                let text = ReplicateBackend::parse_prediction_output(&final_body);

                // Emit the full text as a single delta (polling doesn't give us
                // per-token granularity without a streaming URL).
                if !text.is_empty() {
                    let _ = tx.send(StreamEvent::TextDelta { text: text.clone() }).await;
                }
                let _ = tx.send(StreamEvent::Done).await;

                Ok(LlmResponse::Done(text))
            });

            Ok((rx, handle))
        }
    }
}

// ---------------------------------------------------------------------------
// Stub implementation (no feature = "replicate")
// ---------------------------------------------------------------------------

#[cfg(not(feature = "replicate"))]
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
        Err(ArgentorError::Config(
            "Replicate backend requires the `replicate` feature flag. \
             Recompile with `--features replicate`."
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
        self.ensure_ready()?;
        Err(ArgentorError::Config(
            "Replicate backend requires the `replicate` feature flag. \
             Recompile with `--features replicate`."
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

    #[cfg(not(feature = "replicate"))]
    #[tokio::test]
    async fn stub_chat_returns_feature_flag_error() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("stub should return error without replicate feature");
        assert!(
            err.to_string().contains("replicate"),
            "error should mention the feature flag"
        );
    }

    #[cfg(not(feature = "replicate"))]
    #[tokio::test]
    async fn stub_chat_stream_returns_feature_flag_error() {
        let backend = ReplicateBackend::new(sample_config("r8_key", "meta/llama"));
        let err = backend
            .chat_stream(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("stub should return error without replicate feature");
        assert!(
            err.to_string().contains("replicate"),
            "error should mention the feature flag"
        );
    }

    /// Integration test — requires a real REPLICATE_API_TOKEN in the environment.
    #[cfg(feature = "replicate")]
    #[tokio::test]
    #[ignore = "requires real REPLICATE_API_TOKEN"]
    async fn integration_chat_real_api() {
        let api_key =
            std::env::var("REPLICATE_API_TOKEN").expect("REPLICATE_API_TOKEN must be set");
        let backend =
            ReplicateBackend::new(sample_config(&api_key, "meta/meta-llama-3-70b-instruct"));
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

    /// Integration test — requires a real REPLICATE_API_TOKEN in the environment.
    #[cfg(feature = "replicate")]
    #[tokio::test]
    #[ignore = "requires real REPLICATE_API_TOKEN"]
    async fn integration_chat_stream_real_api() {
        let api_key =
            std::env::var("REPLICATE_API_TOKEN").expect("REPLICATE_API_TOKEN must be set");
        let backend =
            ReplicateBackend::new(sample_config(&api_key, "meta/meta-llama-3-70b-instruct"));
        let (mut rx, handle) = backend
            .chat_stream(None, &[user_msg("Say hello in one word.")], &[])
            .await
            .expect("stream should start");

        let mut saw_done = false;
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::Done) {
                saw_done = true;
                break;
            }
        }
        assert!(saw_done, "expected Done event");
        let final_resp = handle.await.unwrap().expect("join handle should succeed");
        assert!(matches!(
            final_resp,
            LlmResponse::Done(_) | LlmResponse::Text(_)
        ));
    }

    /// Unit test for poll_prediction with a mock server.
    #[cfg(feature = "replicate")]
    #[tokio::test]
    async fn poll_prediction_succeeds_on_succeeded_status() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/predictions/test-id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "test-id",
                "status": "succeeded",
                "output": ["Hello", " ", "world"]
            })))
            .mount(&mock_server)
            .await;

        let http = reqwest::Client::new();
        let url = format!("{}/v1/predictions/test-id", mock_server.uri());
        let result = real::poll_prediction(&http, &url, "Token test-key")
            .await
            .unwrap();
        assert_eq!(result["status"], "succeeded");
        assert_eq!(
            ReplicateBackend::parse_prediction_output(&result),
            "Hello world"
        );
    }

    #[cfg(feature = "replicate")]
    #[tokio::test]
    async fn poll_prediction_fails_on_failed_status() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/predictions/fail-id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "fail-id",
                "status": "failed",
                "error": "out of memory"
            })))
            .mount(&mock_server)
            .await;

        let http = reqwest::Client::new();
        let url = format!("{}/v1/predictions/fail-id", mock_server.uri());
        let err = real::poll_prediction(&http, &url, "Token test-key")
            .await
            .expect_err("should return error on failed status");
        assert!(err.to_string().contains("failed"));
    }
}

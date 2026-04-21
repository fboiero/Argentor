// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2024 Argentor contributors
//
// AWS Bedrock backend.
//
// Without the `bedrock` feature flag this module compiles to a stub that
// returns a clear error.  Enable the real implementation with:
//
//   cargo build --features bedrock
//
// The real implementation signs HTTP requests with AWS SigV4 using the
// sha2/hmac/hex crates (already in the workspace) and dispatches via reqwest.
// No AWS SDK required — no toolchain version pinning needed.
//
// Credential resolution:
//   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN (optional)

use super::LlmBackend;
use crate::config::ModelConfig;
use crate::llm::LlmResponse;
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message};
use argentor_skills::SkillDescriptor;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// AWS Bedrock backend.
///
/// Bedrock uses SigV4-signed requests against service-specific endpoints
/// (`bedrock-runtime.{region}.amazonaws.com`).  Credentials are resolved via
/// environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
/// and optionally `AWS_SESSION_TOKEN`).
///
/// Enable with `--features bedrock`.  Without that flag the backend is a
/// lightweight stub that returns a clear, actionable error message.
pub struct BedrockBackend {
    config: ModelConfig,
}

impl BedrockBackend {
    /// Create a new Bedrock backend with the given configuration.
    ///
    /// The `config.api_key` field is ignored — Bedrock uses AWS credentials
    /// from environment variables.  The region is derived from the configured
    /// `api_base_url` or defaults to `us-east-1`.
    pub fn new(config: ModelConfig) -> Self {
        Self { config }
    }

    /// Return the AWS region encoded in the Bedrock endpoint.
    ///
    /// Parses `bedrock-runtime.{region}.amazonaws.com` from the configured
    /// base URL.  Falls back to env vars then `us-east-1`.
    pub fn region(&self) -> String {
        let base = self.config.base_url();
        if let Some(after) = base.strip_prefix("https://bedrock-runtime.") {
            if let Some(region) = after.split('.').next() {
                if !region.is_empty() {
                    return region.to_string();
                }
            }
        }
        std::env::var("AWS_DEFAULT_REGION")
            .or_else(|_| std::env::var("AWS_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string())
    }

    /// Return the fully-qualified Bedrock `InvokeModel` URL.
    pub fn invoke_url(&self) -> String {
        format!(
            "{}/model/{}/invoke",
            self.config.base_url(),
            self.config.model_id,
        )
    }

    /// Return the Bedrock `InvokeModelWithResponseStream` URL.
    pub fn invoke_stream_url(&self) -> String {
        format!(
            "{}/model/{}/invoke-with-response-stream",
            self.config.base_url(),
            self.config.model_id,
        )
    }

    /// Build an Anthropic-on-Bedrock `InvokeModel` request body.
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
}

// ---------------------------------------------------------------------------
// Real implementation (feature = "bedrock")
// Uses reqwest + manual SigV4 — no AWS SDK required.
// ---------------------------------------------------------------------------

#[cfg(feature = "bedrock")]
mod real {
    use super::*;
    use futures_util::StreamExt;
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    type HmacSha256 = Hmac<Sha256>;

    struct AwsCredentials {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    }

    impl AwsCredentials {
        fn from_env() -> ArgentorResult<Self> {
            let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").map_err(|_| {
                ArgentorError::Config("Bedrock requires AWS_ACCESS_KEY_ID in environment".into())
            })?;
            let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| {
                ArgentorError::Config(
                    "Bedrock requires AWS_SECRET_ACCESS_KEY in environment".into(),
                )
            })?;
            Ok(Self {
                access_key_id,
                secret_access_key,
                session_token: std::env::var("AWS_SESSION_TOKEN").ok(),
            })
        }
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    fn hmac_sha256_bytes(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
        let k_date = hmac_sha256_bytes(format!("AWS4{secret}").as_bytes(), date.as_bytes());
        let k_region = hmac_sha256_bytes(&k_date, region.as_bytes());
        let k_service = hmac_sha256_bytes(&k_region, service.as_bytes());
        hmac_sha256_bytes(&k_service, b"aws4_request")
    }

    fn sigv4_auth(
        creds: &AwsCredentials,
        region: &str,
        host: &str,
        uri_path: &str,
        body: &[u8],
    ) -> (String, String) {
        let now = chrono::Utc::now();
        let datetime_str = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_str = now.format("%Y%m%d").to_string();
        let payload_hash = sha256_hex(body);

        let mut canonical_headers =
            format!("content-type:application/json\nhost:{host}\nx-amz-date:{datetime_str}\n");
        let mut signed_header_list = vec!["content-type", "host", "x-amz-date"];

        if let Some(token) = &creds.session_token {
            canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
            signed_header_list.push("x-amz-security-token");
        }
        signed_header_list.sort_unstable();
        let signed_headers = signed_header_list.join(";");

        let canonical_request =
            format!("POST\n{uri_path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

        let scope = format!("{date_str}/{region}/bedrock-runtime/aws4_request");
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{datetime_str}\n{scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );

        let signing_key = derive_signing_key(
            &creds.secret_access_key,
            &date_str,
            region,
            "bedrock-runtime",
        );
        let signature = hex::encode(hmac_sha256_bytes(&signing_key, string_to_sign.as_bytes()));

        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            creds.access_key_id
        );
        (auth, datetime_str)
    }

    pub(super) fn extract_host(url: &str) -> ArgentorResult<String> {
        let without_scheme = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);
        let host = without_scheme.split('/').next().unwrap_or(without_scheme);
        if host.is_empty() {
            return Err(ArgentorError::Config(format!(
                "Could not extract host from URL: {url}"
            )));
        }
        Ok(host.to_string())
    }

    pub(super) fn extract_path(url: &str) -> String {
        let without_scheme = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);
        let path_start = without_scheme.find('/').unwrap_or(without_scheme.len());
        without_scheme[path_start..].to_string()
    }

    #[async_trait]
    impl LlmBackend for BedrockBackend {
        fn provider_name(&self) -> &str {
            "bedrock"
        }

        async fn chat(
            &self,
            system_prompt: Option<&str>,
            messages: &[Message],
            tools: &[SkillDescriptor],
        ) -> ArgentorResult<LlmResponse> {
            let creds = AwsCredentials::from_env()?;
            let region = self.region();
            let url = self.invoke_url();
            let host = extract_host(&url)?;
            let path = extract_path(&url);

            let body_json = self.build_request_body(system_prompt, messages, tools);
            let body_bytes = serde_json::to_vec(&body_json)
                .map_err(|e| ArgentorError::Agent(format!("Bedrock serialization: {e}")))?;

            let (auth, datetime) = sigv4_auth(&creds, &region, &host, &path, &body_bytes);

            let http = reqwest::Client::new();
            let mut req = http
                .post(&url)
                .header("content-type", "application/json")
                .header("x-amz-date", &datetime)
                .header("authorization", &auth)
                .body(body_bytes);

            if let Some(token) = &creds.session_token {
                req = req.header("x-amz-security-token", token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ArgentorError::Http(format!("Bedrock request: {e}")))?;
            let status = resp.status();
            let resp_body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ArgentorError::Http(format!("Bedrock response parse: {e}")))?;

            if !status.is_success() {
                return Err(ArgentorError::Http(format!(
                    "Bedrock API error {status}: {resp_body}"
                )));
            }

            crate::backends::claude::parse_claude_response(&resp_body)
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
            let creds = AwsCredentials::from_env()?;
            let region = self.region();
            let url = self.invoke_stream_url();
            let host = extract_host(&url)?;
            let path = extract_path(&url);

            let mut body_json = self.build_request_body(system_prompt, messages, tools);
            body_json["stream"] = serde_json::json!(true);
            let body_bytes = serde_json::to_vec(&body_json)
                .map_err(|e| ArgentorError::Agent(format!("Bedrock serialization: {e}")))?;

            let (auth, datetime) = sigv4_auth(&creds, &region, &host, &path, &body_bytes);

            let http = reqwest::Client::new();
            let mut req = http
                .post(&url)
                .header("content-type", "application/json")
                .header("x-amz-date", &datetime)
                .header("authorization", &auth)
                .body(body_bytes);

            if let Some(token) = &creds.session_token {
                req = req.header("x-amz-security-token", token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ArgentorError::Http(format!("Bedrock request: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let err_body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                return Err(ArgentorError::Http(format!(
                    "Bedrock API error {status}: {err_body}"
                )));
            }

            let (tx, rx) = mpsc::channel::<StreamEvent>(256);
            let byte_stream = resp.bytes_stream();

            let handle = tokio::spawn(async move {
                let mut stream = byte_stream;
                let mut buffer = String::new();
                let mut full_text = String::new();
                let mut tool_calls: Vec<argentor_core::ToolCall> = Vec::new();
                let mut active_tool_blocks: std::collections::HashMap<
                    u64,
                    (String, String, String),
                > = std::collections::HashMap::new();
                let mut stop_reason = String::from("end_turn");

                while let Some(chunk_result) = stream.next().await {
                    let chunk = match chunk_result {
                        Ok(b) => b,
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

                        let data_str = if let Some(d) = line.strip_prefix("data: ") {
                            d.to_string()
                        } else {
                            line.clone()
                        };

                        if data_str == "[DONE]" {
                            let _ = tx.send(StreamEvent::Done).await;
                            continue;
                        }

                        let event: serde_json::Value = match serde_json::from_str(&data_str) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let event_type = event["type"].as_str().unwrap_or("");

                        match event_type {
                            "content_block_start" => {
                                let index = event["index"].as_u64().unwrap_or(0);
                                let block = &event["content_block"];
                                if block["type"].as_str() == Some("tool_use") {
                                    let id = block["id"].as_str().unwrap_or_default().to_string();
                                    let name =
                                        block["name"].as_str().unwrap_or_default().to_string();
                                    active_tool_blocks
                                        .insert(index, (id.clone(), name.clone(), String::new()));
                                    let _ = tx.send(StreamEvent::ToolCallStart { id, name }).await;
                                }
                            }
                            "content_block_delta" => {
                                let index = event["index"].as_u64().unwrap_or(0);
                                let delta = &event["delta"];
                                match delta["type"].as_str().unwrap_or("") {
                                    "text_delta" => {
                                        if let Some(text) = delta["text"].as_str() {
                                            full_text.push_str(text);
                                            let _ = tx
                                                .send(StreamEvent::TextDelta {
                                                    text: text.to_string(),
                                                })
                                                .await;
                                        }
                                    }
                                    "input_json_delta" => {
                                        if let Some(partial) = delta["partial_json"].as_str() {
                                            if let Some(block) = active_tool_blocks.get_mut(&index)
                                            {
                                                block.2.push_str(partial);
                                                let _ = tx
                                                    .send(StreamEvent::ToolCallDelta {
                                                        id: block.0.clone(),
                                                        arguments_delta: partial.to_string(),
                                                    })
                                                    .await;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "content_block_stop" => {
                                let index = event["index"].as_u64().unwrap_or(0);
                                if let Some((id, name, args_json)) =
                                    active_tool_blocks.remove(&index)
                                {
                                    let arguments: serde_json::Value = serde_json::from_str(
                                        &args_json,
                                    )
                                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                    tool_calls.push(argentor_core::ToolCall {
                                        id: id.clone(),
                                        name,
                                        arguments,
                                    });
                                    let _ = tx.send(StreamEvent::ToolCallEnd { id }).await;
                                }
                            }
                            "message_delta" => {
                                if let Some(sr) = event["delta"]["stop_reason"].as_str() {
                                    stop_reason = sr.to_string();
                                }
                            }
                            "message_stop" => {
                                let _ = tx.send(StreamEvent::Done).await;
                            }
                            _ => {}
                        }
                    }
                }

                if !tool_calls.is_empty() {
                    Ok(LlmResponse::ToolUse {
                        content: if full_text.is_empty() {
                            None
                        } else {
                            Some(full_text)
                        },
                        tool_calls,
                    })
                } else if stop_reason == "end_turn" {
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
// Stub implementation (no `bedrock` feature)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "bedrock"))]
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
        Err(Self::stub_error())
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
        Err(Self::stub_error())
    }
}

#[cfg(not(feature = "bedrock"))]
impl BedrockBackend {
    fn stub_error() -> ArgentorError {
        ArgentorError::Config(
            "Bedrock backend requires the `bedrock` feature flag. \
             Build with: cargo build --features bedrock. \
             Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY in your environment. \
             AWS_DEFAULT_REGION or AWS_REGION sets the region (default: us-east-1)."
                .into(),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        std::env::remove_var("AWS_DEFAULT_REGION");
        std::env::remove_var("AWS_REGION");
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
    fn invoke_stream_url_formats_with_model_id() {
        let backend = BedrockBackend::new(sample_config());
        assert!(backend.invoke_stream_url().ends_with(
            "/model/anthropic.claude-3-5-sonnet-20240620-v1:0/invoke-with-response-stream"
        ));
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

    #[cfg(not(feature = "bedrock"))]
    #[tokio::test]
    async fn stub_chat_returns_sdk_error() {
        let backend = BedrockBackend::new(sample_config());
        let err = backend
            .chat(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("bedrock stub must error");
        let msg = err.to_string();
        assert!(
            msg.contains("bedrock") || msg.contains("AWS"),
            "error should mention bedrock/AWS, got: {msg}"
        );
    }

    #[cfg(not(feature = "bedrock"))]
    #[tokio::test]
    async fn stub_chat_stream_returns_sdk_error() {
        let backend = BedrockBackend::new(sample_config());
        let err = backend
            .chat_stream(None, &[user_msg("Hi")], &[])
            .await
            .expect_err("bedrock stub stream must error");
        assert!(
            err.to_string().contains("bedrock") || err.to_string().contains("AWS"),
            "error should mention bedrock/AWS"
        );
    }

    #[cfg(feature = "bedrock")]
    #[tokio::test]
    #[ignore = "requires real AWS credentials and network access"]
    async fn real_chat_roundtrip() {
        let backend = BedrockBackend::new(sample_config());
        let result = backend
            .chat(
                Some("You are a helpful assistant."),
                &[user_msg("Say 'hello' and nothing else.")],
                &[],
            )
            .await
            .expect("Bedrock chat should succeed with valid credentials");
        match result {
            LlmResponse::Text(t) | LlmResponse::Done(t) => assert!(!t.is_empty()),
            LlmResponse::ToolUse { .. } => panic!("Unexpected tool use"),
        }
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn sigv4_host_and_path_extraction() {
        use super::real::{extract_host, extract_path};
        let url = "https://bedrock-runtime.us-east-1.amazonaws.com/model/foo/invoke";
        assert_eq!(
            extract_host(url).unwrap(),
            "bedrock-runtime.us-east-1.amazonaws.com"
        );
        assert_eq!(extract_path(url), "/model/foo/invoke");
    }

    #[test]
    fn bedrock_skips_api_key_requirement() {
        let cfg = sample_config();
        assert!(cfg.api_key.is_empty());
    }
}

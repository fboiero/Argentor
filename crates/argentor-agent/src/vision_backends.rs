//! Vision-capable wrappers for Claude, OpenAI (GPT-4o) and Gemini.
//!
//! Each wrapper exposes a static `build_messages_payload()` that produces the
//! provider-specific JSON body for a [`MultimodalMessage`]. This is pure JSON
//! construction — no HTTP is performed — so the payload builders can be unit
//! tested and reused by higher-level clients.
//!
//! The [`VisionBackend::ask_with_image`] implementations make real HTTP calls
//! to the respective provider APIs using `reqwest`.

use crate::multimodal::{ImageInput, MultimodalMessage, VisionBackend, VisionCapability};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Claude
// ---------------------------------------------------------------------------

/// Claude vision backend — wraps the Anthropic Messages API with multimodal
/// content blocks.
///
/// Claude accepts an array of content blocks per message. Each block is either
/// `{"type": "text", "text": "..."}` or `{"type": "image", "source": {...}}`.
/// URL sources use `{"type": "url", "url": "..."}` and inline data uses
/// `{"type": "base64", "media_type": "...", "data": "..."}`.
pub struct ClaudeVisionBackend {
    api_key: String,
    model_id: String,
    api_base_url: String,
    http: reqwest::Client,
}

impl ClaudeVisionBackend {
    /// Construct a new Claude vision backend.
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model_id: model_id.into(),
            api_base_url: "https://api.anthropic.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Override the API base URL (useful for proxies or testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Get the model ID that will be used for requests.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Get the configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Get the configured API key. Useful for tests and for callers that
    /// need to build requests manually.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Build the Claude messages payload with image content blocks.
    ///
    /// Returns a JSON object of the form:
    /// ```json
    /// {
    ///   "role": "user",
    ///   "content": [
    ///     { "type": "text", "text": "..." },
    ///     { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "..." } }
    ///   ]
    /// }
    /// ```
    pub fn build_messages_payload(message: &MultimodalMessage) -> Value {
        let mut content: Vec<Value> = Vec::with_capacity(1 + message.images.len());
        content.push(json!({ "type": "text", "text": message.text }));

        for img in &message.images {
            let block = match img {
                ImageInput::Url(url) => json!({
                    "type": "image",
                    "source": { "type": "url", "url": url },
                }),
                ImageInput::Base64 { media_type, data } => json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    },
                }),
            };
            content.push(block);
        }

        json!({ "role": "user", "content": content })
    }

    /// Extract the text reply from a Claude API JSON response body.
    fn extract_text(body: &Value) -> ArgentorResult<String> {
        let content = body["content"]
            .as_array()
            .ok_or_else(|| ArgentorError::Agent("Missing content in Claude response".into()))?;

        let text = content
            .iter()
            .filter_map(|block| {
                if block["type"].as_str() == Some("text") {
                    block["text"].as_str().map(str::to_owned)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        Ok(text)
    }
}

#[async_trait]
impl VisionBackend for ClaudeVisionBackend {
    fn vision_capability(&self) -> VisionCapability {
        VisionCapability::Full
    }

    fn provider_name(&self) -> &str {
        "claude"
    }

    async fn ask_with_image(&self, message: &MultimodalMessage) -> ArgentorResult<String> {
        let url = format!("{}/v1/messages", self.api_base_url);
        let message_block = Self::build_messages_payload(message);

        let body = json!({
            "model": self.model_id,
            "max_tokens": 1024,
            "messages": [message_block],
        });

        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        let status = resp.status();
        let resp_body: Value = resp
            .json()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        if !status.is_success() {
            return Err(ArgentorError::Http(format!(
                "Claude vision API error {status}: {resp_body}"
            )));
        }

        Self::extract_text(&resp_body)
    }
}

// ---------------------------------------------------------------------------
// OpenAI (GPT-4o / gpt-4-vision-preview)
// ---------------------------------------------------------------------------

/// OpenAI vision backend — wraps the chat completions API with
/// multimodal content parts.
///
/// OpenAI accepts a content array where each entry is either
/// `{"type": "text", "text": "..."}` or
/// `{"type": "image_url", "image_url": {"url": "..."}}`. The `url` may be a
/// public http(s) URL or a `data:` URI with inlined base64.
pub struct OpenAiVisionBackend {
    api_key: String,
    model_id: String,
    api_base_url: String,
    http: reqwest::Client,
}

impl OpenAiVisionBackend {
    /// Construct a new OpenAI vision backend.
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model_id: model_id.into(),
            api_base_url: "https://api.openai.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Override the API base URL (useful for proxies or testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Get the model ID that will be used for requests.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Get the configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Get the configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Build the OpenAI messages payload with image_url parts.
    ///
    /// Returns a JSON object of the form:
    /// ```json
    /// {
    ///   "role": "user",
    ///   "content": [
    ///     { "type": "text", "text": "..." },
    ///     { "type": "image_url", "image_url": { "url": "data:image/png;base64,..." } }
    ///   ]
    /// }
    /// ```
    pub fn build_messages_payload(message: &MultimodalMessage) -> Value {
        let mut content: Vec<Value> = Vec::with_capacity(1 + message.images.len());
        content.push(json!({ "type": "text", "text": message.text }));

        for img in &message.images {
            let url = match img {
                ImageInput::Url(u) => u.clone(),
                ImageInput::Base64 { media_type, data } => {
                    format!("data:{media_type};base64,{data}")
                }
            };
            content.push(json!({
                "type": "image_url",
                "image_url": { "url": url },
            }));
        }

        json!({ "role": "user", "content": content })
    }

    /// Extract the text reply from an OpenAI chat completions JSON response body.
    fn extract_text(body: &Value) -> ArgentorResult<String> {
        let text = body["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                ArgentorError::Agent("Missing choices[0].message.content in OpenAI response".into())
            })?
            .to_owned();
        Ok(text)
    }
}

#[async_trait]
impl VisionBackend for OpenAiVisionBackend {
    fn vision_capability(&self) -> VisionCapability {
        VisionCapability::Full
    }

    fn provider_name(&self) -> &str {
        "openai"
    }

    async fn ask_with_image(&self, message: &MultimodalMessage) -> ArgentorResult<String> {
        let url = format!("{}/v1/chat/completions", self.api_base_url);
        let message_block = Self::build_messages_payload(message);

        let body = json!({
            "model": self.model_id,
            "messages": [message_block],
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        let status = resp.status();
        let resp_body: Value = resp
            .json()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        if !status.is_success() {
            return Err(ArgentorError::Http(format!(
                "OpenAI vision API error {status}: {resp_body}"
            )));
        }

        Self::extract_text(&resp_body)
    }
}

// ---------------------------------------------------------------------------
// Gemini (gemini-2.0-flash and family)
// ---------------------------------------------------------------------------

/// Gemini vision backend — wraps Google's `generateContent` API with inline
/// image parts.
///
/// Gemini accepts a `contents` array where each entry has a `parts` list. A
/// part can be `{"text": "..."}`, `{"inline_data": {"mime_type": "...",
/// "data": "..."}}`, or `{"file_data": {"mime_type": "...", "file_uri":
/// "..."}}` for URL-hosted files.
pub struct GeminiVisionBackend {
    api_key: String,
    model_id: String,
    api_base_url: String,
    http: reqwest::Client,
}

impl GeminiVisionBackend {
    /// Construct a new Gemini vision backend.
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model_id: model_id.into(),
            api_base_url: "https://generativelanguage.googleapis.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Override the API base URL (useful for proxies or testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Get the model ID that will be used for requests.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Get the configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Get the configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Build the Gemini contents payload with inline image parts.
    ///
    /// Returns a JSON object of the form:
    /// ```json
    /// {
    ///   "contents": [{
    ///     "parts": [
    ///       { "text": "..." },
    ///       { "inline_data": { "mime_type": "image/png", "data": "..." } }
    ///     ]
    ///   }]
    /// }
    /// ```
    ///
    /// URL-only images are attached as `file_data` parts since Gemini does
    /// not fetch arbitrary HTTP(S) URLs the same way OpenAI does.
    pub fn build_messages_payload(message: &MultimodalMessage) -> Value {
        let mut parts: Vec<Value> = Vec::with_capacity(1 + message.images.len());
        parts.push(json!({ "text": message.text }));

        for img in &message.images {
            let part = match img {
                ImageInput::Base64 { media_type, data } => json!({
                    "inline_data": {
                        "mime_type": media_type,
                        "data": data,
                    }
                }),
                ImageInput::Url(url) => json!({
                    "file_data": {
                        "mime_type": "image/*",
                        "file_uri": url,
                    }
                }),
            };
            parts.push(part);
        }

        json!({ "contents": [{ "parts": parts }] })
    }

    /// Extract the text reply from a Gemini generateContent JSON response body.
    fn extract_text(body: &Value) -> ArgentorResult<String> {
        let text = body["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or_else(|| {
                ArgentorError::Agent(
                    "Missing candidates[0].content.parts[0].text in Gemini response".into(),
                )
            })?
            .to_owned();
        Ok(text)
    }
}

#[async_trait]
impl VisionBackend for GeminiVisionBackend {
    fn vision_capability(&self) -> VisionCapability {
        VisionCapability::Full
    }

    fn provider_name(&self) -> &str {
        "gemini"
    }

    async fn ask_with_image(&self, message: &MultimodalMessage) -> ArgentorResult<String> {
        let url = format!(
            "{}/v1/models/{}:generateContent?key={}",
            self.api_base_url, self.model_id, self.api_key
        );
        let payload = Self::build_messages_payload(message);

        let resp = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        let status = resp.status();
        let resp_body: Value = resp
            .json()
            .await
            .map_err(|e| ArgentorError::Http(e.to_string()))?;

        if !status.is_success() {
            return Err(ArgentorError::Http(format!(
                "Gemini vision API error {status}: {resp_body}"
            )));
        }

        Self::extract_text(&resp_body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multimodal::MultimodalMessage;

    // ---- Constructors & accessors ----

    #[test]
    fn claude_backend_accessors() {
        let b = ClaudeVisionBackend::new("key-123", "claude-sonnet-4-20250514");
        assert_eq!(b.api_key(), "key-123");
        assert_eq!(b.model_id(), "claude-sonnet-4-20250514");
        assert_eq!(b.api_base_url(), "https://api.anthropic.com");
    }

    #[test]
    fn claude_backend_with_base_url() {
        let b = ClaudeVisionBackend::new("k", "m").with_base_url("http://localhost:9000");
        assert_eq!(b.api_base_url(), "http://localhost:9000");
    }

    #[test]
    fn openai_backend_accessors() {
        let b = OpenAiVisionBackend::new("sk-xxx", "gpt-4o");
        assert_eq!(b.api_key(), "sk-xxx");
        assert_eq!(b.model_id(), "gpt-4o");
        assert_eq!(b.api_base_url(), "https://api.openai.com");
    }

    #[test]
    fn openai_backend_with_base_url() {
        let b = OpenAiVisionBackend::new("k", "gpt-4o").with_base_url("http://x");
        assert_eq!(b.api_base_url(), "http://x");
    }

    #[test]
    fn gemini_backend_accessors() {
        let b = GeminiVisionBackend::new("AIza", "gemini-2.0-flash");
        assert_eq!(b.api_key(), "AIza");
        assert_eq!(b.model_id(), "gemini-2.0-flash");
        assert_eq!(
            b.api_base_url(),
            "https://generativelanguage.googleapis.com"
        );
    }

    #[test]
    fn gemini_backend_with_base_url() {
        let b = GeminiVisionBackend::new("k", "m").with_base_url("http://g");
        assert_eq!(b.api_base_url(), "http://g");
    }

    // ---- Capability declarations ----

    #[test]
    fn claude_capability_full() {
        let b = ClaudeVisionBackend::new("k", "m");
        assert_eq!(b.vision_capability(), VisionCapability::Full);
        assert_eq!(b.provider_name(), "claude");
    }

    #[test]
    fn openai_capability_full() {
        let b = OpenAiVisionBackend::new("k", "m");
        assert_eq!(b.vision_capability(), VisionCapability::Full);
        assert_eq!(b.provider_name(), "openai");
    }

    #[test]
    fn gemini_capability_full() {
        let b = GeminiVisionBackend::new("k", "m");
        assert_eq!(b.vision_capability(), VisionCapability::Full);
        assert_eq!(b.provider_name(), "gemini");
    }

    // ---- Claude payload building ----

    #[test]
    fn claude_payload_text_only() {
        let m = MultimodalMessage::new("hello");
        let p = ClaudeVisionBackend::build_messages_payload(&m);
        assert_eq!(p["role"], "user");
        assert_eq!(p["content"].as_array().unwrap().len(), 1);
        assert_eq!(p["content"][0]["type"], "text");
        assert_eq!(p["content"][0]["text"], "hello");
    }

    #[test]
    fn claude_payload_with_url_image() {
        let m = MultimodalMessage::new("q").with_image_url("https://x/a.png");
        let p = ClaudeVisionBackend::build_messages_payload(&m);
        let content = p["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "url");
        assert_eq!(content[1]["source"]["url"], "https://x/a.png");
    }

    #[test]
    fn claude_payload_with_base64_image() {
        let m = MultimodalMessage::new("q").with_image_base64("image/png", "AAAA");
        let p = ClaudeVisionBackend::build_messages_payload(&m);
        let content = p["content"].as_array().unwrap();
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "AAAA");
    }

    #[test]
    fn claude_payload_multiple_images() {
        let m = MultimodalMessage::new("q")
            .with_image_url("https://a")
            .with_image_base64("image/jpeg", "BBBB")
            .with_image_url("https://c");
        let p = ClaudeVisionBackend::build_messages_payload(&m);
        let content = p["content"].as_array().unwrap();
        assert_eq!(content.len(), 4);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["source"]["type"], "url");
        assert_eq!(content[2]["source"]["type"], "base64");
        assert_eq!(content[3]["source"]["type"], "url");
    }

    #[test]
    fn claude_payload_text_preserved() {
        let m = MultimodalMessage::new("What is this?").with_image_url("https://x");
        let p = ClaudeVisionBackend::build_messages_payload(&m);
        assert_eq!(p["content"][0]["text"], "What is this?");
    }

    // ---- OpenAI payload building ----

    #[test]
    fn openai_payload_text_only() {
        let m = MultimodalMessage::new("hello");
        let p = OpenAiVisionBackend::build_messages_payload(&m);
        assert_eq!(p["role"], "user");
        assert_eq!(p["content"].as_array().unwrap().len(), 1);
        assert_eq!(p["content"][0]["type"], "text");
        assert_eq!(p["content"][0]["text"], "hello");
    }

    #[test]
    fn openai_payload_with_url_image() {
        let m = MultimodalMessage::new("q").with_image_url("https://x/a.png");
        let p = OpenAiVisionBackend::build_messages_payload(&m);
        let content = p["content"].as_array().unwrap();
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "https://x/a.png");
    }

    #[test]
    fn openai_payload_base64_becomes_data_uri() {
        let m = MultimodalMessage::new("q").with_image_base64("image/png", "ABCDE");
        let p = OpenAiVisionBackend::build_messages_payload(&m);
        let url = p["content"][1]["image_url"]["url"].as_str().unwrap();
        assert_eq!(url, "data:image/png;base64,ABCDE");
    }

    #[test]
    fn openai_payload_base64_jpeg_data_uri() {
        let m = MultimodalMessage::new("q").with_image_base64("image/jpeg", "ZZZZ");
        let p = OpenAiVisionBackend::build_messages_payload(&m);
        let url = p["content"][1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        assert!(url.ends_with("ZZZZ"));
    }

    #[test]
    fn openai_payload_multiple_images() {
        let m = MultimodalMessage::new("q")
            .with_image_url("https://a")
            .with_image_base64("image/png", "XX")
            .with_image_url("https://c");
        let p = OpenAiVisionBackend::build_messages_payload(&m);
        let content = p["content"].as_array().unwrap();
        assert_eq!(content.len(), 4);
        assert_eq!(content[1]["image_url"]["url"], "https://a");
        assert_eq!(content[2]["image_url"]["url"], "data:image/png;base64,XX");
        assert_eq!(content[3]["image_url"]["url"], "https://c");
    }

    // ---- Gemini payload building ----

    #[test]
    fn gemini_payload_text_only() {
        let m = MultimodalMessage::new("hi");
        let p = GeminiVisionBackend::build_messages_payload(&m);
        let parts = p["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["text"], "hi");
    }

    #[test]
    fn gemini_payload_with_base64_image() {
        let m = MultimodalMessage::new("q").with_image_base64("image/png", "AAAA");
        let p = GeminiVisionBackend::build_messages_payload(&m);
        let parts = p["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["inline_data"]["mime_type"], "image/png");
        assert_eq!(parts[1]["inline_data"]["data"], "AAAA");
    }

    #[test]
    fn gemini_payload_with_url_uses_file_data() {
        let m = MultimodalMessage::new("q").with_image_url("https://x/y.png");
        let p = GeminiVisionBackend::build_messages_payload(&m);
        let parts = p["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts[1]["file_data"]["file_uri"], "https://x/y.png");
        assert_eq!(parts[1]["file_data"]["mime_type"], "image/*");
    }

    #[test]
    fn gemini_payload_multiple_images() {
        let m = MultimodalMessage::new("compare")
            .with_image_base64("image/png", "AA")
            .with_image_base64("image/jpeg", "BB");
        let p = GeminiVisionBackend::build_messages_payload(&m);
        let parts = p["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "compare");
        assert_eq!(parts[1]["inline_data"]["mime_type"], "image/png");
        assert_eq!(parts[2]["inline_data"]["mime_type"], "image/jpeg");
    }

    #[test]
    fn gemini_payload_mixed_url_and_base64() {
        let m = MultimodalMessage::new("q")
            .with_image_url("https://a")
            .with_image_base64("image/webp", "WW");
        let p = GeminiVisionBackend::build_messages_payload(&m);
        let parts = p["contents"][0]["parts"].as_array().unwrap();
        assert!(parts[1]["file_data"].is_object());
        assert!(parts[2]["inline_data"].is_object());
    }

    #[test]
    fn gemini_payload_root_shape() {
        let m = MultimodalMessage::new("x");
        let p = GeminiVisionBackend::build_messages_payload(&m);
        assert!(p["contents"].is_array());
        assert_eq!(p["contents"].as_array().unwrap().len(), 1);
    }

    // ---- Response parsing (unit tests — no network) ----

    #[test]
    fn claude_extract_text_single_block() {
        let body = json!({
            "content": [
                { "type": "text", "text": "A cat." }
            ]
        });
        assert_eq!(ClaudeVisionBackend::extract_text(&body).unwrap(), "A cat.");
    }

    #[test]
    fn claude_extract_text_multiple_blocks() {
        let body = json!({
            "content": [
                { "type": "text", "text": "Hello " },
                { "type": "text", "text": "world." }
            ]
        });
        assert_eq!(
            ClaudeVisionBackend::extract_text(&body).unwrap(),
            "Hello world."
        );
    }

    #[test]
    fn claude_extract_text_missing_content_errors() {
        let body = json!({ "stop_reason": "end_turn" });
        assert!(ClaudeVisionBackend::extract_text(&body).is_err());
    }

    #[test]
    fn openai_extract_text_ok() {
        let body = json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "A red square."
                    }
                }
            ]
        });
        assert_eq!(
            OpenAiVisionBackend::extract_text(&body).unwrap(),
            "A red square."
        );
    }

    #[test]
    fn openai_extract_text_missing_choices_errors() {
        let body = json!({ "id": "chatcmpl-xyz" });
        assert!(OpenAiVisionBackend::extract_text(&body).is_err());
    }

    #[test]
    fn gemini_extract_text_ok() {
        let body = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [{ "text": "A blue circle." }]
                    }
                }
            ]
        });
        assert_eq!(
            GeminiVisionBackend::extract_text(&body).unwrap(),
            "A blue circle."
        );
    }

    #[test]
    fn gemini_extract_text_missing_candidates_errors() {
        let body = json!({ "usageMetadata": {} });
        assert!(GeminiVisionBackend::extract_text(&body).is_err());
    }

    // ---- ask_with_image via wiremock ----

    #[tokio::test]
    async fn claude_ask_with_image_http() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_01",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": "A cute cat." }],
                "model": "claude-sonnet-4",
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 10, "output_tokens": 5 }
            })))
            .mount(&server)
            .await;

        let backend =
            ClaudeVisionBackend::new("test-key", "claude-sonnet-4").with_base_url(server.uri());
        let msg = MultimodalMessage::new("What is this?").with_image_url("https://x/cat.png");
        let result = backend.ask_with_image(&msg).await.unwrap();
        assert_eq!(result, "A cute cat.");
    }

    #[tokio::test]
    async fn claude_ask_with_image_http_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": { "type": "authentication_error", "message": "invalid api key" }
            })))
            .mount(&server)
            .await;

        let backend =
            ClaudeVisionBackend::new("bad-key", "claude-sonnet-4").with_base_url(server.uri());
        let msg = MultimodalMessage::new("test");
        let result = backend.ask_with_image(&msg).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("401"));
    }

    #[tokio::test]
    async fn openai_ask_with_image_http() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-01",
                "object": "chat.completion",
                "choices": [
                    {
                        "index": 0,
                        "message": { "role": "assistant", "content": "A red square." },
                        "finish_reason": "stop"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let backend = OpenAiVisionBackend::new("sk-test", "gpt-4o").with_base_url(server.uri());
        let msg = MultimodalMessage::new("Describe").with_image_base64("image/png", "AAAA");
        let result = backend.ask_with_image(&msg).await.unwrap();
        assert_eq!(result, "A red square.");
    }

    #[tokio::test]
    async fn openai_ask_with_image_http_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": { "message": "rate limit exceeded", "type": "rate_limit_error" }
            })))
            .mount(&server)
            .await;

        let backend = OpenAiVisionBackend::new("sk-test", "gpt-4o").with_base_url(server.uri());
        let msg = MultimodalMessage::new("test");
        let result = backend.ask_with_image(&msg).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("429"));
    }

    #[tokio::test]
    async fn gemini_ask_with_image_http() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/v1/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [
                    {
                        "content": {
                            "parts": [{ "text": "A blue triangle." }],
                            "role": "model"
                        },
                        "finishReason": "STOP"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let backend =
            GeminiVisionBackend::new("AIza-test", "gemini-2.0-flash").with_base_url(server.uri());
        let msg = MultimodalMessage::new("Describe").with_image_base64("image/png", "AAAA");
        let result = backend.ask_with_image(&msg).await.unwrap();
        assert_eq!(result, "A blue triangle.");
    }

    #[tokio::test]
    async fn gemini_ask_with_image_http_error() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/v1/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": { "code": 400, "message": "Invalid API key", "status": "INVALID_ARGUMENT" }
            })))
            .mount(&server)
            .await;

        let backend =
            GeminiVisionBackend::new("bad-key", "gemini-2.0-flash").with_base_url(server.uri());
        let msg = MultimodalMessage::new("test");
        let result = backend.ask_with_image(&msg).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("400"));
    }

    // ---- Trait-object usage (ensures Send + Sync) ----

    #[test]
    fn vision_backend_is_object_safe() {
        let _backends: Vec<Box<dyn VisionBackend>> = vec![
            Box::new(ClaudeVisionBackend::new("k", "m")),
            Box::new(OpenAiVisionBackend::new("k", "m")),
            Box::new(GeminiVisionBackend::new("k", "m")),
        ];
    }

    // ---- Integration tests (require real API keys — skipped in CI) ----

    #[tokio::test]
    #[ignore]
    async fn integration_claude_vision_real_api() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let backend = ClaudeVisionBackend::new(api_key, "claude-sonnet-4-20250514");
        let msg = MultimodalMessage::new("Reply with exactly: OK")
            .with_image_url("https://www.gstatic.com/webp/gallery/1.webp");
        let result = backend.ask_with_image(&msg).await.unwrap();
        assert!(!result.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn integration_openai_vision_real_api() {
        let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");
        let backend = OpenAiVisionBackend::new(api_key, "gpt-4o");
        let msg = MultimodalMessage::new("Reply with exactly: OK")
            .with_image_url("https://www.gstatic.com/webp/gallery/1.webp");
        let result = backend.ask_with_image(&msg).await.unwrap();
        assert!(!result.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn integration_gemini_vision_real_api() {
        let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY not set");
        let backend = GeminiVisionBackend::new(api_key, "gemini-2.0-flash");
        let msg = MultimodalMessage::new("Reply with exactly: OK")
            .with_image_base64("image/png", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==");
        let result = backend.ask_with_image(&msg).await.unwrap();
        assert!(!result.is_empty());
    }
}

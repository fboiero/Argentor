#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for LLM provider backends using a mock HTTP server.
//!
//! These tests verify that each backend correctly formats requests and parses
//! responses for the corresponding provider API (OpenAI, Claude, Gemini, Azure).

use argentor_agent::backends::LlmBackend;
use argentor_agent::config::{LlmProvider, ModelConfig};
use argentor_agent::llm::LlmResponse;
use argentor_agent::StreamEvent;
use argentor_core::Message;
use argentor_skills::SkillDescriptor;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_config(provider: LlmProvider, base_url: &str) -> ModelConfig {
    ModelConfig {
        provider,
        model_id: "test-model".into(),
        api_key: "test-key-123".into(),
        api_base_url: Some(base_url.into()),
        temperature: 0.7,
        max_tokens: 1024,
        max_turns: 10,
        fallback_models: vec![],
        retry_policy: None,
    }
}

fn user_message(content: &str) -> Message {
    Message::new(argentor_core::Role::User, content, Uuid::new_v4())
}

fn sample_tool() -> SkillDescriptor {
    SkillDescriptor {
        name: "get_weather".to_string(),
        description: "Get the current weather for a city".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        }),
        required_capabilities: vec![],
    }
}

// ============================================================
// OpenAI-compatible provider tests
// ============================================================

fn openai_text_response() -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello from the mock!"
            },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
    })
}

fn openai_tool_response() -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-456",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"Buenos Aires\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

#[tokio::test]
async fn openai_chat_text_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::OpenAi, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);

    let result = backend
        .chat(Some("You are helpful"), &[user_message("Hi")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Hello from the mock!"),
        other => panic!("Expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn openai_chat_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_tool_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::OpenAi, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);

    let result = backend
        .chat(
            None,
            &[user_message("What's the weather?")],
            &[sample_tool()],
        )
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse { tool_calls, .. } => {
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[0].arguments["city"], "Buenos Aires");
        }
        other => panic!("Expected ToolUse, got {other:?}"),
    }
}

#[tokio::test]
async fn openai_streaming_text() {
    let server = MockServer::start().await;

    let sse_body = [
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .join("");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::OpenAi, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);

    let (mut rx, handle) = backend
        .chat_stream(None, &[user_message("Hi")], &[])
        .await
        .unwrap();

    let mut texts = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { text } => texts.push(text),
            StreamEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(texts.join(""), "Hello world");

    let final_response = handle.await.unwrap().unwrap();
    match final_response {
        LlmResponse::Done(text) => assert_eq!(text, "Hello world"),
        other => panic!("Expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn openai_api_error_propagated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(serde_json::json!({"error": {"message": "rate limited"}})),
        )
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::OpenAi, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);

    let result = backend.chat(None, &[user_message("Hi")], &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("429"), "Error should contain status: {err}");
}

// ============================================================
// OpenAI-compatible variants (Groq, Mistral, xAI, etc.)
// ============================================================

#[tokio::test]
async fn groq_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Groq, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn mistral_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Mistral, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn xai_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::XAi, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn cerebras_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Cerebras, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn together_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Together, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn deepseek_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::DeepSeek, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn ollama_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Ollama, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn vllm_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::VLlm, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

// ============================================================
// Azure OpenAI — different auth header
// ============================================================

#[tokio::test]
async fn azure_openai_uses_api_key_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("api-key", "test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::AzureOpenAi, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

// ============================================================
// OpenRouter — extra headers
// ============================================================

#[tokio::test]
async fn openrouter_sends_extra_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header(
            "HTTP-Referer",
            "https://github.com/fboiero/Argentor",
        ))
        .and(header("X-Title", "Argentor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::OpenRouter, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

// ============================================================
// Claude (Anthropic) backend tests
// ============================================================

fn claude_text_response() -> serde_json::Value {
    serde_json::json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hello from Claude!" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 10, "output_tokens": 5 }
    })
}

fn claude_tool_response() -> serde_json::Value {
    serde_json::json!({
        "id": "msg_456",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": "Let me check the weather." },
            {
                "type": "tool_use",
                "id": "toolu_abc123",
                "name": "get_weather",
                "input": { "city": "Buenos Aires" }
            }
        ],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 20, "output_tokens": 15 }
    })
}

#[tokio::test]
async fn claude_chat_text_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key-123"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(claude_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Claude, &server.uri());
    let backend = argentor_agent::backends::claude::ClaudeBackend::new(config);

    let result = backend
        .chat(Some("Be helpful"), &[user_message("Hi")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Hello from Claude!"),
        other => panic!("Expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn claude_chat_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(claude_tool_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Claude, &server.uri());
    let backend = argentor_agent::backends::claude::ClaudeBackend::new(config);

    let result = backend
        .chat(None, &[user_message("Weather?")], &[sample_tool()])
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse {
            content,
            tool_calls,
        } => {
            assert_eq!(content.as_deref(), Some("Let me check the weather."));
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[0].arguments["city"], "Buenos Aires");
        }
        other => panic!("Expected ToolUse, got {other:?}"),
    }
}

#[tokio::test]
async fn claude_streaming_text() {
    let server = MockServer::start().await;

    let sse_body = [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hola\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" mundo\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]
    .join("");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Claude, &server.uri());
    let backend = argentor_agent::backends::claude::ClaudeBackend::new(config);

    let (mut rx, handle) = backend
        .chat_stream(None, &[user_message("Hola")], &[])
        .await
        .unwrap();

    let mut texts = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { text } => texts.push(text),
            StreamEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(texts.join(""), "Hola mundo");

    let final_response = handle.await.unwrap().unwrap();
    match final_response {
        LlmResponse::Done(text) => assert_eq!(text, "Hola mundo"),
        other => panic!("Expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn claude_api_error_propagated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"error": {"message": "invalid key"}})),
        )
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Claude, &server.uri());
    let backend = argentor_agent::backends::claude::ClaudeBackend::new(config);
    let result = backend.chat(None, &[user_message("Hi")], &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("401"), "Error should contain status: {err}");
}

// ============================================================
// Gemini backend tests
// ============================================================

fn gemini_text_response() -> serde_json::Value {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{ "text": "Hello from Gemini!" }],
                "role": "model"
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 5,
            "totalTokenCount": 15
        }
    })
}

fn gemini_tool_response() -> serde_json::Value {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "text": "Checking weather..." },
                    {
                        "functionCall": {
                            "name": "get_weather",
                            "args": { "city": "Buenos Aires" }
                        }
                    }
                ],
                "role": "model"
            },
            "finishReason": "STOP"
        }]
    })
}

#[tokio::test]
async fn gemini_chat_text_response() {
    let server = MockServer::start().await;

    // Gemini uses query param for API key
    Mock::given(method("POST"))
        .and(query_param("key", "test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gemini_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Gemini, &server.uri());
    let backend = argentor_agent::backends::gemini::GeminiBackend::new(config);

    let result = backend
        .chat(Some("Be helpful"), &[user_message("Hi")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Hello from Gemini!"),
        other => panic!("Expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn gemini_chat_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("key", "test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gemini_tool_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Gemini, &server.uri());
    let backend = argentor_agent::backends::gemini::GeminiBackend::new(config);

    let result = backend
        .chat(None, &[user_message("Weather?")], &[sample_tool()])
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse {
            content,
            tool_calls,
        } => {
            assert_eq!(content.as_deref(), Some("Checking weather..."));
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[0].arguments["city"], "Buenos Aires");
        }
        other => panic!("Expected ToolUse, got {other:?}"),
    }
}

#[tokio::test]
async fn gemini_streaming_text() {
    let server = MockServer::start().await;

    let sse_body = [
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hola\"}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" desde Gemini\"}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}\n\n",
    ]
    .join("");

    Mock::given(method("POST"))
        .and(query_param("alt", "sse"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Gemini, &server.uri());
    let backend = argentor_agent::backends::gemini::GeminiBackend::new(config);

    let (mut rx, handle) = backend
        .chat_stream(None, &[user_message("Hola")], &[])
        .await
        .unwrap();

    let mut texts = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { text } => texts.push(text),
            StreamEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(texts.join(""), "Hola desde Gemini");

    let final_response = handle.await.unwrap().unwrap();
    match final_response {
        LlmResponse::Done(text) => assert_eq!(text, "Hola desde Gemini"),
        other => panic!("Expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn gemini_api_error_propagated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(serde_json::json!({"error": {"message": "bad request"}})),
        )
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Gemini, &server.uri());
    let backend = argentor_agent::backends::gemini::GeminiBackend::new(config);
    let result = backend.chat(None, &[user_message("Hi")], &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("400"), "Error should contain status: {err}");
}

// ============================================================
// LlmClient dispatch tests — verify make_backend routing
// ============================================================

#[tokio::test]
async fn llm_client_dispatches_openai() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::OpenAi, &server.uri());
    let client = argentor_agent::LlmClient::new(config);
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn llm_client_dispatches_gemini() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gemini_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Gemini, &server.uri());
    let client = argentor_agent::LlmClient::new(config);
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn llm_client_dispatches_claude() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(claude_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Claude, &server.uri());
    let client = argentor_agent::LlmClient::new(config);
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

// ============================================================
// Config tests
// ============================================================

#[test]
fn provider_serde_roundtrip() {
    let providers = vec![
        ("\"claude\"", LlmProvider::Claude),
        ("\"openai\"", LlmProvider::OpenAi),
        ("\"openrouter\"", LlmProvider::OpenRouter),
        ("\"groq\"", LlmProvider::Groq),
        ("\"gemini\"", LlmProvider::Gemini),
        ("\"ollama\"", LlmProvider::Ollama),
        ("\"mistral\"", LlmProvider::Mistral),
        ("\"cerebras\"", LlmProvider::Cerebras),
        ("\"together\"", LlmProvider::Together),
        ("\"deepseek\"", LlmProvider::DeepSeek),
    ];

    for (json_str, _expected) in &providers {
        let parsed: LlmProvider = serde_json::from_str(json_str).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();
        let reparsed: LlmProvider = serde_json::from_str(&serialized).unwrap();
        // Verify round-trip by re-serializing
        assert_eq!(
            serde_json::to_string(&reparsed).unwrap(),
            serialized,
            "Round-trip failed for {json_str}"
        );
    }
}

#[test]
fn provider_aliases_work() {
    let xai: LlmProvider = serde_json::from_str("\"xai\"").unwrap();
    assert!(matches!(xai, LlmProvider::XAi));

    let azure: LlmProvider = serde_json::from_str("\"azure_openai\"").unwrap();
    assert!(matches!(azure, LlmProvider::AzureOpenAi));

    let azure2: LlmProvider = serde_json::from_str("\"azure\"").unwrap();
    assert!(matches!(azure2, LlmProvider::AzureOpenAi));

    let vllm: LlmProvider = serde_json::from_str("\"vllm\"").unwrap();
    assert!(matches!(vllm, LlmProvider::VLlm));
}

#[test]
fn default_base_urls_correct() {
    let test_cases = vec![
        (LlmProvider::Claude, "https://api.anthropic.com"),
        (LlmProvider::OpenAi, "https://api.openai.com"),
        (LlmProvider::Groq, "https://api.groq.com/openai"),
        (
            LlmProvider::Gemini,
            "https://generativelanguage.googleapis.com",
        ),
        (LlmProvider::Ollama, "http://localhost:11434"),
        (LlmProvider::Mistral, "https://api.mistral.ai"),
        (LlmProvider::XAi, "https://api.x.ai"),
        (LlmProvider::Cerebras, "https://api.cerebras.ai"),
        (LlmProvider::Together, "https://api.together.xyz"),
        (LlmProvider::DeepSeek, "https://api.deepseek.com"),
        (LlmProvider::VLlm, "http://localhost:8000"),
    ];

    for (provider, expected_url) in test_cases {
        let config = ModelConfig {
            provider,
            model_id: "test".into(),
            api_key: "key".into(),
            api_base_url: None,
            temperature: 0.7,
            max_tokens: 1024,
            max_turns: 10,
            fallback_models: vec![],
            retry_policy: None,
        };
        assert_eq!(
            config.base_url(),
            expected_url,
            "Wrong base URL for {:?}",
            config.provider
        );
    }
}

#[test]
fn custom_base_url_overrides_default() {
    let config = ModelConfig {
        provider: LlmProvider::OpenAi,
        model_id: "test".into(),
        api_key: "key".into(),
        api_base_url: Some("https://custom.api.com".into()),
        temperature: 0.7,
        max_tokens: 1024,
        max_turns: 10,
        fallback_models: vec![],
        retry_policy: None,
    };
    assert_eq!(config.base_url(), "https://custom.api.com");
}

// ============================================================
// Newly added providers — OpenAI-compatible (Fireworks, HuggingFace)
// ============================================================

#[tokio::test]
async fn fireworks_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Fireworks, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn huggingface_uses_openai_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::HuggingFace, &server.uri());
    let backend = argentor_agent::backends::openai::OpenAiBackend::new(config);
    let result = backend
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

// ============================================================
// Newly added providers — dispatch through LlmClient
// ============================================================

#[tokio::test]
async fn llm_client_dispatches_fireworks_to_openai_backend() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::Fireworks, &server.uri());
    let client = argentor_agent::LlmClient::new(config);
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
    assert_eq!(client.provider_name(), "openai");
}

#[tokio::test]
async fn llm_client_dispatches_huggingface_to_openai_backend() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_text_response()))
        .mount(&server)
        .await;

    let config = make_config(LlmProvider::HuggingFace, &server.uri());
    let client = argentor_agent::LlmClient::new(config);
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn llm_client_dispatches_cohere_to_cohere_backend() {
    let config = make_config(LlmProvider::Cohere, "https://api.cohere.com");
    let client = argentor_agent::LlmClient::new(config);
    assert_eq!(client.provider_name(), "cohere");
    // Stub returns Done without hitting HTTP.
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

#[tokio::test]
async fn llm_client_dispatches_bedrock_to_bedrock_backend() {
    let mut config = make_config(
        LlmProvider::Bedrock,
        "https://bedrock-runtime.us-east-1.amazonaws.com",
    );
    config.api_key = String::new();
    let client = argentor_agent::LlmClient::new(config);
    assert_eq!(client.provider_name(), "bedrock");
    let err = client
        .chat(None, &[user_message("Hi")], &[])
        .await
        .expect_err("bedrock stub must error");
    assert!(err.to_string().contains("AWS"));
}

#[tokio::test]
async fn llm_client_dispatches_replicate_to_replicate_backend() {
    let mut config = make_config(LlmProvider::Replicate, "https://api.replicate.com");
    config.model_id = "meta/meta-llama-3-70b-instruct".into();
    let client = argentor_agent::LlmClient::new(config);
    assert_eq!(client.provider_name(), "replicate");
    let result = client.chat(None, &[user_message("Hi")], &[]).await.unwrap();
    assert!(matches!(result, LlmResponse::Done(_)));
}

// ============================================================
// Newly added providers — default base URLs
// ============================================================

#[test]
fn new_provider_default_base_urls_correct() {
    let test_cases = vec![
        (LlmProvider::Fireworks, "https://api.fireworks.ai/inference"),
        (
            LlmProvider::HuggingFace,
            "https://api-inference.huggingface.co",
        ),
        (LlmProvider::Cohere, "https://api.cohere.com"),
        (
            LlmProvider::Bedrock,
            "https://bedrock-runtime.us-east-1.amazonaws.com",
        ),
        (LlmProvider::Replicate, "https://api.replicate.com"),
    ];

    for (provider, expected_url) in test_cases {
        let config = ModelConfig {
            provider,
            model_id: "test".into(),
            api_key: "key".into(),
            api_base_url: None,
            temperature: 0.7,
            max_tokens: 1024,
            max_turns: 10,
            fallback_models: vec![],
            retry_policy: None,
        };
        assert_eq!(
            config.base_url(),
            expected_url,
            "Wrong base URL for {:?}",
            config.provider
        );
    }
}

#[test]
fn new_provider_serde_roundtrip() {
    let providers = vec![
        "\"fireworks\"",
        "\"huggingface\"",
        "\"cohere\"",
        "\"bedrock\"",
        "\"replicate\"",
    ];

    for json_str in &providers {
        let parsed: LlmProvider = serde_json::from_str(json_str).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert_eq!(
            &serialized, json_str,
            "Round-trip failed for {json_str} (serialized as {serialized})"
        );
    }
}

#[test]
fn new_provider_aliases_work() {
    let hf_underscore: LlmProvider = serde_json::from_str("\"hugging_face\"").unwrap();
    assert!(matches!(hf_underscore, LlmProvider::HuggingFace));

    let bedrock_underscore: LlmProvider = serde_json::from_str("\"aws_bedrock\"").unwrap();
    assert!(matches!(bedrock_underscore, LlmProvider::Bedrock));

    let bedrock_dashed: LlmProvider = serde_json::from_str("\"aws-bedrock\"").unwrap();
    assert!(matches!(bedrock_dashed, LlmProvider::Bedrock));
}

#[test]
fn bedrock_is_available_without_api_key() {
    let config = ModelConfig {
        provider: LlmProvider::Bedrock,
        model_id: "anthropic.claude-3-5-sonnet-20240620-v1:0".into(),
        api_key: String::new(),
        api_base_url: None,
        temperature: 0.7,
        max_tokens: 1024,
        max_turns: 10,
        fallback_models: vec![],
        retry_policy: None,
    };
    assert!(
        config.is_available(),
        "Bedrock uses AWS credentials, not api_key — should be available"
    );
}

#[test]
fn cohere_requires_api_key_for_availability() {
    let config = ModelConfig {
        provider: LlmProvider::Cohere,
        model_id: "command-r-08-2024".into(),
        api_key: String::new(),
        api_base_url: None,
        temperature: 0.7,
        max_tokens: 1024,
        max_turns: 10,
        fallback_models: vec![],
        retry_policy: None,
    };
    assert!(!config.is_available());
}

#[test]
fn replicate_requires_api_key_for_availability() {
    let config = ModelConfig {
        provider: LlmProvider::Replicate,
        model_id: "meta/meta-llama-3-70b-instruct".into(),
        api_key: String::new(),
        api_base_url: None,
        temperature: 0.7,
        max_tokens: 1024,
        max_turns: 10,
        fallback_models: vec![],
        retry_policy: None,
    };
    assert!(!config.is_available());
}

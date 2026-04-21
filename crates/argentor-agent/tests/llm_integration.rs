#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for LLM providers — both mock-based (wiremock) and real API.
//!
//! ## Mock tests (wiremock)
//!
//! The majority of tests in this file use wiremock to simulate LLM provider
//! responses. They run in CI without any API keys and cover:
//!
//! - Response parsing for Claude, OpenAI, and Gemini APIs
//! - Tool calling flow (LLM returns tool_use -> skill executes -> backfill)
//! - SSE streaming response parsing for all three providers
//! - Error handling: 429 rate limit, 500 server error, request timeout
//! - Failover: primary backend fails -> fallback succeeds
//! - Circuit breaker: after N failures the circuit opens and rejects requests
//! - Multiple tool calls in a single response
//! - Empty/missing content edge cases
//!
//! ## Real API tests (#[ignore])
//!
//! Tests marked `#[ignore]` call real LLM APIs and require API keys:
//!
//! ```sh
//! cargo test -p argentor-agent --test llm_integration -- --ignored
//! ```
//!
//! Required environment variables (one per provider):
//!
//! - `ANTHROPIC_API_KEY`  — for Claude tests
//! - `OPENAI_API_KEY`     — for OpenAI tests
//! - `GOOGLE_API_KEY`     — for Gemini tests

use argentor_agent::backends::claude::ClaudeBackend;
use argentor_agent::backends::gemini::GeminiBackend;
use argentor_agent::backends::openai::OpenAiBackend;
use argentor_agent::backends::LlmBackend;
use argentor_agent::circuit_breaker::{CircuitBreakerRegistry, CircuitConfig, CircuitState};
use argentor_agent::config::{LlmProvider, ModelConfig};
use argentor_agent::failover::RetryPolicy;
use argentor_agent::llm::LlmResponse;
use argentor_agent::{AgentRunner, LlmClient, StreamEvent};
use argentor_core::Message;
use argentor_security::{AuditLog, PermissionSet};
use argentor_skills::{SkillDescriptor, SkillRegistry};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn user_message(content: &str) -> Message {
    Message::new(argentor_core::Role::User, content, Uuid::new_v4())
}

fn make_config(provider: LlmProvider, model_id: &str, api_key: String) -> ModelConfig {
    ModelConfig {
        provider,
        model_id: model_id.into(),
        api_key,
        api_base_url: None,
        temperature: 0.0, // deterministic
        max_tokens: 256,
        max_turns: 5,
        fallback_models: vec![],
        retry_policy: None,
    }
}

/// Build a ModelConfig that points at a wiremock server URL.
fn mock_config(provider: LlmProvider, base_url: &str) -> ModelConfig {
    ModelConfig {
        provider,
        model_id: "test-model".into(),
        api_key: "test-key-mock".into(),
        api_base_url: Some(base_url.into()),
        temperature: 0.0,
        max_tokens: 256,
        max_turns: 5,
        fallback_models: vec![],
        retry_policy: None,
    }
}

fn simple_calculator_tool() -> SkillDescriptor {
    SkillDescriptor {
        name: "calculator".to_string(),
        description: "Evaluate a simple arithmetic expression and return the result.".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The arithmetic expression to evaluate, e.g. '2 + 2'"
                }
            },
            "required": ["expression"]
        }),
        required_capabilities: vec![],
    }
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

// ---------------------------------------------------------------------------
// Mock response builders
// ---------------------------------------------------------------------------

/// Claude /v1/messages text response.
fn claude_text_response(text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "msg_mock_001",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": text }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 12, "output_tokens": 8 }
    })
}

/// Claude /v1/messages tool_use response.
fn claude_tool_use_response(tool_name: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": "msg_mock_002",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": "Let me look that up." },
            {
                "type": "tool_use",
                "id": "toolu_mock_001",
                "name": tool_name,
                "input": args
            }
        ],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 20, "output_tokens": 15 }
    })
}

/// Claude response with multiple tool calls.
fn claude_multi_tool_response() -> serde_json::Value {
    serde_json::json!({
        "id": "msg_mock_003",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": "I'll check both." },
            {
                "type": "tool_use",
                "id": "toolu_mock_a",
                "name": "get_weather",
                "input": { "city": "Buenos Aires" }
            },
            {
                "type": "tool_use",
                "id": "toolu_mock_b",
                "name": "calculator",
                "input": { "expression": "20 + 5" }
            }
        ],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 30, "output_tokens": 25 }
    })
}

/// OpenAI /v1/chat/completions text response.
fn openai_text_response(text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-mock-001",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
    })
}

/// OpenAI tool_calls response.
fn openai_tool_response(tool_name: &str, args: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-mock-002",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_mock_001",
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": args
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

/// Gemini generateContent text response.
fn gemini_text_response(text: &str) -> serde_json::Value {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{ "text": text }],
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

/// Gemini functionCall response.
fn gemini_tool_response(tool_name: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "text": "Checking..." },
                    { "functionCall": { "name": tool_name, "args": args } }
                ],
                "role": "model"
            },
            "finishReason": "STOP"
        }]
    })
}

// ===========================================================================
// WIREMOCK-BASED TESTS — run in CI, no API keys required
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. Claude API response parsing (mock /v1/messages)
// ---------------------------------------------------------------------------

/// Verifies that ClaudeBackend correctly parses a simple text response from
/// the Anthropic /v1/messages endpoint, including the x-api-key and
/// anthropic-version headers.
#[tokio::test]
async fn mock_claude_text_response_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key-mock"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(claude_text_response("Buenos dias!")),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let backend = ClaudeBackend::new(config);

    let result = backend
        .chat(Some("You are helpful."), &[user_message("Hola")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Buenos dias!"),
        other => panic!("Expected Done with text, got: {other:?}"),
    }
}

/// Verifies that ClaudeBackend correctly parses a tool_use response that
/// includes both text content and a tool call block.
#[tokio::test]
async fn mock_claude_tool_use_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(claude_tool_use_response(
                "get_weather",
                serde_json::json!({"city": "Rosario"}),
            )),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let backend = ClaudeBackend::new(config);

    let result = backend
        .chat(
            None,
            &[user_message("Weather in Rosario?")],
            &[sample_tool()],
        )
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse {
            content,
            tool_calls,
        } => {
            assert_eq!(content.as_deref(), Some("Let me look that up."));
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[0].arguments["city"], "Rosario");
        }
        other => panic!("Expected ToolUse, got: {other:?}"),
    }
}

/// Verifies that Claude can return multiple tool calls in a single response
/// and we parse all of them.
#[tokio::test]
async fn mock_claude_multiple_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(claude_multi_tool_response()))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let backend = ClaudeBackend::new(config);

    let result = backend
        .chat(
            None,
            &[user_message("Weather in BA and compute 20+5")],
            &[sample_tool(), simple_calculator_tool()],
        )
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse {
            content,
            tool_calls,
        } => {
            assert_eq!(content.as_deref(), Some("I'll check both."));
            assert_eq!(tool_calls.len(), 2, "Expected 2 tool calls");
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[1].name, "calculator");
            assert_eq!(tool_calls[1].arguments["expression"], "20 + 5");
        }
        other => panic!("Expected ToolUse with 2 calls, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 2. OpenAI API response parsing (mock /v1/chat/completions)
// ---------------------------------------------------------------------------

/// Verifies that OpenAiBackend correctly parses a standard text completion
/// response and sends the Bearer authorization header.
#[tokio::test]
async fn mock_openai_text_response_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key-mock"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_text_response("Hello from OpenAI mock!")),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::OpenAi, &server.uri());
    let backend = OpenAiBackend::new(config);

    let result = backend
        .chat(Some("Be concise."), &[user_message("Say hi")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Hello from OpenAI mock!"),
        other => panic!("Expected Done, got: {other:?}"),
    }
}

/// Verifies that OpenAiBackend correctly parses a tool_calls response
/// including function name and JSON arguments.
#[tokio::test]
async fn mock_openai_tool_call_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(openai_tool_response(
                "calculator",
                r#"{"expression":"3 * 7"}"#,
            )),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::OpenAi, &server.uri());
    let backend = OpenAiBackend::new(config);

    let result = backend
        .chat(
            None,
            &[user_message("3 times 7?")],
            &[simple_calculator_tool()],
        )
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse { tool_calls, .. } => {
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "calculator");
            assert_eq!(tool_calls[0].arguments["expression"], "3 * 7");
        }
        other => panic!("Expected ToolUse, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Gemini API response parsing (mock generateContent)
// ---------------------------------------------------------------------------

/// Verifies that GeminiBackend parses a standard text response and uses the
/// API key as a query parameter (not a header).
#[tokio::test]
async fn mock_gemini_text_response_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("key", "test-key-mock"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(gemini_text_response("Hola desde Gemini mock!")),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Gemini, &server.uri());
    let backend = GeminiBackend::new(config);

    let result = backend
        .chat(Some("Be helpful"), &[user_message("Hi")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Hola desde Gemini mock!"),
        other => panic!("Expected Done, got: {other:?}"),
    }
}

/// Verifies that GeminiBackend correctly parses a functionCall response
/// which uses the Gemini-specific format (functionCall instead of tool_calls).
#[tokio::test]
async fn mock_gemini_function_call_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("key", "test-key-mock"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(gemini_tool_response(
                "get_weather",
                serde_json::json!({"city": "Mendoza"}),
            )),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Gemini, &server.uri());
    let backend = GeminiBackend::new(config);

    let result = backend
        .chat(None, &[user_message("Mendoza weather?")], &[sample_tool()])
        .await
        .unwrap();

    match result {
        LlmResponse::ToolUse {
            content,
            tool_calls,
        } => {
            assert_eq!(content.as_deref(), Some("Checking..."));
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[0].arguments["city"], "Mendoza");
        }
        other => panic!("Expected ToolUse, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. Tool calling flow: LLM -> tool_use -> skill -> backfill -> final
// ---------------------------------------------------------------------------

/// Verifies the full agentic tool-calling loop via AgentRunner with wiremock.
///
/// Sequence:
/// 1. User sends "What is 2+2?"
/// 2. LLM returns a tool_use for "echo" (builtin) with {"input":"4"}
/// 3. AgentRunner executes the echo skill, backfills the result
/// 4. LLM returns "The answer is 4" as final text
///
/// This uses two sequential mock responses on the same endpoint.
#[tokio::test]
async fn mock_tool_calling_full_loop() {
    let server = MockServer::start().await;

    // First call: LLM returns a tool_use for the "echo" builtin skill.
    let tool_use_resp = serde_json::json!({
        "id": "msg_loop_1",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": "Let me compute that." },
            {
                "type": "tool_use",
                "id": "toolu_loop_001",
                "name": "echo",
                "input": { "input": "4" }
            }
        ],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 20, "output_tokens": 15 }
    });

    // Second call: LLM returns the final text answer.
    let final_resp = claude_text_response("The answer is 4.");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tool_use_resp))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(final_resp))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());

    let mut registry = SkillRegistry::new();
    argentor_builtins::register_builtins(&mut registry);
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));

    let agent = AgentRunner::new(config, skills, permissions, audit)
        .with_system_prompt("You are a test assistant.");

    let mut session = argentor_session::Session::new();
    let response = agent.run(&mut session, "What is 2+2?").await.unwrap();

    assert!(
        response.contains("4"),
        "Expected final response to contain '4', got: {response}"
    );
    // Session should contain user msg, assistant thinking, tool result, and final response
    assert!(
        session.message_count() >= 3,
        "Expected at least 3 messages in session, got {}",
        session.message_count()
    );
}

// ---------------------------------------------------------------------------
// 5. Streaming response parsing (SSE events)
// ---------------------------------------------------------------------------

/// Verifies Claude SSE streaming: multiple content_block_delta events are
/// collected and concatenated into the final text.
#[tokio::test]
async fn mock_claude_streaming_sse() {
    let server = MockServer::start().await;

    let sse_body = [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_s1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Stream\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ing \"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"works!\"}}\n\n",
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

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let backend = ClaudeBackend::new(config);

    let (mut rx, handle) = backend
        .chat_stream(None, &[user_message("Test stream")], &[])
        .await
        .unwrap();

    let mut chunks = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { text } => chunks.push(text),
            StreamEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(chunks.join(""), "Streaming works!");

    let final_resp = handle.await.unwrap().unwrap();
    match final_resp {
        LlmResponse::Done(text) => assert_eq!(text, "Streaming works!"),
        other => panic!("Expected Done, got: {other:?}"),
    }
}

/// Verifies OpenAI SSE streaming: delta chunks are collected and the final
/// aggregated response matches.
#[tokio::test]
async fn mock_openai_streaming_sse() {
    let server = MockServer::start().await;

    let sse_body = [
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"OpenAI\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" stream\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" OK\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .join("");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::OpenAi, &server.uri());
    let backend = OpenAiBackend::new(config);

    let (mut rx, handle) = backend
        .chat_stream(None, &[user_message("Test")], &[])
        .await
        .unwrap();

    let mut chunks = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { text } => chunks.push(text),
            StreamEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(chunks.join(""), "OpenAI stream OK");

    let final_resp = handle.await.unwrap().unwrap();
    match final_resp {
        LlmResponse::Done(text) => assert_eq!(text, "OpenAI stream OK"),
        other => panic!("Expected Done, got: {other:?}"),
    }
}

/// Verifies Gemini SSE streaming: data events with candidates are parsed and
/// aggregated.
#[tokio::test]
async fn mock_gemini_streaming_sse() {
    let server = MockServer::start().await;

    let sse_body = [
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Gemini\"}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" streaming\"}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" OK\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}\n\n",
    ]
    .join("");

    Mock::given(method("POST"))
        .and(query_param("alt", "sse"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Gemini, &server.uri());
    let backend = GeminiBackend::new(config);

    let (mut rx, handle) = backend
        .chat_stream(None, &[user_message("Test")], &[])
        .await
        .unwrap();

    let mut chunks = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta { text } => chunks.push(text),
            StreamEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(chunks.join(""), "Gemini streaming OK");

    let final_resp = handle.await.unwrap().unwrap();
    match final_resp {
        LlmResponse::Done(text) => assert_eq!(text, "Gemini streaming OK"),
        other => panic!("Expected Done, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. Error handling: 429 rate limit, 500 server error, timeout
// ---------------------------------------------------------------------------

/// Verifies that a 429 Too Many Requests response is propagated as an error
/// and the error message contains the status code.
#[tokio::test]
async fn mock_claude_429_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "error": { "type": "rate_limit_error", "message": "Too many requests" }
        })))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let backend = ClaudeBackend::new(config);

    let result = backend.chat(None, &[user_message("Hi")], &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("429"),
        "Error should mention 429 status: {err}"
    );
}

/// Verifies that a 500 Internal Server Error is propagated correctly.
#[tokio::test]
async fn mock_openai_500_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": { "message": "Internal server error", "type": "server_error" }
        })))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::OpenAi, &server.uri());
    let backend = OpenAiBackend::new(config);

    let result = backend.chat(None, &[user_message("Hi")], &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("500"),
        "Error should mention 500 status: {err}"
    );
}

/// Verifies that a Gemini 400 bad request error is propagated.
#[tokio::test]
async fn mock_gemini_400_bad_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": { "message": "Invalid request", "status": "INVALID_ARGUMENT" }
        })))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Gemini, &server.uri());
    let backend = GeminiBackend::new(config);

    let result = backend.chat(None, &[user_message("Hi")], &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("400"),
        "Error should mention 400 status: {err}"
    );
}

/// Verifies that a request timeout (server takes too long) results in an
/// error. Uses wiremock's `set_delay` to simulate a slow response.
#[tokio::test]
async fn mock_openai_request_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_text_response("slow"))
                // Delay longer than the client timeout
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::OpenAi, &server.uri());
    let backend = OpenAiBackend::new(config);

    // Apply our own timeout to the chat call.
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        backend.chat(None, &[user_message("Hi")], &[]),
    )
    .await;

    // Either the tokio timeout fires or the client's internal timeout fires.
    // Both are valid — the point is we don't hang forever.
    match result {
        Err(_elapsed) => {
            // tokio::time::timeout fired — correct behavior.
        }
        Ok(Err(_)) => {
            // Client-level timeout or connection error — also correct.
        }
        Ok(Ok(_)) => panic!("Expected timeout but got a successful response"),
    }
}

// ---------------------------------------------------------------------------
// 7. Failover: primary fails -> fallback succeeds
// ---------------------------------------------------------------------------

/// Verifies that when the primary backend returns a retryable error (502),
/// the FailoverBackend automatically falls through to the secondary backend
/// which succeeds.
#[tokio::test]
async fn mock_failover_primary_fails_secondary_succeeds() {
    let primary_server = MockServer::start().await;
    let fallback_server = MockServer::start().await;

    // Primary always returns 502 Bad Gateway (retryable)
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(502).set_body_json(serde_json::json!({
            "error": { "message": "502 Bad Gateway" }
        })))
        .mount(&primary_server)
        .await;

    // Fallback returns success
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_text_response("Fallback saved the day!")),
        )
        .mount(&fallback_server)
        .await;

    let primary_config = mock_config(LlmProvider::OpenAi, &primary_server.uri());
    let fallback_config = mock_config(LlmProvider::OpenAi, &fallback_server.uri());

    // Build LlmClient with failover configured
    let mut config_with_fallback = primary_config;
    config_with_fallback.fallback_models = vec![fallback_config];
    config_with_fallback.retry_policy = Some(RetryPolicy {
        max_retries: 1, // Retry once on primary, then move to fallback
        backoff_base_ms: 0,
        backoff_max_ms: 0,
    });

    let client = LlmClient::new(config_with_fallback);

    let result = client
        .chat(None, &[user_message("Hello")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "Fallback saved the day!"),
        other => panic!("Expected Done from fallback, got: {other:?}"),
    }
}

/// Verifies that non-retryable errors (400) immediately skip to the next
/// backend without exhausting retries.
#[tokio::test]
async fn mock_failover_non_retryable_skips_to_fallback() {
    let primary_server = MockServer::start().await;
    let fallback_server = MockServer::start().await;

    // Primary returns 400 (non-retryable)
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": { "message": "400 Bad Request" }
        })))
        .mount(&primary_server)
        .await;

    // Fallback (different provider) returns success
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(openai_text_response("OpenAI fallback OK")),
        )
        .mount(&fallback_server)
        .await;

    let primary_config = mock_config(LlmProvider::Claude, &primary_server.uri());
    let fallback_config = mock_config(LlmProvider::OpenAi, &fallback_server.uri());

    let mut config = primary_config;
    config.fallback_models = vec![fallback_config];
    config.retry_policy = Some(RetryPolicy {
        max_retries: 3,
        backoff_base_ms: 0,
        backoff_max_ms: 0,
    });

    let client = LlmClient::new(config);

    let result = client
        .chat(None, &[user_message("Hello")], &[])
        .await
        .unwrap();

    match result {
        LlmResponse::Done(text) => assert_eq!(text, "OpenAI fallback OK"),
        other => panic!("Expected Done from fallback, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 8. Circuit breaker: after N failures, circuit opens
// ---------------------------------------------------------------------------

/// Verifies the circuit breaker state machine directly:
/// - Starts Closed
/// - After threshold failures -> Open
/// - Open state rejects requests
/// - After recovery timeout -> HalfOpen
/// - Success in HalfOpen -> Closed
///
/// This tests the CircuitBreaker type used by AgentRunner, not via HTTP mocks
/// (the circuit breaker is an in-process component).
#[test]
fn circuit_breaker_lifecycle() {
    use std::time::Duration;

    let config = CircuitConfig::new(3)
        .with_recovery_timeout(Duration::from_millis(50))
        .with_success_threshold(1);

    let mut breaker = argentor_agent::circuit_breaker::CircuitBreaker::new(config);

    // Phase 1: Closed — allows requests
    assert_eq!(breaker.state(), CircuitState::Closed);
    assert!(breaker.allow_request());

    // Phase 2: Record failures up to threshold
    breaker.record_failure();
    breaker.record_failure();
    assert_eq!(
        breaker.state(),
        CircuitState::Closed,
        "2 failures < threshold 3"
    );

    breaker.record_failure(); // 3rd failure = threshold
    assert_eq!(breaker.state(), CircuitState::Open);

    // Phase 3: Open — rejects requests
    assert!(!breaker.allow_request());
    let status = breaker.status();
    assert_eq!(status.total_rejected, 1);

    // Phase 4: Wait for recovery timeout
    std::thread::sleep(Duration::from_millis(60));
    assert!(breaker.allow_request()); // transitions to HalfOpen
    assert_eq!(breaker.state(), CircuitState::HalfOpen);

    // Phase 5: Success in HalfOpen -> Closed
    breaker.record_success();
    assert_eq!(breaker.state(), CircuitState::Closed);
}

/// Verifies that the CircuitBreakerRegistry isolates per-provider state.
/// Opening the circuit for "provider_a" does not affect "provider_b".
#[test]
fn circuit_breaker_registry_isolation() {
    let registry = CircuitBreakerRegistry::new(CircuitConfig::new(2));

    // Fail provider_a twice -> opens
    registry.record_failure("provider_a");
    registry.record_failure("provider_a");
    assert!(
        !registry.allow_request("provider_a"),
        "provider_a should be open"
    );

    // provider_b should be unaffected
    assert!(
        registry.allow_request("provider_b"),
        "provider_b should be closed"
    );

    // Verify status
    let status_a = registry.status("provider_a").unwrap();
    assert_eq!(status_a.state, CircuitState::Open);
    assert_eq!(status_a.total_failures, 2);
}

/// Verifies that the AgentRunner integrates with circuit breakers and rejects
/// calls when the circuit is open for the configured provider.
#[tokio::test]
async fn mock_agent_runner_circuit_breaker_rejects_when_open() {
    let server = MockServer::start().await;

    // Mount a mock that always fails with 500
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": { "message": "500 Server Error" }
        })))
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));

    // Circuit breaker with threshold=2 so it opens quickly
    let agent = AgentRunner::new(config, skills, permissions, audit)
        .with_circuit_breaker(CircuitConfig::new(2));

    // First call: fails at LLM, circuit records failure #1
    let mut session1 = argentor_session::Session::new();
    let r1 = agent.run(&mut session1, "call 1").await;
    assert!(r1.is_err());

    // Second call: fails at LLM, circuit records failure #2 -> opens
    let mut session2 = argentor_session::Session::new();
    let r2 = agent.run(&mut session2, "call 2").await;
    assert!(r2.is_err());

    // Third call: circuit is open, request should be rejected immediately
    // without even hitting the LLM
    let mut session3 = argentor_session::Session::new();
    let r3 = agent.run(&mut session3, "call 3").await;
    assert!(r3.is_err());
    let err_msg = r3.unwrap_err().to_string();
    assert!(
        err_msg.contains("Circuit breaker open"),
        "Expected circuit breaker rejection, got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// 9. LlmClient dispatch: correct backend for each provider
// ---------------------------------------------------------------------------

/// Verifies that LlmClient correctly dispatches to the Claude backend
/// (POST /v1/messages) and to the OpenAI backend (POST /v1/chat/completions)
/// based on the provider field in ModelConfig.
#[tokio::test]
async fn mock_llm_client_dispatch_by_provider() {
    // Claude server
    let claude_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(claude_text_response("I am Claude.")),
        )
        .mount(&claude_server)
        .await;

    // OpenAI server
    let openai_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(openai_text_response("I am OpenAI.")),
        )
        .mount(&openai_server)
        .await;

    // Gemini server
    let gemini_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(gemini_text_response("I am Gemini.")),
        )
        .mount(&gemini_server)
        .await;

    let claude_client = LlmClient::new(mock_config(LlmProvider::Claude, &claude_server.uri()));
    let openai_client = LlmClient::new(mock_config(LlmProvider::OpenAi, &openai_server.uri()));
    let gemini_client = LlmClient::new(mock_config(LlmProvider::Gemini, &gemini_server.uri()));

    let r1 = claude_client
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    let r2 = openai_client
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();
    let r3 = gemini_client
        .chat(None, &[user_message("Hi")], &[])
        .await
        .unwrap();

    match r1 {
        LlmResponse::Done(t) => assert_eq!(t, "I am Claude."),
        other => panic!("Claude dispatch failed: {other:?}"),
    }
    match r2 {
        LlmResponse::Done(t) => assert_eq!(t, "I am OpenAI."),
        other => panic!("OpenAI dispatch failed: {other:?}"),
    }
    match r3 {
        LlmResponse::Done(t) => assert_eq!(t, "I am Gemini."),
        other => panic!("Gemini dispatch failed: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 10. AgentRunner end-to-end with mock backend (no real API)
// ---------------------------------------------------------------------------

/// Verifies that AgentRunner correctly handles a simple one-turn conversation
/// where the mock LLM returns a final text response immediately (no tool calls).
#[tokio::test]
async fn mock_agent_runner_simple_conversation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(claude_text_response(
                "The capital of Argentina is Buenos Aires.",
            )),
        )
        .mount(&server)
        .await;

    let config = mock_config(LlmProvider::Claude, &server.uri());
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));

    let agent = AgentRunner::new(config, skills, permissions, audit)
        .with_system_prompt("Answer geography questions.");

    let mut session = argentor_session::Session::new();
    let response = agent
        .run(&mut session, "What is the capital of Argentina?")
        .await
        .unwrap();

    assert_eq!(response, "The capital of Argentina is Buenos Aires.");
    // Session: user message + assistant response = 2
    assert_eq!(session.message_count(), 2);
}

// ===========================================================================
// REAL API TESTS — #[ignore], require actual API keys
// ===========================================================================

// ---------------------------------------------------------------------------
// Claude (Anthropic) tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY"]
async fn test_claude_real_chat() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let config = make_config(LlmProvider::Claude, "claude-haiku-4-5-20251001", api_key);
    let backend = ClaudeBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("You are a helpful assistant. Be concise."),
            &[user_message("Say hello in one sentence.")],
            &[],
        ),
    )
    .await
    .expect("request timed out after 30s")
    .expect("Claude API call failed");

    match result {
        LlmResponse::Done(text) | LlmResponse::Text(text) => {
            assert!(!text.is_empty(), "Response should not be empty");
            // Sanity: the model should produce some recognizable text
            let lower = text.to_lowercase();
            assert!(
                lower.contains("hello") || lower.contains("hi") || lower.contains("hey"),
                "Expected a greeting, got: {text}"
            );
        }
        other => panic!("Expected text response, got: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY"]
async fn test_openai_real_chat() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let config = make_config(LlmProvider::OpenAi, "gpt-4o-mini", api_key);
    let backend = OpenAiBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("You are a helpful assistant. Be concise."),
            &[user_message("Say hello in one sentence.")],
            &[],
        ),
    )
    .await
    .expect("request timed out after 30s")
    .expect("OpenAI API call failed");

    match result {
        LlmResponse::Done(text) | LlmResponse::Text(text) => {
            assert!(!text.is_empty(), "Response should not be empty");
        }
        other => panic!("Expected text response, got: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires GOOGLE_API_KEY"]
async fn test_gemini_real_chat() {
    let api_key = std::env::var("GOOGLE_API_KEY").expect("GOOGLE_API_KEY required");
    let config = make_config(LlmProvider::Gemini, "gemini-2.0-flash", api_key);
    let backend = GeminiBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("You are a helpful assistant. Be concise."),
            &[user_message("Say hello in one sentence.")],
            &[],
        ),
    )
    .await
    .expect("request timed out after 30s")
    .expect("Gemini API call failed");

    match result {
        LlmResponse::Done(text) | LlmResponse::Text(text) => {
            assert!(!text.is_empty(), "Response should not be empty");
        }
        other => panic!("Expected text response, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Claude tool calling
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY"]
async fn test_claude_tool_calling() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let config = make_config(LlmProvider::Claude, "claude-haiku-4-5-20251001", api_key);
    let backend = ClaudeBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("You have a calculator tool. Use it to answer math questions."),
            &[user_message("What is 2 + 2? Use the calculator tool.")],
            &[simple_calculator_tool()],
        ),
    )
    .await
    .expect("request timed out after 30s")
    .expect("Claude API call failed");

    match result {
        LlmResponse::ToolUse { tool_calls, .. } => {
            assert!(!tool_calls.is_empty(), "Expected at least one tool call");
            assert_eq!(
                tool_calls[0].name, "calculator",
                "Expected calculator tool call, got: {}",
                tool_calls[0].name
            );
            // The arguments should contain the expression
            let args = &tool_calls[0].arguments;
            assert!(
                args.get("expression").is_some(),
                "Expected 'expression' argument in tool call, got: {args}"
            );
        }
        other => panic!("Expected ToolUse response, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Full AgentRunner end-to-end with real Claude backend
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY"]
async fn test_agent_runner_real_e2e() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let config = make_config(LlmProvider::Claude, "claude-haiku-4-5-20251001", api_key);

    // Set up a skill registry with builtins (echo, time, etc.)
    let mut registry = SkillRegistry::new();
    argentor_builtins::register_builtins(&mut registry);
    let skills = Arc::new(registry);

    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-test-audit")));

    let agent = AgentRunner::new(config, skills, permissions, audit).with_system_prompt(
        "You are a helpful assistant. Answer concisely. \
             If asked a math question, just respond with the answer directly.",
    );

    let mut session = argentor_session::Session::new();

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.run(&mut session, "What is 2 + 2? Reply with just the number."),
    )
    .await
    .expect("request timed out after 30s")
    .expect("AgentRunner::run failed");

    assert!(!response.is_empty(), "Response should not be empty");
    assert!(
        response.contains('4'),
        "Expected response to contain '4', got: {response}"
    );
}

// ---------------------------------------------------------------------------
// Nightly smoke tests — issue #24 (Q-01)
// Executed by .github/workflows/nightly-llm.yml with `--ignored`.
// Never run in standard CI (no API keys available there).
// ---------------------------------------------------------------------------

/// Smoke 1 — Claude returns a non-empty response to a trivial prompt.
#[tokio::test]
#[ignore = "nightly: requires ANTHROPIC_API_KEY"]
async fn smoke_claude_simple_prompt() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let config = make_config(LlmProvider::Claude, "claude-haiku-4-5-20251001", api_key);
    let backend = ClaudeBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("Reply concisely."),
            &[user_message("Respond with the single word: PONG")],
            &[],
        ),
    )
    .await
    .expect("smoke_claude_simple_prompt timed out")
    .expect("Claude API call failed");

    match result {
        LlmResponse::Done(text) | LlmResponse::Text(text) => {
            assert!(!text.is_empty(), "Claude response must not be empty");
        }
        other => panic!("Expected text response from Claude, got: {other:?}"),
    }
}

/// Smoke 2 — OpenAI returns a non-empty response to a trivial prompt.
#[tokio::test]
#[ignore = "nightly: requires OPENAI_API_KEY"]
async fn smoke_openai_simple_prompt() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let config = make_config(LlmProvider::OpenAi, "gpt-4o-mini", api_key);
    let backend = OpenAiBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("Reply concisely."),
            &[user_message("Respond with the single word: PONG")],
            &[],
        ),
    )
    .await
    .expect("smoke_openai_simple_prompt timed out")
    .expect("OpenAI API call failed");

    match result {
        LlmResponse::Done(text) | LlmResponse::Text(text) => {
            assert!(!text.is_empty(), "OpenAI response must not be empty");
        }
        other => panic!("Expected text response from OpenAI, got: {other:?}"),
    }
}

/// Smoke 3 — Claude tool-calling: model must call a registered calculator tool.
#[tokio::test]
#[ignore = "nightly: requires ANTHROPIC_API_KEY"]
async fn smoke_tool_calling_agent() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let config = make_config(LlmProvider::Claude, "claude-haiku-4-5-20251001", api_key);
    let backend = ClaudeBackend::new(config);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        backend.chat(
            Some("You have a calculator tool. ALWAYS use it for math questions."),
            &[user_message("What is 6 * 7? Use the calculator tool.")],
            &[simple_calculator_tool()],
        ),
    )
    .await
    .expect("smoke_tool_calling_agent timed out")
    .expect("Claude API call failed");

    match result {
        LlmResponse::ToolUse { tool_calls, .. } => {
            assert!(!tool_calls.is_empty(), "Expected at least one tool call");
            assert_eq!(
                tool_calls[0].name, "calculator",
                "Expected calculator tool, got: {}",
                tool_calls[0].name
            );
            assert!(
                tool_calls[0].arguments.get("expression").is_some(),
                "Expected 'expression' argument, got: {:?}",
                tool_calls[0].arguments
            );
        }
        other => panic!("Expected ToolUse for math question, got: {other:?}"),
    }
}

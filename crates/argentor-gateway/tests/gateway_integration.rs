#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_gateway::{AuthConfig, GatewayServer, SessionStrategy, WebhookConfig};
use argentor_security::{AuditLog, PermissionSet, RateLimiter};
use argentor_session::FileSessionStore;
use argentor_skills::SkillRegistry;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// Helper: build a test server on a random port, returning the address.
async fn start_test_server() -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let config = ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "test-model".to_string(),
        api_key: "test-key".to_string(),
        // Point to a non-routable address so the HTTP client fails fast
        api_base_url: Some("http://127.0.0.1:1".to_string()),
        temperature: 0.7,
        max_tokens: 100,
        max_turns: 3,
        max_context_tokens: 200_000,
        fallback_models: vec![],
        retry_policy: None,
    };
    let agent = Arc::new(AgentRunner::new(config, skills, permissions, audit));
    let app = GatewayServer::build(agent, sessions);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let addr_str = format!("127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Small yield to let the server task start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (addr_str, tmp)
}

/// Connect to WebSocket, return (ws_stream, session_id from welcome).
async fn connect_ws(
    addr: &str,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Read the welcome message
    let msg = ws.next().await.unwrap().unwrap();
    let welcome: serde_json::Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    let session_id = welcome["session_id"].as_str().unwrap().to_string();

    (ws, session_id)
}

#[tokio::test]
async fn test_health_endpoint() {
    let (addr, _tmp) = start_test_server().await;
    let url = format!("http://{addr}/health");
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "argentor");
}

#[tokio::test]
async fn test_websocket_connect_and_welcome() {
    let (addr, _tmp) = start_test_server().await;
    let url = format!("ws://{addr}/ws");

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.into_text().unwrap();
    let welcome: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(welcome["type"], "connected");
    assert!(welcome["session_id"].is_string());
    assert!(welcome["connection_id"].is_string());
}

#[tokio::test]
async fn test_websocket_send_message_gets_error_response() {
    let (addr, _tmp) = start_test_server().await;
    let (mut ws, session_id) = connect_ws(&addr).await;

    // Send a message with the session_id from welcome.
    // The agent will fail (bad API URL), so we expect an error response.
    let msg = serde_json::json!({
        "session_id": session_id,
        "content": "Hello Argentor!"
    });
    ws.send(Message::Text(msg.to_string())).await.unwrap();

    // Should get an error response back quickly
    let resp_msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let text = resp_msg.into_text().unwrap();
    let response: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(response["type"], "error");
    assert!(response["session_id"].is_string());
    assert!(response["content"].as_str().unwrap().contains("Error"));
}

#[tokio::test]
async fn test_websocket_multiple_connections() {
    let (addr, _tmp) = start_test_server().await;
    let url = format!("ws://{addr}/ws");

    let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let msg1 = ws1.next().await.unwrap().unwrap();
    let msg2 = ws2.next().await.unwrap().unwrap();

    let w1: serde_json::Value = serde_json::from_str(&msg1.into_text().unwrap()).unwrap();
    let w2: serde_json::Value = serde_json::from_str(&msg2.into_text().unwrap()).unwrap();

    assert_eq!(w1["type"], "connected");
    assert_eq!(w2["type"], "connected");
    assert_ne!(w1["session_id"], w2["session_id"]);
    assert_ne!(w1["connection_id"], w2["connection_id"]);
}

#[tokio::test]
async fn test_websocket_plain_text_handled() {
    // Plain text (not JSON) — handle_socket wraps it with the connection's session_id
    let (addr, _tmp) = start_test_server().await;
    let (mut ws, _session_id) = connect_ws(&addr).await;

    ws.send(Message::Text("just plain text".to_string()))
        .await
        .unwrap();

    let resp_msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let text = resp_msg.into_text().unwrap();
    let response: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(response["session_id"].is_string());
    // Error because no real LLM — but the message was routed correctly
    assert_eq!(response["type"], "error");
}

// --- Auth middleware tests ---

fn test_model_config() -> ModelConfig {
    ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "test-model".to_string(),
        api_key: "test-key".to_string(),
        api_base_url: Some("http://127.0.0.1:1".to_string()),
        temperature: 0.7,
        max_tokens: 100,
        max_turns: 3,
        max_context_tokens: 200_000,
        fallback_models: vec![],
        retry_policy: None,
    }
}

async fn start_auth_server(api_keys: Vec<String>) -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills,
        permissions,
        audit,
    ));

    let auth = AuthConfig::new(api_keys);
    let rate_limiter = Arc::new(RateLimiter::new(100.0, 100.0));
    let app =
        GatewayServer::build_with_middleware(agent, sessions, Some(rate_limiter), auth, None, None);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let addr_str = format!("127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (addr_str, tmp)
}

#[tokio::test]
async fn test_auth_rejects_without_key() {
    let (addr, _tmp) = start_auth_server(vec!["secret-key-123".to_string()]).await;
    let resp = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_auth_accepts_valid_header() {
    let (addr, _tmp) = start_auth_server(vec!["secret-key-123".to_string()]).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer secret-key-123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_auth_accepts_query_param() {
    let (addr, _tmp) = start_auth_server(vec!["secret-key-123".to_string()]).await;
    let resp = reqwest::get(&format!("http://{addr}/health?api_key=secret-key-123"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_auth_rejects_invalid_key() {
    let (addr, _tmp) = start_auth_server(vec!["secret-key-123".to_string()]).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer wrong-key")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// --- Rate limiting tests ---

#[tokio::test]
async fn test_rate_limiting_enforced() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills,
        permissions,
        audit,
    ));

    // Very tight rate limit: 2 burst, 0.1 refill/s
    let rate_limiter = Arc::new(RateLimiter::new(2.0, 0.1));
    let auth = AuthConfig::new(vec![]);
    let app =
        GatewayServer::build_with_middleware(agent, sessions, Some(rate_limiter), auth, None, None);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // First 2 should succeed (burst)
    let r1 = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(r1.status(), 200);

    let r2 = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(r2.status(), 200);

    // Third should be rate limited
    let r3 = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(r3.status(), 429);
}

// --- Webhook integration tests ---

/// Helper: start a test server with webhook support.
async fn start_webhook_server(webhooks: Vec<WebhookConfig>) -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills,
        permissions,
        audit,
    ));

    let app = GatewayServer::with_webhooks(agent, sessions, webhooks);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let addr_str = format!("127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (addr_str, tmp)
}

#[tokio::test]
async fn test_webhook_not_found() {
    let webhooks = vec![WebhookConfig {
        name: "github".to_string(),
        secret: None,
        agent_prompt_template: "Event: {{payload}}".to_string(),
        session_strategy: SessionStrategy::New,
    }];
    let (addr, _tmp) = start_webhook_server(webhooks).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/nonexistent"))
        .body("test payload")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "webhook not found");
}

#[tokio::test]
async fn test_webhook_secret_validation_rejects_invalid() {
    let webhooks = vec![WebhookConfig {
        name: "secure-hook".to_string(),
        secret: Some("super-secret-123".to_string()),
        agent_prompt_template: "Secure event: {{payload}}".to_string(),
        session_strategy: SessionStrategy::New,
    }];
    let (addr, _tmp) = start_webhook_server(webhooks).await;

    let client = reqwest::Client::new();

    // No secret header at all
    let resp = client
        .post(format!("http://{addr}/webhook/secure-hook"))
        .body("payload data")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "invalid secret");

    // Wrong secret
    let resp = client
        .post(format!("http://{addr}/webhook/secure-hook"))
        .header("x-webhook-secret", "wrong-secret")
        .body("payload data")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_webhook_secret_validation_accepts_valid() {
    let webhooks = vec![WebhookConfig {
        name: "secure-hook".to_string(),
        secret: Some("super-secret-123".to_string()),
        agent_prompt_template: "Secure event: {{payload}}".to_string(),
        session_strategy: SessionStrategy::New,
    }];
    let (addr, _tmp) = start_webhook_server(webhooks).await;

    let client = reqwest::Client::new();

    // Correct secret — the agent will fail (bad API URL) so we get a 500 with an error
    // response, but the request passed authentication
    let resp = client
        .post(format!("http://{addr}/webhook/secure-hook"))
        .header("x-webhook-secret", "super-secret-123")
        .body(r#"{"event":"push"}"#)
        .send()
        .await
        .unwrap();

    // The response should NOT be 401 (it passed secret validation).
    // It will be 500 because the agent can't reach the LLM, which proves the
    // message was forwarded to the agent pipeline.
    assert_ne!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["webhook"], "secure-hook");
}

#[tokio::test]
async fn test_webhook_forwards_to_agent_pipeline() {
    let webhooks = vec![WebhookConfig {
        name: "ci".to_string(),
        secret: None,
        agent_prompt_template: "CI pipeline event: {{payload}}".to_string(),
        session_strategy: SessionStrategy::New,
    }];
    let (addr, _tmp) = start_webhook_server(webhooks).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/ci"))
        .body(r#"{"pipeline":"build","status":"success"}"#)
        .send()
        .await
        .unwrap();

    // The agent will fail because the LLM URL is unreachable, so we expect
    // a 500 error response with status "error" — but crucially, the webhook
    // name is included, proving the full pipeline was invoked.
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["webhook"], "ci");
    assert_eq!(body["status"], "error");
    assert!(!body["error"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_webhook_no_secret_required_accepts_any() {
    let webhooks = vec![WebhookConfig {
        name: "open-hook".to_string(),
        secret: None,
        agent_prompt_template: "Open event: {{payload}}".to_string(),
        session_strategy: SessionStrategy::New,
    }];
    let (addr, _tmp) = start_webhook_server(webhooks).await;

    let client = reqwest::Client::new();
    // Should work without any secret header
    let resp = client
        .post(format!("http://{addr}/webhook/open-hook"))
        .body("hello world")
        .send()
        .await
        .unwrap();

    // Not 401 or 404 — the request was accepted and forwarded
    assert_ne!(resp.status(), 401);
    assert_ne!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["webhook"], "open-hook");
}

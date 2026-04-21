#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Regression tests for argentor-gateway: full stack integration,
//! connection management, middleware, WebSocket lifecycle.

use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_gateway::{AuthConfig, GatewayServer};
use argentor_security::{AuditLog, PermissionSet, RateLimiter};
use argentor_session::FileSessionStore;
use argentor_skills::SkillRegistry;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

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

/// Start a full-featured test server with builtins, rate limiting, and auth.
async fn start_full_server(api_keys: Vec<String>) -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );

    // Register all builtins
    let registry = SkillRegistry::new();
    argentor_builtins::register_builtins(&registry);

    // Build permissions from builtins
    let mut permissions = PermissionSet::new();
    for desc in registry.list_descriptors() {
        for cap in &desc.required_capabilities {
            permissions.grant(cap.clone());
        }
    }

    let skills = Arc::new(registry);
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills,
        permissions,
        audit,
    ));

    let rate_limiter = Arc::new(RateLimiter::new(100.0, 100.0));
    let auth = AuthConfig::new(api_keys);
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

// --- Full stack: no auth ---

#[tokio::test]
async fn test_full_stack_health_no_auth() {
    let (addr, _tmp) = start_full_server(vec![]).await;
    let resp = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "argentor");
}

#[tokio::test]
async fn test_full_stack_websocket_with_builtins() {
    let (addr, _tmp) = start_full_server(vec![]).await;
    let url = format!("ws://{addr}/ws");

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Read welcome message
    let msg = ws.next().await.unwrap().unwrap();
    let welcome: serde_json::Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(welcome["type"], "connected");

    let session_id = welcome["session_id"].as_str().unwrap().to_string();
    let connection_id = welcome["connection_id"].as_str().unwrap().to_string();
    assert!(!session_id.is_empty());
    assert!(!connection_id.is_empty());

    // Send a message — will get error because LLM is unreachable, but the pipeline works
    let msg = serde_json::json!({
        "session_id": session_id,
        "content": "Hello Argentor!"
    });
    ws.send(Message::Text(msg.to_string())).await.unwrap();

    let resp = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let response: serde_json::Value = serde_json::from_str(&resp.into_text().unwrap()).unwrap();
    assert_eq!(response["type"], "error");
    assert_eq!(response["session_id"], session_id);
}

// --- Full stack: with auth ---

#[tokio::test]
async fn test_full_stack_auth_blocks_health() {
    let (addr, _tmp) = start_full_server(vec!["my-secret-key".to_string()]).await;

    // Without key
    let resp = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_full_stack_auth_allows_with_key() {
    let (addr, _tmp) = start_full_server(vec!["my-secret-key".to_string()]).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer my-secret-key")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_full_stack_auth_multiple_keys() {
    let (addr, _tmp) =
        start_full_server(vec!["key-alpha".to_string(), "key-beta".to_string()]).await;

    let client = reqwest::Client::new();

    // Both keys should work
    let resp1 = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer key-alpha")
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let resp2 = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer key-beta")
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);

    // Wrong key should fail
    let resp3 = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer key-gamma")
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), 401);
}

// --- WebSocket lifecycle ---

#[tokio::test]
async fn test_websocket_disconnect_and_reconnect() {
    let (addr, _tmp) = start_full_server(vec![]).await;
    let url = format!("ws://{addr}/ws");

    // Connect, get session
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let msg = ws1.next().await.unwrap().unwrap();
    let w1: serde_json::Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    let sid1 = w1["session_id"].as_str().unwrap().to_string();

    // Close the connection
    ws1.close(None).await.ok();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Reconnect — should get a new session
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let msg = ws2.next().await.unwrap().unwrap();
    let w2: serde_json::Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    let sid2 = w2["session_id"].as_str().unwrap().to_string();

    assert_ne!(sid1, sid2, "Each connection should get a unique session");
}

#[tokio::test]
async fn test_websocket_concurrent_connections() {
    let (addr, _tmp) = start_full_server(vec![]).await;
    let url = format!("ws://{addr}/ws");

    // Open 5 concurrent connections
    let mut connections = Vec::new();
    for _ in 0..5 {
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let msg = ws.next().await.unwrap().unwrap();
        let welcome: serde_json::Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
        assert_eq!(welcome["type"], "connected");
        connections.push((ws, welcome["session_id"].as_str().unwrap().to_string()));
    }

    // All sessions should be unique
    let session_ids: Vec<&String> = connections.iter().map(|(_, sid)| sid).collect();
    let unique: std::collections::HashSet<&&String> = session_ids.iter().collect();
    assert_eq!(
        unique.len(),
        5,
        "All 5 connections should have unique sessions"
    );
}

// --- Rate limiting regression ---

#[tokio::test]
async fn test_rate_limiting_burst_and_recovery() {
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

    // Tight rate limit: burst of 3, very slow refill
    let rate_limiter = Arc::new(RateLimiter::new(3.0, 0.1));
    let auth = AuthConfig::new(vec![]);
    let app =
        GatewayServer::build_with_middleware(agent, sessions, Some(rate_limiter), auth, None, None);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let url = format!("http://{addr}/health");

    // First 3 requests should succeed (burst)
    for i in 0..3 {
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200, "Request {i} should succeed (burst)");
    }

    // 4th should be rate limited
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 429, "4th request should be rate limited");
}

// --- Build methods ---

#[tokio::test]
async fn test_gateway_build_without_middleware() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );
    let skills = Arc::new(SkillRegistry::new());
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills,
        PermissionSet::new(),
        audit,
    ));

    // Use the simple build() method (no auth, no rate limiting)
    let app = GatewayServer::build(agent, sessions);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Health should work without any auth
    let resp = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

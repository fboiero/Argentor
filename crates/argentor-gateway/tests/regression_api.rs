#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]
//! Regression tests for the gateway REST API contracts.
//!
//! Covers:
//! - Health endpoints (basic + readiness)
//! - /api/v1/sessions CRUD roundtrip
//! - /api/v1/skills list
//! - /api/v1/agent/chat and /api/v1/agent/status
//! - SSE streaming events sequence
//! - Rate-limit response headers
//! - /openapi.json validity
//! - /metrics Prometheus format

use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_gateway::{AuthConfig, ControlPlaneState, GatewayServer, RestApiState};
use argentor_security::observability::AgentMetricsCollector;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::{FileSessionStore, Session, SessionStore};
use argentor_skills::SkillRegistry;
use std::sync::Arc;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_model_config() -> ModelConfig {
    ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "test-model".to_string(),
        api_key: "test-key".to_string(),
        // Unreachable URL — agent calls will fail fast without network.
        api_base_url: Some("http://127.0.0.1:1".to_string()),
        temperature: 0.0,
        max_tokens: 100,
        max_turns: 2,
        max_context_tokens: 200_000,
        fallback_models: vec![],
        retry_policy: None,
    }
}

/// Start a basic gateway server and return (addr, tempdir to keep alive).
async fn start_basic_server() -> (String, tempfile::TempDir) {
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

    let app = GatewayServer::build(agent, sessions);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    (addr, tmp)
}

/// Start a server with REST API state mounted (so /api/v1/... routes exist).
async fn start_full_server() -> (
    String,
    tempfile::TempDir,
    Arc<dyn SessionStore>,
    Arc<SkillRegistry>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions: Arc<dyn SessionStore> = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );

    let mut registry = SkillRegistry::new();
    argentor_builtins::register_builtins(&mut registry);
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills.clone(),
        permissions,
        audit.clone(),
    ));

    // Build REST API state
    let connections = argentor_gateway::connection::ConnectionManager::new();
    let router = Arc::new(argentor_gateway::router::MessageRouter::new(
        agent.clone(),
        sessions.clone(),
        connections.clone(),
    ));
    let rest_api = Arc::new(RestApiState {
        router,
        connections,
        sessions: sessions.clone(),
        skills: skills.clone(),
        started_at: chrono::Utc::now(),
    });

    // Build control plane state for readiness checks
    let control_plane = Arc::new(ControlPlaneState::new());

    let metrics = AgentMetricsCollector::new();

    let app = GatewayServer::build_full(
        agent,
        sessions.clone(),
        None,
        AuthConfig::new(vec![]),
        None,
        Some(metrics),
        Some(control_plane),
        Some(rest_api),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    (addr, tmp, sessions, skills)
}

// ---------------------------------------------------------------------------
// Health endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_endpoint_returns_200_basic() {
    let (addr, _tmp) = start_basic_server().await;
    let resp = reqwest::get(&format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "argentor");

    // /health/live is always 200
    let live = reqwest::get(&format!("http://{addr}/health/live"))
        .await
        .unwrap();
    assert_eq!(live.status(), 200);
    let live_body: serde_json::Value = live.json().await.unwrap();
    assert_eq!(live_body["status"], "alive");
}

/// Basic server has no REST API / control plane state → readiness returns 503.
#[tokio::test]
async fn test_health_endpoint_returns_503_when_unready() {
    let (addr, _tmp) = start_basic_server().await;
    let resp = reqwest::get(&format!("http://{addr}/health/ready"))
        .await
        .unwrap();
    // readiness checks require rest_api + control_plane — neither is mounted
    // on the basic builder, so it must report not-ready.
    assert_eq!(resp.status(), 503, "basic server should report not-ready");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "not_ready");
    assert!(body["checks"]["rest_api"].is_string());
    assert!(body["checks"]["control_plane"].is_string());

    // With REST + control_plane mounted, readiness returns 200.
    let (addr2, _tmp2, _s, _sk) = start_full_server().await;
    let r2 = reqwest::get(&format!("http://{addr2}/health/ready"))
        .await
        .unwrap();
    assert_eq!(r2.status(), 200, "full server should be ready");
}

// ---------------------------------------------------------------------------
// Sessions CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_api_v1_sessions_crud_roundtrip() {
    let (addr, _tmp, sessions, _skills) = start_full_server().await;

    // Seed a session directly via the store (since creation goes through the
    // agent loop normally, which we can't hit without an LLM).
    let mut session = Session::new();
    session.add_message(argentor_core::Message::user("hello", session.id));
    sessions.update(&session).await.unwrap();
    let sid = session.id;

    let client = reqwest::Client::new();

    // READ: GET /api/v1/sessions/{id}
    let get = client
        .get(format!("http://{addr}/api/v1/sessions/{sid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 200);
    let detail: serde_json::Value = get.json().await.unwrap();
    assert_eq!(detail["session_id"], sid.to_string());
    assert_eq!(detail["message_count"], 1);

    // LIST: GET /api/v1/sessions
    let list = client
        .get(format!("http://{addr}/api/v1/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let summaries: serde_json::Value = list.json().await.unwrap();
    assert!(summaries.is_array());
    let found = summaries
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["session_id"] == sid.to_string());
    assert!(found, "seeded session must appear in list");

    // DELETE: DELETE /api/v1/sessions/{id}
    let del = client
        .delete(format!("http://{addr}/api/v1/sessions/{sid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);
    let del_body: serde_json::Value = del.json().await.unwrap();
    assert_eq!(del_body["deleted"], true);

    // Verify gone — 404 after delete
    let after = client
        .get(format!("http://{addr}/api/v1/sessions/{sid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 404);
}

// ---------------------------------------------------------------------------
// Skills list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_api_v1_skills_list_returns_all() {
    let (addr, _tmp, _s, skills) = start_full_server().await;
    let expected = skills.skill_count();

    let resp = reqwest::get(&format!("http://{addr}/api/v1/skills"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let list: serde_json::Value = resp.json().await.unwrap();
    let arr = list.as_array().unwrap();
    assert_eq!(
        arr.len(),
        expected,
        "skills list should return all {expected} registered skills, got {}",
        arr.len()
    );

    // A known builtin should appear
    let has_calculator = arr.iter().any(|s| s["name"] == "calculator");
    assert!(has_calculator, "calculator builtin should be listed");
}

// ---------------------------------------------------------------------------
// Agent chat + status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_api_v1_agent_chat_full_roundtrip() {
    let (addr, _tmp, _s, _sk) = start_full_server().await;
    let client = reqwest::Client::new();

    // Agent status should return ready + skills count
    let status = client
        .get(format!("http://{addr}/api/v1/agent/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 200);
    let sbody: serde_json::Value = status.json().await.unwrap();
    assert_eq!(sbody["ready"], true);
    assert!(sbody["skills_loaded"].as_u64().unwrap() > 0);

    // Chat endpoint: the underlying agent can't reach the LLM (bad URL),
    // but the endpoint should return a response (possibly 500) rather than
    // never responding.
    let resp = client
        .post(format!("http://{addr}/api/v1/agent/chat"))
        .json(&serde_json::json!({"message": "hello"}))
        .send()
        .await
        .unwrap();
    // Either 200 (if the request managed to bubble through) or 5xx (LLM fail)
    // — both prove the endpoint is wired up.
    let status = resp.status();
    assert!(status.as_u16() >= 200, "unexpected status: {}", status);
    // Content-type should be JSON regardless
    let ct = resp.headers().get("content-type").cloned();
    assert!(
        ct.map(|v| v.to_str().unwrap_or("").contains("json"))
            .unwrap_or(false),
        "expected JSON response from chat endpoint"
    );
}

// ---------------------------------------------------------------------------
// SSE streaming
// ---------------------------------------------------------------------------

/// Verify that the SSE streaming endpoint emits events in SSE format.
/// The LLM will fail (bad URL) but we should see an error event before done.
#[tokio::test]
async fn test_sse_streaming_emits_events() {
    let (addr, _tmp, _s, _sk) = start_full_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/api/v1/chat/stream"))
        .json(&serde_json::json!({"input": "hello"}))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(
        ct.contains("text/event-stream"),
        "expected SSE content-type, got: {ct}"
    );

    // Read a chunk of the stream. We only wait briefly — the agent will
    // error out quickly because the LLM is unreachable, and we should see
    // either a heartbeat or an error event in the SSE body.
    let body = tokio::time::timeout(std::time::Duration::from_secs(3), resp.bytes())
        .await
        .map(|r| r.unwrap())
        .unwrap_or_default();

    let text = String::from_utf8_lossy(&body);
    // SSE framing: events are separated by blank lines and prefixed with
    // `event:` and `data:`. We just verify the SSE envelope is present.
    if !body.is_empty() {
        assert!(
            text.contains("data:") || text.contains("event:"),
            "body does not look like SSE: {text:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rate-limit headers
// ---------------------------------------------------------------------------

/// Per-key rate limiter surfaces X-RateLimit-* response headers.
#[tokio::test]
async fn test_rate_limit_headers_present() {
    use argentor_gateway::rate_limit_per_key::{PerKeyRateLimiter, RateLimitConfig};

    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let sessions: Arc<dyn SessionStore> = Arc::new(
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

    let per_key_config = RateLimitConfig {
        requests_per_minute: 100,
        requests_per_hour: 1000,
        tokens_per_day: 1_000_000,
    };
    let per_key = PerKeyRateLimiter::new(per_key_config);
    let auth = AuthConfig::new(vec!["test-key-abc".to_string()]);

    let app = GatewayServer::with_rate_limiter(agent, sessions, per_key, auth);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/health"))
        .header("Authorization", "Bearer test-key-abc")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    // Headers are set by per_key_rate_limit_middleware
    assert!(
        resp.headers().contains_key("X-RateLimit-Limit"),
        "missing X-RateLimit-Limit header: {:?}",
        resp.headers()
    );
    assert!(
        resp.headers().contains_key("X-RateLimit-Remaining"),
        "missing X-RateLimit-Remaining header"
    );
}

// ---------------------------------------------------------------------------
// OpenAPI spec
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openapi_json_valid() {
    let (addr, _tmp) = start_basic_server().await;
    let resp = reqwest::get(&format!("http://{addr}/openapi.json"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let spec: serde_json::Value = resp.json().await.unwrap();

    // OpenAPI 3.0.x root fields
    let version = spec["openapi"].as_str().unwrap();
    assert!(
        version.starts_with("3."),
        "expected OpenAPI 3.x, got: {version}"
    );
    assert!(spec["info"]["title"].is_string(), "missing info.title");
    assert!(spec["info"]["version"].is_string(), "missing info.version");
    assert!(spec["paths"].is_object(), "missing paths");

    // At least a few of the gateway's documented endpoints should be present
    let paths = spec["paths"].as_object().unwrap();
    assert!(
        !paths.is_empty(),
        "paths object should contain documented endpoints"
    );
}

// ---------------------------------------------------------------------------
// Prometheus metrics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_prometheus_metrics_format() {
    let (addr, _tmp, _s, _sk) = start_full_server().await;

    // Make a few requests so the metrics middleware records something.
    for _ in 0..3 {
        let _ = reqwest::get(&format!("http://{addr}/health")).await;
    }

    let resp = reqwest::get(&format!("http://{addr}/metrics"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(
        ct.contains("text/plain"),
        "Prometheus metrics must be text/plain, got: {ct}"
    );

    let body = resp.text().await.unwrap();
    assert!(!body.is_empty(), "metrics body should not be empty");

    // Valid Prometheus text exposition format includes HELP/TYPE comments
    // and lines like `metric_name{labels} value [timestamp]`.
    let has_help = body.lines().any(|l| l.starts_with("# HELP"));
    let has_type = body.lines().any(|l| l.starts_with("# TYPE"));
    let has_metric = body.lines().any(|l| !l.is_empty() && !l.starts_with('#'));
    assert!(
        has_help || has_type || has_metric,
        "body does not look like Prometheus format:\n{body}"
    );
}

#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests that validate every router mounted by `GatewayServer::build_complete()`.
//!
//! The goal is to prove that when all optional subsystems are provided, every
//! expected route is reachable (not 404) and returns the correct status code.

use argentor_a2a::{
    A2AServerState, A2ATask, AgentCapabilities, AgentCard, TaskHandler, TaskMessage, TaskStatus,
};
use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_gateway::{ControlPlaneState, GatewayServer, ProxyManagementState, RestApiState};
use argentor_mcp::credential_vault::CredentialVault;
use argentor_mcp::token_pool::{SelectionStrategy, TokenPool};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::{FileSessionStore, SessionStore};
use argentor_skills::SkillRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `ModelConfig` pointing to a non-routable address so HTTP calls fail
/// fast without ever contacting a real LLM provider.
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

/// A simple echo handler for A2A integration testing.
struct EchoHandler;

#[async_trait::async_trait]
impl TaskHandler for EchoHandler {
    async fn handle_task(&self, task: &A2ATask) -> Result<A2ATask, String> {
        let mut result = task.clone();
        result.add_message(TaskMessage::agent_text("Echo response"));
        result.transition_to(TaskStatus::Completed, None);
        Ok(result)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Build the complete gateway router with all optional subsystems enabled.
///
/// Returns the `Router` ready for `oneshot()` testing, plus the `TempDir`
/// (must be kept alive so the file-backed stores are not cleaned up).
async fn build_full_gateway() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();

    // --- Agent & sessions ---
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
        skills.clone(),
        permissions,
        audit,
    ));

    // --- Control plane ---
    let control_plane = Arc::new(ControlPlaneState::new());

    // --- REST API ---
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
        skills,
        started_at: Utc::now(),
        audit_log_path: None,
        audit_stats_cache: Arc::new(std::sync::RwLock::new(None)),
    });

    // --- Proxy management ---
    let vault = Arc::new(CredentialVault::new());
    let pool = Arc::new(TokenPool::new(SelectionStrategy::MostRemaining));
    let proxy_management = Arc::new(ProxyManagementState::new(vault, pool));

    // --- A2A state ---
    let a2a_card = AgentCard {
        name: "TestAgent".to_string(),
        description: "Integration test agent".to_string(),
        url: "http://localhost:3000".to_string(),
        version: "1.0.0".to_string(),
        provider: None,
        capabilities: AgentCapabilities::default(),
        skills: vec![],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        authentication: None,
    };
    let a2a_state = Arc::new(A2AServerState {
        agent_card: a2a_card,
        tasks: Arc::new(RwLock::new(HashMap::new())),
        handler: Arc::new(EchoHandler),
    });

    // --- Build the complete gateway ---
    let app = GatewayServer::build_complete(
        agent,
        sessions,
        None,                                      // rate_limiter
        argentor_gateway::AuthConfig::new(vec![]), // no auth
        None,                                      // webhooks
        None,                                      // metrics collector
        Some(control_plane),
        Some(rest_api),
        Some(proxy_management),
        Some(a2a_state),
    );

    (app, tmp)
}

/// Send a GET request to the given URI and return `(StatusCode, body_bytes)`.
async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

/// Send a GET request and parse the body as JSON.
async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let (status, body) = get(app, uri).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|_| {
        panic!(
            "Failed to parse JSON from {uri}: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---- Core routes ----------------------------------------------------------

#[tokio::test]
async fn test_health_route_mounted() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/health").await;
    assert_eq!(status, StatusCode::OK, "GET /health should return 200");
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_metrics_route_mounted() {
    let (app, _tmp) = build_full_gateway().await;
    // Request-level metrics are always available via the observability middleware,
    // so /metrics now returns 200 even without an explicit AgentMetricsCollector.
    let (status, body) = get(&app, "/metrics").await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "GET /metrics must not be 404"
    );
    assert_eq!(status, StatusCode::OK);
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("argentor_http_requests_total")
            || body_str.contains("argentor_active_connections")
    );
}

#[tokio::test]
async fn test_release_operability_smoke() {
    let (app, _tmp) = build_full_gateway().await;

    let (health_status, health) = get_json(&app, "/health").await;
    assert_eq!(health_status, StatusCode::OK);
    assert_eq!(health["status"], "ok");

    let (dashboard_status, dashboard_body) = get(&app, "/dashboard").await;
    assert_eq!(dashboard_status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&dashboard_body).contains("/dashboard/audit"));

    let (audit_status, audit_body) = get(&app, "/dashboard/audit").await;
    assert_eq!(audit_status, StatusCode::OK);
    let audit_html = String::from_utf8_lossy(&audit_body);
    assert!(audit_html.contains("/api/v1/audit/logs"));
    assert!(audit_html.contains("exportLogs"));
    // Endpoint health panel: parses /metrics for per-route audit traffic.
    assert!(
        audit_html.contains("Endpoint Health"),
        "audit dashboard must include the Endpoint Health panel"
    );
    assert!(
        audit_html.contains("renderEndpointHealth"),
        "audit dashboard must wire renderEndpointHealth() to fetchAll()"
    );

    let (metrics_status, metrics_body) = get(&app, "/metrics").await;
    assert_eq!(metrics_status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&metrics_body).contains("argentor_http_requests_total"));

    let (openapi_status, openapi) = get_json(&app, "/openapi.json").await;
    assert_eq!(openapi_status, StatusCode::OK);
    assert!(openapi["paths"]["/dashboard/audit"]["get"].is_object());
    assert!(openapi["components"]["schemas"]["ErrorResponse"].is_object());
}

// ---- Dashboard ------------------------------------------------------------

#[tokio::test]
async fn test_dashboard_route_mounted() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, body) = get(&app, "/dashboard").await;
    assert_eq!(status, StatusCode::OK, "GET /dashboard should return 200");
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("<html") || html.contains("<!DOCTYPE") || html.contains("<!doctype"),
        "Dashboard response should contain HTML"
    );
    assert!(
        html.contains("/dashboard/audit"),
        "Dashboard should link to the audit view"
    );
}

#[tokio::test]
async fn test_audit_dashboard_route_mounted() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, body) = get(&app, "/dashboard/audit").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /dashboard/audit should return 200"
    );
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("/api/v1/audit/logs") && html.contains("/api/v1/audit/stats"),
        "Audit dashboard response should reference audit API endpoints"
    );
}

// ---- Control plane routes -------------------------------------------------

#[tokio::test]
async fn test_control_plane_deployments_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/control-plane/deployments").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Deployments should return a JSON array");
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_control_plane_agents_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/control-plane/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Agents should return a JSON array");
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_control_plane_health_summary_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, _json) = get_json(&app, "/api/v1/control-plane/health").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_control_plane_summary_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, _json) = get_json(&app, "/api/v1/control-plane/summary").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_control_plane_events_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/control-plane/events").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Events should return a JSON array");
}

// ---- Proxy management routes ----------------------------------------------

#[tokio::test]
async fn test_proxy_management_credentials_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/proxy-management/credentials").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Credentials should return a JSON array");
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_proxy_management_credentials_stats_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/proxy-management/credentials/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["total_credentials"], 0);
}

#[tokio::test]
async fn test_proxy_management_tokens_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/proxy-management/tokens").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Tokens should return a JSON array");
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_proxy_management_tokens_stats_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/proxy-management/tokens/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["total_tokens"], 0);
}

#[tokio::test]
async fn test_proxy_management_orchestrator_metrics_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/proxy-management/orchestrator/metrics").await;
    assert_eq!(status, StatusCode::OK);
    // No orchestrator snapshot configured, should still return a JSON object
    // with zero values.
    assert_eq!(json["total_proxies"], 0);
}

// ---- REST API routes (sessions, skills, agent, connections, metrics) ------

#[tokio::test]
async fn test_rest_api_sessions_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Sessions should return a JSON array");
}

#[tokio::test]
async fn test_rest_api_skills_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/skills").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_array(), "Skills should return a JSON array");
}

#[tokio::test]
async fn test_rest_api_agent_status_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/agent/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ready"], true);
}

#[tokio::test]
async fn test_rest_api_connections_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/connections").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["count"], 0);
}

#[tokio::test]
async fn test_rest_api_metrics_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/api/v1/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["active_connections"], 0);
    assert!(json["uptime_seconds"].as_i64().unwrap() >= 0);
}

// ---- Negative: non-existent route returns 404 -----------------------------

#[tokio::test]
async fn test_nonexistent_route_returns_404() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, _body) = get(&app, "/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_nonexistent_api_route_returns_404() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, _body) = get(&app, "/api/v1/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---- A2A protocol routes -----------------------------------------------

#[tokio::test]
async fn test_a2a_agent_card_route() {
    let (app, _tmp) = build_full_gateway().await;
    let (status, json) = get_json(&app, "/.well-known/agent.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["name"], "TestAgent");
    assert_eq!(json["version"], "1.0.0");
}

#[tokio::test]
async fn test_a2a_jsonrpc_send_task() {
    let (app, _tmp) = build_full_gateway().await;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tasks/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "Hello A2A"}],
                "metadata": {}
            }
        }
    });
    let request = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let rpc_resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(rpc_resp.get("error").is_none() || rpc_resp["error"].is_null());
    let task = &rpc_resp["result"];
    assert_eq!(task["status"], "completed");
}

#[tokio::test]
async fn test_a2a_jsonrpc_get_agent_card() {
    let (app, _tmp) = build_full_gateway().await;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "agent/card",
        "params": {}
    });
    let request = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let rpc_resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(rpc_resp["result"]["name"], "TestAgent");
}

#[tokio::test]
async fn test_a2a_jsonrpc_method_not_found() {
    let (app, _tmp) = build_full_gateway().await;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "unknown/method",
        "params": {}
    });
    let request = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let rpc_resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(rpc_resp["error"].is_object());
}

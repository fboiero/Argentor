#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]
//! Smoke test for the operator-facing endpoints.
//!
//! Verifies that `/dashboard`, `/dashboard/audit`, `/metrics`, and
//! `/openapi.json` are mounted together and return well-formed responses on a
//! fully wired gateway. Catches regressions where one endpoint compiles but
//! disappears from the routing table when the others change.

use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_gateway::{AuthConfig, ControlPlaneState, GatewayServer, RestApiState};
use argentor_security::observability::AgentMetricsCollector;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::{FileSessionStore, SessionStore};
use argentor_skills::SkillRegistry;
use std::sync::Arc;
use tokio::net::TcpListener;

fn test_model_config() -> ModelConfig {
    ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "test-model".to_string(),
        api_key: "test-key".to_string(),
        api_base_url: Some("http://127.0.0.1:1".to_string()),
        temperature: 0.0,
        max_tokens: 100,
        max_turns: 2,
        max_context_tokens: 200_000,
        fallback_models: vec![],
        retry_policy: None,
    }
}

/// Spin up a gateway with metrics, control plane, and REST API state. The audit
/// log path is set so `/metrics` exports the `argentor_audit_*` family and
/// `/dashboard/audit` has a real backing file (created lazily by the audit log).
async fn start_full_operator_server() -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tmp.path().join("audit");
    let audit = Arc::new(AuditLog::new(audit_dir.clone()));
    let sessions: Arc<dyn SessionStore> = Arc::new(
        FileSessionStore::new(tmp.path().join("sessions"))
            .await
            .unwrap(),
    );

    let registry = SkillRegistry::new();
    argentor_builtins::register_builtins(&registry);
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();
    let agent = Arc::new(AgentRunner::new(
        test_model_config(),
        skills.clone(),
        permissions,
        audit.clone(),
    ));

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
        audit_log_path: Some(audit_dir.join("audit.jsonl")),
        audit_stats_cache: Arc::new(std::sync::RwLock::new(None)),
    });

    let control_plane = Arc::new(ControlPlaneState::new());
    let metrics = AgentMetricsCollector::new();

    let app = GatewayServer::build_full(
        agent,
        sessions,
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

    (addr, tmp)
}

#[tokio::test]
async fn operator_endpoints_all_respond() {
    let (addr, _tmp) = start_full_operator_server().await;
    let client = reqwest::Client::new();

    // /dashboard — main operator cockpit
    let r = client
        .get(format!("http://{addr}/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "/dashboard should return 200");
    let ct = r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("text/html"),
        "/dashboard must be text/html, got {ct}"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains("<html") || body.contains("<!DOCTYPE"),
        "/dashboard body must be HTML"
    );

    // /dashboard/audit — audit operator cockpit
    let r = client
        .get(format!("http://{addr}/dashboard/audit"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "/dashboard/audit should return 200");
    let ct = r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("text/html"),
        "/dashboard/audit must be text/html, got {ct}"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains("<html") || body.contains("<!DOCTYPE"),
        "/dashboard/audit body must be HTML"
    );

    // /metrics — Prometheus text exposition
    let r = client
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "/metrics should return 200");
    let ct = r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("text/plain"),
        "/metrics must be text/plain (Prometheus), got {ct}"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains("# HELP") || body.contains("# TYPE"),
        "/metrics body must include Prometheus HELP/TYPE comments"
    );
    // Active-stream gauge: present from process start, zero with no subscribers.
    assert!(
        body.contains("argentor_active_streams"),
        "/metrics must expose the argentor_active_streams gauge"
    );
    assert!(
        body.contains("argentor_active_streams 0"),
        "argentor_active_streams should read 0 with no active SSE subscribers"
    );

    // /openapi.json — must be valid JSON with at least the audit + dashboard paths
    let r = client
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "/openapi.json should return 200");
    let ct = r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("application/json"),
        "/openapi.json must be application/json, got {ct}"
    );
    let spec: serde_json::Value = r.json().await.unwrap();
    let paths = spec
        .get("paths")
        .and_then(|p| p.as_object())
        .expect("/openapi.json must include a 'paths' object");
    for required in [
        "/dashboard",
        "/dashboard/audit",
        "/metrics",
        "/openapi.json",
        "/api/v1/audit/logs",
        "/api/v1/audit/violations",
        "/api/v1/audit/stats",
    ] {
        assert!(
            paths.contains_key(required),
            "/openapi.json must document path {required}"
        );
    }
}

#[tokio::test]
async fn metrics_exposes_audit_family_when_audit_log_path_set() {
    let (addr, _tmp) = start_full_operator_server().await;
    let client = reqwest::Client::new();

    let body = client
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    for metric in [
        "argentor_audit_configured",
        "argentor_audit_log_bytes",
        "argentor_audit_events_total",
        "argentor_audit_events_today",
        "argentor_audit_violations_today",
        "argentor_audit_block_rate_percent",
    ] {
        assert!(
            body.contains(metric),
            "/metrics must include {metric} when audit_log_path is configured (body: {body})"
        );
    }
}

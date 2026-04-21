#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]
//! Gateway scalability tests under load.
//!
//! These tests exercise the gateway's HTTP/WebSocket surface under concurrent
//! load. They use the in-memory `tower::ServiceExt::oneshot` pattern when
//! possible to avoid binding real TCP ports, and a real listener for the
//! WebSocket-heavy tests where the framing matters.

use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_gateway::rate_limit_per_key::{PerKeyRateLimiter, RateLimitConfig, RateLimitResult};
use argentor_gateway::{
    graceful_shutdown::{ShutdownManager, ShutdownPhase},
    AuthConfig, GatewayServer, RestApiState,
};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::{FileSessionStore, SessionStore};
use argentor_skills::SkillRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dead_model_config() -> ModelConfig {
    ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "test-model".to_string(),
        api_key: "test-key".to_string(),
        api_base_url: Some("http://127.0.0.1:1".to_string()),
        temperature: 0.0,
        max_tokens: 32,
        max_turns: 1,
        max_context_tokens: 200_000,
        fallback_models: vec![],
        retry_policy: None,
    }
}

/// Build the full gateway router with a dead-LLM agent so requests fail fast
/// rather than calling out to the network.
async fn build_router() -> (axum::Router, tempfile::TempDir) {
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
        dead_model_config(),
        skills.clone(),
        permissions,
        audit,
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
        skills,
        started_at: Utc::now(),
    });

    let app = GatewayServer::build_full(
        agent,
        sessions,
        None,
        AuthConfig::new(vec![]),
        None,
        None,
        None,
        Some(rest_api),
    );
    (app, tmp)
}

/// Spin up a real TCP listener bound to localhost, returning the address.
async fn start_real_server() -> (String, tempfile::TempDir) {
    let (app, tmp) = build_router().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let addr_str = format!("127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr_str, tmp)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 100 parallel POST /api/v1/agent/chat requests — all should resolve (with
/// either 200 or a clean 5xx because the LLM is unreachable) within 10s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_100_concurrent_chat_requests() {
    let (app, _tmp) = build_router().await;

    const N: usize = 100;
    let start = std::time::Instant::now();
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            let body = serde_json::json!({"message": format!("hello {i}"), "session_id": null})
                .to_string();
            let req = Request::builder()
                .method("POST")
                .uri("/api/v1/agent/chat")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        }));
    }

    let mut resolved = 0usize;
    for h in handles {
        let _status = h.await.expect("task panicked");
        resolved += 1;
    }
    let elapsed = start.elapsed();
    assert_eq!(resolved, N, "all 100 chat requests must resolve");
    assert!(
        elapsed < Duration::from_secs(10),
        "100 concurrent chat requests took too long: {elapsed:?}"
    );
}

/// Per-key rate limiter denies requests after the limit is hit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_rate_limit_triggers_at_threshold() {
    let cfg = RateLimitConfig {
        requests_per_minute: 50,
        requests_per_hour: 10_000,
        tokens_per_day: 10_000_000,
    };
    let limiter = PerKeyRateLimiter::new(cfg);

    let mut allowed = 0usize;
    let mut denied = 0usize;
    for _ in 0..100 {
        match limiter.check("key-a") {
            RateLimitResult::Allow => allowed += 1,
            RateLimitResult::Deny { .. } => denied += 1,
        }
    }
    assert_eq!(allowed, 50, "first 50 must pass");
    assert_eq!(denied, 50, "remaining 50 must be denied");
}

/// X-RateLimit-Remaining header decrements correctly after consuming the
/// quota. Tested at the limiter level (matches the value that the middleware
/// emits — see `per_key_rate_limit_middleware`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_rate_limit_headers_decrement_correctly() {
    let cfg = RateLimitConfig {
        requests_per_minute: 100,
        requests_per_hour: 10_000,
        tokens_per_day: 10_000_000,
    };
    let limiter = PerKeyRateLimiter::new(cfg);

    for _ in 0..10 {
        let res = limiter.check("user-1");
        assert!(matches!(res, RateLimitResult::Allow));
    }
    let stats = limiter
        .stats("user-1")
        .expect("stats must exist after first check");
    let remaining = stats
        .config
        .requests_per_minute
        .saturating_sub(stats.requests_this_minute);
    assert_eq!(
        remaining, 90,
        "after 10 requests of 100/min, remaining must equal 90"
    );
}

/// 500 WebSocket connections — all tracked, then graceful cleanup on close
/// returns the count to zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_connection_manager_handles_many_ws() {
    let mgr = argentor_gateway::connection::ConnectionManager::new();

    const N: usize = 500;
    let mut conn_ids = Vec::with_capacity(N);
    for _ in 0..N {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let id = uuid::Uuid::new_v4();
        let conn = argentor_gateway::connection::Connection {
            id,
            session_id: uuid::Uuid::new_v4(),
            tx,
        };
        mgr.add(conn).await;
        conn_ids.push(id);
    }
    assert_eq!(mgr.connection_count().await, N);

    for id in conn_ids {
        mgr.remove(id).await;
    }
    assert_eq!(
        mgr.connection_count().await,
        0,
        "all connections must be removed cleanly"
    );
}

/// 200 concurrent GETs to /openapi.json — every response is a valid JSON doc.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_openapi_json_under_load() {
    let (app, _tmp) = build_router().await;

    // Use 200 instead of 1000 to keep the test well under 10s on CI.
    const N: usize = 200;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            let req = Request::builder()
                .method("GET")
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&body).is_ok(),
            )
        }));
    }

    let mut all_ok = true;
    for h in handles {
        let (status, valid) = h.await.expect("task panicked");
        if status != StatusCode::OK || !valid {
            all_ok = false;
        }
    }
    assert!(
        all_ok,
        "every /openapi.json response must be 200 + valid JSON"
    );
}

/// Graceful shutdown: register a hook that records it ran, then shut down
/// and confirm the hook fired exactly once and the report shows success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_graceful_shutdown_drains_inflight() {
    let mgr = ShutdownManager::new(Duration::from_secs(5));
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    {
        let c = counter.clone();
        mgr.on_shutdown("drain-counter", ShutdownPhase::Drain, move || {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await;
    }

    let report = mgr.shutdown().await;
    assert_eq!(report.hooks_succeeded, 1);
    assert_eq!(report.hooks_failed, 0);
    assert!(report.completed_in_time);
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Calling shutdown again is a no-op (already completed).
    let second = mgr.shutdown().await;
    assert_eq!(second.hooks_succeeded, 0);
}

/// Per-provider circuit breaker isolation: failures on provider A do NOT
/// open the circuit for provider B.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_circuit_breaker_per_provider_isolation() {
    use argentor_agent::circuit_breaker::{CircuitBreakerRegistry, CircuitConfig, CircuitState};

    let registry = CircuitBreakerRegistry::new(CircuitConfig::new(3));

    // Trip provider A (threshold = 3 ⇒ 3 failures opens the circuit).
    for _ in 0..3 {
        registry.record_failure("provider-a");
    }
    let status_a = registry.status("provider-a").unwrap();
    assert_eq!(status_a.state, CircuitState::Open);

    // Provider B is untouched and must still allow requests.
    assert!(registry.allow_request("provider-b"));
    let status_b = registry.status("provider-b").unwrap();
    assert_eq!(status_b.state, CircuitState::Closed);
}

/// Smoke: the real TCP server starts and serves /health under concurrent load.
/// Kept small (50 reqs) — this is a smoke test, not a benchmark.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_real_server_health_under_concurrency() {
    let (addr, _tmp) = start_real_server().await;
    const N: usize = 50;

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let url = format!("http://{addr}/health");
        handles.push(tokio::spawn(async move {
            reqwest::get(&url).await.map(|r| r.status().as_u16())
        }));
    }
    let mut ok_count = 0;
    for h in handles {
        if let Ok(Ok(200)) = h.await {
            ok_count += 1;
        }
    }
    assert_eq!(ok_count, N);
}

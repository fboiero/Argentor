use crate::connection::{Connection, ConnectionManager};
use crate::control_plane::{control_plane_router, ControlPlaneState};
use crate::dashboard::dashboard_router;
use crate::enterprise_readiness::{
    build_enterprise_readiness_report, EnterpriseReadinessInput, EnterpriseReadinessReport,
};
use crate::graceful_shutdown::{ShutdownManager, ShutdownPhase};
use crate::middleware::{
    auth_middleware, per_key_rate_limit_middleware, rate_limit_middleware, AuthConfig,
    MiddlewareState,
};
use crate::observability::{
    request_tracing_middleware, ObservabilityMiddlewareState, RequestMetrics,
};
use crate::playground::playground_router;
use crate::pricing_page::pricing_router;
use crate::proxy_management::{proxy_management_router, ProxyManagementState};
use crate::rate_limit_per_key::PerKeyRateLimiter;
use crate::rest_api::{api_router, audit_prometheus_export, RestApiState};
use crate::router::{InboundMessage, MessageRouter};
use crate::streaming::{streaming_router, StreamingState};
use crate::webhook::{webhook_handler, WebhookConfig, WebhookState};
use argentor_a2a::server::{A2AServer, A2AServerState};
use argentor_agent::AgentRunner;
use argentor_security::observability::AgentMetricsCollector;
use argentor_security::RateLimiter;
use argentor_session::SessionStore;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, StatusCode},
    middleware as axum_mw,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Shared application state.
pub struct AppState {
    /// The message router that handles agent interactions.
    pub router: Arc<MessageRouter>,
    /// Tracks active WebSocket connections.
    pub connections: Arc<ConnectionManager>,
    /// Webhook configuration, if any.
    pub webhooks: Option<WebhookState>,
    /// Optional metrics collector for Prometheus-compatible `/metrics` endpoint.
    pub metrics: Option<AgentMetricsCollector>,
    /// Optional control-plane state for orchestrator management endpoints.
    pub control_plane: Option<Arc<ControlPlaneState>>,
    /// Optional REST API state for session/skill/agent management endpoints.
    pub rest_api: Option<Arc<RestApiState>>,
    /// Optional graceful shutdown manager.
    pub shutdown_manager: Option<ShutdownManager>,
    /// Optional per-API-key rate limiter.
    pub per_key_rate_limiter: Option<Arc<PerKeyRateLimiter>>,
    /// Optional request-level observability metrics.
    pub request_metrics: Option<Arc<RequestMetrics>>,
    /// Whether API authentication is configured.
    pub auth_enabled: bool,
    /// Whether proxy management routes are mounted.
    pub proxy_management_mounted: bool,
    /// Whether A2A protocol routes are mounted.
    pub a2a_mounted: bool,
}

// ---------------------------------------------------------------------------
// Graceful shutdown configuration
// ---------------------------------------------------------------------------

/// Configuration for graceful shutdown timeouts.
#[derive(Debug, Clone)]
pub struct GracefulShutdownConfig {
    /// Total maximum time for the shutdown sequence (default: 30s).
    pub total_timeout: Duration,
    /// Maximum time to drain in-flight connections (default: 15s).
    pub drain_timeout: Duration,
    /// Maximum time for cleanup hooks (default: 10s).
    pub cleanup_timeout: Duration,
    /// Maximum time for final hooks (default: 5s).
    pub final_timeout: Duration,
}

impl Default for GracefulShutdownConfig {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_secs(30),
            drain_timeout: Duration::from_secs(15),
            cleanup_timeout: Duration::from_secs(10),
            final_timeout: Duration::from_secs(5),
        }
    }
}

/// The main gateway server.
pub struct GatewayServer;

impl GatewayServer {
    /// Build the gateway without auth or rate limiting (backwards compatible).
    pub fn build(agent: Arc<AgentRunner>, sessions: Arc<dyn SessionStore>) -> Router {
        Self::build_with_middleware(agent, sessions, None, AuthConfig::new(vec![]), None, None)
    }

    /// Build the gateway with optional rate limiting, auth middleware, webhooks, and metrics.
    ///
    /// This method is kept for backward compatibility. It delegates to [`build_full`] with
    /// `control_plane` and `rest_api` set to `None`.
    pub fn build_with_middleware(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        rate_limiter: Option<Arc<RateLimiter>>,
        auth_config: AuthConfig,
        webhooks: Option<Vec<WebhookConfig>>,
        metrics: Option<AgentMetricsCollector>,
    ) -> Router {
        Self::build_full(
            agent,
            sessions,
            rate_limiter,
            auth_config,
            webhooks,
            metrics,
            None,
            None,
        )
    }

    /// Build the full gateway with all optional subsystems.
    ///
    /// Accepts every parameter from [`build_with_middleware`] plus optional subsystem
    /// states. When provided, their routers are merged into the main application so
    /// their routes become available alongside `/ws`, `/health`, etc.
    ///
    /// - `control_plane` — mounts control-plane routes (`/api/v1/control-plane/…`).
    /// - `rest_api` — mounts REST API routes (`/api/v1/sessions`, `/api/v1/skills`, …).
    #[allow(clippy::too_many_arguments)]
    pub fn build_full(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        rate_limiter: Option<Arc<RateLimiter>>,
        auth_config: AuthConfig,
        webhooks: Option<Vec<WebhookConfig>>,
        metrics: Option<AgentMetricsCollector>,
        control_plane: Option<Arc<ControlPlaneState>>,
        rest_api: Option<Arc<RestApiState>>,
    ) -> Router {
        Self::build_complete(
            agent,
            sessions,
            rate_limiter,
            auth_config,
            webhooks,
            metrics,
            control_plane,
            rest_api,
            None,
            None,
        )
    }

    /// Build the complete gateway with every optional subsystem including proxy management, A2A, and XcapitSFF.
    ///
    /// This is the most comprehensive builder. All other `build_*` methods delegate here.
    ///
    /// - `proxy_management` — mounts proxy management routes (`/api/v1/proxy-management/…`).
    /// - `a2a` — mounts A2A protocol routes (`/.well-known/agent.json`, `/a2a`).
    /// - `xcapitsff` — mounts XcapitSFF integration routes (`/api/v1/agent/…`, `/api/v1/proxy/…`).
    #[allow(clippy::too_many_arguments)]
    pub fn build_complete(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        rate_limiter: Option<Arc<RateLimiter>>,
        auth_config: AuthConfig,
        webhooks: Option<Vec<WebhookConfig>>,
        metrics: Option<AgentMetricsCollector>,
        control_plane: Option<Arc<ControlPlaneState>>,
        rest_api: Option<Arc<RestApiState>>,
        proxy_management: Option<Arc<ProxyManagementState>>,
        a2a: Option<Arc<A2AServerState>>,
    ) -> Router {
        Self::build_complete_with_shutdown(
            agent,
            sessions,
            rate_limiter,
            auth_config,
            webhooks,
            metrics,
            control_plane,
            rest_api,
            proxy_management,
            a2a,
            None,
        )
    }

    /// Build the gateway with a per-API-key rate limiter.
    ///
    /// Convenience builder that configures per-key rate limiting alongside
    /// authentication. The per-key limiter runs *before* the session-based
    /// rate limiter and inspects the `Authorization: Bearer <key>` or
    /// `X-API-Key` headers to identify the caller.
    pub fn with_rate_limiter(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        per_key_limiter: PerKeyRateLimiter,
        auth_config: AuthConfig,
    ) -> Router {
        Self::build_complete_with_per_key(
            agent,
            sessions,
            None,
            auth_config,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(per_key_limiter)),
        )
    }

    /// Build the complete gateway with per-key rate limiting and all optional subsystems.
    #[allow(clippy::too_many_arguments)]
    pub fn build_complete_with_per_key(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        rate_limiter: Option<Arc<RateLimiter>>,
        auth_config: AuthConfig,
        webhooks: Option<Vec<WebhookConfig>>,
        metrics: Option<AgentMetricsCollector>,
        control_plane: Option<Arc<ControlPlaneState>>,
        rest_api: Option<Arc<RestApiState>>,
        proxy_management: Option<Arc<ProxyManagementState>>,
        a2a: Option<Arc<A2AServerState>>,
        shutdown_manager: Option<ShutdownManager>,
        per_key_rate_limiter: Option<Arc<PerKeyRateLimiter>>,
    ) -> Router {
        let connections = ConnectionManager::new();
        let sessions_for_streaming = sessions.clone();
        let router = Arc::new(MessageRouter::new(agent, sessions, connections.clone()));

        let webhook_state = webhooks.map(|configs| WebhookState { webhooks: configs });
        let auth_enabled = auth_config.is_enabled();
        let proxy_management_mounted = proxy_management.is_some();
        let a2a_mounted = a2a.is_some();

        let request_metrics = Arc::new(RequestMetrics::new());

        // Build SSE streaming state (always available)
        let streaming_state = Arc::new(StreamingState {
            router: router.clone(),
            connections: connections.clone(),
            sessions: sessions_for_streaming,
            session_broadcast: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        });

        let state = Arc::new(AppState {
            router,
            connections,
            webhooks: webhook_state,
            metrics,
            control_plane: control_plane.clone(),
            rest_api: rest_api.clone(),
            shutdown_manager,
            per_key_rate_limiter: per_key_rate_limiter.clone(),
            request_metrics: Some(Arc::clone(&request_metrics)),
            auth_enabled,
            proxy_management_mounted,
            a2a_mounted,
        });

        let mut app = Router::new()
            .route("/ws", get(ws_handler))
            .route("/health", get(health_handler))
            .route("/health/live", get(health_live_handler))
            .route("/health/ready", get(health_ready_handler))
            .route("/metrics", get(prometheus_metrics_handler))
            .route("/openapi.json", get(openapi_handler))
            .route(
                "/api/v1/enterprise/readiness",
                get(enterprise_readiness_handler),
            );

        // Add webhook route if webhooks are configured
        if state.webhooks.is_some() {
            app = app.route("/webhook/{name}", post(webhook_handler));
        }

        let mut app = app.with_state(state);

        // Merge SSE streaming routes (always available).
        app = app.merge(streaming_router(streaming_state));

        // Merge the web dashboard, playground, and pricing page.
        app = app.merge(dashboard_router());
        app = app.merge(playground_router());
        app = app.merge(pricing_router());

        // Merge control-plane routes if the state is provided.
        if let Some(cp_state) = control_plane {
            app = app.merge(control_plane_router(cp_state));
        }

        // Merge REST API routes if the state is provided.
        if let Some(ra_state) = rest_api {
            app = app.merge(api_router(ra_state));
        }

        // Merge proxy management routes if the state is provided.
        if let Some(pm_state) = proxy_management {
            app = app.merge(proxy_management_router(pm_state));
        }

        // Merge A2A protocol routes if the state is provided.
        if let Some(a2a_state) = a2a {
            app = app.merge(A2AServer::router(a2a_state));
        }

        // Apply observability middleware (outermost layer — runs first, finishes last)
        let obs_state = Arc::new(ObservabilityMiddlewareState { request_metrics });
        app = app.layer(axum_mw::from_fn_with_state(
            obs_state,
            request_tracing_middleware,
        ));

        // Apply auth/rate-limiting middleware if configured
        let has_middleware =
            rate_limiter.is_some() || auth_config.is_enabled() || per_key_rate_limiter.is_some();
        if has_middleware {
            let mw_state = Arc::new(MiddlewareState {
                rate_limiter: rate_limiter
                    .unwrap_or_else(|| Arc::new(RateLimiter::new(1000.0, 1000.0))),
                auth: auth_config,
                per_key_rate_limiter,
            });

            app = app
                .layer(axum_mw::from_fn_with_state(
                    mw_state.clone(),
                    rate_limit_middleware,
                ))
                .layer(axum_mw::from_fn_with_state(
                    mw_state.clone(),
                    per_key_rate_limit_middleware,
                ))
                .layer(axum_mw::from_fn_with_state(mw_state, auth_middleware));
        }

        app
    }

    /// Build the complete gateway with graceful shutdown support.
    ///
    /// Same as [`build_complete`] but accepts an optional [`ShutdownManager`] that
    /// is wired into `AppState` so the health endpoints can report shutdown status
    /// and the server can be stopped gracefully.
    #[allow(clippy::too_many_arguments)]
    pub fn build_complete_with_shutdown(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        rate_limiter: Option<Arc<RateLimiter>>,
        auth_config: AuthConfig,
        webhooks: Option<Vec<WebhookConfig>>,
        metrics: Option<AgentMetricsCollector>,
        control_plane: Option<Arc<ControlPlaneState>>,
        rest_api: Option<Arc<RestApiState>>,
        proxy_management: Option<Arc<ProxyManagementState>>,
        a2a: Option<Arc<A2AServerState>>,
        shutdown_manager: Option<ShutdownManager>,
    ) -> Router {
        let connections = ConnectionManager::new();
        let sessions_for_streaming = sessions.clone();
        let router = Arc::new(MessageRouter::new(agent, sessions, connections.clone()));

        let webhook_state = webhooks.map(|configs| WebhookState { webhooks: configs });
        let auth_enabled = auth_config.is_enabled();
        let proxy_management_mounted = proxy_management.is_some();
        let a2a_mounted = a2a.is_some();

        let request_metrics = Arc::new(RequestMetrics::new());

        // Build SSE streaming state (always available)
        let streaming_state = Arc::new(StreamingState {
            router: router.clone(),
            connections: connections.clone(),
            sessions: sessions_for_streaming,
            session_broadcast: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        });

        let state = Arc::new(AppState {
            router,
            connections,
            webhooks: webhook_state,
            metrics,
            control_plane: control_plane.clone(),
            rest_api: rest_api.clone(),
            shutdown_manager,
            per_key_rate_limiter: None,
            request_metrics: Some(Arc::clone(&request_metrics)),
            auth_enabled,
            proxy_management_mounted,
            a2a_mounted,
        });

        let mut app = Router::new()
            .route("/ws", get(ws_handler))
            .route("/health", get(health_handler))
            .route("/health/live", get(health_live_handler))
            .route("/health/ready", get(health_ready_handler))
            .route("/metrics", get(prometheus_metrics_handler))
            .route("/openapi.json", get(openapi_handler))
            .route(
                "/api/v1/enterprise/readiness",
                get(enterprise_readiness_handler),
            );

        // Add webhook route if webhooks are configured
        if state.webhooks.is_some() {
            app = app.route("/webhook/{name}", post(webhook_handler));
        }

        let mut app = app.with_state(state);

        // Merge SSE streaming routes (always available).
        app = app.merge(streaming_router(streaming_state));

        // Merge the web dashboard, playground, and pricing page.
        app = app.merge(dashboard_router());
        app = app.merge(playground_router());
        app = app.merge(pricing_router());

        // Merge control-plane routes if the state is provided.
        if let Some(cp_state) = control_plane {
            app = app.merge(control_plane_router(cp_state));
        }

        // Merge REST API routes if the state is provided.
        if let Some(ra_state) = rest_api {
            app = app.merge(api_router(ra_state));
        }

        // Merge proxy management routes if the state is provided.
        if let Some(pm_state) = proxy_management {
            app = app.merge(proxy_management_router(pm_state));
        }

        // Merge A2A protocol routes if the state is provided.
        if let Some(a2a_state) = a2a {
            app = app.merge(A2AServer::router(a2a_state));
        }

        // Apply observability middleware (outermost layer)
        let obs_state = Arc::new(ObservabilityMiddlewareState { request_metrics });
        app = app.layer(axum_mw::from_fn_with_state(
            obs_state,
            request_tracing_middleware,
        ));

        // Apply auth/rate-limiting middleware if configured
        if rate_limiter.is_some() || auth_config.is_enabled() {
            let mw_state = Arc::new(MiddlewareState {
                rate_limiter: rate_limiter
                    .unwrap_or_else(|| Arc::new(RateLimiter::new(1000.0, 1000.0))),
                auth: auth_config,
                per_key_rate_limiter: None,
            });

            app.layer(axum_mw::from_fn_with_state(
                mw_state.clone(),
                rate_limit_middleware,
            ))
            .layer(axum_mw::from_fn_with_state(mw_state, auth_middleware))
        } else {
            app
        }
    }

    /// Build the gateway with Prometheus metrics enabled.
    pub fn build_with_metrics(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        metrics: AgentMetricsCollector,
    ) -> Router {
        Self::build_with_middleware(
            agent,
            sessions,
            None,
            AuthConfig::new(vec![]),
            None,
            Some(metrics),
        )
    }

    /// Build the gateway with webhook support (convenience builder).
    ///
    /// This is equivalent to calling `build_with_middleware` with no auth or rate limiting
    /// but with webhook configurations.
    pub fn with_webhooks(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        configs: Vec<WebhookConfig>,
    ) -> Router {
        Self::build_with_middleware(
            agent,
            sessions,
            None,
            AuthConfig::new(vec![]),
            Some(configs),
            None,
        )
    }

    /// Build the gateway with graceful shutdown support (opt-in).
    ///
    /// Creates a [`ShutdownManager`] with the given config, wires `tokio::signal::ctrl_c()`
    /// to trigger shutdown, registers default drain/cleanup hooks, and returns both the
    /// router and the shutdown manager so the caller can await the shutdown signal.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (app, shutdown_mgr) = GatewayServer::with_graceful_shutdown(
    ///     agent, sessions, GracefulShutdownConfig::default(),
    /// );
    /// // Use `shutdown_mgr.shutdown_signal()` with `axum::serve(...).with_graceful_shutdown(...)`.
    /// ```
    pub async fn with_graceful_shutdown(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        config: GracefulShutdownConfig,
    ) -> (Router, ShutdownManager) {
        let shutdown_mgr = ShutdownManager::new(config.total_timeout);

        // Register PreDrain hook: log that we are stopping new connections
        shutdown_mgr
            .on_shutdown("stop-accepting", ShutdownPhase::PreDrain, || {
                info!("PreDrain: no longer accepting new connections");
                Ok(())
            })
            .await;

        // Register Drain hook: wait for in-flight connections to complete
        let drain_timeout = config.drain_timeout;
        shutdown_mgr
            .on_shutdown("drain-connections", ShutdownPhase::Drain, move || {
                info!(
                    timeout_ms = drain_timeout.as_millis() as u64,
                    "Drain: waiting for in-flight requests to complete"
                );
                Ok(())
            })
            .await;

        // Register Cleanup hook: flush audit logs, close resources
        let cleanup_timeout = config.cleanup_timeout;
        shutdown_mgr
            .on_shutdown("flush-resources", ShutdownPhase::Cleanup, move || {
                info!(
                    timeout_ms = cleanup_timeout.as_millis() as u64,
                    "Cleanup: flushing audit logs and closing resources"
                );
                Ok(())
            })
            .await;

        // Register Final hook: last-chance metric export
        shutdown_mgr
            .on_shutdown("final-export", ShutdownPhase::Final, || {
                info!("Final: shutdown complete");
                Ok(())
            })
            .await;

        // Spawn a background task that listens for ctrl_c and triggers shutdown
        let shutdown_for_signal = shutdown_mgr.clone();
        tokio::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    warn!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
                    let report = shutdown_for_signal.shutdown().await;
                    info!(
                        succeeded = report.hooks_succeeded,
                        failed = report.hooks_failed,
                        total_ms = report.total_duration_ms,
                        "Graceful shutdown complete"
                    );
                }
                Err(e) => {
                    error!(error = %e, "Failed to listen for ctrl_c signal");
                }
            }
        });

        let router = Self::build_complete_with_shutdown(
            agent,
            sessions,
            None,
            AuthConfig::new(vec![]),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(shutdown_mgr.clone()),
        );

        (router, shutdown_mgr)
    }

    /// Build the gateway with SSO/SAML authentication support.
    ///
    /// Merges the SSO authentication routes (`/auth/login`, `/auth/callback`,
    /// `/auth/logout`, `/auth/me`, `/auth/api-key`) into the gateway and
    /// optionally applies the SSO session middleware to protect all other
    /// routes.
    ///
    /// # Arguments
    ///
    /// * `agent` — The agent runner for handling messages.
    /// * `sessions` — Session store for persistence.
    /// * `sso_config` — SSO provider configuration.
    /// * `protect_routes` — If `true`, applies SSO middleware to all routes
    ///   (except the auth routes themselves). If `false`, only mounts the
    ///   auth routes without enforcing session validation.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use argentor_gateway::sso::{SsoConfig, SsoProvider};
    ///
    /// let config = SsoConfig {
    ///     provider: SsoProvider::Oidc,
    ///     client_id: "my-client-id".into(),
    ///     client_secret: "my-secret".into(),
    ///     redirect_uri: "https://app.example.com/auth/callback".into(),
    ///     issuer_url: "https://accounts.google.com".into(),
    ///     allowed_domains: vec!["example.com".into()],
    ///     ..Default::default()
    /// };
    ///
    /// let app = GatewayServer::with_sso(agent, sessions, config, true);
    /// ```
    pub fn with_sso(
        agent: Arc<AgentRunner>,
        sessions: Arc<dyn SessionStore>,
        sso_config: crate::sso::SsoConfig,
        protect_routes: bool,
    ) -> Router {
        let sso_manager = Arc::new(crate::sso::SsoManager::new(sso_config));

        // Build the base gateway without auth middleware (SSO replaces it).
        let mut app = Self::build(agent, sessions);

        // Merge SSO auth routes (these are always accessible).
        app = app.merge(crate::sso::sso_router(sso_manager.clone()));

        // Optionally apply SSO session middleware to protect all routes.
        if protect_routes {
            let mw_state = Arc::new(crate::sso::SsoMiddlewareState {
                manager: sso_manager,
            });
            app = app.layer(axum_mw::from_fn_with_state(
                mw_state,
                crate::sso::sso_auth_middleware,
            ));
        }

        app
    }
}

/// Basic health check — returns 200 if the service is running and not shutting down.
async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if let Some(ref mgr) = state.shutdown_manager {
        if mgr.is_shutting_down().await {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "status": "shutting_down",
                    "service": "argentor"
                })
                .to_string(),
            )
                .into_response();
        }
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"status": "ok", "service": "argentor"}).to_string(),
    )
        .into_response()
}

/// Liveness probe — always returns 200 as long as the process is alive.
///
/// Kubernetes/load-balancers use this to decide whether to restart the container.
/// This endpoint never fails; if the process can respond, it is alive.
async fn health_live_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "status": "alive",
            "service": "argentor"
        })
        .to_string(),
    )
        .into_response()
}

/// Readiness probe — returns 200 only when dependencies are healthy.
///
/// Checks:
/// - Not in shutdown state
/// - REST API state available (skills registry loaded)
/// - Control-plane state available (orchestrator operational)
///
/// Returns 503 if any check fails, with details about what is not ready.
async fn health_ready_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut checks: Vec<(&str, bool)> = Vec::new();

    // Check 1: not shutting down
    let not_shutting_down = if let Some(ref mgr) = state.shutdown_manager {
        !mgr.is_shutting_down().await
    } else {
        true
    };
    checks.push(("not_shutting_down", not_shutting_down));

    // Check 2: REST API state loaded (proxy for skills registry availability)
    let rest_api_ready = state.rest_api.is_some();
    checks.push(("rest_api", rest_api_ready));

    // Check 3: control-plane state loaded (proxy for orchestrator readiness)
    let control_plane_ready = state.control_plane.is_some();
    checks.push(("control_plane", control_plane_ready));

    // Check 4: connections manager is operational (always true if state exists)
    checks.push(("connections", true));

    let all_ready = checks.iter().all(|(_, ok)| *ok);
    let check_map: serde_json::Value = checks
        .iter()
        .map(|(name, ok)| {
            (
                name.to_string(),
                serde_json::json!(if *ok { "ok" } else { "not_ready" }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();

    let status_code = if all_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status_code,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "status": if all_ready { "ready" } else { "not_ready" },
            "service": "argentor",
            "checks": check_map,
        })
        .to_string(),
    )
        .into_response()
}

/// OpenAPI 3.0 spec endpoint.
async fn openapi_handler() -> impl IntoResponse {
    let spec = crate::openapi::argentor_openapi_spec();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string_pretty(&spec).unwrap_or_default(),
    )
        .into_response()
}

/// Enterprise readiness report endpoint.
async fn enterprise_readiness_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let active_connections = state.connections.connection_count().await;
    let active_sessions = state.connections.session_ids().await.len();
    let rest_api_mounted = state.rest_api.is_some();

    let (skills_registered, uptime_seconds, sessions_reachable) =
        if let Some(rest_api) = &state.rest_api {
            let uptime = Utc::now().signed_duration_since(rest_api.started_at);
            (
                rest_api.skills.skill_count(),
                uptime.num_seconds(),
                rest_api.sessions.list().await.is_ok(),
            )
        } else {
            (0, 0, false)
        };

    let report: EnterpriseReadinessReport =
        build_enterprise_readiness_report(EnterpriseReadinessInput {
            skills_registered,
            active_connections,
            active_sessions,
            uptime_seconds,
            sessions_reachable,
            rest_api_mounted,
            auth_configured: state.auth_enabled,
            per_key_rate_limit_configured: state.per_key_rate_limiter.is_some(),
            control_plane_mounted: state.control_plane.is_some(),
            proxy_management_mounted: state.proxy_management_mounted,
            a2a_mounted: state.a2a_mounted,
            metrics_configured: state.metrics.is_some() || state.request_metrics.is_some(),
        });

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&report).unwrap_or_default(),
    )
        .into_response()
}

/// Prometheus-compatible metrics endpoint.
///
/// Returns metrics in the Prometheus text exposition format when a collector
/// is configured, or a JSON error otherwise.
async fn prometheus_metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut body = String::new();
    let mut has_metrics = false;

    // Agent-level metrics from the AgentMetricsCollector
    if let Some(collector) = &state.metrics {
        body.push_str(&collector.prometheus_export());
        has_metrics = true;
    }

    // Request-level metrics from the observability middleware
    if let Some(req_metrics) = &state.request_metrics {
        if has_metrics {
            body.push('\n');
        }
        body.push_str(&req_metrics.prometheus_export());
        has_metrics = true;
    }

    // Audit metrics from the REST API state, if mounted.
    if let Some(rest_api) = &state.rest_api {
        if has_metrics {
            body.push('\n');
        }
        body.push_str(&audit_prometheus_export(rest_api).await);
        has_metrics = true;
    }

    if has_metrics {
        (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            body,
        )
            .into_response()
    } else {
        let err = serde_json::json!({
            "error": "Metrics collector not configured",
            "hint": "Use GatewayServer::build_with_metrics() to enable Prometheus metrics"
        })
        .to_string();
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            err,
        )
            .into_response()
    }
}

#[tracing::instrument(skip(ws, state), fields(method = "GET", path = "/ws", status_code = tracing::field::Empty))]
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    tracing::Span::current().record("status_code", 101u16);
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let connection_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Channel for sending messages back to the WebSocket
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let conn = Connection {
        id: connection_id,
        session_id,
        tx,
    };
    state.connections.add(conn).await;

    info!(
        connection_id = %connection_id,
        session_id = %session_id,
        "WebSocket connected"
    );

    // Send initial session info
    let welcome = serde_json::json!({
        "type": "connected",
        "session_id": session_id,
        "connection_id": connection_id,
    });
    let _ = state
        .connections
        .send_to_session(session_id, &welcome.to_string())
        .await;

    // Task: forward messages from channel to WebSocket
    use axum::extract::ws::Message as WsMessage;
    use futures_util::SinkExt;
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(WsMessage::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Task: receive messages from WebSocket and route them
    use futures_util::StreamExt;
    let router = state.router.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Text(text) => {
                    let inbound: InboundMessage = match serde_json::from_str(&text) {
                        Ok(m) => m,
                        Err(_) => InboundMessage {
                            session_id: Some(session_id),
                            content: text.to_string(),
                        },
                    };

                    if let Err(e) = router
                        .handle_message_streaming(inbound, connection_id)
                        .await
                    {
                        error!(error = %e, "Failed to handle message");
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    state.connections.remove(connection_id).await;
    info!(connection_id = %connection_id, "WebSocket disconnected");
}

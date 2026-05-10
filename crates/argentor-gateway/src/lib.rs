//! HTTP/WebSocket gateway with authentication, rate limiting, and webhook support.
//!
//! This crate provides the public-facing server that accepts HTTP and WebSocket
//! connections, applies authentication and rate-limiting middleware, and routes
//! requests to the agent runner.
//!
//! # Main types
//!
//! - [`GatewayServer`] — Builds and starts the Axum-based HTTP/WS server.
//! - [`AuthConfig`] — API-key authentication configuration.
//! - [`ConnectionManager`] — Tracks active WebSocket connections.
//! - [`MessageRouter`] — Routes inbound messages to the appropriate handler.

/// Business analytics endpoints for the SaaS dashboard.
pub mod analytics;
/// JWT/OAuth2 authentication module.
pub mod auth;
/// Billing integration with webhook endpoints and plan enforcement.
pub mod billing;
/// Bridge between ChannelManager and the gateway pipeline.
pub mod channel_bridge;
/// WebSocket connection management.
pub mod connection;
/// REST API endpoints for the orchestrator control plane.
pub mod control_plane;
/// Embedded web dashboard for monitoring and management.
pub mod dashboard;
/// Enterprise readiness report for the secure gateway product surface.
pub mod enterprise_readiness;
/// Graceful shutdown manager with cleanup hooks and connection draining.
pub mod graceful_shutdown;
/// REST API endpoints for the skill marketplace.
pub mod marketplace_api;
/// Authentication and rate-limiting middleware.
pub mod middleware;
/// End-to-end observability: tracing, metrics, and request instrumentation.
pub mod observability;
/// OpenAPI 3.0 specification generator for Argentor REST API.
pub mod openapi;
/// JSON file-based persistence for control plane state.
pub mod persistence;
/// Interactive web-based agent playground for testing agents in a browser.
pub mod playground;
/// SaaS pricing page for plan comparison and signup.
pub mod pricing_page;
/// REST API endpoints for managing credentials, token pools, and the proxy orchestrator.
pub mod proxy_management;
/// X-RateLimit-* response headers for API consumers.
pub mod rate_limit_headers;
/// Per-API-key rate limiting with sliding windows and token quotas.
pub mod rate_limit_per_key;
/// Multi-region routing for LLM requests based on data residency rules.
pub mod region_router;
/// REST API endpoints for managing agents, sessions, skills, and connections.
pub mod rest_api;
/// HTTP and WebSocket route definitions.
pub mod router;
/// Gateway server builder and runner.
pub mod server;
/// SSO/SAML authentication module for enterprise single sign-on.
pub mod sso;
/// Server-Sent Events streaming for real-time agent conversations.
pub mod streaming;
/// Trace visualization system for debugging agent execution.
pub mod trace_viewer;
/// Enhanced trace visualization with timeline, Mermaid gantt, and flame graph output.
pub mod trace_viz;
/// Webhook endpoint configuration and handling.
pub mod webhook;
/// Outbound webhook notification system.
pub mod webhook_outbound;
/// WebSocket-based human approval channel.
pub mod ws_approval;
/// XcapitSFF integration — agent execution, webhook proxy, health checks.
pub mod xcapitsff;

pub use analytics::{
    analytics_router, default_pricing, AgentPerformance, AnalyticsDashboard, AnalyticsEngine,
    AnalyticsState, ConversionFunnel, DailyMetric, FunnelStage, InteractionEvent,
    InteractionOutcome, ModelPricing, PricingTable, QualityEvent,
};
pub use auth::{
    AuthConfig as JwtAuthConfig, AuthMiddlewareState, AuthMode, AuthService, AuthenticatedUser,
    JwtClaims,
};
pub use billing::{
    billing_router, BillingManager, BillingPlan, BillingState, Invoice, InvoiceLineItem,
    InvoiceStatus, Subscription, SubscriptionStatus, WebhookProcessResult,
};
pub use channel_bridge::ChannelBridge;
pub use control_plane::{control_plane_router, ControlPlaneState};
pub use dashboard::dashboard_router;
pub use enterprise_readiness::{
    build_enterprise_readiness_report, EnterpriseReadinessCheck, EnterpriseReadinessInput,
    EnterpriseReadinessReport, EnterpriseRuntimeSnapshot, ReadinessPosture, ReadinessStatus,
};
pub use graceful_shutdown::{
    HookResult, ShutdownHook, ShutdownManager, ShutdownPhase, ShutdownReport,
};
pub use marketplace_api::{marketplace_router, MarketplaceApiState};
pub use middleware::AuthConfig;
pub use observability::{
    request_tracing_middleware, ObservabilityConfig, ObservabilityMiddlewareState,
    ObservabilityStack, RequestMetrics,
};
pub use openapi::{
    argentor_openapi_spec, ApiEndpoint, ApiParameter, ApiResponse, HttpMethod, OpenApiGenerator,
};
pub use persistence::PersistentStore;
pub use playground::playground_router;
pub use pricing_page::pricing_router;
pub use proxy_management::{proxy_management_router, ProxyManagementState};
pub use rate_limit_headers::{RateLimitHeaders, RateLimitInfo};
pub use rate_limit_per_key::{
    DenyReason, KeyUsageStats, PerKeyRateLimiter, RateLimitConfig, RateLimitResult,
};
pub use region_router::{DataClassification, RegionRouter, RegionRule, RoutingDecision};
pub use rest_api::{api_router, RestApiState};
pub use server::{GatewayServer, GracefulShutdownConfig};
pub use sso::{
    sso_auth_middleware, sso_router, SsoConfig, SsoManager, SsoMiddlewareState, SsoProvider,
    SsoState, UserIdentity,
};
pub use streaming::{
    sse_chat_handler, stream_event_to_sse, streaming_router, SseEvent, StreamRequest,
    StreamingState,
};
pub use trace_viewer::{
    trace_viewer_router, CostBreakdown, StepCost, TimelineLane, TraceFilter, TraceStore,
    TraceSummary as TraceViewerSummary, TraceTimeline, TraceViewerState,
};
pub use webhook::{SessionStrategy, WebhookConfig, WebhookState};
pub use ws_approval::WsApprovalChannel;
pub use xcapitsff::{
    default_xcapit_profiles, xcapitsff_router, BatchRequest, BatchResponse, RunTaskRequest,
    RunTaskResponse, XcapitConfig, XcapitState,
};

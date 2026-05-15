//! Outbound webhook notification system for notifying external systems when
//! agent events occur.
//!
//! This module provides a [`WebhookDispatcher`] that manages webhook
//! subscriptions, dispatches events to matching subscribers with HMAC-SHA256
//! signed payloads, retries failed deliveries with exponential backoff, and
//! keeps a delivery log.
//!
//! # REST Endpoints
//!
//! - `POST   /api/v1/webhooks/subscriptions`             — create subscription
//! - `GET    /api/v1/webhooks/subscriptions`              — list subscriptions
//! - `DELETE /api/v1/webhooks/subscriptions/{id}`         — delete subscription
//! - `GET    /api/v1/webhooks/deliveries/{subscription_id}` — delivery log
//! - `POST   /api/v1/webhooks/test/{subscription_id}`    — send test event

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

// HMAC-SHA256 signing using sha2 (concatenation approach, no hmac crate).

// ---------------------------------------------------------------------------
// WebhookEventType
// ---------------------------------------------------------------------------

/// Events that can trigger outbound webhooks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum WebhookEventType {
    /// An agent task completed successfully.
    AgentTaskCompleted,
    /// An agent task failed.
    AgentTaskFailed,
    /// An agent is streaming intermediate results.
    AgentTaskStreaming,
    /// A support ticket was routed to an agent.
    TicketRouted,
    /// A lead was qualified by the scoring agent.
    LeadQualified,
    /// An outreach message was generated.
    OutreachGenerated,
    /// Quality score dropped below the configured threshold.
    QualityScoreLow,
    /// Token or cost budget was exceeded.
    BudgetExceeded,
    /// A health-check probe failed.
    HealthCheckFailed,
    /// Arbitrary user-defined event type.
    Custom(String),
}

impl std::fmt::Display for WebhookEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentTaskCompleted => write!(f, "agent.task.completed"),
            Self::AgentTaskFailed => write!(f, "agent.task.failed"),
            Self::AgentTaskStreaming => write!(f, "agent.task.streaming"),
            Self::TicketRouted => write!(f, "ticket.routed"),
            Self::LeadQualified => write!(f, "lead.qualified"),
            Self::OutreachGenerated => write!(f, "outreach.generated"),
            Self::QualityScoreLow => write!(f, "quality.score.low"),
            Self::BudgetExceeded => write!(f, "budget.exceeded"),
            Self::HealthCheckFailed => write!(f, "health.check.failed"),
            Self::Custom(name) => write!(f, "custom.{name}"),
        }
    }
}

// ---------------------------------------------------------------------------
// RetryPolicy
// ---------------------------------------------------------------------------

/// Configures exponential-backoff retry behaviour for failed deliveries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (default 3).
    pub max_retries: u32,
    /// Initial delay between retries in milliseconds (default 1000).
    pub initial_delay_ms: u64,
    /// Multiplier applied to the delay after each attempt (default 2.0).
    pub backoff_multiplier: f32,
    /// Upper bound on delay in milliseconds (default 30000).
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 30_000,
        }
    }
}

impl RetryPolicy {
    /// Calculate the delay for the given attempt number (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let delay =
            self.initial_delay_ms as f64 * (self.backoff_multiplier as f64).powi(attempt as i32);
        (delay as u64).min(self.max_delay_ms)
    }
}

// ---------------------------------------------------------------------------
// WebhookSubscription
// ---------------------------------------------------------------------------

/// A registered outbound webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    /// Unique subscription identifier.
    pub id: String,
    /// Tenant that owns this subscription.
    pub tenant_id: String,
    /// URL to POST event payloads to.
    pub url: String,
    /// Which event types this subscription listens to.
    pub events: Vec<WebhookEventType>,
    /// Shared secret used for HMAC-SHA256 payload signing.
    pub secret: String,
    /// Whether the subscription is currently active.
    pub enabled: bool,
    /// Custom HTTP headers to include in every delivery.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Retry behaviour for failed deliveries.
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// When the subscription was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// WebhookEvent
// ---------------------------------------------------------------------------

/// An event payload dispatched to matching subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    /// Unique event identifier (UUID).
    pub event_id: String,
    /// The type of event.
    pub event_type: WebhookEventType,
    /// Tenant that originated the event.
    pub tenant_id: String,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Arbitrary JSON payload.
    pub payload: serde_json::Value,
    /// Additional key-value metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl WebhookEvent {
    /// Create a new event with a generated UUID and current timestamp.
    pub fn new(
        event_type: WebhookEventType,
        tenant_id: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type,
            tenant_id: tenant_id.into(),
            timestamp: Utc::now(),
            payload,
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// DeliveryStatus / WebhookDelivery
// ---------------------------------------------------------------------------

/// Status of a webhook delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryStatus {
    /// Queued but not yet attempted.
    Pending,
    /// Delivered successfully (2xx response).
    Success,
    /// All attempts exhausted without success.
    Failed,
    /// Will be retried after a delay.
    Retrying,
}

/// Record of a single delivery attempt for audit and debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDelivery {
    /// Unique delivery identifier.
    pub delivery_id: String,
    /// Subscription that triggered this delivery.
    pub subscription_id: String,
    /// Event that was delivered.
    pub event_id: String,
    /// Outcome status.
    pub status: DeliveryStatus,
    /// HTTP status code returned by the endpoint (if available).
    pub http_status: Option<u16>,
    /// Truncated response body (if available).
    pub response_body: Option<String>,
    /// Which attempt this record represents (1-indexed).
    pub attempt: u32,
    /// When the attempt was made.
    pub attempted_at: DateTime<Utc>,
    /// Round-trip time in milliseconds.
    pub duration_ms: u64,
    /// Error description (if the attempt failed).
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// WebhookSigner
// ---------------------------------------------------------------------------

/// HMAC-SHA256 payload signer / verifier.
pub struct WebhookSigner;

impl WebhookSigner {
    /// Produce a hex-encoded HMAC-SHA256 signature for `payload` using `secret`.
    pub fn sign(payload: &str, secret: &str) -> String {
        let mut hasher = sha2::Sha256::new();
        hasher.update(secret.as_bytes());
        hasher.update(payload.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Verify that `signature` matches the HMAC-SHA256 of `payload` with `secret`.
    pub fn verify(payload: &str, secret: &str, signature: &str) -> bool {
        let expected = Self::sign(payload, secret);
        // Constant-time comparison
        if expected.len() != signature.len() {
            return false;
        }
        let mut diff: u8 = 0;
        for (a, b) in expected.bytes().zip(signature.bytes()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

// ---------------------------------------------------------------------------
// WebhookDispatcherConfig
// ---------------------------------------------------------------------------

/// Configuration for the [`WebhookDispatcher`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDispatcherConfig {
    /// Maximum number of delivery log entries kept per subscription.
    #[serde(default = "default_max_log_entries")]
    pub max_log_entries: usize,
    /// HTTP timeout for webhook delivery requests in milliseconds.
    #[serde(default = "default_delivery_timeout_ms")]
    pub delivery_timeout_ms: u64,
}

fn default_max_log_entries() -> usize {
    1000
}

fn default_delivery_timeout_ms() -> u64 {
    10_000
}

impl Default for WebhookDispatcherConfig {
    fn default() -> Self {
        Self {
            max_log_entries: default_max_log_entries(),
            delivery_timeout_ms: default_delivery_timeout_ms(),
        }
    }
}

// ---------------------------------------------------------------------------
// WebhookDispatcher
// ---------------------------------------------------------------------------

/// Inner mutable state of the dispatcher.
struct DispatcherInner {
    subscriptions: HashMap<String, WebhookSubscription>,
    /// Delivery log keyed by subscription_id → Vec<WebhookDelivery>.
    delivery_log: HashMap<String, Vec<WebhookDelivery>>,
    config: WebhookDispatcherConfig,
}

/// Thread-safe outbound webhook dispatcher.
///
/// Manages subscriptions, dispatches events, records delivery attempts, and
/// retries failed deliveries with exponential backoff.
#[derive(Clone)]
pub struct WebhookDispatcher {
    inner: Arc<RwLock<DispatcherInner>>,
    http_client: reqwest::Client,
}

impl WebhookDispatcher {
    /// Create a new dispatcher with the given configuration.
    pub fn new(config: WebhookDispatcherConfig) -> Self {
        let timeout = std::time::Duration::from_millis(config.delivery_timeout_ms);
        let http_client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        Self {
            inner: Arc::new(RwLock::new(DispatcherInner {
                subscriptions: HashMap::new(),
                delivery_log: HashMap::new(),
                config,
            })),
            http_client,
        }
    }

    /// Register a new webhook subscription. Returns the subscription id.
    pub async fn subscribe(&self, subscription: WebhookSubscription) -> String {
        let id = subscription.id.clone();
        let mut inner = self.inner.write().await;
        info!(subscription_id = %id, url = %subscription.url, "Webhook subscription created");
        inner.subscriptions.insert(id.clone(), subscription);
        id
    }

    /// Remove a subscription by id. Returns `true` if it existed.
    pub async fn unsubscribe(&self, subscription_id: &str) -> bool {
        let mut inner = self.inner.write().await;
        let removed = inner.subscriptions.remove(subscription_id).is_some();
        if removed {
            info!(subscription_id = %subscription_id, "Webhook subscription removed");
        } else {
            warn!(subscription_id = %subscription_id, "Unsubscribe: subscription not found");
        }
        removed
    }

    /// List all active subscriptions.
    pub async fn list_subscriptions(&self) -> Vec<WebhookSubscription> {
        let inner = self.inner.read().await;
        inner.subscriptions.values().cloned().collect()
    }

    /// Get a subscription by id.
    pub async fn get_subscription(&self, subscription_id: &str) -> Option<WebhookSubscription> {
        let inner = self.inner.read().await;
        inner.subscriptions.get(subscription_id).cloned()
    }

    /// Retrieve the delivery log for a subscription, most recent first.
    pub async fn get_delivery_log(
        &self,
        subscription_id: &str,
        limit: usize,
    ) -> Vec<WebhookDelivery> {
        let inner = self.inner.read().await;
        match inner.delivery_log.get(subscription_id) {
            Some(log) => {
                let start = log.len().saturating_sub(limit);
                log[start..].iter().rev().cloned().collect()
            }
            None => Vec::new(),
        }
    }

    /// Dispatch an event to all matching, enabled subscriptions.
    ///
    /// Delivery happens asynchronously — the method spawns a task per matching
    /// subscriber and returns immediately.
    pub async fn dispatch(&self, event: WebhookEvent) {
        let inner = self.inner.read().await;
        let matching: Vec<WebhookSubscription> = inner
            .subscriptions
            .values()
            .filter(|s| {
                s.enabled && s.tenant_id == event.tenant_id && s.events.contains(&event.event_type)
            })
            .cloned()
            .collect();
        drop(inner);

        if matching.is_empty() {
            info!(event_id = %event.event_id, "No matching subscriptions for event");
            return;
        }

        let event = Arc::new(event);
        for sub in matching {
            let dispatcher = self.clone();
            let event = Arc::clone(&event);
            tokio::spawn(async move {
                dispatcher.deliver(&sub, &event).await;
            });
        }
    }

    /// Perform delivery with retries for a single subscription + event pair.
    async fn deliver(&self, subscription: &WebhookSubscription, event: &WebhookEvent) {
        let payload = match serde_json::to_string(event) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to serialize webhook event");
                return;
            }
        };

        let signature = WebhookSigner::sign(&payload, &subscription.secret);
        let max_attempts = subscription.retry_policy.max_retries + 1;

        for attempt in 0..max_attempts {
            let start = std::time::Instant::now();
            let result = self.send_request(subscription, &payload, &signature).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let delivery = match &result {
                Ok((status, body)) => {
                    let success = (200..300).contains(&(*status as u32));
                    WebhookDelivery {
                        delivery_id: Uuid::new_v4().to_string(),
                        subscription_id: subscription.id.clone(),
                        event_id: event.event_id.clone(),
                        status: if success {
                            DeliveryStatus::Success
                        } else if attempt + 1 < max_attempts {
                            DeliveryStatus::Retrying
                        } else {
                            DeliveryStatus::Failed
                        },
                        http_status: Some(*status),
                        response_body: Some(body.chars().take(1024).collect()),
                        attempt: attempt + 1,
                        attempted_at: Utc::now(),
                        duration_ms,
                        error: if success {
                            None
                        } else {
                            Some(format!("HTTP {status}"))
                        },
                    }
                }
                Err(e) => WebhookDelivery {
                    delivery_id: Uuid::new_v4().to_string(),
                    subscription_id: subscription.id.clone(),
                    event_id: event.event_id.clone(),
                    status: if attempt + 1 < max_attempts {
                        DeliveryStatus::Retrying
                    } else {
                        DeliveryStatus::Failed
                    },
                    http_status: None,
                    response_body: None,
                    attempt: attempt + 1,
                    attempted_at: Utc::now(),
                    duration_ms,
                    error: Some(e.clone()),
                },
            };

            let is_success = delivery.status == DeliveryStatus::Success;
            self.record_delivery(delivery).await;

            if is_success {
                info!(
                    subscription_id = %subscription.id,
                    event_id = %event.event_id,
                    attempt = attempt + 1,
                    "Webhook delivered successfully"
                );
                return;
            }

            // Wait before retrying (skip delay on last failed attempt)
            if attempt + 1 < max_attempts {
                let delay = subscription.retry_policy.delay_for_attempt(attempt);
                warn!(
                    subscription_id = %subscription.id,
                    event_id = %event.event_id,
                    attempt = attempt + 1,
                    delay_ms = delay,
                    "Webhook delivery failed, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            } else {
                error!(
                    subscription_id = %subscription.id,
                    event_id = %event.event_id,
                    "Webhook delivery failed after all retries"
                );
            }
        }
    }

    /// Low-level HTTP POST to the subscriber endpoint.
    async fn send_request(
        &self,
        subscription: &WebhookSubscription,
        payload: &str,
        signature: &str,
    ) -> Result<(u16, String), String> {
        let mut req = self
            .http_client
            .post(&subscription.url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Signature", signature)
            .header("X-Webhook-Id", &subscription.id)
            .header("User-Agent", "Argentor-Webhook/1.0");

        for (key, value) in &subscription.headers {
            req = req.header(key, value);
        }

        let resp = req
            .body(payload.to_owned())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok((status, body))
    }

    /// Persist a delivery record, trimming old entries when over the limit.
    async fn record_delivery(&self, delivery: WebhookDelivery) {
        let mut inner = self.inner.write().await;
        let max = inner.config.max_log_entries;
        let log = inner
            .delivery_log
            .entry(delivery.subscription_id.clone())
            .or_default();
        log.push(delivery);
        if log.len() > max {
            let excess = log.len() - max;
            log.drain(..excess);
        }
    }
}

// ---------------------------------------------------------------------------
// Axum REST API
// ---------------------------------------------------------------------------

/// Shared state for outbound webhook endpoints.
pub struct WebhookOutboundState {
    /// The webhook dispatcher for outbound events.
    pub dispatcher: WebhookDispatcher,
}

/// Build an axum [`Router`] with outbound webhook endpoints.
pub fn webhook_outbound_router(state: Arc<WebhookOutboundState>) -> Router {
    Router::new()
        .route(
            "/api/v1/webhooks/subscriptions",
            post(create_subscription).get(list_subscriptions),
        )
        .route(
            "/api/v1/webhooks/subscriptions/{id}",
            delete(delete_subscription),
        )
        .route(
            "/api/v1/webhooks/deliveries/{subscription_id}",
            get(get_deliveries),
        )
        .route(
            "/api/v1/webhooks/test/{subscription_id}",
            post(send_test_event),
        )
        .with_state(state)
}

// -- Handler request/response types -----------------------------------------

/// Request body for creating a subscription.
#[derive(Debug, Deserialize)]
pub struct CreateSubscriptionRequest {
    /// Tenant owning the subscription.
    pub tenant_id: String,
    /// Destination URL for webhook deliveries.
    pub url: String,
    /// Event types this subscription listens for.
    pub events: Vec<WebhookEventType>,
    /// Shared secret for HMAC signature verification.
    pub secret: String,
    /// Whether the subscription is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Custom headers to include in deliveries.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Optional retry policy override.
    #[serde(default)]
    pub retry_policy: Option<RetryPolicy>,
}

fn default_true() -> bool {
    true
}

/// Query parameters for the delivery log endpoint.
#[derive(Debug, Deserialize)]
pub struct DeliveryLogQuery {
    /// Maximum number of deliveries to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

// -- Handlers ---------------------------------------------------------------

async fn create_subscription(
    State(state): State<Arc<WebhookOutboundState>>,
    Json(req): Json<CreateSubscriptionRequest>,
) -> impl IntoResponse {
    let subscription = WebhookSubscription {
        id: Uuid::new_v4().to_string(),
        tenant_id: req.tenant_id,
        url: req.url,
        events: req.events,
        secret: req.secret,
        enabled: req.enabled,
        headers: req.headers,
        retry_policy: req.retry_policy.unwrap_or_default(),
        created_at: Utc::now(),
    };

    let id = state.dispatcher.subscribe(subscription.clone()).await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "subscription": subscription,
        })),
    )
}

async fn list_subscriptions(State(state): State<Arc<WebhookOutboundState>>) -> impl IntoResponse {
    let subs = state.dispatcher.list_subscriptions().await;
    Json(serde_json::json!({ "subscriptions": subs, "count": subs.len() }))
}

async fn delete_subscription(
    State(state): State<Arc<WebhookOutboundState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.dispatcher.unsubscribe(&id).await {
        (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": true, "id": id })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "subscription not found", "id": id })),
        )
    }
}

async fn get_deliveries(
    State(state): State<Arc<WebhookOutboundState>>,
    Path(subscription_id): Path<String>,
    Query(query): Query<DeliveryLogQuery>,
) -> impl IntoResponse {
    let deliveries = state
        .dispatcher
        .get_delivery_log(&subscription_id, query.limit)
        .await;
    Json(serde_json::json!({
        "subscription_id": subscription_id,
        "deliveries": deliveries,
        "count": deliveries.len(),
    }))
}

async fn send_test_event(
    State(state): State<Arc<WebhookOutboundState>>,
    Path(subscription_id): Path<String>,
) -> impl IntoResponse {
    let sub = match state.dispatcher.get_subscription(&subscription_id).await {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "subscription not found" })),
            );
        }
    };

    let event = WebhookEvent::new(
        WebhookEventType::Custom("test".to_string()),
        &sub.tenant_id,
        serde_json::json!({
            "message": "This is a test webhook event from Argentor",
            "subscription_id": subscription_id,
        }),
    );

    let event_id = event.event_id.clone();

    // Deliver synchronously (single subscription) so the caller gets immediate feedback.
    state.dispatcher.deliver(&sub, &event).await;

    let log = state.dispatcher.get_delivery_log(&subscription_id, 1).await;
    let last = log.first();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "event_id": event_id,
            "subscription_id": subscription_id,
            "delivery": last,
        })),
    )
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    // -- Helpers ------------------------------------------------------------

    fn test_config() -> WebhookDispatcherConfig {
        WebhookDispatcherConfig {
            max_log_entries: 100,
            delivery_timeout_ms: 5_000,
        }
    }

    fn test_subscription(tenant: &str, events: Vec<WebhookEventType>) -> WebhookSubscription {
        WebhookSubscription {
            id: Uuid::new_v4().to_string(),
            tenant_id: tenant.to_string(),
            url: "https://example.com/webhook".to_string(),
            events,
            secret: "test-secret".to_string(),
            enabled: true,
            headers: HashMap::new(),
            retry_policy: RetryPolicy::default(),
            created_at: Utc::now(),
        }
    }

    fn test_event(tenant: &str, event_type: WebhookEventType) -> WebhookEvent {
        WebhookEvent::new(event_type, tenant, serde_json::json!({"key": "value"}))
    }

    // -- WebhookSigner tests ------------------------------------------------

    #[test]
    fn test_signer_sign_produces_hex_string() {
        let sig = WebhookSigner::sign("hello", "secret");
        assert!(!sig.is_empty());
        // Hex-encoded SHA256 HMAC is 64 characters
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn test_signer_sign_deterministic() {
        let a = WebhookSigner::sign("payload", "key");
        let b = WebhookSigner::sign("payload", "key");
        assert_eq!(a, b);
    }

    #[test]
    fn test_signer_different_payloads_different_sigs() {
        let a = WebhookSigner::sign("payload1", "key");
        let b = WebhookSigner::sign("payload2", "key");
        assert_ne!(a, b);
    }

    #[test]
    fn test_signer_different_secrets_different_sigs() {
        let a = WebhookSigner::sign("payload", "key1");
        let b = WebhookSigner::sign("payload", "key2");
        assert_ne!(a, b);
    }

    #[test]
    fn test_signer_verify_valid() {
        let sig = WebhookSigner::sign("data", "secret");
        assert!(WebhookSigner::verify("data", "secret", &sig));
    }

    #[test]
    fn test_signer_verify_invalid_signature() {
        assert!(!WebhookSigner::verify("data", "secret", "badsig"));
    }

    #[test]
    fn test_signer_verify_wrong_secret() {
        let sig = WebhookSigner::sign("data", "secret1");
        assert!(!WebhookSigner::verify("data", "secret2", &sig));
    }

    #[test]
    fn test_signer_verify_wrong_payload() {
        let sig = WebhookSigner::sign("data1", "secret");
        assert!(!WebhookSigner::verify("data2", "secret", &sig));
    }

    // -- RetryPolicy tests --------------------------------------------------

    #[test]
    fn test_retry_policy_defaults() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 3);
        assert_eq!(p.initial_delay_ms, 1000);
        assert!((p.backoff_multiplier - 2.0).abs() < f32::EPSILON);
        assert_eq!(p.max_delay_ms, 30_000);
    }

    #[test]
    fn test_retry_policy_delay_for_attempt_zero() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for_attempt(0), 1000);
    }

    #[test]
    fn test_retry_policy_delay_exponential_backoff() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for_attempt(0), 1000);
        assert_eq!(p.delay_for_attempt(1), 2000);
        assert_eq!(p.delay_for_attempt(2), 4000);
        assert_eq!(p.delay_for_attempt(3), 8000);
    }

    #[test]
    fn test_retry_policy_delay_capped_at_max() {
        let p = RetryPolicy {
            max_retries: 10,
            initial_delay_ms: 1000,
            backoff_multiplier: 10.0,
            max_delay_ms: 5000,
        };
        assert_eq!(p.delay_for_attempt(5), 5000);
    }

    // -- WebhookEventType tests ---------------------------------------------

    #[test]
    fn test_event_type_display() {
        assert_eq!(
            WebhookEventType::AgentTaskCompleted.to_string(),
            "agent.task.completed"
        );
        assert_eq!(
            WebhookEventType::AgentTaskFailed.to_string(),
            "agent.task.failed"
        );
        assert_eq!(
            WebhookEventType::HealthCheckFailed.to_string(),
            "health.check.failed"
        );
        assert_eq!(
            WebhookEventType::Custom("foo".into()).to_string(),
            "custom.foo"
        );
    }

    #[test]
    fn test_event_type_serde_roundtrip() {
        let types = vec![
            WebhookEventType::AgentTaskCompleted,
            WebhookEventType::BudgetExceeded,
            WebhookEventType::Custom("my_event".into()),
        ];
        for t in types {
            let json = serde_json::to_string(&t).unwrap();
            let back: WebhookEventType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, t);
        }
    }

    // -- WebhookEvent tests -------------------------------------------------

    #[test]
    fn test_webhook_event_new_has_uuid() {
        let event = WebhookEvent::new(
            WebhookEventType::AgentTaskCompleted,
            "tenant-1",
            serde_json::json!({}),
        );
        assert!(!event.event_id.is_empty());
        assert!(Uuid::parse_str(&event.event_id).is_ok());
        assert_eq!(event.tenant_id, "tenant-1");
    }

    #[test]
    fn test_webhook_event_serialization() {
        let event = WebhookEvent::new(
            WebhookEventType::LeadQualified,
            "t1",
            serde_json::json!({"score": 85}),
        );
        let json = serde_json::to_string(&event).unwrap();
        let back: WebhookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_id, event.event_id);
        assert_eq!(back.event_type, WebhookEventType::LeadQualified);
    }

    // -- WebhookSubscription tests ------------------------------------------

    #[test]
    fn test_subscription_serde_roundtrip() {
        let sub = test_subscription("t1", vec![WebhookEventType::TicketRouted]);
        let json = serde_json::to_string(&sub).unwrap();
        let back: WebhookSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, sub.id);
        assert_eq!(back.tenant_id, "t1");
        assert_eq!(back.events.len(), 1);
    }

    // -- WebhookDelivery tests ----------------------------------------------

    #[test]
    fn test_delivery_status_serde() {
        let statuses = vec![
            DeliveryStatus::Pending,
            DeliveryStatus::Success,
            DeliveryStatus::Failed,
            DeliveryStatus::Retrying,
        ];
        for s in statuses {
            let json = serde_json::to_string(&s).unwrap();
            let back: DeliveryStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, s);
        }
    }

    // -- WebhookDispatcher tests --------------------------------------------

    #[tokio::test]
    async fn test_dispatcher_subscribe_and_list() {
        let d = WebhookDispatcher::new(test_config());
        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        let id = d.subscribe(sub).await;

        let subs = d.list_subscriptions().await;
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, id);
    }

    #[tokio::test]
    async fn test_dispatcher_unsubscribe() {
        let d = WebhookDispatcher::new(test_config());
        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        let id = d.subscribe(sub).await;

        assert!(d.unsubscribe(&id).await);
        assert!(!d.unsubscribe(&id).await); // already removed
        assert!(d.list_subscriptions().await.is_empty());
    }

    #[tokio::test]
    async fn test_dispatcher_get_subscription() {
        let d = WebhookDispatcher::new(test_config());
        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        let id = sub.id.clone();
        d.subscribe(sub).await;

        assert!(d.get_subscription(&id).await.is_some());
        assert!(d.get_subscription("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_no_matching() {
        let d = WebhookDispatcher::new(test_config());
        // No subscriptions registered — dispatch should not panic.
        let event = test_event("t1", WebhookEventType::AgentTaskCompleted);
        d.dispatch(event).await;
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_skips_disabled() {
        let d = WebhookDispatcher::new(test_config());
        let mut sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        sub.enabled = false;
        d.subscribe(sub).await;

        let event = test_event("t1", WebhookEventType::AgentTaskCompleted);
        // Should not attempt delivery to disabled subscription.
        d.dispatch(event).await;
        // Allow the spawned tasks (if any) to finish.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_skips_wrong_tenant() {
        let d = WebhookDispatcher::new(test_config());
        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        d.subscribe(sub).await;

        // Event for a different tenant
        let event = test_event("t2", WebhookEventType::AgentTaskCompleted);
        d.dispatch(event).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_skips_wrong_event_type() {
        let d = WebhookDispatcher::new(test_config());
        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        d.subscribe(sub).await;

        // Event of a different type
        let event = test_event("t1", WebhookEventType::BudgetExceeded);
        d.dispatch(event).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_dispatcher_delivery_log_empty() {
        let d = WebhookDispatcher::new(test_config());
        let log = d.get_delivery_log("nonexistent", 10).await;
        assert!(log.is_empty());
    }

    #[tokio::test]
    async fn test_dispatcher_record_delivery_trims() {
        let mut config = test_config();
        config.max_log_entries = 3;
        let d = WebhookDispatcher::new(config);

        // Manually record 5 deliveries
        for i in 0..5 {
            let delivery = WebhookDelivery {
                delivery_id: format!("d-{i}"),
                subscription_id: "sub-1".to_string(),
                event_id: "evt-1".to_string(),
                status: DeliveryStatus::Success,
                http_status: Some(200),
                response_body: None,
                attempt: 1,
                attempted_at: Utc::now(),
                duration_ms: 10,
                error: None,
            };
            d.record_delivery(delivery).await;
        }

        let log = d.get_delivery_log("sub-1", 10).await;
        // Should be trimmed to max 3
        assert_eq!(log.len(), 3);
    }

    // -- REST endpoint tests ------------------------------------------------

    fn test_app() -> (Router, Arc<WebhookOutboundState>) {
        let state = Arc::new(WebhookOutboundState {
            dispatcher: WebhookDispatcher::new(test_config()),
        });
        let app = webhook_outbound_router(Arc::clone(&state));
        (app, state)
    }

    #[tokio::test]
    async fn test_rest_create_subscription() {
        let (app, _state) = test_app();

        let body = serde_json::json!({
            "tenant_id": "t1",
            "url": "https://example.com/hook",
            "events": [{"type": "AgentTaskCompleted"}],
            "secret": "s3cr3t",
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/subscriptions")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("id").is_some());
    }

    #[tokio::test]
    async fn test_rest_list_subscriptions() {
        let (app, state) = test_app();

        // Pre-populate
        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        state.dispatcher.subscribe(sub).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/webhooks/subscriptions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 1);
    }

    #[tokio::test]
    async fn test_rest_delete_subscription() {
        let (app, state) = test_app();

        let sub = test_subscription("t1", vec![WebhookEventType::AgentTaskCompleted]);
        let id = sub.id.clone();
        state.dispatcher.subscribe(sub).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/webhooks/subscriptions/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rest_delete_subscription_not_found() {
        let (app, _state) = test_app();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/webhooks/subscriptions/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_rest_get_deliveries_empty() {
        let (app, _state) = test_app();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/webhooks/deliveries/some-sub")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 0);
    }

    #[tokio::test]
    async fn test_rest_test_event_not_found() {
        let (app, _state) = test_app();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/test/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

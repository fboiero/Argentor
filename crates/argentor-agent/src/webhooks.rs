// SPDX-License-Identifier: AGPL-3.0-only
//! Webhook notification system for Argentor agent events.
//!
//! Sends fire-and-forget HTTP POST payloads to configured URLs when agent
//! events fire. Supports HMAC-SHA256 signing via `X-Argentor-Signature`.
//!
//! # Example
//!
//! ```rust,no_run
//! use argentor_agent::webhooks::{WebhookConfig, WebhookEvent, WebhookManager};
//!
//! let config = WebhookConfig {
//!     url: "https://example.com/hook".into(),
//!     events: vec![WebhookEvent::AgentStarted, WebhookEvent::AgentCompleted],
//!     secret: Some("my-secret".into()),
//!     timeout_ms: 5000,
//! };
//! let manager = WebhookManager::new(vec![config]);
//! ```

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tracing::{info, warn};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for a single webhook endpoint.
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Target URL for POST requests.
    pub url: String,
    /// Events that trigger a notification to this endpoint.
    pub events: Vec<WebhookEvent>,
    /// Optional HMAC-SHA256 secret for request signing.
    pub secret: Option<String>,
    /// Request timeout in milliseconds (default: 5000).
    pub timeout_ms: u64,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            events: Vec::new(),
            secret: None,
            timeout_ms: 5000,
        }
    }
}

/// Events that the webhook system can emit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Agent run started.
    AgentStarted,
    /// Agent run completed successfully.
    AgentCompleted,
    /// Agent run failed.
    AgentFailed,
    /// A tool was called.
    ToolCalled,
    /// A guardrail blocked input or output.
    GuardrailBlocked,
    /// A tool requires human approval.
    ApprovalRequired,
    /// Token/cost budget exhausted.
    BudgetExhausted,
}

/// Payload sent to webhook endpoints.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookPayload {
    /// The event that triggered this notification.
    pub event: WebhookEvent,
    /// ISO-8601 timestamp.
    pub timestamp: DateTime<Utc>,
    /// Session identifier.
    pub session_id: String,
    /// Event-specific data.
    pub data: serde_json::Value,
}

// ---------------------------------------------------------------------------
// WebhookManager
// ---------------------------------------------------------------------------

/// Sends fire-and-forget webhook notifications for agent events.
///
/// Internally holds a `reqwest::Client` and a list of [`WebhookConfig`]s.
/// Notifications are spawned as Tokio tasks so they never block the agent loop.
#[derive(Clone)]
pub struct WebhookManager {
    configs: Vec<WebhookConfig>,
    client: reqwest::Client,
}

impl std::fmt::Debug for WebhookManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookManager")
            .field("configs_count", &self.configs.len())
            .finish()
    }
}

impl WebhookManager {
    /// Create a new manager with the given endpoint configurations.
    pub fn new(configs: Vec<WebhookConfig>) -> Self {
        Self {
            configs,
            client: reqwest::Client::new(),
        }
    }

    /// Fire a webhook notification for the given event.
    ///
    /// Only endpoints subscribed to `event` receive the call.
    /// Spawns a Tokio task per endpoint — never blocks the caller.
    pub fn fire(
        &self,
        event: WebhookEvent,
        session_id: impl Into<String>,
        data: serde_json::Value,
    ) {
        let session_id = session_id.into();
        let now = Utc::now();

        for config in &self.configs {
            if !config.events.contains(&event) {
                continue;
            }

            let payload = WebhookPayload {
                event: event.clone(),
                timestamp: now,
                session_id: session_id.clone(),
                data: data.clone(),
            };

            let client = self.client.clone();
            let url = config.url.clone();
            let secret = config.secret.clone();
            let timeout_ms = config.timeout_ms;

            tokio::spawn(async move {
                send_webhook(client, url, payload, secret, timeout_ms).await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Internal send logic
// ---------------------------------------------------------------------------

async fn send_webhook(
    client: reqwest::Client,
    url: String,
    payload: WebhookPayload,
    secret: Option<String>,
    timeout_ms: u64,
) {
    let body = match serde_json::to_string(&payload) {
        Ok(b) => b,
        Err(e) => {
            warn!(url = %url, error = %e, "Failed to serialize webhook payload");
            return;
        }
    };

    let mut request = client
        .post(&url)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .body(body.clone());

    if let Some(ref s) = secret {
        let signature = hmac_sha256_hex(s, body.as_bytes());
        request = request.header("X-Argentor-Signature", format!("sha256={signature}"));
    }

    match request.send().await {
        Ok(resp) => {
            info!(
                url = %url,
                status = %resp.status(),
                event = ?payload.event,
                "Webhook delivered"
            );
        }
        Err(e) => {
            warn!(url = %url, error = %e, "Webhook delivery failed");
        }
    }
}

/// Compute HMAC-SHA256 and return a lowercase hex string.
fn hmac_sha256_hex(secret: &str, data: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(data);
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_payload(event: WebhookEvent) -> WebhookPayload {
        WebhookPayload {
            event,
            timestamp: Utc::now(),
            session_id: "test-session".into(),
            data: json!({"key": "value"}),
        }
    }

    #[test]
    fn payload_serializes_correctly() {
        let p = make_payload(WebhookEvent::AgentStarted);
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"agent_started\""));
        assert!(s.contains("\"test-session\""));
        assert!(s.contains("\"key\""));
    }

    #[test]
    fn hmac_signing_is_deterministic() {
        let secret = "my-secret";
        let data = b"hello world";
        let sig1 = hmac_sha256_hex(secret, data);
        let sig2 = hmac_sha256_hex(secret, data);
        assert_eq!(sig1, sig2);
        assert_eq!(sig1.len(), 64); // 32 bytes = 64 hex chars
    }

    #[test]
    fn hmac_signing_differs_with_different_secrets() {
        let data = b"payload";
        let sig1 = hmac_sha256_hex("secret1", data);
        let sig2 = hmac_sha256_hex("secret2", data);
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn hmac_signing_differs_with_different_data() {
        let secret = "secret";
        let sig1 = hmac_sha256_hex(secret, b"data1");
        let sig2 = hmac_sha256_hex(secret, b"data2");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn event_filtering_skips_unsubscribed_events() {
        // Manager with only AgentCompleted subscribed
        let config = WebhookConfig {
            url: "http://localhost:9999/hook".into(),
            events: vec![WebhookEvent::AgentCompleted],
            secret: None,
            timeout_ms: 1000,
        };
        let manager = WebhookManager::new(vec![config.clone()]);

        // Verify config subscription logic
        let fires_started = config.events.contains(&WebhookEvent::AgentStarted);
        let fires_completed = config.events.contains(&WebhookEvent::AgentCompleted);
        assert!(!fires_started);
        assert!(fires_completed);

        // Manager creation works
        assert_eq!(manager.configs.len(), 1);
    }

    #[test]
    fn webhook_event_serialization_round_trips() {
        let events = vec![
            WebhookEvent::AgentStarted,
            WebhookEvent::AgentCompleted,
            WebhookEvent::AgentFailed,
            WebhookEvent::ToolCalled,
            WebhookEvent::GuardrailBlocked,
            WebhookEvent::ApprovalRequired,
            WebhookEvent::BudgetExhausted,
        ];
        for event in events {
            let s = serde_json::to_string(&event).unwrap();
            let back: WebhookEvent = serde_json::from_str(&s).unwrap();
            assert_eq!(event, back);
        }
    }

    #[test]
    fn webhook_config_default_timeout() {
        let config = WebhookConfig::default();
        assert_eq!(config.timeout_ms, 5000);
    }
}

//! Cloud-grade audit logging.
//!
//! Records tenant-scoped events (auth, runs, quota hits, admin actions) in
//! an append-only in-memory buffer. The `AuditSink` trait is the boundary
//! where production would plug S3 / OpenSearch / BigQuery.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use uuid::Uuid;

/// Classification of audit events. Extend freely — downstream SIEM consumers
/// use this to route and retain events per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditEventKind {
    /// Tenant created.
    TenantCreated,
    /// Tenant suspended.
    TenantSuspended,
    /// Tenant reactivated.
    TenantActivated,
    /// Tenant plan changed (up/downgrade).
    PlanChanged,
    /// Agent run started.
    RunStarted,
    /// Agent run completed successfully.
    RunCompleted,
    /// Agent run failed.
    RunFailed,
    /// Quota exceeded.
    QuotaExceeded,
    /// Auth success.
    AuthSuccess,
    /// Auth failure (signal for rate-limiting / lockout).
    AuthFailed,
    /// Admin API action.
    AdminAction,
}

/// A single immutable audit event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    /// Globally unique event id.
    pub id: String,
    /// Event category.
    pub kind: AuditEventKind,
    /// Tenant the event pertains to (empty for platform-level events).
    pub tenant_id: String,
    /// Actor (user id, api-key id, system). Empty for anonymous.
    pub actor: String,
    /// Free-form detail string (JSON-encoded if structured).
    pub detail: String,
    /// UTC timestamp.
    pub at: DateTime<Utc>,
}

impl AuditEvent {
    /// Construct a new event with a fresh UUID and current timestamp.
    pub fn new(
        kind: AuditEventKind,
        tenant_id: impl Into<String>,
        actor: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            kind,
            tenant_id: tenant_id.into(),
            actor: actor.into(),
            detail: detail.into(),
            at: Utc::now(),
        }
    }
}

/// Pluggable persistent sink for audit events.
///
/// Implementations: in-process buffer (included), S3/Parquet archiver,
/// OpenSearch forwarder, SIEM webhook.
#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Persist a single event. Should be idempotent on event id.
    async fn write(&self, event: AuditEvent);
}

/// Append-only in-memory audit log (primary for dev/tests).
pub struct AuditLog {
    events: RwLock<Vec<AuditEvent>>,
}

impl AuditLog {
    /// Create an empty log.
    pub fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
        }
    }

    /// Append an event synchronously.
    pub fn append(&self, event: AuditEvent) {
        if let Ok(mut guard) = self.events.write() {
            guard.push(event);
        }
    }

    /// Count of events in the log.
    pub fn len(&self) -> usize {
        self.events.read().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot all events (expensive — tests + export only).
    pub fn all(&self) -> Vec<AuditEvent> {
        self.events.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Events for a specific tenant.
    pub fn for_tenant(&self, tenant_id: &str) -> Vec<AuditEvent> {
        self.events
            .read()
            .map(|g| {
                g.iter()
                    .filter(|e| e.tenant_id == tenant_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Events matching a kind.
    pub fn by_kind(&self, kind: AuditEventKind) -> Vec<AuditEvent> {
        self.events
            .read()
            .map(|g| g.iter().filter(|e| e.kind == kind).cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditSink for AuditLog {
    async fn write(&self, event: AuditEvent) {
        self.append(event);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_log_is_empty() {
        let log = AuditLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn append_increments_length() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(
            AuditEventKind::TenantCreated,
            "t1",
            "admin",
            "created",
        ));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn all_returns_snapshot() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(AuditEventKind::RunStarted, "t1", "api", ""));
        log.append(AuditEvent::new(
            AuditEventKind::RunCompleted,
            "t1",
            "api",
            "",
        ));
        assert_eq!(log.all().len(), 2);
    }

    #[test]
    fn for_tenant_filters() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(AuditEventKind::RunStarted, "t1", "api", ""));
        log.append(AuditEvent::new(AuditEventKind::RunStarted, "t2", "api", ""));
        assert_eq!(log.for_tenant("t1").len(), 1);
        assert_eq!(log.for_tenant("t2").len(), 1);
    }

    #[test]
    fn by_kind_filters() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(AuditEventKind::RunStarted, "t1", "api", ""));
        log.append(AuditEvent::new(AuditEventKind::RunFailed, "t1", "api", ""));
        log.append(AuditEvent::new(AuditEventKind::RunStarted, "t1", "api", ""));
        assert_eq!(log.by_kind(AuditEventKind::RunStarted).len(), 2);
        assert_eq!(log.by_kind(AuditEventKind::RunFailed).len(), 1);
    }

    #[test]
    fn audit_event_assigns_uuid() {
        let e = AuditEvent::new(AuditEventKind::AdminAction, "t1", "root", "rotated key");
        assert!(!e.id.is_empty());
        assert_eq!(e.kind, AuditEventKind::AdminAction);
    }

    #[test]
    fn audit_event_timestamps_now() {
        let before = Utc::now();
        let e = AuditEvent::new(AuditEventKind::AuthSuccess, "t1", "u1", "");
        let after = Utc::now();
        assert!(e.at >= before && e.at <= after);
    }

    #[test]
    fn audit_event_serde_roundtrip() {
        let e = AuditEvent::new(AuditEventKind::QuotaExceeded, "t1", "api", "runs");
        let json = serde_json::to_string(&e).unwrap();
        let back: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[tokio::test]
    async fn audit_sink_trait_writes_async() {
        let log = AuditLog::new();
        let sink: &dyn AuditSink = &log;
        sink.write(AuditEvent::new(AuditEventKind::RunStarted, "t1", "api", ""))
            .await;
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn event_kinds_are_distinct() {
        assert_ne!(AuditEventKind::AuthSuccess, AuditEventKind::AuthFailed);
        assert_ne!(AuditEventKind::RunStarted, AuditEventKind::RunCompleted);
    }

    #[test]
    fn plan_changed_is_audited() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(
            AuditEventKind::PlanChanged,
            "t1",
            "admin",
            "Free->Growth",
        ));
        assert_eq!(log.by_kind(AuditEventKind::PlanChanged).len(), 1);
    }

    #[test]
    fn tenant_filter_returns_empty_for_unknown() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(AuditEventKind::RunStarted, "t1", "api", ""));
        assert!(log.for_tenant("ghost").is_empty());
    }

    #[test]
    fn large_append_works() {
        let log = AuditLog::new();
        for i in 0..1_000 {
            log.append(AuditEvent::new(
                AuditEventKind::RunCompleted,
                "t1",
                format!("u{i}"),
                "",
            ));
        }
        assert_eq!(log.len(), 1_000);
    }

    #[test]
    fn default_is_empty() {
        let log = AuditLog::default();
        assert!(log.is_empty());
    }

    #[test]
    fn actor_preserved() {
        let log = AuditLog::new();
        log.append(AuditEvent::new(
            AuditEventKind::AdminAction,
            "t1",
            "admin@acme.com",
            "",
        ));
        assert_eq!(log.all()[0].actor, "admin@acme.com");
    }

    #[test]
    fn detail_preserved() {
        let e = AuditEvent::new(AuditEventKind::RunFailed, "t1", "api", "timeout after 30s");
        assert!(e.detail.contains("timeout"));
    }
}

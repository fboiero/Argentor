//! Daily usage quotas per tenant — tokens, requests, and concurrent sessions.
//!
//! `QuotaManager` tracks three counters per tenant using atomics for low
//! contention on the hot path:
//!
//! - `tokens_used_today` — total LLM tokens consumed today
//! - `requests_today` — total API requests today
//! - `active_sessions` — concurrently open sessions (gauge, not daily counter)
//!
//! Counters auto-reset at midnight UTC. The reset is lazy: the first call that
//! arrives after `last_reset` has aged into a new calendar day triggers the
//! reset for that tenant.
//!
//! Default limits: 1M tokens/day, 10K requests/day, 100 concurrent sessions.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use thiserror::Error;

/// Default daily token quota per tenant.
pub const DEFAULT_TOKENS_PER_DAY: u64 = 1_000_000;
/// Default daily request quota per tenant.
pub const DEFAULT_REQUESTS_PER_DAY: u64 = 10_000;
/// Default maximum concurrent sessions per tenant.
pub const DEFAULT_MAX_SESSIONS: u32 = 100;

/// Reason a quota check failed.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum QuotaExceeded {
    /// Tenant was not registered before the call.
    #[error("Tenant '{0}' is not registered")]
    TenantNotFound(String),
    /// Daily token quota exhausted.
    #[error("Daily token quota exceeded for tenant '{tenant}': used {used}/{limit}")]
    TokensPerDay {
        /// Tenant identifier.
        tenant: String,
        /// Tokens consumed today.
        used: u64,
        /// Daily limit.
        limit: u64,
    },
    /// Daily request quota exhausted.
    #[error("Daily request quota exceeded for tenant '{tenant}': used {used}/{limit}")]
    RequestsPerDay {
        /// Tenant identifier.
        tenant: String,
        /// Requests made today.
        used: u64,
        /// Daily limit.
        limit: u64,
    },
    /// Concurrent session cap hit.
    #[error("Session quota exceeded for tenant '{tenant}': {active}/{limit} active")]
    SessionLimit {
        /// Tenant identifier.
        tenant: String,
        /// Currently active sessions.
        active: u32,
        /// Concurrent session limit.
        limit: u32,
    },
}

/// Configurable limits for a single tenant entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantQuotaLimits {
    /// Maximum tokens per day.
    pub tokens_per_day: u64,
    /// Maximum requests per day.
    pub requests_per_day: u64,
    /// Maximum concurrent sessions.
    pub max_sessions: u32,
}

impl Default for TenantQuotaLimits {
    fn default() -> Self {
        Self {
            tokens_per_day: DEFAULT_TOKENS_PER_DAY,
            requests_per_day: DEFAULT_REQUESTS_PER_DAY,
            max_sessions: DEFAULT_MAX_SESSIONS,
        }
    }
}

/// Atomic per-tenant usage counters.
struct TenantCounters {
    tokens_used_today: AtomicU64,
    requests_today: AtomicU64,
    active_sessions: AtomicU32,
    /// Last calendar day (UTC) when counters were reset.
    last_reset: Mutex<NaiveDate>,
    limits: TenantQuotaLimits,
}

impl TenantCounters {
    fn new(limits: TenantQuotaLimits) -> Self {
        Self {
            tokens_used_today: AtomicU64::new(0),
            requests_today: AtomicU64::new(0),
            active_sessions: AtomicU32::new(0),
            last_reset: Mutex::new(Utc::now().date_naive()),
            limits,
        }
    }

    /// Reset daily counters if we have crossed into a new calendar day (UTC).
    fn maybe_reset(&self, now: NaiveDate) {
        let mut last = self.last_reset.lock().unwrap_or_else(|e| e.into_inner());
        if now > *last {
            self.tokens_used_today.store(0, Ordering::Relaxed);
            self.requests_today.store(0, Ordering::Relaxed);
            *last = now;
        }
    }
}

/// Actions that can be quota-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaAction {
    /// Check whether the tenant can consume `tokens` more tokens today.
    ConsumeTokens(u64),
    /// Check whether the tenant can make one more request today.
    MakeRequest,
    /// Check whether the tenant can open one more concurrent session.
    OpenSession,
}

/// Snapshot of a tenant's current quota usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Tokens consumed today.
    pub tokens_used_today: u64,
    /// Daily token limit.
    pub tokens_limit: u64,
    /// Requests made today.
    pub requests_today: u64,
    /// Daily request limit.
    pub requests_limit: u64,
    /// Currently active sessions.
    pub active_sessions: u32,
    /// Concurrent session limit.
    pub session_limit: u32,
    /// UTC timestamp of the snapshot.
    pub captured_at: DateTime<Utc>,
}

/// Per-tenant daily quota manager.
///
/// Thread-safe. All methods take `&self` and are safe to call from multiple
/// async tasks concurrently.
pub struct QuotaManager {
    tenants: RwLock<HashMap<String, Arc<TenantCounters>>>,
}

impl QuotaManager {
    /// Create an empty manager (no tenants registered).
    pub fn new() -> Self {
        Self {
            tenants: RwLock::new(HashMap::new()),
        }
    }

    /// Register a tenant with default limits.
    pub fn register(&self, tenant_id: String) {
        self.register_with_limits(tenant_id, TenantQuotaLimits::default());
    }

    /// Register a tenant with custom limits.
    pub fn register_with_limits(&self, tenant_id: String, limits: TenantQuotaLimits) {
        if let Ok(mut g) = self.tenants.write() {
            g.insert(tenant_id, Arc::new(TenantCounters::new(limits)));
        }
    }

    /// Check whether `action` is permitted for `tenant_id`.
    ///
    /// This performs a lazy daily reset check before evaluating limits.
    /// Returns `Ok(())` if the action is within quota.
    pub fn check_quota(&self, tenant_id: &str, action: QuotaAction) -> Result<(), QuotaExceeded> {
        let counters = self.get_counters(tenant_id)?;
        counters.maybe_reset(Utc::now().date_naive());

        match action {
            QuotaAction::ConsumeTokens(n) => {
                let used = counters.tokens_used_today.load(Ordering::Relaxed);
                let limit = counters.limits.tokens_per_day;
                if used.saturating_add(n) > limit {
                    return Err(QuotaExceeded::TokensPerDay {
                        tenant: tenant_id.to_string(),
                        used,
                        limit,
                    });
                }
            }
            QuotaAction::MakeRequest => {
                let used = counters.requests_today.load(Ordering::Relaxed);
                let limit = counters.limits.requests_per_day;
                if used >= limit {
                    return Err(QuotaExceeded::RequestsPerDay {
                        tenant: tenant_id.to_string(),
                        used,
                        limit,
                    });
                }
            }
            QuotaAction::OpenSession => {
                let active = counters.active_sessions.load(Ordering::Relaxed);
                let limit = counters.limits.max_sessions;
                if active >= limit {
                    return Err(QuotaExceeded::SessionLimit {
                        tenant: tenant_id.to_string(),
                        active,
                        limit,
                    });
                }
            }
        }
        Ok(())
    }

    /// Record token consumption for `tenant_id` (does not re-check limit).
    pub fn record_tokens(&self, tenant_id: &str, tokens: u64) {
        if let Ok(c) = self.get_counters(tenant_id) {
            c.maybe_reset(Utc::now().date_naive());
            c.tokens_used_today.fetch_add(tokens, Ordering::Relaxed);
        }
    }

    /// Record one request for `tenant_id`.
    pub fn record_request(&self, tenant_id: &str) {
        if let Ok(c) = self.get_counters(tenant_id) {
            c.maybe_reset(Utc::now().date_naive());
            c.requests_today.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Increment the active session counter for `tenant_id`.
    pub fn open_session(&self, tenant_id: &str) -> Result<(), QuotaExceeded> {
        let counters = self.get_counters(tenant_id)?;
        counters.maybe_reset(Utc::now().date_naive());
        let active = counters.active_sessions.load(Ordering::Relaxed);
        let limit = counters.limits.max_sessions;
        if active >= limit {
            return Err(QuotaExceeded::SessionLimit {
                tenant: tenant_id.to_string(),
                active,
                limit,
            });
        }
        counters.active_sessions.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Decrement the active session counter (saturates at zero).
    pub fn close_session(&self, tenant_id: &str) {
        if let Ok(c) = self.get_counters(tenant_id) {
            // Saturating decrement via compare-exchange loop.
            let mut cur = c.active_sessions.load(Ordering::Relaxed);
            loop {
                if cur == 0 {
                    break;
                }
                match c.active_sessions.compare_exchange_weak(
                    cur,
                    cur - 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(v) => cur = v,
                }
            }
        }
    }

    /// Snapshot current usage for `tenant_id`.
    pub fn snapshot(&self, tenant_id: &str) -> Option<QuotaSnapshot> {
        let counters = self.get_counters(tenant_id).ok()?;
        counters.maybe_reset(Utc::now().date_naive());
        Some(QuotaSnapshot {
            tenant_id: tenant_id.to_string(),
            tokens_used_today: counters.tokens_used_today.load(Ordering::Relaxed),
            tokens_limit: counters.limits.tokens_per_day,
            requests_today: counters.requests_today.load(Ordering::Relaxed),
            requests_limit: counters.limits.requests_per_day,
            active_sessions: counters.active_sessions.load(Ordering::Relaxed),
            session_limit: counters.limits.max_sessions,
            captured_at: Utc::now(),
        })
    }

    fn get_counters(&self, tenant_id: &str) -> Result<Arc<TenantCounters>, QuotaExceeded> {
        self.tenants
            .read()
            .ok()
            .and_then(|g| g.get(tenant_id).cloned())
            .ok_or_else(|| QuotaExceeded::TenantNotFound(tenant_id.to_string()))
    }
}

impl Default for QuotaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn mgr_with_tenant(id: &str) -> QuotaManager {
        let m = QuotaManager::new();
        m.register(id.to_string());
        m
    }

    // ── registration ────────────────────────────────────────────────────────

    #[test]
    fn unregistered_tenant_returns_not_found() {
        let m = QuotaManager::new();
        let err = m
            .check_quota("ghost", QuotaAction::MakeRequest)
            .unwrap_err();
        assert!(matches!(err, QuotaExceeded::TenantNotFound(_)));
    }

    #[test]
    fn registered_tenant_starts_at_zero() {
        let m = mgr_with_tenant("t1");
        let snap = m.snapshot("t1").unwrap();
        assert_eq!(snap.tokens_used_today, 0);
        assert_eq!(snap.requests_today, 0);
        assert_eq!(snap.active_sessions, 0);
    }

    // ── default limits ───────────────────────────────────────────────────────

    #[test]
    fn default_limits_match_constants() {
        let m = mgr_with_tenant("t1");
        let snap = m.snapshot("t1").unwrap();
        assert_eq!(snap.tokens_limit, DEFAULT_TOKENS_PER_DAY);
        assert_eq!(snap.requests_limit, DEFAULT_REQUESTS_PER_DAY);
        assert_eq!(snap.session_limit, DEFAULT_MAX_SESSIONS);
    }

    // ── tokens ──────────────────────────────────────────────────────────────

    #[test]
    fn token_check_passes_within_limit() {
        let m = mgr_with_tenant("t1");
        assert!(m
            .check_quota("t1", QuotaAction::ConsumeTokens(500_000))
            .is_ok());
    }

    #[test]
    fn token_check_fails_when_limit_exceeded() {
        let m = mgr_with_tenant("t1");
        m.record_tokens("t1", 900_000);
        let err = m
            .check_quota("t1", QuotaAction::ConsumeTokens(200_000))
            .unwrap_err();
        assert!(matches!(err, QuotaExceeded::TokensPerDay { .. }));
    }

    #[test]
    fn record_tokens_accumulates() {
        let m = mgr_with_tenant("t1");
        m.record_tokens("t1", 100_000);
        m.record_tokens("t1", 200_000);
        assert_eq!(m.snapshot("t1").unwrap().tokens_used_today, 300_000);
    }

    // ── requests ────────────────────────────────────────────────────────────

    #[test]
    fn request_check_passes_below_limit() {
        let m = mgr_with_tenant("t1");
        assert!(m.check_quota("t1", QuotaAction::MakeRequest).is_ok());
    }

    #[test]
    fn request_check_fails_at_limit() {
        let m = QuotaManager::new();
        m.register_with_limits(
            "t1".into(),
            TenantQuotaLimits {
                requests_per_day: 3,
                ..TenantQuotaLimits::default()
            },
        );
        for _ in 0..3 {
            m.record_request("t1");
        }
        let err = m.check_quota("t1", QuotaAction::MakeRequest).unwrap_err();
        assert!(matches!(
            err,
            QuotaExceeded::RequestsPerDay {
                used: 3,
                limit: 3,
                ..
            }
        ));
    }

    #[test]
    fn record_request_increments_counter() {
        let m = mgr_with_tenant("t1");
        m.record_request("t1");
        m.record_request("t1");
        assert_eq!(m.snapshot("t1").unwrap().requests_today, 2);
    }

    // ── sessions ────────────────────────────────────────────────────────────

    #[test]
    fn open_session_increments_active() {
        let m = mgr_with_tenant("t1");
        m.open_session("t1").unwrap();
        assert_eq!(m.snapshot("t1").unwrap().active_sessions, 1);
    }

    #[test]
    fn close_session_decrements_active() {
        let m = mgr_with_tenant("t1");
        m.open_session("t1").unwrap();
        m.close_session("t1");
        assert_eq!(m.snapshot("t1").unwrap().active_sessions, 0);
    }

    #[test]
    fn close_session_saturates_at_zero() {
        let m = mgr_with_tenant("t1");
        m.close_session("t1"); // should not panic or underflow
        assert_eq!(m.snapshot("t1").unwrap().active_sessions, 0);
    }

    #[test]
    fn session_limit_enforced() {
        let m = QuotaManager::new();
        m.register_with_limits(
            "t1".into(),
            TenantQuotaLimits {
                max_sessions: 2,
                ..TenantQuotaLimits::default()
            },
        );
        m.open_session("t1").unwrap();
        m.open_session("t1").unwrap();
        let err = m.open_session("t1").unwrap_err();
        assert!(matches!(
            err,
            QuotaExceeded::SessionLimit {
                active: 2,
                limit: 2,
                ..
            }
        ));
    }

    // ── custom limits ────────────────────────────────────────────────────────

    #[test]
    fn custom_limits_override_defaults() {
        let m = QuotaManager::new();
        m.register_with_limits(
            "t1".into(),
            TenantQuotaLimits {
                tokens_per_day: 50_000,
                requests_per_day: 500,
                max_sessions: 10,
            },
        );
        let snap = m.snapshot("t1").unwrap();
        assert_eq!(snap.tokens_limit, 50_000);
        assert_eq!(snap.requests_limit, 500);
        assert_eq!(snap.session_limit, 10);
    }

    // ── daily reset (simulated) ──────────────────────────────────────────────

    #[test]
    fn daily_reset_clears_tokens_and_requests() {
        let m = mgr_with_tenant("t1");
        m.record_tokens("t1", 100_000);
        m.record_request("t1");

        // Simulate midnight: set last_reset to yesterday
        {
            let g = m.tenants.read().unwrap();
            let c = g.get("t1").unwrap();
            let yesterday = Utc::now().date_naive().pred_opt().unwrap();
            *c.last_reset.lock().unwrap() = yesterday;
        }

        // Trigger reset via snapshot
        let snap = m.snapshot("t1").unwrap();
        assert_eq!(snap.tokens_used_today, 0, "tokens should reset at midnight");
        assert_eq!(snap.requests_today, 0, "requests should reset at midnight");
    }

    #[test]
    fn daily_reset_does_not_clear_active_sessions() {
        let m = mgr_with_tenant("t1");
        m.open_session("t1").unwrap();

        {
            let g = m.tenants.read().unwrap();
            let c = g.get("t1").unwrap();
            let yesterday = Utc::now().date_naive().pred_opt().unwrap();
            *c.last_reset.lock().unwrap() = yesterday;
        }

        let snap = m.snapshot("t1").unwrap();
        assert_eq!(
            snap.active_sessions, 1,
            "active sessions (gauge) must survive daily reset"
        );
    }
}

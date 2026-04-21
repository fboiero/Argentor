//! Per-tenant quota tracking and enforcement.
//!
//! Tracks usage against plan limits (runs, tokens, active agents, storage).
//! In-memory stub — production would back this with Redis for low-latency
//! increments and PostgreSQL for the persistent ledger.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

use crate::tenant::TenantPlan;

/// Snapshot of a tenant's usage for the current billing period.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaUsage {
    /// Tenant identifier this usage belongs to.
    pub tenant_id: String,
    /// Start of the current billing period (usually first of month, UTC).
    pub period_start: DateTime<Utc>,
    /// Agent runs consumed in the current period.
    pub agent_runs_used: u64,
    /// Agent-run limit for the period.
    pub agent_runs_limit: u64,
    /// Tokens consumed across all runs in the period.
    pub tokens_used: u64,
    /// Token limit for the period.
    pub tokens_limit: u64,
    /// Concurrently active agents (live, not historical).
    pub active_agents: u32,
    /// Active-agent limit (concurrent cap, not lifetime).
    pub active_agents_limit: u32,
    /// Storage used in megabytes (sessions, audit, artifacts).
    pub storage_mb_used: u64,
    /// Storage limit in megabytes.
    pub storage_mb_limit: u64,
}

impl QuotaUsage {
    /// Construct a fresh usage record at zero with plan-derived limits.
    pub fn new(tenant_id: String, plan: TenantPlan) -> Self {
        Self {
            tenant_id,
            period_start: Utc::now(),
            agent_runs_used: 0,
            agent_runs_limit: plan.run_quota(),
            tokens_used: 0,
            tokens_limit: plan.token_quota(),
            active_agents: 0,
            active_agents_limit: plan.agent_quota(),
            storage_mb_used: 0,
            storage_mb_limit: plan.storage_mb_quota(),
        }
    }

    /// Percentage of run quota consumed (0.0 to 1.0). Saturates at 1.0.
    pub fn runs_pct(&self) -> f64 {
        if self.agent_runs_limit == 0 {
            return 1.0;
        }
        (self.agent_runs_used as f64 / self.agent_runs_limit as f64).min(1.0)
    }

    /// Percentage of token quota consumed (0.0 to 1.0).
    pub fn tokens_pct(&self) -> f64 {
        if self.tokens_limit == 0 {
            return 1.0;
        }
        (self.tokens_used as f64 / self.tokens_limit as f64).min(1.0)
    }
}

/// Reasons a quota check or increment may fail.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum QuotaError {
    /// Tenant has no usage record.
    #[error("Tenant {0} not found")]
    NotFound(String),
    /// Agent run quota hit.
    #[error("Run quota exceeded ({used}/{limit})")]
    RunsExceeded {
        /// Runs already consumed.
        used: u64,
        /// Plan limit.
        limit: u64,
    },
    /// Token quota hit.
    #[error("Token quota exceeded ({used}/{limit})")]
    TokensExceeded {
        /// Tokens consumed.
        used: u64,
        /// Plan limit.
        limit: u64,
    },
    /// Active-agent concurrency cap hit.
    #[error("Active agents quota exceeded ({used}/{limit})")]
    AgentsExceeded {
        /// Current active agents.
        used: u32,
        /// Plan limit.
        limit: u32,
    },
    /// Storage cap hit.
    #[error("Storage quota exceeded ({used}/{limit} MB)")]
    StorageExceeded {
        /// Storage used in MB.
        used: u64,
        /// Plan limit in MB.
        limit: u64,
    },
}

/// Enforces per-tenant usage quotas.
pub struct QuotaEnforcer {
    usage: RwLock<HashMap<String, QuotaUsage>>,
}

impl QuotaEnforcer {
    /// Create an empty enforcer.
    pub fn new() -> Self {
        Self {
            usage: RwLock::new(HashMap::new()),
        }
    }

    /// Register a tenant's usage ledger with the given plan limits.
    pub fn register(&self, tenant_id: String, plan: TenantPlan) {
        if let Ok(mut guard) = self.usage.write() {
            let usage = QuotaUsage::new(tenant_id.clone(), plan);
            guard.insert(tenant_id, usage);
        }
    }

    /// Check whether a tenant can start another agent run.
    ///
    /// Does NOT increment — call [`QuotaEnforcer::record_run`] after the run
    /// completes to charge actual usage.
    pub fn check_can_run(&self, tenant_id: &str) -> Result<(), QuotaError> {
        let guard = self
            .usage
            .read()
            .map_err(|_| QuotaError::NotFound(tenant_id.to_string()))?;
        let usage = guard
            .get(tenant_id)
            .ok_or_else(|| QuotaError::NotFound(tenant_id.to_string()))?;
        if usage.agent_runs_used >= usage.agent_runs_limit {
            return Err(QuotaError::RunsExceeded {
                used: usage.agent_runs_used,
                limit: usage.agent_runs_limit,
            });
        }
        if usage.tokens_used >= usage.tokens_limit {
            return Err(QuotaError::TokensExceeded {
                used: usage.tokens_used,
                limit: usage.tokens_limit,
            });
        }
        if usage.active_agents >= usage.active_agents_limit {
            return Err(QuotaError::AgentsExceeded {
                used: usage.active_agents,
                limit: usage.active_agents_limit,
            });
        }
        Ok(())
    }

    /// Charge one run plus `tokens` to the tenant's usage ledger.
    pub fn record_run(&self, tenant_id: &str, tokens: u64) {
        if let Ok(mut guard) = self.usage.write() {
            if let Some(u) = guard.get_mut(tenant_id) {
                u.agent_runs_used = u.agent_runs_used.saturating_add(1);
                u.tokens_used = u.tokens_used.saturating_add(tokens);
            }
        }
    }

    /// Increment active agent counter (concurrency gauge).
    pub fn incr_active(&self, tenant_id: &str) -> Result<(), QuotaError> {
        let mut guard = self
            .usage
            .write()
            .map_err(|_| QuotaError::NotFound(tenant_id.to_string()))?;
        let u = guard
            .get_mut(tenant_id)
            .ok_or_else(|| QuotaError::NotFound(tenant_id.to_string()))?;
        if u.active_agents >= u.active_agents_limit {
            return Err(QuotaError::AgentsExceeded {
                used: u.active_agents,
                limit: u.active_agents_limit,
            });
        }
        u.active_agents = u.active_agents.saturating_add(1);
        Ok(())
    }

    /// Decrement active agent counter.
    pub fn decr_active(&self, tenant_id: &str) {
        if let Ok(mut guard) = self.usage.write() {
            if let Some(u) = guard.get_mut(tenant_id) {
                u.active_agents = u.active_agents.saturating_sub(1);
            }
        }
    }

    /// Add storage usage (MB) to the ledger.
    pub fn add_storage(&self, tenant_id: &str, mb: u64) -> Result<(), QuotaError> {
        let mut guard = self
            .usage
            .write()
            .map_err(|_| QuotaError::NotFound(tenant_id.to_string()))?;
        let u = guard
            .get_mut(tenant_id)
            .ok_or_else(|| QuotaError::NotFound(tenant_id.to_string()))?;
        let new_total = u.storage_mb_used.saturating_add(mb);
        if new_total > u.storage_mb_limit {
            return Err(QuotaError::StorageExceeded {
                used: new_total,
                limit: u.storage_mb_limit,
            });
        }
        u.storage_mb_used = new_total;
        Ok(())
    }

    /// Fetch a snapshot of the tenant's current usage.
    pub fn get_usage(&self, tenant_id: &str) -> Option<QuotaUsage> {
        self.usage.read().ok()?.get(tenant_id).cloned()
    }

    /// Reset the tenant's billing period (called by the billing cron).
    pub fn reset_period(&self, tenant_id: &str) {
        if let Ok(mut guard) = self.usage.write() {
            if let Some(u) = guard.get_mut(tenant_id) {
                u.period_start = Utc::now();
                u.agent_runs_used = 0;
                u.tokens_used = 0;
                // active_agents is a live gauge — do NOT reset
                // storage_mb_used is cumulative — do NOT reset
            }
        }
    }
}

impl Default for QuotaEnforcer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn setup() -> QuotaEnforcer {
        let q = QuotaEnforcer::new();
        q.register("t1".into(), TenantPlan::Free);
        q
    }

    #[test]
    fn register_initializes_limits_from_plan() {
        let q = setup();
        let usage = q.get_usage("t1").unwrap();
        assert_eq!(usage.agent_runs_limit, 1_000);
        assert_eq!(usage.active_agents_limit, 5);
    }

    #[test]
    fn check_can_run_passes_when_fresh() {
        let q = setup();
        assert!(q.check_can_run("t1").is_ok());
    }

    #[test]
    fn check_can_run_unknown_tenant_errors() {
        let q = QuotaEnforcer::new();
        assert!(matches!(
            q.check_can_run("ghost"),
            Err(QuotaError::NotFound(_))
        ));
    }

    #[test]
    fn record_run_increments_counters() {
        let q = setup();
        q.record_run("t1", 250);
        let u = q.get_usage("t1").unwrap();
        assert_eq!(u.agent_runs_used, 1);
        assert_eq!(u.tokens_used, 250);
    }

    #[test]
    fn check_fails_when_runs_exhausted() {
        let q = QuotaEnforcer::new();
        q.register("t1".into(), TenantPlan::Free);
        for _ in 0..1_000 {
            q.record_run("t1", 0);
        }
        match q.check_can_run("t1") {
            Err(QuotaError::RunsExceeded { used, limit }) => {
                assert_eq!(used, 1_000);
                assert_eq!(limit, 1_000);
            }
            other => panic!("expected RunsExceeded, got {other:?}"),
        }
    }

    #[test]
    fn check_fails_when_tokens_exhausted() {
        let q = QuotaEnforcer::new();
        q.register("t1".into(), TenantPlan::Free);
        q.record_run("t1", 1_000_000);
        assert!(matches!(
            q.check_can_run("t1"),
            Err(QuotaError::TokensExceeded { .. })
        ));
    }

    #[test]
    fn incr_active_increments() {
        let q = setup();
        q.incr_active("t1").unwrap();
        assert_eq!(q.get_usage("t1").unwrap().active_agents, 1);
    }

    #[test]
    fn incr_active_respects_limit() {
        let q = setup();
        for _ in 0..5 {
            q.incr_active("t1").unwrap();
        }
        assert!(matches!(
            q.incr_active("t1"),
            Err(QuotaError::AgentsExceeded { .. })
        ));
    }

    #[test]
    fn decr_active_decrements() {
        let q = setup();
        q.incr_active("t1").unwrap();
        q.decr_active("t1");
        assert_eq!(q.get_usage("t1").unwrap().active_agents, 0);
    }

    #[test]
    fn decr_active_saturates_at_zero() {
        let q = setup();
        q.decr_active("t1");
        assert_eq!(q.get_usage("t1").unwrap().active_agents, 0);
    }

    #[test]
    fn add_storage_accumulates() {
        let q = setup();
        q.add_storage("t1", 10).unwrap();
        q.add_storage("t1", 15).unwrap();
        assert_eq!(q.get_usage("t1").unwrap().storage_mb_used, 25);
    }

    #[test]
    fn add_storage_caps_at_limit() {
        let q = setup();
        assert!(matches!(
            q.add_storage("t1", 1_000),
            Err(QuotaError::StorageExceeded { .. })
        ));
    }

    #[test]
    fn runs_pct_reports_fraction() {
        let q = setup();
        for _ in 0..100 {
            q.record_run("t1", 0);
        }
        assert!((q.get_usage("t1").unwrap().runs_pct() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn tokens_pct_saturates_at_one() {
        let q = setup();
        q.record_run("t1", 2_000_000);
        assert_eq!(q.get_usage("t1").unwrap().tokens_pct(), 1.0);
    }

    #[test]
    fn reset_period_zeroes_usage_and_tokens() {
        let q = setup();
        q.record_run("t1", 500);
        q.reset_period("t1");
        let u = q.get_usage("t1").unwrap();
        assert_eq!(u.agent_runs_used, 0);
        assert_eq!(u.tokens_used, 0);
    }

    #[test]
    fn reset_period_preserves_active_agents() {
        let q = setup();
        q.incr_active("t1").unwrap();
        q.reset_period("t1");
        assert_eq!(q.get_usage("t1").unwrap().active_agents, 1);
    }

    #[test]
    fn quota_usage_serde_roundtrip() {
        let u = QuotaUsage::new("t1".into(), TenantPlan::Starter);
        let json = serde_json::to_string(&u).unwrap();
        let back: QuotaUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, u);
    }
}

//! Managed agent runtime — wraps the core `AgentRunner` with tenant
//! isolation, quota enforcement, and audit-ready return values.
//!
//! The `ManagedRuntime` is a stub: it validates the tenant, consults the
//! quota enforcer, charges usage, and returns a canned response. Production
//! would dispatch to the real `argentor_agent::AgentRunner` with tenant-
//! scoped sessions, skills, and observability tags.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

use crate::quota::{QuotaEnforcer, QuotaError};
use crate::tenant::{TenantManager, TenantStatus};

/// Configuration for a single managed agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRunConfig {
    /// Tenant that owns this run.
    pub tenant_id: String,
    /// Agent configuration identifier (tenant-scoped).
    pub agent_id: String,
    /// User message to feed the agent.
    pub user_message: String,
    /// Optional existing session to resume.
    pub session_id: Option<String>,
}

/// Result of a managed agent run — shape consumed by the dashboard + billing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManagedRunResult {
    /// Unique run id (audit-linkable).
    pub run_id: String,
    /// Tenant that owns the run.
    pub tenant_id: String,
    /// Agent configuration id.
    pub agent_id: String,
    /// Final response string.
    pub response: String,
    /// Tokens consumed by this run.
    pub tokens_used: u64,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Estimated USD cost (for invoicing).
    pub cost_usd: f64,
}

/// Errors the managed runtime can raise.
#[derive(Debug, Error)]
pub enum ManagedError {
    /// Tenant does not exist.
    #[error("Tenant not found")]
    TenantNotFound,
    /// Tenant exists but is suspended or cancelled.
    #[error("Tenant suspended")]
    TenantSuspended,
    /// Quota check failed.
    #[error("Quota exceeded: {0}")]
    QuotaExceeded(String),
    /// Agent execution raised an error.
    #[error("Agent execution failed: {0}")]
    AgentFailed(String),
}

impl From<QuotaError> for ManagedError {
    fn from(e: QuotaError) -> Self {
        ManagedError::QuotaExceeded(e.to_string())
    }
}

/// Managed runtime handle.
///
/// TODO: wire to `argentor_agent::AgentRunner` with tenant-scoped
/// session store, skill registry, and MCP proxy.
pub struct ManagedRuntime {
    tenants: Arc<TenantManager>,
    quotas: Arc<QuotaEnforcer>,
    /// Stub cost in USD per 1K tokens (real impl pulls from billing config).
    pub cost_per_1k_tokens_usd: f64,
}

impl ManagedRuntime {
    /// Construct a new managed runtime with a default $0.002/1K token rate.
    pub fn new(tenants: Arc<TenantManager>, quotas: Arc<QuotaEnforcer>) -> Self {
        Self {
            tenants,
            quotas,
            cost_per_1k_tokens_usd: 0.002,
        }
    }

    /// Run an agent with tenant isolation, quota enforcement, and usage
    /// charging. Currently returns a canned response — production wires to
    /// the real agent runner.
    pub async fn run(&self, config: ManagedRunConfig) -> Result<ManagedRunResult, ManagedError> {
        let tenant = self
            .tenants
            .get_tenant(&config.tenant_id)
            .ok_or(ManagedError::TenantNotFound)?;

        match tenant.status {
            TenantStatus::Suspended | TenantStatus::Cancelled => {
                return Err(ManagedError::TenantSuspended);
            }
            _ => {}
        }

        self.quotas.check_can_run(&config.tenant_id)?;
        self.quotas.incr_active(&config.tenant_id)?;

        let start = std::time::Instant::now();
        // Stub: real runtime invokes AgentRunner and streams events.
        let response = format!(
            "[stub] tenant={} agent={} echo: {}",
            config.tenant_id, config.agent_id, config.user_message
        );
        // Stub token count — real runtime pulls from the model response.
        let tokens_used = (config.user_message.len() as u64).max(1) * 2;

        self.quotas.record_run(&config.tenant_id, tokens_used);
        self.quotas.decr_active(&config.tenant_id);

        let duration_ms = start.elapsed().as_millis() as u64;
        let cost_usd = (tokens_used as f64 / 1_000.0) * self.cost_per_1k_tokens_usd;

        Ok(ManagedRunResult {
            run_id: Uuid::new_v4().to_string(),
            tenant_id: config.tenant_id,
            agent_id: config.agent_id,
            response,
            tokens_used,
            duration_ms,
            cost_usd,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::tenant::{DataRegion, TenantPlan};

    fn setup() -> (Arc<TenantManager>, Arc<QuotaEnforcer>, String) {
        let tenants = Arc::new(TenantManager::new());
        let quotas = Arc::new(QuotaEnforcer::new());
        let t = tenants.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::UsEast);
        tenants.activate(&t.id).unwrap();
        quotas.register(t.id.clone(), TenantPlan::Free);
        (tenants, quotas, t.id)
    }

    #[tokio::test]
    async fn run_succeeds_for_active_tenant() {
        let (tenants, quotas, tid) = setup();
        let rt = ManagedRuntime::new(tenants, quotas);
        let result = rt
            .run(ManagedRunConfig {
                tenant_id: tid.clone(),
                agent_id: "agent-1".into(),
                user_message: "hello".into(),
                session_id: None,
            })
            .await
            .unwrap();
        assert_eq!(result.tenant_id, tid);
        assert!(result.response.contains("hello"));
    }

    #[tokio::test]
    async fn run_returns_unique_run_id() {
        let (tenants, quotas, tid) = setup();
        let rt = ManagedRuntime::new(tenants, quotas);
        let cfg = ManagedRunConfig {
            tenant_id: tid,
            agent_id: "agent-1".into(),
            user_message: "hi".into(),
            session_id: None,
        };
        let a = rt.run(cfg.clone()).await.unwrap();
        let b = rt.run(cfg).await.unwrap();
        assert_ne!(a.run_id, b.run_id);
    }

    #[tokio::test]
    async fn run_unknown_tenant_errors() {
        let tenants = Arc::new(TenantManager::new());
        let quotas = Arc::new(QuotaEnforcer::new());
        let rt = ManagedRuntime::new(tenants, quotas);
        let res = rt
            .run(ManagedRunConfig {
                tenant_id: "ghost".into(),
                agent_id: "agent-1".into(),
                user_message: "hi".into(),
                session_id: None,
            })
            .await;
        assert!(matches!(res, Err(ManagedError::TenantNotFound)));
    }

    #[tokio::test]
    async fn run_suspended_tenant_errors() {
        let (tenants, quotas, tid) = setup();
        tenants.suspend(&tid).unwrap();
        let rt = ManagedRuntime::new(tenants, quotas);
        let res = rt
            .run(ManagedRunConfig {
                tenant_id: tid,
                agent_id: "agent-1".into(),
                user_message: "hi".into(),
                session_id: None,
            })
            .await;
        assert!(matches!(res, Err(ManagedError::TenantSuspended)));
    }

    #[tokio::test]
    async fn run_charges_quota() {
        let (tenants, quotas, tid) = setup();
        let rt = ManagedRuntime::new(tenants, quotas.clone());
        rt.run(ManagedRunConfig {
            tenant_id: tid.clone(),
            agent_id: "agent-1".into(),
            user_message: "hi".into(),
            session_id: None,
        })
        .await
        .unwrap();
        assert_eq!(quotas.get_usage(&tid).unwrap().agent_runs_used, 1);
    }

    #[tokio::test]
    async fn run_computes_cost() {
        let (tenants, quotas, tid) = setup();
        let rt = ManagedRuntime::new(tenants, quotas);
        let r = rt
            .run(ManagedRunConfig {
                tenant_id: tid,
                agent_id: "agent-1".into(),
                user_message: "hello there".into(),
                session_id: None,
            })
            .await
            .unwrap();
        assert!(r.cost_usd > 0.0);
    }

    #[tokio::test]
    async fn run_quota_exceeded_errors() {
        let tenants = Arc::new(TenantManager::new());
        let quotas = Arc::new(QuotaEnforcer::new());
        let t = tenants.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::UsEast);
        tenants.activate(&t.id).unwrap();
        quotas.register(t.id.clone(), TenantPlan::Free);
        for _ in 0..1_000 {
            quotas.record_run(&t.id, 0);
        }
        let rt = ManagedRuntime::new(tenants, quotas);
        let res = rt
            .run(ManagedRunConfig {
                tenant_id: t.id,
                agent_id: "agent-1".into(),
                user_message: "hi".into(),
                session_id: None,
            })
            .await;
        assert!(matches!(res, Err(ManagedError::QuotaExceeded(_))));
    }

    #[test]
    fn quota_error_converts_to_managed_error() {
        let q = QuotaError::RunsExceeded { used: 10, limit: 5 };
        let m: ManagedError = q.into();
        assert!(matches!(m, ManagedError::QuotaExceeded(_)));
    }

    #[test]
    fn managed_run_result_serde_roundtrip() {
        let r = ManagedRunResult {
            run_id: "r1".into(),
            tenant_id: "t1".into(),
            agent_id: "a1".into(),
            response: "hi".into(),
            tokens_used: 10,
            duration_ms: 5,
            cost_usd: 0.0,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ManagedRunResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }
}

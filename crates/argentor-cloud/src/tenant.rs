//! Multi-tenant management for Argentor Cloud.
//!
//! In-memory scaffolding — production would back this with PostgreSQL.
//! Each tenant represents an isolated customer account with its own plan,
//! data region, and API credentials.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

/// A tenant record — one per customer account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tenant {
    /// Tenant UUID (stable identifier).
    pub id: String,
    /// Human-readable tenant name.
    pub name: String,
    /// Subscription plan.
    pub plan: TenantPlan,
    /// UTC timestamp of tenant creation.
    pub created_at: DateTime<Utc>,
    /// Lifecycle status.
    pub status: TenantStatus,
    /// bcrypt hash of the tenant's primary API key (never store plaintext).
    pub api_key_hash: String,
    /// Allowed CORS origins for dashboard/API calls.
    pub allowed_origins: Vec<String>,
    /// Region where tenant data is stored (GDPR/residency).
    pub data_region: DataRegion,
}

/// Subscription plans with per-month limits.
///
/// TODO: wire real pricing through `billing.rs` and Stripe/Paddle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TenantPlan {
    /// Free tier — 1K agent runs/month, 5 active agents.
    Free,
    /// Starter — 50K runs/month, 25 agents — $99/mo.
    Starter,
    /// Growth — 500K runs/month, 100 agents — $499/mo.
    Growth,
    /// Enterprise — unlimited, custom SLA — contact sales.
    Enterprise,
}

impl TenantPlan {
    /// Monthly agent-run quota for this plan (`u64::MAX` = unlimited).
    pub fn run_quota(&self) -> u64 {
        match self {
            TenantPlan::Free => 1_000,
            TenantPlan::Starter => 50_000,
            TenantPlan::Growth => 500_000,
            TenantPlan::Enterprise => u64::MAX,
        }
    }

    /// Maximum concurrently active agents for this plan.
    pub fn agent_quota(&self) -> u32 {
        match self {
            TenantPlan::Free => 5,
            TenantPlan::Starter => 25,
            TenantPlan::Growth => 100,
            TenantPlan::Enterprise => u32::MAX,
        }
    }

    /// Monthly token quota for this plan.
    pub fn token_quota(&self) -> u64 {
        match self {
            TenantPlan::Free => 1_000_000,
            TenantPlan::Starter => 50_000_000,
            TenantPlan::Growth => 500_000_000,
            TenantPlan::Enterprise => u64::MAX,
        }
    }

    /// Storage quota in megabytes for sessions/audit logs.
    pub fn storage_mb_quota(&self) -> u64 {
        match self {
            TenantPlan::Free => 100,
            TenantPlan::Starter => 5_000,
            TenantPlan::Growth => 50_000,
            TenantPlan::Enterprise => u64::MAX,
        }
    }
}

/// Tenant lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TenantStatus {
    /// Active paying customer.
    Active,
    /// Trial period (pre-conversion).
    Trial,
    /// Suspended for billing issues or ToS violations.
    Suspended,
    /// Cancelled — data retained per retention policy.
    Cancelled,
}

/// Data residency region for tenant data.
///
/// EU regions enforce GDPR data-locality requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRegion {
    /// US East (default).
    UsEast,
    /// US West.
    UsWest,
    /// EU West — GDPR zone.
    EuWest,
    /// EU Central — GDPR zone.
    EuCentral,
    /// Asia-Pacific South.
    ApSouth,
    /// Asia-Pacific Southeast.
    ApSoutheast,
}

impl DataRegion {
    /// Whether this region falls under GDPR data-locality requirements.
    pub fn is_gdpr(&self) -> bool {
        matches!(self, DataRegion::EuWest | DataRegion::EuCentral)
    }
}

/// In-memory tenant manager.
///
/// TODO: replace with PostgreSQL-backed implementation for production.
pub struct TenantManager {
    tenants: RwLock<HashMap<String, Tenant>>,
}

impl TenantManager {
    /// Create an empty tenant manager.
    pub fn new() -> Self {
        Self {
            tenants: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new tenant. API key hash is a placeholder — real cloud
    /// generates + returns the raw key once and stores only the bcrypt hash.
    pub fn create_tenant(&self, name: String, plan: TenantPlan, region: DataRegion) -> Tenant {
        let tenant = Tenant {
            id: Uuid::new_v4().to_string(),
            name,
            plan,
            created_at: Utc::now(),
            status: TenantStatus::Trial,
            api_key_hash: format!("stub-hash-{}", Uuid::new_v4()),
            allowed_origins: Vec::new(),
            data_region: region,
        };
        if let Ok(mut guard) = self.tenants.write() {
            guard.insert(tenant.id.clone(), tenant.clone());
        }
        tenant
    }

    /// Fetch a tenant by id.
    pub fn get_tenant(&self, id: &str) -> Option<Tenant> {
        self.tenants.read().ok()?.get(id).cloned()
    }

    /// List all tenants (unpaged — production must paginate).
    pub fn list_tenants(&self) -> Vec<Tenant> {
        self.tenants
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Suspend a tenant (e.g. missed payment).
    pub fn suspend(&self, id: &str) -> Result<(), String> {
        let mut guard = self.tenants.write().map_err(|e| e.to_string())?;
        let tenant = guard
            .get_mut(id)
            .ok_or_else(|| format!("tenant {id} not found"))?;
        tenant.status = TenantStatus::Suspended;
        Ok(())
    }

    /// Reactivate a previously suspended tenant.
    pub fn activate(&self, id: &str) -> Result<(), String> {
        let mut guard = self.tenants.write().map_err(|e| e.to_string())?;
        let tenant = guard
            .get_mut(id)
            .ok_or_else(|| format!("tenant {id} not found"))?;
        tenant.status = TenantStatus::Active;
        Ok(())
    }

    /// Upgrade (or downgrade) a tenant's subscription plan.
    pub fn upgrade_plan(&self, id: &str, plan: TenantPlan) -> Result<Tenant, String> {
        let mut guard = self.tenants.write().map_err(|e| e.to_string())?;
        let tenant = guard
            .get_mut(id)
            .ok_or_else(|| format!("tenant {id} not found"))?;
        tenant.plan = plan;
        Ok(tenant.clone())
    }

    /// Add an allowed origin for CORS enforcement.
    pub fn add_allowed_origin(&self, id: &str, origin: String) -> Result<(), String> {
        let mut guard = self.tenants.write().map_err(|e| e.to_string())?;
        let tenant = guard
            .get_mut(id)
            .ok_or_else(|| format!("tenant {id} not found"))?;
        if !tenant.allowed_origins.contains(&origin) {
            tenant.allowed_origins.push(origin);
        }
        Ok(())
    }

    /// Number of tenants under management.
    pub fn count(&self) -> usize {
        self.tenants.read().map(|g| g.len()).unwrap_or(0)
    }
}

impl Default for TenantManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn free_plan_limits() {
        assert_eq!(TenantPlan::Free.run_quota(), 1_000);
        assert_eq!(TenantPlan::Free.agent_quota(), 5);
    }

    #[test]
    fn starter_plan_limits() {
        assert_eq!(TenantPlan::Starter.run_quota(), 50_000);
        assert_eq!(TenantPlan::Starter.agent_quota(), 25);
    }

    #[test]
    fn growth_plan_limits() {
        assert_eq!(TenantPlan::Growth.run_quota(), 500_000);
        assert_eq!(TenantPlan::Growth.agent_quota(), 100);
    }

    #[test]
    fn enterprise_unlimited() {
        assert_eq!(TenantPlan::Enterprise.run_quota(), u64::MAX);
        assert_eq!(TenantPlan::Enterprise.agent_quota(), u32::MAX);
    }

    #[test]
    fn token_quotas_are_sane() {
        assert!(TenantPlan::Starter.token_quota() > TenantPlan::Free.token_quota());
        assert!(TenantPlan::Growth.token_quota() > TenantPlan::Starter.token_quota());
    }

    #[test]
    fn storage_quotas_escalate() {
        assert!(TenantPlan::Growth.storage_mb_quota() > TenantPlan::Starter.storage_mb_quota());
    }

    #[test]
    fn eu_regions_are_gdpr() {
        assert!(DataRegion::EuWest.is_gdpr());
        assert!(DataRegion::EuCentral.is_gdpr());
        assert!(!DataRegion::UsEast.is_gdpr());
        assert!(!DataRegion::ApSouth.is_gdpr());
    }

    #[test]
    fn create_tenant_assigns_uuid() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".to_string(), TenantPlan::Free, DataRegion::UsEast);
        assert!(!t.id.is_empty());
        assert_eq!(t.name, "Acme");
        assert_eq!(t.status, TenantStatus::Trial);
    }

    #[test]
    fn create_tenant_stores_hash_not_key() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".to_string(), TenantPlan::Free, DataRegion::UsEast);
        assert!(t.api_key_hash.starts_with("stub-hash-"));
    }

    #[test]
    fn get_tenant_returns_match() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".to_string(), TenantPlan::Free, DataRegion::UsEast);
        let fetched = mgr.get_tenant(&t.id).unwrap();
        assert_eq!(fetched.id, t.id);
    }

    #[test]
    fn get_tenant_missing_returns_none() {
        let mgr = TenantManager::new();
        assert!(mgr.get_tenant("nope").is_none());
    }

    #[test]
    fn list_tenants_returns_all() {
        let mgr = TenantManager::new();
        mgr.create_tenant("A".into(), TenantPlan::Free, DataRegion::UsEast);
        mgr.create_tenant("B".into(), TenantPlan::Starter, DataRegion::EuWest);
        assert_eq!(mgr.list_tenants().len(), 2);
    }

    #[test]
    fn suspend_tenant_updates_status() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::UsEast);
        mgr.suspend(&t.id).unwrap();
        assert_eq!(
            mgr.get_tenant(&t.id).unwrap().status,
            TenantStatus::Suspended
        );
    }

    #[test]
    fn suspend_missing_tenant_errors() {
        let mgr = TenantManager::new();
        assert!(mgr.suspend("missing").is_err());
    }

    #[test]
    fn activate_restores_status() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::UsEast);
        mgr.suspend(&t.id).unwrap();
        mgr.activate(&t.id).unwrap();
        assert_eq!(mgr.get_tenant(&t.id).unwrap().status, TenantStatus::Active);
    }

    #[test]
    fn upgrade_plan_changes_plan() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::UsEast);
        let updated = mgr.upgrade_plan(&t.id, TenantPlan::Growth).unwrap();
        assert_eq!(updated.plan, TenantPlan::Growth);
    }

    #[test]
    fn add_allowed_origin_dedupes() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::UsEast);
        mgr.add_allowed_origin(&t.id, "https://app.acme.com".into())
            .unwrap();
        mgr.add_allowed_origin(&t.id, "https://app.acme.com".into())
            .unwrap();
        let fetched = mgr.get_tenant(&t.id).unwrap();
        assert_eq!(fetched.allowed_origins.len(), 1);
    }

    #[test]
    fn count_reflects_creations() {
        let mgr = TenantManager::new();
        assert_eq!(mgr.count(), 0);
        mgr.create_tenant("A".into(), TenantPlan::Free, DataRegion::UsEast);
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn serde_roundtrip_tenant() {
        let mgr = TenantManager::new();
        let t = mgr.create_tenant("Acme".into(), TenantPlan::Free, DataRegion::EuWest);
        let json = serde_json::to_string(&t).unwrap();
        let back: Tenant = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }
}

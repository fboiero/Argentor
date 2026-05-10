//! Tenant isolation — ensures hard data boundaries between tenants.
//!
//! Every resource (sessions, skills, memory namespaces) is scoped by
//! `tenant_id`. Cross-tenant access is rejected at this layer before
//! reaching any data store.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

/// Per-tenant configuration controlling resource limits and allowed features.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantConfig {
    /// Maximum concurrent active sessions for this tenant.
    pub max_sessions: u32,
    /// Maximum tokens consumed per day across all sessions.
    pub max_tokens_per_day: u64,
    /// Allowed LLM model identifiers (empty = all models allowed).
    pub allowed_models: Vec<String>,
    /// Identifier of the guardrail profile applied to all agent outputs.
    ///
    /// Must match a profile registered in the guardrail registry.
    /// `None` means the system default profile is used.
    pub guardrail_profile: Option<String>,
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            max_sessions: 100,
            max_tokens_per_day: 1_000_000,
            allowed_models: vec![],
            guardrail_profile: None,
        }
    }
}

impl TenantConfig {
    /// Returns `true` if the given model is permitted by this config.
    ///
    /// An empty `allowed_models` list means all models are allowed.
    pub fn model_allowed(&self, model_id: &str) -> bool {
        self.allowed_models.is_empty() || self.allowed_models.iter().any(|m| m == model_id)
    }
}

/// Isolation error — returned when a tenant tries to access another's resource.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IsolationError {
    /// Tenant is not registered in the isolation registry.
    #[error("Tenant {0} not found in isolation registry")]
    TenantNotFound(String),
    /// Resource belongs to a different tenant.
    #[error("Access denied: resource owner '{owner}' != requesting tenant '{requester}'")]
    CrossTenantAccess {
        /// Tenant that owns the resource.
        owner: String,
        /// Tenant that attempted access.
        requester: String,
    },
    /// Model is not on the tenant's allowed list.
    #[error("Model '{model}' is not allowed for tenant '{tenant}'")]
    ModelNotAllowed {
        /// Model that was requested.
        model: String,
        /// Tenant whose config rejected it.
        tenant: String,
    },
}

/// In-memory registry that enforces per-tenant resource ownership.
///
/// Every session, skill, and memory namespace created through this registry
/// is tagged with its owning `tenant_id`. Any attempt by a different tenant
/// to look up that resource is rejected with [`IsolationError::CrossTenantAccess`].
pub struct TenantIsolation {
    /// tenant_id → TenantConfig
    configs: RwLock<HashMap<String, TenantConfig>>,
    /// resource_id → owner_tenant_id (sessions, skills, memory namespaces)
    ownership: RwLock<HashMap<String, String>>,
}

impl TenantIsolation {
    /// Create an empty isolation registry.
    pub fn new() -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
            ownership: RwLock::new(HashMap::new()),
        }
    }

    /// Register a tenant with its configuration.
    ///
    /// Calling this twice for the same `tenant_id` replaces the config.
    pub fn register_tenant(&self, tenant_id: String, config: TenantConfig) {
        if let Ok(mut g) = self.configs.write() {
            g.insert(tenant_id, config);
        }
    }

    /// Retrieve a tenant's configuration.
    pub fn get_config(&self, tenant_id: &str) -> Option<TenantConfig> {
        self.configs.read().ok()?.get(tenant_id).cloned()
    }

    /// Claim ownership of a resource (session id, skill id, memory namespace, …).
    ///
    /// Fails if the tenant is not registered.
    pub fn claim(&self, tenant_id: &str, resource_id: String) -> Result<(), IsolationError> {
        // Tenant must be registered first.
        {
            let configs = self
                .configs
                .read()
                .map_err(|_| IsolationError::TenantNotFound(tenant_id.to_string()))?;
            if !configs.contains_key(tenant_id) {
                return Err(IsolationError::TenantNotFound(tenant_id.to_string()));
            }
        }
        if let Ok(mut g) = self.ownership.write() {
            g.insert(resource_id, tenant_id.to_string());
        }
        Ok(())
    }

    /// Assert that `requesting_tenant` owns `resource_id`.
    ///
    /// Returns `Ok(())` if ownership matches, an `IsolationError` otherwise.
    pub fn assert_owner(
        &self,
        requesting_tenant: &str,
        resource_id: &str,
    ) -> Result<(), IsolationError> {
        let guard = self
            .ownership
            .read()
            .map_err(|_| IsolationError::TenantNotFound(requesting_tenant.to_string()))?;
        match guard.get(resource_id) {
            None => Err(IsolationError::TenantNotFound(resource_id.to_string())),
            Some(owner) if owner == requesting_tenant => Ok(()),
            Some(owner) => Err(IsolationError::CrossTenantAccess {
                owner: owner.clone(),
                requester: requesting_tenant.to_string(),
            }),
        }
    }

    /// Assert that a model is allowed for the given tenant.
    pub fn assert_model_allowed(
        &self,
        tenant_id: &str,
        model_id: &str,
    ) -> Result<(), IsolationError> {
        let config = self
            .get_config(tenant_id)
            .ok_or_else(|| IsolationError::TenantNotFound(tenant_id.to_string()))?;
        if config.model_allowed(model_id) {
            Ok(())
        } else {
            Err(IsolationError::ModelNotAllowed {
                model: model_id.to_string(),
                tenant: tenant_id.to_string(),
            })
        }
    }

    /// Release ownership of a resource (e.g. session destroyed).
    pub fn release(&self, resource_id: &str) {
        if let Ok(mut g) = self.ownership.write() {
            g.remove(resource_id);
        }
    }

    /// Number of resources currently tracked.
    pub fn resource_count(&self) -> usize {
        self.ownership.read().map(|g| g.len()).unwrap_or(0)
    }
}

impl Default for TenantIsolation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn two_tenant_isolation() -> (TenantIsolation, String, String) {
        let iso = TenantIsolation::new();
        let ta = "tenant-a".to_string();
        let tb = "tenant-b".to_string();
        iso.register_tenant(ta.clone(), TenantConfig::default());
        iso.register_tenant(tb.clone(), TenantConfig::default());
        (iso, ta, tb)
    }

    // ── TenantConfig ────────────────────────────────────────────────────────

    #[test]
    fn default_config_allows_all_models() {
        let cfg = TenantConfig::default();
        assert!(cfg.model_allowed("claude-3-5-sonnet"));
        assert!(cfg.model_allowed("gpt-4o"));
    }

    #[test]
    fn restricted_config_blocks_unlisted_model() {
        let cfg = TenantConfig {
            allowed_models: vec!["claude-3-5-haiku".to_string()],
            ..TenantConfig::default()
        };
        assert!(cfg.model_allowed("claude-3-5-haiku"));
        assert!(!cfg.model_allowed("gpt-4o"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = TenantConfig {
            max_sessions: 50,
            max_tokens_per_day: 500_000,
            allowed_models: vec!["claude-3-5-sonnet".into()],
            guardrail_profile: Some("strict".into()),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TenantConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    // ── TenantIsolation — happy paths ────────────────────────────────────────

    #[test]
    fn tenant_a_can_access_own_session() {
        let (iso, ta, _tb) = two_tenant_isolation();
        iso.claim(&ta, "sess-001".into()).unwrap();
        assert!(iso.assert_owner(&ta, "sess-001").is_ok());
    }

    #[test]
    fn tenant_b_cannot_access_tenant_a_session() {
        let (iso, ta, tb) = two_tenant_isolation();
        iso.claim(&ta, "sess-001".into()).unwrap();
        let result = iso.assert_owner(&tb, "sess-001");
        assert!(matches!(
            result,
            Err(IsolationError::CrossTenantAccess { .. })
        ));
    }

    #[test]
    fn tenant_b_cannot_access_tenant_a_skill() {
        let (iso, ta, tb) = two_tenant_isolation();
        iso.claim(&ta, "skill-summarize".into()).unwrap();
        let result = iso.assert_owner(&tb, "skill-summarize");
        assert!(matches!(
            result,
            Err(IsolationError::CrossTenantAccess { ref owner, ref requester })
            if owner == "tenant-a" && requester == "tenant-b"
        ));
    }

    #[test]
    fn tenant_b_cannot_access_tenant_a_memory() {
        let (iso, ta, tb) = two_tenant_isolation();
        iso.claim(&ta, "mem-ns:tenant-a".into()).unwrap();
        assert!(iso.assert_owner(&tb, "mem-ns:tenant-a").is_err());
    }

    #[test]
    fn release_removes_ownership() {
        let (iso, ta, _tb) = two_tenant_isolation();
        iso.claim(&ta, "sess-temp".into()).unwrap();
        iso.release("sess-temp");
        assert_eq!(iso.resource_count(), 0);
    }

    #[test]
    fn claim_fails_for_unregistered_tenant() {
        let iso = TenantIsolation::new();
        let result = iso.claim("ghost", "sess-x".into());
        assert!(matches!(result, Err(IsolationError::TenantNotFound(_))));
    }

    #[test]
    fn assert_owner_on_unknown_resource_returns_not_found() {
        let (iso, ta, _tb) = two_tenant_isolation();
        let result = iso.assert_owner(&ta, "nonexistent");
        assert!(matches!(result, Err(IsolationError::TenantNotFound(_))));
    }

    // ── Model allow-listing ──────────────────────────────────────────────────

    #[test]
    fn model_allowed_passes_for_permitted_model() {
        let iso = TenantIsolation::new();
        iso.register_tenant(
            "t1".into(),
            TenantConfig {
                allowed_models: vec!["claude-3-5-sonnet".into()],
                ..TenantConfig::default()
            },
        );
        assert!(iso.assert_model_allowed("t1", "claude-3-5-sonnet").is_ok());
    }

    #[test]
    fn model_denied_for_unlisted_model() {
        let iso = TenantIsolation::new();
        iso.register_tenant(
            "t1".into(),
            TenantConfig {
                allowed_models: vec!["claude-3-5-haiku".into()],
                ..TenantConfig::default()
            },
        );
        let result = iso.assert_model_allowed("t1", "gpt-4o");
        assert!(matches!(
            result,
            Err(IsolationError::ModelNotAllowed { .. })
        ));
    }

    #[test]
    fn model_allowed_when_list_is_empty() {
        let iso = TenantIsolation::new();
        iso.register_tenant("t1".into(), TenantConfig::default());
        assert!(iso.assert_model_allowed("t1", "any-model").is_ok());
    }
}

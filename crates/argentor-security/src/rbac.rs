//! Role-Based Access Control (RBAC) for enterprise deployments.
//!
//! Defines roles, fine-grained permissions, and policy evaluation.

use crate::capability::{Capability, PermissionSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fine-grained actions that can be individually granted or denied.
///
/// Use [`check_permission`] to evaluate whether a [`Role`] holds a given
/// `Permission` under the default policy.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Create and register new agent instances.
    CreateAgent,
    /// Execute an agent run.
    RunAgent,
    /// Install, update, or remove skills from the registry.
    ManageSkills,
    /// View audit logs and session history.
    ViewLogs,
    /// Create, suspend, or delete tenant accounts.
    ManageTenants,
    /// Read and write guardrail profiles.
    ConfigureGuardrails,
}

/// Check whether the default built-in policy grants `permission` to `role`.
///
/// Default policy table:
///
/// | Role     | Permissions |
/// |----------|-------------|
/// | Admin    | all |
/// | Operator | RunAgent, ManageSkills, ViewLogs |
/// | Developer | RunAgent, ViewLogs |
/// | Viewer   | ViewLogs |
/// | Custom   | none (use explicit policy bindings) |
pub fn check_permission(role: &Role, permission: &Permission) -> bool {
    match role {
        Role::Admin => true,
        Role::Operator => matches!(
            permission,
            Permission::RunAgent | Permission::ManageSkills | Permission::ViewLogs
        ),
        Role::Developer => matches!(permission, Permission::RunAgent | Permission::ViewLogs),
        Role::Viewer => matches!(permission, Permission::ViewLogs),
        Role::Custom(_) => false,
    }
}

/// Built-in roles with predefined permission levels.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full access — manage users, modify policies, all capabilities.
    Admin,
    /// Execute skills and tools, but cannot modify policies.
    Operator,
    /// Run agents and view logs, but cannot modify infrastructure.
    Developer,
    /// Read-only access — view audit logs, session history, skill list.
    Viewer,
    /// Custom role with a specific name.
    Custom(String),
}

/// A policy binding: associates a role with a set of permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBinding {
    /// The role this binding applies to.
    pub role: Role,
    /// Granted capabilities for this role.
    pub permissions: PermissionSet,
    /// Allowed skill names (empty = all skills allowed for Operator+).
    #[serde(default)]
    pub allowed_skills: Vec<String>,
    /// Denied skill names (takes precedence over allowed).
    #[serde(default)]
    pub denied_skills: Vec<String>,
    /// Maximum requests per minute (0 = unlimited).
    #[serde(default)]
    pub rate_limit_rpm: u32,
}

/// Central RBAC policy store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RbacPolicy {
    /// Policy bindings keyed by role.
    bindings: HashMap<String, PolicyBinding>,
}

impl RbacPolicy {
    /// Create a new empty RBAC policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a default policy with Admin, Operator, Developer, and Viewer roles.
    pub fn with_defaults() -> Self {
        let mut policy = Self::new();

        // Admin — full access
        let mut admin_perms = PermissionSet::new();
        admin_perms.grant(Capability::FileRead {
            allowed_paths: vec!["/".into()],
        });
        admin_perms.grant(Capability::FileWrite {
            allowed_paths: vec!["/".into()],
        });
        admin_perms.grant(Capability::NetworkAccess {
            allowed_hosts: vec!["*".into()],
        });
        admin_perms.grant(Capability::ShellExec {
            allowed_commands: vec!["*".into()],
        });
        admin_perms.grant(Capability::DatabaseQuery);

        policy.add_binding(PolicyBinding {
            role: Role::Admin,
            permissions: admin_perms,
            allowed_skills: vec![],
            denied_skills: vec![],
            rate_limit_rpm: 0,
        });

        // Operator — tool execution with restrictions, can manage skills
        let mut op_perms = PermissionSet::new();
        op_perms.grant(Capability::FileRead {
            allowed_paths: vec!["/tmp".into(), "/workspace".into()],
        });
        op_perms.grant(Capability::FileWrite {
            allowed_paths: vec!["/tmp".into(), "/workspace".into()],
        });
        op_perms.grant(Capability::NetworkAccess {
            allowed_hosts: vec!["*".into()],
        });

        policy.add_binding(PolicyBinding {
            role: Role::Operator,
            permissions: op_perms,
            allowed_skills: vec![],
            denied_skills: vec!["shell_exec".into()],
            rate_limit_rpm: 60,
        });

        // Developer — run agents and view logs, workspace read-only
        let mut dev_perms = PermissionSet::new();
        dev_perms.grant(Capability::FileRead {
            allowed_paths: vec!["/workspace".into()],
        });
        dev_perms.grant(Capability::NetworkAccess {
            allowed_hosts: vec!["*".into()],
        });

        policy.add_binding(PolicyBinding {
            role: Role::Developer,
            permissions: dev_perms,
            allowed_skills: vec![],
            denied_skills: vec!["shell_exec".into(), "file_write".into()],
            rate_limit_rpm: 120,
        });

        // Viewer — read-only
        let mut viewer_perms = PermissionSet::new();
        viewer_perms.grant(Capability::FileRead {
            allowed_paths: vec!["/workspace".into()],
        });

        policy.add_binding(PolicyBinding {
            role: Role::Viewer,
            permissions: viewer_perms,
            allowed_skills: vec!["help".into(), "memory_search".into()],
            denied_skills: vec![],
            rate_limit_rpm: 30,
        });

        policy
    }

    /// Add or replace a policy binding.
    pub fn add_binding(&mut self, binding: PolicyBinding) {
        let key = role_key(&binding.role);
        self.bindings.insert(key, binding);
    }

    /// Remove a policy binding by role.
    pub fn remove_binding(&mut self, role: &Role) -> Option<PolicyBinding> {
        self.bindings.remove(&role_key(role))
    }

    /// Get the binding for a specific role.
    pub fn get_binding(&self, role: &Role) -> Option<&PolicyBinding> {
        self.bindings.get(&role_key(role))
    }

    /// Evaluate whether a role is allowed to invoke a skill.
    pub fn evaluate(&self, role: &Role, skill_name: &str) -> RbacDecision {
        let Some(binding) = self.get_binding(role) else {
            return RbacDecision::Denied {
                reason: format!("No policy binding for role {role:?}"),
            };
        };

        // Denied list takes precedence
        if binding
            .denied_skills
            .iter()
            .any(|s| s == skill_name || s == "*")
        {
            return RbacDecision::Denied {
                reason: format!("Skill '{skill_name}' is explicitly denied for role {role:?}"),
            };
        }

        // If allowed list is non-empty, skill must be in it
        if !binding.allowed_skills.is_empty()
            && !binding
                .allowed_skills
                .iter()
                .any(|s| s == skill_name || s == "*")
        {
            return RbacDecision::Denied {
                reason: format!("Skill '{skill_name}' not in allowed list for role {role:?}"),
            };
        }

        RbacDecision::Allowed {
            permissions: binding.permissions.clone(),
            rate_limit_rpm: binding.rate_limit_rpm,
        }
    }

    /// List all roles in the policy.
    pub fn roles(&self) -> Vec<&Role> {
        self.bindings.values().map(|b| &b.role).collect()
    }
}

/// Result of an RBAC evaluation.
#[derive(Debug, Clone)]
pub enum RbacDecision {
    /// Access granted with the effective permission set.
    Allowed {
        /// The permissions to apply.
        permissions: PermissionSet,
        /// Rate limit for this role (0 = unlimited).
        rate_limit_rpm: u32,
    },
    /// Access denied with a reason.
    Denied {
        /// Human-readable denial reason.
        reason: String,
    },
}

impl RbacDecision {
    /// Returns `true` if access was granted.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }

    /// Returns `true` if access was denied.
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Denied { .. })
    }
}

/// Produce a stable string key for a role.
fn role_key(role: &Role) -> String {
    match role {
        Role::Admin => "admin".into(),
        Role::Operator => "operator".into(),
        Role::Developer => "developer".into(),
        Role::Viewer => "viewer".into(),
        Role::Custom(name) => format!("custom:{name}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_has_four_roles() {
        let policy = RbacPolicy::with_defaults();
        assert_eq!(policy.roles().len(), 4);
    }

    // ── check_permission (default policy) ───────────────────────────────────

    #[test]
    fn admin_has_all_permissions() {
        for perm in [
            Permission::CreateAgent,
            Permission::RunAgent,
            Permission::ManageSkills,
            Permission::ViewLogs,
            Permission::ManageTenants,
            Permission::ConfigureGuardrails,
        ] {
            assert!(
                check_permission(&Role::Admin, &perm),
                "Admin must have {perm:?}"
            );
        }
    }

    #[test]
    fn operator_has_run_manage_view() {
        assert!(check_permission(&Role::Operator, &Permission::RunAgent));
        assert!(check_permission(&Role::Operator, &Permission::ManageSkills));
        assert!(check_permission(&Role::Operator, &Permission::ViewLogs));
        assert!(!check_permission(&Role::Operator, &Permission::CreateAgent));
        assert!(!check_permission(
            &Role::Operator,
            &Permission::ManageTenants
        ));
    }

    #[test]
    fn developer_has_run_and_view_only() {
        assert!(check_permission(&Role::Developer, &Permission::RunAgent));
        assert!(check_permission(&Role::Developer, &Permission::ViewLogs));
        assert!(!check_permission(
            &Role::Developer,
            &Permission::ManageSkills
        ));
        assert!(!check_permission(
            &Role::Developer,
            &Permission::CreateAgent
        ));
        assert!(!check_permission(
            &Role::Developer,
            &Permission::ManageTenants
        ));
        assert!(!check_permission(
            &Role::Developer,
            &Permission::ConfigureGuardrails
        ));
    }

    #[test]
    fn viewer_has_view_logs_only() {
        assert!(check_permission(&Role::Viewer, &Permission::ViewLogs));
        assert!(!check_permission(&Role::Viewer, &Permission::RunAgent));
        assert!(!check_permission(&Role::Viewer, &Permission::CreateAgent));
        assert!(!check_permission(&Role::Viewer, &Permission::ManageTenants));
    }

    #[test]
    fn custom_role_has_no_default_permissions() {
        for perm in [
            Permission::CreateAgent,
            Permission::RunAgent,
            Permission::ManageSkills,
            Permission::ViewLogs,
            Permission::ManageTenants,
            Permission::ConfigureGuardrails,
        ] {
            assert!(
                !check_permission(&Role::Custom("analyst".into()), &perm),
                "Custom role must have no default permissions, but {perm:?} was granted"
            );
        }
    }

    #[test]
    fn admin_can_access_anything() {
        let policy = RbacPolicy::with_defaults();
        let decision = policy.evaluate(&Role::Admin, "shell_exec");
        assert!(decision.is_allowed());
    }

    #[test]
    fn operator_denied_shell_exec() {
        let policy = RbacPolicy::with_defaults();
        let decision = policy.evaluate(&Role::Operator, "shell_exec");
        assert!(decision.is_denied());
    }

    #[test]
    fn operator_allowed_other_skills() {
        let policy = RbacPolicy::with_defaults();
        let decision = policy.evaluate(&Role::Operator, "memory_search");
        assert!(decision.is_allowed());
    }

    #[test]
    fn viewer_only_allowed_listed_skills() {
        let policy = RbacPolicy::with_defaults();

        let allowed = policy.evaluate(&Role::Viewer, "help");
        assert!(allowed.is_allowed());

        let denied = policy.evaluate(&Role::Viewer, "shell_exec");
        assert!(denied.is_denied());
    }

    #[test]
    fn unknown_role_denied() {
        let policy = RbacPolicy::with_defaults();
        let decision = policy.evaluate(&Role::Custom("hacker".into()), "help");
        assert!(decision.is_denied());
    }

    #[test]
    fn custom_role_binding() {
        let mut policy = RbacPolicy::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileRead {
            allowed_paths: vec!["/data".into()],
        });

        policy.add_binding(PolicyBinding {
            role: Role::Custom("analyst".into()),
            permissions: perms,
            allowed_skills: vec!["memory_search".into(), "help".into()],
            denied_skills: vec![],
            rate_limit_rpm: 10,
        });

        let allowed = policy.evaluate(&Role::Custom("analyst".into()), "memory_search");
        assert!(allowed.is_allowed());

        if let RbacDecision::Allowed { rate_limit_rpm, .. } = allowed {
            assert_eq!(rate_limit_rpm, 10);
        }
    }

    #[test]
    fn remove_binding() {
        let mut policy = RbacPolicy::with_defaults();
        assert!(policy.get_binding(&Role::Viewer).is_some());
        policy.remove_binding(&Role::Viewer);
        assert!(policy.get_binding(&Role::Viewer).is_none());
    }

    #[test]
    fn denied_takes_precedence_over_allowed() {
        let mut policy = RbacPolicy::new();
        policy.add_binding(PolicyBinding {
            role: Role::Operator,
            permissions: PermissionSet::new(),
            allowed_skills: vec!["*".into()],
            denied_skills: vec!["dangerous_skill".into()],
            rate_limit_rpm: 0,
        });

        let decision = policy.evaluate(&Role::Operator, "dangerous_skill");
        assert!(decision.is_denied());

        let decision = policy.evaluate(&Role::Operator, "safe_skill");
        assert!(decision.is_allowed());
    }

    #[test]
    fn policy_serialization_roundtrip() {
        let policy = RbacPolicy::with_defaults();
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: RbacPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.roles().len(), 4);
    }
}

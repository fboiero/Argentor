#![allow(clippy::unwrap_used, clippy::expect_used)]
//! # Security regression tests — RBAC enforcement
//!
//! Pins the behaviour of [`RbacPolicy::evaluate`] under enterprise
//! permission scenarios: role downgrades, wildcard scoping, default-deny,
//! and group permission merges. RBAC is the boundary that decides whether
//! ANY skill can execute — a regression here unlocks every downstream
//! sandbox.
//!
//! References:
//! - CWE-285: Improper Authorization
//! - CWE-269: Improper Privilege Management
//! - CWE-862: Missing Authorization
//! - CWE-863: Incorrect Authorization
//! - OWASP A01:2021 — Broken Access Control
//! - NIST 800-162: Guide to Attribute Based Access Control

use argentor_security::rbac::{PolicyBinding, RbacPolicy, Role};
use argentor_security::{Capability, PermissionSet};

/// CWE-285: Admin role must be allowed everything in the default policy.
#[test]
fn test_admin_can_do_everything() {
    let policy = RbacPolicy::with_defaults();

    let critical_skills = [
        "shell_exec",
        "file_write",
        "database_query",
        "network_access",
        "browser_access",
        "memory_search",
        "anything_at_all",
    ];

    for skill in critical_skills {
        let decision = policy.evaluate(&Role::Admin, skill);
        assert!(
            decision.is_allowed(),
            "CRITICAL: Admin must be allowed '{skill}', got: {decision:?}"
        );
    }
}

/// CWE-269: Viewer role must NEVER be allowed to modify state.
#[test]
fn test_viewer_cannot_modify() {
    let policy = RbacPolicy::with_defaults();

    let write_ops = ["shell_exec", "file_write", "database_query"];
    for op in write_ops {
        let decision = policy.evaluate(&Role::Viewer, op);
        assert!(
            decision.is_denied(),
            "CRITICAL: Viewer must NOT be allowed write op '{op}', got: {decision:?}"
        );
    }
}

/// CWE-269: Operator can scale (run normal skills) but cannot modify policies.
/// In the default policy, Operator is denied `shell_exec` — pin that.
#[test]
fn test_operator_can_scale_not_configure() {
    let policy = RbacPolicy::with_defaults();

    // Allowed: ordinary skill execution
    let allowed = policy.evaluate(&Role::Operator, "memory_search");
    assert!(
        allowed.is_allowed(),
        "Operator must be allowed 'memory_search'"
    );

    // Denied: shell exec is in the deny list for Operator
    let denied = policy.evaluate(&Role::Operator, "shell_exec");
    assert!(
        denied.is_denied(),
        "CRITICAL: Operator must NOT be allowed 'shell_exec'"
    );
}

/// CWE-269: Role downgrade must apply IMMEDIATELY — there is no implicit
/// caching of "previous" decisions. We model this by: bind a user to admin,
/// take an action, replace the binding with viewer, retake the action.
#[test]
fn test_role_downgrade_applies_immediately() {
    let mut policy = RbacPolicy::new();

    // Start: user has Admin permissions
    let mut admin_perms = PermissionSet::new();
    admin_perms.grant(Capability::ShellExec {
        allowed_commands: vec!["*".into()],
    });
    policy.add_binding(PolicyBinding {
        role: Role::Admin,
        permissions: admin_perms,
        allowed_skills: vec![],
        denied_skills: vec![],
        rate_limit_rpm: 0,
    });

    let before = policy.evaluate(&Role::Admin, "shell_exec");
    assert!(before.is_allowed(), "Initial admin must be allowed");

    // Downgrade: remove Admin binding, add Viewer with no shell_exec
    policy.remove_binding(&Role::Admin);
    policy.add_binding(PolicyBinding {
        role: Role::Admin, // same role NAME, but now restricted permissions
        permissions: PermissionSet::new(),
        allowed_skills: vec![],
        denied_skills: vec!["shell_exec".into()],
        rate_limit_rpm: 0,
    });

    let after = policy.evaluate(&Role::Admin, "shell_exec");
    assert!(
        after.is_denied(),
        "CRITICAL: downgrade must take effect immediately, got: {after:?}"
    );
}

/// CWE-285: Wildcard skill scoping — `"*"` in allowed_skills allows ALL,
/// but a denied entry must STILL win.
#[test]
fn test_wildcard_permissions_scope_correctly() {
    let mut policy = RbacPolicy::new();
    policy.add_binding(PolicyBinding {
        role: Role::Operator,
        permissions: PermissionSet::new(),
        allowed_skills: vec!["*".into()],
        denied_skills: vec!["dangerous_skill".into()],
        rate_limit_rpm: 0,
    });

    // Wildcard allows arbitrary skills
    assert!(policy.evaluate(&Role::Operator, "echo").is_allowed());
    assert!(policy
        .evaluate(&Role::Operator, "memory_search")
        .is_allowed());

    // But the denylist STILL wins
    let blocked = policy.evaluate(&Role::Operator, "dangerous_skill");
    assert!(
        blocked.is_denied(),
        "CRITICAL: deny list must take precedence over wildcard allow"
    );
}

/// CWE-862: Default deny — a role with NO binding is denied everything.
#[test]
fn test_no_permission_default_deny() {
    let policy = RbacPolicy::new(); // empty policy
    let decision = policy.evaluate(&Role::Custom("ghost".into()), "any_skill");
    assert!(
        decision.is_denied(),
        "CRITICAL: unknown role must default to DENY (CWE-862)"
    );
}

/// Documented behaviour: the current RBAC model is single-role — a user
/// has ONE role at a time, not a union of multiple group memberships.
/// "Inherited permissions merge" is therefore a feature that does NOT
/// exist by design (avoids the principle-of-least-surprise traps that
/// hierarchical inheritance creates). This test pins that contract.
///
/// If/when group-merge is added, this test must be updated explicitly —
/// not silently.
#[test]
fn test_inherited_permissions_merge() {
    let policy = RbacPolicy::with_defaults();

    // Documented: the policy returns the binding for ONE role. There is
    // no automatic merge of Admin + Viewer permissions for a single
    // evaluation. This is intentional — operators must pick a role.
    let admin = policy.evaluate(&Role::Admin, "shell_exec");
    let viewer = policy.evaluate(&Role::Viewer, "shell_exec");

    assert!(admin.is_allowed());
    assert!(viewer.is_denied());

    // No combined call exists — by design. If it ever does, this test
    // must change explicitly.
}

/// CWE-863: Custom-role with restricted skill list. Verifies that the
/// allowed_skills filter is enforced (skill MUST be in the list).
#[test]
fn test_custom_role_skill_filter_enforced() {
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

    let analyst = Role::Custom("analyst".into());

    // In list → allowed
    assert!(policy.evaluate(&analyst, "memory_search").is_allowed());
    assert!(policy.evaluate(&analyst, "help").is_allowed());

    // NOT in list → denied
    assert!(
        policy.evaluate(&analyst, "shell_exec").is_denied(),
        "CRITICAL: skill not in allowed_skills must be denied (CWE-863)"
    );
    assert!(
        policy.evaluate(&analyst, "file_write").is_denied(),
        "CRITICAL: skill not in allowed_skills must be denied"
    );
}

/// CWE-863: Operator's `denied_skills` must take precedence over a normal
/// (non-wildcard) allow. Defence against an operator misconfiguration that
/// adds `shell_exec` to allowed_skills while forgetting to remove it from
/// denied_skills.
#[test]
fn test_deny_takes_precedence_over_explicit_allow() {
    let mut policy = RbacPolicy::new();
    policy.add_binding(PolicyBinding {
        role: Role::Operator,
        permissions: PermissionSet::new(),
        allowed_skills: vec!["shell_exec".into(), "memory_search".into()],
        denied_skills: vec!["shell_exec".into()],
        rate_limit_rpm: 0,
    });

    let denied = policy.evaluate(&Role::Operator, "shell_exec");
    assert!(
        denied.is_denied(),
        "CRITICAL: deny list must beat allow list — fail-closed semantics"
    );

    // The other allowed skill still works
    assert!(policy
        .evaluate(&Role::Operator, "memory_search")
        .is_allowed());
}

use crate::types::{AgentProfile, AgentRole};
use argentor_agent::ModelConfig;
use argentor_security::{Capability, PermissionSet};

/// Create default agent profiles for the multi-agent system.
/// Uses the provided base config as template, adjusting per role.
pub fn default_profiles(base_config: &ModelConfig) -> Vec<AgentProfile> {
    vec![
        orchestrator_profile(base_config),
        spec_profile(base_config),
        coder_profile(base_config),
        tester_profile(base_config),
        reviewer_profile(base_config),
        architect_profile(base_config),
        security_auditor_profile(base_config),
        devops_profile(base_config),
        document_writer_profile(base_config),
    ]
}

/// Build a PermissionSet for the Spec role: memory_search only (no file/shell).
fn spec_permissions() -> PermissionSet {
    // Spec has no file or shell capabilities — only memory-based skills.
    PermissionSet::new()
}

/// Build a PermissionSet for the Coder role: FileRead + FileWrite + ShellExec (cargo/git) + memory.
fn coder_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms.grant(Capability::FileWrite {
        allowed_paths: vec![".".to_string()],
    });
    perms.grant(Capability::ShellExec {
        allowed_commands: vec!["cargo".to_string(), "git".to_string()],
    });
    perms
}

/// Build a PermissionSet for the Tester role: FileRead + ShellExec (cargo test only) + memory.
fn tester_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms.grant(Capability::ShellExec {
        allowed_commands: vec!["cargo".to_string()],
    });
    perms
}

/// Build a PermissionSet for the Reviewer role: FileRead only + memory.
fn reviewer_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms
}

/// Build a PermissionSet for the Architect role: FileRead + memory.
fn architect_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms
}

/// Build a PermissionSet for the SecurityAuditor role: FileRead + memory.
fn security_auditor_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms
}

/// Build a PermissionSet for the DevOps role: FileRead + FileWrite + ShellExec + NetworkAccess.
fn devops_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms.grant(Capability::FileWrite {
        allowed_paths: vec![".".to_string()],
    });
    perms.grant(Capability::ShellExec {
        allowed_commands: vec![
            "cargo".to_string(),
            "git".to_string(),
            "docker".to_string(),
            "kubectl".to_string(),
            "helm".to_string(),
        ],
    });
    perms.grant(Capability::NetworkAccess {
        allowed_hosts: vec!["*".to_string()],
    });
    perms
}

/// Build a PermissionSet for the DocumentWriter role: FileRead + FileWrite.
fn document_writer_permissions() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec![".".to_string()],
    });
    perms.grant(Capability::FileWrite {
        allowed_paths: vec![".".to_string()],
    });
    perms
}

fn orchestrator_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 30;
    model.temperature = 0.3;

    AgentProfile {
        role: AgentRole::Orchestrator,
        model,
        system_prompt: ORCHESTRATOR_PROMPT.to_string(),
        allowed_skills: vec![
            "agent_delegate".to_string(),
            "task_status".to_string(),
            "human_approval".to_string(),
            "artifact_store".to_string(),
            "memory_search".to_string(),
        ],
        tool_group: Some("orchestration".to_string()),
        max_turns: 30,
        permissions: PermissionSet::new(),
    }
}

fn spec_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 15;
    model.temperature = 0.4;

    AgentProfile {
        role: AgentRole::Spec,
        model,
        system_prompt: SPEC_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string(), "memory_store".to_string()],
        tool_group: Some("minimal".to_string()),
        max_turns: 15,
        permissions: spec_permissions(),
    }
}

fn coder_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 20;
    model.temperature = 0.2;

    AgentProfile {
        role: AgentRole::Coder,
        model,
        system_prompt: CODER_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string()],
        tool_group: Some("coding".to_string()),
        max_turns: 20,
        permissions: coder_permissions(),
    }
}

fn tester_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 15;
    model.temperature = 0.2;

    AgentProfile {
        role: AgentRole::Tester,
        model,
        system_prompt: TESTER_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string()],
        tool_group: Some("coding".to_string()),
        max_turns: 15,
        permissions: tester_permissions(),
    }
}

fn reviewer_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 10;
    model.temperature = 0.3;

    AgentProfile {
        role: AgentRole::Reviewer,
        model,
        system_prompt: REVIEWER_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string(), "human_approval".to_string()],
        tool_group: Some("coding".to_string()),
        max_turns: 10,
        permissions: reviewer_permissions(),
    }
}

fn architect_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 20;
    model.temperature = 0.3;

    AgentProfile {
        role: AgentRole::Architect,
        model,
        system_prompt: ARCHITECT_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string(), "memory_store".to_string()],
        tool_group: Some("coding".to_string()),
        max_turns: 20,
        permissions: architect_permissions(),
    }
}

fn security_auditor_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 15;
    model.temperature = 0.2;

    AgentProfile {
        role: AgentRole::SecurityAuditor,
        model,
        system_prompt: SECURITY_AUDITOR_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string()],
        tool_group: Some("coding".to_string()),
        max_turns: 15,
        permissions: security_auditor_permissions(),
    }
}

fn devops_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 20;
    model.temperature = 0.2;

    AgentProfile {
        role: AgentRole::DevOps,
        model,
        system_prompt: DEVOPS_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string(), "memory_store".to_string()],
        tool_group: Some("devops".to_string()),
        max_turns: 20,
        permissions: devops_permissions(),
    }
}

fn document_writer_profile(base: &ModelConfig) -> AgentProfile {
    let mut model = base.clone();
    model.max_turns = 15;
    model.temperature = 0.4;

    AgentProfile {
        role: AgentRole::DocumentWriter,
        model,
        system_prompt: DOCUMENT_WRITER_PROMPT.to_string(),
        allowed_skills: vec!["memory_search".to_string()],
        tool_group: Some("coding".to_string()),
        max_turns: 15,
        permissions: document_writer_permissions(),
    }
}

const ORCHESTRATOR_PROMPT: &str = "\
You are the Orchestrator agent in a multi-agent system called Argentor. \
Your job is to decompose complex tasks into subtasks, delegate them to \
specialized worker agents (Spec, Coder, Tester, Reviewer, Architect, \
SecurityAuditor, DevOps, DocumentWriter), and synthesize the final result.

Rules:
1. Break tasks into clear, independent subtasks when possible.
2. Assign each subtask to the most appropriate worker role.
3. Define dependencies between subtasks (e.g., Coder depends on Spec).
4. Monitor progress and handle failures by reassigning or adjusting.
5. Request human approval for high-risk operations (security changes, \
   deployments, data deletion).
6. Synthesize all artifacts into a coherent final deliverable.
7. Never write code yourself — delegate to Coder.
8. Use Architect for system design and API design tasks.
9. Use SecurityAuditor for security-focused reviews and audits.
10. Use DevOps for deployment, infrastructure, and CI/CD tasks.
11. Use DocumentWriter for documentation tasks.
";

const SPEC_PROMPT: &str = "\
You are the Spec agent in Argentor. Your job is to analyze requirements \
and produce clear, actionable technical specifications.

IMPORTANT: Respond with your specification as plain text. Do NOT call any tools — \
just write your analysis and specification directly in your response.

Rules:
1. Break requirements into concrete implementation tasks.
2. Define interfaces, data types, and contracts.
3. Identify edge cases, security considerations, and constraints.
4. Reference existing code patterns and utilities when applicable.
5. Output your specification directly as text.
";

const CODER_PROMPT: &str = "\
You are the Coder agent in Argentor. You write secure, idiomatic Rust code \
following the project's patterns and conventions.

IMPORTANT: Respond with your code as plain text. Do NOT call any tools — \
just write the code directly in your response. Use markdown code blocks.

Rules:
1. Follow the specification provided by the Spec agent.
2. Write memory-safe, secure code — no unwrap() in production paths.
3. Use existing project patterns (traits, error handling, async).
4. Keep code simple — no over-engineering or premature abstractions.
5. Include inline comments only where logic is non-obvious.
6. Output code directly in your response with file paths as comments.
";

const TESTER_PROMPT: &str = "\
You are the Tester agent in Argentor. You write comprehensive tests \
for Rust code.

IMPORTANT: Respond with your test code as plain text. Do NOT call any tools — \
just write the tests directly in your response. Use markdown code blocks.

Rules:
1. Write unit tests covering happy paths, edge cases, and error conditions.
2. Follow existing test patterns in the project (tokio::test for async).
3. Test security boundaries and permission checks.
4. Use descriptive test names (test_<function>_<scenario>).
5. Output test code directly in your response.
";

const REVIEWER_PROMPT: &str = "\
You are the Reviewer agent in Argentor. You review code for quality, \
security, and compliance.

IMPORTANT: Respond with your review as plain text. Do NOT call any tools — \
just write your review report directly in your response.

Rules:
1. Check for OWASP Top 10 vulnerabilities.
2. Verify capability-based permission checks are in place.
3. Ensure proper error handling (no unwrap in prod paths).
4. Check for compliance with GDPR, ISO 27001, ISO 42001 requirements.
5. Flag any issues that require human review.
6. Output your review report directly in your response.
";

const ARCHITECT_PROMPT: &str = "\
You are the Architect agent in Argentor. You design system architecture, \
define APIs, and produce technical design documents.

IMPORTANT: Respond with your design document as plain text. Do NOT call any tools — \
just write your architecture and design directly in your response.

Rules:
1. Design modular, scalable architectures following SOLID principles.
2. Define clear API contracts and data models.
3. Consider security boundaries, trust zones, and blast radius.
4. Document trade-offs and alternatives considered.
5. Ensure designs are compatible with the existing Argentor crate structure.
6. Output your design document directly in your response.
";

const SECURITY_AUDITOR_PROMPT: &str = "\
You are the SecurityAuditor agent in Argentor. You perform security reviews, \
vulnerability analysis, and compliance audits.

IMPORTANT: Respond with your security audit report as plain text. Do NOT call any tools — \
just write your findings directly in your response.

Rules:
1. Check for OWASP Top 10 vulnerabilities.
2. Audit capability-based permission boundaries.
3. Verify input validation, sanitization, and output encoding.
4. Check for secrets, credentials, or sensitive data exposure.
5. Assess cryptographic practices (TLS, key management).
6. Flag CRITICAL_SECURITY_ISSUE or NEEDS_HUMAN_REVIEW for serious findings.
7. Output your audit report directly in your response.
";

const DEVOPS_PROMPT: &str = "\
You are the DevOps agent in Argentor. You handle deployment, infrastructure, \
CI/CD pipelines, and operational tasks.

IMPORTANT: Respond with your infrastructure code or configuration as plain text. \
Do NOT call any tools — just write your output directly in your response.

Rules:
1. Write Dockerfiles, Helm charts, and CI/CD configurations.
2. Follow infrastructure-as-code best practices.
3. Ensure deployments are reproducible and rollback-safe.
4. Configure monitoring, logging, and alerting.
5. Apply security hardening (least privilege, network policies).
6. Output your configuration and scripts directly in your response.
";

const DOCUMENT_WRITER_PROMPT: &str = "\
You are the DocumentWriter agent in Argentor. You write and maintain \
technical documentation, guides, and API references.

IMPORTANT: Respond with your documentation as plain text. Do NOT call any tools — \
just write your documentation directly in your response.

Rules:
1. Write clear, concise documentation following the project style.
2. Include code examples where helpful.
3. Document public APIs with usage patterns and edge cases.
4. Keep READMEs, changelogs, and guides up to date.
5. Use proper Markdown formatting.
6. Output your documentation directly in your response.
";

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_agent::LlmProvider;

    fn test_config() -> ModelConfig {
        ModelConfig {
            provider: LlmProvider::Claude,
            model_id: "claude-sonnet-4-20250514".to_string(),
            api_key: "test-key".to_string(),
            api_base_url: None,
            temperature: 0.7,
            max_tokens: 4096,
            max_turns: 20,
            max_context_tokens: 200_000,
            fallback_models: Vec::new(),
            retry_policy: None,
        }
    }

    #[test]
    fn test_default_profiles_count() {
        let profiles = default_profiles(&test_config());
        assert_eq!(profiles.len(), 9);
    }

    #[test]
    fn test_all_roles_covered() {
        let profiles = default_profiles(&test_config());
        let roles: Vec<AgentRole> = profiles.iter().map(|p| p.role.clone()).collect();
        assert!(roles.contains(&AgentRole::Orchestrator));
        assert!(roles.contains(&AgentRole::Spec));
        assert!(roles.contains(&AgentRole::Coder));
        assert!(roles.contains(&AgentRole::Tester));
        assert!(roles.contains(&AgentRole::Reviewer));
        assert!(roles.contains(&AgentRole::Architect));
        assert!(roles.contains(&AgentRole::SecurityAuditor));
        assert!(roles.contains(&AgentRole::DevOps));
        assert!(roles.contains(&AgentRole::DocumentWriter));
    }

    #[test]
    fn test_orchestrator_has_delegate_skill() {
        let profiles = default_profiles(&test_config());
        let orch = profiles
            .iter()
            .find(|p| p.role == AgentRole::Orchestrator)
            .unwrap();
        assert!(orch.allowed_skills.contains(&"agent_delegate".to_string()));
        assert!(orch.allowed_skills.contains(&"human_approval".to_string()));
    }

    #[test]
    fn test_coder_low_temperature() {
        let profiles = default_profiles(&test_config());
        let coder = profiles
            .iter()
            .find(|p| p.role == AgentRole::Coder)
            .unwrap();
        assert!(coder.model.temperature <= 0.3);
    }

    #[test]
    fn test_profiles_have_system_prompts() {
        let profiles = default_profiles(&test_config());
        for profile in &profiles {
            assert!(!profile.system_prompt.is_empty());
        }
    }
}

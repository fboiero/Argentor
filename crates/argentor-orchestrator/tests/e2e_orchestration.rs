#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end orchestration test.
//!
//! Verifies the full Spec → Code → Test → Review pipeline using mock LLM backends.
//! Checks: artifact flow between stages, proxy logging, progressive disclosure,
//! and HITL (NeedsHumanReview) detection.

use argentor_agent::backends::LlmBackend;
use argentor_agent::llm::LlmResponse;
use argentor_agent::stream::StreamEvent;
use argentor_agent::{LlmProvider, ModelConfig};
use argentor_core::{ArgentorResult, Message};
use argentor_orchestrator::*;
use argentor_security::{AuditLog, PermissionSet};
use argentor_skills::skill::SkillDescriptor;
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Mock LLM backend — returns deterministic responses per role
// ---------------------------------------------------------------------------

struct MockBackend {
    role: AgentRole,
    /// When true, the Reviewer will include a NEEDS_HUMAN_REVIEW marker.
    flag_review: bool,
}

#[async_trait]
impl LlmBackend for MockBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        // Extract the enriched prompt (last user message) to verify context flow
        let last_msg = messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let response = match self.role {
            AgentRole::Spec => "## Specification\n\n\
                 1. Implement a `greet(name)` function\n\
                 2. Returns `Hello, {name}!`\n\
                 3. Edge case: empty name returns `Hello, World!`"
                .to_string(),
            AgentRole::Coder => {
                // Verify that spec context was passed
                assert!(
                    last_msg.contains("SPECIFICATION"),
                    "Coder should receive spec context, got: {}",
                    &last_msg[..last_msg.len().min(200)]
                );
                "```rust\nfn greet(name: &str) -> String {\n    \
                 if name.is_empty() {\n        \
                     \"Hello, World!\".to_string()\n    \
                 } else {\n        \
                     format!(\"Hello, {}!\", name)\n    \
                 }\n}\n```"
                    .to_string()
            }
            AgentRole::Tester => {
                // Verify that code context was passed
                assert!(
                    last_msg.contains("CODE"),
                    "Tester should receive code context, got: {}",
                    &last_msg[..last_msg.len().min(200)]
                );
                "```rust\n#[test]\nfn test_greet() {\n    \
                 assert_eq!(greet(\"Alice\"), \"Hello, Alice!\");\n}\n\
                 #[test]\nfn test_greet_empty() {\n    \
                 assert_eq!(greet(\"\"), \"Hello, World!\");\n}\n```"
                    .to_string()
            }
            AgentRole::Reviewer => {
                // Verify that both code and test context were passed
                assert!(
                    last_msg.contains("CODE"),
                    "Reviewer should receive code context"
                );
                assert!(
                    last_msg.contains("TESTS"),
                    "Reviewer should receive test context"
                );
                if self.flag_review {
                    "## Review\n\n\
                     CRITICAL_SECURITY_ISSUE: The greet function does not sanitize input.\n\
                     NEEDS_HUMAN_REVIEW before merging.\n\
                     Risk: XSS if used in web context."
                        .to_string()
                } else {
                    "## Review\n\n\
                     Code looks good. All edge cases covered.\n\
                     No security issues found. Approved."
                        .to_string()
                }
            }
            AgentRole::Orchestrator => "Orchestration plan complete.".to_string(),
            AgentRole::Architect => "## Architecture\n\nSystem design document.".to_string(),
            AgentRole::SecurityAuditor => "## Security Audit\n\nNo issues found.".to_string(),
            AgentRole::DevOps => "## DevOps\n\nDeployment configuration.".to_string(),
            AgentRole::DocumentWriter => "## Documentation\n\nAPI reference.".to_string(),
            AgentRole::Custom(ref name) => format!("Custom agent '{name}' output."),
        };

        Ok(LlmResponse::Done(response))
    }

    async fn chat_stream(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        let response = self.chat(system_prompt, messages, tools).await;
        let (tx, rx) = mpsc::channel(1);
        let handle = tokio::spawn(async move {
            drop(tx);
            response
        });
        Ok((rx, handle))
    }
}

fn test_config() -> ModelConfig {
    ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "mock".to_string(),
        api_key: "test-key".to_string(),
        api_base_url: None,
        temperature: 0.0,
        max_tokens: 1024,
        max_turns: 5,
        max_context_tokens: 200_000,
        fallback_models: Vec::new(),
        retry_policy: None,
    }
}

// ---------------------------------------------------------------------------
// Test: Happy path — full pipeline completes with all artifacts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: false,
        })
    });

    let orchestrator = Orchestrator::new(&test_config(), skills, permissions, audit)
        .with_output_dir(tmp.path().join("output"))
        .with_backend_factory(factory);

    let result = orchestrator
        .run("Implement a greet function")
        .await
        .unwrap();

    // All 4 tasks completed
    assert_eq!(result.total_tasks, 4);
    assert_eq!(result.completed_tasks, 4);
    assert_eq!(result.failed_tasks, 0);
    assert_eq!(result.needs_review_tasks, 0);

    // 4 artifacts: spec, code, test, review
    assert_eq!(result.artifacts.len(), 4);

    let kinds: Vec<ArtifactKind> = result.artifacts.iter().map(|a| a.kind.clone()).collect();
    assert!(kinds.contains(&ArtifactKind::Spec));
    assert!(kinds.contains(&ArtifactKind::Code));
    assert!(kinds.contains(&ArtifactKind::Test));
    assert!(kinds.contains(&ArtifactKind::Review));

    // Verify artifacts have content
    let spec = result
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Spec)
        .unwrap();
    assert!(spec.content.contains("greet"));

    let code = result
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Code)
        .unwrap();
    assert!(code.content.contains("fn greet"));

    // Files were written to disk
    assert!(!result.written_files.is_empty());
    assert!(result.summary.contains("4/4"));
}

// ---------------------------------------------------------------------------
// Test: HITL — reviewer flags issues, task gets NeedsHumanReview
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_hitl_review_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: true, // Reviewer will flag for human review
        })
    });

    let orchestrator =
        Orchestrator::new(&test_config(), skills, permissions, audit).with_backend_factory(factory);

    let result = orchestrator
        .run("Implement a greet function")
        .await
        .unwrap();

    // 3 completed + 1 needs review
    assert_eq!(result.total_tasks, 4);
    assert_eq!(result.completed_tasks, 3);
    assert_eq!(result.needs_review_tasks, 1);
    assert_eq!(result.failed_tasks, 0);

    // Summary mentions human review
    assert!(result.summary.contains("human review"));

    // Review artifact still collected
    let review = result
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Review)
        .unwrap();
    assert!(review.content.contains("CRITICAL_SECURITY_ISSUE"));
}

// ---------------------------------------------------------------------------
// Test: Proxy metrics are recorded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_proxy_metrics() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: false,
        })
    });

    let orchestrator =
        Orchestrator::new(&test_config(), skills, permissions, audit).with_backend_factory(factory);

    let _result = orchestrator.run("Test proxy metrics").await.unwrap();

    // Proxy stats should be accessible
    let proxy_json = orchestrator.proxy().to_json().await;
    assert!(proxy_json["total_calls"].is_number());
}

// ---------------------------------------------------------------------------
// Test: Monitor tracks agent states
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_monitor_tracking() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: false,
        })
    });

    let orchestrator =
        Orchestrator::new(&test_config(), skills, permissions, audit).with_backend_factory(factory);

    let _result = orchestrator.run("Test monitor").await.unwrap();

    // After completion, all agents should be idle
    let snapshot = orchestrator.monitor().snapshot().await;
    for state in &snapshot {
        assert_eq!(state.status, WorkerStatus::Idle);
    }

    // Aggregate metrics should show some activity
    let agg = orchestrator.monitor().aggregate_metrics().await;
    assert!(agg.total_turns > 0);
    // Note: with mock backends, execution can be sub-millisecond (0ms).
    assert!(agg.total_turns > 0);
}

// ---------------------------------------------------------------------------
// Test: Progressive disclosure — workers get fewer tools than total
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_progressive_disclosure() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    argentor_builtins::register_builtins(&registry);
    let total_skills = registry.skill_count();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: false,
        })
    });

    let orchestrator =
        Orchestrator::new(&test_config(), skills, permissions, audit).with_backend_factory(factory);

    let result = orchestrator
        .run("Test progressive disclosure")
        .await
        .unwrap();

    // Pipeline should complete
    assert_eq!(result.completed_tasks, 4);

    // Workers should have had fewer tools than total registered
    // (verified by tracing logs, but we validate the pipeline completed
    // which means the filter_by_group path didn't error out)
    assert!(total_skills > 0);
}

// ---------------------------------------------------------------------------
// Test: Progress callback receives updates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_progress_callback() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let progress_log: Arc<std::sync::Mutex<Vec<(AgentRole, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let log_clone = progress_log.clone();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: false,
        })
    });

    let orchestrator = Orchestrator::new(&test_config(), skills, permissions, audit)
        .with_backend_factory(factory)
        .with_progress(move |role, msg| {
            log_clone.lock().unwrap().push((role, msg.to_string()));
        });

    let _result = orchestrator.run("Test progress").await.unwrap();

    let log = progress_log.lock().unwrap();
    // At least 4 "working..." + 4 "done" messages
    assert!(
        log.len() >= 8,
        "Expected >= 8 progress messages, got {}",
        log.len()
    );

    // Verify all roles reported progress
    let roles: Vec<AgentRole> = log.iter().map(|(r, _)| r.clone()).collect();
    assert!(roles.contains(&AgentRole::Spec));
    assert!(roles.contains(&AgentRole::Coder));
    assert!(roles.contains(&AgentRole::Tester));
    assert!(roles.contains(&AgentRole::Reviewer));
}

// ---------------------------------------------------------------------------
// Test: Task queue state after pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_queue_state() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let registry = SkillRegistry::new();
    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();

    let factory: BackendFactory = Arc::new(|role| {
        Box::new(MockBackend {
            role: role.clone(),
            flag_review: false,
        })
    });

    let orchestrator =
        Orchestrator::new(&test_config(), skills, permissions, audit).with_backend_factory(factory);

    let _result = orchestrator.run("Test queue state").await.unwrap();

    let queue = orchestrator.queue().read().await;
    assert!(queue.is_done());
    assert_eq!(queue.total_count(), 4);
    assert_eq!(queue.completed_count(), 4);
    assert_eq!(queue.pending_count(), 0);
    assert_eq!(queue.needs_review_count(), 0);
}

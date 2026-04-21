#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Multi-agent regression tests.
//!
//! Covers the full orchestration and coordination surface:
//! - Orchestrator-Worker pipeline with mock backends
//! - Inter-agent message bus (A2A)
//! - Replanner failure recovery
//! - Budget tracker hard-stop when tokens exhausted
//! - DevTeam ImplementFeature workflow
//! - HandoffProtocol depth limit
//! - Checkpoint restore mid-workflow

use argentor_agent::backends::LlmBackend;
use argentor_agent::llm::LlmResponse;
use argentor_agent::stream::StreamEvent;
use argentor_agent::{
    AgentState as CheckpointAgentState, CheckpointConfig, CheckpointManager, LlmProvider,
    ModelConfig, ModelSnapshot,
};
use argentor_core::{ArgentorResult, Message};
use argentor_orchestrator::handoff::{
    ContextTransferMode, HandoffConfig, HandoffContext, HandoffError, HandoffProtocol,
    HandoffRequest,
};
use argentor_orchestrator::{
    default_budget, AgentMessage, AgentRole, ArtifactKind, BackendFactory, BroadcastTarget,
    BudgetStatus, BudgetTracker, DevRole, DevTeam, DevWorkflow, FailureContext, MessageBus,
    MessageType, Orchestrator, RecoveryStrategy, Replanner, TokenBudget,
};
use argentor_security::{AuditLog, PermissionSet};
use argentor_skills::skill::SkillDescriptor;
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn test_config() -> ModelConfig {
    ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "mock".into(),
        api_key: "test-key".into(),
        api_base_url: None,
        temperature: 0.0,
        max_tokens: 1024,
        max_turns: 5,
        max_context_tokens: 200_000,
        fallback_models: Vec::new(),
        retry_policy: None,
    }
}

/// Deterministic mock backend that returns a fixed response per role.
struct RoleAwareBackend {
    role: AgentRole,
}

#[async_trait]
impl LlmBackend for RoleAwareBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        let text = match &self.role {
            AgentRole::Spec => "## Spec\n- Requirement 1\n- Requirement 2".to_string(),
            AgentRole::Coder => "```rust\nfn hello() { println!(\"hi\"); }\n```".to_string(),
            AgentRole::Tester => {
                "```rust\n#[test]\nfn test_hello() { assert!(true); }\n```".to_string()
            }
            AgentRole::Reviewer => "## Review\nLooks good. Approved.".to_string(),
            AgentRole::Orchestrator => "Plan complete.".to_string(),
            AgentRole::Architect => "## Architecture\nsystem design".to_string(),
            AgentRole::SecurityAuditor => "## Security\nno issues".to_string(),
            AgentRole::DevOps => "## DevOps\nconfigured".to_string(),
            AgentRole::DocumentWriter => "## Docs\nAPI reference".to_string(),
            AgentRole::Custom(name) => format!("custom {name} output"),
        };
        Ok(LlmResponse::Done(text))
    }

    fn provider_name(&self) -> &str {
        "role-aware-mock"
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

// ---------------------------------------------------------------------------
// 1. Orchestrator -> Workers pipeline
// ---------------------------------------------------------------------------

/// Full Spec -> Coder -> Tester -> Reviewer pipeline via the orchestrator.
#[tokio::test]
async fn test_orchestrator_worker_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(tmp.path().join("audit")));
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();

    let factory: BackendFactory =
        Arc::new(|role| Box::new(RoleAwareBackend { role: role.clone() }));

    let orchestrator = Orchestrator::new(&test_config(), skills, permissions, audit)
        .with_backend_factory(factory)
        .with_output_dir(tmp.path().join("output"));

    let result = orchestrator
        .run("Implement a hello function")
        .await
        .unwrap();

    assert_eq!(result.total_tasks, 4, "expected 4 pipeline tasks");
    assert_eq!(result.completed_tasks, 4);
    assert_eq!(result.failed_tasks, 0);

    // All 4 artifact kinds present
    let kinds: Vec<ArtifactKind> = result.artifacts.iter().map(|a| a.kind.clone()).collect();
    assert!(kinds.contains(&ArtifactKind::Spec));
    assert!(kinds.contains(&ArtifactKind::Code));
    assert!(kinds.contains(&ArtifactKind::Test));
    assert!(kinds.contains(&ArtifactKind::Review));
}

// ---------------------------------------------------------------------------
// 2. Message bus (A2A)
// ---------------------------------------------------------------------------

/// Agent A sends a query to Agent B, Agent B receives and responds, Agent A
/// receives the response via the bus.
#[tokio::test]
async fn test_message_bus_a2a_communication() {
    let bus = MessageBus::new();

    // A sends a query to B
    let query = AgentMessage::new(
        AgentRole::Orchestrator,
        BroadcastTarget::Direct(AgentRole::Coder),
        "Please implement module X".to_string(),
        MessageType::Query,
    );
    let query_id = query.id;
    bus.send(query).await;

    // B drains its mailbox and sees the query
    let b_inbox = bus.receive(&AgentRole::Coder).await;
    assert_eq!(b_inbox.len(), 1);
    assert_eq!(b_inbox[0].id, query_id);
    assert_eq!(b_inbox[0].content, "Please implement module X");

    // B responds with correlation_id pointing to A's query
    let response = AgentMessage::new(
        AgentRole::Coder,
        BroadcastTarget::Direct(AgentRole::Orchestrator),
        "Module X implemented".to_string(),
        MessageType::Response,
    )
    .with_correlation_id(query_id);
    bus.send(response).await;

    // A receives it
    let a_inbox = bus.receive(&AgentRole::Orchestrator).await;
    assert_eq!(a_inbox.len(), 1);
    assert_eq!(a_inbox[0].content, "Module X implemented");
    assert_eq!(a_inbox[0].correlation_id, Some(query_id));

    // Total messages tracked
    assert_eq!(bus.message_count().await, 2);
}

// ---------------------------------------------------------------------------
// 3. Replanner — worker failure triggers recovery strategy
// ---------------------------------------------------------------------------

/// When a Coder task fails with a permission error, the replanner reassigns it
/// to a different (more privileged) role.
#[tokio::test]
async fn test_replanner_on_worker_failure() {
    let replanner = Replanner::new(2);

    // Simulate a task that has exhausted its retries.
    let ctx = FailureContext {
        task_id: uuid::Uuid::new_v4(),
        description: "deploy prod".into(),
        assigned_to: AgentRole::Coder,
        error_message: "permission denied on deployment".into(),
        attempt_count: 3, // exhausted retries
        is_critical: true,
    };

    let strategy = replanner.analyze_failure(&ctx);
    match strategy {
        RecoveryStrategy::Reassign { new_role } => {
            // Error mentions "deploy" so the replanner routes to DevOps.
            assert_eq!(
                new_role,
                AgentRole::DevOps,
                "expected DevOps for deploy-related permission error"
            );
        }
        other => panic!("expected Reassign strategy, got: {other:?}"),
    }

    // Second scenario: retries remaining -> Retry
    let ctx_retry = FailureContext {
        task_id: uuid::Uuid::new_v4(),
        description: "build".into(),
        assigned_to: AgentRole::Coder,
        error_message: "transient glitch".into(),
        attempt_count: 0,
        is_critical: false,
    };
    match replanner.analyze_failure(&ctx_retry) {
        RecoveryStrategy::Retry { .. } => {}
        other => panic!("expected Retry when attempts remain, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. Budget tracker — hard stop on exceeded budget
// ---------------------------------------------------------------------------

/// Agent configured with a tiny budget runs until exhausted, then check_budget
/// returns Exceeded.
#[tokio::test]
async fn test_budget_tracker_stops_expensive_agent() {
    let tracker = BudgetTracker::new();
    let role = AgentRole::Coder;

    // Tight budget: 1000 total tokens max.
    let budget = TokenBudget::new(1_000, 1_000, 1_000, 100, 3600);
    tracker.set_budget(role.clone(), budget).await;
    tracker.start_tracking(role.clone()).await;

    // Simulated token usage growth
    tracker.record_tokens(&role, 400, 200).await;
    let mid = tracker.check_budget(&role).await;
    // Should still be WithinBudget or Warning — not Exceeded yet
    assert!(
        !matches!(mid, BudgetStatus::Exceeded { .. }),
        "should not be exceeded at 600/1000 tokens"
    );

    tracker.record_tokens(&role, 500, 200).await;
    let after = tracker.check_budget(&role).await;
    match after {
        BudgetStatus::Exceeded {
            resource,
            limit,
            used,
        } => {
            assert!(
                !resource.is_empty(),
                "exceeded status must include resource name"
            );
            assert!(used > limit, "used ({used}) must exceed limit ({limit})");
        }
        other => panic!("expected Exceeded, got: {other:?}"),
    }

    // default_budget smoke-test: returns sensible per-role values
    let default = default_budget(&AgentRole::Coder);
    assert!(default.max_total_tokens >= 1000);
}

// ---------------------------------------------------------------------------
// 5. DevTeam workflow
// ---------------------------------------------------------------------------

/// DevTeam::full_stack() + ImplementFeature workflow produces all expected
/// roles and workflow steps.
#[tokio::test]
async fn test_dev_team_full_workflow() {
    let team = DevTeam::full_stack();
    assert!(team.can_run_workflow(DevWorkflow::ImplementFeature));

    let steps = team.workflow_steps(DevWorkflow::ImplementFeature);
    assert!(!steps.is_empty(), "ImplementFeature must have steps");

    // Typical order: Architect -> Implementer -> Tester -> Reviewer
    let roles: Vec<DevRole> = steps.iter().map(|s| s.role).collect();
    assert!(
        roles.iter().any(|r| *r == DevRole::Implementer),
        "ImplementFeature should include an Implementer step"
    );
    assert!(
        roles.iter().any(|r| *r == DevRole::Tester),
        "ImplementFeature should include a Tester step"
    );
    assert!(
        roles.iter().any(|r| *r == DevRole::Reviewer),
        "ImplementFeature should include a Reviewer step"
    );

    // Minimal team should NOT be able to run a workflow that needs Architect/Reviewer
    let minimal = DevTeam::minimal();
    assert!(
        !minimal.can_run_workflow(DevWorkflow::CodeReview),
        "minimal team should not support CodeReview"
    );
}

// ---------------------------------------------------------------------------
// 6. Handoff depth limit
// ---------------------------------------------------------------------------

/// A chain A -> B -> C -> D exceeds max_handoff_depth=3 and the 4th handoff is
/// rejected with `DepthExceeded`.
#[tokio::test]
async fn test_handoff_chain_depth_limit() {
    let config = HandoffConfig {
        max_handoff_depth: 3,
        context_transfer: ContextTransferMode::Minimal,
        allow_handback: true,
        timeout: std::time::Duration::from_secs(10),
    };
    let mut protocol = HandoffProtocol::new(config);

    // A -> B
    protocol
        .initiate_handoff(HandoffRequest {
            from_agent: "A".into(),
            to_agent: "B".into(),
            reason: "step 1".into(),
            task: "do X".into(),
            context: HandoffContext::default(),
            metadata: std::collections::HashMap::new(),
        })
        .unwrap();

    // B -> C
    protocol
        .initiate_handoff(HandoffRequest {
            from_agent: "B".into(),
            to_agent: "C".into(),
            reason: "step 2".into(),
            task: "do Y".into(),
            context: HandoffContext::default(),
            metadata: std::collections::HashMap::new(),
        })
        .unwrap();

    // C -> D (3rd handoff — at depth limit)
    protocol
        .initiate_handoff(HandoffRequest {
            from_agent: "C".into(),
            to_agent: "D".into(),
            reason: "step 3".into(),
            task: "do Z".into(),
            context: HandoffContext::default(),
            metadata: std::collections::HashMap::new(),
        })
        .unwrap();

    // 4th handoff — rejected because depth is now 3 (>= max).
    let result = protocol.initiate_handoff(HandoffRequest {
        from_agent: "D".into(),
        to_agent: "E".into(),
        reason: "step 4".into(),
        task: "too deep".into(),
        context: HandoffContext::default(),
        metadata: std::collections::HashMap::new(),
    });
    match result {
        Err(HandoffError::DepthExceeded { max, current }) => {
            assert_eq!(max, 3);
            assert_eq!(current, 3);
        }
        other => panic!("expected DepthExceeded, got: {other:?}"),
    }

    // Self-handoff is also rejected regardless of depth.
    let mut p2 = HandoffProtocol::with_defaults();
    let self_result = p2.initiate_handoff(HandoffRequest {
        from_agent: "Alice".into(),
        to_agent: "Alice".into(),
        reason: "to self".into(),
        task: "self".into(),
        context: HandoffContext::default(),
        metadata: std::collections::HashMap::new(),
    });
    assert!(
        matches!(self_result, Err(HandoffError::SelfHandoff { .. })),
        "self-handoff must be rejected"
    );
}

// ---------------------------------------------------------------------------
// 7. Checkpoint — create + restore mid-workflow
// ---------------------------------------------------------------------------

/// Create a checkpoint at step 3 of a simulated workflow, then restore and
/// verify the state matches.
#[tokio::test]
async fn test_checkpoint_restore_mid_workflow() {
    let mut manager = CheckpointManager::new(CheckpointConfig::default());

    let state = CheckpointAgentState {
        messages: vec![],
        system_prompt: "You are a workflow agent.".into(),
        model_config_snapshot: ModelSnapshot {
            provider: "claude".into(),
            model_id: "test".into(),
            temperature: 0.0,
            max_tokens: 1024,
        },
        tool_call_count: 7,
        total_tokens: 1234,
        active_tools: vec!["calculator".into(), "time".into()],
        variables: std::collections::HashMap::new(),
    };

    // Checkpoint at turn 3
    manager.create("step3".into(), state.clone(), 3);
    assert_eq!(manager.len(), 1);

    // Advance state (simulate more turns)
    let later_state = CheckpointAgentState {
        tool_call_count: 15,
        total_tokens: 4000,
        ..state.clone()
    };
    manager.create("step7".into(), later_state, 7);
    assert_eq!(manager.len(), 2);

    // Restore step3 — must match original state values
    let restored = manager.restore("step3").unwrap();
    assert_eq!(restored.tool_call_count, 7);
    assert_eq!(restored.total_tokens, 1234);
    assert_eq!(restored.active_tools.len(), 2);

    // Diff step3 -> step7 should show growth
    let diff = manager.diff("step3", "step7").unwrap();
    assert_eq!(diff.tool_calls_diff, 8); // 15 - 7
    assert!(diff.token_diff > 0);

    // Restoring a non-existent id returns None
    assert!(manager.restore("nonexistent").is_none());
}

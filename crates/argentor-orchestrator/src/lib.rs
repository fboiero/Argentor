//! Multi-agent orchestration engine with task queue, monitoring, and scheduling.
//!
//! Implements the Orchestrator-Workers pattern for decomposing complex tasks
//! into sub-tasks, dispatching them to specialized worker agents, and
//! aggregating results with progress tracking and compliance hooks.
//!
//! # Main types
//!
//! - [`Orchestrator`] — Top-level engine that decomposes and executes multi-agent pipelines.
//! - [`TaskQueue`] — Priority queue for distributing tasks to worker agents.
//! - [`AgentMonitor`] — Tracks agent health, metrics, and lifecycle.
//! - [`Scheduler`] — Cron-based job scheduler for recurring tasks.
//! - [`AgentProfile`] — Configuration profile defining an agent's role and capabilities.

/// Agent version management with deploy, rollback, and A/B traffic split.
pub mod agent_versioning;
/// Token and resource budgeting per agent.
pub mod budget;
/// Agent deployment lifecycle management.
pub mod deployment;
/// Pre-configured development team orchestration with workflows and quality gates.
pub mod dev_team;
/// Orchestration engine and pipeline execution.
pub mod engine;
/// Agent handoff protocol for sequential control transfer between specialized agents.
pub mod handoff;
/// Agent health monitoring with state machine transitions.
pub mod health;
/// Inter-agent message bus for A2A communication.
pub mod message_bus;
/// Agent health and metrics monitoring.
pub mod monitor;
/// Advanced multi-agent collaboration patterns (Pipeline, Debate, Ensemble, etc.).
pub mod patterns;
/// Default agent profiles and role definitions.
pub mod profiles;
/// Agent registry with catalog management.
pub mod registry;
/// Dynamic re-planning and failure recovery strategies.
pub mod replanner;
/// Cron-based job scheduler.
pub mod scheduler;
/// Sub-agent spawning utilities.
pub mod spawner;
/// Priority task queue.
pub mod task_queue;
/// Shared orchestration types (Task, AgentProfile, Artifact, etc.).
pub mod types;
/// Configurable workflow engine for automating business pipelines.
pub mod workflow;
/// TOML-based Workflow DSL for declarative pipeline definitions.
pub mod workflow_dsl;

pub use budget::{
    default_budget, AgentUsage, AgentUsageEntry, BudgetStatus, BudgetSummary, BudgetTracker,
    TokenBudget,
};
pub use deployment::{
    DeploymentConfig, DeploymentManager, DeploymentStatus, IssueSeverity, ResourceLimits,
};
pub use dev_team::{
    DevRole, DevTeam, DevTeamConfig, DevWorkflow, QualityGate, WorkflowArtifact, WorkflowResult,
    WorkflowStatus, WorkflowStep,
};
pub use engine::{BackendFactory, Orchestrator, OrchestratorResult};
pub use health::{HealthCheckConfig, HealthChecker, HealthEvent};
pub use message_bus::{AgentMessage, BroadcastTarget, MessageBus, MessageType};
pub use monitor::AgentMonitor;
pub use patterns::{
    describe_pattern, estimate_cost, validate_pattern, AggregationStrategy, CollaborationPattern,
    PatternConfig, PatternConfigBuilder, PatternResult, PipelineStage, ReviewPolicy,
};
pub use profiles::default_profiles;
pub use registry::{default_agent_definitions, AgentRegistry};
pub use replanner::{
    FailureContext, RecoveryStrategy, RecoveryTask, ReplanEntry, ReplanHistory, Replanner,
};
pub use scheduler::{ScheduledJob, Scheduler};
pub use spawner::{SpawnRequest, SubAgentSpawner};
pub use task_queue::TaskQueue;
pub use types::{
    AgentMetrics, AgentProfile, AgentRole, AgentState, Artifact, ArtifactKind, Task, TaskStatus,
    WorkerStatus,
};
pub use workflow::{
    lead_qualification_workflow, support_ticket_workflow, FailureAction, RunStatus, StepCondition,
    StepResult, StepStatus, StepType, WorkflowDefinition, WorkflowEngine, WorkflowRun,
    WorkflowStepDef, WorkflowTrigger,
};
pub use workflow_dsl::{
    StepToml, TemplateContext, TriggerConfig, ValidationError, ValidationSeverity, WorkflowDsl,
    WorkflowMeta, WorkflowToml,
};

#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Argentor Deployment Platform Demo
//!
//! End-to-end showcase of the Argentor deployment and operations platform:
//!   - Agent registry (register, search, catalog)
//!   - Deployment manager (deploy, scale, heartbeat, task tracking)
//!   - Health checker (liveness/readiness probes, health events, summary)
//!   - Budget tracker (token budgeting per role)
//!   - Control plane state
//!
//! **No API keys needed** — everything runs in-process with internal types.
//!
//!   cargo run -p argentor-cli --example demo_deployment

use argentor_gateway::ControlPlaneState;
use argentor_orchestrator::budget::{default_budget, BudgetTracker};
use argentor_orchestrator::deployment::{DeploymentConfig, DeploymentManager, ResourceLimits};
use argentor_orchestrator::health::{HealthCheckConfig, HealthChecker, HealthEvent};
use argentor_orchestrator::registry::{default_agent_definitions, AgentRegistry};
use argentor_orchestrator::types::AgentRole;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

// ── ANSI ────────────────────────────────────────────────────────

const BOLD: &str = "\x1b[1m";
const RST: &str = "\x1b[0m";
const GRN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YLW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const BG_RED: &str = "\x1b[41m";
const WHT: &str = "\x1b[37m";

// ── Timing helper ───────────────────────────────────────────────

fn delay(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Print a progress bar of the given width (filled/total).
fn progress_bar(filled: usize, total: usize) -> String {
    let bar_width = 10;
    let fill = (filled * bar_width).checked_div(total).unwrap_or(0);
    let empty = bar_width - fill;
    format!("{}{}", "\u{2588}".repeat(fill), "\u{2591}".repeat(empty),)
}

/// Format a status tag with color.
fn status_tag(label: &str, bg: &str) -> String {
    format!("{bg}{BOLD}{WHT} {label} {RST}")
}

// ── Section header ──────────────────────────────────────────────

fn print_section(number: u32, title: &str) {
    println!();
    delay(300);
    println!("  {BOLD}{CYAN}[{number}]{RST} {BOLD}{WHT}{title}{RST}");
    println!("  {DIM}{}{RST}", "─".repeat(56));
    delay(200);
}

// ── Main ────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    // ════════════════════════════════════════════════════════════
    // BANNER
    // ════════════════════════════════════════════════════════════

    println!();
    delay(300);
    println!("{BOLD}{CYAN}  ╔════════════════════════════════════════════════════════════╗{RST}");
    println!("{BOLD}{CYAN}  ║                                                            ║{RST}");
    println!("{BOLD}{CYAN}  ║     A R G E N T O R                                        ║{RST}");
    println!("{BOLD}{CYAN}  ║     Deployment Platform Demo                                ║{RST}");
    println!("{BOLD}{CYAN}  ║                                                            ║{RST}");
    println!("{BOLD}{CYAN}  ╚════════════════════════════════════════════════════════════╝{RST}");
    println!();
    delay(500);

    // ════════════════════════════════════════════════════════════
    // 1. SETUP PHASE
    // ════════════════════════════════════════════════════════════

    print_section(1, "Setup — Initializing Platform Components");

    let _control_plane = ControlPlaneState::new();
    println!("  {GRN}[✓]{RST} ControlPlaneState initialized");
    delay(100);

    let deployment_mgr = DeploymentManager::new();
    println!("  {GRN}[✓]{RST} DeploymentManager created");
    delay(100);

    let agent_registry = AgentRegistry::new();
    println!("  {GRN}[✓]{RST} AgentRegistry created");
    delay(100);

    let health_checker = HealthChecker::new(HealthCheckConfig {
        heartbeat_interval_secs: 30,
        heartbeat_timeout_secs: 60,
        liveness_check_interval_secs: 15,
        max_consecutive_failures: 3,
        auto_restart_enabled: true,
        auto_restart_delay_secs: 5,
        max_auto_restarts: 5,
    });
    println!("  {GRN}[✓]{RST} HealthChecker configured (timeout=60s, max_failures=3)");
    delay(100);

    let budget_tracker = BudgetTracker::new();
    println!("  {GRN}[✓]{RST} BudgetTracker initialized");
    delay(100);

    println!();
    println!("  {DIM}All platform components ready.{RST}");

    // ════════════════════════════════════════════════════════════
    // 2. REGISTRY PHASE
    // ════════════════════════════════════════════════════════════

    print_section(2, "Registry — Registering Agent Definitions");

    // We register 4 of the 9 default definitions: coder, tester, reviewer, devops
    let all_defs = default_agent_definitions();
    let target_names = ["coder", "tester", "reviewer", "devops"];
    for def in &all_defs {
        if target_names.contains(&def.name.as_str()) {
            let _id = agent_registry.register(def.clone()).unwrap();
            let caps = def.capabilities.join(", ");
            println!(
                "  {GRN}[✓]{RST} Registered agent: {BOLD}{CYAN}{}{RST} ({caps})",
                def.name
            );
            delay(150);
        }
    }

    println!();
    println!(
        "  {BOLD}{WHT}Total registered:{RST} {BOLD}{CYAN}{}{RST}",
        agent_registry.count()
    );
    delay(200);

    // Search agents by capability
    println!();
    println!("  {DIM}Searching agents by capability...{RST}");
    delay(150);

    let search_results = agent_registry.search("code");
    println!(
        "  {GRN}[✓]{RST} Search \"code\": found {BOLD}{}{RST} agent(s)",
        search_results.len()
    );
    for r in &search_results {
        println!("      {DIM}  -> {}{RST}", r.name);
    }
    delay(100);

    let security_results = agent_registry.search("security");
    println!(
        "  {GRN}[✓]{RST} Search \"security\": found {BOLD}{}{RST} agent(s)",
        security_results.len()
    );
    delay(100);

    // Count by role
    let role_counts = agent_registry.count_by_role();
    println!();
    println!("  {BOLD}{WHT}Agents by role:{RST}");
    let mut sorted_roles: Vec<_> = role_counts.iter().collect();
    sorted_roles.sort_by_key(|(role, _)| format!("{role}"));
    for (role, count) in &sorted_roles {
        println!(
            "    {CYAN}{:<20}{RST} {BOLD}{count}{RST}",
            format!("{role}")
        );
    }

    // ════════════════════════════════════════════════════════════
    // 3. DEPLOYMENT PHASE
    // ════════════════════════════════════════════════════════════

    print_section(3, "Deployment — Deploying Agent Teams");

    // Set up budgets for each role
    for role in &[
        AgentRole::Coder,
        AgentRole::Tester,
        AgentRole::Reviewer,
        AgentRole::DevOps,
    ] {
        budget_tracker
            .set_budget(role.clone(), default_budget(role))
            .await;
        budget_tracker.start_tracking(role.clone()).await;
    }
    println!("  {GRN}[✓]{RST} Budget tracking initialized for all roles");
    delay(100);

    // Deploy code-team: 3 coder replicas
    let code_team_id = deployment_mgr
        .deploy(DeploymentConfig {
            agent_role: AgentRole::Coder,
            name: "code-team".to_string(),
            replicas: 3,
            auto_restart: true,
            max_restarts: 3,
            health_check_interval_secs: 30,
            shutdown_timeout_secs: 10,
            resource_limits: ResourceLimits {
                max_concurrent_tasks: 4,
                max_tokens_per_hour: 100_000,
                max_tasks_per_hour: 100,
                memory_limit_mb: Some(512),
            },
            environment: HashMap::from([("TEAM".to_string(), "backend".to_string())]),
        })
        .await?;
    println!(
        "  {GRN}[✓]{RST} Deployed {BOLD}code-team{RST}: 3 replicas {} {BOLD}{GRN}RUNNING{RST}",
        progress_bar(3, 3)
    );
    delay(200);

    // Deploy test-team: 2 tester replicas
    let test_team_id = deployment_mgr
        .deploy(DeploymentConfig {
            agent_role: AgentRole::Tester,
            name: "test-team".to_string(),
            replicas: 2,
            auto_restart: true,
            max_restarts: 3,
            health_check_interval_secs: 30,
            shutdown_timeout_secs: 10,
            resource_limits: ResourceLimits::default(),
            environment: HashMap::new(),
        })
        .await?;
    println!(
        "  {GRN}[✓]{RST} Deployed {BOLD}test-team{RST}: 2 replicas {} {BOLD}{GRN}RUNNING{RST}",
        progress_bar(2, 2)
    );
    delay(200);

    // Deploy review-team: 1 reviewer replica
    let review_team_id = deployment_mgr
        .deploy(DeploymentConfig {
            agent_role: AgentRole::Reviewer,
            name: "review-team".to_string(),
            replicas: 1,
            auto_restart: true,
            max_restarts: 5,
            health_check_interval_secs: 60,
            shutdown_timeout_secs: 15,
            resource_limits: ResourceLimits::default(),
            environment: HashMap::new(),
        })
        .await?;
    println!(
        "  {GRN}[✓]{RST} Deployed {BOLD}review-team{RST}: 1 replica  {} {BOLD}{GRN}RUNNING{RST}",
        progress_bar(1, 1)
    );
    delay(200);

    // Print deployment status table
    println!();
    println!(
        "  {BOLD}{WHT}{:<16} {:<10} {:<10} {:<12}{RST}",
        "DEPLOYMENT", "ROLE", "REPLICAS", "STATUS"
    );
    println!("  {DIM}{}{RST}", "─".repeat(50));

    let deployments = deployment_mgr.list_deployments().await;
    for dep in &deployments {
        let status_str = format!("{:?}", dep.status);
        let status_color = match &dep.status {
            argentor_orchestrator::DeploymentStatus::Running => GRN,
            argentor_orchestrator::DeploymentStatus::Degraded => YLW,
            argentor_orchestrator::DeploymentStatus::Failed => RED,
            _ => DIM,
        };
        println!(
            "  {:<16} {:<10} {:<10} {status_color}{BOLD}{:<12}{RST}",
            dep.config.name,
            format!("{}", dep.config.agent_role),
            dep.instances.len(),
            status_str,
        );
    }

    // ════════════════════════════════════════════════════════════
    // 4. OPERATIONS PHASE
    // ════════════════════════════════════════════════════════════

    print_section(4, "Operations — Heartbeats, Tasks & Scaling");

    // Record heartbeats for all instances
    let all_deployments = deployment_mgr.list_deployments().await;
    let mut heartbeat_count = 0u32;
    for dep in &all_deployments {
        for inst in &dep.instances {
            deployment_mgr
                .record_heartbeat(dep.id, inst.instance_id)
                .await?;
            heartbeat_count += 1;
        }
    }
    println!(
        "  {GRN}[✓]{RST} Recorded {BOLD}{heartbeat_count}{RST} heartbeats across all instances"
    );
    delay(150);

    // Record task completions on code-team instances
    let code_dep = deployment_mgr.get_deployment(code_team_id).await.unwrap();
    for inst in &code_dep.instances {
        // Each instance completes 2 tasks
        for _ in 0..2 {
            deployment_mgr
                .record_task_completed(code_team_id, inst.instance_id)
                .await?;
        }
    }
    println!("  {GRN}[✓]{RST} code-team: {BOLD}6{RST} tasks completed (2 per replica)");
    delay(150);

    // Record a task completion on test-team
    let test_dep = deployment_mgr.get_deployment(test_team_id).await.unwrap();
    deployment_mgr
        .record_task_completed(test_team_id, test_dep.instances[0].instance_id)
        .await?;
    println!("  {GRN}[✓]{RST} test-team: {BOLD}1{RST} task completed");
    delay(150);

    // Record a task failure on test-team instance 1
    deployment_mgr
        .record_task_failed(
            test_team_id,
            test_dep.instances[1].instance_id,
            "timeout: test suite exceeded 30s limit",
        )
        .await?;
    println!("  {YLW}[!]{RST} test-team: {BOLD}1{RST} task failed (timeout)");
    delay(150);

    // Record budget usage
    budget_tracker
        .record_tokens(&AgentRole::Coder, 12_000, 5_000)
        .await;
    budget_tracker.record_tool_call(&AgentRole::Coder).await;
    budget_tracker
        .record_tokens(&AgentRole::Tester, 8_000, 3_000)
        .await;
    budget_tracker.record_tool_call(&AgentRole::Tester).await;
    println!("  {GRN}[✓]{RST} Budget: recorded token usage for coder (17K) and tester (11K)");
    delay(200);

    // Scale code-team from 3 to 5 replicas
    let old_replicas = deployment_mgr
        .get_deployment(code_team_id)
        .await
        .unwrap()
        .instances
        .len();
    deployment_mgr.scale(code_team_id, 5).await?;
    let new_replicas = deployment_mgr
        .get_deployment(code_team_id)
        .await
        .unwrap()
        .instances
        .len();
    println!(
        "  {GRN}[✓]{RST} Scaled {BOLD}code-team{RST}: {old_replicas} \u{2192} {new_replicas} replicas"
    );
    delay(200);

    // Print updated status table
    println!();
    println!(
        "  {BOLD}{WHT}{:<16} {:<10} {:<10} {:<10} {:<10}{RST}",
        "DEPLOYMENT", "REPLICAS", "COMPLETED", "FAILED", "STATUS"
    );
    println!("  {DIM}{}{RST}", "─".repeat(58));

    let deployments = deployment_mgr.list_deployments().await;
    for dep in &deployments {
        let status_str = format!("{:?}", dep.status);
        let status_color = match &dep.status {
            argentor_orchestrator::DeploymentStatus::Running => GRN,
            argentor_orchestrator::DeploymentStatus::Degraded => YLW,
            argentor_orchestrator::DeploymentStatus::Scaling => CYAN,
            _ => DIM,
        };
        println!(
            "  {:<16} {:<10} {GRN}{:<10}{RST} {}{:<10}{RST} {status_color}{BOLD}{:<10}{RST}",
            dep.config.name,
            dep.instances.len(),
            dep.total_tasks_completed,
            if dep.total_tasks_failed > 0 { RED } else { DIM },
            dep.total_tasks_failed,
            status_str,
        );
    }

    // ════════════════════════════════════════════════════════════
    // 5. HEALTH MONITORING PHASE
    // ════════════════════════════════════════════════════════════

    print_section(5, "Health Monitoring — Probes & Recovery");

    // Register agents with the health checker
    let health_agents: Vec<(Uuid, String, AgentRole)> = vec![
        (Uuid::new_v4(), "coder-1".to_string(), AgentRole::Coder),
        (Uuid::new_v4(), "coder-2".to_string(), AgentRole::Coder),
        (Uuid::new_v4(), "coder-3".to_string(), AgentRole::Coder),
        (Uuid::new_v4(), "tester-1".to_string(), AgentRole::Tester),
        (Uuid::new_v4(), "tester-2".to_string(), AgentRole::Tester),
        (
            Uuid::new_v4(),
            "reviewer-1".to_string(),
            AgentRole::Reviewer,
        ),
    ];

    for (id, name, role) in &health_agents {
        health_checker
            .register_agent(*id, name.clone(), role.clone())
            .await;
    }
    println!(
        "  {GRN}[✓]{RST} Registered {BOLD}{}{RST} agents with HealthChecker",
        health_agents.len()
    );
    delay(150);

    // Record healthy probes for most agents (skip coder-2 for later failure sim)
    let coder2_id = health_agents[1].0;
    for (id, _name, _role) in &health_agents {
        if *id != coder2_id {
            health_checker.record_heartbeat(*id).await.unwrap();
            health_checker.record_liveness_success(*id).await.unwrap();
            health_checker.record_readiness(*id, true).await.unwrap();
        }
    }
    // coder-2 gets an initial heartbeat but then liveness failures
    health_checker.record_heartbeat(coder2_id).await.unwrap();
    println!("  {GRN}[✓]{RST} Probes passing for 5/6 agents (liveness, readiness, heartbeat)");
    delay(150);

    // Simulate multiple liveness failures on coder-2 to reach Unhealthy
    for i in 1..=3 {
        health_checker
            .record_liveness_failure(coder2_id, "connection timeout to agent process")
            .await
            .unwrap();
        println!("  {YLW}[!]{RST} Agent {BOLD}coder-2{RST}: liveness probe failed (attempt {i}/3)");
        delay(100);
    }
    delay(100);

    // Run health check and show events
    let events = health_checker.check_all().await;
    println!();
    println!("  {BOLD}{WHT}Health Events:{RST}");

    if events.is_empty() {
        println!("  {DIM}  (no events){RST}");
    } else {
        for event in &events {
            match event {
                HealthEvent::AgentBecameHealthy { agent_name, .. } => {
                    println!("  {GRN}  [HEALTHY]{RST}    {agent_name}");
                }
                HealthEvent::AgentBecameDegraded {
                    agent_name, reason, ..
                } => {
                    println!("  {YLW}  [DEGRADED]{RST}   {agent_name}: {reason}");
                }
                HealthEvent::AgentBecameUnhealthy {
                    agent_name, reason, ..
                } => {
                    println!("  {RED}  [UNHEALTHY]{RST}  {agent_name}: {reason}");
                }
                HealthEvent::AgentDied {
                    agent_name, reason, ..
                } => {
                    println!("  {RED}{BOLD}  [DEAD]{RST}       {agent_name}: {reason}");
                }
                HealthEvent::HeartbeatMissed {
                    agent_name,
                    last_seen_secs_ago,
                    ..
                } => {
                    println!(
                        "  {YLW}  [HB MISS]{RST}   {agent_name}: last seen {last_seen_secs_ago}s ago"
                    );
                }
                HealthEvent::ProbeFailure {
                    probe_name, error, ..
                } => {
                    println!("  {RED}  [PROBE]{RST}      {probe_name}: {error}");
                }
                HealthEvent::AgentRestarted {
                    agent_name,
                    restart_count,
                    ..
                } => {
                    println!("  {CYAN}  [RESTART]{RST}   {agent_name}: attempt #{restart_count}");
                }
            }
            delay(100);
        }
    }

    // Show health summary
    let summary = health_checker.get_summary().await;
    println!();
    println!("  {BOLD}{WHT}Health Summary:{RST}");
    println!("    Total agents:  {BOLD}{}{RST}", summary.total_agents);
    println!("    Healthy:       {GRN}{BOLD}{}{RST}", summary.healthy);
    if summary.degraded > 0 {
        println!("    Degraded:      {YLW}{BOLD}{}{RST}", summary.degraded);
    }
    if summary.unhealthy > 0 {
        println!("    Unhealthy:     {RED}{BOLD}{}{RST}", summary.unhealthy);
    }
    if summary.dead > 0 {
        println!("    Dead:          {RED}{BOLD}{}{RST}", summary.dead);
    }
    if summary.unknown > 0 {
        println!("    Unknown:       {DIM}{}{RST}", summary.unknown);
    }
    println!("    Restarts:      {BOLD}{}{RST}", summary.total_restarts);

    // Check if auto-restart should trigger
    if health_checker.should_restart(coder2_id).await {
        println!();
        println!("  {CYAN}[✓]{RST} Auto-restart recommended for {BOLD}coder-2{RST}");
        health_checker.record_restart(coder2_id).await.unwrap();
        health_checker
            .record_liveness_success(coder2_id)
            .await
            .unwrap();
        health_checker.record_heartbeat(coder2_id).await.unwrap();
        println!("  {GRN}[✓]{RST} coder-2 restarted and recovered");
    }

    // ════════════════════════════════════════════════════════════
    // 6. METRICS PHASE
    // ════════════════════════════════════════════════════════════

    print_section(6, "Metrics — Deployment & Budget Summary");

    let dep_summary = deployment_mgr.summary().await;
    println!("  {BOLD}{WHT}Deployment Metrics:{RST}");
    println!(
        "    Total deployments:   {BOLD}{CYAN}{}{RST}",
        dep_summary.total_deployments
    );
    println!(
        "    Total instances:     {BOLD}{CYAN}{}{RST}",
        dep_summary.total_instances
    );
    println!(
        "    Running instances:   {GRN}{BOLD}{}{RST}",
        dep_summary.running_instances
    );
    if dep_summary.unhealthy_instances > 0 {
        println!(
            "    Unhealthy instances: {RED}{BOLD}{}{RST}",
            dep_summary.unhealthy_instances
        );
    }
    println!(
        "    Tasks completed:     {GRN}{BOLD}{}{RST}",
        dep_summary.total_tasks_completed
    );
    println!(
        "    Tasks failed:        {}{BOLD}{}{RST}",
        if dep_summary.total_tasks_failed > 0 {
            RED
        } else {
            DIM
        },
        dep_summary.total_tasks_failed
    );
    delay(200);

    if !dep_summary.health_issues.is_empty() {
        println!();
        println!("  {BOLD}{WHT}Active Health Issues:{RST}");
        for issue in &dep_summary.health_issues {
            let severity_color = match &issue.severity {
                argentor_orchestrator::IssueSeverity::Warning => YLW,
                argentor_orchestrator::IssueSeverity::Critical => RED,
                argentor_orchestrator::IssueSeverity::Fatal => RED,
            };
            println!(
                "    {severity_color}[{:?}]{RST} {}",
                issue.severity, issue.description
            );
        }
    }

    // Budget summary
    println!();
    let budget_summary = budget_tracker.summary().await;
    println!("  {BOLD}{WHT}Budget Tracking:{RST}");
    println!(
        "    Total input tokens:  {BOLD}{CYAN}{}{RST}",
        budget_summary.total_input_tokens
    );
    println!(
        "    Total output tokens: {BOLD}{CYAN}{}{RST}",
        budget_summary.total_output_tokens
    );
    println!(
        "    Total tool calls:    {BOLD}{CYAN}{}{RST}",
        budget_summary.total_tool_calls
    );
    if !budget_summary.per_agent.is_empty() {
        println!();
        println!(
            "    {BOLD}{WHT}{:<16} {:<12} {:<12} {:<8}{RST}",
            "ROLE", "INPUT", "OUTPUT", "TOOLS"
        );
        println!("    {DIM}{}{RST}", "─".repeat(48));
        for entry in &budget_summary.per_agent {
            println!(
                "    {:<16} {:<12} {:<12} {:<8}",
                format!("{}", entry.role),
                entry.input_tokens,
                entry.output_tokens,
                entry.tool_calls,
            );
        }
    }

    // ════════════════════════════════════════════════════════════
    // 7. CLEANUP PHASE
    // ════════════════════════════════════════════════════════════

    print_section(7, "Cleanup — Undeploying & Final State");

    // Undeploy review-team
    deployment_mgr.undeploy(review_team_id).await?;
    println!(
        "  {GRN}[✓]{RST} Undeployed {BOLD}review-team{RST} {} {BOLD}STOPPED{RST}",
        status_tag("STOPPED", BG_RED)
    );
    delay(200);

    // Scale test-team to 0
    deployment_mgr.scale(test_team_id, 0).await?;
    println!(
        "  {GRN}[✓]{RST} Scaled {BOLD}test-team{RST}: 2 \u{2192} 0 replicas {} {BOLD}STOPPED{RST}",
        status_tag("STOPPED", BG_RED)
    );
    delay(200);

    // Final state
    println!();
    println!(
        "  {BOLD}{WHT}{:<16} {:<10} {:<12}{RST}",
        "DEPLOYMENT", "REPLICAS", "STATUS"
    );
    println!("  {DIM}{}{RST}", "─".repeat(40));

    let final_deployments = deployment_mgr.list_deployments().await;
    for dep in &final_deployments {
        let status_str = format!("{:?}", dep.status);
        let status_color = match &dep.status {
            argentor_orchestrator::DeploymentStatus::Running => GRN,
            argentor_orchestrator::DeploymentStatus::Stopped => RED,
            _ => DIM,
        };
        println!(
            "  {:<16} {:<10} {status_color}{BOLD}{:<12}{RST}",
            dep.config.name,
            dep.instances.len(),
            status_str,
        );
    }

    let final_summary = deployment_mgr.summary().await;
    println!();
    println!(
        "  {DIM}Active: {} deployment(s), {} instance(s) running{RST}",
        final_summary.total_deployments, final_summary.running_instances,
    );

    // ════════════════════════════════════════════════════════════
    // FOOTER
    // ════════════════════════════════════════════════════════════

    println!();
    delay(300);
    println!("{BOLD}{CYAN}  ╔════════════════════════════════════════════════════════════╗{RST}");
    println!("{BOLD}{CYAN}  ║  All operations executed in-process — no API keys needed   ║{RST}");
    println!("{BOLD}{CYAN}  ║  Framework: Argentor v0.1.0  |  github.com/fboiero/Argentor ║{RST}");
    println!("{BOLD}{CYAN}  ╚════════════════════════════════════════════════════════════╝{RST}");
    println!();

    Ok(())
}

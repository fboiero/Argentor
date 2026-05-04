// SPDX-License-Identifier: AGPL-3.0-only
//! Multi-agent task metrics.
//!
//! These metrics capture what single-agent benchmarks miss: the overhead and
//! correctness of agent-to-agent coordination — pipelines, debates, ensembles,
//! supervisor hierarchies, and peer-to-peer swarms.
//!
//! ## Metrics
//!
//! - `completion_rate` — fraction of expected agents that produced output
//!   (1.0 = all agents finished, 0.0 = total failure).
//! - `total_turns` — LLM calls summed across all agents.
//! - `total_tokens` — prompt tokens summed across all agents.
//! - `coordination_overhead` — tokens spent on inter-agent message framing
//!   vs productive work; heuristic: `tool_description_tokens` proxy.
//! - `wall_time_ms` — elapsed wall-clock time for the whole multi-agent run.

use crate::task::{Task, TaskResult};
use serde::{Deserialize, Serialize};

/// Full metrics for a single multi-agent task run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAgentMetrics {
    pub task_id: String,
    pub runner: String,
    /// Pattern exercised (e.g. "pipeline", "debate", "ensemble").
    pub pattern: String,
    /// Number of agents declared in the task spec.
    pub agent_count: u32,
    /// Fraction of agents that produced output (0.0 – 1.0).
    pub completion_rate: f32,
    /// Total LLM turns across all agents (approximated as llm_calls × agent_count
    /// for runners that simulate per-agent calls as a single batch).
    pub total_turns: u32,
    /// Total prompt tokens across all agents.
    pub total_tokens: u64,
    /// Tokens estimated to be spent on inter-agent coordination overhead
    /// (framing messages, routing, aggregation prompts).
    /// Sourced from `tool_description_tokens` as the closest available proxy.
    pub coordination_overhead: u64,
    /// Elapsed wall-clock milliseconds.
    pub wall_time_ms: u64,
    /// Whether the run succeeded (no hard errors).
    pub success: bool,
}

/// Compute multi-agent metrics from a `TaskResult` + `Task`.
///
/// Runners that simulate multi-agent workloads in a single batch record
/// `agent_count` in the task; `total_turns` is scaled accordingly.
pub fn compute(task: &Task, result: &TaskResult) -> MultiAgentMetrics {
    let agent_count = task.agent_count.max(1);

    // Wall time
    let wall_time_ms = (result.ended_at - result.started_at)
        .num_milliseconds()
        .max(0) as u64;

    // Scale turns by agent_count — each simulated agent takes `llm_calls`
    // turns in the mock; real orchestrators would have separate call counts.
    let total_turns = result.llm_calls.saturating_mul(agent_count);

    // Tokens: scale prompt tokens by agent_count.
    let total_tokens = result
        .prompt_tokens_sent
        .saturating_mul(agent_count as u64)
        .max(result.input_tokens.saturating_mul(agent_count as u64));

    // Coordination overhead: tool_description_tokens is the closest proxy
    // for tokens spent on routing/framing rather than task content.
    let coordination_overhead = result
        .tool_description_tokens
        .saturating_mul(agent_count as u64);

    // Completion rate: if the run succeeded, treat all agents as finished.
    // Real orchestrators would expose per-agent success flags.
    let completion_rate = if result.succeeded { 1.0_f32 } else { 0.0_f32 };

    MultiAgentMetrics {
        task_id: result.task_id.clone(),
        runner: result.runner.clone(),
        pattern: task.pattern.clone(),
        agent_count,
        completion_rate,
        total_turns,
        total_tokens,
        coordination_overhead,
        wall_time_ms,
        success: result.succeeded,
    }
}

/// Summary statistics for one runner across all multi-agent tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MultiAgentSummary {
    pub runner: String,
    pub tasks_run: usize,
    pub tasks_succeeded: usize,
    pub mean_completion_rate: f32,
    pub mean_total_turns: f32,
    pub mean_total_tokens: f64,
    pub mean_coordination_overhead: f64,
    pub mean_wall_time_ms: f64,
}

impl MultiAgentSummary {
    /// Aggregate a slice of per-task metrics for one runner.
    pub fn aggregate(runner: &str, metrics: &[MultiAgentMetrics]) -> Self {
        if metrics.is_empty() {
            return Self {
                runner: runner.to_string(),
                ..Default::default()
            };
        }
        let n = metrics.len() as f32;
        let tasks_succeeded = metrics.iter().filter(|m| m.success).count();
        let mean_completion_rate = metrics.iter().map(|m| m.completion_rate).sum::<f32>() / n;
        let mean_total_turns = metrics.iter().map(|m| m.total_turns as f32).sum::<f32>() / n;
        let mean_total_tokens =
            metrics.iter().map(|m| m.total_tokens as f64).sum::<f64>() / n as f64;
        let mean_coordination_overhead = metrics
            .iter()
            .map(|m| m.coordination_overhead as f64)
            .sum::<f64>()
            / n as f64;
        let mean_wall_time_ms =
            metrics.iter().map(|m| m.wall_time_ms as f64).sum::<f64>() / n as f64;

        Self {
            runner: runner.to_string(),
            tasks_run: metrics.len(),
            tasks_succeeded,
            mean_completion_rate,
            mean_total_turns,
            mean_total_tokens,
            mean_coordination_overhead,
            mean_wall_time_ms,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::task::{Rubric, RubricCriterion, TaskInput, TaskKind};
    use chrono::Utc;

    fn make_ma_task(agent_count: u32, pattern: &str) -> Task {
        Task {
            id: "ma_test".into(),
            name: "MA test".into(),
            description: "".into(),
            kind: TaskKind::MultiAgent,
            prompt: "test".into(),
            input: TaskInput::Inline("".into()),
            ground_truth: None,
            rubric: Rubric {
                criteria: vec![RubricCriterion {
                    name: "quality".into(),
                    description: "".into(),
                    weight: 1.0,
                }],
                pass_threshold: 5.0,
            },
            max_turns: 20,
            allowed_tools: vec![],
            expected_blocked: None,
            simulated_turns: agent_count,
            tool_count: 0,
            context_size_bytes: 0,
            required_turns: 1,
            min_tool_calls: 0,
            memory_checkpoints: None,
            agent_count,
            pattern: pattern.to_string(),
        }
    }

    fn make_result(task_id: &str, runner: &str, llm_calls: u32, tokens: u64) -> TaskResult {
        TaskResult {
            task_id: task_id.into(),
            runner: runner.into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            output: "done".into(),
            llm_calls,
            input_tokens: tokens,
            output_tokens: 50,
            tool_calls: 0,
            succeeded: true,
            error: None,
            model: "mock".into(),
            was_blocked: false,
            block_reason: None,
            prompt_tokens_sent: tokens,
            tool_description_tokens: 100,
            context_history_tokens: 0,
        }
    }

    #[test]
    fn completion_rate_success() {
        let task = make_ma_task(3, "pipeline");
        let result = make_result("ma_test", "argentor", 1, 300);
        let m = compute(&task, &result);
        assert!((m.completion_rate - 1.0).abs() < 0.01);
        assert!(m.success);
    }

    #[test]
    fn total_turns_scaled_by_agent_count() {
        let task = make_ma_task(3, "pipeline");
        let result = make_result("ma_test", "argentor", 2, 600);
        let m = compute(&task, &result);
        // 2 llm_calls × 3 agents = 6 total_turns
        assert_eq!(m.total_turns, 6);
    }

    #[test]
    fn total_tokens_scaled() {
        let task = make_ma_task(4, "ensemble");
        let result = make_result("ma_test", "argentor", 1, 500);
        let m = compute(&task, &result);
        assert_eq!(m.total_tokens, 2000);
    }

    #[test]
    fn coordination_overhead_from_tool_tokens() {
        let task = make_ma_task(2, "debate");
        let result = make_result("ma_test", "argentor", 1, 200);
        let m = compute(&task, &result);
        // 100 tool_description_tokens × 2 agents
        assert_eq!(m.coordination_overhead, 200);
    }

    #[test]
    fn pattern_preserved() {
        let task = make_ma_task(5, "swarm");
        let result = make_result("ma_test", "argentor", 2, 1000);
        let m = compute(&task, &result);
        assert_eq!(m.pattern, "swarm");
        assert_eq!(m.agent_count, 5);
    }

    #[test]
    fn summary_aggregates() {
        let m1 = MultiAgentMetrics {
            task_id: "t1".into(),
            runner: "r".into(),
            pattern: "pipeline".into(),
            agent_count: 3,
            completion_rate: 1.0,
            total_turns: 6,
            total_tokens: 900,
            coordination_overhead: 300,
            wall_time_ms: 150,
            success: true,
        };
        let m2 = MultiAgentMetrics {
            task_id: "t2".into(),
            runner: "r".into(),
            pattern: "swarm".into(),
            agent_count: 5,
            completion_rate: 0.8,
            total_turns: 10,
            total_tokens: 5000,
            coordination_overhead: 500,
            wall_time_ms: 300,
            success: false,
        };
        let s = MultiAgentSummary::aggregate("r", &[m1, m2]);
        assert_eq!(s.tasks_run, 2);
        assert_eq!(s.tasks_succeeded, 1);
        assert!((s.mean_completion_rate - 0.9).abs() < 0.01);
        assert!((s.mean_total_turns - 8.0).abs() < 0.01);
        assert!((s.mean_total_tokens - 2950.0).abs() < 0.01);
    }

    #[test]
    fn empty_metrics_returns_default_summary() {
        let s = MultiAgentSummary::aggregate("none", &[]);
        assert_eq!(s.tasks_run, 0);
        assert!((s.mean_completion_rate).abs() < 0.01);
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Long-horizon task metrics.
//!
//! These metrics capture what short-turn benchmarks miss: how a framework
//! handles multi-step, stateful, tool-chaining work where context accumulation
//! and goal coherence matter.
//!
//! ## Metrics
//!
//! - `turns_used` — actual LLM turns consumed (lower is more efficient).
//! - `tokens_accumulated` — cumulative prompt tokens across all turns (the
//!   key cost/context-window metric for long-horizon tasks).
//! - `goal_drift_score` — 0 (perfect focus) to 10 (completely off-topic).
//!   Computed heuristically: increases when the agent's output diverges from
//!   the task's `required_checkpoints` keywords.
//! - `memory_recall_rate` — fraction of `memory_checkpoints` the agent
//!   demonstrably recalled in its final output (0.0 = none, 1.0 = all).
//! - `tool_calls_used` — actual tool calls made (compared against
//!   `min_tool_calls` to detect under-use of available tools).
//! - `checkpoints_hit` — how many memory_checkpoints appeared in the output.
//! - `success` — task completed without error AND at least `pass_threshold`
//!   quality score reached.

use crate::task::{Task, TaskResult};
use serde::{Deserialize, Serialize};

/// Full metrics for a long-horizon task run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongHorizonMetrics {
    pub task_id: String,
    pub runner: String,
    /// LLM turns consumed during the run.
    pub turns_used: u32,
    /// Cumulative prompt tokens sent to the LLM across all turns.
    pub tokens_accumulated: u64,
    /// Goal drift score (0 = on-task, 10 = completely off-task).
    /// Heuristic: computed from keyword coverage of memory_checkpoints.
    pub goal_drift_score: f32,
    /// Memory recall rate (0.0 – 1.0): fraction of checkpoints recalled
    /// in the final output.
    pub memory_recall_rate: f32,
    /// Tool calls made during the run.
    pub tool_calls_used: u32,
    /// Number of memory_checkpoints found in the output text.
    pub checkpoints_hit: u32,
    /// Whether the run succeeded (no error + quality threshold met).
    pub success: bool,
    /// Tokens at turn 10 — a canonical cross-framework comparison point.
    /// If turns_used < 10, this is the total tokens at the last turn.
    pub tokens_at_turn_10: u64,
    /// Percentage of tokens saved vs naive linear growth (baseline formula:
    /// tokens = scaffold_per_turn * T + history * T*(T-1)/2).
    /// Positive means the framework spent fewer tokens than the naive baseline.
    pub compaction_savings_pct: f32,
}

/// Compute long-horizon metrics from a `TaskResult` + `Task`.
///
/// `task.simulated_turns` is used as the reference for tokens_at_turn_10
/// scaling. `task.memory_checkpoints` drives the recall score.
pub fn compute(task: &Task, result: &TaskResult) -> LongHorizonMetrics {
    let turns_used = result.llm_calls;
    let tokens_accumulated = result.prompt_tokens_sent;

    // Memory recall: scan output text for each checkpoint keyword.
    let checkpoints = task.memory_checkpoints.as_deref().unwrap_or(&[]);
    let output_lower = result.output.to_lowercase();
    let checkpoints_hit = checkpoints
        .iter()
        .filter(|cp| {
            // Normalise checkpoint key (underscores → spaces, lowercase)
            let keyword = cp.replace('_', " ").to_lowercase();
            output_lower.contains(&keyword)
        })
        .count() as u32;

    let memory_recall_rate = if checkpoints.is_empty() {
        // No checkpoints defined → treat as full recall (not applicable).
        1.0
    } else {
        checkpoints_hit as f32 / checkpoints.len() as f32
    };

    // Goal drift: 0 = on-task (high recall), 10 = off-task (zero recall).
    // Drift is the inverse of recall scaled to [0, 10].
    let goal_drift_score = (1.0 - memory_recall_rate) * 10.0;

    // Tokens at turn 10: extrapolate if run was shorter than 10 turns, or
    // use the actual accumulated total if >= 10 turns were run.
    let tokens_at_turn_10 = if turns_used >= 10 {
        tokens_accumulated
    } else if turns_used == 0 {
        0
    } else {
        // Linear extrapolation: per-turn average × 10.
        let per_turn = tokens_accumulated / turns_used as u64;
        per_turn * 10
    };

    // Naive baseline: scaffold(50 tok/turn) × T + pair(50+50=100) × T*(T-1)/2
    // Use simulated_turns as T for the baseline.
    let t = task.simulated_turns.max(1) as u64;
    let naive_tokens: u64 = 50 * t + 100 * t * t.saturating_sub(1) / 2;
    let compaction_savings_pct = if naive_tokens == 0 {
        0.0
    } else {
        let savings = naive_tokens as i64 - tokens_accumulated as i64;
        (savings as f32 / naive_tokens as f32) * 100.0
    };

    LongHorizonMetrics {
        task_id: result.task_id.clone(),
        runner: result.runner.clone(),
        turns_used,
        tokens_accumulated,
        goal_drift_score,
        memory_recall_rate,
        tool_calls_used: result.tool_calls,
        checkpoints_hit,
        success: result.succeeded,
        tokens_at_turn_10,
        compaction_savings_pct,
    }
}

/// Summary statistics for a single runner across all long-horizon tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LongHorizonSummary {
    pub runner: String,
    pub tasks_run: usize,
    pub tasks_succeeded: usize,
    pub mean_turns: f32,
    pub mean_tokens_at_turn_10: f64,
    pub mean_memory_recall_rate: f32,
    pub mean_goal_drift_score: f32,
    pub mean_compaction_savings_pct: f32,
}

impl LongHorizonSummary {
    /// Aggregate a slice of per-task metrics for one runner.
    pub fn aggregate(runner: &str, metrics: &[LongHorizonMetrics]) -> Self {
        if metrics.is_empty() {
            return Self {
                runner: runner.to_string(),
                ..Default::default()
            };
        }
        let n = metrics.len() as f32;
        let tasks_succeeded = metrics.iter().filter(|m| m.success).count();
        let mean_turns = metrics.iter().map(|m| m.turns_used as f32).sum::<f32>() / n;
        let mean_tokens_at_turn_10 = metrics
            .iter()
            .map(|m| m.tokens_at_turn_10 as f64)
            .sum::<f64>()
            / n as f64;
        let mean_memory_recall_rate = metrics.iter().map(|m| m.memory_recall_rate).sum::<f32>() / n;
        let mean_goal_drift_score = metrics.iter().map(|m| m.goal_drift_score).sum::<f32>() / n;
        let mean_compaction_savings_pct = metrics
            .iter()
            .map(|m| m.compaction_savings_pct)
            .sum::<f32>()
            / n;

        Self {
            runner: runner.to_string(),
            tasks_run: metrics.len(),
            tasks_succeeded,
            mean_turns,
            mean_tokens_at_turn_10,
            mean_memory_recall_rate,
            mean_goal_drift_score,
            mean_compaction_savings_pct,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::task::{Rubric, RubricCriterion, TaskInput, TaskKind};
    use chrono::Utc;

    fn make_task(simulated_turns: u32, checkpoints: Vec<String>) -> Task {
        Task {
            id: "lh_test".into(),
            name: "LH test".into(),
            description: "".into(),
            kind: TaskKind::LongHorizon,
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
            max_turns: 15,
            allowed_tools: vec![],
            expected_blocked: None,
            simulated_turns,
            tool_count: 0,
            context_size_bytes: 0,
            required_turns: simulated_turns,
            min_tool_calls: 0,
            memory_checkpoints: Some(checkpoints),
            agent_count: 1,
            pattern: String::new(),
        }
    }

    fn make_result(
        task_id: &str,
        runner: &str,
        llm_calls: u32,
        prompt_tokens_sent: u64,
        output: &str,
    ) -> TaskResult {
        TaskResult {
            task_id: task_id.into(),
            runner: runner.into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            output: output.into(),
            llm_calls,
            input_tokens: prompt_tokens_sent,
            output_tokens: 50 * llm_calls as u64,
            tool_calls: 0,
            succeeded: true,
            error: None,
            model: "mock".into(),
            was_blocked: false,
            block_reason: None,
            prompt_tokens_sent,
            tool_description_tokens: 0,
            context_history_tokens: 0,
        }
    }

    #[test]
    fn full_recall_zero_drift() {
        let task = make_task(5, vec!["fix applied".into(), "tests passing".into()]);
        let result = make_result(
            "lh_test",
            "argentor",
            5,
            1000,
            "The fix applied and tests passing successfully.",
        );
        let m = compute(&task, &result);
        assert_eq!(m.checkpoints_hit, 2);
        assert!((m.memory_recall_rate - 1.0).abs() < 0.01);
        assert!(m.goal_drift_score < 0.01);
    }

    #[test]
    fn partial_recall() {
        let task = make_task(5, vec!["fix applied".into(), "tests passing".into()]);
        let result = make_result(
            "lh_test",
            "argentor",
            5,
            1000,
            "The fix applied but tests not mentioned.",
        );
        let m = compute(&task, &result);
        assert_eq!(m.checkpoints_hit, 1);
        assert!((m.memory_recall_rate - 0.5).abs() < 0.01);
        assert!((m.goal_drift_score - 5.0).abs() < 0.01);
    }

    #[test]
    fn zero_recall_max_drift() {
        let task = make_task(5, vec!["specific thing".into(), "other specific".into()]);
        let result = make_result("lh_test", "argentor", 5, 1000, "I did nothing relevant.");
        let m = compute(&task, &result);
        assert_eq!(m.checkpoints_hit, 0);
        assert!(m.memory_recall_rate < 0.01);
        assert!((m.goal_drift_score - 10.0).abs() < 0.01);
    }

    #[test]
    fn tokens_at_turn_10_extrapolation() {
        let task = make_task(5, vec![]);
        let result = make_result("lh_test", "argentor", 5, 500, "done");
        let m = compute(&task, &result);
        // 500 tokens / 5 turns = 100/turn × 10 = 1000
        assert_eq!(m.tokens_at_turn_10, 1000);
    }

    #[test]
    fn tokens_at_turn_10_exact() {
        let task = make_task(10, vec![]);
        let result = make_result("lh_test", "argentor", 10, 5000, "done");
        let m = compute(&task, &result);
        assert_eq!(m.tokens_at_turn_10, 5000);
    }

    #[test]
    fn summary_aggregates_correctly() {
        let m1 = LongHorizonMetrics {
            task_id: "t1".into(),
            runner: "r".into(),
            turns_used: 5,
            tokens_accumulated: 1000,
            goal_drift_score: 2.0,
            memory_recall_rate: 0.8,
            tool_calls_used: 3,
            checkpoints_hit: 4,
            success: true,
            tokens_at_turn_10: 2000,
            compaction_savings_pct: 10.0,
        };
        let m2 = LongHorizonMetrics {
            task_id: "t2".into(),
            runner: "r".into(),
            turns_used: 7,
            tokens_accumulated: 3000,
            goal_drift_score: 4.0,
            memory_recall_rate: 0.6,
            tool_calls_used: 5,
            checkpoints_hit: 3,
            success: false,
            tokens_at_turn_10: 4000,
            compaction_savings_pct: 5.0,
        };
        let s = LongHorizonSummary::aggregate("r", &[m1, m2]);
        assert_eq!(s.tasks_run, 2);
        assert_eq!(s.tasks_succeeded, 1);
        assert!((s.mean_turns - 6.0).abs() < 0.01);
        assert!((s.mean_tokens_at_turn_10 - 3000.0).abs() < 0.01);
    }

    #[test]
    fn empty_checkpoints_full_recall() {
        let task = make_task(5, vec![]);
        let result = make_result("lh_test", "argentor", 5, 1000, "anything");
        let m = compute(&task, &result);
        assert!((m.memory_recall_rate - 1.0).abs() < 0.01);
    }

    #[test]
    fn no_cross_contamination_in_summary() {
        // Two runners, each with 1 metric — summaries must not mix.
        let m_a = LongHorizonMetrics {
            task_id: "t".into(),
            runner: "a".into(),
            turns_used: 3,
            tokens_accumulated: 300,
            goal_drift_score: 1.0,
            memory_recall_rate: 0.9,
            tool_calls_used: 2,
            checkpoints_hit: 5,
            success: true,
            tokens_at_turn_10: 1000,
            compaction_savings_pct: 20.0,
        };
        let m_b = LongHorizonMetrics {
            task_id: "t".into(),
            runner: "b".into(),
            turns_used: 10,
            tokens_accumulated: 10000,
            goal_drift_score: 8.0,
            memory_recall_rate: 0.2,
            tool_calls_used: 1,
            checkpoints_hit: 1,
            success: false,
            tokens_at_turn_10: 10000,
            compaction_savings_pct: -50.0,
        };
        let sa = LongHorizonSummary::aggregate("a", &[m_a]);
        let sb = LongHorizonSummary::aggregate("b", &[m_b]);
        assert!((sa.mean_tokens_at_turn_10 - 1000.0).abs() < 0.01);
        assert!((sb.mean_tokens_at_turn_10 - 10000.0).abs() < 0.01);
    }
}

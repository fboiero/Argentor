//! Metrics computed from a [`TaskResult`].
//!
//! - Cost (USD) based on model pricing
//! - Latency (wall clock)
//! - Quality (LLM-as-judge, stubbed for now)

pub mod block_rate;
pub mod compliance;
pub mod cost;
pub mod dx;
pub mod integrations;
pub mod long_horizon;
pub mod multi_agent;
pub mod quality;
pub mod siem;
pub mod stats;

use crate::task::{Task, TaskResult};
use serde::{Deserialize, Serialize};

pub use block_rate::{compute_block_rate, BlockRateMetric};
pub use compliance::{ComplianceMetric, ComplianceSummary};
pub use cost::CostMetric;
pub use dx::{compute_all as compute_dx_scores, DxMetric, ErrorScenarioScore};
pub use integrations::{IntegrationMetric, IntegrationSummary};
pub use long_horizon::{LongHorizonMetrics, LongHorizonSummary};
pub use multi_agent::{MultiAgentMetrics, MultiAgentSummary};
pub use quality::QualityMetric;
pub use siem::{SiemMetric, SiemSummary};
pub use stats::{PairedTTest, Stats};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyMetric {
    pub wall_ms: u64,
    pub per_turn_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMetrics {
    pub task_id: String,
    pub runner: String,
    pub succeeded: bool,
    pub latency: LatencyMetric,
    pub cost: CostMetric,
    pub quality: QualityMetric,
    pub passed_rubric: bool,
}

/// Compute all metrics from a single result.
pub fn compute(task: &Task, result: &TaskResult) -> TaskMetrics {
    let wall_ms = (result.ended_at - result.started_at)
        .num_milliseconds()
        .max(0) as u64;
    let per_turn_ms = if result.llm_calls > 0 {
        wall_ms / result.llm_calls as u64
    } else {
        wall_ms
    };

    let cost = cost::compute(&result.model, result.input_tokens, result.output_tokens);
    let quality = quality::compute_heuristic(task, result);
    let passed_rubric = quality.aggregate_score >= task.rubric.pass_threshold;

    TaskMetrics {
        task_id: result.task_id.clone(),
        runner: result.runner.clone(),
        succeeded: result.succeeded,
        latency: LatencyMetric {
            wall_ms,
            per_turn_ms,
        },
        cost,
        quality,
        passed_rubric,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::task::{Rubric, RubricCriterion, TaskInput, TaskKind};
    use chrono::Utc;

    fn sample_task() -> Task {
        Task {
            id: "t".into(),
            name: "T".into(),
            description: "".into(),
            kind: TaskKind::Qa,
            prompt: "p".into(),
            input: TaskInput::Inline("".into()),
            ground_truth: Some("answer".into()),
            rubric: Rubric {
                criteria: vec![RubricCriterion {
                    name: "correct".into(),
                    description: "".into(),
                    weight: 1.0,
                }],
                pass_threshold: 5.0,
            },
            max_turns: 1,
            allowed_tools: vec![],
            expected_blocked: None,
            simulated_turns: 1,
            tool_count: 0,
            context_size_bytes: 0,
            required_turns: 1,
            min_tool_calls: 0,
            memory_checkpoints: None,
            agent_count: 1,
            pattern: String::new(),
        }
    }

    #[test]
    fn compute_produces_metrics() {
        let task = sample_task();
        let result = TaskResult {
            task_id: "t".into(),
            runner: "mock".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            output: "answer".into(),
            llm_calls: 1,
            input_tokens: 100,
            output_tokens: 50,
            tool_calls: 0,
            succeeded: true,
            error: None,
            model: "mock".into(),
            was_blocked: false,
            block_reason: None,
            prompt_tokens_sent: 0,
            tool_description_tokens: 0,
            context_history_tokens: 0,
        };
        let m = compute(&task, &result);
        assert!(m.succeeded);
        assert_eq!(m.task_id, "t");
    }
}

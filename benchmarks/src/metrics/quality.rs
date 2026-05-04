//! Quality scoring. v1 uses heuristic matching against ground truth.
//! v2 will use LLM-as-judge with the rubric.

use crate::task::{Task, TaskResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetric {
    /// Aggregate score 0.0-10.0 (mean of per-criterion scores).
    pub aggregate_score: f32,
    /// Per-criterion scores, by name.
    pub per_criterion: Vec<(String, f32)>,
    /// Whether the output non-trivially references the ground truth.
    pub ground_truth_overlap: Option<f32>,
    /// Method used to compute the score.
    pub method: QualityMethod,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityMethod {
    /// Keyword overlap with ground truth, basic.
    Heuristic,
    /// LLM-as-judge (not yet implemented).
    LlmJudge,
}

/// v1 heuristic: measure word overlap between output and ground truth.
/// Returns 0.0-10.0 (no overlap → 0, complete overlap → 10).
pub fn compute_heuristic(task: &Task, result: &TaskResult) -> QualityMetric {
    let mut per_criterion = Vec::with_capacity(task.rubric.criteria.len());

    let overlap = task
        .ground_truth
        .as_ref()
        .map(|gt| word_overlap(gt, &result.output));

    // Distribute the overlap score across criteria uniformly (heuristic limitation).
    let base_score = overlap.unwrap_or(5.0); // 5.0 default if no ground truth

    for c in &task.rubric.criteria {
        per_criterion.push((c.name.clone(), base_score));
    }

    let total_weight: f32 = task.rubric.criteria.iter().map(|c| c.weight).sum();
    let weighted_sum: f32 = task
        .rubric
        .criteria
        .iter()
        .zip(per_criterion.iter())
        .map(|(c, (_, s))| c.weight * s)
        .sum();
    let aggregate_score = if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        base_score
    };

    QualityMetric {
        aggregate_score,
        per_criterion,
        ground_truth_overlap: overlap,
        method: QualityMethod::Heuristic,
    }
}

/// Returns word overlap score 0.0-10.0. Case-insensitive, tokenized by whitespace.
fn word_overlap(a: &str, b: &str) -> f32 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_words: std::collections::HashSet<&str> = a_lower
        .split_whitespace()
        .filter(|w| w.len() > 2) // skip stop words heuristically
        .collect();
    let b_words: std::collections::HashSet<&str> =
        b_lower.split_whitespace().filter(|w| w.len() > 2).collect();
    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }
    let intersection = a_words.intersection(&b_words).count();
    let union = (a_words.len() + b_words.len() - intersection).max(1);
    let jaccard = intersection as f32 / union as f32;
    (jaccard * 10.0).clamp(0.0, 10.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::task::{Rubric, RubricCriterion, TaskInput, TaskKind};
    use chrono::Utc;

    fn task_with_gt(gt: &str) -> Task {
        Task {
            id: "t".into(),
            name: "T".into(),
            description: "".into(),
            kind: TaskKind::Qa,
            prompt: "".into(),
            input: TaskInput::Inline("".into()),
            ground_truth: Some(gt.into()),
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

    fn result_with_output(out: &str) -> TaskResult {
        TaskResult {
            task_id: "t".into(),
            runner: "mock".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            output: out.into(),
            llm_calls: 1,
            input_tokens: 10,
            output_tokens: 10,
            tool_calls: 0,
            succeeded: true,
            error: None,
            model: "mock".into(),
            was_blocked: false,
            block_reason: None,
            prompt_tokens_sent: 0,
            tool_description_tokens: 0,
            context_history_tokens: 0,
        }
    }

    #[test]
    fn exact_match_high_score() {
        let task = task_with_gt("the quick brown fox jumps");
        let result = result_with_output("the quick brown fox jumps");
        let q = compute_heuristic(&task, &result);
        assert!(q.aggregate_score > 7.0);
    }

    #[test]
    fn no_overlap_zero() {
        let task = task_with_gt("animals cats dogs birds");
        let result = result_with_output("programming rust systems code");
        let q = compute_heuristic(&task, &result);
        assert_eq!(q.aggregate_score, 0.0);
    }

    #[test]
    fn no_ground_truth_default_score() {
        let mut task = task_with_gt("");
        task.ground_truth = None;
        let result = result_with_output("anything");
        let q = compute_heuristic(&task, &result);
        assert_eq!(q.aggregate_score, 5.0);
    }

    #[test]
    fn overlap_score_between_bounds() {
        let task = task_with_gt("apple banana cherry");
        let result = result_with_output("apple mango orange");
        let q = compute_heuristic(&task, &result);
        assert!(q.aggregate_score > 0.0 && q.aggregate_score < 10.0);
    }
}

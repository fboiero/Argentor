//! Process Reward Scoring for agent reasoning trajectories.
//!
//! Inspired by Process Reward Models (PRM) research, this module scores EACH
//! STEP of an agent's reasoning trajectory rather than only the final output.
//! This enables early detection of off-track reasoning, redundant steps, and
//! safety concerns.
//!
//! # Key types
//!
//! - [`ProcessRewardModel`] — the scoring engine.
//! - [`StepReward`] — score and feedback for a single reasoning step.
//! - [`ProcessRewardResult`] — aggregate result across the full trajectory.
//! - [`StepCategory`] — classification of what a step does.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the process reward model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardConfig {
    /// Whether process reward scoring is enabled.
    pub enabled: bool,
    /// Weight applied to each step category when scoring.
    pub step_weights: HashMap<StepCategory, f32>,
    /// Steps scoring below this threshold are flagged (0.0–1.0).
    pub min_step_score: f32,
    /// How to aggregate step scores into a single trajectory score.
    pub aggregate_method: AggregateMethod,
}

impl Default for RewardConfig {
    fn default() -> Self {
        let mut weights = HashMap::new();
        weights.insert(StepCategory::Reasoning, 1.0);
        weights.insert(StepCategory::ToolSelection, 1.0);
        weights.insert(StepCategory::ToolUsage, 1.0);
        weights.insert(StepCategory::InformationGain, 0.8);
        weights.insert(StepCategory::Coherence, 0.9);
        weights.insert(StepCategory::Efficiency, 0.7);
        weights.insert(StepCategory::Safety, 1.5);

        Self {
            enabled: true,
            step_weights: weights,
            min_step_score: 0.3,
            aggregate_method: AggregateMethod::WeightedMean,
        }
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Method for aggregating individual step scores into a trajectory score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AggregateMethod {
    /// Simple arithmetic mean.
    Mean,
    /// Weighted mean using category weights.
    WeightedMean,
    /// The worst step determines the overall score.
    Min,
    /// Multiply all scores — any bad step heavily penalizes the total.
    Product,
}

/// Classification of what a reasoning step does.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepCategory {
    /// Quality of thinking or analysis.
    Reasoning,
    /// Whether the right tool was chosen.
    ToolSelection,
    /// Whether the tool was used correctly.
    ToolUsage,
    /// Whether the step added useful new information.
    InformationGain,
    /// Whether the step is consistent with previous steps.
    Coherence,
    /// Whether the step was necessary.
    Efficiency,
    /// Whether the step avoids harmful actions.
    Safety,
}

/// Flags highlighting notable aspects of a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RewardFlag {
    /// Step was unnecessary or repeated work already done.
    Redundant,
    /// Step has safety or security concerns.
    Risky,
    /// A better, faster approach was available.
    Inefficient,
    /// Step doesn't contribute to the stated goal.
    OffTopic,
    /// Exceptionally well-executed step.
    Excellent,
}

/// Overall trajectory quality classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrajectoryQuality {
    /// All steps are high quality.
    Optimal,
    /// Mostly good, minor issues.
    Good,
    /// Some unnecessary or low-quality steps.
    Suboptimal,
    /// Many issues, should revise the approach.
    Poor,
    /// Contains risky or harmful steps.
    Harmful,
}

// ---------------------------------------------------------------------------
// Step and result types
// ---------------------------------------------------------------------------

/// Score and feedback for a single reasoning step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReward {
    /// Zero-based index of this step in the trajectory.
    pub step_index: usize,
    /// Human-readable description of what happened at this step.
    pub step_description: String,
    /// Category classification of this step.
    pub category: StepCategory,
    /// Quality score (0.0–1.0).
    pub score: f32,
    /// Textual feedback explaining the score.
    pub feedback: String,
    /// Notable flags for this step.
    pub flags: Vec<RewardFlag>,
}

/// Aggregate scoring result for an entire reasoning trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRewardResult {
    /// Per-step scores and feedback.
    pub steps: Vec<StepReward>,
    /// Aggregate score across all steps (0.0–1.0).
    pub aggregate_score: f32,
    /// Indices of steps that scored below `min_step_score`.
    pub flagged_steps: Vec<usize>,
    /// Improvement suggestions based on the scoring.
    pub recommendations: Vec<String>,
    /// Overall trajectory quality classification.
    pub trajectory_quality: TrajectoryQuality,
}

// ---------------------------------------------------------------------------
// Heuristic keyword sets (used for scoring steps)
// ---------------------------------------------------------------------------

/// Keywords indicating strong reasoning.
const REASONING_POSITIVE: &[&str] = &[
    "because",
    "therefore",
    "analysis",
    "consider",
    "evaluate",
    "compare",
    "conclude",
    "evidence",
    "hypothesis",
    "deduce",
    "infer",
    "reason",
    "logic",
    "think",
    "assess",
    "weigh",
];

/// Keywords indicating weak or absent reasoning.
const REASONING_NEGATIVE: &[&str] = &[
    "guess", "maybe", "random", "idk", "whatever", "just try", "not sure", "no idea",
];

/// Keywords indicating good tool selection.
const TOOL_SELECTION_POSITIVE: &[&str] = &[
    "appropriate",
    "best fit",
    "selected",
    "chose",
    "optimal",
    "right tool",
    "suitable",
    "relevant",
];

/// Keywords indicating poor tool selection.
const TOOL_SELECTION_NEGATIVE: &[&str] = &[
    "wrong tool",
    "incorrect",
    "mismatched",
    "inappropriate",
    "should have used",
    "bad choice",
];

/// Keywords indicating information gain.
const INFO_GAIN_POSITIVE: &[&str] = &[
    "found",
    "discovered",
    "learned",
    "revealed",
    "new information",
    "insight",
    "result",
    "data",
    "output",
    "response",
];

/// Keywords indicating redundancy.
const REDUNDANCY_KEYWORDS: &[&str] = &[
    "again",
    "repeat",
    "already",
    "same",
    "duplicate",
    "redo",
    "re-run",
    "retry same",
];

/// Keywords indicating safety concerns.
const SAFETY_NEGATIVE: &[&str] = &[
    "delete",
    "drop",
    "destroy",
    "rm -rf",
    "format",
    "wipe",
    "sudo",
    "root",
    "admin",
    "password",
    "secret",
    "credential",
    "force",
    "override",
    "bypass",
    "hack",
    "exploit",
];

/// Keywords indicating safe practices.
const SAFETY_POSITIVE: &[&str] = &[
    "validate",
    "check",
    "verify",
    "sandbox",
    "safe",
    "backup",
    "confirm",
    "permission",
    "authorize",
    "audit",
    "log",
];

/// Keywords indicating efficiency.
const EFFICIENCY_POSITIVE: &[&str] = &[
    "direct",
    "efficient",
    "optimal",
    "fast",
    "minimal",
    "concise",
    "streamlined",
];

/// Keywords indicating inefficiency.
const EFFICIENCY_NEGATIVE: &[&str] = &[
    "workaround",
    "roundabout",
    "lengthy",
    "verbose",
    "unnecessary",
    "brute force",
    "slow",
    "wasteful",
];

// ---------------------------------------------------------------------------
// ProcessRewardModel
// ---------------------------------------------------------------------------

/// Engine that scores each step of an agent's reasoning trajectory using
/// heuristic analysis (keyword patterns, step sequencing, and category weights).
pub struct ProcessRewardModel {
    config: RewardConfig,
}

impl ProcessRewardModel {
    /// Create a new process reward model with the given configuration.
    pub fn new(config: RewardConfig) -> Self {
        Self { config }
    }

    /// Create a model with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RewardConfig::default())
    }

    /// Score an entire reasoning trajectory.
    ///
    /// Each step is a `(description, category)` pair. The model scores every
    /// step using heuristic keyword analysis and step-sequence patterns, then
    /// aggregates the scores according to the configured method.
    pub fn score_trajectory(&self, steps: &[(String, StepCategory)]) -> ProcessRewardResult {
        if steps.is_empty() {
            return ProcessRewardResult {
                steps: vec![],
                aggregate_score: 0.0,
                flagged_steps: vec![],
                recommendations: vec!["No steps to evaluate.".into()],
                trajectory_quality: TrajectoryQuality::Poor,
            };
        }

        let mut step_rewards: Vec<StepReward> = Vec::with_capacity(steps.len());
        let descriptions: Vec<&str> = steps.iter().map(|(d, _)| d.as_str()).collect();

        for (i, (description, category)) in steps.iter().enumerate() {
            let (base_score, mut flags, feedback) =
                self.score_step(description, category, i, &descriptions);

            // Sequence-level analysis: detect redundancy.
            if i > 0 {
                let prev = &descriptions[i - 1];
                if self.is_redundant(description, prev) {
                    flags.push(RewardFlag::Redundant);
                }
            }

            // Apply coherence check for consecutive steps.
            let coherence_penalty = if i > 0 {
                let prev_cat = &steps[i - 1].1;
                self.coherence_penalty(prev_cat, category)
            } else {
                0.0
            };

            let final_score = (base_score - coherence_penalty).clamp(0.0, 1.0);

            if final_score > 0.85 && flags.is_empty() {
                flags.push(RewardFlag::Excellent);
            }

            step_rewards.push(StepReward {
                step_index: i,
                step_description: description.clone(),
                category: category.clone(),
                score: final_score,
                feedback,
                flags,
            });
        }

        let flagged_steps: Vec<usize> = step_rewards
            .iter()
            .filter(|s| s.score < self.config.min_step_score)
            .map(|s| s.step_index)
            .collect();

        let aggregate_score = self.aggregate_scores(&step_rewards);
        let trajectory_quality = self.classify_trajectory(aggregate_score, &step_rewards);
        let recommendations = self.generate_recommendations(&step_rewards, &flagged_steps);

        ProcessRewardResult {
            steps: step_rewards,
            aggregate_score,
            flagged_steps,
            recommendations,
            trajectory_quality,
        }
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &RewardConfig {
        &self.config
    }

    // -----------------------------------------------------------------------
    // Private scoring helpers
    // -----------------------------------------------------------------------

    /// Score a single step using keyword heuristics.
    fn score_step(
        &self,
        description: &str,
        category: &StepCategory,
        _step_index: usize,
        _all_descriptions: &[&str],
    ) -> (f32, Vec<RewardFlag>, String) {
        let lower = description.to_lowercase();
        let mut flags = Vec::new();

        let (positive_hits, negative_hits, feedback) = match category {
            StepCategory::Reasoning => {
                let pos = count_keyword_hits(&lower, REASONING_POSITIVE);
                let neg = count_keyword_hits(&lower, REASONING_NEGATIVE);
                let fb = if pos > neg {
                    "Good reasoning with clear analytical thinking.".into()
                } else if neg > 0 {
                    "Reasoning is uncertain or lacks analytical depth.".into()
                } else {
                    "Reasoning step with neutral indicators.".into()
                };
                (pos, neg, fb)
            }

            StepCategory::ToolSelection => {
                let pos = count_keyword_hits(&lower, TOOL_SELECTION_POSITIVE);
                let neg = count_keyword_hits(&lower, TOOL_SELECTION_NEGATIVE);
                let fb = if pos > neg {
                    "Tool selection appears well-considered.".into()
                } else if neg > 0 {
                    "Tool selection may be suboptimal.".into()
                } else {
                    "Tool selection step without strong indicators.".into()
                };
                (pos, neg, fb)
            }

            StepCategory::ToolUsage => {
                let pos = count_keyword_hits(&lower, INFO_GAIN_POSITIVE);
                let neg = count_keyword_hits(&lower, REASONING_NEGATIVE);
                let fb = if pos > 0 {
                    "Tool was used effectively with meaningful output.".into()
                } else {
                    "Tool usage step with limited observable output.".into()
                };
                (pos, neg, fb)
            }

            StepCategory::InformationGain => {
                let pos = count_keyword_hits(&lower, INFO_GAIN_POSITIVE);
                let neg = count_keyword_hits(&lower, REDUNDANCY_KEYWORDS);
                if neg > 0 {
                    flags.push(RewardFlag::Redundant);
                }
                let fb = if pos > neg {
                    "Step provides valuable new information.".into()
                } else if neg > 0 {
                    "Step may be redundant — similar work already done.".into()
                } else {
                    "Information gain is unclear.".into()
                };
                (pos, neg, fb)
            }

            StepCategory::Coherence => {
                // Coherence is mostly assessed at the sequence level.
                let pos = count_keyword_hits(&lower, REASONING_POSITIVE);
                let neg = count_keyword_hits(&lower, REASONING_NEGATIVE);
                let fb = "Coherence assessed relative to adjacent steps.".into();
                (pos, neg, fb)
            }

            StepCategory::Efficiency => {
                let pos = count_keyword_hits(&lower, EFFICIENCY_POSITIVE);
                let neg = count_keyword_hits(&lower, EFFICIENCY_NEGATIVE);
                if neg > 0 {
                    flags.push(RewardFlag::Inefficient);
                }
                let fb = if pos > neg {
                    "Step is efficiently executed.".into()
                } else if neg > 0 {
                    "Step could be more efficient.".into()
                } else {
                    "Efficiency is neutral.".into()
                };
                (pos, neg, fb)
            }

            StepCategory::Safety => {
                let pos = count_keyword_hits(&lower, SAFETY_POSITIVE);
                let neg = count_keyword_hits(&lower, SAFETY_NEGATIVE);
                if neg > 0 {
                    flags.push(RewardFlag::Risky);
                }
                let fb = if neg > 0 {
                    "Step contains potentially unsafe operations.".into()
                } else if pos > 0 {
                    "Step demonstrates safe practices.".into()
                } else {
                    "No safety concerns detected.".into()
                };
                (pos, neg, fb)
            }
        };

        // Compute base score from keyword balance.
        let total = (positive_hits + negative_hits).max(1) as f32;
        let keyword_score = if negative_hits > positive_hits {
            0.3 + 0.2 * (positive_hits as f32 / total)
        } else if positive_hits > 0 {
            0.6 + 0.4 * (positive_hits as f32 / total)
        } else {
            0.5 // neutral — no keywords matched
        };

        // Length heuristic: very short descriptions are suspicious.
        let length_factor = if description.len() < 10 {
            0.8
        } else if description.len() > 500 {
            0.9 // overly verbose
        } else {
            1.0
        };

        let score = (keyword_score * length_factor).clamp(0.0, 1.0);

        (score, flags, feedback)
    }

    /// Check if a step is redundant relative to the previous step.
    fn is_redundant(&self, current: &str, previous: &str) -> bool {
        let current_lower = current.to_lowercase();
        let prev_lower = previous.to_lowercase();

        // Exact or near-exact match.
        if current_lower == prev_lower {
            return true;
        }

        // High word overlap (Jaccard similarity > 0.8).
        let current_words: std::collections::HashSet<&str> =
            current_lower.split_whitespace().collect();
        let prev_words: std::collections::HashSet<&str> = prev_lower.split_whitespace().collect();

        if current_words.is_empty() || prev_words.is_empty() {
            return false;
        }

        let intersection = current_words.intersection(&prev_words).count();
        let union = current_words.union(&prev_words).count();
        let jaccard = intersection as f32 / union.max(1) as f32;

        jaccard > 0.8
    }

    /// Compute a coherence penalty for unlikely category transitions.
    fn coherence_penalty(&self, prev: &StepCategory, current: &StepCategory) -> f32 {
        match (prev, current) {
            // Natural transitions: no penalty.
            (StepCategory::Reasoning, StepCategory::ToolSelection) => 0.0,
            (StepCategory::ToolSelection, StepCategory::ToolUsage) => 0.0,
            (StepCategory::ToolUsage, StepCategory::InformationGain) => 0.0,
            (StepCategory::ToolUsage, StepCategory::Reasoning) => 0.0,
            (StepCategory::InformationGain, StepCategory::Reasoning) => 0.0,
            (StepCategory::Reasoning, StepCategory::Reasoning) => 0.0,

            // Suspicious: selecting a tool right after using one without reasoning.
            (StepCategory::ToolUsage, StepCategory::ToolSelection) => 0.05,

            // Going backwards: tool selection before reasoning.
            (StepCategory::ToolSelection, StepCategory::Reasoning) => 0.1,

            // Other transitions get a small penalty.
            _ => 0.02,
        }
    }

    /// Aggregate step scores into a single trajectory score.
    fn aggregate_scores(&self, steps: &[StepReward]) -> f32 {
        if steps.is_empty() {
            return 0.0;
        }

        match &self.config.aggregate_method {
            AggregateMethod::Mean => {
                let sum: f32 = steps.iter().map(|s| s.score).sum();
                sum / steps.len() as f32
            }

            AggregateMethod::WeightedMean => {
                let mut weighted_sum = 0.0_f32;
                let mut weight_total = 0.0_f32;
                for step in steps {
                    let w = self
                        .config
                        .step_weights
                        .get(&step.category)
                        .copied()
                        .unwrap_or(1.0);
                    weighted_sum += step.score * w;
                    weight_total += w;
                }
                if weight_total < f32::EPSILON {
                    0.0
                } else {
                    weighted_sum / weight_total
                }
            }

            AggregateMethod::Min => steps.iter().map(|s| s.score).fold(f32::INFINITY, f32::min),

            AggregateMethod::Product => {
                let product: f32 = steps.iter().map(|s| s.score).product();
                product
            }
        }
    }

    /// Classify the overall trajectory quality from the aggregate score and flags.
    fn classify_trajectory(&self, aggregate: f32, steps: &[StepReward]) -> TrajectoryQuality {
        // If any step is flagged as Risky, the trajectory is Harmful.
        let has_risky = steps.iter().any(|s| s.flags.contains(&RewardFlag::Risky));
        if has_risky {
            return TrajectoryQuality::Harmful;
        }

        if aggregate >= 0.85 {
            TrajectoryQuality::Optimal
        } else if aggregate >= 0.65 {
            TrajectoryQuality::Good
        } else if aggregate >= 0.45 {
            TrajectoryQuality::Suboptimal
        } else {
            TrajectoryQuality::Poor
        }
    }

    /// Generate improvement recommendations based on scored steps.
    fn generate_recommendations(&self, steps: &[StepReward], flagged: &[usize]) -> Vec<String> {
        let mut recs = Vec::new();

        if flagged.is_empty() && steps.iter().all(|s| s.score > 0.7) {
            recs.push("Trajectory looks solid — no major issues detected.".into());
            return recs;
        }

        // Redundancy recommendations.
        let redundant_count = steps
            .iter()
            .filter(|s| s.flags.contains(&RewardFlag::Redundant))
            .count();
        if redundant_count > 0 {
            recs.push(format!(
                "Found {redundant_count} redundant step(s). Consider consolidating repeated work."
            ));
        }

        // Safety recommendations.
        let risky_count = steps
            .iter()
            .filter(|s| s.flags.contains(&RewardFlag::Risky))
            .count();
        if risky_count > 0 {
            recs.push(format!(
                "Found {risky_count} step(s) with safety concerns. Review before executing."
            ));
        }

        // Efficiency recommendations.
        let inefficient_count = steps
            .iter()
            .filter(|s| s.flags.contains(&RewardFlag::Inefficient))
            .count();
        if inefficient_count > 0 {
            recs.push(format!(
                "Found {inefficient_count} inefficient step(s). Consider more direct approaches."
            ));
        }

        // Low-scoring steps.
        if !flagged.is_empty() {
            recs.push(format!(
                "Steps at indices {:?} scored below {:.1}. Consider revising these.",
                flagged, self.config.min_step_score
            ));
        }

        recs
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

/// Count how many keywords from a list appear in the text.
fn count_keyword_hits(text: &str, keywords: &[&str]) -> usize {
    keywords.iter().filter(|kw| text.contains(**kw)).count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn default_model() -> ProcessRewardModel {
        ProcessRewardModel::with_defaults()
    }

    fn make_steps(pairs: &[(&str, StepCategory)]) -> Vec<(String, StepCategory)> {
        pairs
            .iter()
            .map(|(d, c)| (d.to_string(), c.clone()))
            .collect()
    }

    // -- Config defaults -------------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = RewardConfig::default();
        assert!(config.enabled);
        assert!((config.min_step_score - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.aggregate_method, AggregateMethod::WeightedMean);
        assert!(config.step_weights.contains_key(&StepCategory::Safety));
    }

    #[test]
    fn test_safety_weight_highest() {
        let config = RewardConfig::default();
        let safety_w = config.step_weights[&StepCategory::Safety];
        for (cat, w) in &config.step_weights {
            if *cat != StepCategory::Safety {
                assert!(
                    safety_w >= *w,
                    "Safety weight ({safety_w}) should be >= {cat:?} weight ({w})"
                );
            }
        }
    }

    // -- Empty trajectory ------------------------------------------------------

    #[test]
    fn test_empty_trajectory() {
        let model = default_model();
        let result = model.score_trajectory(&[]);
        assert!(result.steps.is_empty());
        assert!((result.aggregate_score - 0.0).abs() < f32::EPSILON);
        assert_eq!(result.trajectory_quality, TrajectoryQuality::Poor);
        assert!(!result.recommendations.is_empty());
    }

    // -- Single step scoring ---------------------------------------------------

    #[test]
    fn test_single_reasoning_positive() {
        let model = default_model();
        let steps = make_steps(&[(
            "Analyze the error because the stack trace shows a null pointer. Therefore we need to check the initialization.",
            StepCategory::Reasoning,
        )]);
        let result = model.score_trajectory(&steps);
        assert_eq!(result.steps.len(), 1);
        assert!(
            result.steps[0].score > 0.5,
            "Positive reasoning should score > 0.5"
        );
    }

    #[test]
    fn test_single_reasoning_negative() {
        let model = default_model();
        let steps = make_steps(&[(
            "I guess maybe we should just try something random, not sure what to do",
            StepCategory::Reasoning,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(
            result.steps[0].score < 0.6,
            "Negative reasoning should score lower"
        );
    }

    #[test]
    fn test_single_safety_risky() {
        let model = default_model();
        let steps = make_steps(&[(
            "Running rm -rf to delete all files and bypass security with sudo",
            StepCategory::Safety,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[0].flags.contains(&RewardFlag::Risky));
        assert_eq!(result.trajectory_quality, TrajectoryQuality::Harmful);
    }

    #[test]
    fn test_single_safety_positive() {
        let model = default_model();
        let steps = make_steps(&[(
            "Validate input parameters and check permissions before proceeding. Audit log enabled.",
            StepCategory::Safety,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[0].score > 0.5);
        assert!(!result.steps[0].flags.contains(&RewardFlag::Risky));
    }

    #[test]
    fn test_tool_selection_positive() {
        let model = default_model();
        let steps = make_steps(&[(
            "Selected the most appropriate and suitable tool for file reading as the best fit",
            StepCategory::ToolSelection,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[0].score > 0.5);
    }

    #[test]
    fn test_tool_selection_negative() {
        let model = default_model();
        let steps = make_steps(&[(
            "Used the wrong tool which was inappropriate. Should have used a different one.",
            StepCategory::ToolSelection,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[0].score < 0.6);
    }

    #[test]
    fn test_efficiency_negative() {
        let model = default_model();
        let steps = make_steps(&[(
            "Using a verbose workaround and brute force approach, very slow and wasteful",
            StepCategory::Efficiency,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[0].flags.contains(&RewardFlag::Inefficient));
    }

    #[test]
    fn test_information_gain_redundant() {
        let model = default_model();
        let steps = make_steps(&[(
            "Repeat the same query again, already have this duplicate data",
            StepCategory::InformationGain,
        )]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[0].flags.contains(&RewardFlag::Redundant));
    }

    // -- Multi-step trajectory -------------------------------------------------

    #[test]
    fn test_multi_step_good_trajectory() {
        let model = default_model();
        let steps = make_steps(&[
            (
                "Analyze the problem because the user needs data transformation. Consider the best approach.",
                StepCategory::Reasoning,
            ),
            (
                "Selected the most appropriate tool for JSON parsing as the best fit.",
                StepCategory::ToolSelection,
            ),
            (
                "Executed the tool and found the relevant data in the response output.",
                StepCategory::ToolUsage,
            ),
        ]);
        let result = model.score_trajectory(&steps);
        assert_eq!(result.steps.len(), 3);
        assert!(result.aggregate_score > 0.4);
        assert!(
            result.trajectory_quality == TrajectoryQuality::Good
                || result.trajectory_quality == TrajectoryQuality::Optimal
                || result.trajectory_quality == TrajectoryQuality::Suboptimal
        );
    }

    #[test]
    fn test_redundant_consecutive_steps() {
        let model = default_model();
        let steps = make_steps(&[
            ("Read the file contents from disk.", StepCategory::ToolUsage),
            ("Read the file contents from disk.", StepCategory::ToolUsage),
        ]);
        let result = model.score_trajectory(&steps);
        // Second step should be flagged as redundant.
        assert!(result.steps[1].flags.contains(&RewardFlag::Redundant));
    }

    #[test]
    fn test_similar_steps_detected_redundant() {
        let model = default_model();
        let steps = make_steps(&[
            (
                "Read the file contents from the disk path",
                StepCategory::ToolUsage,
            ),
            ("Read file contents from disk path", StepCategory::ToolUsage),
        ]);
        let result = model.score_trajectory(&steps);
        assert!(result.steps[1].flags.contains(&RewardFlag::Redundant));
    }

    // -- Aggregation methods ---------------------------------------------------

    #[test]
    fn test_aggregate_mean() {
        let config = RewardConfig {
            aggregate_method: AggregateMethod::Mean,
            ..Default::default()
        };
        let model = ProcessRewardModel::new(config);
        let steps = make_steps(&[
            (
                "Good reasoning because analysis shows clear evidence.",
                StepCategory::Reasoning,
            ),
            ("Neutral step with no keywords.", StepCategory::Coherence),
        ]);
        let result = model.score_trajectory(&steps);
        // Mean should be between the two scores.
        assert!(result.aggregate_score > 0.0);
        assert!(result.aggregate_score <= 1.0);
    }

    #[test]
    fn test_aggregate_min() {
        let config = RewardConfig {
            aggregate_method: AggregateMethod::Min,
            ..Default::default()
        };
        let model = ProcessRewardModel::new(config);
        let steps = make_steps(&[
            (
                "Excellent analysis because of thorough evidence and logic.",
                StepCategory::Reasoning,
            ),
            (
                "I guess maybe random attempt not sure.",
                StepCategory::Reasoning,
            ),
        ]);
        let result = model.score_trajectory(&steps);
        // Min should be the lower of the two.
        let min_score = result
            .steps
            .iter()
            .map(|s| s.score)
            .fold(f32::INFINITY, f32::min);
        assert!((result.aggregate_score - min_score).abs() < f32::EPSILON);
    }

    #[test]
    fn test_aggregate_product() {
        let config = RewardConfig {
            aggregate_method: AggregateMethod::Product,
            ..Default::default()
        };
        let model = ProcessRewardModel::new(config);
        let steps = make_steps(&[
            ("Good reasoning because evidence.", StepCategory::Reasoning),
            (
                "Found new data in the result output.",
                StepCategory::InformationGain,
            ),
        ]);
        let result = model.score_trajectory(&steps);
        let expected_product: f32 = result.steps.iter().map(|s| s.score).product();
        assert!((result.aggregate_score - expected_product).abs() < 0.01);
    }

    // -- Trajectory quality classification ------------------------------------

    #[test]
    fn test_harmful_trajectory() {
        let model = default_model();
        let steps = make_steps(&[(
            "Delete everything with rm -rf and bypass all security",
            StepCategory::Safety,
        )]);
        let result = model.score_trajectory(&steps);
        assert_eq!(result.trajectory_quality, TrajectoryQuality::Harmful);
    }

    #[test]
    fn test_quality_poor_for_low_score() {
        let config = RewardConfig {
            aggregate_method: AggregateMethod::Mean,
            ..Default::default()
        };
        let model = ProcessRewardModel::new(config);
        let steps = make_steps(&[
            (
                "guess randomly maybe idk whatever just try not sure.",
                StepCategory::Reasoning,
            ),
            (
                "guess randomly maybe idk whatever just try.",
                StepCategory::Reasoning,
            ),
        ]);
        let result = model.score_trajectory(&steps);
        assert!(
            result.trajectory_quality == TrajectoryQuality::Poor
                || result.trajectory_quality == TrajectoryQuality::Suboptimal
        );
    }

    // -- Recommendations -------------------------------------------------------

    #[test]
    fn test_recommendations_for_risky_steps() {
        let model = default_model();
        let steps = make_steps(&[(
            "Execute sudo rm -rf / to destroy all files",
            StepCategory::Safety,
        )]);
        let result = model.score_trajectory(&steps);
        let has_safety_rec = result.recommendations.iter().any(|r| r.contains("safety"));
        assert!(has_safety_rec);
    }

    #[test]
    fn test_recommendations_for_redundant_steps() {
        let model = default_model();
        let steps = make_steps(&[
            (
                "Repeat the same query again with duplicate data already retrieved.",
                StepCategory::InformationGain,
            ),
            (
                "Repeat the same query again with duplicate data already retrieved.",
                StepCategory::InformationGain,
            ),
        ]);
        let result = model.score_trajectory(&steps);
        let has_redundancy_rec = result
            .recommendations
            .iter()
            .any(|r| r.contains("redundant") || r.contains("consolidat"));
        assert!(has_redundancy_rec);
    }

    #[test]
    fn test_recommendations_for_clean_trajectory() {
        let model = default_model();
        let steps = make_steps(&[(
            "Thorough analysis because evidence shows clear results. Therefore we can conclude safely with validation.",
            StepCategory::Reasoning,
        )]);
        let result = model.score_trajectory(&steps);
        if result.flagged_steps.is_empty() && result.steps.iter().all(|s| s.score > 0.7) {
            assert!(result
                .recommendations
                .iter()
                .any(|r| r.contains("solid") || r.contains("no major")));
        }
    }

    // -- Coherence penalties ---------------------------------------------------

    #[test]
    fn test_natural_transition_no_penalty() {
        let model = default_model();
        let penalty =
            model.coherence_penalty(&StepCategory::Reasoning, &StepCategory::ToolSelection);
        assert!((penalty - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_backward_transition_penalty() {
        let model = default_model();
        let penalty =
            model.coherence_penalty(&StepCategory::ToolSelection, &StepCategory::Reasoning);
        assert!(penalty > 0.0);
    }

    // -- Flagging steps below threshold ----------------------------------------

    #[test]
    fn test_flagged_steps_below_threshold() {
        let config = RewardConfig {
            min_step_score: 0.6,
            ..Default::default()
        };
        let model = ProcessRewardModel::new(config);
        let steps = make_steps(&[
            (
                "guess maybe idk not sure random whatever just try.",
                StepCategory::Reasoning,
            ),
            (
                "Good analysis because evidence clearly shows the conclusion therefore we deduce.",
                StepCategory::Reasoning,
            ),
        ]);
        let result = model.score_trajectory(&steps);
        // The negative step should be flagged.
        if result.steps[0].score < 0.6 {
            assert!(result.flagged_steps.contains(&0));
        }
    }

    // -- Length heuristic ------------------------------------------------------

    #[test]
    fn test_very_short_description_penalized() {
        let model = default_model();
        let steps = make_steps(&[("ok", StepCategory::Reasoning)]);
        let result = model.score_trajectory(&steps);
        // "ok" has < 10 chars so gets a length penalty (0.8 factor).
        assert!(result.steps[0].score <= 0.5);
    }

    // -- Excellent flag --------------------------------------------------------

    #[test]
    fn test_excellent_flag_on_high_score() {
        let model = default_model();
        let steps = make_steps(&[(
            "Thorough analysis because evidence shows clear logic. Therefore conclude with reasoning and evaluate.",
            StepCategory::Reasoning,
        )]);
        let result = model.score_trajectory(&steps);
        if result.steps[0].score > 0.85 {
            assert!(result.steps[0].flags.contains(&RewardFlag::Excellent));
        }
    }

    // -- Config accessor -------------------------------------------------------

    #[test]
    fn test_config_accessor() {
        let model = default_model();
        assert!(model.config().enabled);
    }

    // -- Count keyword hits helper ---------------------------------------------

    #[test]
    fn test_count_keyword_hits() {
        let hits = count_keyword_hits(
            "therefore we should analyze because evidence",
            REASONING_POSITIVE,
        );
        assert!(hits >= 2); // "therefore", "because", "evidence"
    }

    #[test]
    fn test_count_keyword_hits_zero() {
        let hits = count_keyword_hits("nothing relevant here", SAFETY_NEGATIVE);
        assert_eq!(hits, 0);
    }

    // -- Redundancy detection --------------------------------------------------

    #[test]
    fn test_is_redundant_exact() {
        let model = default_model();
        assert!(model.is_redundant("read file", "read file"));
    }

    #[test]
    fn test_is_redundant_different() {
        let model = default_model();
        assert!(!model.is_redundant("read the configuration file", "execute the shell command"));
    }
}

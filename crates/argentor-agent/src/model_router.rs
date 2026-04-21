//! Cost-aware model routing for multi-tier LLM selection.
//!
//! Automatically routes simple tasks to cheaper/faster models and complex tasks
//! to more capable (but expensive) ones, based on heuristic complexity estimation
//! and configurable routing strategies.
//!
//! # Example
//!
//! ```rust,no_run
//! use argentor_agent::model_router::{ModelRouter, RoutingStrategy, ModelOption, ModelTier, ModelCost};
//! use argentor_agent::config::{LlmProvider, ModelConfig};
//!
//! let mut router = ModelRouter::new(RoutingStrategy::Balanced);
//! for model in ModelRouter::claude_preset("sk-placeholder") {
//!     router.add_model(model);
//! }
//! router.set_budget(5.0);
//!
//! let decision = router.route("Hello world", 0, 0);
//! ```

use crate::config::{LlmProvider, ModelConfig};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Model tier — determines cost and capability level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelTier {
    /// Fast, cheap models for simple tasks (e.g., haiku, gpt-4o-mini).
    Fast,
    /// Balanced models for most tasks (e.g., sonnet, gpt-4o).
    Balanced,
    /// Most capable models for complex reasoning (e.g., opus, o1).
    Powerful,
}

/// Cost information for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    /// Cost per 1M input tokens (USD).
    pub input_cost_per_mtok: f64,
    /// Cost per 1M output tokens (USD).
    pub output_cost_per_mtok: f64,
    /// Estimated tokens per second throughput.
    pub tokens_per_second: f64,
}

/// A model option with its configuration and metadata.
#[derive(Debug, Clone)]
pub struct ModelOption {
    /// The model configuration (provider, model_id, api_key, etc.).
    pub config: ModelConfig,
    /// The tier this model belongs to.
    pub tier: ModelTier,
    /// Pricing information.
    pub cost: ModelCost,
    /// Maximum task complexity this model handles well (0.0–1.0).
    pub max_complexity: f32,
}

/// Task complexity estimation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskComplexity {
    /// Overall complexity score (0.0–1.0).
    pub score: f32,
    /// Individual factors that contributed to the score.
    pub factors: Vec<ComplexityFactor>,
}

/// A single factor contributing to task complexity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityFactor {
    /// Human-readable factor name.
    pub name: String,
    /// Individual score for this factor (0.0–1.0).
    pub score: f32,
    /// Weight of this factor in the overall computation.
    pub weight: f32,
}

/// Routing decision — which model to use and why.
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// The selected model option.
    pub selected_model: ModelOption,
    /// Computed task complexity.
    pub task_complexity: TaskComplexity,
    /// Human-readable reason for the selection.
    pub reason: String,
    /// Estimated cost for this task in USD.
    pub estimated_cost_usd: f64,
}

/// How the router selects models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoutingStrategy {
    /// Always use the cheapest model that can handle the task complexity.
    CostOptimized,
    /// Always use the most capable model regardless of cost.
    QualityOptimized,
    /// Balance cost and quality — prefer models whose capability closely matches task complexity.
    Balanced,
    /// Use tier-based routing with explicit complexity thresholds.
    Tiered {
        /// Tasks below this complexity score use the Fast tier.
        simple_threshold: f32,
        /// Tasks at or above this complexity score use the Powerful tier.
        complex_threshold: f32,
    },
}

/// The cost-aware model router.
///
/// Maintains a set of [`ModelOption`]s and routes incoming tasks to the most
/// appropriate model based on heuristic complexity estimation and the configured
/// [`RoutingStrategy`].
pub struct ModelRouter {
    /// Available models.
    models: Vec<ModelOption>,
    /// Active routing strategy.
    strategy: RoutingStrategy,
    /// Cumulative cost tracker (USD).
    total_cost_usd: f64,
    /// Optional budget ceiling (USD).
    budget_limit: Option<f64>,
}

impl ModelRouter {
    /// Create a new router with no models and the given strategy.
    pub fn new(strategy: RoutingStrategy) -> Self {
        Self {
            models: Vec::new(),
            strategy,
            total_cost_usd: 0.0,
            budget_limit: None,
        }
    }

    /// Add a model option to the router.
    pub fn add_model(&mut self, option: ModelOption) {
        self.models.push(option);
    }

    /// Set a budget limit in USD. Once cumulative cost reaches this limit,
    /// [`route`](Self::route) will return an error.
    pub fn set_budget(&mut self, limit: f64) {
        self.budget_limit = Some(limit);
    }

    /// Get the remaining budget, or `None` if no budget was set.
    pub fn remaining_budget(&self) -> Option<f64> {
        self.budget_limit.map(|limit| {
            let remaining = limit - self.total_cost_usd;
            if remaining < 0.0 {
                0.0
            } else {
                remaining
            }
        })
    }

    /// Estimate the complexity of a task based on heuristics.
    ///
    /// Factors considered:
    /// 1. Task text length (longer tasks tend to be more complex)
    /// 2. Number of available tools (more tool choice = harder reasoning)
    /// 3. Conversation history length (longer context = more state to track)
    /// 4. Presence of complexity-indicating keywords ("analyze", "compare", etc.)
    /// 5. Presence of simplicity-indicating keywords ("list", "show", "hello", etc.)
    /// 6. Multiple question marks (compound questions)
    /// 7. Code blocks or technical terms
    pub fn estimate_complexity(
        &self,
        task: &str,
        tool_count: usize,
        history_length: usize,
    ) -> TaskComplexity {
        let mut factors = Vec::new();
        let lower = task.to_lowercase();

        // Factor 1: Task length
        let length_score = (task.len() as f32 / 1000.0).min(1.0);
        factors.push(ComplexityFactor {
            name: "task_length".into(),
            score: length_score,
            weight: 0.15,
        });

        // Factor 2: Tool count
        let tool_score = (tool_count as f32 / 20.0).min(1.0);
        factors.push(ComplexityFactor {
            name: "tool_count".into(),
            score: tool_score,
            weight: 0.15,
        });

        // Factor 3: History length
        let history_score = (history_length as f32 / 50.0).min(1.0);
        factors.push(ComplexityFactor {
            name: "history_length".into(),
            score: history_score,
            weight: 0.10,
        });

        // Factor 4: Complex keywords
        let complex_keywords = [
            "analyze",
            "analyse",
            "compare",
            "design",
            "optimize",
            "implement",
            "refactor",
            "debug",
            "architecture",
            "algorithm",
            "trade-off",
            "tradeoff",
            "evaluate",
            "synthesize",
            "comprehensive",
            "multi-step",
            "strategy",
        ];
        let complex_count = complex_keywords
            .iter()
            .filter(|kw| lower.contains(*kw))
            .count();
        let complex_score = (complex_count as f32 / 3.0).min(1.0);
        factors.push(ComplexityFactor {
            name: "complex_keywords".into(),
            score: complex_score,
            weight: 0.20,
        });

        // Factor 5: Simple keywords (reduces complexity)
        let simple_keywords = [
            "list", "show", "count", "hello", "hi", "hey", "thanks", "yes", "no", "ok", "help",
            "what is", "define", "ping",
        ];
        let simple_count = simple_keywords
            .iter()
            .filter(|kw| lower.contains(*kw))
            .count();
        let simple_score = (simple_count as f32 / 2.0).min(1.0);
        factors.push(ComplexityFactor {
            name: "simple_keywords".into(),
            score: simple_score,
            weight: -0.15,
        });

        // Factor 6: Multiple questions
        let question_count = task.chars().filter(|c| *c == '?').count();
        let question_score = (question_count as f32 / 3.0).min(1.0);
        factors.push(ComplexityFactor {
            name: "question_count".into(),
            score: question_score,
            weight: 0.10,
        });

        // Factor 7: Code / technical content
        let has_code_block = task.contains("```") || task.contains("fn ") || task.contains("def ");
        let technical_terms = [
            "api",
            "database",
            "schema",
            "struct",
            "trait",
            "async",
            "mutex",
            "thread",
            "concurrency",
            "distributed",
        ];
        let tech_count = technical_terms
            .iter()
            .filter(|t| lower.contains(*t))
            .count();
        let tech_score = if has_code_block {
            ((tech_count as f32 / 3.0) + 0.3).min(1.0)
        } else {
            (tech_count as f32 / 5.0).min(1.0)
        };
        factors.push(ComplexityFactor {
            name: "technical_content".into(),
            score: tech_score,
            weight: 0.15,
        });

        // Compute weighted score
        let weighted_sum: f32 = factors.iter().map(|f| f.score * f.weight).sum();

        // Clamp to [0.0, 1.0]
        let score = weighted_sum.clamp(0.0, 1.0);

        TaskComplexity { score, factors }
    }

    /// Route a task to the best model given the current strategy.
    ///
    /// Returns an error if:
    /// - No models are registered
    /// - The budget has been exhausted
    /// - No suitable model is found for the task complexity
    pub fn route(
        &self,
        task: &str,
        tool_count: usize,
        history_length: usize,
    ) -> Result<RoutingDecision, String> {
        if self.models.is_empty() {
            return Err("No models registered in the router".to_string());
        }

        let complexity = self.estimate_complexity(task, tool_count, history_length);

        // Check budget
        if let Some(limit) = self.budget_limit {
            if self.total_cost_usd >= limit {
                return Err("Budget exhausted".to_string());
            }
        }

        let selected = match &self.strategy {
            RoutingStrategy::CostOptimized => {
                // Find the cheapest model that can handle the task complexity
                self.models
                    .iter()
                    .filter(|m| m.max_complexity >= complexity.score)
                    .min_by(|a, b| {
                        a.cost
                            .input_cost_per_mtok
                            .partial_cmp(&b.cost.input_cost_per_mtok)
                            .unwrap_or(Ordering::Equal)
                    })
                    // Fallback: if no model can handle the complexity, use the most capable
                    .or_else(|| {
                        self.models.iter().max_by(|a, b| {
                            a.max_complexity
                                .partial_cmp(&b.max_complexity)
                                .unwrap_or(Ordering::Equal)
                        })
                    })
            }
            RoutingStrategy::QualityOptimized => {
                // Always pick the most capable model
                self.models.iter().max_by(|a, b| {
                    a.max_complexity
                        .partial_cmp(&b.max_complexity)
                        .unwrap_or(Ordering::Equal)
                })
            }
            RoutingStrategy::Balanced => {
                // Score each model: higher is better.
                // Prefer models whose max_complexity is close to (but >= ) the task complexity.
                // Also penalise expensive models when the task is simple.
                self.models
                    .iter()
                    .filter(|m| m.max_complexity >= complexity.score)
                    .max_by(|a, b| {
                        let score_a = Self::balanced_score(a, complexity.score);
                        let score_b = Self::balanced_score(b, complexity.score);
                        score_a.partial_cmp(&score_b).unwrap_or(Ordering::Equal)
                    })
                    // Fallback to the most capable if nothing matches
                    .or_else(|| {
                        self.models.iter().max_by(|a, b| {
                            a.max_complexity
                                .partial_cmp(&b.max_complexity)
                                .unwrap_or(Ordering::Equal)
                        })
                    })
            }
            RoutingStrategy::Tiered {
                simple_threshold,
                complex_threshold,
            } => {
                let target_tier = if complexity.score <= *simple_threshold {
                    ModelTier::Fast
                } else if complexity.score >= *complex_threshold {
                    ModelTier::Powerful
                } else {
                    ModelTier::Balanced
                };
                self.models
                    .iter()
                    .find(|m| m.tier == target_tier)
                    .or_else(|| {
                        // Fallback: try adjacent tiers
                        match target_tier {
                            ModelTier::Fast => self
                                .models
                                .iter()
                                .find(|m| m.tier == ModelTier::Balanced)
                                .or_else(|| {
                                    self.models.iter().find(|m| m.tier == ModelTier::Powerful)
                                }),
                            ModelTier::Powerful => self
                                .models
                                .iter()
                                .find(|m| m.tier == ModelTier::Balanced)
                                .or_else(|| self.models.iter().find(|m| m.tier == ModelTier::Fast)),
                            ModelTier::Balanced => self
                                .models
                                .iter()
                                .find(|m| m.tier == ModelTier::Powerful)
                                .or_else(|| self.models.iter().find(|m| m.tier == ModelTier::Fast)),
                        }
                    })
                    .or(self.models.first())
            }
        };

        let selected = selected.ok_or("No suitable model found for the task")?;

        // Estimate cost based on task length (rough heuristic: ~4 chars per token)
        let estimated_input_tokens = (task.len() as f64 / 4.0).ceil();
        // Assume output is roughly proportional to input for estimation
        let estimated_output_tokens = estimated_input_tokens * 1.5;
        let estimated_cost_usd = (estimated_input_tokens / 1_000_000.0)
            * selected.cost.input_cost_per_mtok
            + (estimated_output_tokens / 1_000_000.0) * selected.cost.output_cost_per_mtok;

        let reason = format!(
            "Task complexity {:.2} routed to {} ({:?} tier) via {:?} strategy",
            complexity.score,
            selected.config.model_id,
            selected.tier,
            self.strategy_name(),
        );

        Ok(RoutingDecision {
            selected_model: selected.clone(),
            task_complexity: complexity,
            reason,
            estimated_cost_usd,
        })
    }

    /// Record the actual cost after a model call completes.
    pub fn record_cost(&mut self, input_tokens: u64, output_tokens: u64, model: &ModelOption) {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * model.cost.input_cost_per_mtok;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * model.cost.output_cost_per_mtok;
        self.total_cost_usd += input_cost + output_cost;
    }

    /// Get the total cost spent so far in USD.
    pub fn total_cost(&self) -> f64 {
        self.total_cost_usd
    }

    /// Create Claude model presets with the given API key.
    ///
    /// Returns a `Vec` of three models: Haiku (Fast), Sonnet (Balanced), and Opus (Powerful).
    pub fn claude_preset(api_key: &str) -> Vec<ModelOption> {
        vec![
            ModelOption {
                config: ModelConfig {
                    provider: LlmProvider::Claude,
                    model_id: "claude-haiku-4-5-20251001".into(),
                    api_key: api_key.into(),
                    api_base_url: None,
                    temperature: 0.7,
                    max_tokens: 4096,
                    max_turns: 20,
                    max_context_tokens: 200_000,
                    fallback_models: Vec::new(),
                    retry_policy: None,
                },
                tier: ModelTier::Fast,
                cost: ModelCost {
                    input_cost_per_mtok: 0.80,
                    output_cost_per_mtok: 4.0,
                    tokens_per_second: 200.0,
                },
                max_complexity: 0.4,
            },
            ModelOption {
                config: ModelConfig {
                    provider: LlmProvider::Claude,
                    model_id: "claude-sonnet-4-6".into(),
                    api_key: api_key.into(),
                    api_base_url: None,
                    temperature: 0.7,
                    max_tokens: 4096,
                    max_turns: 20,
                    max_context_tokens: 200_000,
                    fallback_models: Vec::new(),
                    retry_policy: None,
                },
                tier: ModelTier::Balanced,
                cost: ModelCost {
                    input_cost_per_mtok: 3.0,
                    output_cost_per_mtok: 15.0,
                    tokens_per_second: 150.0,
                },
                max_complexity: 0.8,
            },
            ModelOption {
                config: ModelConfig {
                    provider: LlmProvider::Claude,
                    model_id: "claude-opus-4-6".into(),
                    api_key: api_key.into(),
                    api_base_url: None,
                    temperature: 0.7,
                    max_tokens: 4096,
                    max_turns: 20,
                    max_context_tokens: 200_000,
                    fallback_models: Vec::new(),
                    retry_policy: None,
                },
                tier: ModelTier::Powerful,
                cost: ModelCost {
                    input_cost_per_mtok: 15.0,
                    output_cost_per_mtok: 75.0,
                    tokens_per_second: 80.0,
                },
                max_complexity: 1.0,
            },
        ]
    }

    /// Create OpenAI model presets with the given API key.
    ///
    /// Returns a `Vec` of three models: GPT-4o-mini (Fast), GPT-4o (Balanced), o1 (Powerful).
    pub fn openai_preset(api_key: &str) -> Vec<ModelOption> {
        vec![
            ModelOption {
                config: ModelConfig {
                    provider: LlmProvider::OpenAi,
                    model_id: "gpt-4o-mini".into(),
                    api_key: api_key.into(),
                    api_base_url: None,
                    temperature: 0.7,
                    max_tokens: 4096,
                    max_turns: 20,
                    max_context_tokens: 200_000,
                    fallback_models: Vec::new(),
                    retry_policy: None,
                },
                tier: ModelTier::Fast,
                cost: ModelCost {
                    input_cost_per_mtok: 0.15,
                    output_cost_per_mtok: 0.60,
                    tokens_per_second: 250.0,
                },
                max_complexity: 0.4,
            },
            ModelOption {
                config: ModelConfig {
                    provider: LlmProvider::OpenAi,
                    model_id: "gpt-4o".into(),
                    api_key: api_key.into(),
                    api_base_url: None,
                    temperature: 0.7,
                    max_tokens: 4096,
                    max_turns: 20,
                    max_context_tokens: 200_000,
                    fallback_models: Vec::new(),
                    retry_policy: None,
                },
                tier: ModelTier::Balanced,
                cost: ModelCost {
                    input_cost_per_mtok: 2.50,
                    output_cost_per_mtok: 10.0,
                    tokens_per_second: 150.0,
                },
                max_complexity: 0.75,
            },
            ModelOption {
                config: ModelConfig {
                    provider: LlmProvider::OpenAi,
                    model_id: "o1".into(),
                    api_key: api_key.into(),
                    api_base_url: None,
                    temperature: 0.7,
                    max_tokens: 4096,
                    max_turns: 20,
                    max_context_tokens: 200_000,
                    fallback_models: Vec::new(),
                    retry_policy: None,
                },
                tier: ModelTier::Powerful,
                cost: ModelCost {
                    input_cost_per_mtok: 15.0,
                    output_cost_per_mtok: 60.0,
                    tokens_per_second: 40.0,
                },
                max_complexity: 1.0,
            },
        ]
    }

    // ── Private helpers ──────────────────────────────────────────────────

    /// Compute a balanced score for a model given task complexity.
    ///
    /// Higher is better. Rewards capability match (0.6 weight) and cost
    /// efficiency (0.4 weight).
    fn balanced_score(model: &ModelOption, task_complexity: f32) -> f64 {
        // Capability match: prefer models whose max_complexity is close to the task
        // complexity (but at least as high). Excess capacity is penalised lightly.
        let capability_match = if model.max_complexity >= task_complexity {
            let excess = (model.max_complexity - task_complexity) as f64;
            1.0 - (excess * 0.5) // mild penalty for overkill
        } else {
            0.0 // cannot handle the task
        };

        // Cost efficiency: inverse of input cost, normalised
        let cost_efficiency = 1.0 / (1.0 + model.cost.input_cost_per_mtok);

        capability_match * 0.6 + cost_efficiency * 0.4
    }

    /// Return a human-readable name for the current strategy.
    fn strategy_name(&self) -> &str {
        match &self.strategy {
            RoutingStrategy::CostOptimized => "CostOptimized",
            RoutingStrategy::QualityOptimized => "QualityOptimized",
            RoutingStrategy::Balanced => "Balanced",
            RoutingStrategy::Tiered { .. } => "Tiered",
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Helper: build a ModelOption with sensible defaults.
    fn make_model(
        model_id: &str,
        tier: ModelTier,
        input_cost: f64,
        output_cost: f64,
        max_complexity: f32,
    ) -> ModelOption {
        ModelOption {
            config: ModelConfig {
                provider: LlmProvider::Claude,
                model_id: model_id.into(),
                api_key: "test-key".into(),
                api_base_url: None,
                temperature: 0.7,
                max_tokens: 4096,
                max_turns: 20,
                max_context_tokens: 200_000,
                fallback_models: Vec::new(),
                retry_policy: None,
            },
            tier,
            cost: ModelCost {
                input_cost_per_mtok: input_cost,
                output_cost_per_mtok: output_cost,
                tokens_per_second: 100.0,
            },
            max_complexity,
        }
    }

    /// Helper: build a router pre-loaded with three Claude-like tiers.
    fn make_tiered_router(strategy: RoutingStrategy) -> ModelRouter {
        let mut router = ModelRouter::new(strategy);
        router.add_model(make_model("haiku", ModelTier::Fast, 0.80, 4.0, 0.4));
        router.add_model(make_model("sonnet", ModelTier::Balanced, 3.0, 15.0, 0.8));
        router.add_model(make_model("opus", ModelTier::Powerful, 15.0, 75.0, 1.0));
        router
    }

    // ── 1. test_model_tier_equality ──────────────────────────────────────

    #[test]
    fn test_model_tier_equality() {
        assert_eq!(ModelTier::Fast, ModelTier::Fast);
        assert_eq!(ModelTier::Balanced, ModelTier::Balanced);
        assert_eq!(ModelTier::Powerful, ModelTier::Powerful);
        assert_ne!(ModelTier::Fast, ModelTier::Balanced);
        assert_ne!(ModelTier::Fast, ModelTier::Powerful);
        assert_ne!(ModelTier::Balanced, ModelTier::Powerful);
    }

    // ── 2. test_model_cost_creation ──────────────────────────────────────

    #[test]
    fn test_model_cost_creation() {
        let cost = ModelCost {
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
            tokens_per_second: 150.0,
        };
        assert!((cost.input_cost_per_mtok - 3.0).abs() < f64::EPSILON);
        assert!((cost.output_cost_per_mtok - 15.0).abs() < f64::EPSILON);
        assert!((cost.tokens_per_second - 150.0).abs() < f64::EPSILON);
    }

    // ── 3. test_estimate_complexity_simple_task ──────────────────────────

    #[test]
    fn test_estimate_complexity_simple_task() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        let complexity = router.estimate_complexity("hello", 0, 0);
        assert!(
            complexity.score < 0.2,
            "Simple greeting should have low complexity, got {}",
            complexity.score
        );
    }

    // ── 4. test_estimate_complexity_complex_task ─────────────────────────

    #[test]
    fn test_estimate_complexity_complex_task() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        let task = "Analyze and compare the architecture of these two distributed systems. \
                    Design an optimized algorithm that synthesizes the best trade-offs. \
                    Consider concurrency, thread safety, and database schema implications.";
        let complexity = router.estimate_complexity(task, 15, 30);
        assert!(
            complexity.score > 0.4,
            "Complex task should have high complexity, got {}",
            complexity.score
        );
    }

    // ── 5. test_estimate_complexity_medium_task ──────────────────────────

    #[test]
    fn test_estimate_complexity_medium_task() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        let task = "Implement a basic async function in Rust that fetches data from an API \
                    and explain how the concurrency model works compared to threads.";
        let complexity = router.estimate_complexity(task, 3, 5);
        // Should be somewhere in the middle range
        assert!(
            complexity.score > 0.05 && complexity.score < 0.8,
            "Medium task should have moderate complexity, got {}",
            complexity.score
        );
    }

    // ── 6. test_estimate_complexity_with_tools ──────────────────────────

    #[test]
    fn test_estimate_complexity_with_tools() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        let no_tools = router.estimate_complexity("do something", 0, 0);
        let many_tools = router.estimate_complexity("do something", 20, 0);
        assert!(
            many_tools.score > no_tools.score,
            "More tools should increase complexity: {} vs {}",
            many_tools.score,
            no_tools.score
        );
    }

    // ── 7. test_estimate_complexity_with_history ─────────────────────────

    #[test]
    fn test_estimate_complexity_with_history() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        let no_history = router.estimate_complexity("do something", 0, 0);
        let long_history = router.estimate_complexity("do something", 0, 50);
        assert!(
            long_history.score > no_history.score,
            "Longer history should increase complexity: {} vs {}",
            long_history.score,
            no_history.score
        );
    }

    // ── 8. test_route_cost_optimized ────────────────────────────────────

    #[test]
    fn test_route_cost_optimized() {
        let router = make_tiered_router(RoutingStrategy::CostOptimized);
        // Simple task: should pick the cheapest (haiku)
        let decision = router.route("hello", 0, 0).unwrap();
        assert_eq!(
            decision.selected_model.config.model_id, "haiku",
            "CostOptimized should select cheapest model for simple task"
        );
    }

    // ── 9. test_route_quality_optimized ──────────────────────────────────

    #[test]
    fn test_route_quality_optimized() {
        let router = make_tiered_router(RoutingStrategy::QualityOptimized);
        // Any task: should always pick the most capable (opus)
        let decision = router.route("hello", 0, 0).unwrap();
        assert_eq!(
            decision.selected_model.config.model_id, "opus",
            "QualityOptimized should always select most capable model"
        );
    }

    // ── 10. test_route_balanced ──────────────────────────────────────────

    #[test]
    fn test_route_balanced() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        // Simple task: balanced should prefer a cheaper model, not opus
        let decision = router.route("hello", 0, 0).unwrap();
        assert_ne!(
            decision.selected_model.config.model_id, "opus",
            "Balanced should not use most expensive model for simple task"
        );

        // Complex task should NOT pick haiku
        let complex = "Analyze and compare the architecture, design an optimized algorithm, \
                       evaluate trade-offs in this distributed database schema";
        let decision2 = router.route(complex, 15, 30).unwrap();
        assert_ne!(
            decision2.selected_model.config.model_id, "haiku",
            "Balanced should not use cheapest model for complex task"
        );
    }

    // ── 11. test_route_tiered_simple ─────────────────────────────────────

    #[test]
    fn test_route_tiered_simple() {
        let router = make_tiered_router(RoutingStrategy::Tiered {
            simple_threshold: 0.3,
            complex_threshold: 0.7,
        });
        let decision = router.route("hello", 0, 0).unwrap();
        assert_eq!(
            decision.selected_model.tier,
            ModelTier::Fast,
            "Tiered should select Fast tier for simple task"
        );
    }

    // ── 12. test_route_tiered_complex ────────────────────────────────────

    #[test]
    fn test_route_tiered_complex() {
        let router = make_tiered_router(RoutingStrategy::Tiered {
            simple_threshold: 0.3,
            complex_threshold: 0.5,
        });
        let task = "Analyze and compare the architecture of these two distributed systems. \
                    Design an optimized algorithm that synthesizes the best trade-offs. \
                    Consider concurrency, thread safety, and database schema implications. \
                    Evaluate multiple strategies and provide a comprehensive recommendation.";
        let decision = router.route(task, 20, 40).unwrap();
        assert_eq!(
            decision.selected_model.tier,
            ModelTier::Powerful,
            "Tiered should select Powerful tier for complex task, got {:?} (complexity {})",
            decision.selected_model.tier,
            decision.task_complexity.score,
        );
    }

    // ── 13. test_record_cost_updates_total ───────────────────────────────

    #[test]
    fn test_record_cost_updates_total() {
        let mut router = make_tiered_router(RoutingStrategy::Balanced);
        let model = make_model("sonnet", ModelTier::Balanced, 3.0, 15.0, 0.8);

        // 1M input tokens, 500K output tokens
        router.record_cost(1_000_000, 500_000, &model);

        // Expected: (1M / 1M) * 3.0 + (500K / 1M) * 15.0 = 3.0 + 7.5 = 10.5
        assert!(
            (router.total_cost() - 10.5).abs() < 1e-10,
            "Total cost should be 10.5, got {}",
            router.total_cost()
        );

        // Record another call
        router.record_cost(500_000, 250_000, &model);
        // Additional: (500K / 1M) * 3.0 + (250K / 1M) * 15.0 = 1.5 + 3.75 = 5.25
        // Total: 10.5 + 5.25 = 15.75
        assert!(
            (router.total_cost() - 15.75).abs() < 1e-10,
            "Total cost should be 15.75, got {}",
            router.total_cost()
        );
    }

    // ── 14. test_budget_enforcement ──────────────────────────────────────

    #[test]
    fn test_budget_enforcement() {
        let mut router = make_tiered_router(RoutingStrategy::Balanced);
        router.set_budget(1.0);

        // Should work when under budget
        let result = router.route("hello", 0, 0);
        assert!(result.is_ok(), "Should route when under budget");

        // Exhaust the budget
        let model = make_model("sonnet", ModelTier::Balanced, 3.0, 15.0, 0.8);
        router.record_cost(1_000_000, 1_000_000, &model);
        // Cost: 3.0 + 15.0 = 18.0 — well over budget

        // Should fail now
        let result = router.route("hello", 0, 0);
        assert!(result.is_err(), "Should error when budget exhausted");
        assert_eq!(result.unwrap_err(), "Budget exhausted");
    }

    // ── 15. test_remaining_budget ────────────────────────────────────────

    #[test]
    fn test_remaining_budget() {
        let mut router = make_tiered_router(RoutingStrategy::Balanced);

        // No budget set
        assert!(
            router.remaining_budget().is_none(),
            "Should be None when no budget set"
        );

        // Set budget
        router.set_budget(10.0);
        assert!(
            (router.remaining_budget().unwrap() - 10.0).abs() < f64::EPSILON,
            "Should be 10.0 when no cost recorded"
        );

        // Record some cost
        let model = make_model("haiku", ModelTier::Fast, 0.80, 4.0, 0.4);
        router.record_cost(1_000_000, 500_000, &model);
        // Cost: 0.80 + 2.0 = 2.80
        let remaining = router.remaining_budget().unwrap();
        assert!(
            (remaining - 7.2).abs() < 1e-10,
            "Remaining budget should be 7.2, got {remaining}"
        );
    }

    // ── 16. test_route_empty_models_error ────────────────────────────────

    #[test]
    fn test_route_empty_models_error() {
        let router = ModelRouter::new(RoutingStrategy::Balanced);
        let result = router.route("hello", 0, 0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "No models registered in the router");
    }

    // ── 17. test_complexity_factors_weights ──────────────────────────────

    #[test]
    fn test_complexity_factors_weights() {
        let router = make_tiered_router(RoutingStrategy::Balanced);
        let complexity = router.estimate_complexity("analyze this", 5, 10);

        // Should have exactly 7 factors
        assert_eq!(
            complexity.factors.len(),
            7,
            "Should have 7 complexity factors"
        );

        // Verify factor names
        let names: Vec<&str> = complexity.factors.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"task_length"));
        assert!(names.contains(&"tool_count"));
        assert!(names.contains(&"history_length"));
        assert!(names.contains(&"complex_keywords"));
        assert!(names.contains(&"simple_keywords"));
        assert!(names.contains(&"question_count"));
        assert!(names.contains(&"technical_content"));

        // Weights should sum to approximately 0.70 (net, since simple_keywords is negative)
        let weight_sum: f32 = complexity.factors.iter().map(|f| f.weight).sum();
        assert!(
            (weight_sum - 0.70).abs() < 0.01,
            "Weights should sum to ~0.70, got {weight_sum}"
        );
    }

    // ── 18. test_claude_preset ───────────────────────────────────────────

    #[test]
    fn test_claude_preset() {
        let presets = ModelRouter::claude_preset("test-api-key");
        assert_eq!(presets.len(), 3);
        assert_eq!(presets[0].tier, ModelTier::Fast);
        assert_eq!(presets[1].tier, ModelTier::Balanced);
        assert_eq!(presets[2].tier, ModelTier::Powerful);
        assert_eq!(presets[0].config.api_key, "test-api-key");
        assert!(presets[0].max_complexity < presets[1].max_complexity);
        assert!(presets[1].max_complexity < presets[2].max_complexity);
    }

    // ── 19. test_openai_preset ───────────────────────────────────────────

    #[test]
    fn test_openai_preset() {
        let presets = ModelRouter::openai_preset("sk-test");
        assert_eq!(presets.len(), 3);
        assert_eq!(presets[0].tier, ModelTier::Fast);
        assert_eq!(presets[1].tier, ModelTier::Balanced);
        assert_eq!(presets[2].tier, ModelTier::Powerful);
        assert_eq!(presets[2].config.api_key, "sk-test");
    }

    // ── 20. test_cost_optimized_falls_back_to_capable ────────────────────

    #[test]
    fn test_cost_optimized_falls_back_to_capable() {
        let mut router = ModelRouter::new(RoutingStrategy::CostOptimized);
        // Only add a powerful model — even if it's expensive, it should be selected
        router.add_model(make_model("opus", ModelTier::Powerful, 15.0, 75.0, 1.0));

        let decision = router.route("hello", 0, 0).unwrap();
        assert_eq!(decision.selected_model.config.model_id, "opus");
    }

    // ── 21. test_routing_decision_has_estimated_cost ──────────────────────

    #[test]
    fn test_routing_decision_has_estimated_cost() {
        let router = make_tiered_router(RoutingStrategy::CostOptimized);
        let decision = router.route("hello world", 0, 0).unwrap();
        assert!(
            decision.estimated_cost_usd > 0.0,
            "Estimated cost should be positive"
        );
        assert!(!decision.reason.is_empty(), "Reason should not be empty");
    }

    // ── 22. test_remaining_budget_floors_at_zero ─────────────────────────

    #[test]
    fn test_remaining_budget_floors_at_zero() {
        let mut router = make_tiered_router(RoutingStrategy::Balanced);
        router.set_budget(0.01);

        let model = make_model("opus", ModelTier::Powerful, 15.0, 75.0, 1.0);
        router.record_cost(1_000_000, 1_000_000, &model);

        let remaining = router.remaining_budget().unwrap();
        assert!(
            remaining.abs() < f64::EPSILON,
            "Remaining budget should be 0 when overspent, got {remaining}"
        );
    }

    // ── 23. test_tiered_falls_back_when_tier_missing ─────────────────────

    #[test]
    fn test_tiered_falls_back_when_tier_missing() {
        let mut router = ModelRouter::new(RoutingStrategy::Tiered {
            simple_threshold: 0.3,
            complex_threshold: 0.7,
        });
        // Only add a Balanced model — no Fast or Powerful
        router.add_model(make_model("sonnet", ModelTier::Balanced, 3.0, 15.0, 0.8));

        // Simple task targets Fast tier, but should fall back to Balanced
        let decision = router.route("hello", 0, 0).unwrap();
        assert_eq!(decision.selected_model.tier, ModelTier::Balanced);
    }
}

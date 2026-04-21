//! Learning feedback loop for tool selection improvement.
//!
//! Tracks execution outcomes (success/failure, latency, token usage) per tool
//! and per-context, uses exponential moving averages for stats, and detects
//! keyword co-occurrence patterns to improve future tool recommendations.
//!
//! # Key types
//!
//! - [`LearningEngine`] — the main engine that records feedback and produces recommendations.
//! - [`LearningFeedback`] — single feedback record from a tool execution.
//! - [`ToolRecommendation`] — scored recommendation with learned adjustments.
//! - [`LearnedPattern`] — keyword→tool association learned from execution history.
//! - [`LearningReport`] — summary report of the engine's state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the learning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    /// Whether learning is enabled.
    pub enabled: bool,
    /// How fast to adapt stats (0.0–1.0). Higher means faster adaptation.
    pub learning_rate: f32,
    /// Decay factor for older data (0.0–1.0). Applied per feedback record.
    pub decay_factor: f32,
    /// Minimum executions before adjusting recommendations.
    pub min_samples: usize,
    /// Maximum learned patterns kept in cache.
    pub max_patterns: usize,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            learning_rate: 0.1,
            decay_factor: 0.95,
            min_samples: 5,
            max_patterns: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Per-tool learning statistics tracked via exponential moving averages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLearningStats {
    /// Name of the tool.
    pub tool_name: String,
    /// Total number of times the tool was used.
    pub total_uses: u64,
    /// Number of successful invocations.
    pub successes: u64,
    /// Number of failed invocations.
    pub failures: u64,
    /// Exponential moving average of execution time (ms).
    pub avg_execution_time_ms: f64,
    /// Exponential moving average of tokens used.
    pub avg_tokens_used: f64,
    /// Success rate per query context keyword.
    pub context_success_rates: HashMap<String, f32>,
    /// When this stat was last updated.
    pub last_updated: DateTime<Utc>,
    /// Whether performance is improving, stable, or degrading.
    pub trend: Trend,
}

impl ToolLearningStats {
    /// Create new zeroed stats for a tool.
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            total_uses: 0,
            successes: 0,
            failures: 0,
            avg_execution_time_ms: 0.0,
            avg_tokens_used: 0.0,
            context_success_rates: HashMap::new(),
            last_updated: Utc::now(),
            trend: Trend::Stable,
        }
    }

    /// Overall success rate (0.0–1.0).
    pub fn success_rate(&self) -> f32 {
        if self.total_uses == 0 {
            return 0.0;
        }
        self.successes as f32 / self.total_uses as f32
    }
}

/// Performance trend for a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trend {
    /// Success rate is increasing.
    Improving,
    /// Success rate is roughly constant.
    Stable,
    /// Success rate is decreasing.
    Degrading,
}

/// A keyword→tool association learned from execution history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedPattern {
    /// Unique identifier for this pattern.
    pub pattern_id: String,
    /// Keywords in the query context that trigger this pattern.
    pub query_keywords: Vec<String>,
    /// Tools that performed well in this context.
    pub recommended_tools: Vec<String>,
    /// Tools that performed poorly in this context.
    pub avoid_tools: Vec<String>,
    /// Confidence in this pattern (0.0–1.0).
    pub confidence: f32,
    /// How many feedback samples contributed to this pattern.
    pub sample_count: u32,
    /// When this pattern was created.
    pub created_at: DateTime<Utc>,
}

/// A single feedback record from a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningFeedback {
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// The user's query/context that led to this execution.
    pub query_context: String,
    /// Whether the execution was successful.
    pub success: bool,
    /// Execution time in milliseconds.
    pub execution_time_ms: u64,
    /// Number of tokens consumed.
    pub tokens_used: usize,
    /// Error classification, if the execution failed.
    pub error_type: Option<String>,
}

/// A scored tool recommendation with learned adjustments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecommendation {
    /// Name of the recommended tool.
    pub tool_name: String,
    /// Base score from keyword/TF-IDF matching (passed in by caller).
    pub base_score: f32,
    /// Adjustment (positive or negative) from the learning engine.
    pub learned_adjustment: f32,
    /// Final score after adjustment.
    pub final_score: f32,
    /// Explanation of the recommendation.
    pub reasoning: String,
}

/// Summary report of the learning engine's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningReport {
    /// Number of distinct tools being tracked.
    pub total_tools_tracked: usize,
    /// Total feedback records processed since engine creation.
    pub total_feedback_processed: u64,
    /// Number of patterns learned.
    pub patterns_learned: usize,
    /// Top performing tools sorted by success rate (name, rate).
    pub top_performing_tools: Vec<(String, f32)>,
    /// Underperforming tools sorted by success rate ascending (name, rate).
    pub underperforming_tools: Vec<(String, f32)>,
    /// Summary of recent trend across all tools.
    pub recent_trend: String,
}

// ---------------------------------------------------------------------------
// LearningEngine
// ---------------------------------------------------------------------------

/// Engine that tracks tool execution outcomes and improves recommendations
/// over time using exponential moving averages and keyword co-occurrence.
pub struct LearningEngine {
    config: LearningConfig,
    tool_stats: HashMap<String, ToolLearningStats>,
    pattern_cache: Vec<LearnedPattern>,
    total_feedback: u64,
    /// Recent success rates for trend detection (tool → last N rates).
    recent_rates: HashMap<String, Vec<f32>>,
}

impl LearningEngine {
    /// Create a new learning engine with the given configuration.
    pub fn new(config: LearningConfig) -> Self {
        Self {
            config,
            tool_stats: HashMap::new(),
            pattern_cache: Vec::new(),
            total_feedback: 0,
            recent_rates: HashMap::new(),
        }
    }

    /// Create an engine with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(LearningConfig::default())
    }

    /// Record a feedback event from a tool execution.
    ///
    /// Updates exponential moving averages for execution time and token usage,
    /// context-specific success rates, and the tool's performance trend.
    pub fn record_feedback(&mut self, feedback: &LearningFeedback) {
        if !self.config.enabled {
            return;
        }

        self.total_feedback += 1;
        let alpha = self.config.learning_rate as f64;

        let stats = self
            .tool_stats
            .entry(feedback.tool_name.clone())
            .or_insert_with(|| ToolLearningStats::new(&feedback.tool_name));

        stats.total_uses += 1;
        if feedback.success {
            stats.successes += 1;
        } else {
            stats.failures += 1;
        }

        // Exponential moving average for execution time.
        if stats.total_uses == 1 {
            stats.avg_execution_time_ms = feedback.execution_time_ms as f64;
        } else {
            stats.avg_execution_time_ms = alpha * feedback.execution_time_ms as f64
                + (1.0 - alpha) * stats.avg_execution_time_ms;
        }

        // Exponential moving average for token usage.
        if stats.total_uses == 1 {
            stats.avg_tokens_used = feedback.tokens_used as f64;
        } else {
            stats.avg_tokens_used =
                alpha * feedback.tokens_used as f64 + (1.0 - alpha) * stats.avg_tokens_used;
        }

        // Update context success rates using keywords from the query.
        let keywords = Self::extract_keywords(&feedback.query_context);
        let decay = self.config.decay_factor;
        for kw in &keywords {
            let entry = stats.context_success_rates.entry(kw.clone()).or_insert(0.5);
            let outcome = if feedback.success { 1.0 } else { 0.0 };
            *entry = decay * (*entry) + (1.0 - decay) * outcome;
        }

        stats.last_updated = Utc::now();

        // Track recent rates for trend detection.
        let current_rate = stats.success_rate();
        let rates = self
            .recent_rates
            .entry(feedback.tool_name.clone())
            .or_default();
        rates.push(current_rate);
        if rates.len() > 10 {
            rates.remove(0);
        }

        // Update trend — compute inline to avoid borrow conflict.
        let trend = if rates.len() >= 3 {
            let n = rates.len();
            let first_half: f32 = rates[..n / 2].iter().sum::<f32>() / (n / 2) as f32;
            let second_half: f32 = rates[n / 2..].iter().sum::<f32>() / (n - n / 2) as f32;
            let diff = second_half - first_half;
            if diff > 0.05 {
                Trend::Improving
            } else if diff < -0.05 {
                Trend::Degrading
            } else {
                Trend::Stable
            }
        } else {
            Trend::Stable
        };
        stats.trend = trend;
    }

    /// Get tool recommendations adjusted by learned data.
    ///
    /// Takes a list of `(tool_name, base_score)` pairs (e.g., from TF-IDF
    /// matching) and a query context, then adjusts scores using historical
    /// performance data and pattern matches.
    pub fn recommend_tools(
        &self,
        candidates: &[(&str, f32)],
        query_context: &str,
    ) -> Vec<ToolRecommendation> {
        let query_keywords = Self::extract_keywords(query_context);
        let mut recommendations: Vec<ToolRecommendation> = Vec::new();

        for &(tool_name, base_score) in candidates {
            let mut adjustment = 0.0_f32;
            let mut reasons = Vec::new();

            // Adjustment from historical success rate.
            if let Some(stats) = self.tool_stats.get(tool_name) {
                if stats.total_uses as usize >= self.config.min_samples {
                    let rate = stats.success_rate();
                    // Boost high-performing tools, penalize low-performing ones.
                    // Center around 0.5 (neutral).
                    let rate_adj = (rate - 0.5) * 0.3;
                    adjustment += rate_adj;
                    reasons.push(format!("Success rate: {:.0}%", rate * 100.0));

                    // Context-specific adjustment.
                    let context_score = self.context_match_score(stats, &query_keywords);
                    if context_score.abs() > 0.01 {
                        adjustment += context_score * 0.2;
                        reasons.push(format!("Context match: {context_score:.2}"));
                    }

                    // Trend adjustment.
                    match stats.trend {
                        Trend::Improving => {
                            adjustment += 0.05;
                            reasons.push("Trending up".into());
                        }
                        Trend::Degrading => {
                            adjustment -= 0.05;
                            reasons.push("Trending down".into());
                        }
                        Trend::Stable => {}
                    }
                } else {
                    reasons.push(format!(
                        "Insufficient data ({}/{})",
                        stats.total_uses, self.config.min_samples
                    ));
                }
            }

            // Adjustment from learned patterns.
            for pattern in &self.pattern_cache {
                let kw_overlap = query_keywords
                    .iter()
                    .filter(|k| pattern.query_keywords.contains(k))
                    .count();
                if kw_overlap == 0 {
                    continue;
                }

                let relevance = kw_overlap as f32 / pattern.query_keywords.len().max(1) as f32;

                if pattern.recommended_tools.contains(&tool_name.to_string()) {
                    adjustment += relevance * pattern.confidence * 0.15;
                    reasons.push(format!("Pattern match (rec): {}", pattern.pattern_id));
                }
                if pattern.avoid_tools.contains(&tool_name.to_string()) {
                    adjustment -= relevance * pattern.confidence * 0.15;
                    reasons.push(format!("Pattern match (avoid): {}", pattern.pattern_id));
                }
            }

            let final_score = (base_score + adjustment).clamp(0.0, 1.0);
            let reasoning = if reasons.is_empty() {
                "No learned data available.".into()
            } else {
                reasons.join("; ")
            };

            recommendations.push(ToolRecommendation {
                tool_name: tool_name.to_string(),
                base_score,
                learned_adjustment: adjustment,
                final_score,
                reasoning,
            });
        }

        // Sort by final_score descending.
        recommendations.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        recommendations
    }

    /// Get statistics for a specific tool.
    pub fn get_stats(&self, tool_name: &str) -> Option<&ToolLearningStats> {
        self.tool_stats.get(tool_name)
    }

    /// Get statistics for all tracked tools.
    pub fn all_stats(&self) -> &HashMap<String, ToolLearningStats> {
        &self.tool_stats
    }

    /// Generate a summary report of the engine's state.
    pub fn get_report(&self) -> LearningReport {
        let mut tools_by_rate: Vec<(String, f32)> = self
            .tool_stats
            .values()
            .filter(|s| s.total_uses > 0)
            .map(|s| (s.tool_name.clone(), s.success_rate()))
            .collect();

        tools_by_rate.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let top_performing = tools_by_rate.iter().take(5).cloned().collect();

        let underperforming = {
            let mut worst = tools_by_rate.clone();
            worst.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            worst.into_iter().take(5).collect()
        };

        // Overall trend summary.
        let improving = self
            .tool_stats
            .values()
            .filter(|s| s.trend == Trend::Improving)
            .count();
        let degrading = self
            .tool_stats
            .values()
            .filter(|s| s.trend == Trend::Degrading)
            .count();
        let recent_trend = if improving > degrading {
            "Overall improving".to_string()
        } else if degrading > improving {
            "Overall degrading".to_string()
        } else {
            "Overall stable".to_string()
        };

        LearningReport {
            total_tools_tracked: self.tool_stats.len(),
            total_feedback_processed: self.total_feedback,
            patterns_learned: self.pattern_cache.len(),
            top_performing_tools: top_performing,
            underperforming_tools: underperforming,
            recent_trend,
        }
    }

    /// Analyze recorded feedback and extract keyword→tool patterns.
    ///
    /// Looks at context success rates across tools and identifies keywords
    /// that consistently correlate with success or failure for specific tools.
    pub fn learn_patterns(&mut self) {
        if !self.config.enabled {
            return;
        }

        // Collect keyword → [(tool, context_rate)] data.
        let mut keyword_tool_rates: HashMap<String, Vec<(String, f32)>> = HashMap::new();

        for stats in self.tool_stats.values() {
            if (stats.total_uses as usize) < self.config.min_samples {
                continue;
            }
            for (keyword, rate) in &stats.context_success_rates {
                keyword_tool_rates
                    .entry(keyword.clone())
                    .or_default()
                    .push((stats.tool_name.clone(), *rate));
            }
        }

        let mut new_patterns: Vec<LearnedPattern> = Vec::new();

        for (keyword, tool_rates) in &keyword_tool_rates {
            if tool_rates.len() < 2 {
                continue; // Need at least 2 tools for comparison.
            }

            let mut recommended = Vec::new();
            let mut avoid = Vec::new();
            let mut total_samples = 0_u32;

            for (tool_name, rate) in tool_rates {
                if let Some(stats) = self.tool_stats.get(tool_name) {
                    total_samples += stats.total_uses as u32;
                }
                if *rate > 0.7 {
                    recommended.push(tool_name.clone());
                } else if *rate < 0.3 {
                    avoid.push(tool_name.clone());
                }
            }

            if recommended.is_empty() && avoid.is_empty() {
                continue;
            }

            let confidence = if total_samples > 20 {
                0.9
            } else if total_samples > 10 {
                0.7
            } else {
                0.5
            };

            let pattern_id = format!("auto_{keyword}");

            // Check if we already have this pattern and update it.
            if let Some(existing) = new_patterns.iter_mut().find(|p| p.pattern_id == pattern_id) {
                existing.recommended_tools.extend(recommended);
                existing.avoid_tools.extend(avoid);
                existing.sample_count += total_samples;
                existing.confidence = confidence;
            } else {
                new_patterns.push(LearnedPattern {
                    pattern_id,
                    query_keywords: vec![keyword.clone()],
                    recommended_tools: recommended,
                    avoid_tools: avoid,
                    confidence,
                    sample_count: total_samples,
                    created_at: Utc::now(),
                });
            }
        }

        // Replace pattern cache, respecting max_patterns.
        new_patterns.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        new_patterns.truncate(self.config.max_patterns);
        self.pattern_cache = new_patterns;
    }

    /// Serialize the engine state to JSON for persistence.
    pub fn serialize(&self) -> Result<String, String> {
        let state = LearningEngineState {
            tool_stats: self.tool_stats.clone(),
            pattern_cache: self.pattern_cache.clone(),
            total_feedback: self.total_feedback,
            recent_rates: self.recent_rates.clone(),
        };
        serde_json::to_string_pretty(&state).map_err(|e| format!("Serialization failed: {e}"))
    }

    /// Deserialize engine state from JSON.
    pub fn deserialize(&mut self, json: &str) -> Result<(), String> {
        let state: LearningEngineState =
            serde_json::from_str(json).map_err(|e| format!("Deserialization failed: {e}"))?;
        self.tool_stats = state.tool_stats;
        self.pattern_cache = state.pattern_cache;
        self.total_feedback = state.total_feedback;
        self.recent_rates = state.recent_rates;
        Ok(())
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &LearningConfig {
        &self.config
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Extract keywords from a query context (lowercase, > 2 chars, no stopwords).
    fn extract_keywords(text: &str) -> Vec<String> {
        const STOPWORDS: &[&str] = &[
            "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was",
            "one", "our", "out", "has", "have", "from", "with", "they", "been", "this", "that",
            "will", "each", "make", "like", "use", "into",
        ];

        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .filter(|w| !STOPWORDS.contains(w))
            .map(String::from)
            .collect()
    }

    /// Compute a context match score for a tool given query keywords.
    /// Returns a value in [-1.0, 1.0]: positive means good match, negative means poor.
    fn context_match_score(&self, stats: &ToolLearningStats, query_keywords: &[String]) -> f32 {
        if query_keywords.is_empty() || stats.context_success_rates.is_empty() {
            return 0.0;
        }

        let mut sum = 0.0_f32;
        let mut count = 0;

        for kw in query_keywords {
            if let Some(rate) = stats.context_success_rates.get(kw) {
                // Center around 0.5 so positive = good, negative = bad.
                sum += *rate - 0.5;
                count += 1;
            }
        }

        if count == 0 {
            return 0.0;
        }

        (sum / count as f32).clamp(-1.0, 1.0)
    }

    /// Compute the performance trend for a tool from recent success rates.
    #[allow(dead_code)] // kept as public utility; inline version used in record_feedback
    fn compute_trend(&self, tool_name: &str) -> Trend {
        let rates = match self.recent_rates.get(tool_name) {
            Some(r) if r.len() >= 3 => r,
            _ => return Trend::Stable,
        };

        let n = rates.len();
        let first_half: f32 = rates[..n / 2].iter().sum::<f32>() / (n / 2) as f32;
        let second_half: f32 = rates[n / 2..].iter().sum::<f32>() / (n - n / 2) as f32;

        let diff = second_half - first_half;
        if diff > 0.05 {
            Trend::Improving
        } else if diff < -0.05 {
            Trend::Degrading
        } else {
            Trend::Stable
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization helper
// ---------------------------------------------------------------------------

/// Internal state for serialization.
#[derive(Serialize, Deserialize)]
struct LearningEngineState {
    tool_stats: HashMap<String, ToolLearningStats>,
    pattern_cache: Vec<LearnedPattern>,
    total_feedback: u64,
    recent_rates: HashMap<String, Vec<f32>>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn default_engine() -> LearningEngine {
        LearningEngine::with_defaults()
    }

    fn make_feedback(tool: &str, context: &str, success: bool) -> LearningFeedback {
        LearningFeedback {
            tool_name: tool.to_string(),
            query_context: context.to_string(),
            success,
            execution_time_ms: 100,
            tokens_used: 50,
            error_type: if success {
                None
            } else {
                Some("test_error".into())
            },
        }
    }

    fn make_feedback_full(
        tool: &str,
        context: &str,
        success: bool,
        time_ms: u64,
        tokens: usize,
    ) -> LearningFeedback {
        LearningFeedback {
            tool_name: tool.to_string(),
            query_context: context.to_string(),
            success,
            execution_time_ms: time_ms,
            tokens_used: tokens,
            error_type: if success { None } else { Some("error".into()) },
        }
    }

    // -- Config and construction -----------------------------------------------

    #[test]
    fn test_default_config() {
        let config = LearningConfig::default();
        assert!(config.enabled);
        assert!((config.learning_rate - 0.1).abs() < f32::EPSILON);
        assert!((config.decay_factor - 0.95).abs() < f32::EPSILON);
        assert_eq!(config.min_samples, 5);
        assert_eq!(config.max_patterns, 100);
    }

    #[test]
    fn test_new_engine_empty() {
        let engine = default_engine();
        assert!(engine.all_stats().is_empty());
        assert_eq!(engine.get_report().total_tools_tracked, 0);
    }

    // -- Record feedback -------------------------------------------------------

    #[test]
    fn test_record_single_success() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "read file", true));

        let stats = engine.get_stats("tool_a").unwrap();
        assert_eq!(stats.total_uses, 1);
        assert_eq!(stats.successes, 1);
        assert_eq!(stats.failures, 0);
        assert!((stats.success_rate() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_record_single_failure() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "read file", false));

        let stats = engine.get_stats("tool_a").unwrap();
        assert_eq!(stats.total_uses, 1);
        assert_eq!(stats.successes, 0);
        assert_eq!(stats.failures, 1);
        assert!((stats.success_rate() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_record_mixed_feedback() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "read file", true));
        engine.record_feedback(&make_feedback("tool_a", "write file", false));
        engine.record_feedback(&make_feedback("tool_a", "read data", true));

        let stats = engine.get_stats("tool_a").unwrap();
        assert_eq!(stats.total_uses, 3);
        assert_eq!(stats.successes, 2);
        assert_eq!(stats.failures, 1);
        assert!((stats.success_rate() - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_multiple_tools_tracked() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "context", true));
        engine.record_feedback(&make_feedback("tool_b", "context", false));

        assert!(engine.get_stats("tool_a").is_some());
        assert!(engine.get_stats("tool_b").is_some());
        assert!(engine.get_stats("tool_c").is_none());
        assert_eq!(engine.all_stats().len(), 2);
    }

    #[test]
    fn test_disabled_engine_ignores_feedback() {
        let config = LearningConfig {
            enabled: false,
            ..Default::default()
        };
        let mut engine = LearningEngine::new(config);
        engine.record_feedback(&make_feedback("tool_a", "context", true));
        assert!(engine.get_stats("tool_a").is_none());
    }

    // -- EMA tracking ----------------------------------------------------------

    #[test]
    fn test_ema_execution_time() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback_full("tool_a", "ctx", true, 100, 50));
        let stats = engine.get_stats("tool_a").unwrap();
        assert!((stats.avg_execution_time_ms - 100.0).abs() < f64::EPSILON);

        engine.record_feedback(&make_feedback_full("tool_a", "ctx", true, 200, 50));
        let stats = engine.get_stats("tool_a").unwrap();
        // EMA: 0.1 * 200 + 0.9 * 100 = 110
        assert!((stats.avg_execution_time_ms - 110.0).abs() < 1.0);
    }

    #[test]
    fn test_ema_token_usage() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback_full("tool_a", "ctx", true, 100, 1000));
        assert!(
            (engine.get_stats("tool_a").unwrap().avg_tokens_used - 1000.0).abs() < f64::EPSILON
        );

        engine.record_feedback(&make_feedback_full("tool_a", "ctx", true, 100, 500));
        let stats = engine.get_stats("tool_a").unwrap();
        // EMA: 0.1 * 500 + 0.9 * 1000 = 950
        assert!((stats.avg_tokens_used - 950.0).abs() < 1.0);
    }

    // -- Context success rates -------------------------------------------------

    #[test]
    fn test_context_success_rates_updated() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "read file contents", true));
        let stats = engine.get_stats("tool_a").unwrap();
        // Keywords: "read", "file", "contents"
        assert!(stats.context_success_rates.contains_key("read"));
        assert!(stats.context_success_rates.contains_key("file"));
        assert!(stats.context_success_rates.contains_key("contents"));
    }

    #[test]
    fn test_context_rate_increases_on_success() {
        let mut engine = default_engine();
        // Initial: 0.5 baseline
        engine.record_feedback(&make_feedback("tool_a", "read file", true));
        let rate1 = engine.get_stats("tool_a").unwrap().context_success_rates["read"];
        // rate = 0.95 * 0.5 + 0.05 * 1.0 = 0.525
        assert!(rate1 > 0.5);

        engine.record_feedback(&make_feedback("tool_a", "read file", true));
        let rate2 = engine.get_stats("tool_a").unwrap().context_success_rates["read"];
        assert!(rate2 > rate1);
    }

    #[test]
    fn test_context_rate_decreases_on_failure() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "read file", false));
        let rate = engine.get_stats("tool_a").unwrap().context_success_rates["read"];
        // rate = 0.95 * 0.5 + 0.05 * 0.0 = 0.475
        assert!(rate < 0.5);
    }

    // -- Trend detection -------------------------------------------------------

    #[test]
    fn test_trend_stable_initially() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "ctx", true));
        assert_eq!(engine.get_stats("tool_a").unwrap().trend, Trend::Stable);
    }

    #[test]
    fn test_trend_improving() {
        let mut engine = default_engine();
        // First few failures, then successes.
        for _ in 0..3 {
            engine.record_feedback(&make_feedback("tool_a", "ctx", false));
        }
        for _ in 0..5 {
            engine.record_feedback(&make_feedback("tool_a", "ctx", true));
        }
        let trend = &engine.get_stats("tool_a").unwrap().trend;
        // Should detect improvement (second half has higher rate).
        assert!(
            *trend == Trend::Improving || *trend == Trend::Stable,
            "Expected Improving or Stable, got {trend:?}"
        );
    }

    // -- Recommend tools -------------------------------------------------------

    #[test]
    fn test_recommend_no_data() {
        let engine = default_engine();
        let recs = engine.recommend_tools(&[("tool_a", 0.8), ("tool_b", 0.6)], "read file");
        assert_eq!(recs.len(), 2);
        // With no data, adjustments should be 0.
        assert!((recs[0].learned_adjustment - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_recommend_with_data() {
        let mut engine = default_engine();
        // Record enough data for min_samples.
        for _ in 0..10 {
            engine.record_feedback(&make_feedback("tool_a", "read file", true));
        }
        for _ in 0..10 {
            engine.record_feedback(&make_feedback("tool_b", "read file", false));
        }

        let recs = engine.recommend_tools(&[("tool_a", 0.5), ("tool_b", 0.5)], "read file");
        // tool_a should score higher than tool_b.
        let tool_a_rec = recs.iter().find(|r| r.tool_name == "tool_a").unwrap();
        let tool_b_rec = recs.iter().find(|r| r.tool_name == "tool_b").unwrap();
        assert!(tool_a_rec.final_score > tool_b_rec.final_score);
    }

    #[test]
    fn test_recommend_sorted_by_final_score() {
        let engine = default_engine();
        let recs = engine.recommend_tools(
            &[("tool_a", 0.3), ("tool_b", 0.9), ("tool_c", 0.6)],
            "context",
        );
        // Should be sorted descending.
        for i in 1..recs.len() {
            assert!(recs[i - 1].final_score >= recs[i].final_score);
        }
    }

    #[test]
    fn test_recommend_insufficient_samples() {
        let mut engine = default_engine();
        // Only 2 samples (below min_samples of 5).
        engine.record_feedback(&make_feedback("tool_a", "ctx", true));
        engine.record_feedback(&make_feedback("tool_a", "ctx", true));

        let recs = engine.recommend_tools(&[("tool_a", 0.5)], "ctx");
        assert_eq!(recs.len(), 1);
        // Adjustment should be 0 due to insufficient data.
        assert!(
            recs[0].reasoning.contains("Insufficient"),
            "Expected 'Insufficient' in reasoning: {}",
            recs[0].reasoning
        );
    }

    // -- Learn patterns --------------------------------------------------------

    #[test]
    fn test_learn_patterns_basic() {
        let mut engine = default_engine();

        // Build enough data for pattern learning.
        for _ in 0..10 {
            engine.record_feedback(&make_feedback("tool_a", "read file data", true));
            engine.record_feedback(&make_feedback("tool_b", "read file data", false));
        }

        engine.learn_patterns();
        // Should have at least one pattern.
        assert!(
            !engine.pattern_cache.is_empty(),
            "Should have learned at least one pattern"
        );
    }

    #[test]
    fn test_learn_patterns_respects_max() {
        let config = LearningConfig {
            max_patterns: 2,
            ..Default::default()
        };
        let mut engine = LearningEngine::new(config);

        // Generate data across many keywords.
        for i in 0..20 {
            let keyword = format!("keyword_{i}");
            for _ in 0..6 {
                engine.record_feedback(&LearningFeedback {
                    tool_name: "tool_a".into(),
                    query_context: keyword.clone(),
                    success: true,
                    execution_time_ms: 100,
                    tokens_used: 50,
                    error_type: None,
                });
                engine.record_feedback(&LearningFeedback {
                    tool_name: "tool_b".into(),
                    query_context: keyword.clone(),
                    success: false,
                    execution_time_ms: 100,
                    tokens_used: 50,
                    error_type: Some("err".into()),
                });
            }
        }

        engine.learn_patterns();
        assert!(engine.pattern_cache.len() <= 2);
    }

    #[test]
    fn test_disabled_engine_skips_pattern_learning() {
        let config = LearningConfig {
            enabled: false,
            ..Default::default()
        };
        let mut engine = LearningEngine::new(config);
        engine.learn_patterns();
        assert!(engine.pattern_cache.is_empty());
    }

    // -- Report ----------------------------------------------------------------

    #[test]
    fn test_report_empty_engine() {
        let engine = default_engine();
        let report = engine.get_report();
        assert_eq!(report.total_tools_tracked, 0);
        assert_eq!(report.total_feedback_processed, 0);
        assert_eq!(report.patterns_learned, 0);
    }

    #[test]
    fn test_report_with_data() {
        let mut engine = default_engine();
        for _ in 0..5 {
            engine.record_feedback(&make_feedback("tool_a", "ctx", true));
        }
        for _ in 0..5 {
            engine.record_feedback(&make_feedback("tool_b", "ctx", false));
        }

        let report = engine.get_report();
        assert_eq!(report.total_tools_tracked, 2);
        assert_eq!(report.total_feedback_processed, 10);
        // tool_a should be in top performing.
        assert!(report
            .top_performing_tools
            .iter()
            .any(|(name, _)| name == "tool_a"));
    }

    #[test]
    fn test_report_trend_summary() {
        let engine = default_engine();
        let report = engine.get_report();
        // With no data, should be stable.
        assert!(report.recent_trend.contains("stable"));
    }

    // -- Serialization ---------------------------------------------------------

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut engine = default_engine();
        engine.record_feedback(&make_feedback("tool_a", "read file", true));
        engine.record_feedback(&make_feedback("tool_b", "write file", false));

        let json = engine.serialize().unwrap();
        let mut engine2 = default_engine();
        engine2.deserialize(&json).unwrap();

        assert_eq!(engine2.all_stats().len(), 2);
        assert!(engine2.get_stats("tool_a").is_some());
        assert!(engine2.get_stats("tool_b").is_some());
        assert_eq!(engine2.total_feedback, 2);
    }

    #[test]
    fn test_serialize_empty_engine() {
        let engine = default_engine();
        let json = engine.serialize().unwrap();
        assert!(json.contains("tool_stats"));
    }

    // -- Keyword extraction ----------------------------------------------------

    #[test]
    fn test_extract_keywords_basic() {
        let kws = LearningEngine::extract_keywords("Read the file from disk");
        assert!(kws.contains(&"read".to_string()));
        assert!(kws.contains(&"file".to_string()));
        assert!(kws.contains(&"disk".to_string()));
        // "the" and "from" are stopwords.
        assert!(!kws.contains(&"the".to_string()));
        assert!(!kws.contains(&"from".to_string()));
    }

    #[test]
    fn test_extract_keywords_empty() {
        let kws = LearningEngine::extract_keywords("");
        assert!(kws.is_empty());
    }

    #[test]
    fn test_extract_keywords_short_words_filtered() {
        let kws = LearningEngine::extract_keywords("a b cd ef ghi");
        // "a", "b", "cd", "ef" are <= 2 chars.
        assert!(!kws.contains(&"a".to_string()));
        assert!(!kws.contains(&"cd".to_string()));
        assert!(kws.contains(&"ghi".to_string()));
    }

    // -- Context match score ---------------------------------------------------

    #[test]
    fn test_context_match_score_no_overlap() {
        let engine = default_engine();
        let stats = ToolLearningStats::new("tool_a");
        let score = engine.context_match_score(&stats, &["read".to_string()]);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_context_match_score_positive() {
        let engine = default_engine();
        let mut stats = ToolLearningStats::new("tool_a");
        stats.context_success_rates.insert("read".into(), 0.9);
        let score = engine.context_match_score(&stats, &["read".to_string()]);
        assert!(score > 0.0);
    }

    // -- Config accessor -------------------------------------------------------

    #[test]
    fn test_config_accessor() {
        let engine = default_engine();
        assert!(engine.config().enabled);
        assert_eq!(engine.config().min_samples, 5);
    }

    // -- Recommend with patterns -----------------------------------------------

    #[test]
    fn test_recommend_with_pattern_boost() {
        let mut engine = default_engine();

        // Manually add a pattern.
        engine.pattern_cache.push(LearnedPattern {
            pattern_id: "test_pattern".into(),
            query_keywords: vec!["file".into(), "read".into()],
            recommended_tools: vec!["file_reader".into()],
            avoid_tools: vec!["http_fetch".into()],
            confidence: 0.9,
            sample_count: 100,
            created_at: Utc::now(),
        });

        let recs = engine.recommend_tools(
            &[("file_reader", 0.5), ("http_fetch", 0.5)],
            "read file contents",
        );

        let fr = recs.iter().find(|r| r.tool_name == "file_reader").unwrap();
        let hf = recs.iter().find(|r| r.tool_name == "http_fetch").unwrap();
        assert!(fr.final_score > hf.final_score);
    }
}

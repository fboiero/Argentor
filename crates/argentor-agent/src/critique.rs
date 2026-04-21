//! Self-Critique Loop based on the Reflexion pattern.
//!
//! After generating a response, the agent reviews it across multiple quality
//! dimensions and potentially revises it. The critique engine uses heuristic
//! analysis (keyword matching, length checks, tool usage analysis) similar to
//! the evaluator module, but focused on iterative self-improvement.
//!
//! # Architecture
//!
//! ```text
//! ┌────────────┐     ┌─────────────────┐     ┌─────────────────┐
//! │  Response   │ --> │  CritiqueEngine │ --> │ CritiqueResult  │
//! │  + Query    │     │  (multi-dim.)   │     │ (score + revise)│
//! └────────────┘     └─────────────────┘     └─────────────────┘
//!                          │
//!                   ┌──────┴──────────┐
//!                   │  Dimensions:    │
//!                   │  - Accuracy     │
//!                   │  - Completeness │
//!                   │  - Safety       │
//!                   │  - Relevance    │
//!                   │  - Clarity      │
//!                   │  - ToolUsage    │
//!                   └─────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Quality dimensions along which a response can be critiqued.
///
/// Each dimension is scored independently, and the final score is a
/// weighted combination.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CritiqueDimension {
    /// Factual correctness — does the response contain unsupported claims?
    Accuracy,
    /// Completeness — does it address all parts of the question?
    Completeness,
    /// Safety — no harmful, offensive, or dangerous content.
    Safety,
    /// Relevance — stays on topic and addresses the actual question.
    Relevance,
    /// Clarity — clear, well-structured, and easy to understand.
    Clarity,
    /// ToolUsage — appropriate and effective use of available tools.
    ToolUsage,
}

impl CritiqueDimension {
    /// Return the default weight for this dimension in overall scoring.
    pub fn default_weight(&self) -> f32 {
        match self {
            CritiqueDimension::Accuracy => 0.25,
            CritiqueDimension::Completeness => 0.20,
            CritiqueDimension::Safety => 0.20,
            CritiqueDimension::Relevance => 0.15,
            CritiqueDimension::Clarity => 0.10,
            CritiqueDimension::ToolUsage => 0.10,
        }
    }

    /// Return all available dimensions.
    pub fn all() -> Vec<CritiqueDimension> {
        vec![
            CritiqueDimension::Accuracy,
            CritiqueDimension::Completeness,
            CritiqueDimension::Safety,
            CritiqueDimension::Relevance,
            CritiqueDimension::Clarity,
            CritiqueDimension::ToolUsage,
        ]
    }
}

/// A single critique along one dimension.
///
/// Contains the score, textual feedback, and an optional suggestion for
/// how to improve the response along this dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Critique {
    /// The dimension being evaluated.
    pub dimension: CritiqueDimension,
    /// Score for this dimension (0.0 - 1.0).
    pub score: f32,
    /// Human-readable feedback about this dimension.
    pub feedback: String,
    /// Optional specific suggestion for improvement.
    pub suggestion: Option<String>,
}

/// The result of a critique session.
///
/// Contains the original response, all per-dimension critiques, an optional
/// revised response, the number of revisions applied, and the final score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueResult {
    /// The original response that was critiqued.
    pub original_response: String,
    /// Per-dimension critiques.
    pub critiques: Vec<Critique>,
    /// Revised response, if auto-fix was applied.
    pub revised_response: Option<String>,
    /// Number of revision iterations performed.
    pub revision_count: u32,
    /// Final weighted score (0.0 - 1.0).
    pub final_score: f32,
    /// Whether the response was accepted (meets quality threshold).
    pub accepted: bool,
}

/// Configuration for the critique engine.
///
/// Controls which dimensions to evaluate, quality thresholds, revision limits,
/// and whether to attempt automatic fixes.
#[derive(Debug, Clone)]
pub struct CritiqueConfig {
    /// Whether the critique engine is enabled.
    pub enabled: bool,
    /// Maximum number of revision iterations (default: 2).
    pub max_revisions: u32,
    /// Minimum weighted score to accept the response (default: 0.7).
    pub quality_threshold: f32,
    /// Which dimensions to evaluate.
    pub critique_dimensions: Vec<CritiqueDimension>,
    /// Whether to automatically apply heuristic fixes.
    pub auto_fix: bool,
}

impl Default for CritiqueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_revisions: 2,
            quality_threshold: 0.7,
            critique_dimensions: CritiqueDimension::all(),
            auto_fix: false,
        }
    }
}

/// The self-critique engine.
///
/// Evaluates agent responses across configurable quality dimensions using
/// heuristic analysis. Supports iterative revision where each round
/// re-evaluates and optionally applies fixes.
pub struct CritiqueEngine {
    config: CritiqueConfig,
}

impl CritiqueEngine {
    /// Create a new critique engine with the given configuration.
    pub fn new(config: CritiqueConfig) -> Self {
        Self { config }
    }

    /// Create a new critique engine with default configuration.
    pub fn with_defaults() -> Self {
        Self {
            config: CritiqueConfig::default(),
        }
    }

    /// Return a reference to the engine's configuration.
    pub fn config(&self) -> &CritiqueConfig {
        &self.config
    }

    /// Critique a response against the original query.
    ///
    /// Evaluates the response across all configured dimensions, computes a
    /// weighted score, and optionally applies revisions if `auto_fix` is
    /// enabled and the score falls below the quality threshold.
    ///
    /// # Arguments
    ///
    /// * `query` — The original user query.
    /// * `response` — The agent's response to critique.
    /// * `tool_names_used` — Names of tools that were invoked during response generation.
    ///
    /// Returns `None` if the engine is disabled.
    pub fn critique(
        &self,
        query: &str,
        response: &str,
        tool_names_used: &[&str],
    ) -> Option<CritiqueResult> {
        if !self.config.enabled {
            return None;
        }

        let mut current_response = response.to_string();
        let mut revision_count = 0_u32;

        loop {
            let critiques = self.evaluate_dimensions(query, &current_response, tool_names_used);
            let final_score = self.compute_weighted_score(&critiques);
            let accepted = final_score >= self.config.quality_threshold;

            if accepted || revision_count >= self.config.max_revisions || !self.config.auto_fix {
                let revised = if current_response != response {
                    Some(current_response)
                } else {
                    None
                };
                return Some(CritiqueResult {
                    original_response: response.to_string(),
                    critiques,
                    revised_response: revised,
                    revision_count,
                    final_score,
                    accepted,
                });
            }

            // Attempt auto-fix based on lowest-scoring dimensions
            current_response = self.apply_heuristic_fixes(&current_response, &critiques);
            revision_count += 1;
        }
    }

    /// Build a critique prompt for LLM-based evaluation.
    ///
    /// Generates a structured prompt asking the LLM to evaluate its own
    /// response across the configured dimensions.
    pub fn build_critique_prompt(&self, query: &str, response: &str) -> String {
        let dimensions: Vec<String> = self
            .config
            .critique_dimensions
            .iter()
            .enumerate()
            .map(|(i, d)| format!("{}. {:?}: Score 0.0-1.0 with feedback", i + 1, d))
            .collect();

        format!(
            "Critique the following response to the user's query.\n\n\
             Query: {query}\n\n\
             Response: {response}\n\n\
             Evaluate on these dimensions:\n{}\n\n\
             For each dimension, provide:\n\
             - score: 0.0 to 1.0\n\
             - feedback: what's good or bad\n\
             - suggestion: how to improve (if needed)\n\n\
             Respond with JSON array of critiques.",
            dimensions.join("\n")
        )
    }

    /// Build a revision prompt incorporating critique feedback.
    ///
    /// Used when the critique score falls below the threshold to ask the LLM
    /// to revise its response based on the specific feedback provided.
    pub fn build_revision_prompt(
        &self,
        query: &str,
        response: &str,
        critiques: &[Critique],
    ) -> String {
        let feedback_lines: Vec<String> = critiques
            .iter()
            .filter(|c| c.score < self.config.quality_threshold)
            .map(|c| {
                format!(
                    "- {:?} ({:.0}%): {}{}",
                    c.dimension,
                    c.score * 100.0,
                    c.feedback,
                    c.suggestion
                        .as_ref()
                        .map(|s| format!(" Suggestion: {s}"))
                        .unwrap_or_default()
                )
            })
            .collect();

        format!(
            "Your previous response needs improvement.\n\n\
             Original query: {query}\n\n\
             Your response: {response}\n\n\
             Critique feedback:\n{}\n\n\
             Please provide a revised response addressing the feedback above.",
            feedback_lines.join("\n")
        )
    }

    // --- Internal evaluation functions ---

    /// Evaluate all configured dimensions.
    fn evaluate_dimensions(
        &self,
        query: &str,
        response: &str,
        tool_names_used: &[&str],
    ) -> Vec<Critique> {
        self.config
            .critique_dimensions
            .iter()
            .map(|dim| match dim {
                CritiqueDimension::Accuracy => self.evaluate_accuracy(response),
                CritiqueDimension::Completeness => self.evaluate_completeness(query, response),
                CritiqueDimension::Safety => self.evaluate_safety(response),
                CritiqueDimension::Relevance => self.evaluate_relevance(query, response),
                CritiqueDimension::Clarity => self.evaluate_clarity(response),
                CritiqueDimension::ToolUsage => {
                    self.evaluate_tool_usage(query, response, tool_names_used)
                }
            })
            .collect()
    }

    /// Compute the weighted score from per-dimension critiques.
    fn compute_weighted_score(&self, critiques: &[Critique]) -> f32 {
        if critiques.is_empty() {
            return 0.0;
        }

        let total_weight: f32 = critiques.iter().map(|c| c.dimension.default_weight()).sum();
        if total_weight < f32::EPSILON {
            return 0.0;
        }

        let weighted_sum: f32 = critiques
            .iter()
            .map(|c| c.score * c.dimension.default_weight())
            .sum();

        weighted_sum / total_weight
    }

    /// Evaluate accuracy using heuristic indicators.
    ///
    /// Checks for hedging language, unsupported absolute claims, and
    /// contradiction indicators.
    fn evaluate_accuracy(&self, response: &str) -> Critique {
        let lower = response.to_lowercase();
        let mut score = 0.8_f32; // Start optimistic
        let mut issues = Vec::new();

        // Penalize excessive hedging (suggests uncertainty)
        let hedge_words = [
            "maybe",
            "perhaps",
            "might",
            "possibly",
            "i think",
            "i believe",
        ];
        let hedge_count = hedge_words.iter().filter(|w| lower.contains(**w)).count();
        if hedge_count > 2 {
            score -= 0.2;
            issues.push("Excessive hedging suggests uncertainty");
        }

        // Penalize unsupported absolute claims
        let absolute_words = [
            "always",
            "never",
            "definitely",
            "certainly",
            "guaranteed",
            "impossible",
        ];
        let absolute_count = absolute_words
            .iter()
            .filter(|w| lower.contains(**w))
            .count();
        if absolute_count > 1 {
            score -= 0.15;
            issues.push("Multiple absolute claims without evidence");
        }

        // Check for contradiction indicators
        if lower.contains("however") && lower.contains("but") && lower.contains("although") {
            score -= 0.1;
            issues.push("Multiple contradicting qualifiers");
        }

        let feedback = if issues.is_empty() {
            "Response appears factually grounded".to_string()
        } else {
            format!("Accuracy concerns: {}", issues.join("; "))
        };

        let suggestion = if score < 0.7 {
            Some("Reduce hedging and support claims with evidence or tool results".to_string())
        } else {
            None
        };

        Critique {
            dimension: CritiqueDimension::Accuracy,
            score: score.clamp(0.0, 1.0),
            feedback,
            suggestion,
        }
    }

    /// Evaluate completeness by checking if the response addresses key parts of the query.
    fn evaluate_completeness(&self, query: &str, response: &str) -> Critique {
        let query_keywords = extract_significant_words(query);
        let response_lower = response.to_lowercase();

        if query_keywords.is_empty() {
            return Critique {
                dimension: CritiqueDimension::Completeness,
                score: 0.8,
                feedback: "Query too short to assess completeness".to_string(),
                suggestion: None,
            };
        }

        // Check how many query keywords appear in the response
        let covered = query_keywords
            .iter()
            .filter(|kw| response_lower.contains(kw.as_str()))
            .count();
        let coverage_ratio = covered as f32 / query_keywords.len() as f32;

        // Length-based bonus
        let length_score = if response.len() < 20 {
            0.2
        } else if response.len() < 50 {
            0.4
        } else if response.len() < 100 {
            0.6
        } else if response.len() < 200 {
            0.8
        } else {
            1.0
        };

        // Question count in query vs. answer coverage
        let question_count = query.matches('?').count();
        let paragraph_count = response.split("\n\n").count();
        let q_coverage = if question_count > 0 {
            (paragraph_count as f32 / question_count as f32).min(1.0)
        } else {
            1.0
        };

        let score = (coverage_ratio * 0.5 + length_score * 0.3 + q_coverage * 0.2).min(1.0);

        let missing: Vec<&String> = query_keywords
            .iter()
            .filter(|kw| !response_lower.contains(kw.as_str()))
            .collect();

        let feedback = if missing.is_empty() {
            "Response addresses all key aspects of the query".to_string()
        } else {
            format!(
                "Response may miss topics related to: {}",
                missing
                    .iter()
                    .take(5)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        let suggestion = if score < 0.7 {
            Some(format!(
                "Address these missing aspects: {}",
                missing
                    .iter()
                    .take(3)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        } else {
            None
        };

        Critique {
            dimension: CritiqueDimension::Completeness,
            score,
            feedback,
            suggestion,
        }
    }

    /// Evaluate safety by checking for harmful content patterns.
    fn evaluate_safety(&self, response: &str) -> Critique {
        let lower = response.to_lowercase();
        let mut score = 1.0_f32;
        let mut issues = Vec::new();

        // Check for potentially harmful instruction patterns
        let harmful_patterns = [
            "hack into",
            "exploit vulnerability",
            "bypass security",
            "steal password",
            "malicious code",
            "illegal access",
            "without permission",
            "sql injection",
            "cross-site scripting",
        ];

        for pattern in &harmful_patterns {
            if lower.contains(pattern) {
                score -= 0.3;
                issues.push(format!("Contains potentially harmful content: '{pattern}'"));
            }
        }

        // Check for PII exposure patterns
        let pii_patterns = [
            "social security",
            "credit card number",
            "password is",
            "secret key",
            "api key:",
            "private key",
        ];
        for pattern in &pii_patterns {
            if lower.contains(pattern) {
                score -= 0.2;
                issues.push(format!("May expose sensitive information: '{pattern}'"));
            }
        }

        let feedback = if issues.is_empty() {
            "Response appears safe and appropriate".to_string()
        } else {
            format!("Safety concerns: {}", issues.join("; "))
        };

        let suggestion = if score < 0.7 {
            Some("Remove or rephrase harmful or sensitive content".to_string())
        } else {
            None
        };

        Critique {
            dimension: CritiqueDimension::Safety,
            score: score.clamp(0.0, 1.0),
            feedback,
            suggestion,
        }
    }

    /// Evaluate relevance by checking keyword overlap between query and response.
    fn evaluate_relevance(&self, query: &str, response: &str) -> Critique {
        let query_words = extract_significant_words(query);
        let response_lower = response.to_lowercase();

        if query_words.is_empty() {
            return Critique {
                dimension: CritiqueDimension::Relevance,
                score: 0.5,
                feedback: "Query too short to assess relevance".to_string(),
                suggestion: None,
            };
        }

        let overlap = query_words
            .iter()
            .filter(|w| response_lower.contains(w.as_str()))
            .count();
        let ratio = overlap as f32 / query_words.len() as f32;
        let score = (ratio * 1.3).min(1.0); // Scale up slightly

        let feedback = if score >= 0.7 {
            "Response is well-aligned with the query".to_string()
        } else if score >= 0.4 {
            "Response partially addresses the query".to_string()
        } else {
            "Response may be off-topic".to_string()
        };

        let suggestion = if score < 0.5 {
            Some("Refocus the response to directly address the user's question".to_string())
        } else {
            None
        };

        Critique {
            dimension: CritiqueDimension::Relevance,
            score,
            feedback,
            suggestion,
        }
    }

    /// Evaluate clarity based on structural quality.
    fn evaluate_clarity(&self, response: &str) -> Critique {
        let mut score = 0.7_f32;
        let mut issues = Vec::new();

        // Check for structure (paragraphs, lists, code blocks)
        let has_paragraphs = response.contains("\n\n");
        let has_lists = response.contains("\n- ") || response.contains("\n* ");
        let has_code = response.contains("```");

        if has_paragraphs || has_lists || has_code {
            score += 0.15;
        }

        // Check for overly long sentences (no period for a long stretch)
        let sentences: Vec<&str> = response.split('.').collect();
        let long_sentences = sentences.iter().filter(|s| s.len() > 200).count();
        if long_sentences > 2 {
            score -= 0.15;
            issues.push("Contains overly long sentences");
        }

        // Check for very short responses
        if response.len() < 20 {
            score -= 0.2;
            issues.push("Response is very short");
        }

        // Check for repetitive content
        let words: Vec<&str> = response.split_whitespace().collect();
        if words.len() > 10 {
            let unique: HashSet<&&str> = words.iter().collect();
            let uniqueness = unique.len() as f32 / words.len() as f32;
            if uniqueness < 0.4 {
                score -= 0.2;
                issues.push("Response contains repetitive content");
            }
        }

        let feedback = if issues.is_empty() {
            "Response is clear and well-structured".to_string()
        } else {
            format!("Clarity issues: {}", issues.join("; "))
        };

        let suggestion = if score < 0.6 {
            Some("Improve structure with paragraphs, lists, or clearer sentences".to_string())
        } else {
            None
        };

        Critique {
            dimension: CritiqueDimension::Clarity,
            score: score.clamp(0.0, 1.0),
            feedback,
            suggestion,
        }
    }

    /// Evaluate tool usage appropriateness.
    fn evaluate_tool_usage(
        &self,
        query: &str,
        response: &str,
        tool_names_used: &[&str],
    ) -> Critique {
        let lower_query = query.to_lowercase();
        let lower_response = response.to_lowercase();

        // If no tools were used, check if tools should have been
        if tool_names_used.is_empty() {
            let needs_tools = lower_query.contains("file")
                || lower_query.contains("search")
                || lower_query.contains("run")
                || lower_query.contains("execute")
                || lower_query.contains("fetch")
                || lower_query.contains("look up");

            if needs_tools {
                return Critique {
                    dimension: CritiqueDimension::ToolUsage,
                    score: 0.3,
                    feedback: "No tools were used, but the query suggests tool usage would help"
                        .to_string(),
                    suggestion: Some(
                        "Consider using relevant tools to provide a more accurate response"
                            .to_string(),
                    ),
                };
            }

            return Critique {
                dimension: CritiqueDimension::ToolUsage,
                score: 0.8,
                feedback: "No tools needed for this type of query".to_string(),
                suggestion: None,
            };
        }

        // Check if tool results are referenced in the response
        let references_tools = tool_names_used
            .iter()
            .any(|t| lower_response.contains(&t.to_lowercase()));

        let score = if references_tools { 0.9 } else { 0.6 };

        let feedback = if references_tools {
            format!("Good use of tools: {}", tool_names_used.join(", "))
        } else {
            "Tools were used but their results may not be well-integrated into the response"
                .to_string()
        };

        let suggestion = if score < 0.7 {
            Some("Reference tool results explicitly in the response".to_string())
        } else {
            None
        };

        Critique {
            dimension: CritiqueDimension::ToolUsage,
            score,
            feedback,
            suggestion,
        }
    }

    /// Apply heuristic fixes based on critique feedback.
    ///
    /// This is a best-effort improvement: it can trim harmful content,
    /// add structure, or append missing topic mentions. For real revisions,
    /// use `build_revision_prompt` with an LLM.
    fn apply_heuristic_fixes(&self, response: &str, critiques: &[Critique]) -> String {
        let mut fixed = response.to_string();

        for critique in critiques {
            if critique.score >= self.config.quality_threshold {
                continue;
            }

            match critique.dimension {
                CritiqueDimension::Clarity => {
                    // Add paragraph breaks if response is one long block
                    if !fixed.contains("\n\n") && fixed.len() > 200 {
                        let mid = fixed.len() / 2;
                        // Find nearest sentence boundary
                        if let Some(pos) = fixed[mid..].find(". ") {
                            let break_point = mid + pos + 2;
                            fixed.insert_str(break_point, "\n\n");
                        }
                    }
                }
                CritiqueDimension::Safety => {
                    // Remove lines containing harmful patterns (aggressive but safe)
                    let harmful = [
                        "hack into",
                        "exploit vulnerability",
                        "bypass security",
                        "steal password",
                    ];
                    for pattern in &harmful {
                        if fixed.to_lowercase().contains(pattern) {
                            fixed = fixed
                                .lines()
                                .filter(|l| !l.to_lowercase().contains(pattern))
                                .collect::<Vec<_>>()
                                .join("\n");
                        }
                    }
                }
                _ => {
                    // Other dimensions are better handled by LLM revision
                }
            }
        }

        fixed
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract significant words from text (>3 chars, no stopwords).
fn extract_significant_words(text: &str) -> Vec<String> {
    let stopwords: HashSet<&str> = [
        "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "was", "one", "our",
        "out", "has", "have", "from", "with", "they", "been", "this", "that", "will", "each",
        "make", "like", "use", "into", "what", "how", "does", "just", "please", "could", "would",
        "should", "about", "when", "then", "than",
    ]
    .into_iter()
    .collect();

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3)
        .filter(|w| !stopwords.contains(w))
        .map(String::from)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // CritiqueDimension tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_dimension_default_weights_sum_to_one() {
        let total: f32 = CritiqueDimension::all()
            .iter()
            .map(|d| d.default_weight())
            .sum();
        assert!(
            (total - 1.0).abs() < f32::EPSILON,
            "Default weights should sum to 1.0, got {total}"
        );
    }

    #[test]
    fn test_dimension_all_returns_six() {
        assert_eq!(CritiqueDimension::all().len(), 6);
    }

    #[test]
    fn test_dimension_serde_roundtrip() {
        for dim in CritiqueDimension::all() {
            let json = serde_json::to_string(&dim).unwrap();
            let parsed: CritiqueDimension = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, dim);
        }
    }

    // -----------------------------------------------------------------------
    // CritiqueConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_defaults() {
        let config = CritiqueConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_revisions, 2);
        assert!((config.quality_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(config.critique_dimensions.len(), 6);
        assert!(!config.auto_fix);
    }

    // -----------------------------------------------------------------------
    // CritiqueEngine basic tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_engine_with_defaults() {
        let engine = CritiqueEngine::with_defaults();
        assert!(engine.config().enabled);
    }

    #[test]
    fn test_critique_disabled_returns_none() {
        let engine = CritiqueEngine::new(CritiqueConfig {
            enabled: false,
            ..CritiqueConfig::default()
        });
        let result = engine.critique("test query", "test response", &[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_critique_returns_result_when_enabled() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is Rust programming language?",
                "Rust is a systems programming language focused on safety, speed, and concurrency. \
                 It achieves memory safety without garbage collection through its ownership system.",
                &[],
            )
            .unwrap();
        assert!(!result.critiques.is_empty());
        assert!(result.final_score > 0.0);
        assert!(result.final_score <= 1.0);
    }

    // -----------------------------------------------------------------------
    // Accuracy evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_accuracy_penalizes_excessive_hedging() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is Rust?",
                "Maybe Rust is perhaps a language. I think it might possibly be good. I believe it could be fast.",
                &[],
            )
            .unwrap();
        let accuracy = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Accuracy)
            .unwrap();
        assert!(
            accuracy.score < 0.8,
            "Hedging should lower accuracy score, got {}",
            accuracy.score
        );
    }

    #[test]
    fn test_accuracy_penalizes_absolute_claims() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "Tell me about programming",
                "Rust is always the best choice. It is never slow. It is definitely impossible to write bugs in Rust.",
                &[],
            )
            .unwrap();
        let accuracy = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Accuracy)
            .unwrap();
        assert!(
            accuracy.score < 0.8,
            "Absolute claims should lower accuracy score, got {}",
            accuracy.score
        );
    }

    #[test]
    fn test_accuracy_good_response() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is Rust?",
                "Rust is a systems programming language designed for performance and safety. \
                 It uses an ownership system to manage memory without a garbage collector.",
                &[],
            )
            .unwrap();
        let accuracy = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Accuracy)
            .unwrap();
        assert!(
            accuracy.score >= 0.7,
            "Good response should have high accuracy, got {}",
            accuracy.score
        );
    }

    // -----------------------------------------------------------------------
    // Safety evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_safety_flags_harmful_content() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "How to test security?",
                "You can hack into the system by exploiting vulnerability in the login. Then bypass security using sql injection.",
                &[],
            )
            .unwrap();
        let safety = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Safety)
            .unwrap();
        assert!(
            safety.score < 0.5,
            "Harmful content should score low on safety, got {}",
            safety.score
        );
        assert!(safety.suggestion.is_some());
    }

    #[test]
    fn test_safety_clean_response() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is testing?",
                "Testing is the process of verifying that software works correctly. \
                 It includes unit tests, integration tests, and end-to-end tests.",
                &[],
            )
            .unwrap();
        let safety = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Safety)
            .unwrap();
        assert!(
            (safety.score - 1.0).abs() < f32::EPSILON,
            "Clean response should have perfect safety score, got {}",
            safety.score
        );
    }

    // -----------------------------------------------------------------------
    // Completeness evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_completeness_short_response() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is Rust and why is it popular?",
                "Rust is a language.",
                &[],
            )
            .unwrap();
        let completeness = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Completeness)
            .unwrap();
        assert!(
            completeness.score < 0.7,
            "Short response should score low on completeness, got {}",
            completeness.score
        );
    }

    #[test]
    fn test_completeness_thorough_response() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is Rust?",
                "Rust is a systems programming language that focuses on safety, speed, and concurrency. \
                 It was created by Graydon Hoare and first appeared in 2010. Rust achieves memory safety \
                 without garbage collection through its ownership system. The language has been consistently \
                 voted as the most loved programming language in Stack Overflow surveys.",
                &[],
            )
            .unwrap();
        let completeness = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Completeness)
            .unwrap();
        assert!(
            completeness.score >= 0.6,
            "Thorough response should score well on completeness, got {}",
            completeness.score
        );
    }

    // -----------------------------------------------------------------------
    // Relevance evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_relevance_on_topic() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "How does Rust handle memory safety?",
                "Rust handles memory safety through its ownership system. Each value has a single owner, \
                 and when the owner goes out of scope, the value is dropped. Borrowing rules ensure \
                 references are always valid.",
                &[],
            )
            .unwrap();
        let relevance = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Relevance)
            .unwrap();
        assert!(
            relevance.score >= 0.5,
            "On-topic response should score high on relevance, got {}",
            relevance.score
        );
    }

    #[test]
    fn test_relevance_off_topic() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "How does Rust handle memory safety?",
                "Python is a great language for beginners. It has simple syntax and a large community. \
                 Django and Flask are popular web frameworks for Python development.",
                &[],
            )
            .unwrap();
        let relevance = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Relevance)
            .unwrap();
        assert!(
            relevance.score < 0.7,
            "Off-topic response should score low on relevance, got {}",
            relevance.score
        );
    }

    // -----------------------------------------------------------------------
    // Clarity evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_clarity_well_structured() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "Explain ownership in Rust",
                "Ownership in Rust has three rules:\n\n\
                 - Each value has a single owner\n\
                 - There can only be one owner at a time\n\
                 - When the owner goes out of scope, the value is dropped\n\n\
                 This system eliminates the need for garbage collection.",
                &[],
            )
            .unwrap();
        let clarity = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Clarity)
            .unwrap();
        assert!(
            clarity.score >= 0.7,
            "Well-structured response should score high on clarity, got {}",
            clarity.score
        );
    }

    #[test]
    fn test_clarity_very_short() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine.critique("Explain Rust", "It's good.", &[]).unwrap();
        let clarity = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::Clarity)
            .unwrap();
        assert!(
            clarity.score < 0.7,
            "Very short response should score low on clarity, got {}",
            clarity.score
        );
    }

    // -----------------------------------------------------------------------
    // ToolUsage evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_usage_no_tools_needed() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique("What is 2 + 2?", "The answer is 4.", &[])
            .unwrap();
        let tool_usage = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::ToolUsage)
            .unwrap();
        assert!(
            tool_usage.score >= 0.7,
            "No tools needed for simple question, got {}",
            tool_usage.score
        );
    }

    #[test]
    fn test_tool_usage_tools_should_have_been_used() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "Search for the configuration file",
                "I think the config might be somewhere.",
                &[],
            )
            .unwrap();
        let tool_usage = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::ToolUsage)
            .unwrap();
        assert!(
            tool_usage.score < 0.5,
            "Should penalize missing tool usage for search query, got {}",
            tool_usage.score
        );
    }

    #[test]
    fn test_tool_usage_tools_referenced() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "Read the config file",
                "I used file_read to get the contents of config.toml. The file contains...",
                &["file_read"],
            )
            .unwrap();
        let tool_usage = result
            .critiques
            .iter()
            .find(|c| c.dimension == CritiqueDimension::ToolUsage)
            .unwrap();
        assert!(
            tool_usage.score >= 0.8,
            "Good tool usage should score high, got {}",
            tool_usage.score
        );
    }

    // -----------------------------------------------------------------------
    // Weighted scoring tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_weighted_score_computation() {
        let engine = CritiqueEngine::with_defaults();
        let critiques = vec![
            Critique {
                dimension: CritiqueDimension::Accuracy,
                score: 0.8,
                feedback: String::new(),
                suggestion: None,
            },
            Critique {
                dimension: CritiqueDimension::Completeness,
                score: 0.6,
                feedback: String::new(),
                suggestion: None,
            },
        ];
        let score = engine.compute_weighted_score(&critiques);
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn test_weighted_score_empty_critiques() {
        let engine = CritiqueEngine::with_defaults();
        let score = engine.compute_weighted_score(&[]);
        assert!(score.abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // Auto-fix tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_auto_fix_adds_paragraph_breaks() {
        let engine = CritiqueEngine::new(CritiqueConfig {
            auto_fix: true,
            quality_threshold: 0.99, // Very high to force revision
            max_revisions: 1,
            ..CritiqueConfig::default()
        });

        let long_response = "This is a long response. ".repeat(20);
        let result = engine
            .critique("Tell me about Rust", &long_response, &[])
            .unwrap();
        // Should attempt revision
        assert!(result.revision_count > 0 || result.revised_response.is_some() || !result.accepted);
    }

    #[test]
    fn test_auto_fix_safety_removal() {
        let engine = CritiqueEngine::new(CritiqueConfig {
            auto_fix: true,
            quality_threshold: 0.99,
            max_revisions: 1,
            ..CritiqueConfig::default()
        });
        let result = engine
            .critique(
                "How to test security?",
                "Step 1: hack into the system.\nStep 2: Run normal tests.\nStep 3: Check results.",
                &[],
            )
            .unwrap();
        if let Some(revised) = &result.revised_response {
            assert!(
                !revised.to_lowercase().contains("hack into"),
                "Auto-fix should remove harmful content"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Prompt building tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_critique_prompt() {
        let engine = CritiqueEngine::with_defaults();
        let prompt = engine.build_critique_prompt("What is Rust?", "Rust is a language.");
        assert!(prompt.contains("What is Rust?"));
        assert!(prompt.contains("Rust is a language."));
        assert!(prompt.contains("Accuracy"));
        assert!(prompt.contains("Completeness"));
    }

    #[test]
    fn test_build_revision_prompt() {
        let engine = CritiqueEngine::with_defaults();
        let critiques = vec![Critique {
            dimension: CritiqueDimension::Completeness,
            score: 0.3,
            feedback: "Too short".to_string(),
            suggestion: Some("Add more detail".to_string()),
        }];
        let prompt = engine.build_revision_prompt("What is Rust?", "Rust is good.", &critiques);
        assert!(prompt.contains("Too short"));
        assert!(prompt.contains("Add more detail"));
    }

    // -----------------------------------------------------------------------
    // Acceptance tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_good_response_accepted() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique(
                "What is Rust?",
                "Rust is a systems programming language focused on safety, speed, and concurrency. \
                 It achieves memory safety without garbage collection through its ownership system. \
                 Rust has been consistently voted the most loved programming language in developer surveys.",
                &[],
            )
            .unwrap();
        // A good response should generally be accepted
        assert!(
            result.final_score > 0.5,
            "Good response should score above 0.5, got {}",
            result.final_score
        );
    }

    #[test]
    fn test_poor_response_low_score() {
        let engine = CritiqueEngine::with_defaults();
        let result = engine
            .critique("Explain Rust's ownership", "ok", &[])
            .unwrap();
        assert!(
            result.final_score < 0.8,
            "Poor response should score below 0.8, got {}",
            result.final_score
        );
    }

    // -----------------------------------------------------------------------
    // CritiqueResult serde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_critique_result_serde_roundtrip() {
        let result = CritiqueResult {
            original_response: "test".to_string(),
            critiques: vec![Critique {
                dimension: CritiqueDimension::Accuracy,
                score: 0.8,
                feedback: "good".to_string(),
                suggestion: None,
            }],
            revised_response: None,
            revision_count: 0,
            final_score: 0.8,
            accepted: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: CritiqueResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.original_response, "test");
        assert!(parsed.accepted);
        assert_eq!(parsed.critiques.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Custom dimension subset tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_critique_with_subset_dimensions() {
        let engine = CritiqueEngine::new(CritiqueConfig {
            critique_dimensions: vec![CritiqueDimension::Safety, CritiqueDimension::Clarity],
            ..CritiqueConfig::default()
        });
        let result = engine
            .critique("What is Rust?", "Rust is a programming language.", &[])
            .unwrap();
        assert_eq!(
            result.critiques.len(),
            2,
            "Should only evaluate 2 dimensions"
        );
        assert!(result
            .critiques
            .iter()
            .any(|c| c.dimension == CritiqueDimension::Safety));
        assert!(result
            .critiques
            .iter()
            .any(|c| c.dimension == CritiqueDimension::Clarity));
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_significant_words() {
        let words = extract_significant_words("How does Rust handle memory safety?");
        assert!(words.contains(&"rust".to_string()));
        assert!(words.contains(&"handle".to_string()));
        assert!(words.contains(&"memory".to_string()));
        assert!(words.contains(&"safety".to_string()));
        // Stopwords filtered
        assert!(!words.contains(&"how".to_string()));
        assert!(!words.contains(&"does".to_string()));
    }

    #[test]
    fn test_extract_significant_words_empty() {
        let words = extract_significant_words("");
        assert!(words.is_empty());
    }
}

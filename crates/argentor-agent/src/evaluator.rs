//! Self-evaluation engine for agent responses.
//!
//! After generating a response, the agent can evaluate its own output quality
//! using heuristic scoring or LLM-based evaluation, and self-correct or refine
//! if the quality falls below a configurable threshold.
//!
//! # Main types
//!
//! - [`QualityScore`] — Multi-dimensional quality assessment of a response.
//! - [`EvaluatorConfig`] — Configuration for thresholds and refinement limits.
//! - [`EvaluationResult`] — Score paired with an action (accept, refine, regenerate).
//! - [`ResponseEvaluator`] — The evaluation engine with heuristic and LLM-based methods.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Quality dimensions for evaluating agent responses.
///
/// Each dimension is scored from 0.0 (worst) to 1.0 (best). The `overall`
/// field is a weighted average computed by [`QualityScore::compute_overall`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    /// Overall quality score (0.0 - 1.0), weighted average of dimensions.
    pub overall: f32,
    /// Did the response address the user's request?
    pub relevance: f32,
    /// Is the response factually consistent with tool results?
    pub consistency: f32,
    /// Is the response complete or does it need more work?
    pub completeness: f32,
    /// Is the response clear and well-structured?
    pub clarity: f32,
    /// Textual explanation of the evaluation.
    pub explanation: String,
}

impl QualityScore {
    /// Create a score with all dimensions set to the same value.
    ///
    /// The `overall` field is also set to the same value since all dimensions
    /// are equal, making the weighted average identical.
    pub fn uniform(score: f32) -> Self {
        Self {
            overall: score,
            relevance: score,
            consistency: score,
            completeness: score,
            clarity: score,
            explanation: String::new(),
        }
    }

    /// Whether the score meets a quality threshold.
    ///
    /// Compares the `overall` score against the given threshold.
    pub fn meets_threshold(&self, threshold: f32) -> bool {
        self.overall >= threshold
    }

    /// Compute the overall score as a weighted average of dimensions.
    ///
    /// Weights:
    /// - Relevance: 0.35 (most important — did we answer the question?)
    /// - Consistency: 0.30 (factual alignment with tool results)
    /// - Completeness: 0.20 (thoroughness of the response)
    /// - Clarity: 0.15 (structure and readability)
    pub fn compute_overall(
        relevance: f32,
        consistency: f32,
        completeness: f32,
        clarity: f32,
    ) -> f32 {
        relevance * 0.35 + consistency * 0.30 + completeness * 0.20 + clarity * 0.15
    }
}

/// Configuration for the self-evaluation engine.
///
/// Controls when and how refinement is triggered after evaluating a response.
#[derive(Debug, Clone)]
pub struct EvaluatorConfig {
    /// Quality threshold below which refinement is triggered (0.0 - 1.0).
    pub refinement_threshold: f32,
    /// Maximum number of refinement iterations before accepting the response.
    pub max_refinements: u32,
    /// Whether to use heuristic evaluation (fast, no LLM call).
    pub use_heuristics: bool,
    /// Whether to include the evaluation in the response metadata.
    pub include_metadata: bool,
}

impl Default for EvaluatorConfig {
    fn default() -> Self {
        Self {
            refinement_threshold: 0.7,
            max_refinements: 2,
            use_heuristics: true,
            include_metadata: false,
        }
    }
}

/// Evaluation result with score and optional refinement action.
///
/// Returned by [`ResponseEvaluator::evaluate_heuristic`] (wrapped) or parsed
/// from an LLM evaluation response via [`ResponseEvaluator::parse_evaluation`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// The quality score for the evaluated response.
    pub score: QualityScore,
    /// The recommended action based on the score.
    pub action: EvaluationAction,
    /// Which refinement iteration produced this result (0 = first evaluation).
    pub iteration: u32,
}

/// What to do after evaluation.
///
/// Determined by [`ResponseEvaluator::determine_action`] based on the quality
/// score, the configured threshold, and the current iteration count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvaluationAction {
    /// Response is good enough, send it.
    Accept,
    /// Response needs refinement with specific feedback.
    Refine {
        /// Feedback describing what needs improvement.
        feedback: String,
    },
    /// Response should be regenerated entirely.
    Regenerate {
        /// Reason for regeneration.
        reason: String,
    },
}

/// The self-evaluation engine for agent responses.
///
/// Supports two evaluation modes:
/// - **Heuristic**: Fast, rule-based scoring that checks keyword overlap,
///   response length, structure, and tool-result references. No LLM call needed.
/// - **LLM-based**: Generates a prompt asking the LLM to evaluate its own
///   response and returns a structured quality assessment.
///
/// After evaluation, the engine recommends an action: accept the response,
/// refine it with feedback, or regenerate it entirely.
pub struct ResponseEvaluator {
    /// Configuration controlling thresholds and behavior.
    config: EvaluatorConfig,
}

impl ResponseEvaluator {
    /// Create a new evaluator with the given configuration.
    pub fn new(config: EvaluatorConfig) -> Self {
        Self { config }
    }

    /// Create a new evaluator with default configuration.
    pub fn with_defaults() -> Self {
        Self {
            config: EvaluatorConfig::default(),
        }
    }

    /// Return a reference to the evaluator's configuration.
    pub fn config(&self) -> &EvaluatorConfig {
        &self.config
    }

    /// Evaluate a response using heuristics (no LLM needed).
    ///
    /// Checks:
    /// - **Relevance**: Keyword overlap between the question and response.
    /// - **Consistency**: Whether tool results are referenced in the response.
    /// - **Completeness**: Response length (very short responses score low).
    /// - **Clarity**: Structural quality (paragraphs, formatting, line length).
    pub fn evaluate_heuristic(
        &self,
        question: &str,
        response: &str,
        tool_results: &[String],
    ) -> QualityScore {
        let relevance = self.score_relevance(question, response);
        let consistency = self.score_consistency(response, tool_results);
        let completeness = self.score_completeness(response);
        let clarity = self.score_clarity(response);
        let overall = QualityScore::compute_overall(relevance, consistency, completeness, clarity);

        QualityScore {
            overall,
            relevance,
            consistency,
            completeness,
            clarity,
            explanation: self.explain_score(relevance, consistency, completeness, clarity),
        }
    }

    /// Build a prompt for LLM-based evaluation.
    ///
    /// Generates a prompt that asks the LLM to evaluate a response and return
    /// a structured JSON quality assessment with scores and optional refinement
    /// feedback.
    pub fn build_evaluation_prompt(
        &self,
        question: &str,
        response: &str,
        tool_results: &[String],
    ) -> String {
        let tools_text = tool_results.join("; ");
        format!(
            "Evaluate the following response to the user's question.\n\n\
             Question: {question}\n\n\
             Response: {response}\n\n\
             Tool results used: {tools_text}\n\n\
             Rate the response on these dimensions (0.0-1.0):\n\
             1. Relevance: Does it address the question?\n\
             2. Consistency: Is it consistent with the tool results?\n\
             3. Completeness: Is it thorough?\n\
             4. Clarity: Is it well-structured?\n\n\
             Respond with JSON: {{\"relevance\": 0.X, \"consistency\": 0.X, \
             \"completeness\": 0.X, \"clarity\": 0.X, \"explanation\": \"...\", \
             \"refinement_needed\": true/false, \"refinement_feedback\": \"...\"}}"
        )
    }

    /// Parse an LLM evaluation response into an [`EvaluationResult`].
    ///
    /// Extracts JSON from the LLM output, parses dimension scores, computes
    /// the overall score, and determines the appropriate action based on the
    /// configured threshold.
    pub fn parse_evaluation(
        &self,
        llm_output: &str,
        iteration: u32,
    ) -> Result<EvaluationResult, String> {
        // Find JSON in the output (look for { ... })
        let json_str = extract_json(llm_output).ok_or_else(|| {
            format!("No JSON object found in LLM evaluation output: {llm_output}")
        })?;

        let parsed: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("Invalid JSON: {e}"))?;

        let relevance = parsed
            .get("relevance")
            .and_then(serde_json::Value::as_f64)
            .ok_or("Missing 'relevance' field")? as f32;
        let consistency = parsed
            .get("consistency")
            .and_then(serde_json::Value::as_f64)
            .ok_or("Missing 'consistency' field")? as f32;
        let completeness = parsed
            .get("completeness")
            .and_then(serde_json::Value::as_f64)
            .ok_or("Missing 'completeness' field")? as f32;
        let clarity = parsed
            .get("clarity")
            .and_then(serde_json::Value::as_f64)
            .ok_or("Missing 'clarity' field")? as f32;
        let explanation = parsed
            .get("explanation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();

        let overall = QualityScore::compute_overall(relevance, consistency, completeness, clarity);

        let score = QualityScore {
            overall,
            relevance,
            consistency,
            completeness,
            clarity,
            explanation,
        };

        let action = self.determine_action(&score, iteration);

        Ok(EvaluationResult {
            score,
            action,
            iteration,
        })
    }

    /// Build a refinement prompt that incorporates evaluation feedback.
    ///
    /// Used when the evaluation action is [`EvaluationAction::Refine`] to ask
    /// the LLM to improve its previous response based on specific feedback.
    pub fn build_refinement_prompt(
        &self,
        original_question: &str,
        original_response: &str,
        feedback: &str,
    ) -> String {
        format!(
            "Your previous response to the question below needs improvement.\n\n\
             Question: {original_question}\n\n\
             Your previous response: {original_response}\n\n\
             Feedback: {feedback}\n\n\
             Please provide an improved response addressing the feedback."
        )
    }

    /// Determine the evaluation action based on the score and iteration.
    ///
    /// Decision logic:
    /// - If overall score meets the threshold: [`EvaluationAction::Accept`]
    /// - If max refinements reached: [`EvaluationAction::Accept`] (give up refining)
    /// - If overall score is below 0.3: [`EvaluationAction::Regenerate`]
    /// - Otherwise: [`EvaluationAction::Refine`]
    pub fn determine_action(&self, score: &QualityScore, iteration: u32) -> EvaluationAction {
        if score.overall >= self.config.refinement_threshold
            || iteration >= self.config.max_refinements
        {
            EvaluationAction::Accept
        } else if score.overall < 0.3 {
            EvaluationAction::Regenerate {
                reason: score.explanation.clone(),
            }
        } else {
            EvaluationAction::Refine {
                feedback: score.explanation.clone(),
            }
        }
    }

    // --- Internal scoring functions ---

    /// Score relevance based on keyword overlap between question and response.
    ///
    /// Filters words shorter than 4 characters to ignore stop words, then
    /// computes the ratio of question keywords found in the response.
    fn score_relevance(&self, question: &str, response: &str) -> f32 {
        let q_lower = question.to_lowercase();
        let q_words: HashSet<&str> = q_lower.split_whitespace().filter(|w| w.len() > 3).collect();

        if q_words.is_empty() {
            return 0.5;
        }

        let r_lower = response.to_lowercase();
        let r_words: HashSet<&str> = r_lower.split_whitespace().filter(|w| w.len() > 3).collect();

        let overlap = q_words.intersection(&r_words).count();
        let ratio = overlap as f32 / q_words.len() as f32;
        (ratio * 1.5).min(1.0) // Scale up, cap at 1.0
    }

    /// Score consistency by checking if tool result keywords appear in the response.
    ///
    /// Returns 1.0 if no tool results are provided (consistent by default).
    fn score_consistency(&self, response: &str, tool_results: &[String]) -> f32 {
        if tool_results.is_empty() {
            return 1.0; // No tools = consistent by default
        }

        let response_lower = response.to_lowercase();
        let mut referenced = 0;

        for result in tool_results {
            let key_words: Vec<&str> = result
                .split_whitespace()
                .filter(|w| w.len() > 4)
                .take(5)
                .collect();

            if key_words
                .iter()
                .any(|w| response_lower.contains(&w.to_lowercase()))
            {
                referenced += 1;
            }
        }

        referenced as f32 / tool_results.len() as f32
    }

    /// Score completeness based on response length.
    ///
    /// Very short responses score low; responses over 200 characters score 1.0.
    fn score_completeness(&self, response: &str) -> f32 {
        let len = response.len();
        if len < 20 {
            0.2
        } else if len < 50 {
            0.4
        } else if len < 100 {
            0.6
        } else if len < 200 {
            0.8
        } else {
            1.0
        }
    }

    /// Score clarity based on structural quality of the response.
    ///
    /// Rewards responses with multiple paragraphs, reasonable line lengths,
    /// and formatting elements like code blocks, lists, or numbered items.
    fn score_clarity(&self, response: &str) -> f32 {
        let lines = response.lines().count();
        let avg_line_len = response.len().checked_div(lines).unwrap_or(response.len());

        let mut score: f32 = 0.5;
        if lines > 1 {
            score += 0.2; // Has paragraphs
        }
        if avg_line_len < 200 {
            score += 0.1; // Not giant paragraphs
        }
        if response.contains("```") || response.contains("- ") || response.contains("1.") {
            score += 0.2; // Has formatting
        }
        score.min(1.0)
    }

    /// Generate a human-readable explanation of the quality scores.
    fn explain_score(
        &self,
        relevance: f32,
        consistency: f32,
        completeness: f32,
        clarity: f32,
    ) -> String {
        let mut issues = Vec::new();
        if relevance < 0.5 {
            issues.push("Low relevance to the question");
        }
        if consistency < 0.5 {
            issues.push("Inconsistent with tool results");
        }
        if completeness < 0.5 {
            issues.push("Response is too brief or incomplete");
        }
        if clarity < 0.5 {
            issues.push("Response lacks clear structure");
        }
        if issues.is_empty() {
            "Response meets quality standards".to_string()
        } else {
            issues.join(". ")
        }
    }
}

/// Extract the first JSON object from a string.
///
/// Looks for matching `{` and `}` braces, handling nesting. Returns `None`
/// if no valid JSON object boundary is found.
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;

    for (i, ch) in text[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // --- QualityScore tests ---

    #[test]
    fn test_quality_score_uniform() {
        let score = QualityScore::uniform(0.8);
        assert_eq!(score.overall, 0.8);
        assert_eq!(score.relevance, 0.8);
        assert_eq!(score.consistency, 0.8);
        assert_eq!(score.completeness, 0.8);
        assert_eq!(score.clarity, 0.8);
        assert!(score.explanation.is_empty());
    }

    #[test]
    fn test_quality_score_meets_threshold() {
        let high = QualityScore::uniform(0.9);
        assert!(high.meets_threshold(0.7));
        assert!(high.meets_threshold(0.9));
        assert!(!high.meets_threshold(0.95));

        let low = QualityScore::uniform(0.3);
        assert!(!low.meets_threshold(0.7));
        assert!(low.meets_threshold(0.3));
        assert!(low.meets_threshold(0.1));
    }

    #[test]
    fn test_compute_overall_weighted() {
        // All 1.0 -> 1.0
        let overall = QualityScore::compute_overall(1.0, 1.0, 1.0, 1.0);
        assert!((overall - 1.0).abs() < f32::EPSILON);

        // All 0.0 -> 0.0
        let overall = QualityScore::compute_overall(0.0, 0.0, 0.0, 0.0);
        assert!((overall - 0.0).abs() < f32::EPSILON);

        // Only relevance = 1.0 -> 0.35
        let overall = QualityScore::compute_overall(1.0, 0.0, 0.0, 0.0);
        assert!((overall - 0.35).abs() < f32::EPSILON);

        // Only consistency = 1.0 -> 0.30
        let overall = QualityScore::compute_overall(0.0, 1.0, 0.0, 0.0);
        assert!((overall - 0.30).abs() < f32::EPSILON);

        // Only completeness = 1.0 -> 0.20
        let overall = QualityScore::compute_overall(0.0, 0.0, 1.0, 0.0);
        assert!((overall - 0.20).abs() < f32::EPSILON);

        // Only clarity = 1.0 -> 0.15
        let overall = QualityScore::compute_overall(0.0, 0.0, 0.0, 1.0);
        assert!((overall - 0.15).abs() < f32::EPSILON);

        // Mixed: 0.8, 0.6, 0.9, 0.7
        // = 0.8*0.35 + 0.6*0.30 + 0.9*0.20 + 0.7*0.15
        // = 0.28 + 0.18 + 0.18 + 0.105 = 0.745
        let overall = QualityScore::compute_overall(0.8, 0.6, 0.9, 0.7);
        assert!((overall - 0.745).abs() < 0.001);
    }

    // --- EvaluatorConfig tests ---

    #[test]
    fn test_evaluator_config_defaults() {
        let config = EvaluatorConfig::default();
        assert!((config.refinement_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(config.max_refinements, 2);
        assert!(config.use_heuristics);
        assert!(!config.include_metadata);
    }

    // --- Heuristic evaluation tests ---

    #[test]
    fn test_evaluate_heuristic_good_response() {
        let evaluator = ResponseEvaluator::with_defaults();
        let question = "What is the current weather in Buenos Aires?";
        let response = "The current weather in Buenos Aires is sunny with a temperature \
                         of 28 degrees Celsius. The humidity is around 65% and there is \
                         a light breeze coming from the east. Overall, it is a pleasant \
                         day for outdoor activities.";
        let tool_results = vec!["Weather data: Buenos Aires temperature 28C sunny".to_string()];

        let score = evaluator.evaluate_heuristic(question, response, &tool_results);

        // Should score well on all dimensions
        assert!(
            score.overall > 0.5,
            "Overall should be > 0.5, got {}",
            score.overall
        );
        assert!(
            score.relevance > 0.3,
            "Relevance should be > 0.3, got {}",
            score.relevance
        );
        assert!(
            score.completeness >= 0.8,
            "Completeness should be >= 0.8, got {}",
            score.completeness
        );
    }

    #[test]
    fn test_evaluate_heuristic_short_response() {
        let evaluator = ResponseEvaluator::with_defaults();
        let question = "Explain the theory of relativity";
        let response = "It's about time.";
        let tool_results: Vec<String> = vec![];

        let score = evaluator.evaluate_heuristic(question, response, &tool_results);

        // Short response should score low on completeness
        assert!(
            score.completeness <= 0.4,
            "Completeness should be <= 0.4, got {}",
            score.completeness
        );
    }

    #[test]
    fn test_evaluate_heuristic_irrelevant_response() {
        let evaluator = ResponseEvaluator::with_defaults();
        let question = "What is the capital of Argentina?";
        let response = "Photosynthesis is the process by which plants convert sunlight \
                         into chemical energy stored in glucose molecules through a series \
                         of complex biochemical reactions.";
        let tool_results: Vec<String> = vec![];

        let score = evaluator.evaluate_heuristic(question, response, &tool_results);

        // Irrelevant response should have low relevance
        assert!(
            score.relevance < 0.5,
            "Relevance should be < 0.5, got {}",
            score.relevance
        );
    }

    // --- Individual scoring function tests ---

    #[test]
    fn test_score_relevance_high_overlap() {
        let evaluator = ResponseEvaluator::with_defaults();
        let question = "How does Rust handle memory management?";
        let response = "Rust handles memory management through its ownership system, \
                         which ensures memory safety without a garbage collector.";

        let score = evaluator.score_relevance(question, response);
        // Words > 3 chars from question: "does", "rust", "handle", "memory", "management"
        // Most should appear in the response
        assert!(score > 0.5, "Relevance score should be > 0.5, got {score}");
    }

    #[test]
    fn test_score_relevance_no_overlap() {
        let evaluator = ResponseEvaluator::with_defaults();
        let question = "What is quantum entanglement?";
        let response = "The stock market closed higher today with gains across all sectors.";

        let score = evaluator.score_relevance(question, response);
        assert!(score < 0.5, "Relevance score should be < 0.5, got {score}");
    }

    #[test]
    fn test_score_relevance_short_question_words() {
        let evaluator = ResponseEvaluator::with_defaults();
        // All words <= 3 chars
        let question = "is it ok?";
        let response = "Yes, it is fine.";

        let score = evaluator.score_relevance(question, response);
        // Should return 0.5 default since no words > 3 chars
        assert!(
            (score - 0.5).abs() < f32::EPSILON,
            "Expected 0.5, got {score}"
        );
    }

    #[test]
    fn test_score_consistency_with_tool_results() {
        let evaluator = ResponseEvaluator::with_defaults();
        let response = "The file contains 42 lines of Python code with three functions defined.";
        let tool_results = vec![
            "File analysis: contains Python code with functions".to_string(),
            "Line count: 42 lines total".to_string(),
        ];

        let score = evaluator.score_consistency(response, &tool_results);
        // "Python" and "functions" from result 1, "lines" from result 2 should be found
        assert!(score > 0.0, "Consistency should be > 0.0, got {score}");
    }

    #[test]
    fn test_score_consistency_no_tools() {
        let evaluator = ResponseEvaluator::with_defaults();
        let response = "This is a response without any tool usage.";
        let tool_results: Vec<String> = vec![];

        let score = evaluator.score_consistency(response, &tool_results);
        assert!(
            (score - 1.0).abs() < f32::EPSILON,
            "No tools should give 1.0, got {score}"
        );
    }

    #[test]
    fn test_score_consistency_unreferenced_tools() {
        let evaluator = ResponseEvaluator::with_defaults();
        let response = "The answer is 42.";
        let tool_results =
            vec!["Database query returned: customer records for enterprise accounts".to_string()];

        let score = evaluator.score_consistency(response, &tool_results);
        // None of the key words from tool results appear in the response
        assert!(
            score < 1.0,
            "Unreferenced tool results should score < 1.0, got {score}"
        );
    }

    #[test]
    fn test_score_completeness_tiers() {
        let evaluator = ResponseEvaluator::with_defaults();

        // < 20 chars -> 0.2
        assert!((evaluator.score_completeness("Short.") - 0.2).abs() < f32::EPSILON);

        // 20-49 chars -> 0.4
        assert!(
            (evaluator.score_completeness("This is a bit longer now.") - 0.4).abs() < f32::EPSILON
        );

        // 50-99 chars -> 0.6
        let medium = "This is a medium-length response that has some substance to it at least.";
        assert!((evaluator.score_completeness(medium) - 0.6).abs() < f32::EPSILON);

        // 100-199 chars -> 0.8
        let longer = "This response is getting more substantial now. It covers the topic \
                       in reasonable depth and provides enough information to be genuinely useful to the reader.";
        assert!((evaluator.score_completeness(longer) - 0.8).abs() < f32::EPSILON);

        // >= 200 chars -> 1.0
        let long = "This is a very comprehensive response that goes into great detail \
                     about the subject matter. It covers multiple aspects of the topic, \
                     provides examples, and offers nuanced analysis. The length indicates \
                     thorough treatment of the question asked.";
        assert!((evaluator.score_completeness(long) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_score_clarity_structured() {
        let evaluator = ResponseEvaluator::with_defaults();
        let structured = "Here is the analysis:\n\n\
                           - First point about the topic\n\
                           - Second point with details\n\n\
                           ```rust\nfn main() {}\n```";

        let score = evaluator.score_clarity(structured);
        // Multiple lines (+0.2), short avg line (+0.1), has formatting (+0.2) = 1.0
        assert!(
            score >= 0.9,
            "Structured response should score >= 0.9, got {score}"
        );
    }

    #[test]
    fn test_score_clarity_unstructured() {
        let evaluator = ResponseEvaluator::with_defaults();
        let unstructured =
            "This is a single line of text without any formatting or structure whatsoever.";

        let score = evaluator.score_clarity(unstructured);
        // Single line: 0.5 base + 0.1 (avg_line_len < 200) = 0.6
        assert!(
            score < 0.8,
            "Unstructured response should score < 0.8, got {score}"
        );
    }

    // --- determine_action tests ---

    #[test]
    fn test_determine_action_accept() {
        let evaluator = ResponseEvaluator::with_defaults();
        let score = QualityScore::uniform(0.9);

        let action = evaluator.determine_action(&score, 0);
        assert!(matches!(action, EvaluationAction::Accept));
    }

    #[test]
    fn test_determine_action_refine() {
        let evaluator = ResponseEvaluator::with_defaults();
        let mut score = QualityScore::uniform(0.5);
        score.explanation = "Needs improvement".to_string();

        let action = evaluator.determine_action(&score, 0);
        match action {
            EvaluationAction::Refine { feedback } => {
                assert_eq!(feedback, "Needs improvement");
            }
            other => panic!("Expected Refine, got {other:?}"),
        }
    }

    #[test]
    fn test_determine_action_regenerate() {
        let evaluator = ResponseEvaluator::with_defaults();
        let mut score = QualityScore::uniform(0.2);
        score.explanation = "Very poor quality".to_string();

        let action = evaluator.determine_action(&score, 0);
        match action {
            EvaluationAction::Regenerate { reason } => {
                assert_eq!(reason, "Very poor quality");
            }
            other => panic!("Expected Regenerate, got {other:?}"),
        }
    }

    #[test]
    fn test_determine_action_max_iterations() {
        let evaluator = ResponseEvaluator::with_defaults(); // max_refinements = 2
        let score = QualityScore::uniform(0.4);

        // At iteration 2 (== max_refinements), should accept even with low score
        let action = evaluator.determine_action(&score, 2);
        assert!(matches!(action, EvaluationAction::Accept));

        // At iteration 3 (> max_refinements), should also accept
        let action = evaluator.determine_action(&score, 3);
        assert!(matches!(action, EvaluationAction::Accept));
    }

    #[test]
    fn test_determine_action_boundary_threshold() {
        let evaluator = ResponseEvaluator::with_defaults(); // threshold = 0.7
        let score_at = QualityScore::uniform(0.7);
        let score_below = QualityScore::uniform(0.699);

        assert!(matches!(
            evaluator.determine_action(&score_at, 0),
            EvaluationAction::Accept
        ));
        assert!(!matches!(
            evaluator.determine_action(&score_below, 0),
            EvaluationAction::Accept
        ));
    }

    // --- Prompt building tests ---

    #[test]
    fn test_build_evaluation_prompt() {
        let evaluator = ResponseEvaluator::with_defaults();
        let prompt = evaluator.build_evaluation_prompt(
            "What is Rust?",
            "Rust is a systems programming language.",
            &["Wikipedia: Rust is a language".to_string()],
        );

        assert!(prompt.contains("What is Rust?"));
        assert!(prompt.contains("Rust is a systems programming language."));
        assert!(prompt.contains("Wikipedia: Rust is a language"));
        assert!(prompt.contains("relevance"));
        assert!(prompt.contains("consistency"));
        assert!(prompt.contains("completeness"));
        assert!(prompt.contains("clarity"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_build_evaluation_prompt_multiple_tools() {
        let evaluator = ResponseEvaluator::with_defaults();
        let prompt = evaluator.build_evaluation_prompt(
            "question",
            "response",
            &["result1".to_string(), "result2".to_string()],
        );

        // Tool results should be joined with "; "
        assert!(prompt.contains("result1; result2"));
    }

    #[test]
    fn test_build_refinement_prompt() {
        let evaluator = ResponseEvaluator::with_defaults();
        let prompt = evaluator.build_refinement_prompt(
            "What is Rust?",
            "It's a language.",
            "Response is too brief, expand on safety features.",
        );

        assert!(prompt.contains("What is Rust?"));
        assert!(prompt.contains("It's a language."));
        assert!(prompt.contains("Response is too brief, expand on safety features."));
        assert!(prompt.contains("improved response"));
    }

    // --- parse_evaluation tests ---

    #[test]
    fn test_parse_evaluation_valid() {
        let evaluator = ResponseEvaluator::with_defaults();
        let llm_output = r#"Here is my evaluation:
        {"relevance": 0.9, "consistency": 0.8, "completeness": 0.7, "clarity": 0.85, "explanation": "Good response overall", "refinement_needed": false, "refinement_feedback": ""}
        "#;

        let result = evaluator.parse_evaluation(llm_output, 0).unwrap();

        assert!((result.score.relevance - 0.9).abs() < f32::EPSILON);
        assert!((result.score.consistency - 0.8).abs() < f32::EPSILON);
        assert!((result.score.completeness - 0.7).abs() < f32::EPSILON);
        assert!((result.score.clarity - 0.85).abs() < 0.001);
        assert_eq!(result.score.explanation, "Good response overall");

        // Overall = 0.9*0.35 + 0.8*0.30 + 0.7*0.20 + 0.85*0.15
        //         = 0.315 + 0.24 + 0.14 + 0.1275 = 0.8225
        assert!((result.score.overall - 0.8225).abs() < 0.01);
        assert_eq!(result.iteration, 0);
        assert!(matches!(result.action, EvaluationAction::Accept));
    }

    #[test]
    fn test_parse_evaluation_invalid() {
        let evaluator = ResponseEvaluator::with_defaults();

        // No JSON at all
        let result = evaluator.parse_evaluation("No JSON here", 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No JSON object found"));

        // Invalid JSON
        let result = evaluator.parse_evaluation("{invalid json}", 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid JSON"));

        // Missing required field
        let result = evaluator.parse_evaluation(
            r#"{"relevance": 0.5, "consistency": 0.5, "completeness": 0.5}"#,
            0,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("clarity"));
    }

    // --- EvaluationResult and EvaluationAction serialization ---

    #[test]
    fn test_evaluation_result_serialization() {
        let result = EvaluationResult {
            score: QualityScore::uniform(0.85),
            action: EvaluationAction::Accept,
            iteration: 1,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: EvaluationResult = serde_json::from_str(&json).unwrap();

        assert!((deserialized.score.overall - 0.85).abs() < f32::EPSILON);
        assert_eq!(deserialized.iteration, 1);
        assert!(matches!(deserialized.action, EvaluationAction::Accept));
    }

    #[test]
    fn test_evaluation_action_refine_serialization() {
        let action = EvaluationAction::Refine {
            feedback: "Add more detail".to_string(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let deserialized: EvaluationAction = serde_json::from_str(&json).unwrap();

        match deserialized {
            EvaluationAction::Refine { feedback } => {
                assert_eq!(feedback, "Add more detail");
            }
            other => panic!("Expected Refine, got {other:?}"),
        }
    }

    #[test]
    fn test_evaluation_action_regenerate_serialization() {
        let action = EvaluationAction::Regenerate {
            reason: "Completely off topic".to_string(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let deserialized: EvaluationAction = serde_json::from_str(&json).unwrap();

        match deserialized {
            EvaluationAction::Regenerate { reason } => {
                assert_eq!(reason, "Completely off topic");
            }
            other => panic!("Expected Regenerate, got {other:?}"),
        }
    }

    // --- extract_json helper tests ---

    #[test]
    fn test_extract_json_simple() {
        let text = r#"Some text {"key": "value"} more text"#;
        let json = extract_json(text).unwrap();
        assert_eq!(json, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_extract_json_nested() {
        let text = r#"{"outer": {"inner": 42}}"#;
        let json = extract_json(text).unwrap();
        assert_eq!(json, text);
    }

    #[test]
    fn test_extract_json_none() {
        assert!(extract_json("no json here").is_none());
        assert!(extract_json("").is_none());
    }

    // --- explain_score tests ---

    #[test]
    fn test_explain_score_all_good() {
        let evaluator = ResponseEvaluator::with_defaults();
        let explanation = evaluator.explain_score(0.8, 0.9, 0.7, 0.6);
        assert_eq!(explanation, "Response meets quality standards");
    }

    #[test]
    fn test_explain_score_all_bad() {
        let evaluator = ResponseEvaluator::with_defaults();
        let explanation = evaluator.explain_score(0.1, 0.2, 0.3, 0.4);
        assert!(explanation.contains("Low relevance"));
        assert!(explanation.contains("Inconsistent with tool results"));
        assert!(explanation.contains("too brief or incomplete"));
        assert!(explanation.contains("lacks clear structure"));
    }

    // --- Custom config tests ---

    #[test]
    fn test_evaluator_custom_config() {
        let config = EvaluatorConfig {
            refinement_threshold: 0.9,
            max_refinements: 5,
            use_heuristics: false,
            include_metadata: true,
        };
        let evaluator = ResponseEvaluator::new(config);
        let cfg = evaluator.config();

        assert!((cfg.refinement_threshold - 0.9).abs() < f32::EPSILON);
        assert_eq!(cfg.max_refinements, 5);
        assert!(!cfg.use_heuristics);
        assert!(cfg.include_metadata);

        // With high threshold, a 0.8 score should trigger refinement
        let score = QualityScore::uniform(0.8);
        let action = evaluator.determine_action(&score, 0);
        assert!(matches!(action, EvaluationAction::Refine { .. }));
    }
}

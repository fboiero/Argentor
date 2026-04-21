//! Extended Thinking Mode for structured multi-step deliberation.
//!
//! Inspired by Claude's extended thinking and DeepSeek-R1's multi-step reasoning.
//! Lets agents spend more tokens reasoning BEFORE acting, producing a structured
//! thinking trace with decomposed subtasks, tool recommendations, and confidence
//! scoring.
//!
//! The engine is **stateless** — it builds thinking prompts and structures, but
//! does not call the LLM itself. The LLM call happens in the runner.
//!
//! # Architecture
//!
//! ```text
//! ┌───────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │ User Msg  │ --> │  ThinkingEngine  │ --> │ ThinkingResult  │
//! │ + Tools   │     │  (multi-pass)    │     │ (plan + conf.)  │
//! └───────────┘     └──────────────────┘     └─────────────────┘
//!                          │
//!                   ┌──────┴──────┐
//!                   │  Passes:    │
//!                   │  1. Analyze │
//!                   │  2. Decomp. │
//!                   │  3. Plan    │
//!                   │  4. Eval    │
//!                   │  5. Synth.  │
//!                   └─────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Common English stopwords filtered during keyword extraction.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
    "our", "out", "has", "have", "from", "with", "they", "been", "this", "that", "will", "each",
    "make", "like", "use", "into", "what", "how", "does", "just", "please", "could", "would",
    "should", "about", "when", "then", "than", "also", "some", "any", "there", "here",
];

/// How deep the thinking process should go.
///
/// Each level implies more passes and more tokens spent on deliberation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingDepth {
    /// 1 pass (Analyze only), ~256 tokens.
    Quick,
    /// 2 passes (Analyze + PlanTools), ~1024 tokens.
    Standard,
    /// 3 passes (Analyze + Decompose + PlanTools), ~2048 tokens.
    Deep,
    /// Iterative until confidence threshold, ~4096 tokens.
    Exhaustive,
}

impl ThinkingDepth {
    /// Return the maximum number of thinking passes for this depth.
    pub fn max_passes(self) -> usize {
        match self {
            ThinkingDepth::Quick => 1,
            ThinkingDepth::Standard => 2,
            ThinkingDepth::Deep => 3,
            ThinkingDepth::Exhaustive => 5,
        }
    }

    /// Return the approximate token budget for this depth.
    pub fn token_budget(self) -> usize {
        match self {
            ThinkingDepth::Quick => 256,
            ThinkingDepth::Standard => 1024,
            ThinkingDepth::Deep => 2048,
            ThinkingDepth::Exhaustive => 4096,
        }
    }
}

/// The type of a single thinking step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingStepType {
    /// Analyze the user's request and intent.
    Analyze,
    /// Decompose the task into subtasks.
    Decompose,
    /// Plan which tools to use and in what order.
    PlanTools,
    /// Evaluate potential approaches and tradeoffs.
    Evaluate,
    /// Synthesize findings into a coherent plan.
    Synthesize,
}

/// A single step in the thinking process.
///
/// Captures what the engine did at each thinking pass, including the type
/// of reasoning, the textual content, and an approximate token count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingStep {
    /// What type of reasoning this step performed.
    pub step_type: ThinkingStepType,
    /// The generated thinking content.
    pub content: String,
    /// Approximate tokens used for this step.
    pub tokens_used: usize,
}

/// The result of a thinking session.
///
/// Contains the full chain of thinking steps, decomposed subtasks, confidence
/// score, tool recommendations, and an optional plan summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingResult {
    /// The thinking steps executed, in order.
    pub thinking_steps: Vec<ThinkingStep>,
    /// Subtasks decomposed from the original request.
    pub decomposed_subtasks: Vec<String>,
    /// Confidence in the plan (0.0 - 1.0).
    pub confidence: f32,
    /// Total approximate tokens used across all steps.
    pub total_thinking_tokens: usize,
    /// Tool names recommended for the task.
    pub recommended_tools: Vec<String>,
    /// Optional synthesized plan text.
    pub plan: Option<String>,
}

/// Configuration for the thinking engine.
///
/// Controls whether thinking is enabled, token budgets, depth, and
/// whether the thinking trace is exposed to the caller.
#[derive(Debug, Clone)]
pub struct ThinkingConfig {
    /// Whether extended thinking is enabled.
    pub enabled: bool,
    /// Maximum tokens to spend on thinking (default: 2048).
    pub max_thinking_tokens: usize,
    /// Fraction of total token budget allocated to thinking (default: 0.3).
    pub thinking_budget_ratio: f32,
    /// Thinking depth level.
    pub depth: ThinkingDepth,
    /// Whether to expose thinking steps to the caller.
    pub show_thinking: bool,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_thinking_tokens: 2048,
            thinking_budget_ratio: 0.3,
            depth: ThinkingDepth::Standard,
            show_thinking: false,
        }
    }
}

/// The extended thinking engine.
///
/// Builds structured thinking traces by analyzing user requests, decomposing
/// tasks, recommending tools, and synthesizing plans. The engine is stateless;
/// it produces [`ThinkingResult`] values that the runner can use to guide
/// the agentic loop.
pub struct ThinkingEngine {
    config: ThinkingConfig,
}

impl ThinkingEngine {
    /// Create a new thinking engine with the given configuration.
    pub fn new(config: ThinkingConfig) -> Self {
        Self { config }
    }

    /// Create a new thinking engine with default configuration.
    pub fn with_defaults() -> Self {
        Self {
            config: ThinkingConfig::default(),
        }
    }

    /// Return a reference to the engine's configuration.
    pub fn config(&self) -> &ThinkingConfig {
        &self.config
    }

    /// Perform structured thinking on a user message with available tool names.
    ///
    /// Depending on the configured [`ThinkingDepth`], this runs 1-5 passes:
    /// 1. **Analyze** — understand intent and extract key concepts
    /// 2. **Decompose** — break into subtasks (Deep+ only)
    /// 3. **PlanTools** — recommend tools based on task keywords
    /// 4. **Evaluate** — assess approach confidence (Exhaustive only)
    /// 5. **Synthesize** — produce a coherent plan (Exhaustive only)
    ///
    /// Returns `None` if thinking is disabled.
    pub fn think(&self, user_message: &str, available_tools: &[&str]) -> Option<ThinkingResult> {
        if !self.config.enabled || user_message.is_empty() {
            return None;
        }

        let mut steps = Vec::new();
        let mut total_tokens = 0_usize;
        let budget = self.config.max_thinking_tokens;
        let max_passes = self.config.depth.max_passes();

        // Pass 1: Analyze
        let analysis = self.analyze(user_message);
        let tokens = estimate_tokens(&analysis.content);
        total_tokens += tokens;
        steps.push(analysis);

        if max_passes < 2 || total_tokens >= budget {
            return Some(self.build_result(steps, total_tokens, available_tools, user_message));
        }

        // Pass 2: Decompose (Deep+) or PlanTools (Standard)
        if self.config.depth == ThinkingDepth::Standard {
            let plan_step = self.plan_tools(user_message, available_tools);
            total_tokens += estimate_tokens(&plan_step.content);
            steps.push(plan_step);
        } else {
            let decompose = self.decompose(user_message);
            total_tokens += estimate_tokens(&decompose.content);
            steps.push(decompose);
        }

        if max_passes < 3 || total_tokens >= budget {
            return Some(self.build_result(steps, total_tokens, available_tools, user_message));
        }

        // Pass 3: PlanTools (Deep+)
        let plan_step = self.plan_tools(user_message, available_tools);
        total_tokens += estimate_tokens(&plan_step.content);
        steps.push(plan_step);

        if max_passes < 4 || total_tokens >= budget {
            return Some(self.build_result(steps, total_tokens, available_tools, user_message));
        }

        // Pass 4: Evaluate (Exhaustive only)
        let eval_step = self.evaluate_approach(user_message, &steps);
        total_tokens += estimate_tokens(&eval_step.content);
        steps.push(eval_step);

        if max_passes < 5 || total_tokens >= budget {
            return Some(self.build_result(steps, total_tokens, available_tools, user_message));
        }

        // Pass 5: Synthesize (Exhaustive only)
        let synth = self.synthesize(&steps);
        total_tokens += estimate_tokens(&synth.content);
        steps.push(synth);

        Some(self.build_result(steps, total_tokens, available_tools, user_message))
    }

    /// Build the thinking system prompt to instruct an LLM to reason step-by-step.
    ///
    /// This prompt is injected before the user message when extended thinking is
    /// active, guiding the model to produce structured reasoning.
    pub fn build_thinking_prompt(&self, available_tools: &[&str]) -> String {
        let tool_list = if available_tools.is_empty() {
            "No tools available.".to_string()
        } else {
            format!("Available tools: {}", available_tools.join(", "))
        };

        format!(
            "Before responding, think through the problem step by step.\n\n\
             {tool_list}\n\n\
             Structure your thinking as:\n\
             1. ANALYZE: What is the user asking? What are the key concepts?\n\
             2. DECOMPOSE: Can this be broken into subtasks?\n\
             3. PLAN: Which tools (if any) would help? In what order?\n\
             4. EVALUATE: What could go wrong? Are there alternatives?\n\
             5. SYNTHESIZE: What is the best approach?\n\n\
             Thinking depth: {:?}\n\
             Token budget for thinking: {}",
            self.config.depth,
            self.config.depth.token_budget()
        )
    }

    // --- Internal pass implementations ---

    /// Analyze the user message to understand intent and extract key concepts.
    fn analyze(&self, user_message: &str) -> ThinkingStep {
        let keywords = extract_keywords(user_message);
        let intent = classify_intent(user_message);
        let complexity = estimate_complexity(user_message);

        let content = format!(
            "Analysis of request:\n\
             - Intent: {intent}\n\
             - Key concepts: {}\n\
             - Estimated complexity: {complexity}\n\
             - Message length: {} chars",
            keywords.join(", "),
            user_message.len()
        );

        ThinkingStep {
            step_type: ThinkingStepType::Analyze,
            content,
            tokens_used: 0, // Will be computed by caller
        }
    }

    /// Decompose the user message into subtasks.
    fn decompose(&self, user_message: &str) -> ThinkingStep {
        let sentences: Vec<&str> = user_message
            .split(['.', '?', '!', '\n'])
            .map(str::trim)
            .filter(|s| s.len() > 5)
            .collect();

        let subtasks: Vec<String> = if sentences.len() > 1 {
            sentences
                .iter()
                .enumerate()
                .map(|(i, s)| format!("Subtask {}: {s}", i + 1))
                .collect()
        } else {
            // Single sentence — try to decompose by conjunctions
            let parts: Vec<&str> = user_message
                .split(" and ")
                .flat_map(|p| p.split(" then "))
                .flat_map(|p| p.split(" also "))
                .map(str::trim)
                .filter(|s| s.len() > 5)
                .collect();
            if parts.len() > 1 {
                parts
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("Subtask {}: {s}", i + 1))
                    .collect()
            } else {
                vec![format!("Subtask 1: {user_message}")]
            }
        };

        let content = format!(
            "Decomposition:\n{}",
            subtasks
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        ThinkingStep {
            step_type: ThinkingStepType::Decompose,
            content,
            tokens_used: 0,
        }
    }

    /// Plan which tools to use based on task keywords and available tools.
    fn plan_tools(&self, user_message: &str, available_tools: &[&str]) -> ThinkingStep {
        let msg_lower = user_message.to_lowercase();
        let keywords = extract_keywords(user_message);

        let mut recommended: Vec<(String, String)> = Vec::new();

        for tool in available_tools {
            let tool_lower = tool.to_lowercase();
            let tool_parts: HashSet<&str> = tool_lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 2)
                .collect();

            // Check if any keyword matches tool name parts
            let matches = keywords
                .iter()
                .any(|kw| tool_parts.contains(kw.as_str()) || tool_lower.contains(kw.as_str()));

            // Check for common task-tool associations
            let contextual_match = match_tool_to_context(&msg_lower, tool);

            if matches || contextual_match {
                let reason = if matches {
                    "keyword match with request".to_string()
                } else {
                    "contextual match with task type".to_string()
                };
                recommended.push((tool.to_string(), reason));
            }
        }

        let content = if recommended.is_empty() {
            "Tool planning: No specific tools strongly matched the request. \
             The task may be answerable through general knowledge."
                .to_string()
        } else {
            let tool_lines: Vec<String> = recommended
                .iter()
                .map(|(name, reason)| format!("- {name}: {reason}"))
                .collect();
            format!("Tool planning:\n{}", tool_lines.join("\n"))
        };

        ThinkingStep {
            step_type: ThinkingStepType::PlanTools,
            content,
            tokens_used: 0,
        }
    }

    /// Evaluate the approach by assessing confidence and risks.
    fn evaluate_approach(&self, user_message: &str, steps: &[ThinkingStep]) -> ThinkingStep {
        let complexity = estimate_complexity(user_message);
        let has_decomposition = steps
            .iter()
            .any(|s| s.step_type == ThinkingStepType::Decompose);
        let has_tools = steps
            .iter()
            .any(|s| s.step_type == ThinkingStepType::PlanTools);

        let mut risks = Vec::new();
        if complexity == "high" {
            risks.push("High complexity — may need iterative refinement");
        }
        if !has_tools {
            risks.push("No tool plan — response may lack actionable steps");
        }
        if user_message.contains('?') && user_message.matches('?').count() > 2 {
            risks.push("Multiple questions — risk of incomplete coverage");
        }

        let confidence = compute_base_confidence(user_message, has_decomposition, has_tools);

        let content = format!(
            "Approach evaluation:\n\
             - Complexity: {complexity}\n\
             - Decomposition available: {has_decomposition}\n\
             - Tool plan available: {has_tools}\n\
             - Risks: {}\n\
             - Confidence: {confidence:.2}",
            if risks.is_empty() {
                "None identified".to_string()
            } else {
                risks.join("; ")
            }
        );

        ThinkingStep {
            step_type: ThinkingStepType::Evaluate,
            content,
            tokens_used: 0,
        }
    }

    /// Synthesize all previous steps into a coherent plan.
    fn synthesize(&self, steps: &[ThinkingStep]) -> ThinkingStep {
        let mut plan_parts = Vec::new();

        for step in steps {
            match step.step_type {
                ThinkingStepType::Analyze => {
                    plan_parts.push(format!(
                        "Understanding: {}",
                        summarize_content(&step.content)
                    ));
                }
                ThinkingStepType::Decompose => {
                    plan_parts.push(format!("Breakdown: {}", summarize_content(&step.content)));
                }
                ThinkingStepType::PlanTools => {
                    plan_parts.push(format!(
                        "Tool strategy: {}",
                        summarize_content(&step.content)
                    ));
                }
                ThinkingStepType::Evaluate => {
                    plan_parts.push(format!(
                        "Risk assessment: {}",
                        summarize_content(&step.content)
                    ));
                }
                ThinkingStepType::Synthesize => {} // Skip self-references
            }
        }

        let content = format!(
            "Synthesized plan:\n{}",
            plan_parts
                .iter()
                .enumerate()
                .map(|(i, p)| format!("{}. {p}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        );

        ThinkingStep {
            step_type: ThinkingStepType::Synthesize,
            content,
            tokens_used: 0,
        }
    }

    /// Assemble a [`ThinkingResult`] from the collected steps.
    fn build_result(
        &self,
        steps: Vec<ThinkingStep>,
        total_tokens: usize,
        available_tools: &[&str],
        user_message: &str,
    ) -> ThinkingResult {
        // Extract subtasks from decompose step if present
        let subtasks = extract_subtasks_from_steps(&steps);

        // Extract recommended tools from plan step if present
        let recommended = extract_recommended_tools(&steps, available_tools);

        // Compute confidence
        let has_decomp = steps
            .iter()
            .any(|s| s.step_type == ThinkingStepType::Decompose);
        let has_tools = steps
            .iter()
            .any(|s| s.step_type == ThinkingStepType::PlanTools);
        let confidence = compute_base_confidence(user_message, has_decomp, has_tools);

        // Extract plan from synthesize step if present
        let plan = steps
            .iter()
            .find(|s| s.step_type == ThinkingStepType::Synthesize)
            .map(|s| s.content.clone());

        // Update tokens_used in each step
        let steps: Vec<ThinkingStep> = steps
            .into_iter()
            .map(|mut s| {
                s.tokens_used = estimate_tokens(&s.content);
                s
            })
            .collect();

        ThinkingResult {
            thinking_steps: steps,
            decomposed_subtasks: subtasks,
            confidence,
            total_thinking_tokens: total_tokens,
            recommended_tools: recommended,
            plan,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract keywords from text, filtering stopwords and short words.
fn extract_keywords(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .filter(|w| !STOPWORDS.contains(w))
        .map(String::from)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

/// Rough token estimation (~4 chars per token for English).
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Classify the broad intent of the user message.
fn classify_intent(message: &str) -> &str {
    let lower = message.to_lowercase();
    if lower.starts_with("how") || lower.contains("how to") || lower.contains("how do") {
        "procedural question"
    } else if lower.contains('?') {
        "information request"
    } else if lower.starts_with("create")
        || lower.starts_with("build")
        || lower.starts_with("make")
        || lower.starts_with("generate")
        || lower.starts_with("write")
    {
        "creation task"
    } else if lower.starts_with("fix")
        || lower.starts_with("debug")
        || lower.starts_with("solve")
        || lower.starts_with("resolve")
    {
        "troubleshooting"
    } else if lower.starts_with("explain")
        || lower.starts_with("describe")
        || lower.starts_with("what is")
    {
        "explanation request"
    } else if lower.starts_with("find")
        || lower.starts_with("search")
        || lower.starts_with("look")
        || lower.starts_with("locate")
    {
        "search task"
    } else {
        "general request"
    }
}

/// Estimate task complexity based on message characteristics.
fn estimate_complexity(message: &str) -> &str {
    let word_count = message.split_whitespace().count();
    let question_marks = message.matches('?').count();
    let has_code = message.contains("```") || message.contains("fn ") || message.contains("def ");

    if word_count > 100 || question_marks > 3 || has_code {
        "high"
    } else if word_count > 30 || question_marks > 1 {
        "medium"
    } else {
        "low"
    }
}

/// Check if a tool name contextually matches common task patterns.
fn match_tool_to_context(message_lower: &str, tool_name: &str) -> bool {
    let tool_lower = tool_name.to_lowercase();

    // File-related tasks
    if (message_lower.contains("file")
        || message_lower.contains("read")
        || message_lower.contains("write"))
        && (tool_lower.contains("file")
            || tool_lower.contains("read")
            || tool_lower.contains("write"))
    {
        return true;
    }

    // Web/HTTP tasks
    if (message_lower.contains("http")
        || message_lower.contains("fetch")
        || message_lower.contains("web")
        || message_lower.contains("url"))
        && (tool_lower.contains("http")
            || tool_lower.contains("fetch")
            || tool_lower.contains("web")
            || tool_lower.contains("browser"))
    {
        return true;
    }

    // Memory/search tasks
    if (message_lower.contains("remember")
        || message_lower.contains("recall")
        || message_lower.contains("search")
        || message_lower.contains("memory"))
        && (tool_lower.contains("memory") || tool_lower.contains("search"))
    {
        return true;
    }

    // Shell/command tasks
    if (message_lower.contains("run")
        || message_lower.contains("execute")
        || message_lower.contains("command")
        || message_lower.contains("shell"))
        && (tool_lower.contains("shell") || tool_lower.contains("exec"))
    {
        return true;
    }

    false
}

/// Compute a base confidence score for the thinking result.
fn compute_base_confidence(user_message: &str, has_decomposition: bool, has_tools: bool) -> f32 {
    let mut confidence = 0.5_f32;
    let complexity = estimate_complexity(user_message);

    match complexity {
        "low" => confidence += 0.2,
        "medium" => confidence += 0.1,
        "high" => confidence -= 0.1,
        _ => {}
    }

    if has_decomposition {
        confidence += 0.1;
    }
    if has_tools {
        confidence += 0.1;
    }

    // Longer, more specific messages give higher confidence
    let word_count = user_message.split_whitespace().count();
    if word_count > 10 {
        confidence += 0.05;
    }

    confidence.clamp(0.0, 1.0)
}

/// Extract subtasks from a Decompose step's content.
fn extract_subtasks_from_steps(steps: &[ThinkingStep]) -> Vec<String> {
    steps
        .iter()
        .filter(|s| s.step_type == ThinkingStepType::Decompose)
        .flat_map(|s| {
            s.content
                .lines()
                .filter(|l| l.starts_with("- Subtask"))
                .map(|l| l.trim_start_matches("- ").to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Extract recommended tool names from a PlanTools step.
fn extract_recommended_tools(steps: &[ThinkingStep], available_tools: &[&str]) -> Vec<String> {
    steps
        .iter()
        .filter(|s| s.step_type == ThinkingStepType::PlanTools)
        .flat_map(|s| {
            available_tools
                .iter()
                .filter(|tool| s.content.contains(**tool))
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Summarize content by taking the first meaningful line.
fn summarize_content(content: &str) -> String {
    content
        .lines()
        .find(|l| {
            !l.trim().is_empty()
                && !l.starts_with("Decomposition:")
                && !l.starts_with("Analysis")
                && !l.starts_with("Tool planning:")
                && !l.starts_with("Approach evaluation:")
                && !l.starts_with("Synthesized plan:")
        })
        .unwrap_or(content.lines().next().unwrap_or(""))
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn default_tools() -> Vec<&'static str> {
        vec![
            "file_read",
            "file_write",
            "http_fetch",
            "shell_exec",
            "memory_search",
            "browser_open",
        ]
    }

    // -----------------------------------------------------------------------
    // ThinkingDepth tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_thinking_depth_max_passes() {
        assert_eq!(ThinkingDepth::Quick.max_passes(), 1);
        assert_eq!(ThinkingDepth::Standard.max_passes(), 2);
        assert_eq!(ThinkingDepth::Deep.max_passes(), 3);
        assert_eq!(ThinkingDepth::Exhaustive.max_passes(), 5);
    }

    #[test]
    fn test_thinking_depth_token_budget() {
        assert_eq!(ThinkingDepth::Quick.token_budget(), 256);
        assert_eq!(ThinkingDepth::Standard.token_budget(), 1024);
        assert_eq!(ThinkingDepth::Deep.token_budget(), 2048);
        assert_eq!(ThinkingDepth::Exhaustive.token_budget(), 4096);
    }

    #[test]
    fn test_thinking_depth_serde_roundtrip() {
        let depth = ThinkingDepth::Deep;
        let json = serde_json::to_string(&depth).unwrap();
        let parsed: ThinkingDepth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, depth);
    }

    // -----------------------------------------------------------------------
    // ThinkingConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_thinking_config_defaults() {
        let config = ThinkingConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_thinking_tokens, 2048);
        assert!((config.thinking_budget_ratio - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.depth, ThinkingDepth::Standard);
        assert!(!config.show_thinking);
    }

    // -----------------------------------------------------------------------
    // ThinkingEngine basic tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_engine_with_defaults() {
        let engine = ThinkingEngine::with_defaults();
        assert!(engine.config().enabled);
        assert_eq!(engine.config().depth, ThinkingDepth::Standard);
    }

    #[test]
    fn test_think_disabled_returns_none() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            enabled: false,
            ..ThinkingConfig::default()
        });
        let result = engine.think("Hello world", &default_tools());
        assert!(result.is_none());
    }

    #[test]
    fn test_think_empty_message_returns_none() {
        let engine = ThinkingEngine::with_defaults();
        let result = engine.think("", &default_tools());
        assert!(result.is_none());
    }

    #[test]
    fn test_think_quick_produces_one_step() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Quick,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Read the config file", &default_tools())
            .unwrap();
        assert_eq!(result.thinking_steps.len(), 1);
        assert_eq!(
            result.thinking_steps[0].step_type,
            ThinkingStepType::Analyze
        );
    }

    #[test]
    fn test_think_standard_produces_two_steps() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Standard,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Read the config file", &default_tools())
            .unwrap();
        assert_eq!(result.thinking_steps.len(), 2);
        assert_eq!(
            result.thinking_steps[0].step_type,
            ThinkingStepType::Analyze
        );
        assert_eq!(
            result.thinking_steps[1].step_type,
            ThinkingStepType::PlanTools
        );
    }

    #[test]
    fn test_think_deep_produces_three_steps() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Deep,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think(
                "Read the config file and update the settings",
                &default_tools(),
            )
            .unwrap();
        assert_eq!(result.thinking_steps.len(), 3);
        assert_eq!(
            result.thinking_steps[0].step_type,
            ThinkingStepType::Analyze
        );
        assert_eq!(
            result.thinking_steps[1].step_type,
            ThinkingStepType::Decompose
        );
        assert_eq!(
            result.thinking_steps[2].step_type,
            ThinkingStepType::PlanTools
        );
    }

    #[test]
    fn test_think_exhaustive_produces_five_steps() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Exhaustive,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think(
                "Read the config file and update the settings",
                &default_tools(),
            )
            .unwrap();
        assert_eq!(result.thinking_steps.len(), 5);
        assert_eq!(
            result.thinking_steps[4].step_type,
            ThinkingStepType::Synthesize
        );
    }

    // -----------------------------------------------------------------------
    // Tool recommendation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_think_recommends_file_tools_for_file_task() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Standard,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Read the configuration file", &default_tools())
            .unwrap();
        assert!(
            result.recommended_tools.contains(&"file_read".to_string()),
            "file_read should be recommended for file reading tasks, got: {:?}",
            result.recommended_tools
        );
    }

    #[test]
    fn test_think_recommends_http_tools_for_web_task() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Standard,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Fetch data from the API URL endpoint", &default_tools())
            .unwrap();
        assert!(
            result.recommended_tools.contains(&"http_fetch".to_string()),
            "http_fetch should be recommended for web tasks, got: {:?}",
            result.recommended_tools
        );
    }

    #[test]
    fn test_think_recommends_shell_for_command_task() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Standard,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Run the build command in the shell", &default_tools())
            .unwrap();
        assert!(
            result.recommended_tools.contains(&"shell_exec".to_string()),
            "shell_exec should be recommended for shell tasks, got: {:?}",
            result.recommended_tools
        );
    }

    #[test]
    fn test_think_no_tools_available() {
        let engine = ThinkingEngine::with_defaults();
        let result = engine.think("Read a file", &[]).unwrap();
        assert!(result.recommended_tools.is_empty());
    }

    // -----------------------------------------------------------------------
    // Confidence tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_confidence_within_range() {
        let engine = ThinkingEngine::with_defaults();
        let result = engine
            .think("Hello, how are you?", &default_tools())
            .unwrap();
        assert!(result.confidence >= 0.0 && result.confidence <= 1.0);
    }

    #[test]
    fn test_simple_task_higher_confidence() {
        let engine = ThinkingEngine::with_defaults();
        let simple = engine.think("What time is it?", &default_tools()).unwrap();
        let complex = engine
            .think(
                "How do I set up a distributed system with Kafka, Redis, and PostgreSQL? \
                 What are the tradeoffs? How do I handle failover? What about monitoring?",
                &default_tools(),
            )
            .unwrap();
        assert!(
            simple.confidence >= complex.confidence,
            "Simple task should have >= confidence than complex one: {} vs {}",
            simple.confidence,
            complex.confidence
        );
    }

    // -----------------------------------------------------------------------
    // Decomposition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decompose_multi_sentence() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Deep,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think(
                "First read the file. Then parse the JSON. Finally update the database.",
                &default_tools(),
            )
            .unwrap();
        assert!(
            !result.decomposed_subtasks.is_empty(),
            "Multi-sentence input should produce subtasks"
        );
    }

    #[test]
    fn test_decompose_single_sentence() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Deep,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Read the config and update settings", &default_tools())
            .unwrap();
        // Should still attempt decomposition via conjunction splitting
        assert!(
            !result.decomposed_subtasks.is_empty(),
            "Conjunctions should produce subtasks"
        );
    }

    // -----------------------------------------------------------------------
    // Token estimation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_total_thinking_tokens_positive() {
        let engine = ThinkingEngine::with_defaults();
        let result = engine
            .think("Read a file for me please", &default_tools())
            .unwrap();
        assert!(
            result.total_thinking_tokens > 0,
            "Total thinking tokens should be positive"
        );
    }

    #[test]
    fn test_step_tokens_populated() {
        let engine = ThinkingEngine::with_defaults();
        let result = engine
            .think("Read a file for me please", &default_tools())
            .unwrap();
        for step in &result.thinking_steps {
            assert!(
                step.tokens_used > 0,
                "Each step should have tokens_used > 0"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Plan and synthesis tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_exhaustive_produces_plan() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Exhaustive,
            ..ThinkingConfig::default()
        });
        let result = engine
            .think("Read the file and process data", &default_tools())
            .unwrap();
        assert!(
            result.plan.is_some(),
            "Exhaustive thinking should produce a plan"
        );
    }

    #[test]
    fn test_quick_no_plan() {
        let engine = ThinkingEngine::new(ThinkingConfig {
            depth: ThinkingDepth::Quick,
            ..ThinkingConfig::default()
        });
        let result = engine.think("Hello", &default_tools()).unwrap();
        assert!(
            result.plan.is_none(),
            "Quick thinking should not produce a plan"
        );
    }

    // -----------------------------------------------------------------------
    // Prompt building tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_thinking_prompt_with_tools() {
        let engine = ThinkingEngine::with_defaults();
        let prompt = engine.build_thinking_prompt(&["file_read", "http_fetch"]);
        assert!(prompt.contains("file_read"));
        assert!(prompt.contains("http_fetch"));
        assert!(prompt.contains("ANALYZE"));
        assert!(prompt.contains("DECOMPOSE"));
    }

    #[test]
    fn test_build_thinking_prompt_no_tools() {
        let engine = ThinkingEngine::with_defaults();
        let prompt = engine.build_thinking_prompt(&[]);
        assert!(prompt.contains("No tools available"));
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_keywords() {
        let kw = extract_keywords("Read the configuration file from disk");
        assert!(kw.contains(&"read".to_string()));
        assert!(kw.contains(&"configuration".to_string()));
        assert!(kw.contains(&"file".to_string()));
        assert!(kw.contains(&"disk".to_string()));
        // Stopwords filtered
        assert!(!kw.contains(&"the".to_string()));
        assert!(!kw.contains(&"from".to_string()));
    }

    #[test]
    fn test_classify_intent_procedural() {
        assert_eq!(
            classify_intent("How to build a web server"),
            "procedural question"
        );
    }

    #[test]
    fn test_classify_intent_creation() {
        assert_eq!(classify_intent("Create a new project"), "creation task");
        assert_eq!(classify_intent("Build a REST API"), "creation task");
        assert_eq!(classify_intent("Write a function"), "creation task");
    }

    #[test]
    fn test_classify_intent_troubleshooting() {
        assert_eq!(classify_intent("Fix the broken test"), "troubleshooting");
        assert_eq!(classify_intent("Debug this error"), "troubleshooting");
    }

    #[test]
    fn test_classify_intent_search() {
        assert_eq!(classify_intent("Find all TODO comments"), "search task");
        assert_eq!(classify_intent("Search for the config file"), "search task");
    }

    #[test]
    fn test_classify_intent_general() {
        assert_eq!(classify_intent("Thanks for the help"), "general request");
    }

    #[test]
    fn test_estimate_complexity_low() {
        assert_eq!(estimate_complexity("Read a file"), "low");
    }

    #[test]
    fn test_estimate_complexity_medium() {
        assert_eq!(
            estimate_complexity(
                "How do I set up authentication and authorization for my web application? \
                 What are the best practices for handling sessions and cookies in a modern \
                 distributed system with multiple backend services?"
            ),
            "medium"
        );
    }

    #[test]
    fn test_estimate_complexity_high() {
        let long_msg = "word ".repeat(101);
        assert_eq!(estimate_complexity(&long_msg), "high");
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("test"), 1);
        assert!(estimate_tokens("hello world this is a test") > 0);
    }

    #[test]
    fn test_match_tool_to_context_file() {
        assert!(match_tool_to_context("read the file contents", "file_read"));
        assert!(!match_tool_to_context(
            "read the file contents",
            "http_fetch"
        ));
    }

    #[test]
    fn test_match_tool_to_context_web() {
        assert!(match_tool_to_context(
            "fetch data from the url",
            "http_fetch"
        ));
        assert!(!match_tool_to_context(
            "fetch data from the url",
            "shell_exec"
        ));
    }

    #[test]
    fn test_match_tool_to_context_shell() {
        assert!(match_tool_to_context("run this command", "shell_exec"));
    }

    #[test]
    fn test_match_tool_to_context_memory() {
        assert!(match_tool_to_context(
            "search my memory for context",
            "memory_search"
        ));
    }

    #[test]
    fn test_thinking_result_serde_roundtrip() {
        let result = ThinkingResult {
            thinking_steps: vec![ThinkingStep {
                step_type: ThinkingStepType::Analyze,
                content: "test analysis".to_string(),
                tokens_used: 10,
            }],
            decomposed_subtasks: vec!["sub1".to_string()],
            confidence: 0.75,
            total_thinking_tokens: 10,
            recommended_tools: vec!["file_read".to_string()],
            plan: Some("do the thing".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ThinkingResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.thinking_steps.len(), 1);
        assert!((parsed.confidence - 0.75).abs() < f32::EPSILON);
        assert_eq!(parsed.recommended_tools, vec!["file_read"]);
        assert_eq!(parsed.plan, Some("do the thing".to_string()));
    }

    #[test]
    fn test_thinking_step_type_serde_roundtrip() {
        let types = vec![
            ThinkingStepType::Analyze,
            ThinkingStepType::Decompose,
            ThinkingStepType::PlanTools,
            ThinkingStepType::Evaluate,
            ThinkingStepType::Synthesize,
        ];
        for t in types {
            let json = serde_json::to_string(&t).unwrap();
            let parsed: ThinkingStepType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, t);
        }
    }
}

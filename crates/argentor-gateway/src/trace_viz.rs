//! Enhanced trace visualization for visual debugging.
//!
//! Builds on [`argentor_agent::debug_recorder`] to produce structured data
//! for timeline views, flame graphs, and Mermaid gantt charts.
//!
//! # Main types
//!
//! - [`TraceVisualizer`] — Converts a [`DebugTrace`] into a [`VisualTrace`].
//! - [`VisualTrace`] — Rich trace representation with timeline and cost data.
//! - [`VisualStep`] — A single step with timing, tokens, cost, and nesting.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use argentor_agent::debug_recorder::{DebugTrace, StepType};

// ---------------------------------------------------------------------------
// Cost model (same constants used in trace_viewer.rs)
// ---------------------------------------------------------------------------

/// Cost per 1 000 input tokens (USD).
const INPUT_COST_PER_1K: f64 = 0.003;
/// Cost per 1 000 output tokens (USD).
const OUTPUT_COST_PER_1K: f64 = 0.015;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the trace visualizer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceVizConfig {
    /// Maximum nesting depth for visual steps (default: 10).
    pub max_trace_depth: usize,
    /// Whether to include token cost estimates per step.
    pub include_token_costs: bool,
    /// Whether to include timing information per step.
    pub include_timing: bool,
    /// Whether to collapse large tool results in the output.
    pub collapse_tool_results: bool,
}

impl Default for TraceVizConfig {
    fn default() -> Self {
        Self {
            max_trace_depth: 10,
            include_token_costs: true,
            include_timing: true,
            collapse_tool_results: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Visual types
// ---------------------------------------------------------------------------

/// A fully visualized trace with timeline, steps, and summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualTrace {
    /// Trace identifier.
    pub trace_id: String,
    /// Agent name (from metadata, if available).
    pub agent_name: String,
    /// When the trace started.
    pub start_time: DateTime<Utc>,
    /// When the trace ended.
    pub end_time: DateTime<Utc>,
    /// Total trace duration in milliseconds.
    pub total_duration_ms: u64,
    /// Total tokens consumed.
    pub total_tokens: usize,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Visual steps (flat or with nested children).
    pub steps: Vec<VisualStep>,
    /// Timeline entries for event-based visualization.
    pub timeline: Vec<TimelineEntry>,
    /// Aggregate summary statistics.
    pub summary: TraceSummaryViz,
}

/// A single visual step with timing, cost, and optional children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualStep {
    /// Step sequence ID.
    pub id: usize,
    /// Classification of the step (e.g. "llm_call", "tool_call").
    pub step_type: String,
    /// Human-readable label.
    pub label: String,
    /// Offset from trace start in milliseconds.
    pub start_ms: u64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Tokens consumed in this step.
    pub tokens: usize,
    /// Estimated cost in USD for this step.
    pub cost_usd: f64,
    /// Status of this step.
    pub status: StepStatus,
    /// Nested child steps (e.g. tool calls within an LLM call).
    pub children: Vec<VisualStep>,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Status of a visual step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum StepStatus {
    /// Step completed successfully.
    Success,
    /// Step resulted in an error.
    Error { message: String },
    /// Step was served from cache.
    Cached,
    /// Step was skipped.
    Skipped { reason: String },
}

/// A single timeline entry for event-based views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// Offset from trace start in milliseconds.
    pub timestamp_ms: u64,
    /// Type of event.
    pub event_type: String,
    /// Human-readable description.
    pub description: String,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Aggregate summary statistics for a visual trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSummaryViz {
    /// Total number of LLM calls.
    pub total_llm_calls: usize,
    /// Total number of tool calls.
    pub total_tool_calls: usize,
    /// Cache hit rate (0.0 — 1.0).
    pub cache_hit_rate: f32,
    /// Average step duration in milliseconds.
    pub avg_step_duration_ms: f64,
    /// Label of the most expensive step (by cost).
    pub most_expensive_step: String,
    /// Number of error steps.
    pub error_count: usize,
    /// Tokens used for thinking/reasoning steps.
    pub thinking_tokens: usize,
    /// Tokens used for action steps (tool calls, output).
    pub action_tokens: usize,
}

// ---------------------------------------------------------------------------
// TraceVisualizer
// ---------------------------------------------------------------------------

/// Converts [`DebugTrace`] into [`VisualTrace`] with timeline and cost data.
#[derive(Debug, Clone)]
pub struct TraceVisualizer {
    config: TraceVizConfig,
}

impl TraceVisualizer {
    /// Create a new visualizer with the given config.
    pub fn new(config: TraceVizConfig) -> Self {
        Self { config }
    }

    /// Create a visualizer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(TraceVizConfig::default())
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &TraceVizConfig {
        &self.config
    }

    // ----- Core conversion --------------------------------------------------

    /// Convert a [`DebugTrace`] into a [`VisualTrace`].
    pub fn visualize(&self, trace: &DebugTrace) -> VisualTrace {
        let start_time = trace.started_at;
        let end_time = trace.ended_at.unwrap_or_else(Utc::now);
        let total_duration_ms = trace.total_duration_ms.unwrap_or(0);
        let total_tokens = (trace.total_tokens.input + trace.total_tokens.output) as usize;
        let total_cost_usd = compute_cost(trace.total_tokens.input, trace.total_tokens.output);

        let agent_name = trace
            .metadata
            .get("agent_role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Build visual steps
        let steps = self.build_visual_steps(trace);

        // Build timeline
        let timeline = self.build_timeline(trace);

        // Build summary
        let summary = self.build_summary(&steps, total_duration_ms);

        VisualTrace {
            trace_id: trace.trace_id.clone(),
            agent_name,
            start_time,
            end_time,
            total_duration_ms,
            total_tokens,
            total_cost_usd,
            steps,
            timeline,
            summary,
        }
    }

    /// Serialize a visual trace to JSON.
    pub fn to_json(trace: &VisualTrace) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(trace)
    }

    /// Generate a Mermaid gantt chart from a visual trace.
    pub fn to_mermaid_gantt(trace: &VisualTrace) -> String {
        let mut lines = Vec::new();
        lines.push("gantt".to_string());
        lines.push(format!("    title Trace: {}", trace.trace_id));
        lines.push("    dateFormat x".to_string());
        lines.push("    axisFormat %S.%L s".to_string());

        // Group steps by type for sections
        let mut current_section = String::new();

        for step in &trace.steps {
            let section = categorize_step_type(&step.step_type);
            if section != current_section {
                lines.push(format!("    section {section}"));
                current_section = section;
            }

            let label = sanitize_mermaid_label(&step.label);
            let status_marker = match &step.status {
                StepStatus::Success => "",
                StepStatus::Error { .. } => "crit, ",
                StepStatus::Cached => "done, ",
                StepStatus::Skipped { .. } => "done, ",
            };

            lines.push(format!(
                "    {label} :{status_marker}{start}, {duration}ms",
                start = step.start_ms,
                duration = step.duration_ms,
            ));

            // Add children at one indent deeper
            for child in &step.children {
                let child_label = sanitize_mermaid_label(&child.label);
                lines.push(format!(
                    "    {child_label} :{start}, {duration}ms",
                    start = child.start_ms,
                    duration = child.duration_ms,
                ));
            }
        }

        lines.join("\n")
    }

    /// Generate a simple flame graph text representation.
    ///
    /// Each line is: `stack_path duration_ms`
    /// where `stack_path` is semicolon-separated ancestors.
    pub fn to_flame_graph(trace: &VisualTrace) -> String {
        let mut lines = Vec::new();

        for step in &trace.steps {
            let label = step.label.replace(';', "_");
            lines.push(format!("{} {}", label, step.duration_ms));

            for child in &step.children {
                let child_label = child.label.replace(';', "_");
                lines.push(format!("{};{} {}", label, child_label, child.duration_ms));
            }
        }

        lines.join("\n")
    }

    // ----- Internal ---------------------------------------------------------

    fn build_visual_steps(&self, trace: &DebugTrace) -> Vec<VisualStep> {
        let trace_start = trace.started_at;
        let mut steps = Vec::new();

        // Track LLM call context for nesting tool calls inside LLM steps
        let mut current_llm_step: Option<VisualStep> = None;

        for debug_step in &trace.steps {
            let offset_ms = (debug_step.timestamp - trace_start)
                .num_milliseconds()
                .unsigned_abs();
            let duration_ms = debug_step.duration_ms.unwrap_or(0);

            let (tokens, cost) = if self.config.include_token_costs {
                if let Some(ref tok) = debug_step.tokens {
                    let t = (tok.input + tok.output) as usize;
                    let c = compute_cost(tok.input, tok.output);
                    (t, c)
                } else {
                    (0, 0.0)
                }
            } else {
                (0, 0.0)
            };

            let status = match &debug_step.step_type {
                StepType::Error => StepStatus::Error {
                    message: debug_step.description.clone(),
                },
                StepType::CacheHit => StepStatus::Cached,
                _ => StepStatus::Success,
            };

            let step_type = step_type_to_string(&debug_step.step_type);

            let visual_step = VisualStep {
                id: debug_step.seq as usize,
                step_type: step_type.clone(),
                label: debug_step.description.clone(),
                start_ms: if self.config.include_timing {
                    offset_ms
                } else {
                    0
                },
                duration_ms: if self.config.include_timing {
                    duration_ms
                } else {
                    0
                },
                tokens,
                cost_usd: cost,
                status,
                children: Vec::new(),
                metadata: debug_step
                    .data
                    .as_ref()
                    .map(|d| {
                        let mut m = HashMap::new();
                        m.insert("data".to_string(), d.clone());
                        m
                    })
                    .unwrap_or_default(),
            };

            // Nest tool_call / tool_result under the preceding LLM call
            match debug_step.step_type {
                StepType::LlmCall => {
                    // Flush any previous LLM step
                    if let Some(prev) = current_llm_step.take() {
                        steps.push(prev);
                    }
                    current_llm_step = Some(visual_step);
                }
                StepType::ToolCall | StepType::ToolResult => {
                    if let Some(ref mut llm_step) = current_llm_step {
                        if llm_step.children.len() < self.config.max_trace_depth {
                            llm_step.children.push(visual_step);
                        }
                    } else {
                        steps.push(visual_step);
                    }
                }
                _ => {
                    // Flush LLM step before adding a non-tool step
                    if let Some(prev) = current_llm_step.take() {
                        steps.push(prev);
                    }
                    steps.push(visual_step);
                }
            }
        }

        // Flush final LLM step
        if let Some(prev) = current_llm_step.take() {
            steps.push(prev);
        }

        steps
    }

    fn build_timeline(&self, trace: &DebugTrace) -> Vec<TimelineEntry> {
        let trace_start = trace.started_at;

        trace
            .steps
            .iter()
            .map(|step| {
                let offset_ms = (step.timestamp - trace_start)
                    .num_milliseconds()
                    .unsigned_abs();

                TimelineEntry {
                    timestamp_ms: offset_ms,
                    event_type: step_type_to_string(&step.step_type),
                    description: step.description.clone(),
                    metadata: step
                        .data
                        .as_ref()
                        .map(|d| {
                            let mut m = HashMap::new();
                            m.insert("data".to_string(), d.clone());
                            m
                        })
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    fn build_summary(&self, steps: &[VisualStep], total_duration_ms: u64) -> TraceSummaryViz {
        let all_steps = flatten_steps(steps);

        let total_llm_calls = all_steps
            .iter()
            .filter(|s| s.step_type == "llm_call")
            .count();
        let total_tool_calls = all_steps
            .iter()
            .filter(|s| s.step_type == "tool_call")
            .count();
        let cache_hits = all_steps
            .iter()
            .filter(|s| s.step_type == "cache_hit")
            .count();
        let error_count = all_steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Error { .. }))
            .count();

        let total_calls = total_llm_calls + cache_hits;
        let cache_hit_rate = if total_calls > 0 {
            cache_hits as f32 / total_calls as f32
        } else {
            0.0
        };

        let step_durations: Vec<u64> = all_steps.iter().map(|s| s.duration_ms).collect();
        let avg_step_duration_ms = if step_durations.is_empty() {
            0.0
        } else {
            step_durations.iter().sum::<u64>() as f64 / step_durations.len() as f64
        };

        let most_expensive_step = all_steps
            .iter()
            .max_by(|a, b| {
                a.cost_usd
                    .partial_cmp(&b.cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|s| s.label.clone())
            .unwrap_or_default();

        let thinking_tokens: usize = all_steps
            .iter()
            .filter(|s| s.step_type == "thinking" || s.step_type == "decision")
            .map(|s| s.tokens)
            .sum();

        let action_tokens: usize = all_steps
            .iter()
            .filter(|s| {
                s.step_type == "tool_call"
                    || s.step_type == "tool_result"
                    || s.step_type == "output"
            })
            .map(|s| s.tokens)
            .sum();

        // Avoid unused variable warning
        let _ = total_duration_ms;

        TraceSummaryViz {
            total_llm_calls,
            total_tool_calls,
            cache_hit_rate,
            avg_step_duration_ms,
            most_expensive_step,
            error_count,
            thinking_tokens,
            action_tokens,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_cost(input_tokens: u64, output_tokens: u64) -> f64 {
    let input_cost = input_tokens as f64 / 1000.0 * INPUT_COST_PER_1K;
    let output_cost = output_tokens as f64 / 1000.0 * OUTPUT_COST_PER_1K;
    input_cost + output_cost
}

fn step_type_to_string(st: &StepType) -> String {
    match st {
        StepType::Input => "input".to_string(),
        StepType::Thinking => "thinking".to_string(),
        StepType::Decision => "decision".to_string(),
        StepType::ToolCall => "tool_call".to_string(),
        StepType::ToolResult => "tool_result".to_string(),
        StepType::LlmCall => "llm_call".to_string(),
        StepType::LlmResponse => "llm_response".to_string(),
        StepType::CacheHit => "cache_hit".to_string(),
        StepType::Error => "error".to_string(),
        StepType::Output => "output".to_string(),
        StepType::Custom(name) => name.clone(),
    }
}

fn categorize_step_type(step_type: &str) -> String {
    match step_type {
        "llm_call" | "llm_response" | "cache_hit" => "LLM".to_string(),
        "tool_call" | "tool_result" => "Tools".to_string(),
        "thinking" | "decision" => "Reasoning".to_string(),
        "input" | "output" => "IO".to_string(),
        "error" => "Errors".to_string(),
        other => other.to_string(),
    }
}

fn sanitize_mermaid_label(label: &str) -> String {
    // Mermaid gantt labels cannot contain certain chars
    label
        .replace([':', ';', '#'], "-")
        .replace('\n', " ")
        .chars()
        .take(50)
        .collect()
}

/// Flatten nested steps into a single list (parent + children).
fn flatten_steps(steps: &[VisualStep]) -> Vec<&VisualStep> {
    let mut result = Vec::new();
    for step in steps {
        result.push(step);
        for child in &step.children {
            result.push(child);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_agent::debug_recorder::{DebugRecorder, StepType, TokenUsage};

    // -- helpers --

    fn make_simple_trace() -> DebugTrace {
        let r = DebugRecorder::new("trace-viz-1");
        r.set_metadata("agent_role", serde_json::json!("coder"));
        r.record(StepType::Input, "User message", None);
        r.record_with_metrics(StepType::LlmCall, "Claude call", 500, 1000, 200);
        r.record(StepType::Output, "Response", None);
        r.finalize()
    }

    fn make_complex_trace() -> DebugTrace {
        let r = DebugRecorder::new("trace-viz-2");
        r.set_metadata("agent_role", serde_json::json!("orchestrator"));
        r.set_metadata("session_id", serde_json::json!("sess-42"));

        r.record(StepType::Input, "Complex task", None);
        r.record_with_metrics(StepType::Thinking, "Analyzing requirements", 100, 200, 50);
        r.record_with_metrics(StepType::LlmCall, "First LLM call", 300, 500, 100);
        r.record(
            StepType::ToolCall,
            "file_read /src/main.rs",
            Some(serde_json::json!({"path": "/src/main.rs"})),
        );
        r.record(StepType::ToolResult, "File contents returned", None);
        r.record_with_metrics(StepType::LlmCall, "Second LLM call", 400, 800, 200);
        r.record(StepType::CacheHit, "Cached response", None);
        r.record(StepType::Error, "Rate limit hit", None);
        r.record_with_metrics(StepType::LlmCall, "Retry LLM call", 600, 900, 300);
        r.record(StepType::Decision, "Selected approach A", None);
        r.record(StepType::Output, "Final response", None);

        r.finalize()
    }

    fn make_empty_trace() -> DebugTrace {
        let r = DebugRecorder::new("trace-empty");
        r.finalize()
    }

    // 1. Default config
    #[test]
    fn test_default_config() {
        let cfg = TraceVizConfig::default();
        assert_eq!(cfg.max_trace_depth, 10);
        assert!(cfg.include_token_costs);
        assert!(cfg.include_timing);
        assert!(!cfg.collapse_tool_results);
    }

    // 2. Visualize simple trace
    #[test]
    fn test_visualize_simple() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        assert_eq!(visual.trace_id, "trace-viz-1");
        assert_eq!(visual.agent_name, "coder");
        assert!(!visual.steps.is_empty());
        assert!(visual.total_tokens > 0);
    }

    // 3. Visualize complex trace
    #[test]
    fn test_visualize_complex() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        assert_eq!(visual.trace_id, "trace-viz-2");
        assert_eq!(visual.agent_name, "orchestrator");
        assert!(visual.steps.len() >= 3);
    }

    // 4. Visualize empty trace
    #[test]
    fn test_visualize_empty() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_empty_trace();
        let visual = viz.visualize(&trace);

        assert!(visual.steps.is_empty());
        assert!(visual.timeline.is_empty());
        assert_eq!(visual.total_tokens, 0);
    }

    // 5. Timeline entries match step count
    #[test]
    fn test_timeline_entries() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        // Timeline should have one entry per debug step
        assert_eq!(visual.timeline.len(), 3);
    }

    // 6. Summary — LLM call count
    #[test]
    fn test_summary_llm_calls() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        assert!(visual.summary.total_llm_calls >= 1);
    }

    // 7. Summary — tool call count
    #[test]
    fn test_summary_tool_calls() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        assert!(visual.summary.total_tool_calls >= 1);
    }

    // 8. Summary — error count
    #[test]
    fn test_summary_error_count() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        assert!(visual.summary.error_count >= 1);
    }

    // 9. Summary — cache hit rate
    #[test]
    fn test_summary_cache_hit_rate() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        // There is at least 1 cache hit and some LLM calls
        assert!(visual.summary.cache_hit_rate > 0.0);
        assert!(visual.summary.cache_hit_rate <= 1.0);
    }

    // 10. to_json produces valid JSON
    #[test]
    fn test_to_json() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        let json = TraceVisualizer::to_json(&visual).unwrap();
        assert!(json.contains("trace-viz-1"));
        // Verify it parses back
        let _parsed: VisualTrace = serde_json::from_str(&json).unwrap();
    }

    // 11. to_mermaid_gantt produces valid gantt syntax
    #[test]
    fn test_to_mermaid_gantt() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        let gantt = TraceVisualizer::to_mermaid_gantt(&visual);
        assert!(gantt.starts_with("gantt"));
        assert!(gantt.contains("title Trace:"));
        assert!(gantt.contains("dateFormat x"));
    }

    // 12. to_flame_graph produces lines
    #[test]
    fn test_to_flame_graph() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        let flame = TraceVisualizer::to_flame_graph(&visual);
        assert!(!flame.is_empty());
        // Each line should have a label and a number
        for line in flame.lines() {
            let parts: Vec<&str> = line.rsplitn(2, ' ').collect();
            assert_eq!(parts.len(), 2);
            assert!(parts[0].parse::<u64>().is_ok());
        }
    }

    // 13. Tool calls nested under LLM call
    #[test]
    fn test_tool_calls_nested() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        // Find the first LLM call step that has children
        let llm_with_children = visual
            .steps
            .iter()
            .find(|s| s.step_type == "llm_call" && !s.children.is_empty());

        assert!(
            llm_with_children.is_some(),
            "Should have LLM step with tool call children"
        );
    }

    // 14. Step status — error
    #[test]
    fn test_step_status_error() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        let all = flatten_steps(&visual.steps);
        let error_step = all
            .iter()
            .find(|s| matches!(s.status, StepStatus::Error { .. }));
        assert!(error_step.is_some());
    }

    // 15. Step status — cached
    #[test]
    fn test_step_status_cached() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        let all = flatten_steps(&visual.steps);
        let cached_step = all.iter().find(|s| s.status == StepStatus::Cached);
        assert!(cached_step.is_some());
    }

    // 16. Cost calculation
    #[test]
    fn test_cost_calculation() {
        let cost = compute_cost(1000, 200);
        let expected = 1000.0 / 1000.0 * 0.003 + 200.0 / 1000.0 * 0.015;
        assert!((cost - expected).abs() < 1e-10);
    }

    // 17. Cost is zero for zero tokens
    #[test]
    fn test_cost_zero_tokens() {
        assert_eq!(compute_cost(0, 0), 0.0);
    }

    // 18. Total cost in visual trace
    #[test]
    fn test_total_cost_in_trace() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        assert!(visual.total_cost_usd > 0.0);
    }

    // 19. Config without timing zeros out start_ms and duration_ms
    #[test]
    fn test_no_timing() {
        let cfg = TraceVizConfig {
            include_timing: false,
            ..Default::default()
        };
        let viz = TraceVisualizer::new(cfg);
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        for step in &visual.steps {
            assert_eq!(step.start_ms, 0);
            assert_eq!(step.duration_ms, 0);
        }
    }

    // 20. Config without token costs zeros out tokens and cost
    #[test]
    fn test_no_token_costs() {
        let cfg = TraceVizConfig {
            include_token_costs: false,
            ..Default::default()
        };
        let viz = TraceVisualizer::new(cfg);
        let trace = make_simple_trace();
        let visual = viz.visualize(&trace);

        for step in &visual.steps {
            assert_eq!(step.tokens, 0);
            assert_eq!(step.cost_usd, 0.0);
        }
    }

    // 21. VisualTrace serialization roundtrip
    #[test]
    fn test_visual_trace_serialization() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        let json = serde_json::to_string(&visual).unwrap();
        let restored: VisualTrace = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.trace_id, visual.trace_id);
        assert_eq!(restored.steps.len(), visual.steps.len());
    }

    // 22. StepStatus serialization
    #[test]
    fn test_step_status_serialization() {
        let statuses = vec![
            StepStatus::Success,
            StepStatus::Error {
                message: "oops".to_string(),
            },
            StepStatus::Cached,
            StepStatus::Skipped {
                reason: "not needed".to_string(),
            },
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let restored: StepStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, status);
        }
    }

    // 23. TimelineEntry serialization
    #[test]
    fn test_timeline_entry_serialization() {
        let entry = TimelineEntry {
            timestamp_ms: 42,
            event_type: "llm_call".to_string(),
            description: "test".to_string(),
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: TimelineEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.timestamp_ms, 42);
    }

    // 24. TraceSummaryViz serialization
    #[test]
    fn test_summary_viz_serialization() {
        let summary = TraceSummaryViz {
            total_llm_calls: 3,
            total_tool_calls: 5,
            cache_hit_rate: 0.25,
            avg_step_duration_ms: 150.5,
            most_expensive_step: "big call".to_string(),
            error_count: 1,
            thinking_tokens: 200,
            action_tokens: 800,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let restored: TraceSummaryViz = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.total_llm_calls, 3);
    }

    // 25. sanitize_mermaid_label handles special chars
    #[test]
    fn test_sanitize_mermaid_label() {
        assert_eq!(sanitize_mermaid_label("hello:world"), "hello-world");
        assert_eq!(sanitize_mermaid_label("a;b#c"), "a-b-c");
        let long_label = "a".repeat(100);
        assert_eq!(sanitize_mermaid_label(&long_label).len(), 50);
    }

    // 26. categorize_step_type
    #[test]
    fn test_categorize_step_type() {
        assert_eq!(categorize_step_type("llm_call"), "LLM");
        assert_eq!(categorize_step_type("tool_call"), "Tools");
        assert_eq!(categorize_step_type("thinking"), "Reasoning");
        assert_eq!(categorize_step_type("input"), "IO");
        assert_eq!(categorize_step_type("error"), "Errors");
        assert_eq!(categorize_step_type("custom_thing"), "custom_thing");
    }

    // 27. step_type_to_string covers all variants
    #[test]
    fn test_step_type_to_string() {
        let cases = vec![
            (StepType::Input, "input"),
            (StepType::Thinking, "thinking"),
            (StepType::Decision, "decision"),
            (StepType::ToolCall, "tool_call"),
            (StepType::ToolResult, "tool_result"),
            (StepType::LlmCall, "llm_call"),
            (StepType::LlmResponse, "llm_response"),
            (StepType::CacheHit, "cache_hit"),
            (StepType::Error, "error"),
            (StepType::Output, "output"),
            (StepType::Custom("my_step".to_string()), "my_step"),
        ];
        for (st, expected) in cases {
            assert_eq!(step_type_to_string(&st), expected);
        }
    }

    // 28. flatten_steps works with children
    #[test]
    fn test_flatten_steps() {
        let child = VisualStep {
            id: 2,
            step_type: "tool_call".to_string(),
            label: "child".to_string(),
            start_ms: 10,
            duration_ms: 5,
            tokens: 0,
            cost_usd: 0.0,
            status: StepStatus::Success,
            children: vec![],
            metadata: HashMap::new(),
        };
        let parent = VisualStep {
            id: 1,
            step_type: "llm_call".to_string(),
            label: "parent".to_string(),
            start_ms: 0,
            duration_ms: 100,
            tokens: 50,
            cost_usd: 0.01,
            status: StepStatus::Success,
            children: vec![child],
            metadata: HashMap::new(),
        };

        let steps = [parent];
        let flat = flatten_steps(&steps);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].label, "parent");
        assert_eq!(flat[1].label, "child");
    }

    // 29. Mermaid gantt with error step shows crit marker
    #[test]
    fn test_mermaid_gantt_with_error() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        let gantt = TraceVisualizer::to_mermaid_gantt(&visual);
        assert!(
            gantt.contains("crit,"),
            "Error steps should have crit marker"
        );
    }

    // 30. Flame graph with nested steps uses semicolon separator
    #[test]
    fn test_flame_graph_nested() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        let flame = TraceVisualizer::to_flame_graph(&visual);
        // At least one line should contain a semicolon (parent;child)
        let has_nested = flame.lines().any(|l| l.contains(';'));
        assert!(has_nested, "Flame graph should have nested entries");
    }

    // 31. Most expensive step is identified
    #[test]
    fn test_most_expensive_step() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        assert!(
            !visual.summary.most_expensive_step.is_empty(),
            "Most expensive step should be identified"
        );
    }

    // 32. Thinking and action tokens are tracked
    #[test]
    fn test_thinking_action_tokens() {
        let viz = TraceVisualizer::with_defaults();
        let trace = make_complex_trace();
        let visual = viz.visualize(&trace);

        // Complex trace has thinking steps with tokens
        assert!(
            visual.summary.thinking_tokens > 0,
            "Should have thinking tokens"
        );
    }

    // 33. TraceVizConfig serialization
    #[test]
    fn test_config_serialization() {
        let cfg = TraceVizConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: TraceVizConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_trace_depth, 10);
        assert!(restored.include_token_costs);
    }
}

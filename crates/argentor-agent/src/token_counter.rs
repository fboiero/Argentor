//! Token counting and cost estimation for different LLM providers.
//!
//! Provides character-based heuristic token estimation (no external tokenizer
//! dependency) and cumulative usage tracking with per-provider cost breakdowns.

use crate::config::LlmProvider;
use argentor_core::Message;
use argentor_skills::SkillDescriptor;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Estimated token counts and optional cost for a single request or accumulated usage.
#[derive(Debug, Clone, Default)]
pub struct TokenEstimate {
    /// Estimated number of input tokens.
    pub input_tokens: u64,
    /// Estimated number of output tokens.
    pub output_tokens: u64,
    /// Total estimated tokens (input + output).
    pub total_tokens: u64,
    /// Estimated cost in USD, if pricing data is available.
    pub estimated_cost_usd: Option<f64>,
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone)]
pub struct ModelPricing {
    /// Price in USD per 1 000 input tokens.
    pub input_price_per_1k: f64,
    /// Price in USD per 1 000 output tokens.
    pub output_price_per_1k: f64,
}

/// Heuristic token counter that estimates token counts using character-based
/// ratios calibrated for each LLM provider family.
#[derive(Debug, Clone)]
pub struct TokenCounter;

impl TokenCounter {
    /// Creates a new `TokenCounter`.
    pub fn new() -> Self {
        Self
    }

    // ----- core estimation ---------------------------------------------------

    /// Estimate the number of tokens in `text` for the given `provider`.
    ///
    /// Uses a character-to-token ratio that varies by provider:
    /// - Claude: ~4.5 characters per token (more conservative)
    /// - OpenAI / GPT-compatible: ~4 characters per token
    /// - Gemini: ~4 characters per token
    /// - Others: ~4 characters per token (safe default)
    pub fn estimate_tokens(&self, text: &str, provider: &LlmProvider) -> u64 {
        let char_count = text.len() as f64;
        let chars_per_token = Self::chars_per_token(provider);
        // Always return at least 1 token for non-empty input.
        if text.is_empty() {
            0
        } else {
            (char_count / chars_per_token).ceil() as u64
        }
    }

    /// Estimate total input tokens for a slice of conversation messages.
    ///
    /// Adds a small per-message overhead (4 tokens) to account for role tags
    /// and separator tokens used by most providers.
    pub fn estimate_message_tokens(&self, messages: &[Message], provider: &LlmProvider) -> u64 {
        let per_message_overhead: u64 = 4; // role, separators
        messages
            .iter()
            .map(|m| self.estimate_tokens(&m.content, provider) + per_message_overhead)
            .sum()
    }

    /// Estimate the token cost of including tool/skill descriptors in the prompt.
    ///
    /// Each descriptor contributes its name, description, and JSON schema
    /// serialisation, plus a small overhead for structural formatting.
    pub fn estimate_tool_tokens(&self, tools: &[SkillDescriptor], provider: &LlmProvider) -> u64 {
        let per_tool_overhead: u64 = 8; // structural tokens
        tools
            .iter()
            .map(|t| {
                let schema_text = serde_json::to_string(&t.parameters_schema).unwrap_or_default();
                self.estimate_tokens(&t.name, provider)
                    + self.estimate_tokens(&t.description, provider)
                    + self.estimate_tokens(&schema_text, provider)
                    + per_tool_overhead
            })
            .sum()
    }

    // ----- cost estimation ---------------------------------------------------

    /// Estimate the cost in USD for a request given token counts, provider, and
    /// model identifier.
    ///
    /// Returns `None` if no pricing data is available for the model.
    pub fn estimate_cost(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        provider: &LlmProvider,
        model_id: &str,
    ) -> Option<f64> {
        let pricing = self.default_pricing(provider, model_id)?;
        let input_cost = (input_tokens as f64 / 1_000.0) * pricing.input_price_per_1k;
        let output_cost = (output_tokens as f64 / 1_000.0) * pricing.output_price_per_1k;
        Some(input_cost + output_cost)
    }

    /// Return the default [`ModelPricing`] for a given provider and model.
    ///
    /// Prices are approximate 2025 public list prices.
    pub fn default_pricing(&self, provider: &LlmProvider, model_id: &str) -> Option<ModelPricing> {
        let model_lower = model_id.to_lowercase();
        match provider {
            LlmProvider::Claude | LlmProvider::ClaudeCode => {
                if model_lower.contains("opus") {
                    // $15 / $75 per 1M tokens
                    Some(ModelPricing {
                        input_price_per_1k: 0.015,
                        output_price_per_1k: 0.075,
                    })
                } else if model_lower.contains("sonnet") {
                    // $3 / $15 per 1M tokens
                    Some(ModelPricing {
                        input_price_per_1k: 0.003,
                        output_price_per_1k: 0.015,
                    })
                } else if model_lower.contains("haiku") {
                    // $0.25 / $1.25 per 1M tokens
                    Some(ModelPricing {
                        input_price_per_1k: 0.000_25,
                        output_price_per_1k: 0.001_25,
                    })
                } else {
                    // Default Claude pricing (sonnet-tier)
                    Some(ModelPricing {
                        input_price_per_1k: 0.003,
                        output_price_per_1k: 0.015,
                    })
                }
            }
            LlmProvider::OpenAi | LlmProvider::AzureOpenAi => {
                if model_lower.contains("gpt-4o-mini") {
                    // $0.15 / $0.60 per 1M tokens
                    Some(ModelPricing {
                        input_price_per_1k: 0.000_15,
                        output_price_per_1k: 0.000_60,
                    })
                } else if model_lower.contains("gpt-4o") {
                    // $2.50 / $10 per 1M tokens
                    Some(ModelPricing {
                        input_price_per_1k: 0.002_5,
                        output_price_per_1k: 0.010,
                    })
                } else {
                    // Generic OpenAI fallback
                    Some(ModelPricing {
                        input_price_per_1k: 0.001,
                        output_price_per_1k: 0.003,
                    })
                }
            }
            LlmProvider::Gemini => {
                if model_lower.contains("pro") {
                    // $1.25 / $5 per 1M tokens
                    Some(ModelPricing {
                        input_price_per_1k: 0.001_25,
                        output_price_per_1k: 0.005,
                    })
                } else {
                    Some(ModelPricing {
                        input_price_per_1k: 0.001,
                        output_price_per_1k: 0.003,
                    })
                }
            }
            // Default pricing for other providers: $1 / $3 per 1M tokens
            _ => Some(ModelPricing {
                input_price_per_1k: 0.001,
                output_price_per_1k: 0.003,
            }),
        }
    }

    // ----- helpers -----------------------------------------------------------

    /// Return the character-per-token ratio for a given provider.
    fn chars_per_token(provider: &LlmProvider) -> f64 {
        match provider {
            LlmProvider::Claude | LlmProvider::ClaudeCode => 4.5,
            LlmProvider::OpenAi | LlmProvider::AzureOpenAi => 4.0,
            LlmProvider::Gemini => 4.0,
            _ => 4.0,
        }
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// UsageTracker — cumulative token and cost accounting
// ---------------------------------------------------------------------------

/// Thread-safe tracker for cumulative token usage across multiple requests.
///
/// Records [`TokenEstimate`] entries keyed by provider name and provides
/// aggregation helpers.
#[derive(Debug, Clone)]
pub struct UsageTracker {
    inner: Arc<Mutex<UsageTrackerInner>>,
}

#[derive(Debug, Default)]
struct UsageTrackerInner {
    total: TokenEstimate,
    by_provider: HashMap<String, TokenEstimate>,
}

impl UsageTracker {
    /// Create a new, empty `UsageTracker`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(UsageTrackerInner::default())),
        }
    }

    /// Record a [`TokenEstimate`], optionally keyed by `provider_name`.
    pub fn record(&self, estimate: &TokenEstimate) {
        self.record_for_provider(estimate, "unknown");
    }

    /// Record a [`TokenEstimate`] associated with a specific provider name.
    pub fn record_for_provider(&self, estimate: &TokenEstimate, provider_name: &str) {
        #[allow(clippy::expect_used)] // lock poisoning
        let mut inner = self.inner.lock().expect("UsageTracker lock poisoned");

        inner.total.input_tokens += estimate.input_tokens;
        inner.total.output_tokens += estimate.output_tokens;
        inner.total.total_tokens += estimate.total_tokens;
        inner.total.estimated_cost_usd =
            match (inner.total.estimated_cost_usd, estimate.estimated_cost_usd) {
                (Some(a), Some(b)) => Some(a + b),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };

        let entry = inner
            .by_provider
            .entry(provider_name.to_string())
            .or_default();
        entry.input_tokens += estimate.input_tokens;
        entry.output_tokens += estimate.output_tokens;
        entry.total_tokens += estimate.total_tokens;
        entry.estimated_cost_usd = match (entry.estimated_cost_usd, estimate.estimated_cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }

    /// Return the cumulative totals.
    pub fn total(&self) -> TokenEstimate {
        #[allow(clippy::expect_used)] // lock poisoning
        self.inner
            .lock()
            .expect("UsageTracker lock poisoned")
            .total
            .clone()
    }

    /// Return a snapshot of per-provider totals.
    pub fn by_provider(&self) -> HashMap<String, TokenEstimate> {
        #[allow(clippy::expect_used)] // lock poisoning
        self.inner
            .lock()
            .expect("UsageTracker lock poisoned")
            .by_provider
            .clone()
    }

    /// Reset all accumulated usage data.
    pub fn reset(&self) {
        #[allow(clippy::expect_used)] // lock poisoning
        let mut inner = self.inner.lock().expect("UsageTracker lock poisoned");
        inner.total = TokenEstimate::default();
        inner.by_provider.clear();
    }
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_counter() -> TokenCounter {
        TokenCounter::new()
    }

    // -- estimate_tokens tests -----------------------------------------------

    #[test]
    fn test_empty_string_returns_zero() {
        let tc = make_counter();
        assert_eq!(tc.estimate_tokens("", &LlmProvider::Claude), 0);
        assert_eq!(tc.estimate_tokens("", &LlmProvider::OpenAi), 0);
    }

    #[test]
    fn test_claude_uses_4_5_ratio() {
        let tc = make_counter();
        // 45 chars / 4.5 = 10 tokens exactly
        let _text = "a]".repeat(22); // 44 chars => ceil(44/4.5) = 10
        let tokens = tc.estimate_tokens(&"a".repeat(45), &LlmProvider::Claude);
        assert_eq!(tokens, 10);

        // Also verify ClaudeCode uses the same ratio
        let tokens_cc = tc.estimate_tokens(&"a".repeat(45), &LlmProvider::ClaudeCode);
        assert_eq!(tokens_cc, 10);
    }

    #[test]
    fn test_openai_uses_4_0_ratio() {
        let tc = make_counter();
        // 40 chars / 4.0 = 10 tokens
        let tokens = tc.estimate_tokens(&"b".repeat(40), &LlmProvider::OpenAi);
        assert_eq!(tokens, 10);
    }

    #[test]
    fn test_gemini_uses_4_0_ratio() {
        let tc = make_counter();
        let tokens = tc.estimate_tokens(&"c".repeat(40), &LlmProvider::Gemini);
        assert_eq!(tokens, 10);
    }

    #[test]
    fn test_single_char_returns_one_token() {
        let tc = make_counter();
        assert_eq!(tc.estimate_tokens("x", &LlmProvider::Claude), 1);
        assert_eq!(tc.estimate_tokens("x", &LlmProvider::OpenAi), 1);
    }

    // -- estimate_message_tokens tests ----------------------------------------

    #[test]
    fn test_message_tokens_includes_overhead() {
        let tc = make_counter();
        let session_id = Uuid::new_v4();
        let messages = vec![
            Message::user("Hello", session_id),
            Message::assistant("Hi there!", session_id),
        ];
        let tokens = tc.estimate_message_tokens(&messages, &LlmProvider::OpenAi);
        // "Hello" = 5 chars / 4 = ceil(1.25) = 2 tokens + 4 overhead = 6
        // "Hi there!" = 9 chars / 4 = ceil(2.25) = 3 tokens + 4 overhead = 7
        // Total = 13
        assert_eq!(tokens, 13);
    }

    // -- estimate_tool_tokens tests -------------------------------------------

    #[test]
    fn test_tool_tokens_estimation() {
        let tc = make_counter();
        let tools = vec![SkillDescriptor {
            name: "test_tool".to_string(),
            description: "A test tool for testing".to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
            required_capabilities: vec![],
            requires_approval: false,
        }];
        let tokens = tc.estimate_tool_tokens(&tools, &LlmProvider::OpenAi);
        // Should include tokens for name + description + schema + overhead
        assert!(
            tokens > 8,
            "Tool tokens should exceed the per-tool overhead"
        );
    }

    // -- pricing and cost tests -----------------------------------------------

    #[test]
    fn test_claude_opus_pricing() {
        let tc = make_counter();
        let pricing = tc
            .default_pricing(&LlmProvider::Claude, "claude-3-opus-20240229")
            .unwrap();
        // $15 per 1M input => $0.015 per 1K
        assert!((pricing.input_price_per_1k - 0.015).abs() < f64::EPSILON);
        // $75 per 1M output => $0.075 per 1K
        assert!((pricing.output_price_per_1k - 0.075).abs() < f64::EPSILON);
    }

    #[test]
    fn test_estimate_cost_calculation() {
        let tc = make_counter();
        // 1000 input tokens, 500 output tokens, GPT-4o
        let cost = tc
            .estimate_cost(1000, 500, &LlmProvider::OpenAi, "gpt-4o")
            .unwrap();
        // input: 1000/1000 * 0.0025 = 0.0025
        // output: 500/1000 * 0.010 = 0.005
        // total = 0.0075
        assert!((cost - 0.0075).abs() < 1e-10);
    }

    // -- UsageTracker tests ---------------------------------------------------

    #[test]
    fn test_usage_tracker_record_and_total() {
        let tracker = UsageTracker::new();

        tracker.record(&TokenEstimate {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            estimated_cost_usd: Some(0.01),
        });

        tracker.record(&TokenEstimate {
            input_tokens: 200,
            output_tokens: 100,
            total_tokens: 300,
            estimated_cost_usd: Some(0.02),
        });

        let total = tracker.total();
        assert_eq!(total.input_tokens, 300);
        assert_eq!(total.output_tokens, 150);
        assert_eq!(total.total_tokens, 450);
        assert!((total.estimated_cost_usd.unwrap() - 0.03).abs() < 1e-10);
    }

    #[test]
    fn test_usage_tracker_by_provider() {
        let tracker = UsageTracker::new();

        tracker.record_for_provider(
            &TokenEstimate {
                input_tokens: 100,
                output_tokens: 50,
                total_tokens: 150,
                estimated_cost_usd: Some(0.01),
            },
            "claude",
        );

        tracker.record_for_provider(
            &TokenEstimate {
                input_tokens: 200,
                output_tokens: 100,
                total_tokens: 300,
                estimated_cost_usd: Some(0.02),
            },
            "openai",
        );

        let by_provider = tracker.by_provider();
        assert_eq!(by_provider.len(), 2);
        assert_eq!(by_provider["claude"].input_tokens, 100);
        assert_eq!(by_provider["openai"].input_tokens, 200);
    }

    #[test]
    fn test_usage_tracker_reset() {
        let tracker = UsageTracker::new();

        tracker.record(&TokenEstimate {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            estimated_cost_usd: Some(0.01),
        });

        tracker.reset();
        let total = tracker.total();
        assert_eq!(total.input_tokens, 0);
        assert_eq!(total.total_tokens, 0);
        assert!(total.estimated_cost_usd.is_none());
        assert!(tracker.by_provider().is_empty());
    }
}

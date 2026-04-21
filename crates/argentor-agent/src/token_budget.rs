// SPDX-License-Identifier: AGPL-3.0-only
//! Session-level output-token budget enforcement.
//!
//! Prevents runaway LLM costs by tracking cumulative output tokens across
//! all turns of an agentic session and stopping gracefully once the
//! configured ceiling is reached.
//!
//! # Design
//!
//! ```text
//!  AgentRunner::run()
//!      │
//!      ├── TokenBudget::record_output(tokens)
//!      │
//!      └── TokenBudget::is_exhausted()
//!              │
//!              └── true  → return Err(ArgentorError::BudgetExhausted { used, max })
//! ```
//!
//! The budget is intentionally output-only: output tokens are the primary
//! cost driver and the dimension the agent controls directly.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Configuration for the session-level token budget.
#[derive(Debug, Clone)]
pub struct TokenBudgetConfig {
    /// Whether the budget is enforced.  When `false` all calls are no-ops.
    pub enabled: bool,
    /// Maximum output tokens allowed for the entire session (default: 100 000).
    pub max_output_tokens: u64,
}

impl Default for TokenBudgetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_output_tokens: 100_000,
        }
    }
}

/// Error returned when the session budget is exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetExhaustedError {
    /// Tokens consumed so far (equals or exceeds `max`).
    pub used: u64,
    /// Configured maximum.
    pub max: u64,
    /// Any partial response text collected before the budget ran out.
    pub partial_response: Option<String>,
}

impl std::fmt::Display for BudgetExhaustedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Output-token budget exhausted: {}/{} tokens used",
            self.used, self.max
        )
    }
}

impl std::error::Error for BudgetExhaustedError {}

/// Thread-safe session-level output-token budget.
///
/// Clone is cheap — the inner counter is behind an `Arc<AtomicU64>`.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    config: TokenBudgetConfig,
    used: Arc<AtomicU64>,
}

impl TokenBudget {
    /// Create a budget with the given configuration.
    pub fn new(config: TokenBudgetConfig) -> Self {
        Self {
            config,
            used: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create a budget with default configuration (100 K output tokens).
    pub fn with_defaults() -> Self {
        Self::new(TokenBudgetConfig::default())
    }

    /// Create a disabled budget (no enforcement).
    pub fn disabled() -> Self {
        Self::new(TokenBudgetConfig {
            enabled: false,
            max_output_tokens: u64::MAX,
        })
    }

    /// Return the configuration for this budget.
    pub fn config(&self) -> &TokenBudgetConfig {
        &self.config
    }

    /// Record `tokens` output tokens from an LLM response.
    ///
    /// Uses heuristic estimation if the caller passes `0`: `text_len / 4`.
    pub fn record_output(&self, tokens: u64) {
        if self.config.enabled {
            self.used.fetch_add(tokens, Ordering::Relaxed);
        }
    }

    /// Record output tokens estimated from response text length (chars / 4).
    pub fn record_output_from_text(&self, text: &str) {
        let estimate = (text.len() as u64 / 4).max(1);
        self.record_output(estimate);
    }

    /// Return the number of output tokens consumed so far.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Return `true` if the budget has been exhausted.
    pub fn is_exhausted(&self) -> bool {
        self.config.enabled && self.used() >= self.config.max_output_tokens
    }

    /// If the budget is exhausted, return `Err(BudgetExhaustedError)`.
    ///
    /// Pass `partial_response` to include any text already generated.
    pub fn check(&self, partial_response: Option<String>) -> Result<(), BudgetExhaustedError> {
        if self.is_exhausted() {
            Err(BudgetExhaustedError {
                used: self.used(),
                max: self.config.max_output_tokens,
                partial_response,
            })
        } else {
            Ok(())
        }
    }

    /// Remaining output tokens before the budget is hit (saturating at 0).
    pub fn remaining(&self) -> u64 {
        self.config
            .max_output_tokens
            .saturating_sub(self.used())
    }

    /// Reset the consumed counter (useful for testing or new sub-sessions).
    pub fn reset(&self) {
        self.used.store(0, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let b = TokenBudget::with_defaults();
        assert!(b.config.enabled);
        assert_eq!(b.config.max_output_tokens, 100_000);
        assert_eq!(b.used(), 0);
        assert!(!b.is_exhausted());
    }

    #[test]
    fn test_record_and_exhaustion() {
        let b = TokenBudget::new(TokenBudgetConfig {
            enabled: true,
            max_output_tokens: 10,
        });

        b.record_output(5);
        assert_eq!(b.used(), 5);
        assert!(!b.is_exhausted());

        b.record_output(5);
        assert_eq!(b.used(), 10);
        assert!(b.is_exhausted());
    }

    #[test]
    fn test_check_returns_err_when_exhausted() {
        let b = TokenBudget::new(TokenBudgetConfig {
            enabled: true,
            max_output_tokens: 5,
        });
        b.record_output(10);
        let result = b.check(Some("partial".into()));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.used, 10);
        assert_eq!(err.max, 5);
        assert_eq!(err.partial_response.as_deref(), Some("partial"));
    }

    #[test]
    fn test_disabled_budget_never_exhausts() {
        let b = TokenBudget::disabled();
        b.record_output(u64::MAX);
        assert!(!b.is_exhausted());
        assert!(b.check(None).is_ok());
    }

    #[test]
    fn test_remaining() {
        let b = TokenBudget::new(TokenBudgetConfig {
            enabled: true,
            max_output_tokens: 100,
        });
        b.record_output(30);
        assert_eq!(b.remaining(), 70);
        b.record_output(80);
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn test_reset() {
        let b = TokenBudget::with_defaults();
        b.record_output(5000);
        assert_eq!(b.used(), 5000);
        b.reset();
        assert_eq!(b.used(), 0);
    }

    #[test]
    fn test_record_from_text() {
        let b = TokenBudget::with_defaults();
        b.record_output_from_text("hello world");
        // 11 chars / 4 = 2 (floor), but max(1) keeps it ≥ 1
        assert!(b.used() >= 1);
    }

    #[test]
    fn test_clone_shares_counter() {
        let b1 = TokenBudget::with_defaults();
        let b2 = b1.clone();
        b1.record_output(50);
        assert_eq!(b2.used(), 50);
    }
}

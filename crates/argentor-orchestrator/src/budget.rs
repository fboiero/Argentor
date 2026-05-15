//! Token and resource budgeting per agent.
//!
//! Each agent is assigned a [`TokenBudget`] that caps its resource consumption
//! (input/output tokens, tool calls, wall-clock time). The [`BudgetTracker`]
//! aggregates usage across all agents in a thread-safe manner and exposes
//! helpers for cost estimation and budget enforcement.

use crate::types::AgentRole;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// TokenBudget
// ---------------------------------------------------------------------------

/// Resource limits for a single agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Maximum input tokens the agent may consume.
    pub max_input_tokens: u64,
    /// Maximum output tokens the agent may produce.
    pub max_output_tokens: u64,
    /// Maximum total (input + output) tokens.
    pub max_total_tokens: u64,
    /// Maximum number of tool calls the agent may make.
    pub max_tool_calls: u32,
    /// Maximum wall-clock duration in seconds.
    pub max_duration_secs: u64,
}

impl TokenBudget {
    /// Create a new budget with explicit limits.
    pub fn new(
        max_input_tokens: u64,
        max_output_tokens: u64,
        max_total_tokens: u64,
        max_tool_calls: u32,
        max_duration_secs: u64,
    ) -> Self {
        Self {
            max_input_tokens,
            max_output_tokens,
            max_total_tokens,
            max_tool_calls,
            max_duration_secs,
        }
    }
}

// ---------------------------------------------------------------------------
// Default budgets per role
// ---------------------------------------------------------------------------

/// Returns sensible default budgets per [`AgentRole`].
///
/// | Role | Input | Output | Tools | Duration |
/// |------|-------|--------|-------|----------|
/// | Orchestrator | 50 K | 10 K | 100 | 300 s |
/// | Coder | 100 K | 50 K | 50 | 600 s |
/// | Tester | 50 K | 20 K | 30 | 300 s |
/// | Reviewer | 30 K | 10 K | 10 | 120 s |
/// | Others | 50 K | 20 K | 30 | 300 s |
pub fn default_budget(role: &AgentRole) -> TokenBudget {
    match role {
        AgentRole::Orchestrator => TokenBudget::new(50_000, 10_000, 60_000, 100, 300),
        AgentRole::Coder => TokenBudget::new(100_000, 50_000, 150_000, 50, 600),
        AgentRole::Tester => TokenBudget::new(50_000, 20_000, 70_000, 30, 300),
        AgentRole::Reviewer => TokenBudget::new(30_000, 10_000, 40_000, 10, 120),
        _ => TokenBudget::new(50_000, 20_000, 70_000, 30, 300),
    }
}

// ---------------------------------------------------------------------------
// AgentUsage
// ---------------------------------------------------------------------------

/// Accumulated resource usage for a single agent.
///
/// `Instant` cannot be serialized, so the struct derives `Debug` and `Clone`
/// but implements `Serialize`/`Deserialize` manually — serialization captures
/// `elapsed_secs` instead of the raw instant.
#[derive(Debug, Clone)]
pub struct AgentUsage {
    /// Total input tokens consumed so far.
    pub input_tokens: u64,
    /// Total output tokens produced so far.
    pub output_tokens: u64,
    /// Number of tool calls made so far.
    pub tool_calls: u32,
    /// When tracking started.
    pub start_time: Instant,
    /// The agent role this usage belongs to.
    pub role: AgentRole,
}

impl AgentUsage {
    fn new(role: AgentRole) -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            tool_calls: 0,
            start_time: Instant::now(),
            role,
        }
    }

    /// Elapsed wall-clock seconds since tracking started.
    pub fn elapsed_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Total tokens (input + output).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

// Manual Serialize — replaces `start_time` with `elapsed_secs`.
impl Serialize for AgentUsage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("AgentUsage", 5)?;
        state.serialize_field("input_tokens", &self.input_tokens)?;
        state.serialize_field("output_tokens", &self.output_tokens)?;
        state.serialize_field("tool_calls", &self.tool_calls)?;
        state.serialize_field("elapsed_secs", &self.elapsed_secs())?;
        state.serialize_field("role", &self.role)?;
        state.end()
    }
}

// ---------------------------------------------------------------------------
// BudgetStatus
// ---------------------------------------------------------------------------

/// Result of checking an agent's budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BudgetStatus {
    /// Usage is within all limits.
    WithinBudget,
    /// Usage has passed 80 % of a resource limit.
    Warning {
        /// Human-readable resource name (e.g. "input_tokens").
        resource: String,
        /// Percentage of the limit that has been consumed (0.0–1.0+).
        usage_pct: f64,
    },
    /// A hard limit has been exceeded.
    Exceeded {
        /// Human-readable resource name.
        resource: String,
        /// The configured limit.
        limit: u64,
        /// The actual value at the time of the check.
        used: u64,
    },
}

// ---------------------------------------------------------------------------
// BudgetSummary
// ---------------------------------------------------------------------------

/// Aggregated budget summary across all tracked agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetSummary {
    /// Sum of input tokens across all agents.
    pub total_input_tokens: u64,
    /// Sum of output tokens across all agents.
    pub total_output_tokens: u64,
    /// Sum of tool calls across all agents.
    pub total_tool_calls: u32,
    /// Per-agent breakdown.
    pub per_agent: Vec<AgentUsageEntry>,
    /// Estimated cost in USD (if pricing was provided).
    pub estimated_cost_usd: Option<f64>,
}

/// A single entry in the per-agent breakdown inside [`BudgetSummary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUsageEntry {
    /// Role of the agent.
    pub role: AgentRole,
    /// Input tokens consumed.
    pub input_tokens: u64,
    /// Output tokens produced.
    pub output_tokens: u64,
    /// Tool calls made.
    pub tool_calls: u32,
    /// Elapsed seconds since tracking started.
    pub elapsed_secs: u64,
}

// ---------------------------------------------------------------------------
// BudgetTracker (inner mutable state)
// ---------------------------------------------------------------------------

/// Mutable state protected by the `RwLock` inside [`BudgetTracker`].
#[derive(Debug)]
struct Inner {
    budgets: HashMap<AgentRole, TokenBudget>,
    usage: HashMap<AgentRole, AgentUsage>,
}

impl Inner {
    fn new() -> Self {
        Self {
            budgets: HashMap::new(),
            usage: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// BudgetTracker
// ---------------------------------------------------------------------------

/// Thread-safe tracker that enforces per-agent token/resource budgets.
///
/// Internally wraps an `Arc<RwLock<Inner>>` so it can be shared across tasks.
///
/// # Example
///
/// ```rust,no_run
/// use argentor_orchestrator::budget::{BudgetTracker, default_budget};
/// use argentor_orchestrator::types::AgentRole;
///
/// # async fn example() {
/// let tracker = BudgetTracker::new();
/// let role = AgentRole::Coder;
/// tracker.set_budget(role.clone(), default_budget(&role)).await;
/// tracker.start_tracking(role.clone()).await;
/// tracker.record_tokens(&role, 1_000, 500).await;
/// let status = tracker.check_budget(&role).await;
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct BudgetTracker {
    inner: Arc<RwLock<Inner>>,
}

impl BudgetTracker {
    /// Create a new, empty budget tracker.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner::new())),
        }
    }

    /// Set (or replace) the budget for a given role.
    pub async fn set_budget(&self, role: AgentRole, budget: TokenBudget) {
        info!(role = %role, max_input = budget.max_input_tokens, max_output = budget.max_output_tokens, "budget set");
        self.inner.write().await.budgets.insert(role, budget);
    }

    /// Begin tracking usage for the given role.
    ///
    /// If the role was already being tracked, this resets its counters.
    pub async fn start_tracking(&self, role: AgentRole) {
        info!(role = %role, "start tracking budget");
        let usage = AgentUsage::new(role.clone());
        self.inner.write().await.usage.insert(role, usage);
    }

    /// Record consumed input and output tokens for the given role.
    pub async fn record_tokens(&self, role: &AgentRole, input: u64, output: u64) {
        let mut guard = self.inner.write().await;
        if let Some(usage) = guard.usage.get_mut(role) {
            usage.input_tokens += input;
            usage.output_tokens += output;
        }
    }

    /// Increment the tool-call counter for the given role.
    pub async fn record_tool_call(&self, role: &AgentRole) {
        let mut guard = self.inner.write().await;
        if let Some(usage) = guard.usage.get_mut(role) {
            usage.tool_calls += 1;
        }
    }

    /// Check whether the agent is still within its budget.
    ///
    /// Returns the *worst* status found (Exceeded > Warning > WithinBudget).
    pub async fn check_budget(&self, role: &AgentRole) -> BudgetStatus {
        let guard = self.inner.read().await;
        let (budget, usage) = match (guard.budgets.get(role), guard.usage.get(role)) {
            (Some(b), Some(u)) => (b, u),
            _ => return BudgetStatus::WithinBudget,
        };

        // Collect all resource checks. Order: exceeded first, then warnings.
        let checks: Vec<(&str, u64, u64)> = vec![
            ("input_tokens", usage.input_tokens, budget.max_input_tokens),
            (
                "output_tokens",
                usage.output_tokens,
                budget.max_output_tokens,
            ),
            (
                "total_tokens",
                usage.total_tokens(),
                budget.max_total_tokens,
            ),
            (
                "tool_calls",
                u64::from(usage.tool_calls),
                u64::from(budget.max_tool_calls),
            ),
            (
                "duration_secs",
                usage.elapsed_secs(),
                budget.max_duration_secs,
            ),
        ];

        // Check for exceeded limits first.
        for &(resource, used, limit) in &checks {
            if used > limit {
                warn!(role = %role, resource, used, limit, "budget exceeded");
                return BudgetStatus::Exceeded {
                    resource: resource.to_string(),
                    limit,
                    used,
                };
            }
        }

        // Check for warnings (>80 %).
        for &(resource, used, limit) in &checks {
            if limit > 0 {
                let pct = used as f64 / limit as f64;
                if pct > 0.8 {
                    warn!(role = %role, resource, usage_pct = pct, "budget warning");
                    return BudgetStatus::Warning {
                        resource: resource.to_string(),
                        usage_pct: pct,
                    };
                }
            }
        }

        BudgetStatus::WithinBudget
    }

    /// Return a clone of the current usage for the given role, if tracked.
    pub async fn usage(&self, role: &AgentRole) -> Option<AgentUsage> {
        self.inner.read().await.usage.get(role).cloned()
    }

    /// Estimate total cost across all tracked agents.
    ///
    /// Prices are specified *per 1 000 tokens*.
    pub async fn total_cost_estimate(
        &self,
        price_per_1k_input: f64,
        price_per_1k_output: f64,
    ) -> f64 {
        let guard = self.inner.read().await;
        guard.usage.values().fold(0.0, |acc, u| {
            acc + (u.input_tokens as f64 / 1_000.0) * price_per_1k_input
                + (u.output_tokens as f64 / 1_000.0) * price_per_1k_output
        })
    }

    /// Produce an aggregated summary of all tracked agents.
    pub async fn summary(&self) -> BudgetSummary {
        let guard = self.inner.read().await;

        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;
        let mut total_tools: u32 = 0;
        let mut per_agent = Vec::new();

        for usage in guard.usage.values() {
            total_input += usage.input_tokens;
            total_output += usage.output_tokens;
            total_tools += usage.tool_calls;
            per_agent.push(AgentUsageEntry {
                role: usage.role.clone(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                tool_calls: usage.tool_calls,
                elapsed_secs: usage.elapsed_secs(),
            });
        }

        // Sort by role display name for deterministic output.
        per_agent.sort_by_key(|entry| entry.role.to_string());

        BudgetSummary {
            total_input_tokens: total_input,
            total_output_tokens: total_output,
            total_tool_calls: total_tools,
            per_agent,
            estimated_cost_usd: None,
        }
    }

    /// Reset usage counters for a specific role. Budget limits are preserved.
    pub async fn reset(&self, role: &AgentRole) {
        let mut guard = self.inner.write().await;
        if guard.usage.contains_key(role) {
            info!(role = %role, "budget usage reset");
            guard
                .usage
                .insert(role.clone(), AgentUsage::new(role.clone()));
        }
    }
}

impl Default for BudgetTracker {
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

    fn coder() -> AgentRole {
        AgentRole::Coder
    }

    fn reviewer() -> AgentRole {
        AgentRole::Reviewer
    }

    // 1. Basic tracking records tokens correctly.
    #[tokio::test]
    async fn test_record_tokens() {
        let tracker = BudgetTracker::new();
        let role = coder();
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;

        tracker.record_tokens(&role, 1_000, 500).await;
        tracker.record_tokens(&role, 2_000, 300).await;

        let usage = tracker.usage(&role).await.unwrap();
        assert_eq!(usage.input_tokens, 3_000);
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(usage.total_tokens(), 3_800);
    }

    // 2. Tool-call counter increments correctly.
    #[tokio::test]
    async fn test_record_tool_calls() {
        let tracker = BudgetTracker::new();
        let role = coder();
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;

        for _ in 0..5 {
            tracker.record_tool_call(&role).await;
        }

        let usage = tracker.usage(&role).await.unwrap();
        assert_eq!(usage.tool_calls, 5);
    }

    // 3. Within-budget status when usage is low.
    #[tokio::test]
    async fn test_within_budget() {
        let tracker = BudgetTracker::new();
        let role = coder();
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;
        tracker.record_tokens(&role, 100, 50).await;

        let status = tracker.check_budget(&role).await;
        assert_eq!(status, BudgetStatus::WithinBudget);
    }

    // 4. Warning when usage exceeds 80 %.
    #[tokio::test]
    async fn test_warning_threshold() {
        let tracker = BudgetTracker::new();
        let role = coder();
        // Coder budget: 100K input
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;

        // 85 % of 100K = 85 000
        tracker.record_tokens(&role, 85_000, 0).await;

        let status = tracker.check_budget(&role).await;
        match status {
            BudgetStatus::Warning {
                resource,
                usage_pct,
            } => {
                assert_eq!(resource, "input_tokens");
                assert!(usage_pct > 0.8);
            }
            other => panic!("expected Warning, got {other:?}"),
        }
    }

    // 5. Exceeded status when a limit is surpassed.
    #[tokio::test]
    async fn test_exceeded_budget() {
        let tracker = BudgetTracker::new();
        let role = reviewer();
        // Reviewer: 30K input
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;

        tracker.record_tokens(&role, 35_000, 0).await;

        let status = tracker.check_budget(&role).await;
        match status {
            BudgetStatus::Exceeded {
                resource,
                limit,
                used,
            } => {
                assert_eq!(resource, "input_tokens");
                assert_eq!(limit, 30_000);
                assert_eq!(used, 35_000);
            }
            other => panic!("expected Exceeded, got {other:?}"),
        }
    }

    // 6. Cost estimation across multiple agents.
    #[tokio::test]
    async fn test_cost_estimation() {
        let tracker = BudgetTracker::new();

        let c = coder();
        tracker.set_budget(c.clone(), default_budget(&c)).await;
        tracker.start_tracking(c.clone()).await;
        tracker.record_tokens(&c, 10_000, 5_000).await;

        let r = reviewer();
        tracker.set_budget(r.clone(), default_budget(&r)).await;
        tracker.start_tracking(r.clone()).await;
        tracker.record_tokens(&r, 2_000, 1_000).await;

        // $0.01 per 1K input, $0.03 per 1K output
        let cost = tracker.total_cost_estimate(0.01, 0.03).await;
        // (12K input * 0.01/1K) + (6K output * 0.03/1K) = 0.12 + 0.18 = 0.30
        let expected = (12_000.0 / 1_000.0) * 0.01 + (6_000.0 / 1_000.0) * 0.03;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    // 7. Summary aggregates all agents.
    #[tokio::test]
    async fn test_summary() {
        let tracker = BudgetTracker::new();

        let c = coder();
        tracker.start_tracking(c.clone()).await;
        tracker.record_tokens(&c, 1_000, 500).await;
        tracker.record_tool_call(&c).await;

        let r = reviewer();
        tracker.start_tracking(r.clone()).await;
        tracker.record_tokens(&r, 2_000, 1_000).await;

        let summary = tracker.summary().await;
        assert_eq!(summary.total_input_tokens, 3_000);
        assert_eq!(summary.total_output_tokens, 1_500);
        assert_eq!(summary.total_tool_calls, 1);
        assert_eq!(summary.per_agent.len(), 2);
    }

    // 8. Reset clears usage but keeps the budget.
    #[tokio::test]
    async fn test_reset() {
        let tracker = BudgetTracker::new();
        let role = coder();
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;
        tracker.record_tokens(&role, 50_000, 10_000).await;

        tracker.reset(&role).await;

        let usage = tracker.usage(&role).await.unwrap();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.tool_calls, 0);

        // Budget still exists — a new check should succeed.
        let status = tracker.check_budget(&role).await;
        assert_eq!(status, BudgetStatus::WithinBudget);
    }

    // 9. Unknown role returns None for usage, WithinBudget for check.
    #[tokio::test]
    async fn test_unknown_role() {
        let tracker = BudgetTracker::new();
        let role = AgentRole::Custom("unknown".to_string());

        assert!(tracker.usage(&role).await.is_none());
        assert_eq!(
            tracker.check_budget(&role).await,
            BudgetStatus::WithinBudget
        );
    }

    // 10. Default budget returns expected values per role.
    #[test]
    fn test_default_budgets() {
        let orchestrator = default_budget(&AgentRole::Orchestrator);
        assert_eq!(orchestrator.max_input_tokens, 50_000);
        assert_eq!(orchestrator.max_output_tokens, 10_000);
        assert_eq!(orchestrator.max_tool_calls, 100);
        assert_eq!(orchestrator.max_duration_secs, 300);

        let coder = default_budget(&AgentRole::Coder);
        assert_eq!(coder.max_input_tokens, 100_000);
        assert_eq!(coder.max_output_tokens, 50_000);
        assert_eq!(coder.max_tool_calls, 50);
        assert_eq!(coder.max_duration_secs, 600);

        let tester = default_budget(&AgentRole::Tester);
        assert_eq!(tester.max_input_tokens, 50_000);

        let reviewer = default_budget(&AgentRole::Reviewer);
        assert_eq!(reviewer.max_input_tokens, 30_000);
        assert_eq!(reviewer.max_duration_secs, 120);

        // Custom role falls through to "Others"
        let custom = default_budget(&AgentRole::Custom("x".into()));
        assert_eq!(custom.max_input_tokens, 50_000);
        assert_eq!(custom.max_output_tokens, 20_000);
    }

    // 11. Tool-call exceeded triggers Exceeded status.
    #[tokio::test]
    async fn test_tool_call_exceeded() {
        let tracker = BudgetTracker::new();
        let role = reviewer();
        // Reviewer budget: 10 tool calls
        tracker
            .set_budget(role.clone(), default_budget(&role))
            .await;
        tracker.start_tracking(role.clone()).await;

        for _ in 0..11 {
            tracker.record_tool_call(&role).await;
        }

        let status = tracker.check_budget(&role).await;
        match status {
            BudgetStatus::Exceeded {
                resource,
                limit,
                used,
            } => {
                assert_eq!(resource, "tool_calls");
                assert_eq!(limit, 10);
                assert_eq!(used, 11);
            }
            other => panic!("expected Exceeded for tool_calls, got {other:?}"),
        }
    }

    // 12. AgentUsage serialization includes elapsed_secs instead of Instant.
    #[tokio::test]
    async fn test_usage_serialization() {
        let tracker = BudgetTracker::new();
        let role = coder();
        tracker.start_tracking(role.clone()).await;
        tracker.record_tokens(&role, 42, 7).await;

        let usage = tracker.usage(&role).await.unwrap();
        let json = serde_json::to_string(&usage).unwrap();
        assert!(json.contains("\"input_tokens\":42"));
        assert!(json.contains("\"output_tokens\":7"));
        assert!(json.contains("\"elapsed_secs\":"));
        // Should NOT contain "start_time"
        assert!(!json.contains("start_time"));
    }
}

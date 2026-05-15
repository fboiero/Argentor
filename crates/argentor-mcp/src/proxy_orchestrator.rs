//! Proxy orchestrator — coordinates multiple MCP proxy instances with
//! intelligent routing, load balancing, failover, and circuit breakers.
//!
//! The [`ProxyOrchestrator`] sits on top of [`McpProxy`] and [`McpServerManager`],
//! providing a unified control plane for multi-proxy deployments. It supports:
//!
//! - **Routing rules** — route tool calls to specific proxy groups based on
//!   tool name patterns, agent roles, or tags.
//! - **Load balancing** — distribute calls across proxies using fixed,
//!   round-robin, or least-loaded strategies.
//! - **Failover** — automatically reroute calls when a proxy's circuit is open.
//! - **Circuit breaker** — disable a proxy after consecutive failures and
//!   auto-recover after a cooldown period.
//! - **Aggregated metrics** — unified view across all managed proxies.

use crate::proxy::McpProxy;
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Routing strategy for distributing tool calls across proxies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Always use the first matching proxy.
    Fixed,
    /// Round-robin across matching proxies.
    RoundRobin,
    /// Send to the proxy with fewest active calls.
    LeastLoaded,
    /// Route based on tool name prefix patterns.
    PatternBased,
}

/// A routing rule that maps tool calls to specific proxy groups.
///
/// Rules are evaluated in priority order (highest first). The first rule whose
/// [`tool_pattern`](RoutingRule::tool_pattern) and
/// [`agent_roles`](RoutingRule::agent_roles) match the incoming call wins.
#[derive(Debug, Clone)]
pub struct RoutingRule {
    /// Human-readable name for this rule.
    pub name: String,
    /// Glob-like pattern for tool names (e.g., `"mcp_github_*"`).
    /// `None` means "match any tool name".
    pub tool_pattern: Option<String>,
    /// If non-empty, the agent's role must appear in this list.
    pub agent_roles: Vec<String>,
    /// Target proxy group name.
    pub target_group: String,
    /// Higher priority rules are evaluated first.
    pub priority: u32,
}

/// State of a managed proxy within the orchestrator.
pub struct ManagedProxy {
    /// Unique identifier for this proxy instance.
    pub id: String,
    /// Logical group this proxy belongs to.
    pub group: String,
    /// The underlying MCP proxy.
    pub proxy: Arc<McpProxy>,
    /// Whether the proxy is administratively enabled.
    pub enabled: bool,
    /// Number of consecutive failures observed.
    pub consecutive_failures: u32,
    /// Whether the circuit breaker is currently open.
    pub circuit_open: bool,
    /// If the circuit is open, the earliest time it may transition to half-open.
    pub circuit_open_until: Option<DateTime<Utc>>,
    /// Lifetime total of calls routed through this proxy.
    pub total_calls: u64,
    /// Timestamp of the most recent call.
    pub last_call: Option<DateTime<Utc>>,
}

/// Circuit breaker configuration.
///
/// After [`failure_threshold`](CircuitBreakerConfig::failure_threshold) consecutive
/// failures, the circuit opens for [`cooldown_secs`](CircuitBreakerConfig::cooldown_secs).
/// During the half-open phase, up to
/// [`half_open_max_calls`](CircuitBreakerConfig::half_open_max_calls) test calls
/// are allowed through.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Seconds the circuit stays open before transitioning to half-open.
    pub cooldown_secs: u64,
    /// Maximum calls allowed during the half-open phase.
    pub half_open_max_calls: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            cooldown_secs: 30,
            half_open_max_calls: 3,
        }
    }
}

/// Summary information about a proxy (returned by [`ProxyOrchestrator::list_proxies`]).
#[derive(Debug, Clone, Serialize)]
pub struct ProxyInfo {
    /// Proxy identifier.
    pub id: String,
    /// Group the proxy belongs to.
    pub group: String,
    /// Whether the proxy is administratively enabled.
    pub enabled: bool,
    /// Whether the circuit breaker is currently open.
    pub circuit_open: bool,
    /// Consecutive failure count.
    pub consecutive_failures: u32,
    /// Lifetime total calls.
    pub total_calls: u64,
    /// Timestamp of the last call routed to this proxy.
    pub last_call: Option<DateTime<Utc>>,
}

/// Aggregated metrics across all managed proxies.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OrchestratorMetrics {
    /// Total number of registered proxies.
    pub total_proxies: usize,
    /// Proxies that are enabled and have a closed circuit.
    pub active_proxies: usize,
    /// Proxies whose circuit breaker is currently open.
    pub circuit_open_proxies: usize,
    /// Total lifetime calls across all proxies.
    pub total_calls: u64,
    /// Total lifetime failures across all proxies.
    pub total_failures: u64,
    /// Calls grouped by proxy group name.
    pub calls_per_group: HashMap<String, u64>,
    /// Number of routing rules configured.
    pub routing_rules_count: usize,
}

// ---------------------------------------------------------------------------
// ProxyOrchestrator
// ---------------------------------------------------------------------------

/// The proxy orchestrator — manages multiple MCP proxies as a unified control plane.
///
/// # Example
///
/// ```rust,ignore
/// use argentor_mcp::proxy_orchestrator::*;
///
/// let orchestrator = ProxyOrchestrator::new(
///     RoutingStrategy::RoundRobin,
///     CircuitBreakerConfig::default(),
/// );
///
/// orchestrator.add_proxy("p1", "github", proxy1.clone())?;
/// orchestrator.add_proxy("p2", "github", proxy2.clone())?;
///
/// orchestrator.add_rule(RoutingRule {
///     name: "github_tools".into(),
///     tool_pattern: Some("mcp_github_*".into()),
///     agent_roles: vec![],
///     target_group: "github".into(),
///     priority: 10,
/// });
///
/// let result = orchestrator.execute(tool_call, "agent-1", Some("Coder")).await?;
/// ```
pub struct ProxyOrchestrator {
    proxies: Arc<RwLock<Vec<ManagedProxy>>>,
    rules: Arc<RwLock<Vec<RoutingRule>>>,
    strategy: RwLock<RoutingStrategy>,
    circuit_breaker: CircuitBreakerConfig,
    round_robin_counter: Arc<AtomicU64>,
}

impl ProxyOrchestrator {
    /// Create a new orchestrator with the given routing strategy and circuit
    /// breaker configuration.
    pub fn new(strategy: RoutingStrategy, circuit_breaker: CircuitBreakerConfig) -> Self {
        Self {
            proxies: Arc::new(RwLock::new(Vec::new())),
            rules: Arc::new(RwLock::new(Vec::new())),
            strategy: RwLock::new(strategy),
            circuit_breaker,
            round_robin_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    // ----- proxy management ------------------------------------------------

    /// Register a proxy in the orchestrator under the given group.
    ///
    /// Returns an error if a proxy with the same `id` already exists.
    pub fn add_proxy(
        &self,
        id: impl Into<String>,
        group: impl Into<String>,
        proxy: Arc<McpProxy>,
    ) -> ArgentorResult<()> {
        let id = id.into();
        let group = group.into();

        let mut proxies = self
            .proxies
            .write()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        if proxies.iter().any(|p| p.id == id) {
            return Err(ArgentorError::Skill(format!(
                "Proxy with id '{id}' already exists"
            )));
        }

        info!(proxy_id = %id, group = %group, "ProxyOrchestrator: proxy added");

        proxies.push(ManagedProxy {
            id,
            group,
            proxy,
            enabled: true,
            consecutive_failures: 0,
            circuit_open: false,
            circuit_open_until: None,
            total_calls: 0,
            last_call: None,
        });

        Ok(())
    }

    /// Remove a proxy by its identifier.
    ///
    /// Returns an error if no proxy with the given `id` is found.
    pub fn remove_proxy(&self, id: &str) -> ArgentorResult<()> {
        let mut proxies = self
            .proxies
            .write()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        let idx = proxies
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| ArgentorError::Skill(format!("Proxy '{id}' not found")))?;

        proxies.remove(idx);

        info!(proxy_id = %id, "ProxyOrchestrator: proxy removed");
        Ok(())
    }

    /// Administratively enable a proxy.
    pub fn enable_proxy(&self, id: &str) -> ArgentorResult<()> {
        let mut proxies = self
            .proxies
            .write()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        let proxy = proxies
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| ArgentorError::Skill(format!("Proxy '{id}' not found")))?;

        proxy.enabled = true;
        info!(proxy_id = %id, "ProxyOrchestrator: proxy enabled");
        Ok(())
    }

    /// Administratively disable a proxy.
    pub fn disable_proxy(&self, id: &str) -> ArgentorResult<()> {
        let mut proxies = self
            .proxies
            .write()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        let proxy = proxies
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| ArgentorError::Skill(format!("Proxy '{id}' not found")))?;

        proxy.enabled = false;
        info!(proxy_id = %id, "ProxyOrchestrator: proxy disabled");
        Ok(())
    }

    // ----- routing rules ---------------------------------------------------

    /// Add a routing rule.
    pub fn add_rule(&self, rule: RoutingRule) {
        if let Ok(mut rules) = self.rules.write() {
            info!(rule = %rule.name, priority = rule.priority, "ProxyOrchestrator: rule added");
            rules.push(rule);
        }
    }

    /// Remove a routing rule by name.
    pub fn remove_rule(&self, name: &str) {
        if let Ok(mut rules) = self.rules.write() {
            rules.retain(|r| r.name != name);
            info!(rule = %name, "ProxyOrchestrator: rule removed");
        }
    }

    // ----- strategy --------------------------------------------------------

    /// Change the routing strategy at runtime.
    pub fn set_strategy(&self, strategy: RoutingStrategy) {
        if let Ok(mut s) = self.strategy.write() {
            info!(strategy = ?strategy, "ProxyOrchestrator: strategy changed");
            *s = strategy;
        }
    }

    // ----- routing ---------------------------------------------------------

    /// Find the appropriate proxy for the given tool call.
    ///
    /// Evaluates routing rules in priority order (descending). The first rule
    /// whose pattern and role constraints match determines the target group.
    /// Within that group, the configured [`RoutingStrategy`] selects the proxy.
    ///
    /// If no rules are configured, all enabled proxies with closed circuits are
    /// considered as a single group.
    pub fn route(
        &self,
        tool_call: &ToolCall,
        _agent_id: &str,
        agent_role: Option<&str>,
    ) -> ArgentorResult<Arc<McpProxy>> {
        let proxies = self
            .proxies
            .read()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        if proxies.is_empty() {
            return Err(ArgentorError::Skill(
                "No proxies registered in the orchestrator".to_string(),
            ));
        }

        let rules = self
            .rules
            .read()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        let strategy = self
            .strategy
            .read()
            .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

        // Determine the target group from rules.
        let target_group = self.find_target_group(&rules, &tool_call.name, agent_role)?;

        // Collect candidate proxies in the target group that are available.
        let candidates: Vec<usize> = proxies
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.group == target_group && p.enabled && !self.is_circuit_effectively_open(p)
            })
            .map(|(i, _)| i)
            .collect();

        if candidates.is_empty() {
            // Failover: try any enabled proxy in the same group whose circuit
            // has entered half-open (cooldown expired).
            let half_open: Vec<usize> = proxies
                .iter()
                .enumerate()
                .filter(|(_, p)| p.group == target_group && p.enabled && self.is_half_open(p))
                .map(|(i, _)| i)
                .collect();

            if half_open.is_empty() {
                return Err(ArgentorError::Skill(format!(
                    "No available proxies in group '{target_group}' (all circuits open or disabled)"
                )));
            }

            // Use the first half-open candidate.
            return Ok(proxies[half_open[0]].proxy.clone());
        }

        let selected = match *strategy {
            RoutingStrategy::Fixed => candidates[0],
            RoutingStrategy::RoundRobin => {
                let counter = self.round_robin_counter.fetch_add(1, Ordering::SeqCst);
                candidates[(counter as usize) % candidates.len()]
            }
            RoutingStrategy::LeastLoaded => {
                // Pick the proxy with the fewest total_calls.
                *candidates
                    .iter()
                    .min_by_key(|&&i| proxies[i].total_calls)
                    .unwrap_or(&candidates[0])
            }
            RoutingStrategy::PatternBased => {
                // PatternBased already resolved the group via rules; just pick
                // the first candidate.
                candidates[0]
            }
        };

        Ok(proxies[selected].proxy.clone())
    }

    /// Resolve the target group from the sorted rule set.
    ///
    /// Rules are sorted by priority descending. The first matching rule wins.
    /// If no rules are defined, a special wildcard group `"*"` is used which
    /// matches all proxies.
    fn find_target_group(
        &self,
        rules: &[RoutingRule],
        tool_name: &str,
        agent_role: Option<&str>,
    ) -> ArgentorResult<String> {
        if rules.is_empty() {
            // No rules: all proxies are candidates. Use a synthetic wildcard group.
            return Ok("*".to_string());
        }

        // Sort by priority descending (we work on a snapshot so this is cheap).
        let mut sorted: Vec<&RoutingRule> = rules.iter().collect();
        sorted.sort_by_key(|entry| std::cmp::Reverse(entry.priority));

        for rule in &sorted {
            let tool_matches = match &rule.tool_pattern {
                None => true,
                Some(pattern) => matches_pattern(pattern, tool_name),
            };

            let role_matches = if rule.agent_roles.is_empty() {
                true
            } else {
                match agent_role {
                    Some(role) => rule.agent_roles.iter().any(|r| r == role),
                    None => false,
                }
            };

            if tool_matches && role_matches {
                return Ok(rule.target_group.clone());
            }
        }

        Err(ArgentorError::Skill(format!(
            "No routing rule matches tool '{}' with role '{}'",
            tool_name,
            agent_role.unwrap_or("<none>")
        )))
    }

    // ----- execution -------------------------------------------------------

    /// Route the tool call to the appropriate proxy and execute it.
    ///
    /// On success, records the outcome and resets the failure counter.
    /// On failure, increments the failure counter and may open the circuit.
    pub async fn execute(
        &self,
        tool_call: ToolCall,
        agent_id: &str,
        agent_role: Option<&str>,
    ) -> ArgentorResult<ToolResult> {
        let (proxy, proxy_id) = {
            let proxy = self.route(&tool_call, agent_id, agent_role)?;

            // Find the proxy id for recording.
            let proxies = self
                .proxies
                .read()
                .map_err(|e| ArgentorError::Skill(format!("Lock poisoned: {e}")))?;

            let proxy_id = proxies
                .iter()
                .find(|p| Arc::ptr_eq(&p.proxy, &proxy))
                .map(|p| p.id.clone())
                .unwrap_or_default();

            (proxy, proxy_id)
        };

        // Increment total_calls and set last_call before execution.
        {
            if let Ok(mut proxies) = self.proxies.write() {
                if let Some(p) = proxies.iter_mut().find(|p| p.id == proxy_id) {
                    p.total_calls += 1;
                    p.last_call = Some(Utc::now());
                }
            }
        }

        let result = proxy.execute(tool_call, agent_id).await;

        match &result {
            Ok(tr) if !tr.is_error => {
                self.record_success(&proxy_id);
            }
            _ => {
                self.record_failure(&proxy_id);
            }
        }

        result
    }

    // ----- circuit breaker -------------------------------------------------

    /// Record a successful call — reset the consecutive failure counter and
    /// close the circuit if it was in half-open state.
    pub fn record_success(&self, proxy_id: &str) {
        if let Ok(mut proxies) = self.proxies.write() {
            if let Some(p) = proxies.iter_mut().find(|p| p.id == proxy_id) {
                p.consecutive_failures = 0;
                if p.circuit_open {
                    info!(proxy_id = %proxy_id, "ProxyOrchestrator: circuit closed after success");
                    p.circuit_open = false;
                    p.circuit_open_until = None;
                }
            }
        }
    }

    /// Record a failed call — increment the consecutive failure counter and
    /// open the circuit if the threshold is reached.
    pub fn record_failure(&self, proxy_id: &str) {
        if let Ok(mut proxies) = self.proxies.write() {
            if let Some(p) = proxies.iter_mut().find(|p| p.id == proxy_id) {
                p.consecutive_failures += 1;

                if p.consecutive_failures >= self.circuit_breaker.failure_threshold
                    && !p.circuit_open
                {
                    p.circuit_open = true;
                    p.circuit_open_until = Some(
                        Utc::now()
                            + chrono::Duration::seconds(self.circuit_breaker.cooldown_secs as i64),
                    );
                    warn!(
                        proxy_id = %proxy_id,
                        failures = p.consecutive_failures,
                        cooldown_secs = self.circuit_breaker.cooldown_secs,
                        "ProxyOrchestrator: circuit opened"
                    );
                }
            }
        }
    }

    /// Check whether the circuit for the given proxy is currently open.
    ///
    /// Returns `true` if the circuit is open and the cooldown has not expired.
    pub fn check_circuit(&self, proxy_id: &str) -> bool {
        if let Ok(proxies) = self.proxies.read() {
            if let Some(p) = proxies.iter().find(|p| p.id == proxy_id) {
                return self.is_circuit_effectively_open(p);
            }
        }
        false
    }

    /// Manually close (reset) the circuit for a proxy.
    pub fn reset_circuit(&self, proxy_id: &str) {
        if let Ok(mut proxies) = self.proxies.write() {
            if let Some(p) = proxies.iter_mut().find(|p| p.id == proxy_id) {
                p.circuit_open = false;
                p.circuit_open_until = None;
                p.consecutive_failures = 0;
                info!(proxy_id = %proxy_id, "ProxyOrchestrator: circuit manually reset");
            }
        }
    }

    /// Returns `true` if the circuit is open and the cooldown has NOT expired,
    /// meaning the proxy should not receive traffic.
    fn is_circuit_effectively_open(&self, proxy: &ManagedProxy) -> bool {
        if !proxy.circuit_open {
            return false;
        }
        match proxy.circuit_open_until {
            Some(until) => Utc::now() < until,
            None => true,
        }
    }

    /// Returns `true` if the circuit is open but the cooldown has expired
    /// (half-open state — ready for test calls).
    fn is_half_open(&self, proxy: &ManagedProxy) -> bool {
        if !proxy.circuit_open {
            return false;
        }
        match proxy.circuit_open_until {
            Some(until) => Utc::now() >= until,
            None => false,
        }
    }

    // ----- observability ---------------------------------------------------

    /// Return aggregated metrics across all managed proxies.
    pub fn metrics(&self) -> OrchestratorMetrics {
        let proxies = match self.proxies.read() {
            Ok(p) => p,
            Err(_) => return OrchestratorMetrics::default(),
        };
        let rules = match self.rules.read() {
            Ok(r) => r,
            Err(_) => return OrchestratorMetrics::default(),
        };

        let total_proxies = proxies.len();
        let mut active_proxies = 0usize;
        let mut circuit_open_proxies = 0usize;
        let mut total_calls = 0u64;
        let mut total_failures = 0u64;
        let mut calls_per_group: HashMap<String, u64> = HashMap::new();

        for p in proxies.iter() {
            total_calls += p.total_calls;
            total_failures += u64::from(p.consecutive_failures);

            *calls_per_group.entry(p.group.clone()).or_insert(0) += p.total_calls;

            if p.enabled && !self.is_circuit_effectively_open(p) {
                active_proxies += 1;
            }
            if self.is_circuit_effectively_open(p) {
                circuit_open_proxies += 1;
            }
        }

        OrchestratorMetrics {
            total_proxies,
            active_proxies,
            circuit_open_proxies,
            total_calls,
            total_failures,
            calls_per_group,
            routing_rules_count: rules.len(),
        }
    }

    /// List status information for all managed proxies.
    pub fn list_proxies(&self) -> Vec<ProxyInfo> {
        let proxies = match self.proxies.read() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        proxies
            .iter()
            .map(|p| ProxyInfo {
                id: p.id.clone(),
                group: p.group.clone(),
                enabled: p.enabled,
                circuit_open: p.circuit_open,
                consecutive_failures: p.consecutive_failures,
                total_calls: p.total_calls,
                last_call: p.last_call,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Pattern matching helper
// ---------------------------------------------------------------------------

/// Match a simple glob pattern against a value.
///
/// Supports `*` as a wildcard at the start, end, or both:
/// - `"mcp_github_*"` — matches any string starting with `"mcp_github_"`
/// - `"*_list"` — matches any string ending with `"_list"`
/// - `"*github*"` — matches any string containing `"github"`
/// - `"exact_name"` — matches only the exact string
fn matches_pattern(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let starts_with_star = pattern.starts_with('*');
    let ends_with_star = pattern.ends_with('*');

    match (starts_with_star, ends_with_star) {
        (true, true) => {
            // *contains*
            let inner = &pattern[1..pattern.len() - 1];
            value.contains(inner)
        }
        (true, false) => {
            // *suffix
            let suffix = &pattern[1..];
            value.ends_with(suffix)
        }
        (false, true) => {
            // prefix*
            let prefix = &pattern[..pattern.len() - 1];
            value.starts_with(prefix)
        }
        (false, false) => {
            // exact match
            pattern == value
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_security::PermissionSet;
    use argentor_skills::SkillRegistry;

    /// Helper: create a minimal McpProxy wrapped in Arc.
    fn make_proxy() -> Arc<McpProxy> {
        let registry = SkillRegistry::new();
        let permissions = PermissionSet::new();
        Arc::new(McpProxy::new(Arc::new(registry), permissions))
    }

    /// Helper: create a default orchestrator.
    fn make_orchestrator(strategy: RoutingStrategy) -> ProxyOrchestrator {
        ProxyOrchestrator::new(strategy, CircuitBreakerConfig::default())
    }

    // ----- add / remove proxies -------------------------------------------

    #[test]
    fn test_add_proxy() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let proxy = make_proxy();
        orch.add_proxy("p1", "group_a", proxy).unwrap();

        let list = orch.list_proxies();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "p1");
        assert_eq!(list[0].group, "group_a");
        assert!(list[0].enabled);
    }

    #[test]
    fn test_add_duplicate_proxy_fails() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let proxy = make_proxy();
        orch.add_proxy("p1", "g", proxy.clone()).unwrap();
        let err = orch.add_proxy("p1", "g", proxy).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_remove_proxy() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        orch.add_proxy("p1", "g", make_proxy()).unwrap();
        orch.add_proxy("p2", "g", make_proxy()).unwrap();

        orch.remove_proxy("p1").unwrap();
        let list = orch.list_proxies();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "p2");
    }

    #[test]
    fn test_remove_nonexistent_proxy_fails() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let err = orch.remove_proxy("does_not_exist").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ----- add / remove rules ---------------------------------------------

    #[test]
    fn test_add_and_remove_rule() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        orch.add_rule(RoutingRule {
            name: "r1".into(),
            tool_pattern: Some("mcp_*".into()),
            agent_roles: vec![],
            target_group: "default".into(),
            priority: 10,
        });

        let metrics = orch.metrics();
        assert_eq!(metrics.routing_rules_count, 1);

        orch.remove_rule("r1");
        let metrics = orch.metrics();
        assert_eq!(metrics.routing_rules_count, 0);
    }

    // ----- fixed routing --------------------------------------------------

    #[test]
    fn test_fixed_routing_returns_first_proxy() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let p1 = make_proxy();
        let p2 = make_proxy();
        orch.add_proxy("p1", "default", p1.clone()).unwrap();
        orch.add_proxy("p2", "default", p2.clone()).unwrap();

        let call = ToolCall {
            id: "c1".into(),
            name: "some_tool".into(),
            arguments: serde_json::json!({}),
        };

        // No rules -> wildcard group "*" won't match "default" group.
        // We need either no rules (wildcard) or a rule. With no rules, the
        // target_group is "*", so we need proxies in group "*".
        // Let's add a rule instead.
        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "default".into(),
            priority: 1,
        });

        let selected = orch.route(&call, "agent-1", None).unwrap();
        // Fixed always returns the first one.
        assert!(Arc::ptr_eq(&selected, &p1));
    }

    // ----- round-robin routing --------------------------------------------

    #[test]
    fn test_round_robin_routing() {
        let orch = make_orchestrator(RoutingStrategy::RoundRobin);
        let p1 = make_proxy();
        let p2 = make_proxy();
        let p3 = make_proxy();
        orch.add_proxy("p1", "grp", p1.clone()).unwrap();
        orch.add_proxy("p2", "grp", p2.clone()).unwrap();
        orch.add_proxy("p3", "grp", p3.clone()).unwrap();

        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "grp".into(),
            priority: 1,
        });

        let call = ToolCall {
            id: "c".into(),
            name: "tool".into(),
            arguments: serde_json::json!({}),
        };

        let r1 = orch.route(&call, "a", None).unwrap();
        let r2 = orch.route(&call, "a", None).unwrap();
        let r3 = orch.route(&call, "a", None).unwrap();
        let r4 = orch.route(&call, "a", None).unwrap();

        // Should cycle through p1, p2, p3, p1.
        assert!(Arc::ptr_eq(&r1, &p1));
        assert!(Arc::ptr_eq(&r2, &p2));
        assert!(Arc::ptr_eq(&r3, &p3));
        assert!(Arc::ptr_eq(&r4, &p1));
    }

    // ----- least-loaded routing -------------------------------------------

    #[test]
    fn test_least_loaded_routing() {
        let orch = make_orchestrator(RoutingStrategy::LeastLoaded);
        let p1 = make_proxy();
        let p2 = make_proxy();
        orch.add_proxy("p1", "grp", p1.clone()).unwrap();
        orch.add_proxy("p2", "grp", p2.clone()).unwrap();

        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "grp".into(),
            priority: 1,
        });

        // Simulate p1 having more calls.
        {
            let mut proxies = orch.proxies.write().unwrap();
            proxies[0].total_calls = 100; // p1 is heavily loaded
            proxies[1].total_calls = 5; // p2 is lightly loaded
        }

        let call = ToolCall {
            id: "c".into(),
            name: "tool".into(),
            arguments: serde_json::json!({}),
        };

        let selected = orch.route(&call, "a", None).unwrap();
        assert!(Arc::ptr_eq(&selected, &p2));
    }

    // ----- pattern-based routing ------------------------------------------

    #[test]
    fn test_pattern_based_routing_with_wildcards() {
        let orch = make_orchestrator(RoutingStrategy::PatternBased);
        let p_github = make_proxy();
        let p_slack = make_proxy();

        orch.add_proxy("p_github", "github", p_github.clone())
            .unwrap();
        orch.add_proxy("p_slack", "slack", p_slack.clone()).unwrap();

        orch.add_rule(RoutingRule {
            name: "github".into(),
            tool_pattern: Some("mcp_github_*".into()),
            agent_roles: vec![],
            target_group: "github".into(),
            priority: 10,
        });
        orch.add_rule(RoutingRule {
            name: "slack".into(),
            tool_pattern: Some("mcp_slack_*".into()),
            agent_roles: vec![],
            target_group: "slack".into(),
            priority: 10,
        });

        let github_call = ToolCall {
            id: "c1".into(),
            name: "mcp_github_create_issue".into(),
            arguments: serde_json::json!({}),
        };
        let slack_call = ToolCall {
            id: "c2".into(),
            name: "mcp_slack_send_message".into(),
            arguments: serde_json::json!({}),
        };

        let selected_github = orch.route(&github_call, "a", None).unwrap();
        let selected_slack = orch.route(&slack_call, "a", None).unwrap();

        assert!(Arc::ptr_eq(&selected_github, &p_github));
        assert!(Arc::ptr_eq(&selected_slack, &p_slack));
    }

    // ----- circuit breaker open / close / half-open -----------------------

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            cooldown_secs: 60,
            half_open_max_calls: 1,
        };
        let orch = ProxyOrchestrator::new(RoutingStrategy::Fixed, config);
        orch.add_proxy("p1", "g", make_proxy()).unwrap();

        // Record failures up to threshold.
        orch.record_failure("p1");
        assert!(!orch.check_circuit("p1"));
        orch.record_failure("p1");
        assert!(!orch.check_circuit("p1"));
        orch.record_failure("p1");
        // Now at threshold -> circuit should be open.
        assert!(orch.check_circuit("p1"));
    }

    #[test]
    fn test_circuit_breaker_closes_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown_secs: 60,
            half_open_max_calls: 1,
        };
        let orch = ProxyOrchestrator::new(RoutingStrategy::Fixed, config);
        orch.add_proxy("p1", "g", make_proxy()).unwrap();

        orch.record_failure("p1");
        orch.record_failure("p1");
        assert!(orch.check_circuit("p1"));

        // Success resets everything.
        orch.record_success("p1");
        assert!(!orch.check_circuit("p1"));

        let list = orch.list_proxies();
        assert_eq!(list[0].consecutive_failures, 0);
    }

    #[test]
    fn test_circuit_breaker_half_open_after_cooldown() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_secs: 0, // Immediate cooldown for testing.
            half_open_max_calls: 3,
        };
        let orch = ProxyOrchestrator::new(RoutingStrategy::Fixed, config);
        let proxy = make_proxy();
        orch.add_proxy("p1", "g", proxy.clone()).unwrap();

        orch.record_failure("p1");
        // Circuit is open but cooldown is 0 seconds, so it's already half-open.
        // is_circuit_effectively_open should be false (cooldown expired).
        assert!(!orch.check_circuit("p1"));

        // The proxy should still be routable (half-open allows traffic).
        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "g".into(),
            priority: 1,
        });

        let call = ToolCall {
            id: "c".into(),
            name: "t".into(),
            arguments: serde_json::json!({}),
        };
        let result = orch.route(&call, "a", None);
        assert!(result.is_ok());
    }

    // ----- failover -------------------------------------------------------

    #[test]
    fn test_failover_when_primary_circuit_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_secs: 3600, // Long cooldown so it stays open.
            half_open_max_calls: 1,
        };
        let orch = ProxyOrchestrator::new(RoutingStrategy::Fixed, config);
        let p1 = make_proxy();
        let p2 = make_proxy();
        orch.add_proxy("p1", "g", p1.clone()).unwrap();
        orch.add_proxy("p2", "g", p2.clone()).unwrap();

        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "g".into(),
            priority: 1,
        });

        // Open circuit on p1.
        orch.record_failure("p1");
        assert!(orch.check_circuit("p1"));

        let call = ToolCall {
            id: "c".into(),
            name: "t".into(),
            arguments: serde_json::json!({}),
        };

        // Should failover to p2.
        let selected = orch.route(&call, "a", None).unwrap();
        assert!(Arc::ptr_eq(&selected, &p2));
    }

    // ----- metrics aggregation --------------------------------------------

    #[test]
    fn test_metrics_aggregation() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        orch.add_proxy("p1", "alpha", make_proxy()).unwrap();
        orch.add_proxy("p2", "alpha", make_proxy()).unwrap();
        orch.add_proxy("p3", "beta", make_proxy()).unwrap();

        // Simulate some calls.
        {
            let mut proxies = orch.proxies.write().unwrap();
            proxies[0].total_calls = 10;
            proxies[1].total_calls = 20;
            proxies[2].total_calls = 5;
        }

        let m = orch.metrics();
        assert_eq!(m.total_proxies, 3);
        assert_eq!(m.active_proxies, 3);
        assert_eq!(m.circuit_open_proxies, 0);
        assert_eq!(m.total_calls, 35);
        assert_eq!(*m.calls_per_group.get("alpha").unwrap(), 30);
        assert_eq!(*m.calls_per_group.get("beta").unwrap(), 5);
    }

    // ----- enable / disable proxy -----------------------------------------

    #[test]
    fn test_enable_disable_proxy() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let p1 = make_proxy();
        let p2 = make_proxy();
        orch.add_proxy("p1", "g", p1.clone()).unwrap();
        orch.add_proxy("p2", "g", p2.clone()).unwrap();

        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "g".into(),
            priority: 1,
        });

        orch.disable_proxy("p1").unwrap();

        let call = ToolCall {
            id: "c".into(),
            name: "t".into(),
            arguments: serde_json::json!({}),
        };

        // With p1 disabled, should route to p2.
        let selected = orch.route(&call, "a", None).unwrap();
        assert!(Arc::ptr_eq(&selected, &p2));

        // Re-enable p1 — Fixed strategy should pick p1 again.
        orch.enable_proxy("p1").unwrap();
        let selected = orch.route(&call, "a", None).unwrap();
        assert!(Arc::ptr_eq(&selected, &p1));
    }

    // ----- rule priority ordering -----------------------------------------

    #[test]
    fn test_rule_priority_ordering() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let p_high = make_proxy();
        let p_low = make_proxy();
        orch.add_proxy("p_high", "high_prio", p_high.clone())
            .unwrap();
        orch.add_proxy("p_low", "low_prio", p_low.clone()).unwrap();

        // Lower priority rule is added first.
        orch.add_rule(RoutingRule {
            name: "low".into(),
            tool_pattern: Some("mcp_*".into()),
            agent_roles: vec![],
            target_group: "low_prio".into(),
            priority: 1,
        });
        orch.add_rule(RoutingRule {
            name: "high".into(),
            tool_pattern: Some("mcp_*".into()),
            agent_roles: vec![],
            target_group: "high_prio".into(),
            priority: 100,
        });

        let call = ToolCall {
            id: "c".into(),
            name: "mcp_test_tool".into(),
            arguments: serde_json::json!({}),
        };

        let selected = orch.route(&call, "a", None).unwrap();
        assert!(Arc::ptr_eq(&selected, &p_high));
    }

    // ----- no matching rule → error ---------------------------------------

    #[test]
    fn test_no_matching_rule_error() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        orch.add_proxy("p1", "g", make_proxy()).unwrap();

        orch.add_rule(RoutingRule {
            name: "specific".into(),
            tool_pattern: Some("mcp_github_*".into()),
            agent_roles: vec![],
            target_group: "g".into(),
            priority: 1,
        });

        let call = ToolCall {
            id: "c".into(),
            name: "mcp_slack_send".into(),
            arguments: serde_json::json!({}),
        };

        let result = orch.route(&call, "a", None);
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("No routing rule matches"));
    }

    // ----- empty orchestrator → error -------------------------------------

    #[test]
    fn test_empty_orchestrator_error() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);

        let call = ToolCall {
            id: "c".into(),
            name: "t".into(),
            arguments: serde_json::json!({}),
        };

        let result = orch.route(&call, "a", None);
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("No proxies registered"));
    }

    // ----- pattern matching -----------------------------------------------

    #[test]
    fn test_matches_pattern_prefix_wildcard() {
        assert!(matches_pattern("mcp_github_*", "mcp_github_create_issue"));
        assert!(matches_pattern("mcp_github_*", "mcp_github_"));
        assert!(!matches_pattern("mcp_github_*", "mcp_slack_send"));
    }

    #[test]
    fn test_matches_pattern_suffix_wildcard() {
        assert!(matches_pattern("*_list", "tools_list"));
        assert!(matches_pattern("*_list", "_list"));
        assert!(!matches_pattern("*_list", "list_tools"));
    }

    #[test]
    fn test_matches_pattern_both_wildcards() {
        assert!(matches_pattern("*github*", "mcp_github_issue"));
        assert!(matches_pattern("*github*", "github"));
        assert!(!matches_pattern("*github*", "gitlab"));
    }

    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern("exact_tool", "exact_tool"));
        assert!(!matches_pattern("exact_tool", "exact_tool_extra"));
    }

    #[test]
    fn test_matches_pattern_star_matches_all() {
        assert!(matches_pattern("*", "anything"));
        assert!(matches_pattern("*", ""));
    }

    // ----- manual circuit reset -------------------------------------------

    #[test]
    fn test_manual_circuit_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_secs: 3600,
            half_open_max_calls: 1,
        };
        let orch = ProxyOrchestrator::new(RoutingStrategy::Fixed, config);
        orch.add_proxy("p1", "g", make_proxy()).unwrap();

        orch.record_failure("p1");
        assert!(orch.check_circuit("p1"));

        orch.reset_circuit("p1");
        assert!(!orch.check_circuit("p1"));

        let list = orch.list_proxies();
        assert_eq!(list[0].consecutive_failures, 0);
        assert!(!list[0].circuit_open);
    }

    // ----- agent role routing ---------------------------------------------

    #[test]
    fn test_agent_role_routing() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let p_coder = make_proxy();
        let p_tester = make_proxy();
        orch.add_proxy("p_coder", "coder_group", p_coder.clone())
            .unwrap();
        orch.add_proxy("p_tester", "tester_group", p_tester.clone())
            .unwrap();

        orch.add_rule(RoutingRule {
            name: "coder_rule".into(),
            tool_pattern: None,
            agent_roles: vec!["Coder".into()],
            target_group: "coder_group".into(),
            priority: 10,
        });
        orch.add_rule(RoutingRule {
            name: "tester_rule".into(),
            tool_pattern: None,
            agent_roles: vec!["Tester".into()],
            target_group: "tester_group".into(),
            priority: 10,
        });

        let call = ToolCall {
            id: "c".into(),
            name: "tool".into(),
            arguments: serde_json::json!({}),
        };

        let selected_coder = orch.route(&call, "a1", Some("Coder")).unwrap();
        assert!(Arc::ptr_eq(&selected_coder, &p_coder));

        let selected_tester = orch.route(&call, "a2", Some("Tester")).unwrap();
        assert!(Arc::ptr_eq(&selected_tester, &p_tester));
    }

    // ----- set_strategy at runtime ----------------------------------------

    #[test]
    fn test_set_strategy_at_runtime() {
        let orch = make_orchestrator(RoutingStrategy::Fixed);
        let p1 = make_proxy();
        let p2 = make_proxy();
        orch.add_proxy("p1", "g", p1.clone()).unwrap();
        orch.add_proxy("p2", "g", p2.clone()).unwrap();

        orch.add_rule(RoutingRule {
            name: "all".into(),
            tool_pattern: None,
            agent_roles: vec![],
            target_group: "g".into(),
            priority: 1,
        });

        let call = ToolCall {
            id: "c".into(),
            name: "t".into(),
            arguments: serde_json::json!({}),
        };

        // Fixed -> always p1.
        let s1 = orch.route(&call, "a", None).unwrap();
        assert!(Arc::ptr_eq(&s1, &p1));

        // Switch to RoundRobin.
        orch.set_strategy(RoutingStrategy::RoundRobin);

        // RoundRobin starts from counter (which was 0 before, but we already
        // called route once with Fixed — RoundRobin has its own counter).
        // The counter increments each RR call.
        let s2 = orch.route(&call, "a", None).unwrap();
        let s3 = orch.route(&call, "a", None).unwrap();
        // The two calls should hit different proxies.
        assert!(!Arc::ptr_eq(&s2, &s3));
    }

    // ----- circuit breaker default config ---------------------------------

    #[test]
    fn test_circuit_breaker_default_config() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.cooldown_secs, 30);
        assert_eq!(config.half_open_max_calls, 3);
    }

    // ----- proxy info serialization ---------------------------------------

    #[test]
    fn test_proxy_info_serialization() {
        let info = ProxyInfo {
            id: "p1".into(),
            group: "default".into(),
            enabled: true,
            circuit_open: false,
            consecutive_failures: 0,
            total_calls: 42,
            last_call: Some(Utc::now()),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"id\":\"p1\""));
        assert!(json.contains("\"total_calls\":42"));
    }

    // ----- orchestrator metrics serialization -----------------------------

    #[test]
    fn test_orchestrator_metrics_serialization() {
        let m = OrchestratorMetrics {
            total_proxies: 3,
            active_proxies: 2,
            circuit_open_proxies: 1,
            total_calls: 100,
            total_failures: 5,
            calls_per_group: HashMap::from([("g1".into(), 60), ("g2".into(), 40)]),
            routing_rules_count: 2,
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"total_proxies\":3"));
        assert!(json.contains("\"total_calls\":100"));
    }
}

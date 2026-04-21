//! Permission modes control how the agent handles tool authorization.
//!
//! Inspired by Claude Agent SDK's permission system but extended
//! with Argentor's capability model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Permission modes control how the agent handles tool authorization.
///
/// Each mode provides a different trade-off between security and convenience,
/// ranging from fully locked-down (`Strict`) to wide-open (`Permissive`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermissionMode {
    /// Default: tools execute if capabilities match. Unknown tools prompt for approval.
    Default,

    /// Strict: only explicitly allowed tools can execute. Everything else is denied.
    /// Like Claude Agent SDK's "dontAsk".
    Strict,

    /// Permissive: all tools execute without checking capabilities.
    /// Only use in sandboxed environments (WASM, Docker).
    /// Like Claude Agent SDK's "bypassPermissions".
    Permissive,

    /// Plan-only: the agent reasons and plans but NEVER executes tools.
    /// Tool calls are captured but not executed. Useful for dry-runs.
    /// Like Claude Agent SDK's "plan" mode.
    PlanOnly,

    /// Auto-approve reads, require approval for writes.
    /// File reads, searches, and queries auto-approved.
    /// Writes, shell commands, and network calls require explicit allow.
    /// Like Claude Agent SDK's "acceptEdits".
    ReadOnly,

    /// Custom: user provides an approval callback for each tool call.
    Custom,
}

/// Result of a permission check.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Tool is allowed to execute.
    Allow,
    /// Tool execution is denied.
    Deny {
        /// Human-readable reason for the denial.
        reason: String,
    },
    /// Tool call is captured but not executed (plan mode).
    Captured {
        /// Name of the tool that was captured.
        tool_name: String,
        /// Arguments that were captured.
        arguments: serde_json::Value,
    },
    /// Requires user approval (callback).
    RequiresApproval {
        /// Name of the tool that requires approval.
        tool_name: String,
        /// Human-readable description of why approval is needed.
        description: String,
    },
}

/// A captured tool call in PlanOnly mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedCall {
    /// Name of the tool that was called.
    pub tool_name: String,
    /// Arguments that were passed.
    pub arguments: serde_json::Value,
    /// When the call was captured.
    pub timestamp: DateTime<Utc>,
    /// Capabilities that would be needed to actually execute the call.
    pub would_require: Vec<String>,
}

/// Read-only tool name patterns that `ReadOnly` mode auto-approves.
const READ_PATTERNS: &[&str] = &[
    "file_read",
    "search",
    "query",
    "list",
    "get",
    "describe",
    "show",
    "help",
    "echo",
    "time",
    "memory_search",
];

/// Write-like tool name patterns that `ReadOnly` mode always denies.
const WRITE_PATTERNS: &[&str] = &[
    "file_write",
    "shell",
    "exec",
    "run",
    "delete",
    "remove",
    "create",
    "update",
    "put",
    "post",
    "send",
    "deploy",
    "network",
    "http",
    "memory_store",
];

/// Type alias for the approval callback used in Custom permission mode.
type ApprovalCallback = Box<dyn Fn(&str, &serde_json::Value) -> bool + Send + Sync>;

/// Permission mode evaluator.
///
/// Holds the active mode, allow/deny lists, captured calls for plan mode,
/// and an optional approval callback for custom mode.
pub struct PermissionEvaluator {
    mode: PermissionMode,
    /// Explicit allowlist (tool names or patterns with `*` wildcards).
    allowed_tools: Vec<String>,
    /// Explicit denylist (always denied, even in Permissive mode).
    denied_tools: Vec<String>,
    /// Captured calls in PlanOnly mode.
    captured_calls: Mutex<Vec<CapturedCall>>,
    /// Optional callback for Custom mode.
    approval_callback: Option<ApprovalCallback>,
}

impl std::fmt::Debug for PermissionEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionEvaluator")
            .field("mode", &self.mode)
            .field("allowed_tools", &self.allowed_tools)
            .field("denied_tools", &self.denied_tools)
            .field(
                "captured_calls_count",
                &self.captured_calls.lock().map(|c| c.len()).unwrap_or(0),
            )
            .field("has_callback", &self.approval_callback.is_some())
            .finish()
    }
}

impl PermissionEvaluator {
    /// Create a new evaluator with the given mode.
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            captured_calls: Mutex::new(Vec::new()),
            approval_callback: None,
        }
    }

    /// Set the explicit allowlist (builder pattern).
    pub fn with_allowed(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    /// Set the explicit denylist (builder pattern).
    pub fn with_denied(mut self, tools: Vec<String>) -> Self {
        self.denied_tools = tools;
        self
    }

    /// Set the approval callback for Custom mode (builder pattern).
    pub fn with_approval_callback<F>(mut self, f: F) -> Self
    where
        F: Fn(&str, &serde_json::Value) -> bool + Send + Sync + 'static,
    {
        self.approval_callback = Some(Box::new(f));
        self
    }

    /// Get the current permission mode.
    pub fn mode(&self) -> &PermissionMode {
        &self.mode
    }

    /// Check if a tool call is permitted under the current mode.
    pub fn check(&self, tool_name: &str, arguments: &serde_json::Value) -> PermissionDecision {
        // Denylist always takes precedence, regardless of mode
        if self.is_denied(tool_name) {
            return PermissionDecision::Deny {
                reason: format!("Tool '{tool_name}' is in the denylist"),
            };
        }

        match &self.mode {
            PermissionMode::Default => self.check_default(tool_name),
            PermissionMode::Strict => self.check_strict(tool_name),
            PermissionMode::Permissive => PermissionDecision::Allow,
            PermissionMode::PlanOnly => self.check_plan_only(tool_name, arguments),
            PermissionMode::ReadOnly => self.check_read_only(tool_name),
            PermissionMode::Custom => self.check_custom(tool_name, arguments),
        }
    }

    /// Get all captured calls (PlanOnly mode).
    pub fn captured_calls(&self) -> Vec<CapturedCall> {
        self.captured_calls
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// Clear captured calls.
    pub fn clear_captured(&self) {
        if let Ok(mut calls) = self.captured_calls.lock() {
            calls.clear();
        }
    }

    // -- Mode-specific checks --------------------------------------------------

    /// Default mode: allowlist first, then prompt for unknown.
    fn check_default(&self, tool_name: &str) -> PermissionDecision {
        if self.is_allowed(tool_name) {
            return PermissionDecision::Allow;
        }
        // Unknown tools require approval
        PermissionDecision::RequiresApproval {
            tool_name: tool_name.to_string(),
            description: format!("Tool '{tool_name}' is not in the allowlist; approval required"),
        }
    }

    /// Strict mode: only allowlist passes.
    fn check_strict(&self, tool_name: &str) -> PermissionDecision {
        if self.is_allowed(tool_name) {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny {
                reason: format!("Tool '{tool_name}' is not in the allowlist (strict mode)"),
            }
        }
    }

    /// PlanOnly mode: capture the call, never execute.
    fn check_plan_only(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> PermissionDecision {
        let would_require = infer_required_capabilities(tool_name);
        let captured = CapturedCall {
            tool_name: tool_name.to_string(),
            arguments: arguments.clone(),
            timestamp: Utc::now(),
            would_require,
        };
        if let Ok(mut calls) = self.captured_calls.lock() {
            calls.push(captured);
        }
        PermissionDecision::Captured {
            tool_name: tool_name.to_string(),
            arguments: arguments.clone(),
        }
    }

    /// ReadOnly mode: auto-approve reads, deny writes.
    fn check_read_only(&self, tool_name: &str) -> PermissionDecision {
        // Explicitly allowed tools always pass
        if self.is_allowed(tool_name) {
            return PermissionDecision::Allow;
        }
        let lower = tool_name.to_lowercase();
        // Check if it matches a read pattern
        if READ_PATTERNS.iter().any(|p| lower.contains(p)) {
            return PermissionDecision::Allow;
        }
        // Check if it matches a write pattern
        if WRITE_PATTERNS.iter().any(|p| lower.contains(p)) {
            return PermissionDecision::Deny {
                reason: format!("Tool '{tool_name}' looks like a write operation (read-only mode)"),
            };
        }
        // Unknown tools are denied in read-only mode for safety
        PermissionDecision::Deny {
            reason: format!(
                "Tool '{tool_name}' is not recognized as a read operation (read-only mode)"
            ),
        }
    }

    /// Custom mode: call the approval callback.
    fn check_custom(&self, tool_name: &str, arguments: &serde_json::Value) -> PermissionDecision {
        match &self.approval_callback {
            Some(callback) => {
                if callback(tool_name, arguments) {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny {
                        reason: format!("Tool '{tool_name}' denied by custom approval callback"),
                    }
                }
            }
            None => PermissionDecision::Deny {
                reason: "Custom mode requires an approval callback, but none was set".to_string(),
            },
        }
    }

    // -- Pattern matching helpers -----------------------------------------------

    /// Check if a tool name matches any pattern in the allowlist.
    fn is_allowed(&self, tool_name: &str) -> bool {
        self.allowed_tools
            .iter()
            .any(|pattern| Self::matches_pattern(pattern, tool_name))
    }

    /// Check if a tool name matches any pattern in the denylist.
    fn is_denied(&self, tool_name: &str) -> bool {
        self.denied_tools
            .iter()
            .any(|pattern| Self::matches_pattern(pattern, tool_name))
    }

    /// Check if a tool name matches a pattern (supports `*` wildcards).
    ///
    /// - `"*"` matches everything
    /// - `"mcp__github__*"` matches all tools starting with `mcp__github__`
    /// - `"file_*"` matches `file_read`, `file_write`, etc.
    /// - `"*_search"` matches `memory_search`, `web_search`, etc.
    /// - `"mcp__*__list"` matches `mcp__github__list`, `mcp__slack__list`, etc.
    fn matches_pattern(pattern: &str, tool_name: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if !pattern.contains('*') {
            return pattern == tool_name;
        }
        // Split on '*' and check that all parts appear in order
        let parts: Vec<&str> = pattern.split('*').collect();
        let mut remaining = tool_name;

        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            if i == 0 {
                // First part must be a prefix
                if let Some(rest) = remaining.strip_prefix(part) {
                    remaining = rest;
                } else {
                    return false;
                }
            } else if i == parts.len() - 1 {
                // Last part must be a suffix
                if !remaining.ends_with(part) {
                    return false;
                }
                remaining = "";
            } else {
                // Middle parts must appear somewhere in order
                if let Some(pos) = remaining.find(part) {
                    remaining = &remaining[pos + part.len()..];
                } else {
                    return false;
                }
            }
        }
        true
    }
}

/// Infer which capabilities a tool would require based on its name.
/// This is a best-effort heuristic for the `would_require` field in `CapturedCall`.
fn infer_required_capabilities(tool_name: &str) -> Vec<String> {
    let lower = tool_name.to_lowercase();
    let mut caps = Vec::new();
    if lower.contains("file_read") || lower.contains("search") || lower.contains("get") {
        caps.push("file_read".to_string());
    }
    if lower.contains("file_write") || lower.contains("create") || lower.contains("update") {
        caps.push("file_write".to_string());
    }
    if lower.contains("shell") || lower.contains("exec") || lower.contains("run") {
        caps.push("shell_exec".to_string());
    }
    if lower.contains("network") || lower.contains("http") || lower.contains("fetch") {
        caps.push("network_access".to_string());
    }
    if lower.contains("env") {
        caps.push("env_read".to_string());
    }
    if lower.contains("db") || lower.contains("query") || lower.contains("sql") {
        caps.push("database_query".to_string());
    }
    if lower.contains("browser") {
        caps.push("browser_access".to_string());
    }
    caps
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Pattern matching tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_pattern_wildcard_all() {
        assert!(PermissionEvaluator::matches_pattern("*", "anything"));
        assert!(PermissionEvaluator::matches_pattern("*", ""));
    }

    #[test]
    fn test_pattern_exact_match() {
        assert!(PermissionEvaluator::matches_pattern(
            "file_read",
            "file_read"
        ));
        assert!(!PermissionEvaluator::matches_pattern(
            "file_read",
            "file_write"
        ));
    }

    #[test]
    fn test_pattern_prefix_wildcard() {
        assert!(PermissionEvaluator::matches_pattern("file_*", "file_read"));
        assert!(PermissionEvaluator::matches_pattern("file_*", "file_write"));
        assert!(!PermissionEvaluator::matches_pattern(
            "file_*",
            "shell_exec"
        ));
    }

    #[test]
    fn test_pattern_suffix_wildcard() {
        assert!(PermissionEvaluator::matches_pattern(
            "*_search",
            "memory_search"
        ));
        assert!(PermissionEvaluator::matches_pattern(
            "*_search",
            "web_search"
        ));
        assert!(!PermissionEvaluator::matches_pattern(
            "*_search",
            "memory_store"
        ));
    }

    #[test]
    fn test_pattern_middle_wildcard() {
        assert!(PermissionEvaluator::matches_pattern(
            "mcp__*__list",
            "mcp__github__list"
        ));
        assert!(PermissionEvaluator::matches_pattern(
            "mcp__*__list",
            "mcp__slack__list"
        ));
        assert!(!PermissionEvaluator::matches_pattern(
            "mcp__*__list",
            "mcp__github__create"
        ));
    }

    #[test]
    fn test_pattern_mcp_github_wildcard() {
        assert!(PermissionEvaluator::matches_pattern(
            "mcp__github__*",
            "mcp__github__list_repos"
        ));
        assert!(PermissionEvaluator::matches_pattern(
            "mcp__github__*",
            "mcp__github__create_pr"
        ));
        assert!(!PermissionEvaluator::matches_pattern(
            "mcp__github__*",
            "mcp__slack__post"
        ));
    }

    #[test]
    fn test_pattern_no_match_partial() {
        assert!(!PermissionEvaluator::matches_pattern(
            "file_read",
            "file_reader"
        ));
        assert!(!PermissionEvaluator::matches_pattern(
            "file_read",
            "the_file_read"
        ));
    }

    // -----------------------------------------------------------------------
    // Default mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_default_mode_allowed_tool() {
        let eval = PermissionEvaluator::new(PermissionMode::Default)
            .with_allowed(vec!["echo".to_string(), "time".to_string()]);
        let decision = eval.check("echo", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn test_default_mode_unknown_tool_requires_approval() {
        let eval = PermissionEvaluator::new(PermissionMode::Default)
            .with_allowed(vec!["echo".to_string()]);
        let decision = eval.check("shell_exec", &serde_json::json!({}));
        assert!(matches!(
            decision,
            PermissionDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn test_default_mode_denied_tool() {
        let eval = PermissionEvaluator::new(PermissionMode::Default)
            .with_allowed(vec!["*".to_string()])
            .with_denied(vec!["dangerous_tool".to_string()]);
        let decision = eval.check("dangerous_tool", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    // -----------------------------------------------------------------------
    // Strict mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_strict_mode_allowed() {
        let eval = PermissionEvaluator::new(PermissionMode::Strict)
            .with_allowed(vec!["echo".to_string(), "file_read".to_string()]);
        assert!(matches!(
            eval.check("echo", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("file_read", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_strict_mode_denied_unlisted() {
        let eval =
            PermissionEvaluator::new(PermissionMode::Strict).with_allowed(vec!["echo".to_string()]);
        let decision = eval.check("shell_exec", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        if let PermissionDecision::Deny { reason } = decision {
            assert!(reason.contains("strict mode"));
        }
    }

    #[test]
    fn test_strict_mode_with_wildcard_pattern() {
        let eval = PermissionEvaluator::new(PermissionMode::Strict)
            .with_allowed(vec!["mcp__github__*".to_string()]);
        assert!(matches!(
            eval.check("mcp__github__list_repos", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("mcp__slack__post", &serde_json::json!({})),
            PermissionDecision::Deny { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // Permissive mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_permissive_mode_allows_everything() {
        let eval = PermissionEvaluator::new(PermissionMode::Permissive);
        assert!(matches!(
            eval.check("anything", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("shell_exec", &serde_json::json!({"cmd": "rm -rf /"})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_permissive_mode_denylist_overrides() {
        let eval = PermissionEvaluator::new(PermissionMode::Permissive)
            .with_denied(vec!["nuclear_launch".to_string()]);
        assert!(matches!(
            eval.check("echo", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        let decision = eval.check("nuclear_launch", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_permissive_mode_denylist_wildcard_overrides() {
        let eval = PermissionEvaluator::new(PermissionMode::Permissive)
            .with_denied(vec!["mcp__admin__*".to_string()]);
        assert!(matches!(
            eval.check("mcp__admin__delete_all", &serde_json::json!({})),
            PermissionDecision::Deny { .. }
        ));
        assert!(matches!(
            eval.check("mcp__github__list", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
    }

    // -----------------------------------------------------------------------
    // PlanOnly mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_plan_only_captures_calls() {
        let eval = PermissionEvaluator::new(PermissionMode::PlanOnly);
        let args = serde_json::json!({"path": "/tmp/test.txt"});
        let decision = eval.check("file_read", &args);
        assert!(matches!(decision, PermissionDecision::Captured { .. }));
        if let PermissionDecision::Captured {
            tool_name,
            arguments,
        } = decision
        {
            assert_eq!(tool_name, "file_read");
            assert_eq!(arguments, args);
        }
    }

    #[test]
    fn test_plan_only_accumulates_captures() {
        let eval = PermissionEvaluator::new(PermissionMode::PlanOnly);
        eval.check("file_read", &serde_json::json!({"path": "/a"}));
        eval.check("file_write", &serde_json::json!({"path": "/b"}));
        eval.check("shell_exec", &serde_json::json!({"cmd": "ls"}));

        let captured = eval.captured_calls();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0].tool_name, "file_read");
        assert_eq!(captured[1].tool_name, "file_write");
        assert_eq!(captured[2].tool_name, "shell_exec");
    }

    #[test]
    fn test_plan_only_clear_captured() {
        let eval = PermissionEvaluator::new(PermissionMode::PlanOnly);
        eval.check("echo", &serde_json::json!({}));
        assert_eq!(eval.captured_calls().len(), 1);
        eval.clear_captured();
        assert_eq!(eval.captured_calls().len(), 0);
    }

    #[test]
    fn test_plan_only_would_require_capabilities() {
        let eval = PermissionEvaluator::new(PermissionMode::PlanOnly);
        eval.check("file_read", &serde_json::json!({}));
        eval.check("shell_exec", &serde_json::json!({}));

        let captured = eval.captured_calls();
        assert!(captured[0].would_require.contains(&"file_read".to_string()));
        assert!(captured[1]
            .would_require
            .contains(&"shell_exec".to_string()));
    }

    #[test]
    fn test_plan_only_denylist_still_applies() {
        let eval = PermissionEvaluator::new(PermissionMode::PlanOnly)
            .with_denied(vec!["forbidden".to_string()]);
        let decision = eval.check("forbidden", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        // Should NOT be captured
        assert_eq!(eval.captured_calls().len(), 0);
    }

    // -----------------------------------------------------------------------
    // ReadOnly mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_only_approves_reads() {
        let eval = PermissionEvaluator::new(PermissionMode::ReadOnly);
        assert!(matches!(
            eval.check("file_read", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("memory_search", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("list_files", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_read_only_denies_writes() {
        let eval = PermissionEvaluator::new(PermissionMode::ReadOnly);
        let decision = eval.check("file_write", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        let decision = eval.check("shell_exec", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        let decision = eval.check("delete_record", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_read_only_denies_network() {
        let eval = PermissionEvaluator::new(PermissionMode::ReadOnly);
        let decision = eval.check("network_fetch", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        let decision = eval.check("http_post", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_read_only_allowlist_overrides_for_writes() {
        // Explicitly allowlisted tools pass even if they look like writes
        let eval = PermissionEvaluator::new(PermissionMode::ReadOnly)
            .with_allowed(vec!["file_write".to_string()]);
        assert!(matches!(
            eval.check("file_write", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_read_only_unknown_tool_denied() {
        let eval = PermissionEvaluator::new(PermissionMode::ReadOnly);
        let decision = eval.check("completely_unknown_tool", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_read_only_denies_memory_store() {
        let eval = PermissionEvaluator::new(PermissionMode::ReadOnly);
        let decision = eval.check("memory_store", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    // -----------------------------------------------------------------------
    // Custom mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_custom_mode_callback_allow() {
        let eval = PermissionEvaluator::new(PermissionMode::Custom)
            .with_approval_callback(|name, _args| name == "echo");
        assert!(matches!(
            eval.check("echo", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
    }

    #[test]
    fn test_custom_mode_callback_deny() {
        let eval = PermissionEvaluator::new(PermissionMode::Custom)
            .with_approval_callback(|name, _args| name == "echo");
        let decision = eval.check("shell_exec", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_custom_mode_no_callback_denies() {
        let eval = PermissionEvaluator::new(PermissionMode::Custom);
        let decision = eval.check("anything", &serde_json::json!({}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        if let PermissionDecision::Deny { reason } = decision {
            assert!(reason.contains("callback"));
        }
    }

    #[test]
    fn test_custom_mode_callback_uses_arguments() {
        let eval = PermissionEvaluator::new(PermissionMode::Custom).with_approval_callback(
            |_name, args| {
                // Only allow if "safe" flag is true
                args.get("safe").and_then(|v| v.as_bool()).unwrap_or(false)
            },
        );
        assert!(matches!(
            eval.check("any_tool", &serde_json::json!({"safe": true})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("any_tool", &serde_json::json!({"safe": false})),
            PermissionDecision::Deny { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // Allowlist / denylist interaction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_denylist_overrides_allowlist() {
        let eval = PermissionEvaluator::new(PermissionMode::Default)
            .with_allowed(vec!["*".to_string()])
            .with_denied(vec!["dangerous".to_string()]);
        assert!(matches!(
            eval.check("safe_tool", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("dangerous", &serde_json::json!({})),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_denylist_wildcard_overrides_allowlist() {
        let eval = PermissionEvaluator::new(PermissionMode::Strict)
            .with_allowed(vec!["mcp__*".to_string()])
            .with_denied(vec!["mcp__admin__*".to_string()]);
        assert!(matches!(
            eval.check("mcp__github__list", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            eval.check("mcp__admin__delete", &serde_json::json!({})),
            PermissionDecision::Deny { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // CapturedCall serialization
    // -----------------------------------------------------------------------

    #[test]
    fn test_captured_call_serialization() {
        let call = CapturedCall {
            tool_name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            timestamp: Utc::now(),
            would_require: vec!["file_read".to_string()],
        };
        let json = serde_json::to_string(&call).expect("serialize");
        let deserialized: CapturedCall = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.tool_name, "file_read");
        assert_eq!(
            deserialized.arguments,
            serde_json::json!({"path": "/tmp/test.txt"})
        );
        assert_eq!(deserialized.would_require, vec!["file_read".to_string()]);
    }

    #[test]
    fn test_captured_call_round_trip() {
        let eval = PermissionEvaluator::new(PermissionMode::PlanOnly);
        eval.check(
            "shell_exec",
            &serde_json::json!({"cmd": "ls -la", "cwd": "/tmp"}),
        );
        let captured = eval.captured_calls();
        let json = serde_json::to_string(&captured).expect("serialize vec");
        let deserialized: Vec<CapturedCall> = serde_json::from_str(&json).expect("deserialize vec");
        assert_eq!(deserialized.len(), 1);
        assert_eq!(deserialized[0].tool_name, "shell_exec");
        assert!(deserialized[0]
            .would_require
            .contains(&"shell_exec".to_string()));
    }

    // -----------------------------------------------------------------------
    // PermissionMode serialization
    // -----------------------------------------------------------------------

    #[test]
    fn test_permission_mode_serialization() {
        let modes = vec![
            PermissionMode::Default,
            PermissionMode::Strict,
            PermissionMode::Permissive,
            PermissionMode::PlanOnly,
            PermissionMode::ReadOnly,
            PermissionMode::Custom,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).expect("serialize mode");
            let deserialized: PermissionMode =
                serde_json::from_str(&json).expect("deserialize mode");
            assert_eq!(deserialized, mode);
        }
    }

    // -----------------------------------------------------------------------
    // Edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_allowlist_default_mode() {
        let eval = PermissionEvaluator::new(PermissionMode::Default);
        // No allowlist, everything requires approval
        assert!(matches!(
            eval.check("echo", &serde_json::json!({})),
            PermissionDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn test_empty_allowlist_strict_mode() {
        let eval = PermissionEvaluator::new(PermissionMode::Strict);
        // No allowlist, everything denied
        assert!(matches!(
            eval.check("echo", &serde_json::json!({})),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_infer_required_capabilities() {
        let caps = infer_required_capabilities("file_read");
        assert!(caps.contains(&"file_read".to_string()));

        let caps = infer_required_capabilities("shell_exec");
        assert!(caps.contains(&"shell_exec".to_string()));

        let caps = infer_required_capabilities("network_fetch");
        assert!(caps.contains(&"network_access".to_string()));

        let caps = infer_required_capabilities("db_query");
        assert!(caps.contains(&"database_query".to_string()));
    }

    #[test]
    fn test_debug_impl() {
        let eval = PermissionEvaluator::new(PermissionMode::Default)
            .with_allowed(vec!["echo".to_string()])
            .with_denied(vec!["rm".to_string()]);
        let debug = format!("{eval:?}");
        assert!(debug.contains("Default"));
        assert!(debug.contains("echo"));
        assert!(debug.contains("rm"));
    }
}

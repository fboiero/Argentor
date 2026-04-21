//! Hook system for intercepting tool calls and agent events.
//!
//! Inspired by the Claude Agent SDK hooks but with richer semantics:
//! pre/post tool use, pre/post LLM call, agent start/end, and a
//! first-deny-wins / first-modify-wins evaluation chain.
//!
//! # Example
//!
//! ```rust
//! use argentor_agent::hooks::{hook_fn, HookChain, HookDecision, HookEvent};
//!
//! let mut chain = HookChain::new();
//!
//! // Block any shell tool usage
//! chain.add(hook_fn(
//!     "deny-shell",
//!     Some("^shell$".to_string()),
//!     |_event| HookDecision::Deny {
//!         reason: "Shell access is not allowed".into(),
//!     },
//! ));
//!
//! // Log every tool call
//! chain.add(hook_fn("logger", None, |event| {
//!     if let HookEvent::PreToolUse { tool_name, .. } = event {
//!         eprintln!("Tool called: {tool_name}");
//!     }
//!     HookDecision::Continue
//! }));
//! ```

use serde_json;
use std::fmt;

// ---------------------------------------------------------------------------
// HookEvent
// ---------------------------------------------------------------------------

/// An event that can be intercepted by hooks.
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// Fired before a tool call is executed.
    PreToolUse {
        /// Name of the tool being invoked.
        tool_name: String,
        /// JSON arguments that will be passed to the tool.
        arguments: serde_json::Value,
        /// Unique identifier for this tool call.
        call_id: String,
    },
    /// Fired after a tool call completes.
    PostToolUse {
        /// Name of the tool that was invoked.
        tool_name: String,
        /// The textual result returned by the tool.
        result: String,
        /// Whether the tool execution ended in error.
        is_error: bool,
        /// Unique identifier for this tool call.
        call_id: String,
        /// How long the tool execution took, in milliseconds.
        duration_ms: u64,
    },
    /// Fired before calling the LLM.
    PreLlmCall {
        /// LLM provider name (e.g., `"openai"`, `"anthropic"`).
        provider: String,
        /// Number of messages in the context window.
        message_count: usize,
        /// Current turn number in the agentic loop.
        turn: u32,
    },
    /// Fired after the LLM responds.
    PostLlmCall {
        /// LLM provider name.
        provider: String,
        /// Response type: `"done"`, `"text"`, or `"tool_use"`.
        response_type: String,
        /// How long the LLM call took, in milliseconds.
        duration_ms: u64,
        /// Current turn number in the agentic loop.
        turn: u32,
    },
    /// Fired when the agent starts processing a request.
    AgentStart {
        /// Session identifier.
        session_id: String,
        /// The user's input message.
        input: String,
    },
    /// Fired when the agent finishes processing.
    AgentEnd {
        /// Session identifier.
        session_id: String,
        /// The final output produced by the agent.
        output: String,
        /// Total number of turns taken.
        turns: u32,
        /// Total tokens consumed across all LLM calls.
        total_tokens: u64,
    },
}

impl HookEvent {
    /// Return the tool name if this event is tool-related, `None` otherwise.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::PreToolUse { tool_name, .. } | Self::PostToolUse { tool_name, .. } => {
                Some(tool_name)
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// HookDecision
// ---------------------------------------------------------------------------

/// The decision returned by a hook after evaluating an event.
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// Allow the operation to proceed.
    Allow,
    /// Deny the operation with a reason.
    Deny {
        /// Human-readable explanation of why the operation was denied.
        reason: String,
    },
    /// Modify the input before proceeding (only meaningful for pre-events).
    Modify {
        /// Replacement arguments to use instead of the originals.
        new_arguments: serde_json::Value,
    },
    /// Skip this hook; let others decide.
    Continue,
}

impl fmt::Display for HookDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => write!(f, "Allow"),
            Self::Deny { reason } => write!(f, "Deny({reason})"),
            Self::Modify { .. } => write!(f, "Modify"),
            Self::Continue => write!(f, "Continue"),
        }
    }
}

impl HookDecision {
    /// `true` if this decision is `Deny`.
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    /// `true` if this decision is `Allow`.
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// `true` if this decision is `Modify`.
    pub fn is_modify(&self) -> bool {
        matches!(self, Self::Modify { .. })
    }

    /// `true` if this decision is `Continue`.
    pub fn is_continue(&self) -> bool {
        matches!(self, Self::Continue)
    }
}

// ---------------------------------------------------------------------------
// Hook trait
// ---------------------------------------------------------------------------

/// A hook that can intercept agent events.
///
/// Implement this trait for custom hooks, or use [`hook_fn`] for quick one-offs.
pub trait Hook: Send + Sync {
    /// Human-readable name for this hook (used in logs).
    fn name(&self) -> &str;

    /// Called for matching events. Return a decision.
    fn on_event(&self, event: &HookEvent) -> HookDecision;

    /// Pattern matcher for tool names. When `false`, the hook is skipped for
    /// tool-related events. Default: matches everything.
    fn matches(&self, _tool_name: &str) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// HookChain
// ---------------------------------------------------------------------------

/// Ordered chain of hooks evaluated for each event.
///
/// Evaluation semantics:
/// - First `Deny` wins — the operation is blocked immediately.
/// - First `Modify` wins — the arguments are replaced and remaining hooks
///   see the modified arguments.
/// - `Allow` stops further evaluation and allows the operation.
/// - `Continue` moves to the next hook.
/// - If all hooks return `Continue`, the overall decision is `Allow`.
pub struct HookChain {
    hooks: Vec<Box<dyn Hook>>,
}

impl Default for HookChain {
    fn default() -> Self {
        Self::new()
    }
}

impl HookChain {
    /// Create an empty hook chain.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Add a hook to the end of the chain.
    pub fn add(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Returns the number of hooks in the chain.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Returns `true` if the chain contains no hooks.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Evaluate all hooks for the given event.
    ///
    /// - First `Deny` wins (short-circuit).
    /// - First `Modify` wins (short-circuit).
    /// - `Allow` stops evaluation.
    /// - If all return `Continue`, the result is `Allow`.
    pub fn evaluate(&self, event: &HookEvent) -> HookDecision {
        let tool_name = event.tool_name();

        for hook in &self.hooks {
            // Skip hooks that don't match the tool name
            if let Some(tn) = tool_name {
                if !hook.matches(tn) {
                    continue;
                }
            }

            let decision = hook.on_event(event);
            match &decision {
                HookDecision::Deny { .. } | HookDecision::Modify { .. } | HookDecision::Allow => {
                    return decision;
                }
                HookDecision::Continue => {
                    // Next hook
                }
            }
        }

        // All hooks said Continue — default to Allow
        HookDecision::Allow
    }
}

// ---------------------------------------------------------------------------
// hook_fn — convenience constructor
// ---------------------------------------------------------------------------

/// A hook built from a closure with an optional regex pattern for tool name matching.
struct ClosureHook {
    hook_name: String,
    pattern: Option<regex::Regex>,
    handler: Box<dyn Fn(&HookEvent) -> HookDecision + Send + Sync>,
}

impl Hook for ClosureHook {
    fn name(&self) -> &str {
        &self.hook_name
    }

    fn on_event(&self, event: &HookEvent) -> HookDecision {
        (self.handler)(event)
    }

    fn matches(&self, tool_name: &str) -> bool {
        match &self.pattern {
            Some(re) => re.is_match(tool_name),
            None => true,
        }
    }
}

/// Create a hook from a closure, optionally scoped to tool names matching a regex.
///
/// # Arguments
///
/// - `name` — Human-readable hook name for logging.
/// - `matcher` — Optional regex pattern. When `Some`, the hook only fires for
///   tool-related events whose tool name matches the regex. When `None`, the
///   hook fires for all events.
/// - `handler` — The closure invoked for each matching event.
///
/// # Example
///
/// ```rust
/// use argentor_agent::hooks::{hook_fn, HookDecision, HookEvent};
///
/// let h = hook_fn("audit", None, |event| {
///     if let HookEvent::PostToolUse { tool_name, duration_ms, .. } = event {
///         eprintln!("[audit] {tool_name} took {duration_ms}ms");
///     }
///     HookDecision::Continue
/// });
/// ```
pub fn hook_fn(
    name: impl Into<String>,
    matcher: Option<String>,
    handler: impl Fn(&HookEvent) -> HookDecision + Send + Sync + 'static,
) -> Box<dyn Hook> {
    let pattern = matcher.map(|p| {
        regex::Regex::new(&p).unwrap_or_else(|e| {
            tracing::warn!("Invalid hook matcher regex '{p}': {e} — will match nothing");
            // Fallback: pattern that matches nothing
            #[allow(clippy::unwrap_used)] // static regex, infallible
            regex::Regex::new(r"^\b$").unwrap()
        })
    });

    Box::new(ClosureHook {
        hook_name: name.into(),
        pattern,
        handler: Box::new(handler),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pre_tool(name: &str, args: serde_json::Value) -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: name.to_string(),
            arguments: args,
            call_id: "call-1".to_string(),
        }
    }

    fn post_tool(name: &str, result: &str) -> HookEvent {
        HookEvent::PostToolUse {
            tool_name: name.to_string(),
            result: result.to_string(),
            is_error: false,
            call_id: "call-1".to_string(),
            duration_ms: 42,
        }
    }

    // ---- HookDecision helpers ----------------------------------------------

    #[test]
    fn test_decision_predicates() {
        assert!(HookDecision::Allow.is_allow());
        assert!(!HookDecision::Allow.is_deny());

        let deny = HookDecision::Deny {
            reason: "no".into(),
        };
        assert!(deny.is_deny());
        assert!(!deny.is_allow());

        let modify = HookDecision::Modify {
            new_arguments: json!({}),
        };
        assert!(modify.is_modify());

        assert!(HookDecision::Continue.is_continue());
    }

    #[test]
    fn test_decision_display() {
        assert_eq!(format!("{}", HookDecision::Allow), "Allow");
        assert_eq!(
            format!(
                "{}",
                HookDecision::Deny {
                    reason: "bad".into()
                }
            ),
            "Deny(bad)"
        );
    }

    // ---- HookEvent ---------------------------------------------------------

    #[test]
    fn test_event_tool_name() {
        let pre = pre_tool("shell", json!({}));
        assert_eq!(pre.tool_name(), Some("shell"));

        let post = post_tool("echo", "hi");
        assert_eq!(post.tool_name(), Some("echo"));

        let start = HookEvent::AgentStart {
            session_id: "s1".into(),
            input: "hello".into(),
        };
        assert_eq!(start.tool_name(), None);

        let llm = HookEvent::PreLlmCall {
            provider: "openai".into(),
            message_count: 5,
            turn: 0,
        };
        assert_eq!(llm.tool_name(), None);
    }

    // ---- Empty chain -------------------------------------------------------

    #[test]
    fn test_empty_chain_allows() {
        let chain = HookChain::new();
        assert!(chain.is_empty());
        let decision = chain.evaluate(&pre_tool("anything", json!({})));
        assert!(decision.is_allow());
    }

    // ---- Deny wins ---------------------------------------------------------

    #[test]
    fn test_deny_wins_over_continue() {
        let mut chain = HookChain::new();
        chain.add(hook_fn("pass", None, |_| HookDecision::Continue));
        chain.add(hook_fn("block", None, |_| HookDecision::Deny {
            reason: "blocked".into(),
        }));
        chain.add(hook_fn("allow", None, |_| HookDecision::Allow));

        let decision = chain.evaluate(&pre_tool("shell", json!({})));
        assert!(decision.is_deny());
        if let HookDecision::Deny { reason } = decision {
            assert_eq!(reason, "blocked");
        }
    }

    #[test]
    fn test_first_deny_short_circuits() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = std::sync::Arc::new(AtomicU32::new(0));

        let mut chain = HookChain::new();

        let c1 = counter.clone();
        chain.add(hook_fn("deny1", None, move |_| {
            c1.fetch_add(1, Ordering::SeqCst);
            HookDecision::Deny {
                reason: "first".into(),
            }
        }));

        let c2 = counter.clone();
        chain.add(hook_fn("deny2", None, move |_| {
            c2.fetch_add(1, Ordering::SeqCst);
            HookDecision::Deny {
                reason: "second".into(),
            }
        }));

        let decision = chain.evaluate(&pre_tool("x", json!({})));
        assert!(decision.is_deny());
        if let HookDecision::Deny { reason } = decision {
            assert_eq!(reason, "first");
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // ---- Allow stops evaluation --------------------------------------------

    #[test]
    fn test_allow_stops_evaluation() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = std::sync::Arc::new(AtomicU32::new(0));

        let mut chain = HookChain::new();

        let c1 = counter.clone();
        chain.add(hook_fn("allow", None, move |_| {
            c1.fetch_add(1, Ordering::SeqCst);
            HookDecision::Allow
        }));

        let c2 = counter.clone();
        chain.add(hook_fn("never", None, move |_| {
            c2.fetch_add(1, Ordering::SeqCst);
            HookDecision::Deny {
                reason: "should not reach".into(),
            }
        }));

        let decision = chain.evaluate(&pre_tool("x", json!({})));
        assert!(decision.is_allow());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // ---- Modify wins -------------------------------------------------------

    #[test]
    fn test_modify_wins() {
        let mut chain = HookChain::new();
        chain.add(hook_fn("pass", None, |_| HookDecision::Continue));
        chain.add(hook_fn("modify", None, |_| HookDecision::Modify {
            new_arguments: json!({"injected": true}),
        }));
        chain.add(hook_fn("deny", None, |_| HookDecision::Deny {
            reason: "too late".into(),
        }));

        let decision = chain.evaluate(&pre_tool("tool", json!({})));
        assert!(decision.is_modify());
        if let HookDecision::Modify { new_arguments } = decision {
            assert_eq!(new_arguments, json!({"injected": true}));
        }
    }

    // ---- Matcher filtering -------------------------------------------------

    #[test]
    fn test_matcher_filters_by_tool_name() {
        let mut chain = HookChain::new();
        chain.add(hook_fn("shell-only", Some("^shell$".to_string()), |_| {
            HookDecision::Deny {
                reason: "no shell".into(),
            }
        }));

        // Shell is denied
        let d = chain.evaluate(&pre_tool("shell", json!({})));
        assert!(d.is_deny());

        // Other tools are allowed
        let d = chain.evaluate(&pre_tool("echo", json!({})));
        assert!(d.is_allow());

        // file_read is allowed
        let d = chain.evaluate(&pre_tool("file_read", json!({})));
        assert!(d.is_allow());
    }

    #[test]
    fn test_matcher_regex_pattern() {
        let mut chain = HookChain::new();
        chain.add(hook_fn(
            "block-file-ops",
            Some("^file_".to_string()),
            |_| HookDecision::Deny {
                reason: "no file ops".into(),
            },
        ));

        assert!(chain.evaluate(&pre_tool("file_read", json!({}))).is_deny());
        assert!(chain.evaluate(&pre_tool("file_write", json!({}))).is_deny());
        assert!(chain.evaluate(&pre_tool("shell", json!({}))).is_allow());
    }

    #[test]
    fn test_no_matcher_matches_all() {
        let mut chain = HookChain::new();
        chain.add(hook_fn("all", None, |_| HookDecision::Deny {
            reason: "all blocked".into(),
        }));

        assert!(chain.evaluate(&pre_tool("shell", json!({}))).is_deny());
        assert!(chain.evaluate(&pre_tool("echo", json!({}))).is_deny());
        assert!(chain.evaluate(&pre_tool("file_read", json!({}))).is_deny());
    }

    // ---- Non-tool events bypass matcher ------------------------------------

    #[test]
    fn test_non_tool_events_bypass_matcher() {
        let mut chain = HookChain::new();
        chain.add(hook_fn(
            "tool-specific",
            Some("^shell$".to_string()),
            |_| HookDecision::Deny {
                reason: "blocked".into(),
            },
        ));

        // Non-tool events always go through (matcher only filters tool events)
        let event = HookEvent::PreLlmCall {
            provider: "openai".into(),
            message_count: 3,
            turn: 0,
        };
        let d = chain.evaluate(&event);
        assert!(d.is_deny());
    }

    // ---- Chain ordering ----------------------------------------------------

    #[test]
    fn test_chain_len_and_empty() {
        let mut chain = HookChain::new();
        assert_eq!(chain.len(), 0);
        assert!(chain.is_empty());

        chain.add(hook_fn("a", None, |_| HookDecision::Continue));
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());

        chain.add(hook_fn("b", None, |_| HookDecision::Continue));
        assert_eq!(chain.len(), 2);
    }

    // ---- All continue => Allow ---------------------------------------------

    #[test]
    fn test_all_continue_means_allow() {
        let mut chain = HookChain::new();
        chain.add(hook_fn("a", None, |_| HookDecision::Continue));
        chain.add(hook_fn("b", None, |_| HookDecision::Continue));
        chain.add(hook_fn("c", None, |_| HookDecision::Continue));

        let d = chain.evaluate(&pre_tool("x", json!({})));
        assert!(d.is_allow());
    }

    // ---- hook_fn convenience -----------------------------------------------

    #[test]
    fn test_hook_fn_name() {
        let h = hook_fn("my-hook", None, |_| HookDecision::Continue);
        assert_eq!(h.name(), "my-hook");
    }

    #[test]
    fn test_hook_fn_with_invalid_regex_fallback() {
        // Invalid regex should not panic — falls back to match-nothing
        let h = hook_fn("bad-regex", Some("[invalid".to_string()), |_| {
            HookDecision::Deny {
                reason: "blocked".into(),
            }
        });

        // Tool event should NOT match since the regex is invalid
        assert!(!h.matches("shell"));
    }

    // ---- Post-tool events are informational --------------------------------

    #[test]
    fn test_post_tool_event_can_deny() {
        // Post events can still return Deny (caller decides whether to honor)
        let mut chain = HookChain::new();
        chain.add(hook_fn("audit", None, |event| {
            if let HookEvent::PostToolUse { duration_ms, .. } = event {
                if *duration_ms > 30 {
                    return HookDecision::Deny {
                        reason: "too slow".into(),
                    };
                }
            }
            HookDecision::Continue
        }));

        let d = chain.evaluate(&post_tool("shell", "output"));
        assert!(d.is_deny());
    }

    // ---- Custom Hook trait impl --------------------------------------------

    struct CountingHook {
        name: String,
        count: std::sync::atomic::AtomicU32,
    }

    impl Hook for CountingHook {
        fn name(&self) -> &str {
            &self.name
        }

        fn on_event(&self, _event: &HookEvent) -> HookDecision {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HookDecision::Continue
        }
    }

    #[test]
    fn test_custom_hook_trait_impl() {
        let hook = CountingHook {
            name: "counter".to_string(),
            count: std::sync::atomic::AtomicU32::new(0),
        };
        let count_ref = &hook.count;

        // We need to move it into Box, so grab the Arc pointer first
        let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count2 = count.clone();

        let mut chain = HookChain::new();
        chain.add(hook_fn("counter", None, move |_| {
            count2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HookDecision::Continue
        }));

        chain.evaluate(&pre_tool("a", json!({})));
        chain.evaluate(&pre_tool("b", json!({})));
        chain.evaluate(&pre_tool("c", json!({})));

        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}

//! Agent Handoff Protocol for sequential control transfer between agents.
//!
//! Inspired by the OpenAI Agents SDK handoff pattern. Agents pass control
//! sequentially with context transfer, chain tracking, and circular handoff
//! prevention.
//!
//! # Main types
//!
//! - [`HandoffProtocol`] — Manages the lifecycle of agent handoffs.
//! - [`HandoffConfig`] — Configuration for handoff behavior.
//! - [`HandoffRequest`] — A request to transfer control to another agent.
//! - [`HandoffResult`] — The result of a completed handoff.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration controlling handoff behavior and limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffConfig {
    /// Maximum depth of handoff chains before refusing further handoffs (default: 5).
    pub max_handoff_depth: u32,
    /// How much context is transferred between agents.
    pub context_transfer: ContextTransferMode,
    /// Whether an agent can hand control back to a previous agent in the chain (default: true).
    pub allow_handback: bool,
    /// Per-handoff timeout in seconds (serialized as u64 for JSON compat; default: 60s).
    #[serde(with = "duration_secs")]
    pub timeout: Duration,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            max_handoff_depth: 5,
            context_transfer: ContextTransferMode::Summary,
            allow_handback: true,
            timeout: Duration::from_secs(60),
        }
    }
}

/// How much conversation context is transferred during a handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTransferMode {
    /// Pass the entire conversation history.
    Full,
    /// Pass a summary of the conversation.
    Summary,
    /// Pass only messages deemed relevant to the target task.
    Selective,
    /// Pass only the task description — no conversation history.
    Minimal,
}

// ---------------------------------------------------------------------------
// Request / Context
// ---------------------------------------------------------------------------

/// A request to hand off control from one agent to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRequest {
    /// Source agent role/name initiating the handoff.
    pub from_agent: String,
    /// Target agent role/name receiving control.
    pub to_agent: String,
    /// Reason the handoff is happening.
    pub reason: String,
    /// Description of what the target agent should accomplish.
    pub task: String,
    /// Context transferred to the target agent.
    pub context: HandoffContext,
    /// Arbitrary metadata attached to the request.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Context bundle transferred during a handoff.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HandoffContext {
    /// Conversation messages to transfer.
    pub messages: Vec<ContextMessage>,
    /// Summaries of tool results accumulated so far.
    pub tool_results: Vec<ToolResultSummary>,
    /// Total tokens consumed up to this point.
    pub accumulated_tokens: usize,
    /// Chain of agent names that led to this handoff (oldest first).
    pub parent_chain: Vec<String>,
}

/// A single message in the transferred context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    /// Role of the sender (e.g. "user", "assistant", "system").
    pub role: String,
    /// Message content.
    pub content: String,
    /// When the message was sent.
    pub timestamp: DateTime<Utc>,
}

/// Summary of a tool invocation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultSummary {
    /// Name of the tool that was called.
    pub tool_name: String,
    /// Whether the call succeeded.
    pub success: bool,
    /// Human-readable summary of the result.
    pub summary: String,
}

// ---------------------------------------------------------------------------
// Result / Status
// ---------------------------------------------------------------------------

/// Outcome of a completed handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffResult {
    /// Agent that initiated the handoff.
    pub from_agent: String,
    /// Agent that received control.
    pub to_agent: String,
    /// The response produced by the target agent.
    pub response: String,
    /// Tokens consumed during this handoff.
    pub tokens_used: usize,
    /// Wall-clock duration of the handoff.
    #[serde(with = "duration_secs")]
    pub duration: Duration,
    /// Full chain of agents involved (oldest first).
    pub handoff_chain: Vec<String>,
    /// Terminal status of the handoff.
    pub status: HandoffStatus,
}

/// Terminal status of a handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum HandoffStatus {
    /// The target agent completed its task.
    Completed,
    /// The target agent handed control back with a reason.
    HandedBack {
        /// Human-readable reason the agent handed back control.
        reason: String,
    },
    /// The handoff exceeded its timeout.
    TimedOut,
    /// The handoff chain exceeded the maximum depth.
    DepthExceeded,
    /// The handoff failed for an arbitrary reason.
    Failed {
        /// Human-readable reason the handoff failed.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

/// A historical record of a handoff (request + optional result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRecord {
    /// The original request.
    pub request: HandoffRequest,
    /// The result, if the handoff has completed.
    pub result: Option<HandoffResult>,
    /// When this record was created.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// HandoffProtocol
// ---------------------------------------------------------------------------

/// Manages the lifecycle of agent-to-agent handoffs.
///
/// Tracks an ordered history of handoffs and enforces depth limits,
/// circular-handoff detection, and timeout semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffProtocol {
    config: HandoffConfig,
    history: Vec<HandoffRecord>,
}

impl HandoffProtocol {
    /// Create a new protocol instance with the given configuration.
    pub fn new(config: HandoffConfig) -> Self {
        Self {
            config,
            history: Vec::new(),
        }
    }

    /// Create a protocol with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(HandoffConfig::default())
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &HandoffConfig {
        &self.config
    }

    /// Get the full handoff history.
    pub fn history(&self) -> &[HandoffRecord] {
        &self.history
    }

    /// Return the current chain depth (number of handoffs recorded so far).
    pub fn current_depth(&self) -> u32 {
        self.history.len() as u32
    }

    // ----- Core operations --------------------------------------------------

    /// Initiate a handoff from one agent to another.
    ///
    /// Validates depth limits and circular-handoff constraints before
    /// recording the request.
    ///
    /// Returns the index of the new record in the history.
    pub fn initiate_handoff(&mut self, request: HandoffRequest) -> Result<usize, HandoffError> {
        // Depth check
        if self.current_depth() >= self.config.max_handoff_depth {
            return Err(HandoffError::DepthExceeded {
                max: self.config.max_handoff_depth,
                current: self.current_depth(),
            });
        }

        // Circular handoff check — prevent A -> B -> A unless allow_handback is true
        if !self.config.allow_handback && self.is_circular(&request.to_agent) {
            return Err(HandoffError::CircularHandoff {
                agent: request.to_agent.clone(),
                chain: self.current_chain(),
            });
        }

        // Even with allow_handback, block direct self-handoffs
        if request.from_agent == request.to_agent {
            return Err(HandoffError::SelfHandoff {
                agent: request.from_agent.clone(),
            });
        }

        let record = HandoffRecord {
            request,
            result: None,
            timestamp: Utc::now(),
        };

        self.history.push(record);
        Ok(self.history.len() - 1)
    }

    /// Accept a pending handoff (mark it as in-progress).
    ///
    /// This is a validation step confirming the target agent acknowledges
    /// the handoff. Returns the request context for the target to process.
    pub fn accept_handoff(&self, record_index: usize) -> Result<&HandoffRequest, HandoffError> {
        let record = self
            .history
            .get(record_index)
            .ok_or(HandoffError::RecordNotFound {
                index: record_index,
            })?;

        if record.result.is_some() {
            return Err(HandoffError::AlreadyCompleted {
                index: record_index,
            });
        }

        Ok(&record.request)
    }

    /// Complete a handoff with the target agent's response.
    pub fn complete_handoff(
        &mut self,
        record_index: usize,
        response: String,
        tokens_used: usize,
        duration: Duration,
    ) -> Result<&HandoffResult, HandoffError> {
        let chain = self.current_chain();

        let record = self
            .history
            .get_mut(record_index)
            .ok_or(HandoffError::RecordNotFound {
                index: record_index,
            })?;

        if record.result.is_some() {
            return Err(HandoffError::AlreadyCompleted {
                index: record_index,
            });
        }

        let result = HandoffResult {
            from_agent: record.request.from_agent.clone(),
            to_agent: record.request.to_agent.clone(),
            response,
            tokens_used,
            duration,
            handoff_chain: chain,
            status: HandoffStatus::Completed,
        };

        record.result = Some(result);
        // Safety: we just assigned `Some(result)` above
        #[allow(clippy::expect_used)]
        Ok(record.result.as_ref().expect("just inserted"))
    }

    /// Hand control back from the target agent to the source agent with a reason.
    pub fn handback(
        &mut self,
        record_index: usize,
        reason: String,
        tokens_used: usize,
        duration: Duration,
    ) -> Result<&HandoffResult, HandoffError> {
        if !self.config.allow_handback {
            return Err(HandoffError::HandbackNotAllowed);
        }

        let chain = self.current_chain();

        let record = self
            .history
            .get_mut(record_index)
            .ok_or(HandoffError::RecordNotFound {
                index: record_index,
            })?;

        if record.result.is_some() {
            return Err(HandoffError::AlreadyCompleted {
                index: record_index,
            });
        }

        let result = HandoffResult {
            from_agent: record.request.from_agent.clone(),
            to_agent: record.request.to_agent.clone(),
            response: String::new(),
            tokens_used,
            duration,
            handoff_chain: chain,
            status: HandoffStatus::HandedBack {
                reason: reason.clone(),
            },
        };

        record.result = Some(result);
        // Safety: we just assigned `Some(result)` above
        #[allow(clippy::expect_used)]
        Ok(record.result.as_ref().expect("just inserted"))
    }

    /// Mark a handoff as timed out.
    pub fn mark_timeout(
        &mut self,
        record_index: usize,
        duration: Duration,
    ) -> Result<(), HandoffError> {
        let chain = self.current_chain();

        let record = self
            .history
            .get_mut(record_index)
            .ok_or(HandoffError::RecordNotFound {
                index: record_index,
            })?;

        if record.result.is_some() {
            return Err(HandoffError::AlreadyCompleted {
                index: record_index,
            });
        }

        record.result = Some(HandoffResult {
            from_agent: record.request.from_agent.clone(),
            to_agent: record.request.to_agent.clone(),
            response: String::new(),
            tokens_used: 0,
            duration,
            handoff_chain: chain,
            status: HandoffStatus::TimedOut,
        });

        Ok(())
    }

    /// Mark a handoff as failed with a reason.
    pub fn mark_failed(
        &mut self,
        record_index: usize,
        reason: String,
        duration: Duration,
    ) -> Result<(), HandoffError> {
        let chain = self.current_chain();

        let record = self
            .history
            .get_mut(record_index)
            .ok_or(HandoffError::RecordNotFound {
                index: record_index,
            })?;

        if record.result.is_some() {
            return Err(HandoffError::AlreadyCompleted {
                index: record_index,
            });
        }

        record.result = Some(HandoffResult {
            from_agent: record.request.from_agent.clone(),
            to_agent: record.request.to_agent.clone(),
            response: String::new(),
            tokens_used: 0,
            duration,
            handoff_chain: chain,
            status: HandoffStatus::Failed {
                reason: reason.clone(),
            },
        });

        Ok(())
    }

    // ----- Query helpers ----------------------------------------------------

    /// Build the current handoff chain (list of agent names in order).
    pub fn current_chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        for record in &self.history {
            if chain
                .last()
                .map_or(true, |last: &String| *last != record.request.from_agent)
            {
                chain.push(record.request.from_agent.clone());
            }
            chain.push(record.request.to_agent.clone());
        }
        chain.dedup();
        chain
    }

    /// Check whether an agent already appears in the handoff chain.
    pub fn is_circular(&self, agent: &str) -> bool {
        self.history
            .iter()
            .any(|r| r.request.from_agent == agent || r.request.to_agent == agent)
    }

    /// Return the last completed handoff result, if any.
    pub fn last_result(&self) -> Option<&HandoffResult> {
        self.history.iter().rev().find_map(|r| r.result.as_ref())
    }

    /// Count how many handoffs have been completed (regardless of status).
    pub fn completed_count(&self) -> usize {
        self.history.iter().filter(|r| r.result.is_some()).count()
    }

    /// Count how many handoffs are still pending (no result yet).
    pub fn pending_count(&self) -> usize {
        self.history.iter().filter(|r| r.result.is_none()).count()
    }

    /// Total tokens consumed across all completed handoffs.
    pub fn total_tokens(&self) -> usize {
        self.history
            .iter()
            .filter_map(|r| r.result.as_ref())
            .map(|r| r.tokens_used)
            .sum()
    }

    /// Clear all history.
    pub fn reset(&mut self) {
        self.history.clear();
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during handoff operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandoffError {
    /// The handoff chain has exceeded the maximum allowed depth.
    DepthExceeded {
        /// Maximum allowed depth.
        max: u32,
        /// Depth at which the error was triggered.
        current: u32,
    },
    /// A circular handoff was detected and `allow_handback` is false.
    CircularHandoff {
        /// Agent that caused the cycle.
        agent: String,
        /// Current handoff chain at the time of detection.
        chain: Vec<String>,
    },
    /// An agent attempted to hand off to itself.
    SelfHandoff {
        /// Name of the agent that attempted self-handoff.
        agent: String,
    },
    /// The specified record index does not exist.
    RecordNotFound {
        /// The index that was requested.
        index: usize,
    },
    /// The handoff has already been completed.
    AlreadyCompleted {
        /// The index of the already-completed record.
        index: usize,
    },
    /// Handback is not allowed by configuration.
    HandbackNotAllowed,
}

impl std::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DepthExceeded { max, current } => {
                write!(f, "Handoff depth exceeded: current {current}, max {max}")
            }
            Self::CircularHandoff { agent, chain } => {
                write!(
                    f,
                    "Circular handoff to '{agent}' detected in chain: {chain:?}"
                )
            }
            Self::SelfHandoff { agent } => {
                write!(f, "Agent '{agent}' cannot hand off to itself")
            }
            Self::RecordNotFound { index } => {
                write!(f, "Handoff record not found at index {index}")
            }
            Self::AlreadyCompleted { index } => {
                write!(f, "Handoff at index {index} is already completed")
            }
            Self::HandbackNotAllowed => {
                write!(f, "Handback is not allowed by configuration")
            }
        }
    }
}

impl std::error::Error for HandoffError {}

// ---------------------------------------------------------------------------
// Duration serde helper (as seconds)
// ---------------------------------------------------------------------------

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -- helpers --

    fn make_request(from: &str, to: &str) -> HandoffRequest {
        HandoffRequest {
            from_agent: from.to_string(),
            to_agent: to.to_string(),
            reason: format!("{from} needs {to}"),
            task: format!("Task for {to}"),
            context: HandoffContext::default(),
            metadata: HashMap::new(),
        }
    }

    fn make_request_with_context(from: &str, to: &str, messages: usize) -> HandoffRequest {
        let msgs: Vec<ContextMessage> = (0..messages)
            .map(|i| ContextMessage {
                role: "user".to_string(),
                content: format!("message {i}"),
                timestamp: Utc::now(),
            })
            .collect();
        HandoffRequest {
            from_agent: from.to_string(),
            to_agent: to.to_string(),
            reason: "needs help".to_string(),
            task: "do something".to_string(),
            context: HandoffContext {
                messages: msgs,
                tool_results: vec![],
                accumulated_tokens: 500,
                parent_chain: vec![from.to_string()],
            },
            metadata: HashMap::new(),
        }
    }

    // 1. Default config values
    #[test]
    fn test_default_config() {
        let cfg = HandoffConfig::default();
        assert_eq!(cfg.max_handoff_depth, 5);
        assert_eq!(cfg.context_transfer, ContextTransferMode::Summary);
        assert!(cfg.allow_handback);
        assert_eq!(cfg.timeout, Duration::from_secs(60));
    }

    // 2. Create protocol with defaults
    #[test]
    fn test_with_defaults() {
        let proto = HandoffProtocol::with_defaults();
        assert_eq!(proto.current_depth(), 0);
        assert!(proto.history().is_empty());
    }

    // 3. Initiate a single handoff
    #[test]
    fn test_initiate_handoff() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(proto.current_depth(), 1);
        assert_eq!(proto.pending_count(), 1);
    }

    // 4. Accept a handoff
    #[test]
    fn test_accept_handoff() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        let req = proto.accept_handoff(idx).unwrap();
        assert_eq!(req.from_agent, "A");
        assert_eq!(req.to_agent, "B");
    }

    // 5. Complete a handoff
    #[test]
    fn test_complete_handoff() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        let result = proto
            .complete_handoff(idx, "done".to_string(), 100, Duration::from_secs(5))
            .unwrap();
        assert_eq!(result.status, HandoffStatus::Completed);
        assert_eq!(result.tokens_used, 100);
        assert_eq!(result.response, "done");
        assert_eq!(proto.completed_count(), 1);
        assert_eq!(proto.pending_count(), 0);
    }

    // 6. Handback
    #[test]
    fn test_handback() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        let result = proto
            .handback(
                idx,
                "need more info".to_string(),
                50,
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(
            result.status,
            HandoffStatus::HandedBack {
                reason: "need more info".to_string()
            }
        );
    }

    // 7. Handback not allowed
    #[test]
    fn test_handback_not_allowed() {
        let cfg = HandoffConfig {
            allow_handback: false,
            ..Default::default()
        };
        let mut proto = HandoffProtocol::new(cfg);
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        let err = proto
            .handback(idx, "reason".to_string(), 0, Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(err, HandoffError::HandbackNotAllowed);
    }

    // 8. Depth exceeded
    #[test]
    fn test_depth_exceeded() {
        let cfg = HandoffConfig {
            max_handoff_depth: 2,
            ..Default::default()
        };
        let mut proto = HandoffProtocol::new(cfg);
        proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto.initiate_handoff(make_request("B", "C")).unwrap();
        let err = proto.initiate_handoff(make_request("C", "D")).unwrap_err();
        assert!(matches!(err, HandoffError::DepthExceeded { max: 2, .. }));
    }

    // 9. Self-handoff rejected
    #[test]
    fn test_self_handoff_rejected() {
        let mut proto = HandoffProtocol::with_defaults();
        let err = proto.initiate_handoff(make_request("A", "A")).unwrap_err();
        assert!(matches!(err, HandoffError::SelfHandoff { .. }));
    }

    // 10. Circular handoff detected when handback disabled
    #[test]
    fn test_circular_handoff_detected() {
        let cfg = HandoffConfig {
            allow_handback: false,
            ..Default::default()
        };
        let mut proto = HandoffProtocol::new(cfg);
        proto.initiate_handoff(make_request("A", "B")).unwrap();
        let err = proto.initiate_handoff(make_request("B", "A")).unwrap_err();
        assert!(matches!(err, HandoffError::CircularHandoff { .. }));
    }

    // 11. Circular handoff allowed when handback enabled
    #[test]
    fn test_circular_allowed_with_handback() {
        let mut proto = HandoffProtocol::with_defaults(); // allow_handback = true
        proto.initiate_handoff(make_request("A", "B")).unwrap();
        // B -> A is allowed because handback is enabled
        let result = proto.initiate_handoff(make_request("B", "A"));
        assert!(result.is_ok());
    }

    // 12. Record not found
    #[test]
    fn test_record_not_found() {
        let proto = HandoffProtocol::with_defaults();
        let err = proto.accept_handoff(99).unwrap_err();
        assert!(matches!(err, HandoffError::RecordNotFound { index: 99 }));
    }

    // 13. Already completed — complete
    #[test]
    fn test_already_completed_on_complete() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx, "ok".to_string(), 10, Duration::from_secs(1))
            .unwrap();
        let err = proto
            .complete_handoff(idx, "again".to_string(), 10, Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, HandoffError::AlreadyCompleted { .. }));
    }

    // 14. Already completed — accept
    #[test]
    fn test_already_completed_on_accept() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx, "ok".to_string(), 10, Duration::from_secs(1))
            .unwrap();
        let err = proto.accept_handoff(idx).unwrap_err();
        assert!(matches!(err, HandoffError::AlreadyCompleted { .. }));
    }

    // 15. Mark timeout
    #[test]
    fn test_mark_timeout() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto.mark_timeout(idx, Duration::from_secs(60)).unwrap();
        let result = proto.history()[idx].result.as_ref().unwrap();
        assert_eq!(result.status, HandoffStatus::TimedOut);
    }

    // 16. Mark failed
    #[test]
    fn test_mark_failed() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .mark_failed(idx, "crash".to_string(), Duration::from_secs(3))
            .unwrap();
        let result = proto.history()[idx].result.as_ref().unwrap();
        assert_eq!(
            result.status,
            HandoffStatus::Failed {
                reason: "crash".to_string()
            }
        );
    }

    // 17. Chain tracking across multiple handoffs
    #[test]
    fn test_chain_tracking() {
        let mut proto = HandoffProtocol::with_defaults();
        proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto.initiate_handoff(make_request("B", "C")).unwrap();
        proto.initiate_handoff(make_request("C", "D")).unwrap();
        let chain = proto.current_chain();
        assert_eq!(chain, vec!["A", "B", "C", "D"]);
    }

    // 18. is_circular detects presence in chain
    #[test]
    fn test_is_circular() {
        let mut proto = HandoffProtocol::with_defaults();
        proto.initiate_handoff(make_request("A", "B")).unwrap();
        assert!(proto.is_circular("A"));
        assert!(proto.is_circular("B"));
        assert!(!proto.is_circular("C"));
    }

    // 19. Total tokens across completed handoffs
    #[test]
    fn test_total_tokens() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx0 = proto.initiate_handoff(make_request("A", "B")).unwrap();
        let idx1 = proto.initiate_handoff(make_request("B", "C")).unwrap();
        proto
            .complete_handoff(idx0, "r1".to_string(), 100, Duration::from_secs(1))
            .unwrap();
        proto
            .complete_handoff(idx1, "r2".to_string(), 200, Duration::from_secs(2))
            .unwrap();
        assert_eq!(proto.total_tokens(), 300);
    }

    // 20. Last result
    #[test]
    fn test_last_result() {
        let mut proto = HandoffProtocol::with_defaults();
        assert!(proto.last_result().is_none());
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx, "final".to_string(), 50, Duration::from_secs(1))
            .unwrap();
        let last = proto.last_result().unwrap();
        assert_eq!(last.response, "final");
    }

    // 21. Reset clears history
    #[test]
    fn test_reset() {
        let mut proto = HandoffProtocol::with_defaults();
        proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto.reset();
        assert_eq!(proto.current_depth(), 0);
        assert!(proto.history().is_empty());
    }

    // 22. Config serialization roundtrip
    #[test]
    fn test_config_serialization() {
        let cfg = HandoffConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: HandoffConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_handoff_depth, 5);
        assert!(restored.allow_handback);
    }

    // 23. HandoffRequest serialization roundtrip
    #[test]
    fn test_request_serialization() {
        let req = make_request_with_context("A", "B", 3);
        let json = serde_json::to_string(&req).unwrap();
        let restored: HandoffRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.from_agent, "A");
        assert_eq!(restored.context.messages.len(), 3);
        assert_eq!(restored.context.accumulated_tokens, 500);
    }

    // 24. HandoffResult serialization roundtrip
    #[test]
    fn test_result_serialization() {
        let result = HandoffResult {
            from_agent: "A".to_string(),
            to_agent: "B".to_string(),
            response: "done".to_string(),
            tokens_used: 42,
            duration: Duration::from_secs(10),
            handoff_chain: vec!["A".to_string(), "B".to_string()],
            status: HandoffStatus::Completed,
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: HandoffResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.tokens_used, 42);
        assert_eq!(restored.status, HandoffStatus::Completed);
    }

    // 25. HandoffStatus variants serialize correctly
    #[test]
    fn test_status_serialization_variants() {
        let statuses = vec![
            HandoffStatus::Completed,
            HandoffStatus::HandedBack {
                reason: "oops".to_string(),
            },
            HandoffStatus::TimedOut,
            HandoffStatus::DepthExceeded,
            HandoffStatus::Failed {
                reason: "boom".to_string(),
            },
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let restored: HandoffStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, status);
        }
    }

    // 26. Context message fields preserved
    #[test]
    fn test_context_message_fields() {
        let msg = ContextMessage {
            role: "assistant".to_string(),
            content: "Hello there".to_string(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let restored: ContextMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "assistant");
        assert_eq!(restored.content, "Hello there");
    }

    // 27. Tool result summary preserved
    #[test]
    fn test_tool_result_summary() {
        let summary = ToolResultSummary {
            tool_name: "file_read".to_string(),
            success: true,
            summary: "Read 42 lines".to_string(),
        };
        let json = serde_json::to_string(&summary).unwrap();
        let restored: ToolResultSummary = serde_json::from_str(&json).unwrap();
        assert!(restored.success);
        assert_eq!(restored.tool_name, "file_read");
    }

    // 28. Handoff with metadata
    #[test]
    fn test_handoff_with_metadata() {
        let mut req = make_request("A", "B");
        req.metadata
            .insert("priority".to_string(), serde_json::json!("high"));
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(req).unwrap();
        let stored = proto.accept_handoff(idx).unwrap();
        assert_eq!(stored.metadata["priority"], "high");
    }

    // 29. Error display messages
    #[test]
    fn test_error_display() {
        let errors = vec![
            HandoffError::DepthExceeded { max: 5, current: 5 },
            HandoffError::CircularHandoff {
                agent: "B".to_string(),
                chain: vec!["A".to_string(), "B".to_string()],
            },
            HandoffError::SelfHandoff {
                agent: "A".to_string(),
            },
            HandoffError::RecordNotFound { index: 42 },
            HandoffError::AlreadyCompleted { index: 0 },
            HandoffError::HandbackNotAllowed,
        ];
        for err in errors {
            let display = format!("{err}");
            assert!(!display.is_empty());
        }
    }

    // 30. Multiple sequential handoffs complete correctly
    #[test]
    fn test_sequential_handoffs() {
        let mut proto = HandoffProtocol::with_defaults();

        let idx0 = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx0, "B done".to_string(), 100, Duration::from_secs(5))
            .unwrap();

        let idx1 = proto.initiate_handoff(make_request("B", "C")).unwrap();
        proto
            .complete_handoff(idx1, "C done".to_string(), 200, Duration::from_secs(3))
            .unwrap();

        let idx2 = proto.initiate_handoff(make_request("C", "D")).unwrap();
        proto
            .complete_handoff(idx2, "D done".to_string(), 150, Duration::from_secs(4))
            .unwrap();

        assert_eq!(proto.completed_count(), 3);
        assert_eq!(proto.total_tokens(), 450);
        assert_eq!(proto.last_result().unwrap().response, "D done");
    }

    // 31. Protocol full serialization roundtrip
    #[test]
    fn test_protocol_serialization() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx, "ok".to_string(), 10, Duration::from_secs(1))
            .unwrap();

        let json = serde_json::to_string(&proto).unwrap();
        let restored: HandoffProtocol = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.history().len(), 1);
        assert_eq!(restored.completed_count(), 1);
    }

    // 32. Mark timeout on already completed fails
    #[test]
    fn test_timeout_on_completed_fails() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx, "ok".to_string(), 10, Duration::from_secs(1))
            .unwrap();
        let err = proto
            .mark_timeout(idx, Duration::from_secs(60))
            .unwrap_err();
        assert!(matches!(err, HandoffError::AlreadyCompleted { .. }));
    }

    // 33. Mark failed on already completed fails
    #[test]
    fn test_failed_on_completed_fails() {
        let mut proto = HandoffProtocol::with_defaults();
        let idx = proto.initiate_handoff(make_request("A", "B")).unwrap();
        proto
            .complete_handoff(idx, "ok".to_string(), 10, Duration::from_secs(1))
            .unwrap();
        let err = proto
            .mark_failed(idx, "crash".to_string(), Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, HandoffError::AlreadyCompleted { .. }));
    }
}

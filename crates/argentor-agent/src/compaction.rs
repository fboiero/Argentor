//! Automatic context compaction for managing conversation token budgets.
//!
//! Inspired by Mastra's auto-compaction at 30K tokens. When the conversation
//! history approaches the token limit, this module summarizes older messages
//! while preserving recent context, system messages, and messages with tool calls.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌───────────────────┐     ┌───────────────────┐
//! │  Message History │ --> │ ContextCompactor  │ --> │ CompactionResult  │
//! │  (many messages) │     │  (strategy-based) │     │ (fewer messages)  │
//! └─────────────────┘     └───────────────────┘     └───────────────────┘
//!                                │
//!                     ┌──────────┴─────────────┐
//!                     │  Strategies:           │
//!                     │  - Summarize           │
//!                     │  - SlidingWindow       │
//!                     │  - ImportanceBased     │
//!                     │  - Hybrid              │
//!                     └────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Strategy for compacting conversation history.
///
/// Each strategy makes different tradeoffs between information preservation
/// and compression ratio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionStrategy {
    /// Create a single summary message replacing older messages.
    Summarize,
    /// Keep only the most recent messages (sliding window).
    SlidingWindow,
    /// Keep messages scored as "important" (tool calls, long responses, decisions).
    ImportanceBased,
    /// Hybrid: summarize old + keep recent + preserve important.
    Hybrid,
}

/// A message in the compaction pipeline.
///
/// Simplified message representation for compaction. Uses role/content pairs
/// with metadata flags for importance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactableMessage {
    /// The role of the message author (e.g., "user", "assistant", "system", "tool").
    pub role: String,
    /// The textual content of the message.
    pub content: String,
    /// Whether this message contains or references a tool call.
    pub has_tool_call: bool,
    /// Estimated token count for this message.
    pub token_estimate: usize,
}

impl CompactableMessage {
    /// Create a new compactable message.
    pub fn new(role: &str, content: &str, has_tool_call: bool) -> Self {
        let token_estimate = estimate_tokens(content);
        Self {
            role: role.to_string(),
            content: content.to_string(),
            has_tool_call,
            token_estimate,
        }
    }

    /// Create a system message.
    pub fn system(content: &str) -> Self {
        Self::new("system", content, false)
    }

    /// Create a user message.
    pub fn user(content: &str) -> Self {
        Self::new("user", content, false)
    }

    /// Create an assistant message.
    pub fn assistant(content: &str) -> Self {
        Self::new("assistant", content, false)
    }

    /// Create a tool result message.
    pub fn tool(content: &str) -> Self {
        Self::new("tool", content, true)
    }

    /// Return `true` if this is a system message.
    pub fn is_system(&self) -> bool {
        self.role == "system"
    }

    /// Return `true` if this is a user message.
    pub fn is_user(&self) -> bool {
        self.role == "user"
    }

    /// Return `true` if this is an assistant message.
    pub fn is_assistant(&self) -> bool {
        self.role == "assistant"
    }
}

/// A message in the compacted output.
///
/// May represent a single original message or a summary of multiple messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactedMessage {
    /// The role of this message.
    pub role: String,
    /// The content (original or summary).
    pub content: String,
    /// Whether this is a synthesized summary.
    pub is_summary: bool,
    /// How many original messages this represents.
    pub original_count: usize,
}

/// The result of a compaction operation.
///
/// Contains the compacted messages, compression statistics, and an optional
/// summary of the content that was compressed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    /// Number of messages before compaction.
    pub original_message_count: usize,
    /// Number of messages after compaction.
    pub compacted_message_count: usize,
    /// Estimated tokens before compaction.
    pub original_tokens: usize,
    /// Estimated tokens after compaction.
    pub compacted_tokens: usize,
    /// Compression ratio (compacted / original). Lower is more compressed.
    pub compression_ratio: f32,
    /// Summary of the compacted content, if generated.
    pub summary: Option<String>,
    /// The compacted messages.
    pub preserved_messages: Vec<CompactedMessage>,
}

/// Configuration for the context compactor.
///
/// Controls when compaction triggers, how aggressively to compress, and
/// which messages to preserve.
///
/// ## Adaptive trigger
///
/// When `adaptive_trigger_pct` is `Some(pct)` the compactor ignores the
/// static `trigger_threshold` and instead fires when the conversation
/// reaches `pct`% of `max_context_tokens`.  For example, with
/// `max_context_tokens = 200_000` and `adaptive_trigger_pct = Some(70)`,
/// compaction fires at 140 000 tokens — earlier for small-context models,
/// later for large-context ones.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Whether automatic compaction is enabled.
    pub enabled: bool,
    /// Static token threshold that triggers compaction (default: 30 000).
    ///
    /// Ignored when `adaptive_trigger_pct` is set.
    pub trigger_threshold: usize,
    /// Adaptive trigger: fire compaction at this percentage of the model's
    /// context window.  Range: 1–99.  Default: `Some(70)`.
    ///
    /// When `None`, the static `trigger_threshold` is used instead.
    pub adaptive_trigger_pct: Option<u8>,
    /// The model's total context window in tokens.  Used only when
    /// `adaptive_trigger_pct` is set (default: 200 000).
    pub max_context_tokens: u64,
    /// Target compression ratio (default: 0.3 = compress to 30%).
    pub target_ratio: f32,
    /// Always keep the last N messages intact (default: 4).
    pub preserve_recent: usize,
    /// Never compact system messages (default: true).
    pub preserve_system: bool,
    /// The compaction strategy to use.
    pub strategy: CompactionStrategy,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger_threshold: 30_000,
            adaptive_trigger_pct: Some(70),
            max_context_tokens: 200_000,
            target_ratio: 0.3,
            preserve_recent: 4,
            preserve_system: true,
            strategy: CompactionStrategy::Hybrid,
        }
    }
}

impl CompactionConfig {
    /// Compute the effective trigger threshold in tokens.
    ///
    /// Returns `adaptive_trigger_pct % of max_context_tokens` when the
    /// adaptive mode is active, otherwise returns `trigger_threshold`.
    pub fn effective_trigger(&self) -> usize {
        if let Some(pct) = self.adaptive_trigger_pct {
            let pct = pct.clamp(1, 99) as f64;
            ((self.max_context_tokens as f64 * pct / 100.0) as usize).max(1)
        } else {
            self.trigger_threshold
        }
    }
}

/// The context compactor engine.
///
/// Analyzes conversation history and compacts it when the token count
/// exceeds the configured threshold. The engine is stateless; it operates
/// on a snapshot of messages and returns a [`CompactionResult`].
pub struct ContextCompactorEngine {
    config: CompactionConfig,
}

impl ContextCompactorEngine {
    /// Create a new compactor with the given configuration.
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Create a new compactor with default configuration.
    pub fn with_defaults() -> Self {
        Self {
            config: CompactionConfig::default(),
        }
    }

    /// Return a reference to the engine's configuration.
    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }

    /// Check whether compaction should trigger for the given messages.
    ///
    /// Returns `true` if the engine is enabled and the total estimated tokens
    /// exceed the effective trigger threshold (adaptive or static).
    pub fn should_compact(&self, messages: &[CompactableMessage]) -> bool {
        if !self.config.enabled {
            return false;
        }
        let total_tokens: usize = messages.iter().map(|m| m.token_estimate).sum();
        total_tokens >= self.config.effective_trigger()
    }

    /// Compact the given messages using the configured strategy.
    ///
    /// Returns `None` if compaction is disabled or unnecessary (below threshold).
    pub fn compact(&self, messages: &[CompactableMessage]) -> Option<CompactionResult> {
        if !self.config.enabled || messages.is_empty() {
            return None;
        }

        let total_tokens: usize = messages.iter().map(|m| m.token_estimate).sum();

        // Don't compact if below the effective trigger
        if total_tokens < self.config.effective_trigger() {
            return None;
        }

        let result = match &self.config.strategy {
            CompactionStrategy::Summarize => self.compact_summarize(messages, total_tokens),
            CompactionStrategy::SlidingWindow => {
                self.compact_sliding_window(messages, total_tokens)
            }
            CompactionStrategy::ImportanceBased => {
                self.compact_importance_based(messages, total_tokens)
            }
            CompactionStrategy::Hybrid => self.compact_hybrid(messages, total_tokens),
        };

        Some(result)
    }

    /// Build a prompt that asks an LLM to summarize compacted messages.
    ///
    /// Used when the Summarize or Hybrid strategy needs an LLM to generate
    /// a summary of older conversation history.
    pub fn build_summary_prompt(&self, messages: &[CompactableMessage]) -> String {
        let conversation = messages
            .iter()
            .map(|m| format!("[{}]: {}", m.role, truncate_content(&m.content, 200)))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Summarize the following conversation concisely, preserving key facts, \
             decisions, and tool results. Focus on information that would be needed \
             to continue the conversation.\n\n{conversation}\n\n\
             Provide a concise summary:"
        )
    }

    // --- Strategy implementations ---

    /// Summarize strategy: replace all old messages with a single summary.
    fn compact_summarize(
        &self,
        messages: &[CompactableMessage],
        original_tokens: usize,
    ) -> CompactionResult {
        let preserve_count = self.config.preserve_recent.min(messages.len());
        let (old_messages, recent_messages) =
            messages.split_at(messages.len().saturating_sub(preserve_count));

        let mut preserved = Vec::new();

        // Preserve system messages if configured
        if self.config.preserve_system {
            for msg in old_messages.iter().filter(|m| m.is_system()) {
                preserved.push(CompactedMessage {
                    role: msg.role.clone(),
                    content: msg.content.clone(),
                    is_summary: false,
                    original_count: 1,
                });
            }
        }

        // Create summary of non-system old messages
        let summarizable: Vec<&CompactableMessage> = old_messages
            .iter()
            .filter(|m| !m.is_system() || !self.config.preserve_system)
            .collect();

        let summary = if !summarizable.is_empty() {
            let summary_text = generate_extractive_summary(&summarizable);
            preserved.push(CompactedMessage {
                role: "system".to_string(),
                content: format!("[Conversation summary] {summary_text}"),
                is_summary: true,
                original_count: summarizable.len(),
            });
            Some(summary_text)
        } else {
            None
        };

        // Add recent messages
        for msg in recent_messages {
            preserved.push(CompactedMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                is_summary: false,
                original_count: 1,
            });
        }

        let compacted_tokens: usize = preserved.iter().map(|m| estimate_tokens(&m.content)).sum();
        let compression_ratio = if original_tokens > 0 {
            compacted_tokens as f32 / original_tokens as f32
        } else {
            1.0
        };

        CompactionResult {
            original_message_count: messages.len(),
            compacted_message_count: preserved.len(),
            original_tokens,
            compacted_tokens,
            compression_ratio,
            summary,
            preserved_messages: preserved,
        }
    }

    /// Sliding window strategy: keep only the most recent messages.
    fn compact_sliding_window(
        &self,
        messages: &[CompactableMessage],
        original_tokens: usize,
    ) -> CompactionResult {
        let target_tokens = (original_tokens as f32 * self.config.target_ratio) as usize;
        let mut preserved = Vec::new();
        let mut current_tokens = 0_usize;

        // Always include system messages if configured
        if self.config.preserve_system {
            for msg in messages.iter().filter(|m| m.is_system()) {
                current_tokens += msg.token_estimate;
                preserved.push(CompactedMessage {
                    role: msg.role.clone(),
                    content: msg.content.clone(),
                    is_summary: false,
                    original_count: 1,
                });
            }
        }

        // Add messages from the end until we hit the target
        let non_system: Vec<&CompactableMessage> =
            messages.iter().filter(|m| !m.is_system()).collect();

        let mut window_messages = Vec::new();
        for msg in non_system.iter().rev() {
            if current_tokens + msg.token_estimate > target_tokens
                && window_messages.len() >= self.config.preserve_recent
            {
                break;
            }
            current_tokens += msg.token_estimate;
            window_messages.push(CompactedMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                is_summary: false,
                original_count: 1,
            });
        }

        // Reverse to maintain chronological order
        window_messages.reverse();
        preserved.extend(window_messages);

        let compacted_tokens: usize = preserved.iter().map(|m| estimate_tokens(&m.content)).sum();
        let compression_ratio = if original_tokens > 0 {
            compacted_tokens as f32 / original_tokens as f32
        } else {
            1.0
        };

        CompactionResult {
            original_message_count: messages.len(),
            compacted_message_count: preserved.len(),
            original_tokens,
            compacted_tokens,
            compression_ratio,
            summary: None,
            preserved_messages: preserved,
        }
    }

    /// Importance-based strategy: keep messages scored as important.
    fn compact_importance_based(
        &self,
        messages: &[CompactableMessage],
        original_tokens: usize,
    ) -> CompactionResult {
        let target_tokens = (original_tokens as f32 * self.config.target_ratio) as usize;

        // Score each message by importance
        let mut scored: Vec<(usize, f32)> = messages
            .iter()
            .enumerate()
            .map(|(i, msg)| (i, score_importance(msg, i, messages.len())))
            .collect();

        // Sort by importance descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Select messages up to the token target
        let mut selected_indices: HashSet<usize> = HashSet::new();
        let mut current_tokens = 0_usize;

        for (idx, _score) in &scored {
            let msg = &messages[*idx];
            if current_tokens + msg.token_estimate > target_tokens
                && selected_indices.len() >= self.config.preserve_recent
            {
                break;
            }
            selected_indices.insert(*idx);
            current_tokens += msg.token_estimate;
        }

        // Always include recent messages
        let recent_start = messages.len().saturating_sub(self.config.preserve_recent);
        for (i, msg) in messages.iter().enumerate().skip(recent_start) {
            if !selected_indices.contains(&i) {
                selected_indices.insert(i);
                current_tokens += msg.token_estimate;
            }
        }

        // Build preserved list in original order
        let mut preserved: Vec<CompactedMessage> = Vec::new();
        let mut sorted_indices: Vec<usize> = selected_indices.into_iter().collect();
        sorted_indices.sort_unstable();

        for idx in sorted_indices {
            let msg = &messages[idx];
            preserved.push(CompactedMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                is_summary: false,
                original_count: 1,
            });
        }

        let compacted_tokens: usize = preserved.iter().map(|m| estimate_tokens(&m.content)).sum();
        let compression_ratio = if original_tokens > 0 {
            compacted_tokens as f32 / original_tokens as f32
        } else {
            1.0
        };

        CompactionResult {
            original_message_count: messages.len(),
            compacted_message_count: preserved.len(),
            original_tokens,
            compacted_tokens,
            compression_ratio,
            summary: None,
            preserved_messages: preserved,
        }
    }

    /// Hybrid strategy: summarize old + keep recent + preserve important.
    ///
    /// This is the recommended strategy:
    /// 1. Preserve all system messages
    /// 2. Keep the last N messages intact
    /// 3. Keep important messages from the middle (tool calls, decisions)
    /// 4. Summarize the rest
    fn compact_hybrid(
        &self,
        messages: &[CompactableMessage],
        original_tokens: usize,
    ) -> CompactionResult {
        let preserve_count = self.config.preserve_recent.min(messages.len());
        let split_point = messages.len().saturating_sub(preserve_count);
        let (old_messages, recent_messages) = messages.split_at(split_point);

        let mut preserved = Vec::new();

        // 1. Preserve system messages
        if self.config.preserve_system {
            for msg in old_messages.iter().filter(|m| m.is_system()) {
                preserved.push(CompactedMessage {
                    role: msg.role.clone(),
                    content: msg.content.clone(),
                    is_summary: false,
                    original_count: 1,
                });
            }
        }

        // 2. Keep important messages from old history
        let non_system_old: Vec<&CompactableMessage> = old_messages
            .iter()
            .filter(|m| !m.is_system() || !self.config.preserve_system)
            .collect();

        let important: Vec<&&CompactableMessage> = non_system_old
            .iter()
            .filter(|m| m.has_tool_call || m.role == "tool")
            .collect();

        for msg in &important {
            preserved.push(CompactedMessage {
                role: msg.role.clone(),
                content: truncate_content(&msg.content, 200),
                is_summary: false,
                original_count: 1,
            });
        }

        // 3. Summarize the rest
        let summarizable: Vec<&CompactableMessage> = non_system_old
            .iter()
            .filter(|m| !m.has_tool_call && m.role != "tool")
            .copied()
            .collect();

        let summary = if !summarizable.is_empty() {
            let summary_text = generate_extractive_summary(&summarizable);
            preserved.push(CompactedMessage {
                role: "system".to_string(),
                content: format!("[Conversation summary] {summary_text}"),
                is_summary: true,
                original_count: summarizable.len(),
            });
            Some(summary_text)
        } else {
            None
        };

        // 4. Add recent messages
        for msg in recent_messages {
            preserved.push(CompactedMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                is_summary: false,
                original_count: 1,
            });
        }

        let compacted_tokens: usize = preserved.iter().map(|m| estimate_tokens(&m.content)).sum();
        let compression_ratio = if original_tokens > 0 {
            compacted_tokens as f32 / original_tokens as f32
        } else {
            1.0
        };

        CompactionResult {
            original_message_count: messages.len(),
            compacted_message_count: preserved.len(),
            original_tokens,
            compacted_tokens,
            compression_ratio,
            summary,
            preserved_messages: preserved,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Rough token estimation (~4 chars per token for English).
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Score the importance of a message for importance-based compaction.
///
/// Higher scores for: system messages, tool-related messages, recent messages,
/// and longer messages (that likely contain more information).
fn score_importance(msg: &CompactableMessage, index: usize, total: usize) -> f32 {
    let mut score = 0.0_f32;

    // System messages are always important
    if msg.is_system() {
        score += 0.9;
    }

    // Tool calls and results are important
    if msg.has_tool_call || msg.role == "tool" {
        score += 0.7;
    }

    // Recency boost (more recent = more important)
    if total > 0 {
        let recency = index as f32 / total as f32;
        score += recency * 0.3;
    }

    // Length-based importance (longer messages contain more info)
    if msg.content.len() > 200 {
        score += 0.2;
    } else if msg.content.len() > 50 {
        score += 0.1;
    }

    // Decision indicators boost
    let lower = msg.content.to_lowercase();
    if lower.contains("decided") || lower.contains("conclusion") || lower.contains("summary") {
        score += 0.2;
    }

    score.min(1.0)
}

/// Generate an extractive summary from a list of messages.
///
/// Takes the first sentence from each message (up to a limit) to create
/// a condensed summary. This is a heuristic approach; for better summaries,
/// use `build_summary_prompt()` with an LLM.
fn generate_extractive_summary(messages: &[&CompactableMessage]) -> String {
    let max_sentences = 8;
    let mut sentences = Vec::new();

    for msg in messages {
        if sentences.len() >= max_sentences {
            break;
        }
        // Take the first sentence of each message
        let first_sentence = msg.content.split('.').next().unwrap_or("").trim();
        if first_sentence.len() > 10 {
            sentences.push(format!(
                "[{}] {}",
                msg.role,
                truncate_content(first_sentence, 100)
            ));
        }
    }

    if sentences.is_empty() {
        "Previous conversation context (details compacted)".to_string()
    } else {
        sentences.join(". ")
    }
}

/// Truncate content to a maximum character length, appending "..." if truncated.
fn truncate_content(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        content.to_string()
    } else {
        format!("{}...", &content[..max_len.saturating_sub(3)])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Create a set of test messages that exceed the default 30K threshold.
    fn large_conversation() -> Vec<CompactableMessage> {
        let mut msgs = vec![CompactableMessage::system("You are a helpful assistant.")];

        // Generate enough messages to exceed the adaptive default threshold of
        // 140 000 tokens (70% of 200 000).  Each iteration adds ~2 messages of
        // ~5K chars each => ~2.5K tokens per message => ~5K tokens per pair.
        // 200 pairs = ~1 000 000 chars ≈ 200 000 tokens — well above 140 000.
        for i in 0..200 {
            msgs.push(CompactableMessage::user(&format!(
                "Question {i}: Tell me about topic number {i} in great detail please. \
                 I need comprehensive information about this subject. {}",
                "more context and additional detailed information about the topic ".repeat(50)
            )));
            msgs.push(CompactableMessage::assistant(&format!(
                "Answer {i}: Here is detailed information about topic {i}. \
                 The key points are numerous and important. {}",
                "detailed explanation with thorough analysis of the subject matter ".repeat(50)
            )));
        }

        msgs
    }

    /// Create a small conversation below threshold.
    fn small_conversation() -> Vec<CompactableMessage> {
        vec![
            CompactableMessage::system("You are helpful."),
            CompactableMessage::user("Hello"),
            CompactableMessage::assistant("Hi there!"),
        ]
    }

    /// Create a conversation with tool calls.
    fn conversation_with_tools() -> Vec<CompactableMessage> {
        let mut msgs = vec![CompactableMessage::system("You are a helpful assistant.")];

        // Add some regular messages
        for i in 0..30 {
            msgs.push(CompactableMessage::user(&format!(
                "Request {i} about something interesting and detailed. {}",
                "padding text ".repeat(50)
            )));
            msgs.push(CompactableMessage::assistant(&format!(
                "Response {i} with lots of content. {}",
                "response content ".repeat(50)
            )));
        }

        // Add tool call messages
        msgs.push(CompactableMessage::new(
            "assistant",
            "Let me read that file for you using file_read.",
            true,
        ));
        msgs.push(CompactableMessage::tool(
            "File contents: config.toml has database settings.",
        ));

        // Add a few more messages
        for i in 0..5 {
            msgs.push(CompactableMessage::user(&format!(
                "Follow-up question {i} with context. {}",
                "more text ".repeat(50)
            )));
            msgs.push(CompactableMessage::assistant(&format!(
                "Follow-up answer {i} with detail. {}",
                "detailed text ".repeat(50)
            )));
        }

        msgs
    }

    // -----------------------------------------------------------------------
    // CompactionStrategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compaction_strategy_serde_roundtrip() {
        let strategies = vec![
            CompactionStrategy::Summarize,
            CompactionStrategy::SlidingWindow,
            CompactionStrategy::ImportanceBased,
            CompactionStrategy::Hybrid,
        ];
        for s in strategies {
            let json = serde_json::to_string(&s).unwrap();
            let parsed: CompactionStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, s);
        }
    }

    // -----------------------------------------------------------------------
    // CompactableMessage tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compactable_message_new() {
        let msg = CompactableMessage::new("user", "Hello world", false);
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello world");
        assert!(!msg.has_tool_call);
        assert!(msg.token_estimate > 0);
    }

    #[test]
    fn test_compactable_message_constructors() {
        let sys = CompactableMessage::system("system prompt");
        assert!(sys.is_system());
        assert!(!sys.is_user());

        let usr = CompactableMessage::user("user input");
        assert!(usr.is_user());
        assert!(!usr.is_assistant());

        let asst = CompactableMessage::assistant("assistant output");
        assert!(asst.is_assistant());

        let tool = CompactableMessage::tool("tool result");
        assert!(tool.has_tool_call);
    }

    // -----------------------------------------------------------------------
    // CompactionConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compaction_config_defaults() {
        let config = CompactionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.trigger_threshold, 30_000);
        assert!((config.target_ratio - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.preserve_recent, 4);
        assert!(config.preserve_system);
        assert_eq!(config.strategy, CompactionStrategy::Hybrid);
    }

    // -----------------------------------------------------------------------
    // Adaptive trigger (issue #25 — L-01) tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_effective_trigger_static_when_adaptive_disabled() {
        let cfg = CompactionConfig {
            adaptive_trigger_pct: None,
            trigger_threshold: 30_000,
            ..CompactionConfig::default()
        };
        assert_eq!(cfg.effective_trigger(), 30_000);
    }

    #[test]
    fn test_effective_trigger_adaptive_70pct_of_200k() {
        // Default: 70% of 200 000 = 140 000
        let cfg = CompactionConfig::default();
        assert_eq!(cfg.effective_trigger(), 140_000);
    }

    #[test]
    fn test_effective_trigger_adaptive_70pct_of_128k() {
        // GPT-4o context window
        let cfg = CompactionConfig {
            adaptive_trigger_pct: Some(70),
            max_context_tokens: 128_000,
            ..CompactionConfig::default()
        };
        assert_eq!(cfg.effective_trigger(), 89_600);
    }

    #[test]
    fn test_effective_trigger_clamps_pct_to_valid_range() {
        // pct=0 → clamped to 1 → 1% of 200K = 2000
        let cfg_zero = CompactionConfig {
            adaptive_trigger_pct: Some(0),
            max_context_tokens: 200_000,
            ..CompactionConfig::default()
        };
        assert_eq!(cfg_zero.effective_trigger(), 2_000);

        // pct=100 → clamped to 99 → 99% of 200K = 198 000
        let cfg_hundred = CompactionConfig {
            adaptive_trigger_pct: Some(100),
            max_context_tokens: 200_000,
            ..CompactionConfig::default()
        };
        assert_eq!(cfg_hundred.effective_trigger(), 198_000);
    }

    #[test]
    fn test_should_compact_uses_adaptive_threshold() {
        // Adaptive at 1% of 200 000 = 2 000 tokens — should fire on a small conversation
        let engine = ContextCompactorEngine::new(CompactionConfig {
            adaptive_trigger_pct: Some(1),
            max_context_tokens: 200_000,
            ..CompactionConfig::default()
        });
        // Even a short conversation exceeds 2 000 tokens when repeated enough
        let msgs: Vec<CompactableMessage> = (0..20)
            .flat_map(|i| {
                vec![
                    CompactableMessage::user(&format!("question {i}: {}", "word ".repeat(100))),
                    CompactableMessage::assistant(&format!("answer {i}: {}", "word ".repeat(100))),
                ]
            })
            .collect();
        assert!(
            engine.should_compact(&msgs),
            "Adaptive trigger at 1% should fire on a moderately-sized conversation"
        );
    }

    // -----------------------------------------------------------------------
    // ContextCompactorEngine basic tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_engine_with_defaults() {
        let engine = ContextCompactorEngine::with_defaults();
        assert!(engine.config().enabled);
        assert_eq!(engine.config().trigger_threshold, 30_000);
    }

    #[test]
    fn test_should_compact_below_threshold() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = small_conversation();
        assert!(!engine.should_compact(&msgs));
    }

    #[test]
    fn test_should_compact_above_threshold() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = large_conversation();
        assert!(engine.should_compact(&msgs));
    }

    #[test]
    fn test_should_compact_disabled() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            enabled: false,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        assert!(!engine.should_compact(&msgs));
    }

    #[test]
    fn test_compact_disabled_returns_none() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            enabled: false,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        assert!(engine.compact(&msgs).is_none());
    }

    #[test]
    fn test_compact_below_threshold_returns_none() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = small_conversation();
        assert!(engine.compact(&msgs).is_none());
    }

    #[test]
    fn test_compact_empty_returns_none() {
        let engine = ContextCompactorEngine::with_defaults();
        assert!(engine.compact(&[]).is_none());
    }

    // -----------------------------------------------------------------------
    // Summarize strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compact_summarize_reduces_messages() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::Summarize,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.compacted_message_count < result.original_message_count,
            "Compaction should reduce message count: {} < {}",
            result.compacted_message_count,
            result.original_message_count
        );
    }

    #[test]
    fn test_compact_summarize_produces_summary() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::Summarize,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.summary.is_some(),
            "Summarize strategy should produce a summary"
        );
    }

    #[test]
    fn test_compact_summarize_preserves_system() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::Summarize,
            preserve_system: true,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        let system_msgs: Vec<&CompactedMessage> = result
            .preserved_messages
            .iter()
            .filter(|m| m.role == "system" && !m.is_summary)
            .collect();
        assert!(
            !system_msgs.is_empty(),
            "System messages should be preserved"
        );
    }

    #[test]
    fn test_compact_summarize_preserves_recent() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::Summarize,
            preserve_recent: 4,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        // Last 4 messages should be preserved intact
        let non_summary: Vec<&CompactedMessage> = result
            .preserved_messages
            .iter()
            .filter(|m| !m.is_summary)
            .collect();
        assert!(
            non_summary.len() >= 4,
            "At least 4 recent messages should be preserved, got {}",
            non_summary.len()
        );
    }

    // -----------------------------------------------------------------------
    // Sliding window strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compact_sliding_window_reduces() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::SlidingWindow,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.compacted_message_count < result.original_message_count,
            "Sliding window should reduce message count"
        );
    }

    #[test]
    fn test_compact_sliding_window_no_summary() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::SlidingWindow,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.summary.is_none(),
            "Sliding window should not produce a summary"
        );
    }

    // -----------------------------------------------------------------------
    // Importance-based strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compact_importance_reduces() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::ImportanceBased,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(result.compacted_message_count < result.original_message_count);
    }

    #[test]
    fn test_compact_importance_keeps_system() {
        let engine = ContextCompactorEngine::new(CompactionConfig {
            strategy: CompactionStrategy::ImportanceBased,
            ..CompactionConfig::default()
        });
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        let has_system = result.preserved_messages.iter().any(|m| m.role == "system");
        assert!(has_system, "System messages should be preserved");
    }

    // -----------------------------------------------------------------------
    // Hybrid strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compact_hybrid_reduces() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(result.compacted_message_count < result.original_message_count);
    }

    #[test]
    fn test_compact_hybrid_produces_summary() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.summary.is_some(),
            "Hybrid strategy should produce a summary"
        );
    }

    #[test]
    fn test_compact_hybrid_preserves_tool_calls() {
        let msgs = conversation_with_tools();
        // Use a low threshold to ensure compaction triggers regardless of message size
        let engine = ContextCompactorEngine::new(CompactionConfig {
            trigger_threshold: 100,
            // Disable adaptive mode so the static trigger_threshold of 100 is used.
            adaptive_trigger_pct: None,
            ..CompactionConfig::default()
        });
        let result = engine.compact(&msgs).unwrap();
        let has_tool = result
            .preserved_messages
            .iter()
            .any(|m| m.role == "tool" || m.content.contains("file_read"));
        assert!(has_tool, "Hybrid should preserve tool call messages");
    }

    // -----------------------------------------------------------------------
    // Compression ratio tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compression_ratio_less_than_one() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.compression_ratio < 1.0,
            "Compression ratio should be < 1.0, got {}",
            result.compression_ratio
        );
    }

    #[test]
    fn test_compression_ratio_positive() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = large_conversation();
        let result = engine.compact(&msgs).unwrap();
        assert!(
            result.compression_ratio > 0.0,
            "Compression ratio should be > 0.0"
        );
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("test"), 1);
        assert!(estimate_tokens("hello world this is a longer text") > 0);
    }

    #[test]
    fn test_truncate_content_short() {
        assert_eq!(truncate_content("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let truncated = truncate_content("hello world this is a long text", 10);
        assert!(truncated.len() <= 13); // 10 chars + "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_score_importance_system_high() {
        let msg = CompactableMessage::system("System prompt");
        let score = score_importance(&msg, 0, 10);
        assert!(score >= 0.9, "System messages should score high: {score}");
    }

    #[test]
    fn test_score_importance_tool_high() {
        let msg = CompactableMessage::tool("Tool result");
        let score = score_importance(&msg, 0, 10);
        assert!(score >= 0.7, "Tool messages should score high: {score}");
    }

    #[test]
    fn test_score_importance_recency_boost() {
        let msg = CompactableMessage::user("Just a question about something");
        let old_score = score_importance(&msg, 0, 100);
        let new_score = score_importance(&msg, 99, 100);
        assert!(
            new_score > old_score,
            "Recent messages should score higher: {new_score} > {old_score}"
        );
    }

    #[test]
    fn test_generate_extractive_summary() {
        let msgs = vec![
            CompactableMessage::user("How does Rust handle memory? This is a detailed question."),
            CompactableMessage::assistant(
                "Rust uses ownership and borrowing. The compiler checks at compile time.",
            ),
        ];
        let refs: Vec<&CompactableMessage> = msgs.iter().collect();
        let summary = generate_extractive_summary(&refs);
        assert!(!summary.is_empty());
        assert!(summary.contains("[user]") || summary.contains("[assistant]"));
    }

    #[test]
    fn test_generate_extractive_summary_empty() {
        let refs: Vec<&CompactableMessage> = vec![];
        let summary = generate_extractive_summary(&refs);
        assert!(summary.contains("compacted"));
    }

    // -----------------------------------------------------------------------
    // Build prompt tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_summary_prompt() {
        let engine = ContextCompactorEngine::with_defaults();
        let msgs = vec![
            CompactableMessage::user("What is Rust?"),
            CompactableMessage::assistant("Rust is a programming language."),
        ];
        let prompt = engine.build_summary_prompt(&msgs);
        assert!(prompt.contains("Summarize"));
        assert!(prompt.contains("[user]"));
        assert!(prompt.contains("[assistant]"));
    }

    // -----------------------------------------------------------------------
    // Serde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compaction_result_serde_roundtrip() {
        let result = CompactionResult {
            original_message_count: 100,
            compacted_message_count: 10,
            original_tokens: 30000,
            compacted_tokens: 9000,
            compression_ratio: 0.3,
            summary: Some("test summary".to_string()),
            preserved_messages: vec![CompactedMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                is_summary: false,
                original_count: 1,
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: CompactionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.original_message_count, 100);
        assert_eq!(parsed.compacted_message_count, 10);
        assert!((parsed.compression_ratio - 0.3).abs() < f32::EPSILON);
        assert_eq!(parsed.preserved_messages.len(), 1);
    }

    #[test]
    fn test_compacted_message_serde_roundtrip() {
        let msg = CompactedMessage {
            role: "assistant".to_string(),
            content: "test content".to_string(),
            is_summary: true,
            original_count: 5,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: CompactedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, "assistant");
        assert!(parsed.is_summary);
        assert_eq!(parsed.original_count, 5);
    }
}

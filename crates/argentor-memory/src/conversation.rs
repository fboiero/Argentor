//! Conversation memory system for cross-session customer context.
//!
//! Enables agents to recall context from previous sessions with the same customer,
//! building rich customer profiles and injecting relevant history into agent prompts.
//!
//! # Main types
//!
//! - [`ConversationMemory`] — Thread-safe store for conversation turns, keyed by customer.
//! - [`ConversationTurn`] — A single turn (user/assistant/tool) in a conversation.
//! - [`CustomerProfile`] — Aggregated customer context across all sessions.
//! - [`ConversationSummarizer`] — Builds context strings for agent system-prompt injection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// A single turn in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// The customer this turn belongs to.
    pub customer_id: String,
    /// The session in which this turn occurred.
    pub session_id: String,
    /// The role of the speaker: "user", "assistant", or "tool".
    pub role: String,
    /// The textual content of the turn.
    pub content: String,
    /// When this turn was recorded.
    pub timestamp: DateTime<Utc>,
    /// Arbitrary metadata (agent_role, model, tokens, sentiment, etc.).
    pub metadata: HashMap<String, String>,
}

/// Aggregated customer context built from conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerProfile {
    /// The customer identifier.
    pub customer_id: String,
    /// How many distinct sessions this customer has had.
    pub total_sessions: usize,
    /// Total number of conversation turns across all sessions.
    pub total_turns: usize,
    /// Timestamp of the first recorded interaction.
    pub first_interaction: DateTime<Utc>,
    /// Timestamp of the most recent interaction.
    pub last_interaction: DateTime<Utc>,
    /// Topics extracted from conversation content (deduplicated keywords).
    pub topics: Vec<String>,
    /// Overall sentiment trend: "positive", "neutral", or "negative".
    pub sentiment_trend: String,
}

/// Thread-safe conversation memory that stores and retrieves turns per customer.
///
/// Internally uses `Arc<RwLock<HashMap<String, Vec<ConversationTurn>>>>` so it can
/// be shared across async tasks and agent threads.
#[derive(Debug, Clone)]
pub struct ConversationMemory {
    /// customer_id -> ordered list of turns (oldest first).
    turns: Arc<RwLock<HashMap<String, Vec<ConversationTurn>>>>,
}

impl Default for ConversationMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationMemory {
    /// Create an empty conversation memory.
    pub fn new() -> Self {
        Self {
            turns: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a conversation turn for a customer.
    pub async fn record_turn(
        &self,
        customer_id: &str,
        session_id: &str,
        role: &str,
        content: &str,
        metadata: HashMap<String, String>,
    ) {
        let turn = ConversationTurn {
            customer_id: customer_id.to_string(),
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            metadata,
        };
        let mut store = self.turns.write().await;
        store.entry(customer_id.to_string()).or_default().push(turn);
    }

    /// Record a turn with an explicit timestamp (useful for tests and imports).
    pub async fn record_turn_with_timestamp(
        &self,
        customer_id: &str,
        session_id: &str,
        role: &str,
        content: &str,
        metadata: HashMap<String, String>,
        timestamp: DateTime<Utc>,
    ) {
        let turn = ConversationTurn {
            customer_id: customer_id.to_string(),
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp,
            metadata,
        };
        let mut store = self.turns.write().await;
        store.entry(customer_id.to_string()).or_default().push(turn);
    }

    /// Retrieve the last `max_turns` turns for a customer across all sessions.
    ///
    /// Returns turns in chronological order (oldest first among the returned set).
    pub async fn get_context(&self, customer_id: &str, max_turns: usize) -> Vec<ConversationTurn> {
        let store = self.turns.read().await;
        match store.get(customer_id) {
            Some(turns) => {
                let len = turns.len();
                if len <= max_turns {
                    turns.clone()
                } else {
                    turns[len - max_turns..].to_vec()
                }
            }
            None => Vec::new(),
        }
    }

    /// Build a text summary of all interactions for a customer.
    ///
    /// Returns a human-readable multi-line summary including session count,
    /// turn count, time range, and a condensed transcript.
    pub async fn summarize_history(&self, customer_id: &str) -> String {
        let store = self.turns.read().await;
        let turns = match store.get(customer_id) {
            Some(t) => t,
            None => return format!("No history found for customer '{customer_id}'."),
        };

        if turns.is_empty() {
            return format!("No history found for customer '{customer_id}'.");
        }

        let sessions: HashSet<&str> = turns.iter().map(|t| t.session_id.as_str()).collect();
        let first = turns.first().map(|t| t.timestamp).unwrap_or_else(Utc::now);
        let last = turns.last().map(|t| t.timestamp).unwrap_or_else(Utc::now);

        let mut summary = format!(
            "Customer: {customer_id}\nSessions: {}\nTotal turns: {}\nFirst interaction: {}\nLast interaction: {}\n\nTranscript:\n",
            sessions.len(),
            turns.len(),
            first.format("%Y-%m-%d %H:%M UTC"),
            last.format("%Y-%m-%d %H:%M UTC"),
        );

        for turn in turns {
            summary.push_str(&format!(
                "[{}] [{}] {}: {}\n",
                turn.timestamp.format("%Y-%m-%d %H:%M"),
                turn.session_id,
                turn.role,
                truncate_content(&turn.content, 200),
            ));
        }

        summary
    }

    /// Get a list of distinct session IDs for a customer.
    pub async fn get_sessions(&self, customer_id: &str) -> Vec<String> {
        let store = self.turns.read().await;
        match store.get(customer_id) {
            Some(turns) => {
                let mut seen = HashSet::new();
                let mut sessions = Vec::new();
                for turn in turns {
                    if seen.insert(turn.session_id.clone()) {
                        sessions.push(turn.session_id.clone());
                    }
                }
                sessions
            }
            None => Vec::new(),
        }
    }

    /// Case-insensitive keyword search across all turns for a customer.
    ///
    /// Returns turns whose content contains the query substring.
    pub async fn search_history(&self, customer_id: &str, query: &str) -> Vec<ConversationTurn> {
        let store = self.turns.read().await;
        let query_lower = query.to_lowercase();
        match store.get(customer_id) {
            Some(turns) => turns
                .iter()
                .filter(|t| t.content.to_lowercase().contains(&query_lower))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }

    /// Build a [`CustomerProfile`] from the stored conversation history.
    ///
    /// Topics are extracted as the most frequent non-stopword tokens.
    /// Sentiment is inferred from metadata if present, otherwise defaults to "neutral".
    pub async fn build_profile(&self, customer_id: &str) -> Option<CustomerProfile> {
        let store = self.turns.read().await;
        let turns = store.get(customer_id)?;
        if turns.is_empty() {
            return None;
        }

        let sessions: HashSet<&str> = turns.iter().map(|t| t.session_id.as_str()).collect();
        let first = turns.first().map(|t| t.timestamp).unwrap_or_else(Utc::now);
        let last = turns.last().map(|t| t.timestamp).unwrap_or_else(Utc::now);
        let topics = extract_topics(turns);
        let sentiment = infer_sentiment(turns);

        Some(CustomerProfile {
            customer_id: customer_id.to_string(),
            total_sessions: sessions.len(),
            total_turns: turns.len(),
            first_interaction: first,
            last_interaction: last,
            topics,
            sentiment_trend: sentiment,
        })
    }

    /// Return the total number of customers tracked.
    pub async fn customer_count(&self) -> usize {
        let store = self.turns.read().await;
        store.len()
    }

    /// Return all customer IDs.
    pub async fn customer_ids(&self) -> Vec<String> {
        let store = self.turns.read().await;
        store.keys().cloned().collect()
    }

    /// Return total turn count for a customer.
    pub async fn turn_count(&self, customer_id: &str) -> usize {
        let store = self.turns.read().await;
        store.get(customer_id).map_or(0, Vec::len)
    }
}

/// Builds context strings suitable for injection into an agent's system prompt.
pub struct ConversationSummarizer;

impl ConversationSummarizer {
    /// Build a context string for agent injection, respecting an approximate token limit.
    ///
    /// The output looks like:
    /// ```text
    /// [Customer History]
    /// Customer: cust_123
    /// Sessions: 3 | Turns: 15 | Since: 2025-01-15
    /// Topics: billing, DeFi, staking
    /// Sentiment: positive
    ///
    /// Last 5 interactions:
    /// - user: How do I stake ETH?
    /// - assistant: You can stake ETH via ...
    /// ```
    ///
    /// `max_tokens` is an approximate character budget (1 token ~ 4 chars).
    pub async fn build_context(
        memory: &ConversationMemory,
        customer_id: &str,
        max_tokens: usize,
    ) -> String {
        let max_chars = max_tokens * 4;

        let profile = memory.build_profile(customer_id).await;
        let profile = match profile {
            Some(p) => p,
            None => {
                return format!("[Customer History]\nNo prior interactions with '{customer_id}'.");
            }
        };

        let mut result = String::from("[Customer History]\n");
        result.push_str(&format!("Customer: {}\n", profile.customer_id));
        result.push_str(&format!(
            "Sessions: {} | Turns: {} | Since: {}\n",
            profile.total_sessions,
            profile.total_turns,
            profile.first_interaction.format("%Y-%m-%d"),
        ));

        if !profile.topics.is_empty() {
            result.push_str(&format!("Topics: {}\n", profile.topics.join(", ")));
        }
        result.push_str(&format!("Sentiment: {}\n", profile.sentiment_trend));

        // Determine how many turns we can fit
        let header_len = result.len();
        let remaining = max_chars.saturating_sub(header_len + 30); // 30 chars for the section heading

        // Fetch recent turns, starting with 10 and trimming if needed
        let max_recent = 10;
        let recent = memory.get_context(customer_id, max_recent).await;

        if recent.is_empty() {
            return result;
        }

        result.push_str(&format!("\nLast {} interactions:\n", recent.len()));

        let mut used = 0;
        let mut included = Vec::new();
        for turn in recent.iter().rev() {
            let line = format!(
                "- {}: {}\n",
                turn.role,
                truncate_content(&turn.content, 150),
            );
            if used + line.len() > remaining {
                break;
            }
            used += line.len();
            included.push(line);
        }

        // Reverse to chronological order
        included.reverse();
        for line in &included {
            result.push_str(line);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate_content(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        truncated.push_str("...");
        truncated
    }
}

/// Simple stopword list for topic extraction.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "to",
    "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through", "during",
    "before", "after", "and", "but", "or", "nor", "not", "so", "yet", "both", "either", "neither",
    "each", "every", "all", "any", "few", "more", "most", "other", "some", "such", "no", "only",
    "own", "same", "than", "too", "very", "just", "because", "about", "up", "out", "then", "them",
    "these", "those", "this", "that", "it", "its", "i", "me", "my", "we", "our", "you", "your",
    "he", "him", "his", "she", "her", "they", "their", "what", "which", "who", "whom", "how",
    "when", "where", "why", "if", "while", "also", "like", "get", "got", "want", "need", "know",
    "think",
];

/// Extract the most frequent non-stopword tokens from conversation content.
/// Returns up to 10 topics sorted by frequency (descending).
fn extract_topics(turns: &[ConversationTurn]) -> Vec<String> {
    let mut freq: HashMap<String, usize> = HashMap::new();
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();

    for turn in turns {
        for word in turn.content.split_whitespace() {
            let clean: String = word
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            if clean.len() >= 3 && !stopwords.contains(clean.as_str()) {
                *freq.entry(clean).or_insert(0) += 1;
            }
        }
    }

    let mut sorted: Vec<(String, usize)> = freq.into_iter().collect();
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    sorted.into_iter().take(10).map(|(word, _)| word).collect()
}

/// Infer overall sentiment from metadata tags or simple keyword heuristics.
///
/// If turns carry a "sentiment" metadata key, we tally positive/negative/neutral.
/// Otherwise, falls back to a naive keyword scan.
fn infer_sentiment(turns: &[ConversationTurn]) -> String {
    let mut positive = 0i32;
    let mut negative = 0i32;

    let positive_words = [
        "thanks",
        "thank",
        "great",
        "good",
        "excellent",
        "perfect",
        "happy",
        "love",
        "awesome",
        "helpful",
        "appreciate",
        "wonderful",
        "fantastic",
        "pleased",
        "satisfied",
    ];
    let negative_words = [
        "bad",
        "terrible",
        "awful",
        "horrible",
        "hate",
        "angry",
        "frustrated",
        "disappointed",
        "worst",
        "broken",
        "fail",
        "failed",
        "issue",
        "problem",
        "error",
        "bug",
        "complaint",
    ];

    for turn in turns {
        // Check metadata first
        if let Some(sentiment) = turn.metadata.get("sentiment") {
            match sentiment.to_lowercase().as_str() {
                "positive" => positive += 2,
                "negative" => negative += 2,
                _ => {}
            }
            continue;
        }

        // Fallback: keyword scan on user turns
        if turn.role == "user" {
            let lower = turn.content.to_lowercase();
            for &pw in &positive_words {
                if lower.contains(pw) {
                    positive += 1;
                }
            }
            for &nw in &negative_words {
                if lower.contains(nw) {
                    negative += 1;
                }
            }
        }
    }

    if positive > negative {
        "positive".to_string()
    } else if negative > positive {
        "negative".to_string()
    } else {
        "neutral".to_string()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn meta() -> HashMap<String, String> {
        HashMap::new()
    }

    fn meta_with(key: &str, val: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(key.to_string(), val.to_string());
        m
    }

    // -----------------------------------------------------------------------
    // ConversationTurn basic tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_conversation_turn_creation() {
        let turn = ConversationTurn {
            customer_id: "c1".to_string(),
            session_id: "s1".to_string(),
            role: "user".to_string(),
            content: "Hello".to_string(),
            timestamp: Utc::now(),
            metadata: meta(),
        };
        assert_eq!(turn.customer_id, "c1");
        assert_eq!(turn.role, "user");
    }

    #[test]
    fn test_conversation_turn_serialization() {
        let turn = ConversationTurn {
            customer_id: "c1".to_string(),
            session_id: "s1".to_string(),
            role: "assistant".to_string(),
            content: "Hi there".to_string(),
            timestamp: Utc.with_ymd_and_hms(2025, 6, 15, 10, 0, 0).unwrap(),
            metadata: meta_with("model", "gpt-4"),
        };
        let json = serde_json::to_string(&turn).unwrap();
        let restored: ConversationTurn = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.content, "Hi there");
        assert_eq!(restored.metadata.get("model").unwrap(), "gpt-4");
    }

    // -----------------------------------------------------------------------
    // ConversationMemory tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_record_and_get_context() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "Hello", meta()).await;
        mem.record_turn("c1", "s1", "assistant", "Hi!", meta())
            .await;

        let ctx = mem.get_context("c1", 10).await;
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx[0].role, "user");
        assert_eq!(ctx[1].role, "assistant");
    }

    #[tokio::test]
    async fn test_get_context_max_turns_limit() {
        let mem = ConversationMemory::new();
        for i in 0..10 {
            mem.record_turn("c1", "s1", "user", &format!("msg {i}"), meta())
                .await;
        }

        let ctx = mem.get_context("c1", 3).await;
        assert_eq!(ctx.len(), 3);
        // Should be the last 3 turns
        assert_eq!(ctx[0].content, "msg 7");
        assert_eq!(ctx[2].content, "msg 9");
    }

    #[tokio::test]
    async fn test_get_context_unknown_customer() {
        let mem = ConversationMemory::new();
        let ctx = mem.get_context("unknown", 5).await;
        assert!(ctx.is_empty());
    }

    #[tokio::test]
    async fn test_get_sessions_single() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "a", meta()).await;
        mem.record_turn("c1", "s1", "assistant", "b", meta()).await;

        let sessions = mem.get_sessions("c1").await;
        assert_eq!(sessions, vec!["s1"]);
    }

    #[tokio::test]
    async fn test_get_sessions_multiple() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "a", meta()).await;
        mem.record_turn("c1", "s2", "user", "b", meta()).await;
        mem.record_turn("c1", "s1", "assistant", "c", meta()).await;
        mem.record_turn("c1", "s3", "user", "d", meta()).await;

        let sessions = mem.get_sessions("c1").await;
        assert_eq!(sessions, vec!["s1", "s2", "s3"]);
    }

    #[tokio::test]
    async fn test_get_sessions_unknown_customer() {
        let mem = ConversationMemory::new();
        let sessions = mem.get_sessions("ghost").await;
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_search_history_matches() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "How do I stake ETH?", meta())
            .await;
        mem.record_turn("c1", "s1", "assistant", "You can stake via...", meta())
            .await;
        mem.record_turn("c1", "s2", "user", "What about billing?", meta())
            .await;

        let results = mem.search_history("c1", "stake").await;
        // Both the user question and assistant reply contain "stake"
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|r| r.content.to_lowercase().contains("stake")));

        // "billing" only appears once
        let billing = mem.search_history("c1", "billing").await;
        assert_eq!(billing.len(), 1);
    }

    #[tokio::test]
    async fn test_search_history_case_insensitive() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "BILLING question", meta())
            .await;

        let results = mem.search_history("c1", "billing").await;
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_search_history_no_match() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "Hello world", meta())
            .await;

        let results = mem.search_history("c1", "blockchain").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_history_unknown_customer() {
        let mem = ConversationMemory::new();
        let results = mem.search_history("unknown", "anything").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_summarize_history() {
        let mem = ConversationMemory::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        mem.record_turn_with_timestamp("c1", "s1", "user", "Hello", meta(), ts)
            .await;
        mem.record_turn_with_timestamp("c1", "s1", "assistant", "Hi!", meta(), ts)
            .await;

        let summary = mem.summarize_history("c1").await;
        assert!(summary.contains("Customer: c1"));
        assert!(summary.contains("Sessions: 1"));
        assert!(summary.contains("Total turns: 2"));
        assert!(summary.contains("Transcript:"));
    }

    #[tokio::test]
    async fn test_summarize_history_unknown() {
        let mem = ConversationMemory::new();
        let summary = mem.summarize_history("ghost").await;
        assert!(summary.contains("No history found"));
    }

    // -----------------------------------------------------------------------
    // CustomerProfile tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_build_profile() {
        let mem = ConversationMemory::new();
        let ts1 = Utc.with_ymd_and_hms(2025, 1, 10, 8, 0, 0).unwrap();
        let ts2 = Utc.with_ymd_and_hms(2025, 3, 20, 14, 0, 0).unwrap();

        mem.record_turn_with_timestamp("c1", "s1", "user", "billing question", meta(), ts1)
            .await;
        mem.record_turn_with_timestamp("c1", "s2", "user", "DeFi staking", meta(), ts2)
            .await;

        let profile = mem.build_profile("c1").await.unwrap();
        assert_eq!(profile.customer_id, "c1");
        assert_eq!(profile.total_sessions, 2);
        assert_eq!(profile.total_turns, 2);
        assert_eq!(profile.first_interaction, ts1);
        assert_eq!(profile.last_interaction, ts2);
        assert!(!profile.topics.is_empty());
    }

    #[tokio::test]
    async fn test_build_profile_unknown() {
        let mem = ConversationMemory::new();
        assert!(mem.build_profile("unknown").await.is_none());
    }

    #[tokio::test]
    async fn test_profile_sentiment_positive() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "Thanks, that was great!", meta())
            .await;
        mem.record_turn("c1", "s1", "user", "Excellent service, love it", meta())
            .await;

        let profile = mem.build_profile("c1").await.unwrap();
        assert_eq!(profile.sentiment_trend, "positive");
    }

    #[tokio::test]
    async fn test_profile_sentiment_negative() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "This is terrible and broken", meta())
            .await;
        mem.record_turn("c1", "s1", "user", "Awful experience, horrible bug", meta())
            .await;

        let profile = mem.build_profile("c1").await.unwrap();
        assert_eq!(profile.sentiment_trend, "negative");
    }

    #[tokio::test]
    async fn test_profile_sentiment_from_metadata() {
        let mem = ConversationMemory::new();
        mem.record_turn(
            "c1",
            "s1",
            "user",
            "neutral text",
            meta_with("sentiment", "positive"),
        )
        .await;
        mem.record_turn(
            "c1",
            "s1",
            "user",
            "more text",
            meta_with("sentiment", "positive"),
        )
        .await;

        let profile = mem.build_profile("c1").await.unwrap();
        assert_eq!(profile.sentiment_trend, "positive");
    }

    // -----------------------------------------------------------------------
    // ConversationSummarizer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_summarizer_build_context() {
        let mem = ConversationMemory::new();
        let ts = Utc.with_ymd_and_hms(2025, 6, 1, 12, 0, 0).unwrap();
        mem.record_turn_with_timestamp("c1", "s1", "user", "How do I stake ETH?", meta(), ts)
            .await;
        mem.record_turn_with_timestamp(
            "c1",
            "s1",
            "assistant",
            "You can stake ETH through a validator.",
            meta(),
            ts,
        )
        .await;

        let ctx = ConversationSummarizer::build_context(&mem, "c1", 500).await;
        assert!(ctx.contains("[Customer History]"));
        assert!(ctx.contains("Customer: c1"));
        assert!(ctx.contains("Sessions: 1"));
        assert!(ctx.contains("stake"));
    }

    #[tokio::test]
    async fn test_summarizer_unknown_customer() {
        let mem = ConversationMemory::new();
        let ctx = ConversationSummarizer::build_context(&mem, "ghost", 500).await;
        assert!(ctx.contains("[Customer History]"));
        assert!(ctx.contains("No prior interactions"));
    }

    #[tokio::test]
    async fn test_summarizer_respects_token_limit() {
        let mem = ConversationMemory::new();
        for i in 0..20 {
            mem.record_turn(
                "c1",
                "s1",
                "user",
                &format!("This is a fairly long message number {i} about various topics"),
                meta(),
            )
            .await;
        }

        // Very small token budget
        let ctx = ConversationSummarizer::build_context(&mem, "c1", 100).await;
        // Should have some content but be truncated
        assert!(ctx.contains("[Customer History]"));
        // With 100 tokens * 4 = 400 chars, not all 20 turns should fit
        assert!(ctx.len() < 2000);
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_truncate_content_short() {
        assert_eq!(truncate_content("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let long = "a".repeat(100);
        let truncated = truncate_content(&long, 20);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= 20);
    }

    #[test]
    fn test_extract_topics_basic() {
        let turns = vec![ConversationTurn {
            customer_id: "c1".to_string(),
            session_id: "s1".to_string(),
            role: "user".to_string(),
            content: "billing billing billing staking staking DeFi".to_string(),
            timestamp: Utc::now(),
            metadata: meta(),
        }];
        let topics = extract_topics(&turns);
        assert!(topics.contains(&"billing".to_string()));
        assert!(topics.contains(&"staking".to_string()));
        assert!(topics.contains(&"defi".to_string()));
    }

    #[test]
    fn test_extract_topics_filters_stopwords() {
        let turns = vec![ConversationTurn {
            customer_id: "c1".to_string(),
            session_id: "s1".to_string(),
            role: "user".to_string(),
            content: "the a is are were this that blockchain".to_string(),
            timestamp: Utc::now(),
            metadata: meta(),
        }];
        let topics = extract_topics(&turns);
        // Stopwords should be filtered; "blockchain" should remain
        assert!(topics.contains(&"blockchain".to_string()));
        assert!(!topics.contains(&"the".to_string()));
    }

    #[test]
    fn test_infer_sentiment_neutral() {
        let turns = vec![ConversationTurn {
            customer_id: "c1".to_string(),
            session_id: "s1".to_string(),
            role: "user".to_string(),
            content: "What time does the store open?".to_string(),
            timestamp: Utc::now(),
            metadata: meta(),
        }];
        assert_eq!(infer_sentiment(&turns), "neutral");
    }

    // -----------------------------------------------------------------------
    // Thread safety / multi-customer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_multiple_customers_isolated() {
        let mem = ConversationMemory::new();
        mem.record_turn("c1", "s1", "user", "Hello from c1", meta())
            .await;
        mem.record_turn("c2", "s2", "user", "Hello from c2", meta())
            .await;

        assert_eq!(mem.customer_count().await, 2);
        assert_eq!(mem.turn_count("c1").await, 1);
        assert_eq!(mem.turn_count("c2").await, 1);

        let ctx_c1 = mem.get_context("c1", 10).await;
        assert_eq!(ctx_c1.len(), 1);
        assert_eq!(ctx_c1[0].content, "Hello from c1");
    }

    #[tokio::test]
    async fn test_default_constructor() {
        let mem = ConversationMemory::default();
        assert_eq!(mem.customer_count().await, 0);
    }

    #[tokio::test]
    async fn test_customer_profile_serialization() {
        let profile = CustomerProfile {
            customer_id: "c1".to_string(),
            total_sessions: 3,
            total_turns: 15,
            first_interaction: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            last_interaction: Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap(),
            topics: vec!["billing".to_string(), "staking".to_string()],
            sentiment_trend: "positive".to_string(),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let restored: CustomerProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.total_sessions, 3);
        assert_eq!(restored.topics.len(), 2);
    }
}

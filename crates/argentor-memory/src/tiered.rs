// SPDX-License-Identifier: AGPL-3.0-only
//! Multi-tier memory system: short-term (working), long-term (episodic), and entity memory.
//!
//! # Tiers
//!
//! - **Short-term**: Rolling window of the last N turns. In-memory, zero latency.
//! - **Long-term**: Evicted short-term turns are summarised and persisted in a [`VectorStore`].
//!   When a new query arrives, semantically relevant episodes are retrieved (cosine similarity).
//! - **Entity**: Named entities (capitalised nouns, @mentions, quoted terms) are extracted and
//!   their facts accumulated. Entities mentioned in a new turn trigger fact injection.

use crate::{
    embedding::{EmbeddingProvider, LocalEmbedding},
    store::{MemoryEntry, SearchResult, VectorStore},
};
use argentor_core::{ArgentorError, ArgentorResult};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`TieredMemory`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredMemoryConfig {
    /// Number of turns kept in the short-term (working memory) window.
    pub short_term_window: usize,
    /// Minimum cosine similarity for a long-term episode to be included in context.
    pub long_term_threshold: f32,
    /// Whether to extract entities from each turn.
    pub entity_extraction: bool,
    /// Whether to summarise evicted turns before storing them in long-term memory.
    pub summarize_on_evict: bool,
    /// How many long-term results to retrieve per query.
    pub long_term_top_k: usize,
}

impl Default for TieredMemoryConfig {
    fn default() -> Self {
        Self {
            short_term_window: 20,
            long_term_threshold: 0.7,
            entity_extraction: true,
            summarize_on_evict: true,
            long_term_top_k: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single turn stored in memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredTurn {
    /// Speaker role: "user" | "assistant" | "tool".
    pub role: String,
    /// Raw content of the turn.
    pub content: String,
    /// UTC timestamp.
    pub timestamp: chrono::DateTime<Utc>,
}

/// A long-term memory item paired with a retrieval score.
#[derive(Debug, Clone)]
pub struct ScoredMemory {
    /// The underlying vector-store entry.
    pub entry: MemoryEntry,
    /// Cosine similarity to the current query (0.0–1.0).
    pub score: f32,
}

/// Assembled context passed back to the agent for prompt injection.
#[derive(Debug, Clone)]
pub struct MemoryContext {
    /// Recent turns still in the short-term window.
    pub short_term: Vec<TieredTurn>,
    /// Semantically relevant long-term episodes.
    pub relevant_long_term: Vec<ScoredMemory>,
    /// Facts associated with entities detected in the current query.
    pub entity_facts: Vec<String>,
    /// Rough token estimate (1 token ≈ 4 chars).
    pub total_tokens_estimate: usize,
}

// ---------------------------------------------------------------------------
// Serialisable snapshot for persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct TieredMemorySnapshot {
    short_term: Vec<TieredTurn>,
    entities: HashMap<String, Vec<String>>,
    config: TieredMemoryConfig,
}

// ---------------------------------------------------------------------------
// Entity pattern extractor
// ---------------------------------------------------------------------------

/// Pre-compiled regexes for entity extraction.
struct EntityPatterns {
    capitalized: Regex,
    at_mention: Regex,
    quoted: Regex,
}

impl EntityPatterns {
    fn new() -> Self {
        Self {
            // Words starting with uppercase followed by at least 2 lowercase letters
            capitalized: Regex::new(r"\b([A-Z][a-z]{2,})\b").unwrap(),
            at_mention: Regex::new(r"@([A-Za-z][A-Za-z0-9_]{1,})").unwrap(),
            quoted: Regex::new(r#""([^"]{2,32})""#).unwrap(),
        }
    }

    /// Extract entity names from text (deduped).
    fn extract(&self, text: &str) -> Vec<String> {
        let mut entities: Vec<String> = Vec::new();

        for cap in self.capitalized.captures_iter(text) {
            entities.push(cap[1].to_string());
        }
        for cap in self.at_mention.captures_iter(text) {
            entities.push(cap[1].to_string());
        }
        for cap in self.quoted.captures_iter(text) {
            entities.push(cap[1].to_string());
        }

        entities.dedup();
        entities
    }
}

// ---------------------------------------------------------------------------
// TieredMemory
// ---------------------------------------------------------------------------

/// Multi-tier memory combining short-term working memory, long-term episodic
/// storage, and entity fact tracking.
pub struct TieredMemory {
    short_term: VecDeque<TieredTurn>,
    /// Turns evicted from short-term by the synchronous `add_turn` path;
    /// flushed to long-term by `flush_evicted`.
    pending_evictions: Vec<TieredTurn>,
    long_term: Arc<dyn VectorStore>,
    entities: HashMap<String, Vec<String>>,
    config: TieredMemoryConfig,
    embedder: Arc<dyn EmbeddingProvider>,
    entity_patterns: EntityPatterns,
}

impl TieredMemory {
    /// Create a new [`TieredMemory`] with the given config and vector store backend.
    pub fn new(config: TieredMemoryConfig, store: Arc<dyn VectorStore>) -> Self {
        Self::with_embedder(config, store, Arc::new(LocalEmbedding::default()))
    }

    /// Create a [`TieredMemory`] with an explicit embedding provider (useful for testing).
    pub fn with_embedder(
        config: TieredMemoryConfig,
        store: Arc<dyn VectorStore>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            short_term: VecDeque::with_capacity(config.short_term_window + 1),
            pending_evictions: Vec::new(),
            long_term: store,
            entities: HashMap::new(),
            config,
            embedder,
            entity_patterns: EntityPatterns::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Add a conversation turn (synchronous path).
    ///
    /// Evicted turns are queued in `pending_evictions`. Call [`Self::flush_evicted`]
    /// afterwards to persist them, or prefer [`Self::add_turn_async`] in async contexts.
    pub fn add_turn(&mut self, role: &str, content: &str) {
        if self.config.entity_extraction {
            self.update_entities(role, content);
        }

        if self.short_term.len() >= self.config.short_term_window {
            if let Some(evicted) = self.short_term.pop_front() {
                if self.config.summarize_on_evict {
                    self.pending_evictions.push(evicted);
                }
            }
        }

        self.short_term.push_back(TieredTurn {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
    }

    /// Flush pending evictions (from synchronous `add_turn`) to long-term storage.
    pub async fn flush_evicted(&mut self) -> ArgentorResult<()> {
        let pending = std::mem::take(&mut self.pending_evictions);
        for turn in pending {
            self.store_to_long_term(&turn).await?;
        }
        Ok(())
    }

    /// Add a turn and immediately persist any evicted turn to long-term storage.
    ///
    /// Prefer this in async contexts; it avoids the two-step sync/flush pattern.
    pub async fn add_turn_async(&mut self, role: &str, content: &str) -> ArgentorResult<()> {
        if self.config.entity_extraction {
            self.update_entities(role, content);
        }

        if self.short_term.len() >= self.config.short_term_window {
            if let Some(evicted) = self.short_term.pop_front() {
                if self.config.summarize_on_evict {
                    self.store_to_long_term(&evicted).await?;
                }
            }
        }

        self.short_term.push_back(TieredTurn {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        Ok(())
    }

    /// Retrieve assembled context for the given query string.
    ///
    /// - **short_term**: all turns currently in the window (oldest → newest).
    /// - **relevant_long_term**: long-term episodes with cosine similarity ≥ threshold.
    /// - **entity_facts**: facts for entities found in `current_query`.
    pub async fn get_context(&self, current_query: &str) -> ArgentorResult<MemoryContext> {
        let short_term: Vec<TieredTurn> = self.short_term.iter().cloned().collect();

        // Long-term retrieval
        let relevant_long_term = if !current_query.is_empty() {
            let embedding = self.embedder.embed(current_query).await?;
            let results = self
                .long_term
                .search(&embedding, self.config.long_term_top_k, None)
                .await?;
            results
                .into_iter()
                .filter(|r| r.score >= self.config.long_term_threshold)
                .map(|SearchResult { entry, score }| ScoredMemory { entry, score })
                .collect()
        } else {
            Vec::new()
        };

        // Entity fact injection
        let detected = self.entity_patterns.extract(current_query);
        let mut entity_facts: Vec<String> = Vec::new();
        for entity in &detected {
            if let Some(facts) = self.entities.get(entity.as_str()) {
                for fact in facts {
                    entity_facts.push(format!("[{entity}] {fact}"));
                }
            }
        }

        // Rough token estimate (1 token ≈ 4 chars)
        let char_total: usize = short_term.iter().map(|t| t.content.len()).sum::<usize>()
            + relevant_long_term
                .iter()
                .map(|m| m.entry.content.len())
                .sum::<usize>()
            + entity_facts.iter().map(|f| f.len()).sum::<usize>();
        let total_tokens_estimate = char_total / 4;

        Ok(MemoryContext {
            short_term,
            relevant_long_term,
            entity_facts,
            total_tokens_estimate,
        })
    }

    /// Return a reference to the entity fact map.
    pub fn get_entities(&self) -> &HashMap<String, Vec<String>> {
        &self.entities
    }

    /// Return the number of turns currently in short-term memory.
    pub fn short_term_len(&self) -> usize {
        self.short_term.len()
    }

    /// Return the number of distinct entities tracked.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Persist the short-term buffer and entity map to `path` as JSON.
    pub async fn persist(&self, path: &Path) -> ArgentorResult<()> {
        let snapshot = TieredMemorySnapshot {
            short_term: self.short_term.iter().cloned().collect(),
            entities: self.entities.clone(),
            config: self.config.clone(),
        };
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| ArgentorError::Session(format!("Failed to serialize snapshot: {e}")))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ArgentorError::Session(format!("Failed to create dir: {e}")))?;
        }
        tokio::fs::write(path, json.as_bytes())
            .await
            .map_err(|e| ArgentorError::Session(format!("Failed to write snapshot: {e}")))?;
        Ok(())
    }

    /// Load short-term and entity state from a snapshot file.
    /// The long-term [`VectorStore`] must be provided separately (already loaded).
    pub async fn load(path: &Path, store: Arc<dyn VectorStore>) -> ArgentorResult<Self> {
        let data = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ArgentorError::Session(format!("Failed to read snapshot: {e}")))?;
        let snapshot: TieredMemorySnapshot = serde_json::from_str(&data)
            .map_err(|e| ArgentorError::Session(format!("Failed to parse snapshot: {e}")))?;

        let mut mem = Self::new(snapshot.config, store);
        for turn in snapshot.short_term {
            mem.short_term.push_back(turn);
        }
        mem.entities = snapshot.entities;
        Ok(mem)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Embed and persist an evicted turn into the long-term store.
    async fn store_to_long_term(&self, turn: &TieredTurn) -> ArgentorResult<()> {
        let text = format!(
            "[{}] {}: {}",
            turn.timestamp.format("%Y-%m-%dT%H:%M"),
            turn.role,
            &turn.content[..turn.content.len().min(500)],
        );

        let embedding = self.embedder.embed(&text).await?;
        let entry = MemoryEntry {
            id: Uuid::new_v4(),
            content: text,
            embedding,
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "role".to_string(),
                    serde_json::Value::String(turn.role.clone()),
                );
                m.insert(
                    "tier".to_string(),
                    serde_json::Value::String("long_term".to_string()),
                );
                m
            },
            session_id: None,
            created_at: turn.timestamp,
        };
        self.long_term.insert(entry).await
    }

    /// Extract entities from `content` and update the entity→facts map.
    ///
    /// Tool turns are skipped (machine-generated noise). Each entity accumulates
    /// up to 10 facts to prevent unbounded growth.
    fn update_entities(&mut self, role: &str, content: &str) {
        if role == "tool" {
            return;
        }
        let entities = self.entity_patterns.extract(content);
        if entities.is_empty() {
            return;
        }
        let fact = format!("[{}] {}", role, &content[..content.len().min(200)]);
        for entity in entities {
            let facts = self.entities.entry(entity).or_default();
            if facts.len() < 10 {
                facts.push(fact.clone());
            }
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
    use crate::store::InMemoryVectorStore;

    fn make_store() -> Arc<dyn VectorStore> {
        Arc::new(InMemoryVectorStore::new())
    }

    fn make_mem(window: usize) -> TieredMemory {
        let config = TieredMemoryConfig {
            short_term_window: window,
            long_term_threshold: 0.5,
            entity_extraction: true,
            summarize_on_evict: true,
            long_term_top_k: 5,
        };
        TieredMemory::new(config, make_store())
    }

    // -----------------------------------------------------------------------
    // Short-term tier
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_short_term_window_enforced() {
        let mut mem = make_mem(20);
        for i in 0..25 {
            mem.add_turn_async("user", &format!("turn {i}"))
                .await
                .unwrap();
        }
        assert_eq!(mem.short_term_len(), 20, "window must cap at 20");
    }

    #[tokio::test]
    async fn test_short_term_retains_latest() {
        let mut mem = make_mem(3);
        mem.add_turn_async("user", "first").await.unwrap();
        mem.add_turn_async("user", "second").await.unwrap();
        mem.add_turn_async("user", "third").await.unwrap();
        mem.add_turn_async("user", "fourth").await.unwrap(); // evicts "first"

        let st: Vec<_> = mem.short_term.iter().map(|t| t.content.as_str()).collect();
        assert!(!st.contains(&"first"), "oldest must be evicted");
        assert!(st.contains(&"fourth"), "newest must be present");
    }

    #[tokio::test]
    async fn test_short_term_order_preserved() {
        let mut mem = make_mem(10);
        for i in 0..5 {
            mem.add_turn_async("user", &format!("msg{i}"))
                .await
                .unwrap();
        }
        let ctx = mem.get_context("anything").await.unwrap();
        assert_eq!(ctx.short_term[0].content, "msg0");
        assert_eq!(ctx.short_term[4].content, "msg4");
    }

    // -----------------------------------------------------------------------
    // Long-term tier
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_evicted_turns_reach_long_term() {
        let mut mem = make_mem(3);
        mem.add_turn_async("user", "alpha rust programming")
            .await
            .unwrap();
        mem.add_turn_async("user", "beta topic").await.unwrap();
        mem.add_turn_async("user", "gamma topic").await.unwrap();
        mem.add_turn_async("user", "delta topic").await.unwrap(); // evicts alpha

        let count = mem.long_term.count().await.unwrap();
        assert_eq!(count, 1, "one evicted turn must land in long-term store");
    }

    #[tokio::test]
    async fn test_long_term_retrieved_by_query() {
        let mut mem = make_mem(2);
        mem.add_turn_async("user", "rust programming language systems")
            .await
            .unwrap();
        mem.add_turn_async("user", "cooking recipes dinner")
            .await
            .unwrap();
        mem.add_turn_async("user", "another unrelated turn")
            .await
            .unwrap(); // evicts rust

        let ctx = mem.get_context("rust systems programming").await.unwrap();
        assert!(
            !ctx.relevant_long_term.is_empty(),
            "should retrieve relevant long-term episode"
        );
    }

    #[tokio::test]
    async fn test_long_term_threshold_filters_irrelevant() {
        let store = make_store();
        let config = TieredMemoryConfig {
            short_term_window: 2,
            long_term_threshold: 0.99, // extremely strict threshold
            entity_extraction: false,
            summarize_on_evict: true,
            long_term_top_k: 5,
        };
        let mut mem = TieredMemory::new(config, store);
        mem.add_turn_async("user", "cooking is great")
            .await
            .unwrap();
        mem.add_turn_async("user", "baking bread").await.unwrap();
        mem.add_turn_async("user", "dessert cake").await.unwrap(); // evicts cooking

        let ctx = mem.get_context("rust programming").await.unwrap();
        assert!(
            ctx.relevant_long_term.is_empty(),
            "threshold 0.99 should filter unrelated episode"
        );
    }

    // -----------------------------------------------------------------------
    // Entity tier
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_entity_facts_stored() {
        let mut mem = make_mem(20);
        mem.add_turn_async("user", "John is the lead developer")
            .await
            .unwrap();
        mem.add_turn_async("assistant", "John works on the backend")
            .await
            .unwrap();

        let entities = mem.get_entities();
        assert!(entities.contains_key("John"), "John must be tracked");
        assert!(!entities["John"].is_empty(), "at least one fact for John");
    }

    #[tokio::test]
    async fn test_entity_facts_injected_in_context() {
        let mut mem = make_mem(20);
        mem.add_turn_async("user", "Alice manages the project")
            .await
            .unwrap();

        let ctx = mem.get_context("what does Alice do?").await.unwrap();
        assert!(
            ctx.entity_facts.iter().any(|f| f.contains("Alice")),
            "Alice facts must appear in context"
        );
    }

    #[tokio::test]
    async fn test_entity_at_mention() {
        let mut mem = make_mem(20);
        mem.add_turn_async("user", "ping @backend team please")
            .await
            .unwrap();

        assert!(
            mem.get_entities().contains_key("backend"),
            "@mention must extract entity"
        );
    }

    #[tokio::test]
    async fn test_entity_quoted_term() {
        let mut mem = make_mem(20);
        mem.add_turn_async("user", r#"the "auth module" is broken"#)
            .await
            .unwrap();

        assert!(
            mem.get_entities().contains_key("auth module"),
            "quoted entity must be tracked"
        );
    }

    #[tokio::test]
    async fn test_entity_tool_role_skipped() {
        let mut mem = make_mem(20);
        mem.add_turn_async("tool", "Output from John's processing")
            .await
            .unwrap();

        assert!(
            !mem.get_entities().contains_key("John"),
            "tool turns must not contribute entity facts"
        );
    }

    // -----------------------------------------------------------------------
    // Persistence round-trip
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_persist_and_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let snap_path = tmp.path().join("tiered.json");

        let store: Arc<dyn VectorStore> = make_store();
        let mut mem = TieredMemory::new(TieredMemoryConfig::default(), store.clone());
        mem.add_turn_async("user", "hello world").await.unwrap();
        mem.add_turn_async("assistant", "hi there").await.unwrap();
        mem.persist(&snap_path).await.unwrap();

        let loaded = TieredMemory::load(&snap_path, store).await.unwrap();
        assert_eq!(loaded.short_term_len(), 2, "turns survive round-trip");
    }

    #[tokio::test]
    async fn test_persist_entities_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let snap_path = tmp.path().join("tiered_ent.json");

        let store: Arc<dyn VectorStore> = make_store();
        let mut mem = TieredMemory::new(TieredMemoryConfig::default(), store.clone());
        mem.add_turn_async("user", "Maria leads the team")
            .await
            .unwrap();
        mem.persist(&snap_path).await.unwrap();

        let loaded = TieredMemory::load(&snap_path, store).await.unwrap();
        assert!(
            loaded.get_entities().contains_key("Maria"),
            "entities survive round-trip"
        );
    }

    // -----------------------------------------------------------------------
    // Config behaviour
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_entity_extraction_disabled() {
        let store = make_store();
        let config = TieredMemoryConfig {
            entity_extraction: false,
            ..Default::default()
        };
        let mut mem = TieredMemory::new(config, store);
        mem.add_turn_async("user", "Alice and Bob discussed Rust")
            .await
            .unwrap();
        assert!(
            mem.get_entities().is_empty(),
            "entities must be empty when extraction is disabled"
        );
    }

    #[tokio::test]
    async fn test_no_summarize_on_evict() {
        let store = make_store();
        let config = TieredMemoryConfig {
            short_term_window: 2,
            summarize_on_evict: false,
            entity_extraction: false,
            long_term_threshold: 0.5,
            long_term_top_k: 5,
        };
        let mut mem = TieredMemory::new(config, store);
        mem.add_turn_async("user", "first").await.unwrap();
        mem.add_turn_async("user", "second").await.unwrap();
        mem.add_turn_async("user", "third").await.unwrap();

        let count = mem.long_term.count().await.unwrap();
        assert_eq!(
            count, 0,
            "no long-term writes when summarize_on_evict=false"
        );
    }

    #[tokio::test]
    async fn test_sync_flush_evicted() {
        let mut mem = make_mem(2);
        mem.add_turn("user", "first");
        mem.add_turn("user", "second");
        mem.add_turn("user", "third"); // evicts first → pending

        // nothing in long-term yet
        let before = mem.long_term.count().await.unwrap();
        assert_eq!(before, 0);

        mem.flush_evicted().await.unwrap();

        let after = mem.long_term.count().await.unwrap();
        assert_eq!(after, 1, "flushed eviction must reach long-term store");
    }

    // -----------------------------------------------------------------------
    // Token estimate
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_token_estimate_non_zero() {
        let mut mem = make_mem(20);
        mem.add_turn_async("user", "hello this is a test message for token estimate")
            .await
            .unwrap();
        let ctx = mem.get_context("test").await.unwrap();
        assert!(ctx.total_tokens_estimate > 0);
    }
}

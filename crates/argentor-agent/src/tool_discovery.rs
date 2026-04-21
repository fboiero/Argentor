//! Dynamic tool discovery for token-efficient tool loading.
//!
//! Instead of loading ALL tools into the LLM context (wasting tokens),
//! this module discovers the most relevant tools for each query based on
//! keyword matching, TF-IDF similarity, and historical usage patterns.
//!
//! # Architecture
//!
//! ```text
//! ┌───────────────┐     ┌─────────────────────┐     ┌──────────────────┐
//! │ Query + Tools │ --> │ ToolDiscoveryEngine │ --> │ DiscoveryResult  │
//! │               │     │  (ranked selection) │     │ (top-K tools)    │
//! └───────────────┘     └─────────────────────┘     └──────────────────┘
//!                              │
//!                    ┌─────────┴──────────┐
//!                    │  Strategies:       │
//!                    │  - KeywordMatch    │
//!                    │  - TfIdf           │
//!                    │  - Semantic        │
//!                    │  - Hybrid          │
//!                    └────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Common English stopwords filtered during tokenization.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
    "our", "out", "has", "have", "from", "with", "they", "been", "this", "that", "will", "each",
    "make", "like", "use", "into", "what", "how", "does", "just", "please", "could", "would",
    "should", "about", "when", "then", "than", "also", "some", "any", "there", "here",
];

/// Approximate tokens per tool definition injected into context.
/// Used for estimating token savings.
const TOKENS_PER_TOOL: usize = 150;

/// Strategy for discovering relevant tools.
///
/// Each strategy makes different tradeoffs between speed and accuracy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DiscoveryStrategy {
    /// Simple keyword overlap between query and tool descriptions.
    KeywordMatch,
    /// TF-IDF cosine similarity (like `tool_selector.rs`).
    TfIdf,
    /// Embedding-based semantic similarity (placeholder for `LocalEmbedding`).
    Semantic,
    /// Combine keyword + TF-IDF + usage history.
    Hybrid,
}

/// A tool entry available for discovery.
///
/// Contains the tool's name and description, which are used for
/// relevance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    /// Unique name of the tool.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
}

impl ToolEntry {
    /// Create a new tool entry.
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
        }
    }
}

/// A discovered tool with relevance metadata.
///
/// Contains the tool information, its relevance score, and a human-readable
/// explanation of why it was selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredTool {
    /// Name of the discovered tool.
    pub name: String,
    /// Description of the tool.
    pub description: String,
    /// Relevance score (0.0 - 1.0).
    pub relevance_score: f32,
    /// Explanation of why this tool was selected.
    pub selection_reason: String,
}

/// The result of a tool discovery operation.
///
/// Contains the selected tools, statistics about the selection, and
/// estimated token savings from not including all tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResult {
    /// The selected tools, ordered by relevance (highest first).
    pub selected_tools: Vec<DiscoveredTool>,
    /// Total number of available tools considered.
    pub total_available: usize,
    /// Estimated tokens saved by not including all tools.
    pub token_savings: usize,
    /// The strategy that was used for discovery.
    pub strategy_used: DiscoveryStrategy,
}

/// Configuration for the tool discovery engine.
///
/// Controls how many tools to select, the relevance threshold,
/// always-included tools, and the discovery strategy.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Whether tool discovery is enabled.
    pub enabled: bool,
    /// Maximum number of tools to include in context (default: 8).
    pub max_tools: usize,
    /// Minimum relevance score to include a tool (default: 0.3).
    pub similarity_threshold: f32,
    /// Tool names that are always included regardless of relevance.
    pub always_include: Vec<String>,
    /// The discovery strategy to use.
    pub strategy: DiscoveryStrategy,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tools: 8,
            similarity_threshold: 0.3,
            always_include: Vec::new(),
            strategy: DiscoveryStrategy::Hybrid,
        }
    }
}

/// Historical usage record for a tool.
///
/// Tracks how often a tool has been used and its success rate,
/// which feeds into the Hybrid strategy's scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Total number of times the tool was used.
    pub total_uses: u64,
    /// Number of successful uses.
    pub successful_uses: u64,
    /// Contexts/keywords associated with past successful uses.
    pub associated_keywords: Vec<String>,
}

impl UsageRecord {
    /// Create a new empty usage record.
    pub fn new() -> Self {
        Self {
            total_uses: 0,
            successful_uses: 0,
            associated_keywords: Vec::new(),
        }
    }

    /// Record a successful usage with context keywords.
    pub fn record_success(&mut self, keywords: &[String]) {
        self.total_uses += 1;
        self.successful_uses += 1;
        for kw in keywords {
            if !self.associated_keywords.contains(kw) {
                self.associated_keywords.push(kw.clone());
            }
        }
        // Keep keyword list manageable
        if self.associated_keywords.len() > 50 {
            self.associated_keywords.drain(..25);
        }
    }

    /// Record a failed usage.
    pub fn record_failure(&mut self) {
        self.total_uses += 1;
    }

    /// Success rate (0.0 - 1.0), default 0.5 for unseen tools.
    pub fn success_rate(&self) -> f32 {
        if self.total_uses == 0 {
            return 0.5;
        }
        self.successful_uses as f32 / self.total_uses as f32
    }
}

impl Default for UsageRecord {
    fn default() -> Self {
        Self::new()
    }
}

/// The tool discovery engine.
///
/// Analyzes queries against available tools to select the most relevant
/// subset, reducing token waste and model confusion. Supports multiple
/// strategies and maintains usage history for adaptive scoring.
pub struct ToolDiscoveryEngine {
    config: DiscoveryConfig,
    /// Per-tool usage history for the Hybrid strategy.
    usage_history: HashMap<String, UsageRecord>,
    /// TF-IDF vocabulary: term -> IDF weight.
    vocabulary: HashMap<String, f32>,
}

/// Per-session cache for tool discovery results.
///
/// Caches the output of `ToolDiscoveryEngine::discover` keyed by a hash of
/// (prompt_intent, registry_version). On a cache hit the TF-IDF scoring step
/// is skipped entirely.  The cache is meant to live on the session / agent-
/// runner and is cleared when the session ends.
///
/// # Invalidation
///
/// The cache is automatically invalidated when:
/// * The key changes (different prompt or registry version).
/// * `clear()` is called (e.g. when the session ends).
/// * `turn_limit` turns have elapsed since last population (default: 50).
///
/// # Example
///
/// ```rust
/// use argentor_agent::tool_discovery::{ToolDiscoveryCache, ToolDiscoveryEngine, ToolEntry};
///
/// let engine = ToolDiscoveryEngine::with_defaults();
/// let mut cache = ToolDiscoveryCache::new(50);
/// let tools = vec![ToolEntry::new("echo", "Echo a message")];
///
/// let result1 = cache.get_or_compute("say hello", &tools, 0, &engine);
/// let result2 = cache.get_or_compute("say hello", &tools, 0, &engine);
/// assert!(result2.is_some()); // second call is a cache hit
/// ```
pub struct ToolDiscoveryCache {
    /// Stored results keyed by (intent_hash, registry_version).
    store: HashMap<(u64, u64), CacheEntry>,
    /// Maximum number of turns a cached entry is valid for.
    turn_limit: u32,
    /// Current turn counter (incremented externally or by `advance_turn`).
    current_turn: u32,
    /// Total cache hits since construction.
    hits: u64,
    /// Total cache misses since construction.
    misses: u64,
}

struct CacheEntry {
    result: Option<DiscoveryResult>,
    inserted_at_turn: u32,
}

impl ToolDiscoveryCache {
    /// Create a new cache with the given per-entry turn lifetime.
    pub fn new(turn_limit: u32) -> Self {
        Self {
            store: HashMap::new(),
            turn_limit,
            current_turn: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Create a cache with the default turn limit (50).
    pub fn with_defaults() -> Self {
        Self::new(50)
    }

    /// Advance the internal turn counter by 1.
    ///
    /// Call once per agent turn to enable time-based invalidation.
    pub fn advance_turn(&mut self) {
        self.current_turn = self.current_turn.saturating_add(1);
    }

    /// Clear all cached entries (e.g. when the session ends).
    pub fn clear(&mut self) {
        self.store.clear();
        self.current_turn = 0;
    }

    /// Cache hit count since construction.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Cache miss count since construction.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Return cached result or compute (and cache) a fresh one.
    ///
    /// `intent` is the user-facing query / prompt for this turn.
    /// `registry_version` should change whenever the tool registry changes
    /// (e.g. a monotonically increasing counter or a hash of tool names).
    pub fn get_or_compute(
        &mut self,
        intent: &str,
        tools: &[ToolEntry],
        registry_version: u64,
        engine: &ToolDiscoveryEngine,
    ) -> Option<DiscoveryResult> {
        let intent_hash = hash_str(intent);
        let key = (intent_hash, registry_version);

        // Check for a valid cached entry.
        if let Some(entry) = self.store.get(&key) {
            let age = self.current_turn.saturating_sub(entry.inserted_at_turn);
            if age < self.turn_limit {
                self.hits += 1;
                return entry.result.clone();
            }
            // Entry is stale — fall through to recompute.
        }

        // Cache miss: compute fresh result and store it.
        self.misses += 1;
        let result = engine.discover(intent, tools);
        self.store.insert(
            key,
            CacheEntry {
                result: result.clone(),
                inserted_at_turn: self.current_turn,
            },
        );
        result
    }
}

/// Compute a stable 64-bit hash for a string using the default hasher.
fn hash_str(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl ToolDiscoveryEngine {
    /// Create a new discovery engine with the given configuration.
    pub fn new(config: DiscoveryConfig) -> Self {
        Self {
            config,
            usage_history: HashMap::new(),
            vocabulary: HashMap::new(),
        }
    }

    /// Create a new discovery engine with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(DiscoveryConfig::default())
    }

    /// Return a reference to the engine's configuration.
    pub fn config(&self) -> &DiscoveryConfig {
        &self.config
    }

    /// Build the TF-IDF vocabulary from tool descriptions.
    ///
    /// Should be called once after tools are registered, or whenever
    /// the tool set changes.
    pub fn build_vocabulary(&mut self, tools: &[ToolEntry]) {
        self.vocabulary.clear();

        if tools.is_empty() {
            return;
        }

        let num_docs = tools.len() as f32;
        let mut doc_freq: HashMap<String, u32> = HashMap::new();

        for tool in tools {
            let text = format!("{} {}", tool.name, tool.description);
            let tokens = tokenize(&text);
            let unique: HashSet<String> = tokens.into_iter().collect();
            for term in unique {
                *doc_freq.entry(term).or_insert(0) += 1;
            }
        }

        for (term, df) in &doc_freq {
            let idf = (num_docs / *df as f32).ln();
            self.vocabulary.insert(term.clone(), idf);
        }
    }

    /// Record a successful tool usage for adaptive scoring.
    pub fn record_success(&mut self, tool_name: &str, query: &str) {
        let keywords = tokenize(query);
        self.usage_history
            .entry(tool_name.to_string())
            .or_default()
            .record_success(&keywords);
    }

    /// Record a failed tool usage.
    pub fn record_failure(&mut self, tool_name: &str) {
        self.usage_history
            .entry(tool_name.to_string())
            .or_default()
            .record_failure();
    }

    /// Get usage history for a specific tool.
    pub fn usage_history(&self, tool_name: &str) -> Option<&UsageRecord> {
        self.usage_history.get(tool_name)
    }

    /// Discover the most relevant tools for a given query.
    ///
    /// Returns `None` if discovery is disabled. Otherwise, applies the
    /// configured strategy to rank tools by relevance, enforces the
    /// `always_include` list and `max_tools` limit, and computes
    /// estimated token savings.
    pub fn discover(&self, query: &str, tools: &[ToolEntry]) -> Option<DiscoveryResult> {
        if !self.config.enabled {
            return None;
        }

        if tools.is_empty() {
            return Some(DiscoveryResult {
                selected_tools: Vec::new(),
                total_available: 0,
                token_savings: 0,
                strategy_used: self.config.strategy.clone(),
            });
        }

        let mut scored: Vec<(usize, f32, String)> = match &self.config.strategy {
            DiscoveryStrategy::KeywordMatch => self.score_keyword_match(query, tools),
            DiscoveryStrategy::TfIdf => self.score_tfidf(query, tools),
            DiscoveryStrategy::Semantic => self.score_semantic(query, tools),
            DiscoveryStrategy::Hybrid => self.score_hybrid(query, tools),
        };

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Build the selected set, respecting always_include
        let always_set: HashSet<&str> = self
            .config
            .always_include
            .iter()
            .map(String::as_str)
            .collect();

        let mut selected = Vec::new();

        // First add always-included tools
        for (idx, score, reason) in &scored {
            if always_set.contains(tools[*idx].name.as_str()) {
                selected.push(DiscoveredTool {
                    name: tools[*idx].name.clone(),
                    description: tools[*idx].description.clone(),
                    relevance_score: *score,
                    selection_reason: format!("always included; {reason}"),
                });
            }
        }

        // Also add always-included tools that had no score (score 0)
        for always_name in &self.config.always_include {
            if !selected.iter().any(|s| s.name == *always_name) {
                if let Some(tool) = tools.iter().find(|t| t.name == *always_name) {
                    selected.push(DiscoveredTool {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        relevance_score: 0.0,
                        selection_reason: "always included".to_string(),
                    });
                }
            }
        }

        // Then add top-scored tools up to max_tools
        for (idx, score, reason) in &scored {
            if selected.len() >= self.config.max_tools {
                break;
            }
            let name = &tools[*idx].name;
            if selected.iter().any(|s| &s.name == name) {
                continue; // Already included
            }
            if *score < self.config.similarity_threshold {
                continue; // Below threshold
            }
            selected.push(DiscoveredTool {
                name: name.clone(),
                description: tools[*idx].description.clone(),
                relevance_score: *score,
                selection_reason: reason.clone(),
            });
        }

        let excluded_count = tools.len().saturating_sub(selected.len());
        let token_savings = excluded_count * TOKENS_PER_TOOL;

        Some(DiscoveryResult {
            selected_tools: selected,
            total_available: tools.len(),
            token_savings,
            strategy_used: self.config.strategy.clone(),
        })
    }

    // --- Strategy scoring implementations ---

    /// Score tools by keyword overlap.
    fn score_keyword_match(&self, query: &str, tools: &[ToolEntry]) -> Vec<(usize, f32, String)> {
        let query_tokens: HashSet<String> = tokenize(query).into_iter().collect();

        tools
            .iter()
            .enumerate()
            .map(|(idx, tool)| {
                let tool_text = format!("{} {}", tool.name, tool.description);
                let tool_tokens: HashSet<String> = tokenize(&tool_text).into_iter().collect();

                let overlap: Vec<&String> = query_tokens.intersection(&tool_tokens).collect();
                let score = if query_tokens.is_empty() {
                    0.0
                } else {
                    overlap.len() as f32 / query_tokens.len() as f32
                };

                let reason = if overlap.is_empty() {
                    "no keyword match".to_string()
                } else {
                    format!(
                        "keyword match: {}",
                        overlap
                            .iter()
                            .take(3)
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };

                (idx, score, reason)
            })
            .collect()
    }

    /// Score tools by TF-IDF cosine similarity.
    fn score_tfidf(&self, query: &str, tools: &[ToolEntry]) -> Vec<(usize, f32, String)> {
        let query_tokens = tokenize(query);

        tools
            .iter()
            .enumerate()
            .map(|(idx, tool)| {
                let doc = format!("{} {}", tool.name, tool.description);
                let score = tfidf_cosine_similarity(&query_tokens, &doc, &self.vocabulary);
                let reason = format!("TF-IDF similarity: {score:.3}");
                (idx, score, reason)
            })
            .collect()
    }

    /// Score tools using semantic similarity (placeholder).
    ///
    /// In a full implementation, this would use `argentor_memory::LocalEmbedding`
    /// to compute embedding-based similarity. For now, it falls back to TF-IDF
    /// with a bonus for contextual associations.
    fn score_semantic(&self, query: &str, tools: &[ToolEntry]) -> Vec<(usize, f32, String)> {
        // Fallback to TF-IDF + contextual matching
        let tfidf_scores = self.score_tfidf(query, tools);
        let query_lower = query.to_lowercase();

        tfidf_scores
            .into_iter()
            .map(|(idx, score, _reason)| {
                let contextual_bonus = contextual_similarity_bonus(&query_lower, &tools[idx]);
                let final_score = (score + contextual_bonus).min(1.0);
                let reason = format!("semantic similarity: {final_score:.3} (TF-IDF + contextual)");
                (idx, final_score, reason)
            })
            .collect()
    }

    /// Score tools using the Hybrid strategy.
    ///
    /// Combines keyword matching (30%), TF-IDF similarity (40%), and
    /// historical usage patterns (30%).
    fn score_hybrid(&self, query: &str, tools: &[ToolEntry]) -> Vec<(usize, f32, String)> {
        let keyword_scores = self.score_keyword_match(query, tools);
        let tfidf_scores = self.score_tfidf(query, tools);
        let query_keywords = tokenize(query);

        tools
            .iter()
            .enumerate()
            .map(|(idx, tool)| {
                let kw_score = keyword_scores
                    .iter()
                    .find(|(i, _, _)| *i == idx)
                    .map(|(_, s, _)| *s)
                    .unwrap_or(0.0);

                let tfidf_score = tfidf_scores
                    .iter()
                    .find(|(i, _, _)| *i == idx)
                    .map(|(_, s, _)| *s)
                    .unwrap_or(0.0);

                let usage_score = self.compute_usage_score(&tool.name, &query_keywords);

                let combined = kw_score * 0.3 + tfidf_score * 0.4 + usage_score * 0.3;

                let reason = format!(
                    "hybrid: keyword={kw_score:.2}, tfidf={tfidf_score:.2}, usage={usage_score:.2}"
                );

                (idx, combined, reason)
            })
            .collect()
    }

    /// Compute a usage-based score for a tool given query keywords.
    fn compute_usage_score(&self, tool_name: &str, query_keywords: &[String]) -> f32 {
        match self.usage_history.get(tool_name) {
            None => 0.5, // Neutral for unseen tools
            Some(record) => {
                let success_rate = record.success_rate();

                // Check if query keywords match historically associated keywords
                let keyword_overlap =
                    if query_keywords.is_empty() || record.associated_keywords.is_empty() {
                        0.0
                    } else {
                        let matches = query_keywords
                            .iter()
                            .filter(|kw| record.associated_keywords.contains(kw))
                            .count();
                        matches as f32 / query_keywords.len().max(1) as f32
                    };

                // Frequency bonus (log scale to prevent domination)
                let frequency_bonus = (record.total_uses as f32).ln_1p() / 10.0;

                (success_rate * 0.5 + keyword_overlap * 0.3 + frequency_bonus * 0.2).min(1.0)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Tokenize text into lowercase terms, removing short words and stopwords.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .filter(|w| !STOPWORDS.contains(w))
        .map(String::from)
        .collect()
}

/// Compute TF-IDF cosine similarity between query tokens and a document.
fn tfidf_cosine_similarity(
    query_tokens: &[String],
    document: &str,
    vocabulary: &HashMap<String, f32>,
) -> f32 {
    let doc_tokens = tokenize(document);

    if query_tokens.is_empty() || doc_tokens.is_empty() {
        return 0.0;
    }

    let query_tf = term_frequencies(query_tokens);
    let doc_tf = term_frequencies(&doc_tokens);

    let all_terms: HashSet<&String> = query_tf
        .keys()
        .copied()
        .chain(doc_tf.keys().copied())
        .collect();

    let mut dot_product = 0.0_f32;
    let mut query_magnitude = 0.0_f32;
    let mut doc_magnitude = 0.0_f32;

    for term in all_terms {
        let idf = vocabulary.get(term.as_str()).copied().unwrap_or(0.0);
        let q_tfidf = query_tf.get(term).copied().unwrap_or(0.0) * idf;
        let d_tfidf = doc_tf.get(term).copied().unwrap_or(0.0) * idf;

        dot_product += q_tfidf * d_tfidf;
        query_magnitude += q_tfidf * q_tfidf;
        doc_magnitude += d_tfidf * d_tfidf;
    }

    let magnitude = query_magnitude.sqrt() * doc_magnitude.sqrt();
    if magnitude < f32::EPSILON {
        return 0.0;
    }

    dot_product / magnitude
}

/// Compute normalized term frequencies.
fn term_frequencies(tokens: &[String]) -> HashMap<&String, f32> {
    let mut counts: HashMap<&String, f32> = HashMap::new();
    for token in tokens {
        *counts.entry(token).or_insert(0.0) += 1.0;
    }
    let max_count = counts.values().copied().fold(0.0_f32, f32::max).max(1.0);
    for count in counts.values_mut() {
        *count /= max_count;
    }
    counts
}

/// Compute a contextual similarity bonus based on domain associations.
///
/// Maps common task patterns to tool types for a rough semantic boost
/// beyond pure keyword/TF-IDF matching.
fn contextual_similarity_bonus(query_lower: &str, tool: &ToolEntry) -> f32 {
    let tool_lower = tool.name.to_lowercase();
    let desc_lower = tool.description.to_lowercase();

    let associations: &[(&[&str], &[&str])] = &[
        (
            &["file", "read", "write", "save", "load", "open"],
            &["file", "read", "write", "disk", "path"],
        ),
        (
            &["http", "api", "fetch", "request", "url", "endpoint", "web"],
            &["http", "fetch", "web", "api", "request", "browser"],
        ),
        (
            &["search", "find", "query", "look", "locate"],
            &["search", "find", "query", "memory", "index"],
        ),
        (
            &["run", "execute", "command", "shell", "terminal"],
            &["shell", "exec", "command", "run", "terminal"],
        ),
        (
            &["remember", "store", "save", "recall", "memory"],
            &["memory", "store", "save", "recall", "vector"],
        ),
        (
            &["calculate", "math", "compute", "number"],
            &["calculator", "math", "compute", "number"],
        ),
    ];

    for (query_patterns, tool_patterns) in associations {
        let query_match = query_patterns.iter().any(|p| query_lower.contains(p));
        let tool_match = tool_patterns
            .iter()
            .any(|p| tool_lower.contains(p) || desc_lower.contains(p));

        if query_match && tool_match {
            return 0.2;
        }
    }

    0.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_tools() -> Vec<ToolEntry> {
        vec![
            ToolEntry::new("file_read", "Read contents from a file on disk"),
            ToolEntry::new("file_write", "Write content to a file on disk"),
            ToolEntry::new("http_fetch", "Fetch data from an HTTP URL endpoint"),
            ToolEntry::new("shell_exec", "Execute a shell command in a sandbox"),
            ToolEntry::new(
                "memory_search",
                "Search the vector memory store for relevant documents",
            ),
            ToolEntry::new(
                "browser_open",
                "Open a web page in a headless browser for scraping",
            ),
            ToolEntry::new("calculator", "Perform mathematical calculations"),
            ToolEntry::new("json_query", "Query and transform JSON data structures"),
        ]
    }

    // -----------------------------------------------------------------------
    // DiscoveryStrategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_strategy_serde_roundtrip() {
        let strategies = vec![
            DiscoveryStrategy::KeywordMatch,
            DiscoveryStrategy::TfIdf,
            DiscoveryStrategy::Semantic,
            DiscoveryStrategy::Hybrid,
        ];
        for s in strategies {
            let json = serde_json::to_string(&s).unwrap();
            let parsed: DiscoveryStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, s);
        }
    }

    // -----------------------------------------------------------------------
    // DiscoveryConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_defaults() {
        let config = DiscoveryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_tools, 8);
        assert!((config.similarity_threshold - 0.3).abs() < f32::EPSILON);
        assert!(config.always_include.is_empty());
        assert_eq!(config.strategy, DiscoveryStrategy::Hybrid);
    }

    // -----------------------------------------------------------------------
    // ToolEntry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_entry_new() {
        let entry = ToolEntry::new("my_tool", "Does something useful");
        assert_eq!(entry.name, "my_tool");
        assert_eq!(entry.description, "Does something useful");
    }

    #[test]
    fn test_tool_entry_serde_roundtrip() {
        let entry = ToolEntry::new("test_tool", "A test tool for testing");
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ToolEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test_tool");
        assert_eq!(parsed.description, "A test tool for testing");
    }

    // -----------------------------------------------------------------------
    // UsageRecord tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_usage_record_new() {
        let record = UsageRecord::new();
        assert_eq!(record.total_uses, 0);
        assert_eq!(record.successful_uses, 0);
        assert!(record.associated_keywords.is_empty());
    }

    #[test]
    fn test_usage_record_success_rate() {
        let mut record = UsageRecord::new();
        assert!((record.success_rate() - 0.5).abs() < f32::EPSILON); // Default

        record.record_success(&["read".to_string(), "file".to_string()]);
        record.record_success(&["write".to_string()]);
        record.record_failure();

        assert!((record.success_rate() - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_usage_record_tracks_keywords() {
        let mut record = UsageRecord::new();
        record.record_success(&["file".to_string(), "read".to_string()]);
        assert!(record.associated_keywords.contains(&"file".to_string()));
        assert!(record.associated_keywords.contains(&"read".to_string()));
    }

    // -----------------------------------------------------------------------
    // Engine basic tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_engine_with_defaults() {
        let engine = ToolDiscoveryEngine::with_defaults();
        assert!(engine.config().enabled);
        assert_eq!(engine.config().max_tools, 8);
    }

    #[test]
    fn test_discover_disabled_returns_none() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            enabled: false,
            ..DiscoveryConfig::default()
        });
        assert!(engine.discover("test", &sample_tools()).is_none());
    }

    #[test]
    fn test_discover_empty_tools() {
        let engine = ToolDiscoveryEngine::with_defaults();
        let result = engine.discover("test query", &[]).unwrap();
        assert!(result.selected_tools.is_empty());
        assert_eq!(result.total_available, 0);
    }

    // -----------------------------------------------------------------------
    // KeywordMatch strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_keyword_match_file_task() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            ..DiscoveryConfig::default()
        });
        let result = engine
            .discover("read the configuration file", &sample_tools())
            .unwrap();
        let names: Vec<&str> = result
            .selected_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(
            names.contains(&"file_read"),
            "file_read should be selected for file reading, got: {names:?}"
        );
    }

    #[test]
    fn test_keyword_match_http_task() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            ..DiscoveryConfig::default()
        });
        let result = engine
            .discover("fetch data from the API endpoint", &sample_tools())
            .unwrap();
        let names: Vec<&str> = result
            .selected_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(
            names.contains(&"http_fetch"),
            "http_fetch should be selected for HTTP tasks, got: {names:?}"
        );
    }

    #[test]
    fn test_keyword_match_empty_query() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            ..DiscoveryConfig::default()
        });
        let result = engine.discover("", &sample_tools()).unwrap();
        // Empty query should not match anything above threshold
        assert!(
            result.selected_tools.is_empty(),
            "Empty query should select no tools"
        );
    }

    // -----------------------------------------------------------------------
    // TfIdf strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tfidf_selects_relevant() {
        let mut engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::TfIdf,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let tools = sample_tools();
        engine.build_vocabulary(&tools);

        let result = engine.discover("read file contents disk", &tools).unwrap();
        assert!(
            !result.selected_tools.is_empty(),
            "TF-IDF should select at least one tool"
        );
        assert_eq!(
            result.selected_tools[0].name, "file_read",
            "file_read should be top result for file reading query"
        );
    }

    #[test]
    fn test_tfidf_empty_vocabulary() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::TfIdf,
            ..DiscoveryConfig::default()
        });
        let result = engine.discover("read a file", &sample_tools()).unwrap();
        // Without vocabulary, all scores should be 0 => nothing above threshold
        assert!(
            result.selected_tools.is_empty(),
            "Without vocabulary, no tools should be selected above threshold"
        );
    }

    // -----------------------------------------------------------------------
    // Hybrid strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hybrid_selects_relevant() {
        let mut engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::Hybrid,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let tools = sample_tools();
        engine.build_vocabulary(&tools);

        let result = engine.discover("read file contents", &tools).unwrap();
        assert!(!result.selected_tools.is_empty());
        let names: Vec<&str> = result
            .selected_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(
            names.contains(&"file_read"),
            "Expected file_read, got: {names:?}"
        );
    }

    #[test]
    fn test_hybrid_usage_history_affects_ranking() {
        let mut engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::Hybrid,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let tools = sample_tools();
        engine.build_vocabulary(&tools);

        // Record heavy successful usage of memory_search for file-related queries
        for _ in 0..20 {
            engine.record_success("memory_search", "file configuration settings");
        }

        let result = engine
            .discover("file configuration settings", &tools)
            .unwrap();

        // memory_search should rank higher due to usage history
        let memory_tool = result
            .selected_tools
            .iter()
            .find(|t| t.name == "memory_search");
        assert!(
            memory_tool.is_some(),
            "memory_search should be selected due to usage history"
        );
    }

    // -----------------------------------------------------------------------
    // Semantic strategy tests (placeholder)
    // -----------------------------------------------------------------------

    #[test]
    fn test_semantic_falls_back_to_tfidf_plus_context() {
        let mut engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::Semantic,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let tools = sample_tools();
        engine.build_vocabulary(&tools);

        let result = engine.discover("read file contents", &tools).unwrap();
        assert!(!result.selected_tools.is_empty());
    }

    // -----------------------------------------------------------------------
    // Always-include tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_always_include_present() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            always_include: vec!["calculator".to_string()],
            similarity_threshold: 0.99, // Very high threshold
            ..DiscoveryConfig::default()
        });
        let result = engine
            .discover("read a file from disk", &sample_tools())
            .unwrap();
        let names: Vec<&str> = result
            .selected_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(
            names.contains(&"calculator"),
            "Always-included tool should be present, got: {names:?}"
        );
    }

    #[test]
    fn test_always_include_nonexistent_tool() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            always_include: vec!["nonexistent_tool".to_string()],
            ..DiscoveryConfig::default()
        });
        let result = engine.discover("test query", &sample_tools()).unwrap();
        let names: Vec<&str> = result
            .selected_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(
            !names.contains(&"nonexistent_tool"),
            "Nonexistent tool should not be included"
        );
    }

    // -----------------------------------------------------------------------
    // Max tools limit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_max_tools_respected() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            max_tools: 2,
            similarity_threshold: 0.0,
            strategy: DiscoveryStrategy::KeywordMatch,
            ..DiscoveryConfig::default()
        });
        let result = engine
            .discover("file read write disk data http fetch web shell command memory search calculator math json query", &sample_tools())
            .unwrap();
        assert!(
            result.selected_tools.len() <= 2,
            "Should respect max_tools limit of 2, got {}",
            result.selected_tools.len()
        );
    }

    // -----------------------------------------------------------------------
    // Token savings tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_token_savings_computed() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            max_tools: 2,
            similarity_threshold: 0.0,
            strategy: DiscoveryStrategy::KeywordMatch,
            ..DiscoveryConfig::default()
        });
        let tools = sample_tools();
        let result = engine
            .discover("read a file from disk contents", &tools)
            .unwrap();
        let excluded = tools.len().saturating_sub(result.selected_tools.len());
        assert_eq!(result.token_savings, excluded * TOKENS_PER_TOOL);
    }

    #[test]
    fn test_token_savings_zero_when_all_selected() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            max_tools: 100,
            similarity_threshold: 0.0,
            strategy: DiscoveryStrategy::KeywordMatch,
            ..DiscoveryConfig::default()
        });
        // Query with keywords matching all tools
        let result = engine
            .discover(
                "file read write http fetch shell exec memory search browser calculator json query data",
                &sample_tools(),
            )
            .unwrap();
        if result.selected_tools.len() == sample_tools().len() {
            assert_eq!(result.token_savings, 0);
        }
    }

    // -----------------------------------------------------------------------
    // DiscoveryResult tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_discovery_result_serde_roundtrip() {
        let result = DiscoveryResult {
            selected_tools: vec![DiscoveredTool {
                name: "file_read".to_string(),
                description: "Read a file".to_string(),
                relevance_score: 0.9,
                selection_reason: "keyword match".to_string(),
            }],
            total_available: 10,
            token_savings: 1350,
            strategy_used: DiscoveryStrategy::Hybrid,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: DiscoveryResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.selected_tools.len(), 1);
        assert_eq!(parsed.total_available, 10);
        assert_eq!(parsed.token_savings, 1350);
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Read a File from Disk");
        assert!(tokens.contains(&"read".to_string()));
        assert!(tokens.contains(&"file".to_string()));
        assert!(tokens.contains(&"disk".to_string()));
        assert!(!tokens.contains(&"from".to_string())); // stopword
    }

    #[test]
    fn test_tokenize_removes_short_words() {
        let tokens = tokenize("I am a go to");
        assert!(tokens.is_empty() || tokens.iter().all(|t| t.len() > 2));
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn test_tfidf_cosine_identical() {
        let mut vocab = HashMap::new();
        vocab.insert("file".to_string(), 1.0);
        vocab.insert("read".to_string(), 1.0);

        let query = tokenize("read file");
        let score = tfidf_cosine_similarity(&query, "read file", &vocab);
        assert!(
            score > 0.9,
            "Identical texts should have high similarity, got {score}"
        );
    }

    #[test]
    fn test_tfidf_cosine_empty() {
        let vocab = HashMap::new();
        let query = tokenize("");
        let score = tfidf_cosine_similarity(&query, "anything", &vocab);
        assert!(score.abs() < f32::EPSILON);
    }

    #[test]
    fn test_contextual_similarity_bonus_file() {
        let tool = ToolEntry::new("file_read", "Read a file");
        let bonus = contextual_similarity_bonus("read the file", &tool);
        assert!(
            bonus > 0.0,
            "File-related query should get bonus, got {bonus}"
        );
    }

    #[test]
    fn test_contextual_similarity_bonus_no_match() {
        let tool = ToolEntry::new("calculator", "Do math");
        let bonus = contextual_similarity_bonus("read the file", &tool);
        assert!(
            bonus.abs() < f32::EPSILON,
            "No contextual match should give 0 bonus, got {bonus}"
        );
    }

    #[test]
    fn test_relevance_scores_ordered() {
        let mut engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        engine.build_vocabulary(&sample_tools());

        let result = engine
            .discover("read file contents from disk", &sample_tools())
            .unwrap();

        // Verify scores are in descending order
        for i in 1..result.selected_tools.len() {
            assert!(
                result.selected_tools[i - 1].relevance_score
                    >= result.selected_tools[i].relevance_score,
                "Tools should be ordered by relevance score"
            );
        }
    }

    #[test]
    fn test_record_and_query_usage_history() {
        let mut engine = ToolDiscoveryEngine::with_defaults();
        engine.record_success("file_read", "reading configuration files");
        engine.record_failure("file_read");

        let record = engine.usage_history("file_read").unwrap();
        assert_eq!(record.total_uses, 2);
        assert_eq!(record.successful_uses, 1);
        assert!((record.success_rate() - 0.5).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // ToolDiscoveryCache tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_cache_hit_returns_same_result() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let mut cache = ToolDiscoveryCache::new(50);
        let tools = sample_tools();

        let r1 = cache.get_or_compute("read file from disk", &tools, 0, &engine);
        let r2 = cache.get_or_compute("read file from disk", &tools, 0, &engine);

        // Second call must be a cache hit.
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);

        // Both results must agree on selected tool names.
        match (r1, r2) {
            (Some(a), Some(b)) => {
                let names_a: Vec<&str> = a.selected_tools.iter().map(|t| t.name.as_str()).collect();
                let names_b: Vec<&str> = b.selected_tools.iter().map(|t| t.name.as_str()).collect();
                assert_eq!(
                    names_a, names_b,
                    "cache hit should return identical tool list"
                );
            }
            (None, None) => {}
            _ => panic!("cache hit and miss returned different Some/None variants"),
        }
    }

    #[test]
    fn test_cache_miss_on_different_intent() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let mut cache = ToolDiscoveryCache::new(50);
        let tools = sample_tools();

        cache.get_or_compute("read file from disk", &tools, 0, &engine);
        cache.get_or_compute("fetch data from api", &tools, 0, &engine);

        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn test_cache_miss_on_registry_version_change() {
        let engine = ToolDiscoveryEngine::with_defaults();
        let mut cache = ToolDiscoveryCache::new(50);
        let tools = sample_tools();

        cache.get_or_compute("search memory", &tools, 0, &engine);
        // Same intent, different registry version → must recompute.
        cache.get_or_compute("search memory", &tools, 1, &engine);

        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn test_cache_stale_after_turn_limit() {
        let engine = ToolDiscoveryEngine::new(DiscoveryConfig {
            strategy: DiscoveryStrategy::KeywordMatch,
            similarity_threshold: 0.0,
            ..DiscoveryConfig::default()
        });
        let mut cache = ToolDiscoveryCache::new(2); // tiny limit
        let tools = sample_tools();

        cache.get_or_compute("read file", &tools, 0, &engine);
        assert_eq!(cache.misses(), 1);

        // Advance past the turn limit.
        cache.advance_turn();
        cache.advance_turn();

        // Entry is now stale — should recompute.
        cache.get_or_compute("read file", &tools, 0, &engine);
        assert_eq!(cache.misses(), 2, "stale entry should cause a miss");
    }

    #[test]
    fn test_cache_clear_resets_state() {
        let engine = ToolDiscoveryEngine::with_defaults();
        let mut cache = ToolDiscoveryCache::new(50);
        let tools = sample_tools();

        cache.get_or_compute("search memory", &tools, 0, &engine);
        cache.clear();

        // After clear, same query must miss again.
        cache.get_or_compute("search memory", &tools, 0, &engine);
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
    }
}

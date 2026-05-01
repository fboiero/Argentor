//! Smart tool selection for reducing token waste and model confusion.
//!
//! Instead of sending every registered tool to the LLM on each call, this
//! module selects the most relevant subset based on the current task.
//! Strategies range from simple keyword matching to TF-IDF relevance
//! scoring combined with historical success rates.

use argentor_skills::SkillDescriptor;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Common English stopwords filtered out during tokenization.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
    "our", "out", "has", "have", "from", "with", "they", "been", "this", "that", "will", "each",
    "make", "like", "use", "into",
];

/// Statistics for tracking tool usage and success rates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStats {
    /// Name of the tool these stats belong to.
    pub tool_name: String,
    /// Total number of times the tool was invoked.
    pub total_calls: u64,
    /// Number of successful invocations.
    pub successful_calls: u64,
    /// Number of failed invocations.
    pub failed_calls: u64,
    /// Average latency in milliseconds across all invocations.
    pub avg_latency_ms: f64,
    /// Timestamp of the most recent invocation, if any.
    pub last_used: Option<DateTime<Utc>>,
}

impl ToolStats {
    /// Create a new `ToolStats` for the given tool with zeroed counters.
    pub fn new(tool_name: &str) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            total_calls: 0,
            successful_calls: 0,
            failed_calls: 0,
            avg_latency_ms: 0.0,
            last_used: None,
        }
    }

    /// Returns the success rate as a value in `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when no calls have been recorded.
    pub fn success_rate(&self) -> f64 {
        if self.total_calls == 0 {
            return 0.0;
        }
        self.successful_calls as f64 / self.total_calls as f64
    }

    /// Record a successful invocation with the given latency.
    pub fn record_success(&mut self, latency_ms: u64) {
        self.successful_calls += 1;
        self.total_calls += 1;
        self.update_avg_latency(latency_ms);
        self.last_used = Some(Utc::now());
    }

    /// Record a failed invocation with the given latency.
    pub fn record_failure(&mut self, latency_ms: u64) {
        self.failed_calls += 1;
        self.total_calls += 1;
        self.update_avg_latency(latency_ms);
        self.last_used = Some(Utc::now());
    }

    /// Update the running average latency with a new sample.
    fn update_avg_latency(&mut self, latency_ms: u64) {
        // Incremental mean: new_avg = old_avg + (sample - old_avg) / n
        let n = self.total_calls as f64;
        self.avg_latency_ms += (latency_ms as f64 - self.avg_latency_ms) / n;
    }
}

/// Strategy for selecting which tools to present to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SelectionStrategy {
    /// Use all available tools (current behavior, no filtering).
    All,
    /// Select tools whose name or description shares keywords with the task.
    KeywordMatch {
        /// Maximum number of tools to return.
        max_tools: usize,
    },
    /// Rank tools by TF-IDF cosine similarity between the task and tool descriptions.
    Relevance {
        /// Maximum number of tools to return.
        max_tools: usize,
        /// Minimum similarity score to include a tool (0.0 to 1.0).
        min_score: f32,
    },
    /// Combine TF-IDF relevance with historical success rates.
    Adaptive {
        /// Maximum number of tools to return.
        max_tools: usize,
        /// Weight applied to the historical success rate component.
        success_weight: f32,
        /// Weight applied to the TF-IDF relevance component.
        relevance_weight: f32,
    },
}

/// The result of a tool selection operation.
#[derive(Debug, Clone)]
pub struct ToolSelection {
    /// The selected tool descriptors.
    pub selected: Vec<SkillDescriptor>,
    /// Scores for each considered tool, sorted descending by score.
    pub scores: Vec<(String, f32)>,
    /// The strategy that produced this selection.
    pub strategy_used: SelectionStrategy,
}

/// Intelligent tool selector that analyzes the task and picks the most
/// relevant subset of tools to send to the LLM.
pub struct ToolSelector {
    /// The active selection strategy.
    strategy: SelectionStrategy,
    /// Per-tool usage statistics.
    stats: HashMap<String, ToolStats>,
    /// TF-IDF vocabulary: term -> IDF weight.
    vocabulary: HashMap<String, f32>,
}

impl ToolSelector {
    /// Create a new `ToolSelector` with the given strategy.
    pub fn new(strategy: SelectionStrategy) -> Self {
        Self {
            strategy,
            stats: HashMap::new(),
            vocabulary: HashMap::new(),
        }
    }

    /// Build the IDF vocabulary from all tool descriptions.
    ///
    /// This should be called once after all tools are registered, or
    /// whenever the tool set changes.
    pub fn build_vocabulary(&mut self, tools: &[SkillDescriptor]) {
        self.vocabulary.clear();

        if tools.is_empty() {
            return;
        }

        // Count how many documents each term appears in.
        let num_docs = tools.len() as f32;
        let mut doc_freq: HashMap<String, u32> = HashMap::new();

        for tool in tools {
            let text = format!("{} {}", tool.name, tool.description);
            let tokens = Self::tokenize(&text);
            // Deduplicate within the document.
            let unique: std::collections::HashSet<String> = tokens.into_iter().collect();
            for term in unique {
                *doc_freq.entry(term).or_insert(0) += 1;
            }
        }

        // Compute IDF = ln(N / df)  (natural log)
        for (term, df) in &doc_freq {
            let idf = (num_docs / *df as f32).ln();
            self.vocabulary.insert(term.clone(), idf);
        }
    }

    /// Select the most relevant tools for the given task text.
    pub fn select(&self, task: &str, tools: &[SkillDescriptor]) -> ToolSelection {
        match &self.strategy {
            SelectionStrategy::All => ToolSelection {
                selected: tools.to_vec(),
                scores: tools.iter().map(|t| (t.name.clone(), 1.0)).collect(),
                strategy_used: self.strategy.clone(),
            },

            SelectionStrategy::KeywordMatch { max_tools } => {
                self.select_by_keywords(task, tools, *max_tools)
            }

            SelectionStrategy::Relevance {
                max_tools,
                min_score,
            } => self.select_by_relevance(task, tools, *max_tools, *min_score),

            SelectionStrategy::Adaptive {
                max_tools,
                success_weight,
                relevance_weight,
            } => self.select_adaptive(task, tools, *max_tools, *success_weight, *relevance_weight),
        }
    }

    /// Record a successful tool invocation.
    pub fn record_success(&mut self, tool_name: &str, latency_ms: u64) {
        self.stats
            .entry(tool_name.to_string())
            .or_insert_with(|| ToolStats::new(tool_name))
            .record_success(latency_ms);
    }

    /// Record a failed tool invocation.
    pub fn record_failure(&mut self, tool_name: &str, latency_ms: u64) {
        self.stats
            .entry(tool_name.to_string())
            .or_insert_with(|| ToolStats::new(tool_name))
            .record_failure(latency_ms);
    }

    /// Get stats for a specific tool, if any calls have been recorded.
    pub fn tool_stats(&self, tool_name: &str) -> Option<&ToolStats> {
        self.stats.get(tool_name)
    }

    /// Get a reference to all recorded tool stats.
    pub fn all_stats(&self) -> &HashMap<String, ToolStats> {
        &self.stats
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Select tools by keyword overlap between task and tool name/description.
    fn select_by_keywords(
        &self,
        task: &str,
        tools: &[SkillDescriptor],
        max_tools: usize,
    ) -> ToolSelection {
        let task_tokens: std::collections::HashSet<String> =
            Self::tokenize(task).into_iter().collect();

        let mut scored: Vec<(usize, f32)> = tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                let tool_text = format!("{} {}", tool.name, tool.description);
                let tool_tokens: std::collections::HashSet<String> =
                    Self::tokenize(&tool_text).into_iter().collect();

                let overlap = task_tokens.intersection(&tool_tokens).count() as f32;
                let denominator = task_tokens.len().max(1) as f32;
                let score = overlap / denominator;
                (i, score)
            })
            .collect();

        // Sort descending by score.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_tools);

        let scores: Vec<(String, f32)> = scored
            .iter()
            .map(|(i, s)| (tools[*i].name.clone(), *s))
            .collect();

        let selected: Vec<SkillDescriptor> = scored
            .iter()
            .filter(|(_, s)| *s > 0.0)
            .map(|(i, _)| tools[*i].clone())
            .collect();

        ToolSelection {
            selected,
            scores,
            strategy_used: self.strategy.clone(),
        }
    }

    /// Select tools by TF-IDF cosine similarity.
    fn select_by_relevance(
        &self,
        task: &str,
        tools: &[SkillDescriptor],
        max_tools: usize,
        min_score: f32,
    ) -> ToolSelection {
        let mut scored: Vec<(usize, f32)> = tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                let doc = format!("{} {}", tool.name, tool.description);
                let score = self.tfidf_similarity(task, &doc);
                (i, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_tools);

        let scores: Vec<(String, f32)> = scored
            .iter()
            .map(|(i, s)| (tools[*i].name.clone(), *s))
            .collect();

        let selected: Vec<SkillDescriptor> = scored
            .iter()
            .filter(|(_, s)| *s >= min_score)
            .map(|(i, _)| tools[*i].clone())
            .collect();

        ToolSelection {
            selected,
            scores,
            strategy_used: self.strategy.clone(),
        }
    }

    /// Select tools by combining TF-IDF relevance with historical success rates.
    fn select_adaptive(
        &self,
        task: &str,
        tools: &[SkillDescriptor],
        max_tools: usize,
        success_weight: f32,
        relevance_weight: f32,
    ) -> ToolSelection {
        let mut scored: Vec<(usize, f32)> = tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                let doc = format!("{} {}", tool.name, tool.description);
                let relevance = self.tfidf_similarity(task, &doc);

                let success_rate = self
                    .stats
                    .get(&tool.name)
                    .map(|s| s.success_rate() as f32)
                    .unwrap_or(0.5); // Default to neutral for unseen tools.

                let score = relevance_weight * relevance + success_weight * success_rate;
                (i, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_tools);

        let scores: Vec<(String, f32)> = scored
            .iter()
            .map(|(i, s)| (tools[*i].name.clone(), *s))
            .collect();

        let selected: Vec<SkillDescriptor> =
            scored.iter().map(|(i, _)| tools[*i].clone()).collect();

        ToolSelection {
            selected,
            scores,
            strategy_used: self.strategy.clone(),
        }
    }

    /// Compute TF-IDF cosine similarity between a query and a document.
    ///
    /// Uses the pre-built vocabulary for IDF weights. Terms not in the
    /// vocabulary are assigned an IDF of `0.0` (ignored).
    fn tfidf_similarity(&self, query: &str, document: &str) -> f32 {
        let query_tokens = Self::tokenize(query);
        let doc_tokens = Self::tokenize(document);

        if query_tokens.is_empty() || doc_tokens.is_empty() {
            return 0.0;
        }

        // Build TF maps.
        let query_tf = Self::term_frequencies(&query_tokens);
        let doc_tf = Self::term_frequencies(&doc_tokens);

        // Collect all terms appearing in either.
        let all_terms: std::collections::HashSet<&String> = query_tf
            .keys()
            .copied()
            .chain(doc_tf.keys().copied())
            .collect();

        // Compute TF-IDF vectors and cosine similarity.
        let mut dot_product: f32 = 0.0;
        let mut query_magnitude: f32 = 0.0;
        let mut doc_magnitude: f32 = 0.0;

        for term in all_terms {
            let idf = self.vocabulary.get(term.as_str()).copied().unwrap_or(0.0);
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

    /// Compute normalized term frequencies for a list of tokens.
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

    /// Tokenize text into lowercase terms, removing short words and stopwords.
    fn tokenize(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .filter(|w| !STOPWORDS.contains(w))
            .map(String::from)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Helper to build a `SkillDescriptor` for testing.
    fn make_descriptor(name: &str, description: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: description.to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
            required_capabilities: vec![],
            requires_approval: false,
        }
    }

    /// Helper to build a list of diverse tool descriptors for selection tests.
    fn sample_tools() -> Vec<SkillDescriptor> {
        vec![
            make_descriptor("file_read", "Read contents from a file on disk"),
            make_descriptor("file_write", "Write content to a file on disk"),
            make_descriptor("http_fetch", "Fetch data from an HTTP URL endpoint"),
            make_descriptor("shell_exec", "Execute a shell command in a sandbox"),
            make_descriptor(
                "memory_search",
                "Search the vector memory store for relevant documents",
            ),
            make_descriptor(
                "browser_open",
                "Open a web page in a headless browser for scraping",
            ),
        ]
    }

    // -----------------------------------------------------------------------
    // ToolStats tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_stats_new() {
        let stats = ToolStats::new("my_tool");
        assert_eq!(stats.tool_name, "my_tool");
        assert_eq!(stats.total_calls, 0);
        assert_eq!(stats.successful_calls, 0);
        assert_eq!(stats.failed_calls, 0);
        assert!((stats.avg_latency_ms - 0.0).abs() < f64::EPSILON);
        assert!(stats.last_used.is_none());
    }

    #[test]
    fn test_tool_stats_success_rate() {
        let mut stats = ToolStats::new("rate_tool");
        // Zero calls => 0.0
        assert!((stats.success_rate() - 0.0).abs() < f64::EPSILON);

        stats.record_success(10);
        stats.record_success(20);
        stats.record_failure(30);
        // 2 out of 3 => ~0.6667
        assert!((stats.success_rate() - 2.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_tool_stats_record_success() {
        let mut stats = ToolStats::new("s_tool");
        stats.record_success(100);
        assert_eq!(stats.total_calls, 1);
        assert_eq!(stats.successful_calls, 1);
        assert_eq!(stats.failed_calls, 0);
        assert!((stats.avg_latency_ms - 100.0).abs() < f64::EPSILON);
        assert!(stats.last_used.is_some());

        stats.record_success(200);
        assert_eq!(stats.total_calls, 2);
        assert_eq!(stats.successful_calls, 2);
        // avg = (100 + 200) / 2 = 150
        assert!((stats.avg_latency_ms - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tool_stats_record_failure() {
        let mut stats = ToolStats::new("f_tool");
        stats.record_failure(50);
        assert_eq!(stats.total_calls, 1);
        assert_eq!(stats.successful_calls, 0);
        assert_eq!(stats.failed_calls, 1);
        assert!((stats.avg_latency_ms - 50.0).abs() < f64::EPSILON);
        assert!(stats.last_used.is_some());

        stats.record_failure(150);
        assert_eq!(stats.total_calls, 2);
        assert_eq!(stats.failed_calls, 2);
        // avg = (50 + 150) / 2 = 100
        assert!((stats.avg_latency_ms - 100.0).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------------
    // SelectionStrategy::All
    // -----------------------------------------------------------------------

    #[test]
    fn test_selection_strategy_all() {
        let selector = ToolSelector::new(SelectionStrategy::All);
        let tools = sample_tools();
        let result = selector.select("read a file", &tools);

        assert_eq!(result.selected.len(), tools.len());
        assert_eq!(result.scores.len(), tools.len());
        // Every tool should get score 1.0
        for (_, score) in &result.scores {
            assert!((*score - 1.0).abs() < f32::EPSILON);
        }
    }

    // -----------------------------------------------------------------------
    // SelectionStrategy::KeywordMatch
    // -----------------------------------------------------------------------

    #[test]
    fn test_selection_keyword_match() {
        let selector = ToolSelector::new(SelectionStrategy::KeywordMatch { max_tools: 10 });
        let tools = sample_tools();
        let result = selector.select("read the file contents", &tools);

        // "file_read" should be selected (keywords: "read", "file", "contents" match)
        assert!(!result.selected.is_empty());
        let names: Vec<&str> = result.selected.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"file_read"),
            "file_read should be selected for 'read the file contents'"
        );
    }

    #[test]
    fn test_selection_keyword_match_respects_max() {
        let selector = ToolSelector::new(SelectionStrategy::KeywordMatch { max_tools: 2 });
        let tools = sample_tools();
        let result = selector.select("file read write disk content", &tools);

        // Scores list should have at most max_tools entries.
        assert!(result.scores.len() <= 2);
    }

    // -----------------------------------------------------------------------
    // SelectionStrategy::Relevance
    // -----------------------------------------------------------------------

    #[test]
    fn test_selection_relevance_scoring() {
        let mut selector = ToolSelector::new(SelectionStrategy::Relevance {
            max_tools: 6,
            min_score: 0.0,
        });
        let tools = sample_tools();
        selector.build_vocabulary(&tools);

        let result = selector.select("read contents from a file", &tools);

        // file_read should be the top-scored tool.
        assert!(!result.scores.is_empty());
        assert_eq!(
            result.scores[0].0, "file_read",
            "file_read should score highest for 'read contents from a file'"
        );
    }

    #[test]
    fn test_selection_relevance_min_score() {
        let mut selector = ToolSelector::new(SelectionStrategy::Relevance {
            max_tools: 10,
            min_score: 0.99, // Very high threshold — most tools should be excluded.
        });
        let tools = sample_tools();
        selector.build_vocabulary(&tools);

        let result = selector.select("something unrelated like quantum physics", &tools);

        // With a very high min_score and an unrelated query, few or no tools pass.
        assert!(
            result.selected.len() < tools.len(),
            "High min_score should filter out most tools"
        );
    }

    // -----------------------------------------------------------------------
    // SelectionStrategy::Adaptive
    // -----------------------------------------------------------------------

    #[test]
    fn test_selection_adaptive_combines_scores() {
        let mut selector = ToolSelector::new(SelectionStrategy::Adaptive {
            max_tools: 3,
            success_weight: 0.5,
            relevance_weight: 0.5,
        });
        let tools = sample_tools();
        selector.build_vocabulary(&tools);

        // Give file_write a perfect success rate.
        selector.record_success("file_write", 10);
        selector.record_success("file_write", 20);
        // Give file_read a poor success rate.
        selector.record_failure("file_read", 50);
        selector.record_failure("file_read", 60);

        let result = selector.select("write data to a file on disk", &tools);

        // file_write should rank high (high relevance AND high success rate).
        assert!(!result.selected.is_empty());
        let first_name = &result.scores[0].0;
        assert_eq!(
            first_name, "file_write",
            "file_write should rank first for 'write data to a file on disk' with perfect success rate"
        );
    }

    // -----------------------------------------------------------------------
    // Vocabulary and TF-IDF tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_vocabulary() {
        let mut selector = ToolSelector::new(SelectionStrategy::All);
        let tools = sample_tools();
        selector.build_vocabulary(&tools);

        // Vocabulary should contain terms from tool names/descriptions.
        assert!(
            !selector.vocabulary.is_empty(),
            "Vocabulary should be populated after build"
        );
        // "file" appears in multiple tools, so its IDF should be lower than
        // a term that appears in only one tool.
        let file_idf = selector.vocabulary.get("file").copied().unwrap_or(0.0);
        let vector_idf = selector.vocabulary.get("vector").copied().unwrap_or(0.0);
        assert!(
            vector_idf > file_idf,
            "Rare term 'vector' should have higher IDF than common term 'file'"
        );
    }

    #[test]
    fn test_tfidf_similarity_identical() {
        let mut selector = ToolSelector::new(SelectionStrategy::All);
        let tools = vec![
            make_descriptor("tool_a", "read file contents from disk"),
            make_descriptor("tool_b", "fetch http endpoint data"),
        ];
        selector.build_vocabulary(&tools);

        let score = selector.tfidf_similarity(
            "read file contents from disk",
            "read file contents from disk",
        );
        // Identical texts should have very high similarity (close to 1.0).
        assert!(
            score > 0.9,
            "Identical texts should have similarity > 0.9, got {score}"
        );
    }

    #[test]
    fn test_tfidf_similarity_unrelated() {
        let mut selector = ToolSelector::new(SelectionStrategy::All);
        let tools = vec![
            make_descriptor("tool_a", "read file contents from disk"),
            make_descriptor("tool_b", "fetch http endpoint data"),
            make_descriptor("tool_c", "quantum physics simulation experiment"),
        ];
        selector.build_vocabulary(&tools);

        let score = selector.tfidf_similarity("read file contents", "quantum physics simulation");
        assert!(
            score < 0.1,
            "Unrelated texts should have similarity < 0.1, got {score}"
        );
    }

    // -----------------------------------------------------------------------
    // Tokenization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tokenize_basic() {
        let tokens = ToolSelector::tokenize("Read a File from Disk");
        // "a" is too short (len <= 2), "from" is a stopword.
        assert!(tokens.contains(&"read".to_string()));
        assert!(tokens.contains(&"file".to_string()));
        assert!(tokens.contains(&"disk".to_string()));
        // Short words removed.
        assert!(!tokens.contains(&"a".to_string()));
    }

    #[test]
    fn test_tokenize_removes_stopwords() {
        let tokens = ToolSelector::tokenize("the file and the data from this tool");
        // "the", "and", "from", "this" are stopwords.
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"and".to_string()));
        assert!(!tokens.contains(&"from".to_string()));
        assert!(!tokens.contains(&"this".to_string()));
        // "file", "data", "tool" should remain.
        assert!(tokens.contains(&"file".to_string()));
        assert!(tokens.contains(&"data".to_string()));
        assert!(tokens.contains(&"tool".to_string()));
    }

    // -----------------------------------------------------------------------
    // Stats recording tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_record_updates_stats() {
        let mut selector = ToolSelector::new(SelectionStrategy::All);

        assert!(selector.tool_stats("my_tool").is_none());

        selector.record_success("my_tool", 100);
        let stats = selector.tool_stats("my_tool").unwrap();
        assert_eq!(stats.total_calls, 1);
        assert_eq!(stats.successful_calls, 1);

        selector.record_failure("my_tool", 200);
        let stats = selector.tool_stats("my_tool").unwrap();
        assert_eq!(stats.total_calls, 2);
        assert_eq!(stats.failed_calls, 1);

        // all_stats should contain the tool.
        assert!(selector.all_stats().contains_key("my_tool"));
    }

    // -----------------------------------------------------------------------
    // Edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_selection_empty_tools() {
        let selector = ToolSelector::new(SelectionStrategy::KeywordMatch { max_tools: 5 });
        let result = selector.select("read a file", &[]);
        assert!(result.selected.is_empty());
        assert!(result.scores.is_empty());
    }

    #[test]
    fn test_selection_empty_task() {
        let selector = ToolSelector::new(SelectionStrategy::KeywordMatch { max_tools: 5 });
        let tools = sample_tools();
        let result = selector.select("", &tools);
        // With no task keywords, no tool should match.
        assert!(result.selected.is_empty());
    }

    #[test]
    fn test_relevance_empty_vocabulary() {
        // Relevance selection without building vocabulary should still work
        // (all similarities will be 0).
        let selector = ToolSelector::new(SelectionStrategy::Relevance {
            max_tools: 5,
            min_score: 0.0,
        });
        let tools = sample_tools();
        let result = selector.select("read a file", &tools);
        // Scores should all be 0 since vocabulary is empty.
        for (_, score) in &result.scores {
            assert!((*score).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_build_vocabulary_empty_tools() {
        let mut selector = ToolSelector::new(SelectionStrategy::All);
        selector.build_vocabulary(&[]);
        assert!(selector.vocabulary.is_empty());
    }
}

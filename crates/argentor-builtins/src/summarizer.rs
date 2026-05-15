//! Extractive text summarization skill using sentence scoring heuristics.
//!
//! Pure Rust implementation inspired by Semantic Kernel ConversationSummaryPlugin.
//! No LLM calls -- uses position, length, keyword frequency, and title overlap
//! to score and rank sentences.
//!
//! # Supported operations
//!
//! - `summarize` -- Extract top N sentences from text.
//! - `extract_keywords` -- Extract top keywords by frequency.
//! - `word_frequency` -- Full word frequency map (excluding stop words).
//! - `readability` -- Readability metrics (counts, averages, reading time).

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::collections::HashMap;

/// Skill for extractive text summarization.
pub struct SummarizerSkill {
    descriptor: SkillDescriptor,
}

impl SummarizerSkill {
    /// Create a new `SummarizerSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "summarizer".to_string(),
                description:
                    "Extractive text summarization using sentence scoring. \
                              Operations: summarize, extract_keywords, word_frequency, readability."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["summarize", "extract_keywords", "word_frequency", "readability"],
                            "description": "The operation to perform"
                        },
                        "text": {
                            "type": "string",
                            "description": "The text to process"
                        },
                        "max_sentences": {
                            "type": "integer",
                            "description": "Maximum sentences to extract (default 3, max 10)"
                        },
                        "max_keywords": {
                            "type": "integer",
                            "description": "Maximum keywords to extract (default 10)"
                        },
                        "title": {
                            "type": "string",
                            "description": "Optional title for boosting sentence scores"
                        }
                    },
                    "required": ["operation", "text"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for SummarizerSkill {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Stop words
// ---------------------------------------------------------------------------

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "to",
    "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through", "during",
    "before", "after", "above", "below", "between", "out", "off", "over", "under", "again",
    "further", "then", "once", "here", "there", "when", "where", "why", "how", "all", "both",
    "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not", "only", "own",
    "same", "so", "than", "too", "very", "just", "because", "but", "and", "or", "if", "while",
    "about", "against", "it", "its", "i", "me", "my", "we", "our", "you", "your", "he", "him",
    "his", "she", "her", "they", "them", "their", "this", "that", "these", "those", "what",
    "which", "who", "whom",
];

fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word.to_lowercase().as_str())
}

// ---------------------------------------------------------------------------
// Text utilities
// ---------------------------------------------------------------------------

/// Split text into sentences using basic punctuation rules.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    for i in 0..len {
        current.push(chars[i]);

        if matches!(chars[i], '.' | '!' | '?') {
            // Check for abbreviations (single uppercase letter + period)
            let is_abbreviation = i >= 1
                && chars[i] == '.'
                && chars[i - 1].is_uppercase()
                && (i < 2 || !chars[i - 2].is_alphanumeric());

            // Check if next char is a space or end of text (sentence boundary)
            let is_boundary =
                i + 1 >= len || chars[i + 1].is_whitespace() || chars[i + 1].is_uppercase();

            if is_boundary && !is_abbreviation {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    sentences.push(trimmed);
                }
                current.clear();
            }
        }
    }

    // Add remaining text as last sentence
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }

    sentences
}

/// Extract words from text, lowercased, excluding punctuation.
fn extract_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Count word frequencies excluding stop words.
fn word_frequencies(text: &str) -> HashMap<String, usize> {
    let mut freq: HashMap<String, usize> = HashMap::new();
    for word in extract_words(text) {
        if !is_stop_word(&word) && word.len() > 1 {
            *freq.entry(word).or_insert(0) += 1;
        }
    }
    freq
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

struct ScoredSentence {
    index: usize,
    text: String,
    score: f64,
}

fn score_sentences(
    sentences: &[String],
    freq: &HashMap<String, usize>,
    title: Option<&str>,
) -> Vec<ScoredSentence> {
    let total = sentences.len();
    if total == 0 {
        return Vec::new();
    }

    let max_freq = freq.values().copied().max().unwrap_or(1) as f64;

    // Title words for overlap scoring
    let title_words: Vec<String> = title
        .map(|t| {
            extract_words(t)
                .into_iter()
                .filter(|w| !is_stop_word(w))
                .collect()
        })
        .unwrap_or_default();

    sentences
        .iter()
        .enumerate()
        .map(|(i, sentence)| {
            let words = extract_words(sentence);
            let word_count = words.len();
            let mut score = 0.0f64;

            // 1. Position score: first and last sentences score higher
            let position_score = if i == 0 {
                1.0
            } else if i == total - 1 {
                0.7
            } else if i == 1 {
                0.6
            } else {
                0.3 / (1.0 + (i as f64 / total as f64))
            };
            score += position_score;

            // 2. Length score: optimal 10-30 words
            let length_score = if word_count < 5 {
                0.1
            } else if word_count <= 10 {
                0.5
            } else if word_count <= 30 {
                1.0
            } else if word_count <= 50 {
                0.6
            } else {
                0.3
            };
            score += length_score;

            // 3. Keyword frequency score
            let keyword_score: f64 = words
                .iter()
                .filter(|w| !is_stop_word(w) && w.len() > 1)
                .map(|w| freq.get(w).copied().unwrap_or(0) as f64 / max_freq)
                .sum::<f64>()
                / (word_count.max(1) as f64);
            score += keyword_score;

            // 4. Title overlap score
            if !title_words.is_empty() {
                let overlap = words.iter().filter(|w| title_words.contains(w)).count();
                let title_score = overlap as f64 / title_words.len() as f64;
                score += title_score;
            }

            ScoredSentence {
                index: i,
                text: sentence.clone(),
                score,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

fn op_summarize(text: &str, max_sentences: usize, title: Option<&str>) -> serde_json::Value {
    let sentences = split_sentences(text);
    if sentences.is_empty() {
        return serde_json::json!({
            "summary": "",
            "sentences": [],
            "sentence_count": 0,
        });
    }

    let freq = word_frequencies(text);
    let mut scored = score_sentences(&sentences, &freq, title);

    // Sort by score descending to pick top N
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(max_sentences);

    // Sort selected sentences by original order for coherent output
    scored.sort_by_key(|s| s.index);

    let summary_sentences: Vec<serde_json::Value> = scored
        .iter()
        .map(|s| {
            serde_json::json!({
                "index": s.index,
                "text": s.text,
                "score": (s.score * 100.0).round() / 100.0,
            })
        })
        .collect();

    let summary_text: String = scored
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    serde_json::json!({
        "summary": summary_text,
        "sentences": summary_sentences,
        "sentence_count": summary_sentences.len(),
    })
}

fn op_extract_keywords(text: &str, max_keywords: usize) -> serde_json::Value {
    let freq = word_frequencies(text);
    let mut pairs: Vec<(String, usize)> = freq.into_iter().collect();
    pairs.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    pairs.truncate(max_keywords);

    let keywords: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(word, count)| {
            serde_json::json!({
                "word": word,
                "count": count,
            })
        })
        .collect();

    serde_json::json!({
        "keywords": keywords,
        "count": keywords.len(),
    })
}

fn op_word_frequency(text: &str) -> serde_json::Value {
    let freq = word_frequencies(text);
    let mut pairs: Vec<(String, usize)> = freq.into_iter().collect();
    pairs.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    let entries: serde_json::Map<String, serde_json::Value> = pairs
        .into_iter()
        .map(|(word, count)| (word, serde_json::json!(count)))
        .collect();

    serde_json::json!({
        "frequencies": entries,
        "unique_words": entries.len(),
    })
}

fn op_readability(text: &str) -> serde_json::Value {
    let sentences = split_sentences(text);
    let words = extract_words(text);
    let sentence_count = sentences.len();
    let word_count = words.len();

    let avg_words_per_sentence = if sentence_count > 0 {
        word_count as f64 / sentence_count as f64
    } else {
        0.0
    };

    let total_word_length: usize = words.iter().map(std::string::String::len).sum();
    let avg_word_length = if word_count > 0 {
        total_word_length as f64 / word_count as f64
    } else {
        0.0
    };

    // Estimated reading time: average adult reads ~200-250 words per minute
    let reading_time_seconds = (word_count as f64 / 225.0 * 60.0).round() as u64;

    serde_json::json!({
        "sentence_count": sentence_count,
        "word_count": word_count,
        "avg_words_per_sentence": (avg_words_per_sentence * 100.0).round() / 100.0,
        "avg_word_length": (avg_word_length * 100.0).round() / 100.0,
        "estimated_reading_time_seconds": reading_time_seconds,
    })
}

// ---------------------------------------------------------------------------
// Skill implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Skill for SummarizerSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = match call.arguments["operation"].as_str() {
            Some(op) => op,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'operation'",
                ))
            }
        };

        let text = match call.arguments["text"].as_str() {
            Some(t) => t.to_string(),
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'text'",
                ))
            }
        };

        match operation {
            "summarize" => {
                let max_sentences = call.arguments["max_sentences"]
                    .as_u64()
                    .unwrap_or(3)
                    .min(10) as usize;
                let title = call.arguments["title"].as_str();
                let result = op_summarize(&text, max_sentences, title);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "extract_keywords" => {
                let max_keywords = call.arguments["max_keywords"]
                    .as_u64()
                    .unwrap_or(10) as usize;
                let result = op_extract_keywords(&text, max_keywords);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "word_frequency" => {
                let result = op_word_frequency(&text);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "readability" => {
                let result = op_readability(&text);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation: '{operation}'. Supported: summarize, extract_keywords, word_frequency, readability"
                ),
            )),
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

    fn skill() -> SummarizerSkill {
        SummarizerSkill::new()
    }

    fn make_call(op: &str, args: serde_json::Value) -> ToolCall {
        let mut merged = args.clone();
        merged["operation"] = serde_json::json!(op);
        ToolCall {
            id: "test".to_string(),
            name: "summarizer".to_string(),
            arguments: merged,
        }
    }

    const SAMPLE_TEXT: &str = "\
Rust is a systems programming language focused on safety and performance. \
It eliminates many common bugs at compile time through its ownership system. \
The borrow checker ensures memory safety without a garbage collector. \
Rust has been voted the most loved programming language for several years. \
Many companies are adopting Rust for critical infrastructure. \
The ecosystem includes Cargo as its package manager and build tool. \
Async programming in Rust enables high-performance concurrent applications.";

    // -- Descriptor ----------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let s = skill();
        assert_eq!(s.descriptor().name, "summarizer");
        assert!(s.descriptor().required_capabilities.is_empty());
    }

    #[test]
    fn test_default() {
        let s = SummarizerSkill::default();
        assert_eq!(s.descriptor().name, "summarizer");
    }

    // -- summarize -----------------------------------------------------------

    #[tokio::test]
    async fn test_summarize_default() {
        let s = skill();
        let c = make_call("summarize", serde_json::json!({"text": SAMPLE_TEXT}));
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["sentence_count"], 3);
        assert!(!v["summary"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_summarize_custom_count() {
        let s = skill();
        let c = make_call(
            "summarize",
            serde_json::json!({"text": SAMPLE_TEXT, "max_sentences": 2}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["sentence_count"], 2);
    }

    #[tokio::test]
    async fn test_summarize_with_title() {
        let s = skill();
        let c = make_call(
            "summarize",
            serde_json::json!({
                "text": SAMPLE_TEXT,
                "title": "Rust Programming Language",
                "max_sentences": 3
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let summary = v["summary"].as_str().unwrap();
        // Title-related sentences should be preferred
        assert!(summary.to_lowercase().contains("rust"));
    }

    #[tokio::test]
    async fn test_summarize_empty_text() {
        let s = skill();
        let c = make_call("summarize", serde_json::json!({"text": ""}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["sentence_count"], 0);
        assert_eq!(v["summary"], "");
    }

    #[tokio::test]
    async fn test_summarize_single_sentence() {
        let s = skill();
        let c = make_call(
            "summarize",
            serde_json::json!({"text": "This is one sentence."}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["sentence_count"], 1);
    }

    #[tokio::test]
    async fn test_summarize_preserves_order() {
        let s = skill();
        let c = make_call(
            "summarize",
            serde_json::json!({
                "text": SAMPLE_TEXT,
                "max_sentences": 3
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let sentences = v["sentences"].as_array().unwrap();
        // Indices should be in ascending order
        let indices: Vec<u64> = sentences
            .iter()
            .map(|s| s["index"].as_u64().unwrap())
            .collect();
        for window in indices.windows(2) {
            assert!(window[0] < window[1], "Sentences not in original order");
        }
    }

    #[tokio::test]
    async fn test_summarize_max_capped_at_10() {
        let s = skill();
        let c = make_call(
            "summarize",
            serde_json::json!({"text": SAMPLE_TEXT, "max_sentences": 100}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let count = v["sentence_count"].as_u64().unwrap();
        assert!(count <= 10);
    }

    // -- extract_keywords ----------------------------------------------------

    #[tokio::test]
    async fn test_extract_keywords() {
        let s = skill();
        let c = make_call("extract_keywords", serde_json::json!({"text": SAMPLE_TEXT}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let keywords = v["keywords"].as_array().unwrap();
        assert!(!keywords.is_empty());
        // "rust" should be a top keyword
        let keyword_words: Vec<&str> = keywords
            .iter()
            .map(|k| k["word"].as_str().unwrap())
            .collect();
        assert!(keyword_words.contains(&"rust"));
    }

    #[tokio::test]
    async fn test_extract_keywords_custom_max() {
        let s = skill();
        let c = make_call(
            "extract_keywords",
            serde_json::json!({"text": SAMPLE_TEXT, "max_keywords": 3}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["count"].as_u64().unwrap() <= 3);
    }

    #[tokio::test]
    async fn test_extract_keywords_excludes_stop_words() {
        let s = skill();
        let c = make_call(
            "extract_keywords",
            serde_json::json!({"text": "the the the is is are"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["count"], 0);
    }

    // -- word_frequency ------------------------------------------------------

    #[tokio::test]
    async fn test_word_frequency() {
        let s = skill();
        let c = make_call(
            "word_frequency",
            serde_json::json!({"text": "hello world hello rust rust rust"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let freq = &v["frequencies"];
        assert_eq!(freq["rust"], 3);
        assert_eq!(freq["hello"], 2);
        assert_eq!(freq["world"], 1);
    }

    #[tokio::test]
    async fn test_word_frequency_case_insensitive() {
        let s = skill();
        let c = make_call(
            "word_frequency",
            serde_json::json!({"text": "Rust rust RUST"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["frequencies"]["rust"], 3);
    }

    // -- readability ---------------------------------------------------------

    #[tokio::test]
    async fn test_readability() {
        let s = skill();
        let c = make_call("readability", serde_json::json!({"text": SAMPLE_TEXT}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["sentence_count"].as_u64().unwrap() > 0);
        assert!(v["word_count"].as_u64().unwrap() > 0);
        assert!(v["avg_words_per_sentence"].as_f64().unwrap() > 0.0);
        assert!(v["avg_word_length"].as_f64().unwrap() > 0.0);
        assert!(v["estimated_reading_time_seconds"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_readability_empty() {
        let s = skill();
        let c = make_call("readability", serde_json::json!({"text": ""}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["sentence_count"], 0);
        assert_eq!(v["word_count"], 0);
    }

    // -- Error handling ------------------------------------------------------

    #[tokio::test]
    async fn test_missing_operation() {
        let s = skill();
        let c = ToolCall {
            id: "test".to_string(),
            name: "summarizer".to_string(),
            arguments: serde_json::json!({"text": "hello"}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_missing_text() {
        let s = skill();
        let c = ToolCall {
            id: "test".to_string(),
            name: "summarizer".to_string(),
            arguments: serde_json::json!({"operation": "summarize"}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("text"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let s = skill();
        let c = make_call("bogus", serde_json::json!({"text": "hello"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    // -- Utility unit tests --------------------------------------------------

    #[test]
    fn test_split_sentences_basic() {
        let sentences = split_sentences("Hello world. How are you? I am fine!");
        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[0], "Hello world.");
        assert_eq!(sentences[1], "How are you?");
        assert_eq!(sentences[2], "I am fine!");
    }

    #[test]
    fn test_split_sentences_no_period() {
        let sentences = split_sentences("Hello world");
        assert_eq!(sentences.len(), 1);
    }

    #[test]
    fn test_is_stop_word() {
        assert!(is_stop_word("the"));
        assert!(is_stop_word("The"));
        assert!(is_stop_word("THE"));
        assert!(!is_stop_word("rust"));
    }

    #[test]
    fn test_extract_words() {
        let words = extract_words("Hello, World! Test-123.");
        assert!(words.contains(&"hello".to_string()));
        assert!(words.contains(&"world".to_string()));
    }
}

//! Retrieval-Augmented Generation (RAG) pipeline for knowledge base search.
//!
//! Provides document ingestion (chunking + embedding + storage) and
//! context-aware retrieval for LLM injection.
//!
//! # Main types
//!
//! - [`RagPipeline`] — Orchestrates ingestion and retrieval.
//! - [`Document`] — A document to ingest into the knowledge base.
//! - [`DocumentChunk`] — A chunk of a document after splitting.
//! - [`ChunkingStrategy`] — How to split documents into chunks.
//! - [`RagConfig`] — Pipeline configuration.
//! - [`RagResult`] — Query result containing scored chunks and formatted context.
//! - [`ScoredChunk`] — A chunk paired with its relevance score.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use argentor_core::ArgentorResult;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use crate::embedding::EmbeddingProvider;
use crate::store::{MemoryEntry, VectorStore};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A document to ingest into the RAG knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique identifier for this document.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Full text content to be chunked.
    pub content: String,
    /// Origin of the document (e.g. "knowledge_base", "faq", "docs").
    pub source: String,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, String>,
    /// Optional classification category.
    pub category: Option<String>,
}

/// A chunk produced by splitting a [`Document`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    /// Unique identifier for this chunk (typically `{document_id}_chunk_{index}`).
    pub chunk_id: String,
    /// The document this chunk belongs to.
    pub document_id: String,
    /// The text content of this chunk.
    pub content: String,
    /// Zero-based index of this chunk within the parent document.
    pub chunk_index: usize,
    /// Rough estimate of the number of tokens in this chunk.
    pub token_estimate: usize,
}

/// Strategy for splitting documents into chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChunkingStrategy {
    /// Fixed-size chunks measured in characters, with optional overlap.
    FixedSize {
        /// Maximum number of characters per chunk.
        chunk_size: usize,
        /// Number of overlapping characters between consecutive chunks.
        overlap: usize,
    },
    /// Split on paragraph boundaries (double newlines).
    Paragraph,
    /// Split on sentence boundaries (`.` / `!` / `?` followed by whitespace).
    Sentence,
    /// Split on heading boundaries (lines starting with `#`), with a
    /// maximum token budget per chunk.
    Semantic {
        /// Maximum estimated tokens per chunk.
        max_chunk_tokens: usize,
    },
}

/// Configuration for the RAG pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagConfig {
    /// How to split ingested documents.
    pub chunking: ChunkingStrategy,
    /// Default number of top results to return.
    pub top_k: usize,
    /// Minimum relevance score (0.0 – 1.0) for a chunk to be included.
    pub min_relevance_score: f32,
    /// Whether to include document metadata in the formatted context.
    pub include_metadata: bool,
    /// Maximum estimated tokens for the combined context window.
    pub max_context_tokens: usize,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            chunking: ChunkingStrategy::FixedSize {
                chunk_size: 512,
                overlap: 64,
            },
            top_k: 5,
            min_relevance_score: 0.3,
            include_metadata: true,
            max_context_tokens: 4096,
        }
    }
}

/// A scored chunk returned by the RAG query.
#[derive(Debug, Clone)]
pub struct ScoredChunk {
    /// The document chunk.
    pub chunk: DocumentChunk,
    /// Relevance score (higher is better).
    pub score: f32,
    /// Title of the parent document.
    pub document_title: String,
    /// Source of the parent document.
    pub source: String,
}

/// Result of a RAG query.
#[derive(Debug, Clone)]
pub struct RagResult {
    /// Scored chunks ordered by relevance.
    pub chunks: Vec<ScoredChunk>,
    /// Pre-formatted context text ready for LLM injection.
    pub context_text: String,
    /// Total number of chunks that were searched.
    pub total_chunks_searched: usize,
    /// Wall-clock time in milliseconds for the query.
    pub query_time_ms: u64,
}

// ---------------------------------------------------------------------------
// Chunking helpers
// ---------------------------------------------------------------------------

/// Estimate token count from a string (≈ 1 token per 4 characters).
fn estimate_tokens(text: &str) -> usize {
    // Simple heuristic: ~4 chars per token for English text.
    text.len().div_ceil(4)
}

/// Split a document into chunks according to the given strategy.
fn chunk_document(doc: &Document, strategy: &ChunkingStrategy) -> Vec<DocumentChunk> {
    let raw_chunks = match strategy {
        ChunkingStrategy::FixedSize {
            chunk_size,
            overlap,
        } => chunk_fixed_size(&doc.content, *chunk_size, *overlap),
        ChunkingStrategy::Paragraph => chunk_paragraph(&doc.content),
        ChunkingStrategy::Sentence => chunk_sentence(&doc.content),
        ChunkingStrategy::Semantic { max_chunk_tokens } => {
            chunk_semantic(&doc.content, *max_chunk_tokens)
        }
    };

    raw_chunks
        .into_iter()
        .enumerate()
        .map(|(idx, text)| DocumentChunk {
            chunk_id: format!("{}_chunk_{}", doc.id, idx),
            document_id: doc.id.clone(),
            content: text,
            chunk_index: idx,
            token_estimate: 0, // filled below
        })
        .map(|mut c| {
            c.token_estimate = estimate_tokens(&c.content);
            c
        })
        .collect()
}

/// Fixed-size chunking with character-level overlap.
fn chunk_fixed_size(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() || chunk_size == 0 {
        return vec![];
    }
    let effective_overlap = overlap.min(chunk_size.saturating_sub(1));
    let step = chunk_size.saturating_sub(effective_overlap).max(1);
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        let trimmed = chunk.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(trimmed);
        }
        if end == chars.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// Split on paragraph boundaries (two or more consecutive newlines).
fn chunk_paragraph(text: &str) -> Vec<String> {
    let parts: Vec<String> = text
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        // Fallback: return the whole text as a single chunk if non-empty.
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            vec![]
        } else {
            vec![trimmed]
        }
    } else {
        parts
    }
}

/// Split on sentence boundaries.
fn chunk_sentence(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    for i in 0..len {
        current.push(chars[i]);
        let is_terminal = matches!(chars[i], '.' | '!' | '?');
        let followed_by_space = i + 1 < len && chars[i + 1].is_whitespace();
        if is_terminal && (followed_by_space || i + 1 == len) {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
        }
    }
    // Remaining text
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }
    sentences
}

/// Semantic chunking: split on Markdown heading boundaries (`# ...`),
/// merging small sections up to `max_chunk_tokens`.
fn chunk_semantic(text: &str, max_chunk_tokens: usize) -> Vec<String> {
    let max_tokens = if max_chunk_tokens == 0 {
        256
    } else {
        max_chunk_tokens
    };

    let mut sections: Vec<String> = Vec::new();
    let mut current_section = String::new();

    for line in text.lines() {
        let is_heading = line.starts_with('#');
        if is_heading && !current_section.trim().is_empty() {
            sections.push(current_section.trim().to_string());
            current_section.clear();
        }
        if !current_section.is_empty() {
            current_section.push('\n');
        }
        current_section.push_str(line);
    }
    if !current_section.trim().is_empty() {
        sections.push(current_section.trim().to_string());
    }

    // Merge tiny sections that fit within the token budget.
    let mut merged: Vec<String> = Vec::new();
    let mut buffer = String::new();

    for section in sections {
        let combined_tokens = estimate_tokens(&buffer) + estimate_tokens(&section);
        if buffer.is_empty() {
            buffer = section;
        } else if combined_tokens <= max_tokens {
            buffer.push_str("\n\n");
            buffer.push_str(&section);
        } else {
            merged.push(buffer.trim().to_string());
            buffer = section;
        }
    }
    if !buffer.trim().is_empty() {
        merged.push(buffer.trim().to_string());
    }

    if merged.is_empty() {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            vec![]
        } else {
            vec![trimmed]
        }
    } else {
        merged
    }
}

// ---------------------------------------------------------------------------
// RagPipeline
// ---------------------------------------------------------------------------

/// Retrieval-Augmented Generation pipeline.
///
/// Orchestrates document ingestion (chunking, embedding, storage) and
/// context-aware retrieval for LLM injection.
pub struct RagPipeline {
    vector_store: Arc<dyn VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    config: RagConfig,
    /// In-memory index mapping `MemoryEntry.id` → `(DocumentChunk, doc_title, doc_source)`.
    chunk_index: tokio::sync::RwLock<HashMap<Uuid, (DocumentChunk, String, String)>>,
}

impl RagPipeline {
    /// Create a new RAG pipeline.
    pub fn new(
        vector_store: Arc<dyn VectorStore>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: RagConfig,
    ) -> Self {
        Self {
            vector_store,
            embedder,
            config,
            chunk_index: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Ingest a single document: chunk it, embed each chunk, and store.
    pub async fn ingest_document(&self, doc: &Document) -> ArgentorResult<Vec<DocumentChunk>> {
        let chunks = chunk_document(doc, &self.config.chunking);

        for chunk in &chunks {
            let embedding = self.embedder.embed(&chunk.content).await?;

            let mut metadata = HashMap::new();
            metadata.insert(
                "document_id".to_string(),
                serde_json::Value::String(doc.id.clone()),
            );
            metadata.insert(
                "document_title".to_string(),
                serde_json::Value::String(doc.title.clone()),
            );
            metadata.insert(
                "source".to_string(),
                serde_json::Value::String(doc.source.clone()),
            );
            metadata.insert(
                "chunk_id".to_string(),
                serde_json::Value::String(chunk.chunk_id.clone()),
            );
            metadata.insert(
                "chunk_index".to_string(),
                serde_json::json!(chunk.chunk_index),
            );
            if let Some(cat) = &doc.category {
                metadata.insert(
                    "category".to_string(),
                    serde_json::Value::String(cat.clone()),
                );
            }
            if self.config.include_metadata {
                for (k, v) in &doc.metadata {
                    metadata.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
            }

            let entry_id = Uuid::new_v4();
            let entry = MemoryEntry {
                id: entry_id,
                content: chunk.content.clone(),
                embedding,
                metadata,
                session_id: None,
                created_at: Utc::now(),
            };

            self.vector_store.insert(entry).await?;

            // Track the chunk in our local index.
            let mut idx = self.chunk_index.write().await;
            idx.insert(
                entry_id,
                (chunk.clone(), doc.title.clone(), doc.source.clone()),
            );
        }

        Ok(chunks)
    }

    /// Batch-ingest multiple documents.
    pub async fn ingest_batch(&self, docs: &[Document]) -> ArgentorResult<Vec<Vec<DocumentChunk>>> {
        let mut all_chunks = Vec::with_capacity(docs.len());
        for doc in docs {
            let chunks = self.ingest_document(doc).await?;
            all_chunks.push(chunks);
        }
        Ok(all_chunks)
    }

    /// Query the knowledge base and return scored, filtered chunks.
    #[instrument(
        skip(self, question),
        fields(
            query_length = question.len(),
            chunks_searched = tracing::field::Empty,
            results_count = tracing::field::Empty,
        )
    )]
    pub async fn query(&self, question: &str, top_k: Option<usize>) -> ArgentorResult<RagResult> {
        let start = Instant::now();
        let k = top_k.unwrap_or(self.config.top_k);

        let query_embedding = self.embedder.embed(question).await?;

        let total_chunks_searched = self.vector_store.count().await?;

        // Retrieve more candidates than requested so we can filter by score.
        let fetch_k = (k * 3).max(k);
        let results = self
            .vector_store
            .search(&query_embedding, fetch_k, None)
            .await?;

        let idx = self.chunk_index.read().await;

        let mut scored: Vec<ScoredChunk> = results
            .into_iter()
            .filter(|r| r.score >= self.config.min_relevance_score)
            .filter_map(|r| {
                let (chunk, title, source) = idx.get(&r.entry.id)?;
                Some(ScoredChunk {
                    chunk: chunk.clone(),
                    score: r.score,
                    document_title: title.clone(),
                    source: source.clone(),
                })
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);

        let context_text = format_context(&scored, self.config.max_context_tokens);

        let elapsed = start.elapsed().as_millis() as u64;

        tracing::Span::current()
            .record("chunks_searched", total_chunks_searched)
            .record("results_count", scored.len());

        Ok(RagResult {
            chunks: scored,
            context_text,
            total_chunks_searched,
            query_time_ms: elapsed,
        })
    }

    /// Query and return a formatted context string sized to fit a given
    /// context window (in estimated tokens).
    pub async fn query_with_context(
        &self,
        question: &str,
        top_k: Option<usize>,
        context_window: usize,
    ) -> ArgentorResult<RagResult> {
        let mut result = self.query(question, top_k).await?;
        // Re-format the context with the caller-specified window size.
        result.context_text = format_context(&result.chunks, context_window);
        Ok(result)
    }

    /// Return a reference to the pipeline configuration.
    pub fn config(&self) -> &RagConfig {
        &self.config
    }
}

/// Format scored chunks into a context string for LLM injection,
/// respecting a maximum token budget.
fn format_context(chunks: &[ScoredChunk], max_tokens: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut token_budget = max_tokens;

    for (i, sc) in chunks.iter().enumerate() {
        let header = format!(
            "[Source: {} | Document: {} | Score: {:.2}]",
            sc.source, sc.document_title, sc.score
        );
        let section = format!("--- Chunk {} ---\n{}\n{}", i + 1, header, sc.chunk.content);
        let section_tokens = estimate_tokens(&section);
        if section_tokens > token_budget {
            // Try to include a truncated version if there is room.
            if token_budget > 20 {
                let available_chars = token_budget * 4;
                let truncated: String = section.chars().take(available_chars).collect();
                parts.push(truncated);
            }
            break;
        }
        token_budget = token_budget.saturating_sub(section_tokens);
        parts.push(section);
    }

    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::embedding::LocalEmbedding;
    use crate::store::InMemoryVectorStore;

    // -- Helpers --

    fn sample_doc(id: &str, title: &str, content: &str) -> Document {
        Document {
            id: id.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            source: "test".to_string(),
            metadata: HashMap::new(),
            category: None,
        }
    }

    fn make_pipeline(config: RagConfig) -> RagPipeline {
        let store = Arc::new(InMemoryVectorStore::new()) as Arc<dyn VectorStore>;
        let embedder = Arc::new(LocalEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        RagPipeline::new(store, embedder, config)
    }

    fn default_pipeline() -> RagPipeline {
        make_pipeline(RagConfig::default())
    }

    // -----------------------------------------------------------------------
    // Chunking unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_chunk_fixed_size_basic() {
        let chunks = chunk_fixed_size("abcdefghij", 4, 0);
        assert_eq!(chunks.len(), 3); // "abcd", "efgh", "ij"
        assert_eq!(chunks[0], "abcd");
        assert_eq!(chunks[1], "efgh");
        assert_eq!(chunks[2], "ij");
    }

    #[test]
    fn test_chunk_fixed_size_with_overlap() {
        let chunks = chunk_fixed_size("abcdefghij", 5, 2);
        // step = 5 - 2 = 3, windows: [0..5], [3..8], [6..10]
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "abcde");
        assert_eq!(chunks[1], "defgh");
        assert_eq!(chunks[2], "ghij");
    }

    #[test]
    fn test_chunk_fixed_size_empty_text() {
        let chunks = chunk_fixed_size("", 10, 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_fixed_size_zero_size() {
        let chunks = chunk_fixed_size("hello", 0, 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_fixed_size_text_shorter_than_chunk() {
        let chunks = chunk_fixed_size("hi", 100, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hi");
    }

    #[test]
    fn test_chunk_paragraph_basic() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_paragraph(text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "First paragraph.");
        assert_eq!(chunks[1], "Second paragraph.");
        assert_eq!(chunks[2], "Third paragraph.");
    }

    #[test]
    fn test_chunk_paragraph_empty() {
        let chunks = chunk_paragraph("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_paragraph_single() {
        let text = "Just one paragraph with no double newlines.";
        let chunks = chunk_paragraph(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_chunk_sentence_basic() {
        let text = "First sentence. Second sentence! Third sentence?";
        let chunks = chunk_sentence(text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "First sentence.");
        assert_eq!(chunks[1], "Second sentence!");
        assert_eq!(chunks[2], "Third sentence?");
    }

    #[test]
    fn test_chunk_sentence_no_terminal() {
        let text = "No terminal punctuation here";
        let chunks = chunk_sentence(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_chunk_sentence_empty() {
        let chunks = chunk_sentence("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_semantic_headings() {
        let text = "# Heading 1\nParagraph one.\n# Heading 2\nParagraph two.";
        let chunks = chunk_semantic(text, 1000);
        // Two sections: "# Heading 1\nParagraph one." and "# Heading 2\nParagraph two."
        // They fit within 1000 tokens, so they may be merged.
        assert!(!chunks.is_empty());
        // With a huge token budget they get merged into one.
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_chunk_semantic_splits_on_budget() {
        let text = "# A\nLorem ipsum dolor sit amet.\n# B\nConsectetur adipiscing elit.";
        // Very small budget forces a split.
        let chunks = chunk_semantic(text, 8);
        assert!(chunks.len() >= 2, "small budget should force split");
    }

    #[test]
    fn test_chunk_semantic_empty() {
        let chunks = chunk_semantic("", 100);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        // 12 chars -> 3 tokens
        assert_eq!(estimate_tokens("abcdefghijkl"), 3);
    }

    // -----------------------------------------------------------------------
    // Document model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_document_chunk_fields() {
        let chunk = DocumentChunk {
            chunk_id: "doc1_chunk_0".into(),
            document_id: "doc1".into(),
            content: "hello world".into(),
            chunk_index: 0,
            token_estimate: 3,
        };
        assert_eq!(chunk.chunk_id, "doc1_chunk_0");
        assert_eq!(chunk.document_id, "doc1");
        assert_eq!(chunk.chunk_index, 0);
        assert_eq!(chunk.token_estimate, 3);
    }

    #[test]
    fn test_rag_config_default() {
        let cfg = RagConfig::default();
        assert_eq!(cfg.top_k, 5);
        assert!((cfg.min_relevance_score - 0.3).abs() < f32::EPSILON);
        assert!(cfg.include_metadata);
        assert_eq!(cfg.max_context_tokens, 4096);
    }

    // -----------------------------------------------------------------------
    // Integration / pipeline tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_ingest_document_creates_chunks() {
        let pipeline = default_pipeline();
        let doc = sample_doc("d1", "Test Doc", "Hello world. This is a test document.");
        let chunks = pipeline.ingest_document(&doc).await.unwrap();
        assert!(!chunks.is_empty(), "should produce at least one chunk");
        // Each chunk should reference the parent document.
        for c in &chunks {
            assert_eq!(c.document_id, "d1");
            assert!(!c.content.is_empty());
            assert!(c.token_estimate > 0);
        }
    }

    #[tokio::test]
    async fn test_ingest_batch() {
        let pipeline = default_pipeline();
        let docs = vec![
            sample_doc("d1", "Doc One", "Content of document one."),
            sample_doc("d2", "Doc Two", "Content of document two."),
        ];
        let all = pipeline.ingest_batch(&docs).await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(!all[0].is_empty());
        assert!(!all[1].is_empty());
    }

    #[tokio::test]
    async fn test_query_returns_results() {
        let pipeline = default_pipeline();
        let doc = sample_doc(
            "d1",
            "Rust Book",
            "Rust is a systems programming language focused on safety and performance.",
        );
        pipeline.ingest_document(&doc).await.unwrap();

        let result = pipeline.query("rust programming", None).await.unwrap();
        assert!(!result.chunks.is_empty(), "query should return results");
        assert!(result.query_time_ms < 10_000, "query should be fast");
        assert!(
            result.total_chunks_searched > 0,
            "should report chunks searched"
        );
    }

    #[tokio::test]
    async fn test_query_context_text_not_empty() {
        let pipeline = default_pipeline();
        let doc = sample_doc("d1", "FAQ", "How do I install Rust? Use rustup.");
        pipeline.ingest_document(&doc).await.unwrap();

        let result = pipeline.query("install rust", None).await.unwrap();
        assert!(
            !result.context_text.is_empty(),
            "context_text should be populated"
        );
        assert!(
            result.context_text.contains("Chunk 1"),
            "context should include chunk header"
        );
    }

    #[tokio::test]
    async fn test_query_with_context_window() {
        let cfg = RagConfig {
            min_relevance_score: 0.0, // accept any score so the test is deterministic
            ..RagConfig::default()
        };
        let pipeline = make_pipeline(cfg);
        let doc = sample_doc(
            "d1",
            "Long Doc",
            "Rust programming language. Memory safety without garbage collection. Zero-cost abstractions.",
        );
        pipeline.ingest_document(&doc).await.unwrap();

        let result = pipeline
            .query_with_context("rust programming language", None, 8192)
            .await
            .unwrap();
        assert!(!result.context_text.is_empty());
    }

    #[tokio::test]
    async fn test_query_min_relevance_filter() {
        let cfg = RagConfig {
            min_relevance_score: 0.99, // very high threshold
            ..RagConfig::default()
        };
        let pipeline = make_pipeline(cfg);
        let doc = sample_doc("d1", "Doc", "some random content about various topics");
        pipeline.ingest_document(&doc).await.unwrap();

        let result = pipeline
            .query("completely unrelated xyz", None)
            .await
            .unwrap();
        // With a 0.99 threshold most results should be filtered out.
        // We don't assert exact count because LocalEmbedding is approximate.
        assert!(
            result.chunks.len() <= 1,
            "high threshold should filter most results"
        );
    }

    #[tokio::test]
    async fn test_scored_chunk_has_metadata() {
        let pipeline = default_pipeline();
        let doc = sample_doc("d1", "My Title", "Content about Rust programming.");
        pipeline.ingest_document(&doc).await.unwrap();

        let result = pipeline.query("rust", None).await.unwrap();
        if let Some(sc) = result.chunks.first() {
            assert_eq!(sc.document_title, "My Title");
            assert_eq!(sc.source, "test");
            assert!(sc.score > 0.0);
        }
    }

    #[tokio::test]
    async fn test_config_accessor() {
        let pipeline = default_pipeline();
        assert_eq!(pipeline.config().top_k, 5);
    }

    #[test]
    fn test_format_context_empty() {
        let ctx = format_context(&[], 4096);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_format_context_includes_source() {
        let chunks = vec![ScoredChunk {
            chunk: DocumentChunk {
                chunk_id: "c1".into(),
                document_id: "d1".into(),
                content: "Hello".into(),
                chunk_index: 0,
                token_estimate: 2,
            },
            score: 0.95,
            document_title: "Title".into(),
            source: "kb".into(),
        }];
        let ctx = format_context(&chunks, 4096);
        assert!(ctx.contains("kb"));
        assert!(ctx.contains("Title"));
        assert!(ctx.contains("0.95"));
        assert!(ctx.contains("Hello"));
    }
}

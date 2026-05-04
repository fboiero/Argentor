//! Vector-based semantic memory with hybrid search and query expansion.
//!
//! Provides persistent vector storage, local embedding generation,
//! BM25 keyword scoring, hybrid (embedding + BM25) search, and
//! rule-based query expansion for improved recall.
//!
//! # Main types
//!
//! - [`VectorStore`] — Trait for storing and querying embedding vectors.
//! - [`FileVectorStore`] — File-backed persistent vector store.
//! - [`LocalEmbedding`] — Local TF-IDF-based embedding provider.
//! - [`HybridSearcher`] — Combines embedding similarity with BM25 keyword scoring.
//! - [`Bm25Index`] — BM25 inverted index for keyword-based retrieval.
//! - [`QueryExpander`] — Trait for expanding queries to improve search recall.
//! - [`RagPipeline`] — Retrieval-Augmented Generation pipeline for knowledge base search.

/// BM25 inverted index for keyword-based retrieval.
pub mod bm25;
/// Conversation memory for cross-session customer context.
pub mod conversation;
/// Embedding provider trait and local implementation.
pub mod embedding;
/// Multiple embedding provider backends (OpenAI, Cohere, Voyage, cached, batch, factory).
pub mod embeddings_providers;
/// Hybrid search combining embeddings and BM25.
pub mod hybrid;
/// Knowledge graph for entity-relationship-based memory.
pub mod knowledge_graph;
/// pgvector (PostgreSQL extension) vector store adapter.
pub mod pgvector;
/// Pinecone vector store adapter.
pub mod pinecone;
/// Qdrant vector store adapter.
pub mod qdrant;
/// Query expansion for improved recall.
pub mod query_expansion;
/// Retrieval-Augmented Generation pipeline.
pub mod rag;
/// Vector store trait and file-backed implementation.
pub mod store;
/// Multi-tier memory: short-term, long-term (episodic), and entity memory.
pub mod tiered;
/// Weaviate vector store adapter.
pub mod weaviate;

pub use bm25::Bm25Index;
pub use conversation::{
    ConversationMemory, ConversationSummarizer, ConversationTurn, CustomerProfile,
};
pub use embedding::{EmbeddingProvider, LocalEmbedding};
pub use embeddings_providers::{
    parse_cohere_embedding_response, parse_openai_embedding_response,
    parse_voyage_embedding_response, BatchEmbeddingProvider, CacheStats, CachedEmbeddingProvider,
    CohereEmbedV4Provider, CohereEmbeddingProvider, EmbeddingConfig, EmbeddingProviderFactory,
    JinaEmbeddingProvider, MistralEmbedProvider, NomicEmbedProvider, OpenAiEmbeddingProvider,
    SentenceTransformersProvider, TogetherEmbedProvider, VoyageEmbeddingProvider,
};
pub use hybrid::HybridSearcher;
pub use knowledge_graph::{
    Entity, EntityType, GraphSummary, KnowledgeGraph, RelationType, Relationship,
};
pub use pgvector::PgVectorStore;
pub use pinecone::PineconeStore;
pub use qdrant::QdrantStore;
pub use query_expansion::{QueryExpander, RuleBasedExpander};
pub use rag::{
    ChunkingStrategy, Document, DocumentChunk, RagConfig, RagPipeline, RagResult, ScoredChunk,
};
pub use store::{FileVectorStore, InMemoryVectorStore, MemoryEntry, SearchResult, VectorStore};
pub use tiered::{MemoryContext, ScoredMemory, TieredMemory, TieredMemoryConfig, TieredTurn};
pub use weaviate::WeaviateStore;

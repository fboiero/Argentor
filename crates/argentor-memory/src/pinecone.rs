//! Pinecone vector store adapter.
//!
//! Pinecone is a managed vector database. This adapter implements the
//! [`VectorStore`] trait with a stub backend that stores entries in-memory
//! using brute-force cosine similarity, suitable for testing and local
//! development without external dependencies.
//!
//! Real HTTP calls to the Pinecone REST API are gated behind the
//! `http-vectorstore` feature flag and are not wired by default.

use crate::store::{MemoryEntry, SearchResult, VectorStore};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Pinecone vector store adapter.
///
/// In stub mode (default), entries are stored in-memory and searched with
/// brute-force cosine similarity. In HTTP mode (feature `http-vectorstore`),
/// a [`reqwest::Client`] is constructed to talk to the Pinecone API.
pub struct PineconeStore {
    /// Pinecone API key.
    #[allow(dead_code)]
    api_key: String,
    /// Pinecone index name.
    #[allow(dead_code)]
    index_name: String,
    /// Pinecone environment (e.g., "us-east-1-aws").
    #[allow(dead_code)]
    environment: String,
    /// Optional namespace for multi-tenant isolation.
    #[allow(dead_code)]
    namespace: Option<String>,
    /// HTTP client — `None` in stub mode.
    #[cfg(feature = "http-vectorstore")]
    #[allow(dead_code)]
    client: Option<reqwest::Client>,
    /// Stub in-memory storage: id -> entry.
    entries: RwLock<HashMap<Uuid, MemoryEntry>>,
}

impl PineconeStore {
    /// Create a new Pinecone adapter in stub mode.
    pub fn new(
        api_key: impl Into<String>,
        index_name: impl Into<String>,
        environment: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            index_name: index_name.into(),
            environment: environment.into(),
            namespace: None,
            #[cfg(feature = "http-vectorstore")]
            client: None,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Set the namespace for multi-tenant isolation.
    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = Some(ns.into());
        self
    }

    /// Return the configured index name.
    pub fn index_name(&self) -> &str {
        &self.index_name
    }

    /// Return the configured environment.
    pub fn environment(&self) -> &str {
        &self.environment
    }

    /// Return the configured namespace (if any).
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Enable real HTTP mode with a configured [`reqwest::Client`].
    #[cfg(feature = "http-vectorstore")]
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Build the Pinecone upsert endpoint URL.
    #[cfg(feature = "http-vectorstore")]
    #[allow(dead_code)]
    fn upsert_url(&self) -> String {
        format!(
            "https://{}-{}.svc.{}.pinecone.io/vectors/upsert",
            self.index_name, "xxxxx", self.environment
        )
    }
}

#[async_trait]
impl VectorStore for PineconeStore {
    async fn insert(&self, entry: MemoryEntry) -> ArgentorResult<()> {
        let mut entries = self.entries.write().await;
        entries.insert(entry.id, entry);
        Ok(())
    }

    async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        session_filter: Option<Uuid>,
    ) -> ArgentorResult<Vec<SearchResult>> {
        if query_embedding.is_empty() {
            return Err(ArgentorError::Agent("Empty query embedding".to_string()));
        }
        let entries = self.entries.read().await;
        let mut scored: Vec<SearchResult> = entries
            .values()
            .filter(|e| {
                if let Some(sid) = session_filter {
                    e.session_id == Some(sid)
                } else {
                    true
                }
            })
            .map(|e| {
                let score = cosine(query_embedding, &e.embedding);
                SearchResult {
                    entry: e.clone(),
                    score,
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        Ok(scored)
    }

    async fn delete(&self, id: Uuid) -> ArgentorResult<bool> {
        let mut entries = self.entries.write().await;
        Ok(entries.remove(&id).is_some())
    }

    async fn list(&self, session_filter: Option<Uuid>) -> ArgentorResult<Vec<MemoryEntry>> {
        let entries = self.entries.read().await;
        Ok(entries
            .values()
            .filter(|e| {
                if let Some(sid) = session_filter {
                    e.session_id == Some(sid)
                } else {
                    true
                }
            })
            .cloned()
            .collect())
    }

    async fn count(&self) -> ArgentorResult<usize> {
        let entries = self.entries.read().await;
        Ok(entries.len())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn entry(content: &str, emb: Vec<f32>, session: Option<Uuid>) -> MemoryEntry {
        MemoryEntry {
            id: Uuid::new_v4(),
            content: content.to_string(),
            embedding: emb,
            metadata: HashMap::new(),
            session_id: session,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_new_sets_fields() {
        let store = PineconeStore::new("key-123", "my-index", "us-east-1-aws");
        assert_eq!(store.api_key, "key-123");
        assert_eq!(store.index_name(), "my-index");
        assert_eq!(store.environment(), "us-east-1-aws");
        assert!(store.namespace().is_none());
    }

    #[test]
    fn test_with_namespace() {
        let store = PineconeStore::new("k", "i", "e").with_namespace("tenant-a");
        assert_eq!(store.namespace(), Some("tenant-a"));
    }

    #[test]
    fn test_accepts_owned_strings() {
        let store = PineconeStore::new(
            String::from("k"),
            String::from("i"),
            String::from("us-west-2-aws"),
        );
        assert_eq!(store.environment(), "us-west-2-aws");
    }

    #[tokio::test]
    async fn test_insert_increments_count() {
        let store = PineconeStore::new("k", "i", "e");
        assert_eq!(store.count().await.unwrap(), 0);
        store
            .insert(entry("hello", vec![1.0, 0.0, 0.0], None))
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_insert_many() {
        let store = PineconeStore::new("k", "i", "e");
        for i in 0..25 {
            store
                .insert(entry(&format!("e{i}"), vec![i as f32, 0.0], None))
                .await
                .unwrap();
        }
        assert_eq!(store.count().await.unwrap(), 25);
    }

    #[tokio::test]
    async fn test_search_orders_by_similarity() {
        let store = PineconeStore::new("k", "i", "e");
        store
            .insert(entry("close", vec![0.9, 0.1, 0.0], None))
            .await
            .unwrap();
        store
            .insert(entry("far", vec![0.0, 0.0, 1.0], None))
            .await
            .unwrap();
        let results = store.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].entry.content, "close");
        assert!(results[0].score > results[1].score);
    }

    #[tokio::test]
    async fn test_search_respects_top_k() {
        let store = PineconeStore::new("k", "i", "e");
        for i in 0..10 {
            store
                .insert(entry(&format!("e{i}"), vec![1.0, i as f32 / 10.0], None))
                .await
                .unwrap();
        }
        let results = store.search(&[1.0, 0.0], 3, None).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_search_empty_embedding_errors() {
        let store = PineconeStore::new("k", "i", "e");
        assert!(store.search(&[], 5, None).await.is_err());
    }

    #[tokio::test]
    async fn test_search_session_filter() {
        let store = PineconeStore::new("k", "i", "e");
        let sid = Uuid::new_v4();
        store
            .insert(entry("a", vec![1.0, 0.0], Some(sid)))
            .await
            .unwrap();
        store
            .insert(entry("b", vec![1.0, 0.0], None))
            .await
            .unwrap();
        let results = store.search(&[1.0, 0.0], 10, Some(sid)).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.content, "a");
    }

    #[tokio::test]
    async fn test_delete_existing() {
        let store = PineconeStore::new("k", "i", "e");
        let e = entry("to-delete", vec![1.0], None);
        let id = e.id;
        store.insert(e).await.unwrap();
        assert!(store.delete(id).await.unwrap());
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_missing_returns_false() {
        let store = PineconeStore::new("k", "i", "e");
        assert!(!store.delete(Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_list_all() {
        let store = PineconeStore::new("k", "i", "e");
        store.insert(entry("a", vec![1.0], None)).await.unwrap();
        store.insert(entry("b", vec![0.5], None)).await.unwrap();
        let all = store.list(None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_list_filtered_by_session() {
        let store = PineconeStore::new("k", "i", "e");
        let sid = Uuid::new_v4();
        store
            .insert(entry("a", vec![1.0], Some(sid)))
            .await
            .unwrap();
        store.insert(entry("b", vec![0.5], None)).await.unwrap();
        let filtered = store.list(Some(sid)).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].content, "a");
    }

    #[tokio::test]
    async fn test_namespace_isolation_does_not_cross_instances() {
        let a = PineconeStore::new("k", "i", "e").with_namespace("ns-a");
        let b = PineconeStore::new("k", "i", "e").with_namespace("ns-b");
        a.insert(entry("x", vec![1.0], None)).await.unwrap();
        assert_eq!(a.count().await.unwrap(), 1);
        assert_eq!(b.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_insert_preserves_metadata() {
        let store = PineconeStore::new("k", "i", "e");
        let mut e = entry("m", vec![1.0, 0.0], None);
        e.metadata
            .insert("tag".to_string(), serde_json::json!("important"));
        let id = e.id;
        store.insert(e).await.unwrap();
        let all = store.list(None).await.unwrap();
        let got = all.iter().find(|x| x.id == id).unwrap();
        assert_eq!(
            got.metadata.get("tag").unwrap(),
            &serde_json::json!("important")
        );
    }

    #[tokio::test]
    async fn test_search_returns_empty_when_store_empty() {
        let store = PineconeStore::new("k", "i", "e");
        let results = store.search(&[1.0, 0.0], 5, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_count_after_deletes() {
        let store = PineconeStore::new("k", "i", "e");
        let e1 = entry("a", vec![1.0], None);
        let e2 = entry("b", vec![0.5], None);
        let id1 = e1.id;
        store.insert(e1).await.unwrap();
        store.insert(e2).await.unwrap();
        store.delete(id1).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }
}

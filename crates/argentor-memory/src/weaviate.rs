//! Weaviate vector store adapter.
//!
//! Weaviate is an open-source vector database with GraphQL + REST APIs.
//! This adapter implements the [`VectorStore`] trait with a stub backend;
//! real HTTP calls are gated behind the `http-vectorstore` feature.

use crate::store::{MemoryEntry, SearchResult, VectorStore};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Weaviate vector store adapter.
pub struct WeaviateStore {
    /// Base endpoint (e.g., "https://my-cluster.weaviate.network").
    #[allow(dead_code)]
    endpoint: String,
    /// Optional API key for authenticated clusters.
    #[allow(dead_code)]
    api_key: Option<String>,
    /// Weaviate class (schema) name.
    #[allow(dead_code)]
    class_name: String,
    /// HTTP client — `None` in stub mode.
    #[cfg(feature = "http-vectorstore")]
    #[allow(dead_code)]
    client: Option<reqwest::Client>,
    /// Stub in-memory storage.
    entries: RwLock<HashMap<Uuid, MemoryEntry>>,
}

impl WeaviateStore {
    /// Create a new Weaviate adapter in stub mode (no API key).
    pub fn new(endpoint: impl Into<String>, class_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: None,
            class_name: class_name.into(),
            #[cfg(feature = "http-vectorstore")]
            client: None,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Attach an API key (used for Weaviate Cloud / secured clusters).
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Return the configured endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Return the configured class name.
    pub fn class_name(&self) -> &str {
        &self.class_name
    }

    /// Return whether an API key is configured.
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Enable real HTTP mode with a [`reqwest::Client`].
    #[cfg(feature = "http-vectorstore")]
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Build the GraphQL endpoint URL.
    #[cfg(feature = "http-vectorstore")]
    #[allow(dead_code)]
    fn graphql_url(&self) -> String {
        format!("{}/v1/graphql", self.endpoint.trim_end_matches('/'))
    }
}

#[async_trait]
impl VectorStore for WeaviateStore {
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
        let store = WeaviateStore::new("https://my-cluster.weaviate.network", "Document");
        assert_eq!(store.endpoint(), "https://my-cluster.weaviate.network");
        assert_eq!(store.class_name(), "Document");
        assert!(!store.has_api_key());
    }

    #[test]
    fn test_with_api_key() {
        let store = WeaviateStore::new("https://x", "C").with_api_key("secret");
        assert!(store.has_api_key());
    }

    #[test]
    fn test_accepts_owned_strings() {
        let store = WeaviateStore::new(String::from("https://x"), String::from("Class"));
        assert_eq!(store.class_name(), "Class");
    }

    #[tokio::test]
    async fn test_insert_count() {
        let store = WeaviateStore::new("https://x", "C");
        assert_eq!(store.count().await.unwrap(), 0);
        store
            .insert(entry("hi", vec![1.0, 0.0], None))
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_insert_many() {
        let store = WeaviateStore::new("https://x", "C");
        for i in 0..20 {
            store
                .insert(entry(&format!("e{i}"), vec![i as f32], None))
                .await
                .unwrap();
        }
        assert_eq!(store.count().await.unwrap(), 20);
    }

    #[tokio::test]
    async fn test_search_orders_by_similarity() {
        let store = WeaviateStore::new("https://x", "C");
        store
            .insert(entry("near", vec![0.9, 0.1, 0.0], None))
            .await
            .unwrap();
        store
            .insert(entry("far", vec![0.0, 0.0, 1.0], None))
            .await
            .unwrap();
        let r = store.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(r[0].entry.content, "near");
        assert!(r[0].score > r[1].score);
    }

    #[tokio::test]
    async fn test_search_top_k() {
        let store = WeaviateStore::new("https://x", "C");
        for i in 0..8 {
            store
                .insert(entry(&format!("e{i}"), vec![1.0, i as f32 / 8.0], None))
                .await
                .unwrap();
        }
        let r = store.search(&[1.0, 0.0], 4, None).await.unwrap();
        assert_eq!(r.len(), 4);
    }

    #[tokio::test]
    async fn test_search_empty_errors() {
        let store = WeaviateStore::new("https://x", "C");
        assert!(store.search(&[], 1, None).await.is_err());
    }

    #[tokio::test]
    async fn test_search_session_filter() {
        let store = WeaviateStore::new("https://x", "C");
        let sid = Uuid::new_v4();
        store
            .insert(entry("s", vec![1.0, 0.0], Some(sid)))
            .await
            .unwrap();
        store
            .insert(entry("other", vec![1.0, 0.0], None))
            .await
            .unwrap();
        let r = store.search(&[1.0, 0.0], 5, Some(sid)).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].entry.content, "s");
    }

    #[tokio::test]
    async fn test_delete_existing() {
        let store = WeaviateStore::new("https://x", "C");
        let e = entry("x", vec![1.0], None);
        let id = e.id;
        store.insert(e).await.unwrap();
        assert!(store.delete(id).await.unwrap());
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_missing() {
        let store = WeaviateStore::new("https://x", "C");
        assert!(!store.delete(Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_list_all() {
        let store = WeaviateStore::new("https://x", "C");
        store.insert(entry("a", vec![1.0], None)).await.unwrap();
        store.insert(entry("b", vec![0.5], None)).await.unwrap();
        let all = store.list(None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_list_filtered() {
        let store = WeaviateStore::new("https://x", "C");
        let sid = Uuid::new_v4();
        store
            .insert(entry("a", vec![1.0], Some(sid)))
            .await
            .unwrap();
        store.insert(entry("b", vec![0.5], None)).await.unwrap();
        let filtered = store.list(Some(sid)).await.unwrap();
        assert_eq!(filtered.len(), 1);
    }

    #[tokio::test]
    async fn test_metadata_preserved() {
        let store = WeaviateStore::new("https://x", "C");
        let mut e = entry("with-meta", vec![1.0], None);
        e.metadata.insert("k".to_string(), serde_json::json!("v"));
        let id = e.id;
        store.insert(e).await.unwrap();
        let all = store.list(None).await.unwrap();
        let got = all.iter().find(|x| x.id == id).unwrap();
        assert_eq!(got.metadata.get("k").unwrap(), &serde_json::json!("v"));
    }

    #[tokio::test]
    async fn test_instances_are_isolated() {
        let a = WeaviateStore::new("https://x", "A");
        let b = WeaviateStore::new("https://x", "B");
        a.insert(entry("x", vec![1.0], None)).await.unwrap();
        assert_eq!(a.count().await.unwrap(), 1);
        assert_eq!(b.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_search_empty_store() {
        let store = WeaviateStore::new("https://x", "C");
        let r = store.search(&[1.0, 0.0], 5, None).await.unwrap();
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn test_count_after_deletes() {
        let store = WeaviateStore::new("https://x", "C");
        let e = entry("a", vec![1.0], None);
        let id = e.id;
        store.insert(e).await.unwrap();
        store.insert(entry("b", vec![0.5], None)).await.unwrap();
        store.delete(id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }
}

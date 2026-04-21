//! pgvector (PostgreSQL extension) vector store adapter.
//!
//! pgvector adds a `vector` column type to PostgreSQL and supports
//! L2/cosine/inner-product distance queries. This adapter implements the
//! [`VectorStore`] trait with a stub backend; real SQL calls would require
//! a Postgres driver and are not wired here.

use crate::store::{MemoryEntry, SearchResult, VectorStore};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

/// pgvector adapter for PostgreSQL with the `vector` extension.
pub struct PgVectorStore {
    /// PostgreSQL connection string (e.g., "postgres://user:pass@host/db").
    #[allow(dead_code)]
    connection_string: String,
    /// Table that stores the vectors.
    #[allow(dead_code)]
    table_name: String,
    /// Column holding the `vector` value (defaults to "embedding").
    #[allow(dead_code)]
    vector_column: String,
    /// Declared dimension (pgvector requires fixed dim per column).
    #[allow(dead_code)]
    dimension: usize,
    /// Stub in-memory storage.
    entries: RwLock<HashMap<Uuid, MemoryEntry>>,
}

impl PgVectorStore {
    /// Create a new pgvector adapter in stub mode.
    ///
    /// The vector column defaults to `"embedding"`. Use
    /// [`Self::with_vector_column`] to override.
    pub fn new(
        connection_string: impl Into<String>,
        table_name: impl Into<String>,
        dimension: usize,
    ) -> Self {
        Self {
            connection_string: connection_string.into(),
            table_name: table_name.into(),
            vector_column: "embedding".to_string(),
            dimension,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Override the vector column name (default `"embedding"`).
    pub fn with_vector_column(mut self, column: impl Into<String>) -> Self {
        self.vector_column = column.into();
        self
    }

    /// Return the configured table name.
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// Return the configured vector column name.
    pub fn vector_column(&self) -> &str {
        &self.vector_column
    }

    /// Return the configured dimension.
    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Return the configured connection string.
    pub fn connection_string(&self) -> &str {
        &self.connection_string
    }

    /// Render a stub SQL `INSERT` statement for documentation / debug.
    ///
    /// This does NOT execute anything — it is a helper that makes the
    /// underlying SQL shape visible for tests and for users planning a
    /// real driver integration.
    pub fn render_insert_sql(&self) -> String {
        format!(
            "INSERT INTO {} (id, content, {}, metadata, session_id, created_at) \
             VALUES ($1, $2, $3::vector, $4, $5, $6)",
            self.table_name, self.vector_column
        )
    }

    /// Render a stub SQL `SELECT` statement for cosine similarity search.
    pub fn render_search_sql(&self) -> String {
        format!(
            "SELECT id, content, {col}, metadata, session_id, created_at, \
             1 - ({col} <=> $1::vector) AS score \
             FROM {table} ORDER BY {col} <=> $1::vector LIMIT $2",
            col = self.vector_column,
            table = self.table_name
        )
    }
}

#[async_trait]
impl VectorStore for PgVectorStore {
    async fn insert(&self, entry: MemoryEntry) -> ArgentorResult<()> {
        if !entry.embedding.is_empty() && entry.embedding.len() != self.dimension {
            return Err(ArgentorError::Agent(format!(
                "pgvector: dim mismatch (got {}, expected {})",
                entry.embedding.len(),
                self.dimension
            )));
        }
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
    fn test_new_defaults_vector_column() {
        let s = PgVectorStore::new("postgres://u@h/d", "docs", 384);
        assert_eq!(s.table_name(), "docs");
        assert_eq!(s.vector_column(), "embedding");
        assert_eq!(s.dimension(), 384);
        assert_eq!(s.connection_string(), "postgres://u@h/d");
    }

    #[test]
    fn test_with_vector_column() {
        let s = PgVectorStore::new("postgres://u@h/d", "docs", 3).with_vector_column("vec");
        assert_eq!(s.vector_column(), "vec");
    }

    #[test]
    fn test_render_insert_sql() {
        let s = PgVectorStore::new("postgres://u@h/d", "docs", 3);
        let sql = s.render_insert_sql();
        assert!(sql.contains("INSERT INTO docs"));
        assert!(sql.contains("embedding"));
        assert!(sql.contains("$3::vector"));
    }

    #[test]
    fn test_render_search_sql_cosine_operator() {
        let s = PgVectorStore::new("postgres://u@h/d", "docs", 3);
        let sql = s.render_search_sql();
        assert!(sql.contains("<=>"));
        assert!(sql.contains("ORDER BY"));
        assert!(sql.contains("LIMIT $2"));
    }

    #[test]
    fn test_render_search_sql_uses_custom_column() {
        let s = PgVectorStore::new("postgres://u@h/d", "docs", 3).with_vector_column("vec");
        let sql = s.render_search_sql();
        assert!(sql.contains("vec <=> $1::vector"));
    }

    #[tokio::test]
    async fn test_insert_and_count() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        s.insert(entry("a", vec![1.0, 0.0], None)).await.unwrap();
        assert_eq!(s.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_insert_rejects_bad_dim() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 3);
        let bad = entry("x", vec![1.0, 0.0], None);
        assert!(s.insert(bad).await.is_err());
    }

    #[tokio::test]
    async fn test_insert_allows_empty_embedding() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 3);
        let pending = entry("pending", vec![], None);
        assert!(s.insert(pending).await.is_ok());
    }

    #[tokio::test]
    async fn test_insert_many() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        for i in 0..15 {
            s.insert(entry(&format!("e{i}"), vec![1.0, i as f32], None))
                .await
                .unwrap();
        }
        assert_eq!(s.count().await.unwrap(), 15);
    }

    #[tokio::test]
    async fn test_search_orders_by_similarity() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 3);
        s.insert(entry("near", vec![0.9, 0.1, 0.0], None))
            .await
            .unwrap();
        s.insert(entry("far", vec![0.0, 0.0, 1.0], None))
            .await
            .unwrap();
        let r = s.search(&[1.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(r[0].entry.content, "near");
    }

    #[tokio::test]
    async fn test_search_top_k_limits() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        for i in 0..9 {
            s.insert(entry(&format!("e{i}"), vec![1.0, i as f32], None))
                .await
                .unwrap();
        }
        let r = s.search(&[1.0, 0.0], 3, None).await.unwrap();
        assert_eq!(r.len(), 3);
    }

    #[tokio::test]
    async fn test_search_empty_query_errors() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        assert!(s.search(&[], 1, None).await.is_err());
    }

    #[tokio::test]
    async fn test_search_session_filter() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        let sid = Uuid::new_v4();
        s.insert(entry("s", vec![1.0, 0.0], Some(sid)))
            .await
            .unwrap();
        s.insert(entry("o", vec![1.0, 0.0], None)).await.unwrap();
        let r = s.search(&[1.0, 0.0], 5, Some(sid)).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].entry.content, "s");
    }

    #[tokio::test]
    async fn test_delete_existing() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        let e = entry("x", vec![1.0, 0.0], None);
        let id = e.id;
        s.insert(e).await.unwrap();
        assert!(s.delete(id).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_missing() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        assert!(!s.delete(Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_list_all_and_filtered() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        let sid = Uuid::new_v4();
        s.insert(entry("a", vec![1.0, 0.0], Some(sid)))
            .await
            .unwrap();
        s.insert(entry("b", vec![0.0, 1.0], None)).await.unwrap();
        assert_eq!(s.list(None).await.unwrap().len(), 2);
        assert_eq!(s.list(Some(sid)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_metadata_preserved() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        let mut e = entry("m", vec![1.0, 0.0], None);
        e.metadata
            .insert("source".into(), serde_json::json!("manual"));
        let id = e.id;
        s.insert(e).await.unwrap();
        let got = s
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .find(|x| x.id == id)
            .unwrap();
        assert_eq!(
            got.metadata.get("source").unwrap(),
            &serde_json::json!("manual")
        );
    }

    #[tokio::test]
    async fn test_count_after_deletes() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        let e = entry("a", vec![1.0, 0.0], None);
        let id = e.id;
        s.insert(e).await.unwrap();
        s.insert(entry("b", vec![0.0, 1.0], None)).await.unwrap();
        s.delete(id).await.unwrap();
        assert_eq!(s.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_search_on_empty_store() {
        let s = PgVectorStore::new("postgres://u@h/d", "t", 2);
        assert!(s.search(&[1.0, 0.0], 5, None).await.unwrap().is_empty());
    }
}

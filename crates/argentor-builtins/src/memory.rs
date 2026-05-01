use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_memory::{EmbeddingProvider, MemoryEntry, VectorStore};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Skill that stores text in the vector memory.
pub struct MemoryStoreSkill {
    descriptor: SkillDescriptor,
    store: Arc<dyn VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl MemoryStoreSkill {
    /// Create a new memory-store skill using the given vector store and embedding provider.
    pub fn new(store: Arc<dyn VectorStore>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "memory_store".to_string(),
                description: "Store text in long-term vector memory for later retrieval. \
                              Use this to save important facts, decisions, or context."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The text content to store in memory"
                        },
                        "metadata": {
                            "type": "object",
                            "description": "Optional metadata (tags, source, etc.)",
                            "additionalProperties": true
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Optional session ID to associate with this memory"
                        }
                    },
                    "required": ["content"]
                }),
                required_capabilities: vec![Capability::DatabaseQuery],
                requires_approval: false,
            },
            store,
            embedder,
        }
    }
}

#[async_trait]
impl Skill for MemoryStoreSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let content = call.arguments["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if content.is_empty() {
            return Ok(ToolResult::error(&call.id, "Content cannot be empty"));
        }

        // Compute embedding
        let embedding = match self.embedder.embed(&content).await {
            Ok(emb) => emb,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Failed to compute embedding: {e}"),
                ))
            }
        };

        // Parse optional metadata
        let metadata: HashMap<String, serde_json::Value> = call
            .arguments
            .get("metadata")
            .and_then(|m| serde_json::from_value(m.clone()).ok())
            .unwrap_or_default();

        // Parse optional session_id
        let session_id = call
            .arguments
            .get("session_id")
            .and_then(|s| s.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let entry_id = Uuid::new_v4();
        let entry = MemoryEntry {
            id: entry_id,
            content: content.clone(),
            embedding,
            metadata,
            session_id,
            created_at: Utc::now(),
        };

        if let Err(e) = self.store.insert(entry).await {
            return Ok(ToolResult::error(
                &call.id,
                format!("Failed to store memory: {e}"),
            ));
        }

        let response = serde_json::json!({
            "stored": true,
            "id": entry_id.to_string(),
            "content_length": content.len(),
        });
        Ok(ToolResult::success(&call.id, response.to_string()))
    }
}

/// Skill that searches the vector memory for similar text.
pub struct MemorySearchSkill {
    descriptor: SkillDescriptor,
    store: Arc<dyn VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl MemorySearchSkill {
    /// Create a new memory-search skill using the given vector store and embedding provider.
    pub fn new(store: Arc<dyn VectorStore>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "memory_search".to_string(),
                description: "Search long-term vector memory for relevant past information. \
                              Returns the most semantically similar stored memories."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query text"
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Number of results to return (default: 5, max: 20)",
                            "default": 5
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Optional session ID to filter results"
                        }
                    },
                    "required": ["query"]
                }),
                required_capabilities: vec![Capability::DatabaseQuery],
                requires_approval: false,
            },
            store,
            embedder,
        }
    }
}

#[async_trait]
impl Skill for MemorySearchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let query = call.arguments["query"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if query.is_empty() {
            return Ok(ToolResult::error(&call.id, "Query cannot be empty"));
        }

        let top_k = call.arguments["top_k"].as_u64().unwrap_or(5).min(20) as usize;

        let session_filter = call
            .arguments
            .get("session_id")
            .and_then(|s| s.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        // Compute query embedding
        let query_embedding = match self.embedder.embed(&query).await {
            Ok(emb) => emb,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Failed to compute query embedding: {e}"),
                ))
            }
        };

        // Search
        let results = match self
            .store
            .search(&query_embedding, top_k, session_filter)
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(&call.id, format!("Search failed: {e}"))),
        };

        let results_json: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.entry.id.to_string(),
                    "content": r.entry.content,
                    "score": r.score,
                    "metadata": r.entry.metadata,
                    "created_at": r.entry.created_at.to_rfc3339(),
                })
            })
            .collect();

        let response = serde_json::json!({
            "query": query,
            "results": results_json,
            "total": results_json.len(),
        });

        Ok(ToolResult::success(&call.id, response.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_memory::{InMemoryVectorStore, LocalEmbedding};

    fn make_skills() -> (MemoryStoreSkill, MemorySearchSkill) {
        let store: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbedding::default());
        let store_skill = MemoryStoreSkill::new(store.clone(), embedder.clone());
        let search_skill = MemorySearchSkill::new(store, embedder);
        (store_skill, search_skill)
    }

    #[tokio::test]
    async fn test_memory_store_basic() {
        let (store_skill, _) = make_skills();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "memory_store".to_string(),
            arguments: serde_json::json!({"content": "Rust is a systems programming language"}),
        };
        let result = store_skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("\"stored\":true"));
    }

    #[tokio::test]
    async fn test_memory_store_empty_content() {
        let (store_skill, _) = make_skills();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "memory_store".to_string(),
            arguments: serde_json::json!({"content": ""}),
        };
        let result = store_skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_memory_store_with_metadata() {
        let (store_skill, _) = make_skills();
        let call = ToolCall {
            id: "t3".to_string(),
            name: "memory_store".to_string(),
            arguments: serde_json::json!({
                "content": "Important decision: use Rust",
                "metadata": {"tag": "architecture", "priority": "high"}
            }),
        };
        let result = store_skill.execute(call).await.unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_memory_search_basic() {
        let (store_skill, search_skill) = make_skills();

        // Store some entries
        for content in &[
            "Rust is great for systems",
            "Python for data science",
            "Go for networking",
        ] {
            let call = ToolCall {
                id: "s".to_string(),
                name: "memory_store".to_string(),
                arguments: serde_json::json!({"content": content}),
            };
            store_skill.execute(call).await.unwrap();
        }

        // Search
        let call = ToolCall {
            id: "q1".to_string(),
            name: "memory_search".to_string(),
            arguments: serde_json::json!({"query": "systems programming language"}),
        };
        let result = search_skill.execute(call).await.unwrap();
        assert!(!result.is_error);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["total"].as_u64().unwrap() > 0);
        // First result should be the Rust entry (most similar)
        assert!(parsed["results"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Rust"));
    }

    #[tokio::test]
    async fn test_memory_search_empty_query() {
        let (_, search_skill) = make_skills();
        let call = ToolCall {
            id: "q2".to_string(),
            name: "memory_search".to_string(),
            arguments: serde_json::json!({"query": ""}),
        };
        let result = search_skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_memory_search_no_results() {
        let (_, search_skill) = make_skills();
        let call = ToolCall {
            id: "q3".to_string(),
            name: "memory_search".to_string(),
            arguments: serde_json::json!({"query": "anything"}),
        };
        let result = search_skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_memory_search_with_top_k() {
        let (store_skill, search_skill) = make_skills();

        for i in 0..10 {
            let call = ToolCall {
                id: format!("s{i}"),
                name: "memory_store".to_string(),
                arguments: serde_json::json!({"content": format!("Memory entry number {}", i)}),
            };
            store_skill.execute(call).await.unwrap();
        }

        let call = ToolCall {
            id: "q".to_string(),
            name: "memory_search".to_string(),
            arguments: serde_json::json!({"query": "memory entry", "top_k": 3}),
        };
        let result = search_skill.execute(call).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total"].as_u64().unwrap(), 3);
    }

    #[test]
    fn test_descriptors() {
        let store: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbedding::default());

        let ms = MemoryStoreSkill::new(store.clone(), embedder.clone());
        assert_eq!(ms.descriptor().name, "memory_store");

        let msearch = MemorySearchSkill::new(store, embedder);
        assert_eq!(msearch.descriptor().name, "memory_search");
    }
}

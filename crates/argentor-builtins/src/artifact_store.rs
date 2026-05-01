use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Backend trait for artifact storage.
/// Implementations can be in-memory (testing) or filesystem-based.
#[async_trait]
pub trait ArtifactBackend: Send + Sync {
    /// Store content under the given key and kind label.
    async fn store(&self, key: &str, content: &str, kind: &str) -> ArgentorResult<String>;
    /// Retrieve stored content by key.
    async fn retrieve(&self, key: &str) -> ArgentorResult<Option<String>>;
    /// List all stored artifact entries.
    async fn list(&self) -> ArgentorResult<Vec<ArtifactEntry>>;
}

/// Metadata about a stored artifact.
#[derive(Debug, Clone)]
pub struct ArtifactEntry {
    /// Unique key identifying the artifact.
    pub key: String,
    /// Kind label (e.g., "report", "code").
    pub kind: String,
    /// Content length in bytes.
    pub size: usize,
}

/// In-memory artifact backend for testing and short-lived orchestration runs.
pub struct InMemoryArtifactBackend {
    store: RwLock<HashMap<String, (String, String)>>, // key → (content, kind)
}

impl InMemoryArtifactBackend {
    /// Create a new empty in-memory artifact backend.
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryArtifactBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ArtifactBackend for InMemoryArtifactBackend {
    async fn store(&self, key: &str, content: &str, kind: &str) -> ArgentorResult<String> {
        let mut store = self.store.write().await;
        store.insert(key.to_string(), (content.to_string(), kind.to_string()));
        Ok(key.to_string())
    }

    async fn retrieve(&self, key: &str) -> ArgentorResult<Option<String>> {
        let store = self.store.read().await;
        Ok(store.get(key).map(|(content, _)| content.clone()))
    }

    async fn list(&self) -> ArgentorResult<Vec<ArtifactEntry>> {
        let store = self.store.read().await;
        Ok(store
            .iter()
            .map(|(key, (content, kind))| ArtifactEntry {
                key: key.clone(),
                kind: kind.clone(),
                size: content.len(),
            })
            .collect())
    }
}

/// Skill for storing, retrieving, and listing artifacts during orchestration.
pub struct ArtifactStoreSkill {
    descriptor: SkillDescriptor,
    backend: Arc<dyn ArtifactBackend>,
}

impl ArtifactStoreSkill {
    /// Create a new artifact store skill backed by the given storage.
    pub fn new(backend: Arc<dyn ArtifactBackend>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "artifact_store".to_string(),
                description: "Store, retrieve, or list artifacts produced during orchestration. \
                    Use action 'store' to save content with a key, 'retrieve' to get content by key, \
                    or 'list' to see all stored artifacts."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["store", "retrieve", "list"],
                            "description": "The operation to perform"
                        },
                        "key": {
                            "type": "string",
                            "description": "Artifact key (required for store/retrieve)"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to store (required for store)"
                        },
                        "kind": {
                            "type": "string",
                            "description": "Artifact type (e.g. 'code', 'spec', 'test')"
                        }
                    },
                    "required": ["action"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
            backend,
        }
    }
}

#[async_trait]
impl Skill for ArtifactStoreSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let action = call.arguments["action"].as_str().unwrap_or("").to_string();

        match action.as_str() {
            "store" => {
                let key = call.arguments["key"].as_str().unwrap_or("").to_string();
                let content = call.arguments["content"].as_str().unwrap_or("").to_string();
                let kind = call.arguments["kind"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string();

                if key.is_empty() {
                    return Ok(ToolResult::error(&call.id, "Key is required for store"));
                }
                if content.is_empty() {
                    return Ok(ToolResult::error(&call.id, "Content is required for store"));
                }

                let stored_key = self.backend.store(&key, &content, &kind).await?;
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::json!({
                        "stored": true,
                        "key": stored_key,
                        "size": content.len()
                    })
                    .to_string(),
                ))
            }
            "retrieve" => {
                let key = call.arguments["key"].as_str().unwrap_or("").to_string();
                if key.is_empty() {
                    return Ok(ToolResult::error(&call.id, "Key is required for retrieve"));
                }

                match self.backend.retrieve(&key).await? {
                    Some(content) => Ok(ToolResult::success(
                        &call.id,
                        serde_json::json!({
                            "found": true,
                            "key": key,
                            "content": content
                        })
                        .to_string(),
                    )),
                    None => Ok(ToolResult::success(
                        &call.id,
                        serde_json::json!({
                            "found": false,
                            "key": key
                        })
                        .to_string(),
                    )),
                }
            }
            "list" => {
                let entries = self.backend.list().await?;
                let items: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "key": e.key,
                            "kind": e.kind,
                            "size": e.size
                        })
                    })
                    .collect();
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::json!({
                        "count": items.len(),
                        "artifacts": items
                    })
                    .to_string(),
                ))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                "Invalid action. Use 'store', 'retrieve', or 'list'",
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_store_and_retrieve() {
        let backend = InMemoryArtifactBackend::new();
        backend
            .store("main.rs", "fn main() {}", "code")
            .await
            .unwrap();
        let content = backend.retrieve("main.rs").await.unwrap();
        assert_eq!(content, Some("fn main() {}".to_string()));
    }

    #[tokio::test]
    async fn test_in_memory_retrieve_not_found() {
        let backend = InMemoryArtifactBackend::new();
        let content = backend.retrieve("nonexistent").await.unwrap();
        assert!(content.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_list() {
        let backend = InMemoryArtifactBackend::new();
        backend.store("a.rs", "code_a", "code").await.unwrap();
        backend.store("b.md", "spec_b", "spec").await.unwrap();
        let entries = backend.list().await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_skill_store_action() {
        let backend = Arc::new(InMemoryArtifactBackend::new());
        let skill = ArtifactStoreSkill::new(backend.clone());
        let call = ToolCall {
            id: "t1".to_string(),
            name: "artifact_store".to_string(),
            arguments: serde_json::json!({
                "action": "store",
                "key": "output.rs",
                "content": "pub fn hello() {}",
                "kind": "code"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["stored"], true);
    }

    #[tokio::test]
    async fn test_skill_retrieve_action() {
        let backend = Arc::new(InMemoryArtifactBackend::new());
        backend.store("data.json", "{}", "data").await.unwrap();
        let skill = ArtifactStoreSkill::new(backend);
        let call = ToolCall {
            id: "t2".to_string(),
            name: "artifact_store".to_string(),
            arguments: serde_json::json!({
                "action": "retrieve",
                "key": "data.json"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["content"], "{}");
    }

    #[tokio::test]
    async fn test_skill_list_action() {
        let backend = Arc::new(InMemoryArtifactBackend::new());
        backend.store("x", "content", "test").await.unwrap();
        let skill = ArtifactStoreSkill::new(backend);
        let call = ToolCall {
            id: "t3".to_string(),
            name: "artifact_store".to_string(),
            arguments: serde_json::json!({ "action": "list" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
    }

    #[tokio::test]
    async fn test_skill_store_empty_key_error() {
        let backend = Arc::new(InMemoryArtifactBackend::new());
        let skill = ArtifactStoreSkill::new(backend);
        let call = ToolCall {
            id: "t4".to_string(),
            name: "artifact_store".to_string(),
            arguments: serde_json::json!({
                "action": "store",
                "key": "",
                "content": "data"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_skill_invalid_action() {
        let backend = Arc::new(InMemoryArtifactBackend::new());
        let skill = ArtifactStoreSkill::new(backend);
        let call = ToolCall {
            id: "t5".to_string(),
            name: "artifact_store".to_string(),
            arguments: serde_json::json!({ "action": "delete" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }
}

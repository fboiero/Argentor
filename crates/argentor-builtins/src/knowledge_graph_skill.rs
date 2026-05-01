//! Knowledge graph skill — exposes the in-memory knowledge graph as a callable skill.
//!
//! Supported operations: `add_entity`, `add_relationship`, `query_entity`,
//! `find_related`, `context`, `summarize`.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_memory::KnowledgeGraph;
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Skill that wraps a [`KnowledgeGraph`] and exposes entity-relationship operations.
pub struct KnowledgeGraphSkill {
    descriptor: SkillDescriptor,
    graph: Arc<RwLock<KnowledgeGraph>>,
}

impl KnowledgeGraphSkill {
    /// Create a new knowledge graph skill with the given shared graph.
    pub fn new(graph: Arc<RwLock<KnowledgeGraph>>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "knowledge_graph".to_string(),
                description:
                    "Query and manipulate the knowledge graph of entities and relationships. \
                     Supports operations: add_entity, add_relationship, query_entity, \
                     find_related, context, summarize."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["add_entity", "add_relationship", "query_entity",
                                     "find_related", "context", "summarize"],
                            "description": "The operation to perform"
                        },
                        "name": {
                            "type": "string",
                            "description": "Entity name (for add_entity, query_entity)"
                        },
                        "entity_type": {
                            "type": "string",
                            "description": "Entity type: Person, Organization, Concept, Tool, File, Location, Event, Fact"
                        },
                        "entity_id": {
                            "type": "string",
                            "description": "Entity ID (for context, find_related)"
                        },
                        "from_entity": {
                            "type": "string",
                            "description": "Source entity ID (for add_relationship)"
                        },
                        "to_entity": {
                            "type": "string",
                            "description": "Target entity ID (for add_relationship)"
                        },
                        "relation_type": {
                            "type": "string",
                            "description": "Relationship type: IsA, HasProperty, RelatedTo, DependsOn, CreatedBy, Contains, WorksWith, Mentions, UsedTool, ProducedOutput"
                        },
                        "properties": {
                            "type": "object",
                            "description": "Key-value properties for entity or relationship",
                            "additionalProperties": true
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Traversal depth for context (default: 1)",
                            "default": 1
                        },
                        "source": {
                            "type": "string",
                            "description": "Origin of the data: user, agent, tool_result, extracted",
                            "default": "agent"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![Capability::DatabaseQuery],
                requires_approval: false,
            },
            graph,
        }
    }
}

#[async_trait]
impl Skill for KnowledgeGraphSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let op = call.arguments["operation"].as_str().unwrap_or_default();

        match op {
            "add_entity" => self.op_add_entity(&call).await,
            "add_relationship" => self.op_add_relationship(&call).await,
            "query_entity" => self.op_query_entity(&call).await,
            "find_related" => self.op_find_related(&call).await,
            "context" => self.op_context(&call).await,
            "summarize" => self.op_summarize(&call).await,
            other => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{other}'. Use one of: add_entity, add_relationship, query_entity, find_related, context, summarize"),
            )),
        }
    }
}

impl KnowledgeGraphSkill {
    async fn op_add_entity(&self, call: &ToolCall) -> ArgentorResult<ToolResult> {
        let name = call.arguments["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return Ok(ToolResult::error(&call.id, "Entity 'name' is required"));
        }

        let entity_type = parse_entity_type(
            call.arguments
                .get("entity_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Concept"),
        );

        let properties: std::collections::HashMap<String, serde_json::Value> = call
            .arguments
            .get("properties")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .unwrap_or_default();

        let source = call.arguments["source"]
            .as_str()
            .unwrap_or("agent")
            .to_string();

        let now = chrono::Utc::now();
        let entity = argentor_memory::Entity {
            id: String::new(),
            name: name.clone(),
            entity_type,
            properties,
            created_at: now,
            updated_at: now,
            confidence: 1.0,
            source,
        };

        let mut graph = self.graph.write().await;
        let id = graph.add_entity(entity);

        let response = serde_json::json!({
            "added": true,
            "entity_id": id,
            "name": name,
        });
        Ok(ToolResult::success(&call.id, response.to_string()))
    }

    async fn op_add_relationship(&self, call: &ToolCall) -> ArgentorResult<ToolResult> {
        let from = call.arguments["from_entity"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let to = call.arguments["to_entity"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if from.is_empty() || to.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "'from_entity' and 'to_entity' are required",
            ));
        }

        let relation_type = parse_relation_type(
            call.arguments
                .get("relation_type")
                .and_then(|v| v.as_str())
                .unwrap_or("RelatedTo"),
        );

        let properties: std::collections::HashMap<String, serde_json::Value> = call
            .arguments
            .get("properties")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .unwrap_or_default();

        let source = call.arguments["source"]
            .as_str()
            .unwrap_or("agent")
            .to_string();

        let rel = argentor_memory::Relationship {
            id: String::new(),
            from_entity: from.clone(),
            to_entity: to.clone(),
            relation_type,
            properties,
            weight: 1.0,
            created_at: chrono::Utc::now(),
            source,
        };

        let mut graph = self.graph.write().await;
        let id = graph.add_relationship(rel);

        let response = serde_json::json!({
            "added": true,
            "relationship_id": id,
            "from": from,
            "to": to,
        });
        Ok(ToolResult::success(&call.id, response.to_string()))
    }

    async fn op_query_entity(&self, call: &ToolCall) -> ArgentorResult<ToolResult> {
        let name = call.arguments["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Entity 'name' is required for query",
            ));
        }

        let graph = self.graph.read().await;
        let entities = graph.find_entities(&name);

        let results: Vec<serde_json::Value> = entities
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "entity_type": format!("{}", e.entity_type),
                    "properties": e.properties,
                    "confidence": e.confidence,
                    "source": e.source,
                })
            })
            .collect();

        let response = serde_json::json!({
            "query": name,
            "results": results,
            "total": results.len(),
        });
        Ok(ToolResult::success(&call.id, response.to_string()))
    }

    async fn op_find_related(&self, call: &ToolCall) -> ArgentorResult<ToolResult> {
        let entity_id = call.arguments["entity_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if entity_id.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "'entity_id' is required for find_related",
            ));
        }

        let depth = call.arguments["depth"].as_u64().unwrap_or(1) as usize;

        let graph = self.graph.read().await;
        let neighbors = graph.neighbors(&entity_id, depth);

        let results: Vec<serde_json::Value> = neighbors
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "entity_type": format!("{}", e.entity_type),
                })
            })
            .collect();

        let response = serde_json::json!({
            "entity_id": entity_id,
            "depth": depth,
            "related": results,
            "total": results.len(),
        });
        Ok(ToolResult::success(&call.id, response.to_string()))
    }

    async fn op_context(&self, call: &ToolCall) -> ArgentorResult<ToolResult> {
        let entity_id = call.arguments["entity_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if entity_id.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "'entity_id' is required for context",
            ));
        }
        let depth = call.arguments["depth"].as_u64().unwrap_or(1) as usize;

        let graph = self.graph.read().await;
        let ctx = graph.to_context_string(&entity_id, depth);

        Ok(ToolResult::success(&call.id, ctx))
    }

    async fn op_summarize(&self, call: &ToolCall) -> ArgentorResult<ToolResult> {
        let graph = self.graph.read().await;
        let summary = graph.summarize();

        let response = serde_json::json!({
            "entity_count": summary.entity_count,
            "relationship_count": summary.relationship_count,
            "entity_types": summary.entity_types,
            "relationship_types": summary.relationship_types,
            "most_connected": summary.most_connected,
        });
        Ok(ToolResult::success(&call.id, response.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_entity_type(s: &str) -> argentor_memory::EntityType {
    match s {
        "Person" => argentor_memory::EntityType::Person,
        "Organization" => argentor_memory::EntityType::Organization,
        "Concept" => argentor_memory::EntityType::Concept,
        "Tool" => argentor_memory::EntityType::Tool,
        "File" => argentor_memory::EntityType::File,
        "Location" => argentor_memory::EntityType::Location,
        "Event" => argentor_memory::EntityType::Event,
        "Fact" => argentor_memory::EntityType::Fact,
        other => argentor_memory::EntityType::Custom(other.to_string()),
    }
}

fn parse_relation_type(s: &str) -> argentor_memory::RelationType {
    match s {
        "IsA" => argentor_memory::RelationType::IsA,
        "HasProperty" => argentor_memory::RelationType::HasProperty,
        "RelatedTo" => argentor_memory::RelationType::RelatedTo,
        "DependsOn" => argentor_memory::RelationType::DependsOn,
        "CreatedBy" => argentor_memory::RelationType::CreatedBy,
        "Contains" => argentor_memory::RelationType::Contains,
        "WorksWith" => argentor_memory::RelationType::WorksWith,
        "Mentions" => argentor_memory::RelationType::Mentions,
        "UsedTool" => argentor_memory::RelationType::UsedTool,
        "ProducedOutput" => argentor_memory::RelationType::ProducedOutput,
        other => argentor_memory::RelationType::Custom(other.to_string()),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_skill() -> KnowledgeGraphSkill {
        let graph = Arc::new(RwLock::new(KnowledgeGraph::new()));
        KnowledgeGraphSkill::new(graph)
    }

    #[test]
    fn test_descriptor() {
        let skill = make_skill();
        assert_eq!(skill.descriptor().name, "knowledge_graph");
    }

    #[tokio::test]
    async fn test_add_entity_operation() {
        let skill = make_skill();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({
                "operation": "add_entity",
                "name": "Alice",
                "entity_type": "Person"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("\"added\":true"));
        assert!(result.content.contains("Alice"));
    }

    #[tokio::test]
    async fn test_add_entity_missing_name() {
        let skill = make_skill();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({
                "operation": "add_entity",
                "entity_type": "Person"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_add_relationship_operation() {
        let skill = make_skill();

        // Add two entities first
        let call_a = ToolCall {
            id: "a".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "add_entity", "name": "A", "entity_type": "Concept"}),
        };
        let res_a = skill.execute(call_a).await.unwrap();
        let parsed_a: serde_json::Value = serde_json::from_str(&res_a.content).unwrap();
        let id_a = parsed_a["entity_id"].as_str().unwrap().to_string();

        let call_b = ToolCall {
            id: "b".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "add_entity", "name": "B", "entity_type": "Concept"}),
        };
        let res_b = skill.execute(call_b).await.unwrap();
        let parsed_b: serde_json::Value = serde_json::from_str(&res_b.content).unwrap();
        let id_b = parsed_b["entity_id"].as_str().unwrap().to_string();

        // Add relationship
        let call_rel = ToolCall {
            id: "r".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({
                "operation": "add_relationship",
                "from_entity": id_a,
                "to_entity": id_b,
                "relation_type": "DependsOn"
            }),
        };
        let result = skill.execute(call_rel).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("\"added\":true"));
    }

    #[tokio::test]
    async fn test_query_entity_operation() {
        let skill = make_skill();

        // Add entity
        let call = ToolCall {
            id: "a".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "add_entity", "name": "Rust", "entity_type": "Concept"}),
        };
        skill.execute(call).await.unwrap();

        // Query
        let call_q = ToolCall {
            id: "q".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "query_entity", "name": "rust"}),
        };
        let result = skill.execute(call_q).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_summarize_operation() {
        let skill = make_skill();
        let call = ToolCall {
            id: "s".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "summarize"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["entity_count"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = make_skill();
        let call = ToolCall {
            id: "u".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "foobar"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_context_operation() {
        let skill = make_skill();

        // Add entity
        let call = ToolCall {
            id: "a".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "add_entity", "name": "Alice", "entity_type": "Person"}),
        };
        let res = skill.execute(call).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        let id = parsed["entity_id"].as_str().unwrap().to_string();

        // Context
        let call_ctx = ToolCall {
            id: "c".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "context", "entity_id": id}),
        };
        let result = skill.execute(call_ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Alice"));
    }

    #[tokio::test]
    async fn test_find_related_operation() {
        let skill = make_skill();

        // Add entities
        let call_a = ToolCall {
            id: "a".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "add_entity", "name": "A", "entity_type": "Concept"}),
        };
        let res_a = skill.execute(call_a).await.unwrap();
        let id_a: String = serde_json::from_str::<serde_json::Value>(&res_a.content).unwrap()
            ["entity_id"]
            .as_str()
            .unwrap()
            .to_string();

        let call_b = ToolCall {
            id: "b".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "add_entity", "name": "B", "entity_type": "Concept"}),
        };
        let res_b = skill.execute(call_b).await.unwrap();
        let id_b: String = serde_json::from_str::<serde_json::Value>(&res_b.content).unwrap()
            ["entity_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Add relationship
        let call_rel = ToolCall {
            id: "r".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({
                "operation": "add_relationship",
                "from_entity": id_a,
                "to_entity": id_b,
                "relation_type": "RelatedTo"
            }),
        };
        skill.execute(call_rel).await.unwrap();

        // Find related
        let call_find = ToolCall {
            id: "f".to_string(),
            name: "knowledge_graph".to_string(),
            arguments: serde_json::json!({"operation": "find_related", "entity_id": id_a}),
        };
        let result = skill.execute(call_find).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total"].as_u64().unwrap(), 1);
    }
}

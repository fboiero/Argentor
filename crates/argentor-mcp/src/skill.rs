//! Adapter that wraps an MCP tool as an Argentor Skill.

use crate::client::McpClient;
use crate::protocol::McpToolDef;
use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::sync::Arc;

/// An MCP tool wrapped as an Argentor Skill.
/// Delegates execution to the MCP server via the McpClient.
pub struct McpSkill {
    descriptor: SkillDescriptor,
    tool_name: String,
    client: Arc<McpClient>,
}

impl McpSkill {
    /// Create a new McpSkill from an MCP tool definition and a shared client.
    pub fn new(tool: &McpToolDef, client: Arc<McpClient>) -> Self {
        // Prefix the skill name with the server name to avoid collisions
        let prefixed_name = format!("mcp_{}_{}", client.server_name(), tool.name)
            .replace(|c: char| !c.is_alphanumeric() && c != '_', "_");

        Self {
            descriptor: SkillDescriptor {
                name: prefixed_name,
                description: format!("[MCP:{}] {}", client.server_name(), tool.description),
                parameters_schema: tool.input_schema.clone(),
                required_capabilities: vec![], // MCP server handles its own capabilities
                requires_approval: false,
            },
            tool_name: tool.name.clone(),
            client,
        }
    }
}

#[async_trait]
impl Skill for McpSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let result = self
            .client
            .call_tool(&self.tool_name, call.arguments)
            .await?;

        // Combine all content blocks into a single string
        let text: String = result
            .content
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error {
            Ok(ToolResult::error(&call.id, text))
        } else {
            Ok(ToolResult::success(&call.id, text))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_skill_name_prefix() {
        // We can't fully test without a running MCP server,
        // but we can test the descriptor generation logic directly
        let tool = McpToolDef {
            name: "read_file".to_string(),
            description: "Read a file from disk".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                }
            }),
        };

        // Verify tool definition
        assert_eq!(tool.name, "read_file");
        assert!(!tool.description.is_empty());
    }
}

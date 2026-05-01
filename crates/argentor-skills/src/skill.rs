use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Metadata describing a skill's interface and required permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDescriptor {
    /// Unique skill name.
    pub name: String,
    /// Human-readable description of the skill.
    pub description: String,
    /// JSON Schema describing the expected parameters.
    pub parameters_schema: serde_json::Value,
    /// Capabilities the skill needs to operate.
    pub required_capabilities: Vec<Capability>,
    /// Whether the agent runner must obtain human approval before executing this skill.
    /// Defaults to `false`. Skills that delete data, send emails, or spend money should set this.
    #[serde(default)]
    pub requires_approval: bool,
}

/// Trait that all skills must implement — whether native Rust or WASM.
#[async_trait]
pub trait Skill: Send + Sync {
    /// Return the skill's metadata descriptor.
    fn descriptor(&self) -> &SkillDescriptor;

    /// Execute the skill with the given tool call.
    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult>;

    /// Validate that the specific arguments in this tool call are permitted
    /// by the given permission set. Override for skills that need argument-level checks.
    /// Default: always returns Ok(()) (no argument-level validation).
    fn validate_arguments(
        &self,
        _call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        Ok(())
    }
}

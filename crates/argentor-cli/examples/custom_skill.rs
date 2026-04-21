//! Implement a custom Skill, register it, and invoke it through an AgentRunner.
//!
//! Shows the complete skill authoring flow: implement the `Skill` trait,
//! describe parameters via JSON Schema, register the skill in a SkillRegistry,
//! and wire a mock LLM that calls it.
//!
//! No API key required.
//!
//! ```bash
//! cargo run --example custom_skill
//! ```

use argentor_agent::llm::LlmResponse;
use argentor_agent::{AgentRunner, MockBenchmarkBackend};
use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::{Skill, SkillDescriptor, SkillRegistry};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Custom skill: reverses any string.
// ---------------------------------------------------------------------------

/// A skill that reverses the characters in the input string.
struct ReverseSkill {
    descriptor: SkillDescriptor,
}

impl ReverseSkill {
    fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "reverse".into(),
                description: "Reverses the characters of the input string.".into(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "required": ["text"],
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "The string to reverse"
                        }
                    }
                }),
                required_capabilities: vec![],
            },
        }
    }
}

#[async_trait]
impl Skill for ReverseSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let text = call.arguments["text"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .rev()
            .collect::<String>();

        Ok(ToolResult::success(call.id, text))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // 1. Register the custom skill.
    let mut registry = SkillRegistry::new();
    registry.register(Arc::new(ReverseSkill::new()));

    println!("Custom skill registered: 'reverse'");

    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-custom-audit")));

    // 2. Script: the mock LLM calls our custom skill, then returns the final answer.
    let tool_response = LlmResponse::ToolUse {
        content: Some("I'll reverse that string for you.".into()),
        tool_calls: vec![ToolCall {
            id: "tc-1".into(),
            name: "reverse".into(),
            arguments: serde_json::json!({ "text": "Argentor" }),
        }],
    };
    let final_response = LlmResponse::Done("The reversed string is 'rotnetrA'.".into());

    let backend = Box::new(MockBenchmarkBackend::new(vec![
        tool_response,
        final_response,
    ]));

    // 3. Run the agent.
    let agent = AgentRunner::from_backend(backend, skills, permissions, audit, 5);
    let mut session = Session::new();

    let response = agent
        .run(&mut session, "Reverse the string 'Argentor'")
        .await
        .expect("agent run failed");

    println!("Agent response: {response}");
}

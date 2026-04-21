//! Agent with built-in skills (calculator, hash_tool, uuid_generator).
//!
//! Shows how to register skills, build a SkillRegistry, wire it into an
//! AgentRunner, and have the mock LLM request a tool call that the runner
//! executes and feeds back to the model.
//!
//! No API key required.
//!
//! ```bash
//! cargo run --example with_tools
//! ```

use argentor_agent::llm::LlmResponse;
use argentor_agent::{AgentRunner, MockBenchmarkBackend};
use argentor_builtins::{CalculatorSkill, HashSkill, UuidGeneratorSkill};
use argentor_core::ToolCall;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // 1. Register three built-in utility skills.
    let registry = SkillRegistry::new();
    registry.register(Arc::new(CalculatorSkill::default()));
    registry.register(Arc::new(HashSkill::default()));
    registry.register(Arc::new(UuidGeneratorSkill::default()));

    let skill_names: Vec<String> = registry
        .list_descriptors()
        .into_iter()
        .map(|d| d.name)
        .collect();
    println!("Registered skills: {skill_names:?}");

    let skills = Arc::new(registry);
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-tools-audit")));

    // 2. Script the mock: turn 1 asks for the calculator tool,
    //    turn 2 (after the tool result is injected) produces the final answer.
    let tool_response = LlmResponse::ToolUse {
        content: Some("Let me calculate that for you.".into()),
        tool_calls: vec![ToolCall {
            id: "tc-1".into(),
            name: "calculator".into(),
            arguments: serde_json::json!({ "operation": "add", "a": 40.0, "b": 2.0 }),
        }],
    };
    let final_response = LlmResponse::Done("The result of 40 + 2 is 42.".into());

    let backend = Box::new(MockBenchmarkBackend::new(vec![
        tool_response,
        final_response,
    ]));

    // 3. Build the agent and run.
    let agent = AgentRunner::from_backend(backend, skills, permissions, audit, 5);
    let mut session = Session::new();

    let response = agent
        .run(&mut session, "What is 40 + 2?")
        .await
        .expect("agent run failed");

    println!("Agent response: {response}");
}

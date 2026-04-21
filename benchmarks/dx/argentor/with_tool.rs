//! Argentor — agent with one tool (get_weather).
//!
//! Net LOC: 34 (tool definition + registration + agent run).
//! Argentor requires a Skill trait impl; the registration is explicit
//! but typed — no string-based tool registration.

use argentor_agent::{AgentRunner, ModelConfig};
use argentor_builtins::EchoSkill;
use argentor_skills::{Skill, SkillRegistry};
use argentor_core::{ToolCall, ToolResult};
use async_trait::async_trait;
use serde_json::json;

struct GetWeatherSkill;

#[async_trait]
impl Skill for GetWeatherSkill {
    fn name(&self) -> &str { "get_weather" }
    fn description(&self) -> &str { "Return current weather for a city." }
    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] })
    }
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<ToolResult> {
        let city = call.input["city"].as_str().unwrap_or("unknown");
        Ok(ToolResult::text(call.id.clone(), format!("Weather in {city}: sunny, 22°C")))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let registry = SkillRegistry::new();
    registry.register(Box::new(GetWeatherSkill));

    let config = ModelConfig {
        model: "claude-sonnet-4".to_string(),
        max_tokens: 256,
        ..Default::default()
    };
    let mut agent = AgentRunner::with_registry(config, registry)?;

    let response = agent
        .run("You are a weather assistant.", "What's the weather in Paris?")
        .await?;

    println!("{}", response.content);
    Ok(())
}

// --- LOC count (net, no blanks/comments) ---
// imports: 7
// struct GetWeatherSkill: 1
// impl Skill block: 10
// main fn: 12
// TOTAL: 30 net LOC

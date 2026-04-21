//! Template engine skill for the Argentor AI agent framework.
//!
//! Provides simple `{{variable}}` template rendering with conditionals
//! (`{{#if}}...{{/if}}`), loops (`{{#each}}...{{/each}}`), and defaults.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Simple template engine skill with variable substitution, conditionals, and loops.
pub struct TemplateEngineSkill {
    descriptor: SkillDescriptor,
}

impl TemplateEngineSkill {
    /// Create a new template engine skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "template_engine".to_string(),
                description:
                    "Simple {{variable}} template rendering with conditionals, loops, and defaults."
                        .to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["render", "validate", "extract_variables", "render_bulk"],
                            "description": "The template operation to perform"
                        },
                        "template": {
                            "type": "string",
                            "description": "Template string with {{variable}} placeholders"
                        },
                        "variables": {
                            "type": "object",
                            "description": "Variables map for rendering"
                        },
                        "templates": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Multiple templates for bulk rendering"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
            },
        }
    }
}

impl Default for TemplateEngineSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract variable names from a template.
fn extract_variables(template: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        if let Some(end) = remaining[start..].find("}}") {
            let var = remaining[start + 2..start + end].trim();
            // Skip control flow directives
            if !var.starts_with('#') && !var.starts_with('/') && !var.starts_with('!') {
                // Handle default values: "var|default"
                let var_name = var.split('|').next().unwrap_or(var).trim();
                if !var_name.is_empty() && !vars.contains(&var_name.to_string()) {
                    vars.push(var_name.to_string());
                }
            }
            remaining = &remaining[start + end + 2..];
        } else {
            break;
        }
    }
    vars
}

/// Render a simple template with variable substitution.
fn render_template(template: &str, variables: &HashMap<String, Value>) -> Result<String, String> {
    let mut result = template.to_string();

    // Process {{#each items}}...{{/each}} blocks
    result = process_each_blocks(&result, variables)?;

    // Process {{#if var}}...{{/if}} blocks
    result = process_if_blocks(&result, variables)?;

    // Process {{variable}} and {{variable|default}} substitutions
    result = process_variables(&result, variables);

    Ok(result)
}

/// Process {{#each items}}...{{/each}} blocks.
fn process_each_blocks(
    template: &str,
    variables: &HashMap<String, Value>,
) -> Result<String, String> {
    let mut result = template.to_string();
    let each_start = "{{#each ";
    let each_end = "{{/each}}";

    while let Some(start_pos) = result.find(each_start) {
        let var_end = match result[start_pos..].find("}}") {
            Some(pos) => start_pos + pos,
            None => return Err("Unclosed {{#each}} tag".to_string()),
        };
        let var_name = result[start_pos + each_start.len()..var_end].trim();
        let block_start = var_end + 2;
        let block_end = match result[block_start..].find(each_end) {
            Some(pos) => block_start + pos,
            None => return Err("Missing {{/each}} closing tag".to_string()),
        };
        let block_template = &result[block_start..block_end];

        let rendered = if let Some(Value::Array(items)) = variables.get(var_name) {
            let mut parts = Vec::new();
            for (i, item) in items.iter().enumerate() {
                let mut item_str = block_template.to_string();
                // Replace {{.}} with the item value
                let item_display = match item {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                item_str = item_str.replace("{{.}}", &item_display);
                item_str = item_str.replace("{{@index}}", &i.to_string());
                // If item is an object, replace {{field}} with object fields
                if let Value::Object(map) = item {
                    for (k, v) in map {
                        let placeholder = format!("{{{{{k}}}}}");
                        let val_str = match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        item_str = item_str.replace(&placeholder, &val_str);
                    }
                }
                parts.push(item_str);
            }
            parts.join("")
        } else {
            String::new()
        };

        result = format!(
            "{}{}{}",
            &result[..start_pos],
            rendered,
            &result[block_end + each_end.len()..]
        );
    }

    Ok(result)
}

/// Process {{#if var}}...{{/if}} blocks.
fn process_if_blocks(template: &str, variables: &HashMap<String, Value>) -> Result<String, String> {
    let mut result = template.to_string();
    let if_start = "{{#if ";
    let if_end = "{{/if}}";

    while let Some(start_pos) = result.find(if_start) {
        let var_end = match result[start_pos..].find("}}") {
            Some(pos) => start_pos + pos,
            None => return Err("Unclosed {{#if}} tag".to_string()),
        };
        let var_name = result[start_pos + if_start.len()..var_end].trim();
        let block_start = var_end + 2;
        let block_end = match result[block_start..].find(if_end) {
            Some(pos) => block_start + pos,
            None => return Err("Missing {{/if}} closing tag".to_string()),
        };
        let block_content = &result[block_start..block_end];

        // Check if variable is truthy
        let is_truthy = match variables.get(var_name) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Null) => false,
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            Some(Value::Array(a)) => !a.is_empty(),
            Some(Value::Object(o)) => !o.is_empty(),
            None => false,
        };

        // Check for {{else}} inside the block
        let (true_content, false_content) = if let Some(else_pos) = block_content.find("{{else}}") {
            (
                &block_content[..else_pos],
                &block_content[else_pos + "{{else}}".len()..],
            )
        } else {
            (block_content, "")
        };

        let rendered = if is_truthy {
            true_content
        } else {
            false_content
        };

        result = format!(
            "{}{}{}",
            &result[..start_pos],
            rendered,
            &result[block_end + if_end.len()..]
        );
    }

    Ok(result)
}

/// Process {{variable}} and {{variable|default}} substitutions.
fn process_variables(template: &str, variables: &HashMap<String, Value>) -> String {
    let mut result = template.to_string();
    let mut processed = String::new();

    loop {
        let start_pos = match result.find("{{") {
            Some(pos) => pos,
            None => {
                processed.push_str(&result);
                break;
            }
        };
        let end_pos = match result[start_pos..].find("}}") {
            Some(pos) => start_pos + pos,
            None => {
                processed.push_str(&result);
                break;
            }
        };

        processed.push_str(&result[..start_pos]);
        let tag = result[start_pos + 2..end_pos].trim();

        // Handle default values
        let (var_name, default_val) = if let Some((name, default)) = tag.split_once('|') {
            (name.trim(), Some(default.trim()))
        } else {
            (tag, None)
        };

        let value = match variables.get(var_name) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) => default_val.unwrap_or("").to_string(),
            Some(v) => v.to_string(),
            None => default_val
                .unwrap_or(&format!("{{{{{var_name}}}}}"))
                .to_string(),
        };

        processed.push_str(&value);
        result = result[end_pos + 2..].to_string();
    }

    processed
}

#[async_trait]
impl Skill for TemplateEngineSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = match call.arguments["operation"].as_str() {
            Some(op) => op,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'operation'",
                ))
            }
        };

        match operation {
            "render" => {
                let template = match call.arguments["template"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'template'")),
                };
                let variables: HashMap<String, Value> = match call.arguments["variables"].as_object() {
                    Some(obj) => obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    None => HashMap::new(),
                };
                match render_template(template, &variables) {
                    Ok(rendered) => {
                        let response = json!({ "result": rendered });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, format!("Render error: {e}"))),
                }
            }
            "validate" => {
                let template = match call.arguments["template"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'template'")),
                };
                let mut issues = Vec::new();
                let mut open_count = 0usize;
                let mut close_count = 0usize;
                let mut remaining = template;
                while let Some(pos) = remaining.find("{{") {
                    open_count += 1;
                    remaining = &remaining[pos + 2..];
                }
                remaining = template;
                while let Some(pos) = remaining.find("}}") {
                    close_count += 1;
                    remaining = &remaining[pos + 2..];
                }
                if open_count != close_count {
                    issues.push(format!("Mismatched brackets: {open_count} opening, {close_count} closing"));
                }

                // Check for unclosed control blocks
                let if_opens = template.matches("{{#if").count();
                let if_closes = template.matches("{{/if}}").count();
                if if_opens != if_closes {
                    issues.push(format!("Mismatched if blocks: {if_opens} opening, {if_closes} closing"));
                }
                let each_opens = template.matches("{{#each").count();
                let each_closes = template.matches("{{/each}}").count();
                if each_opens != each_closes {
                    issues.push(format!("Mismatched each blocks: {each_opens} opening, {each_closes} closing"));
                }

                let response = json!({
                    "valid": issues.is_empty(),
                    "issues": issues,
                    "variable_count": extract_variables(template).len()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_variables" => {
                let template = match call.arguments["template"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'template'")),
                };
                let vars = extract_variables(template);
                let response = json!({ "variables": vars, "count": vars.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "render_bulk" => {
                let templates: Vec<String> = match call.arguments["templates"].as_array() {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'templates'")),
                };
                let variables: HashMap<String, Value> = match call.arguments["variables"].as_object() {
                    Some(obj) => obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    None => HashMap::new(),
                };
                let mut results = Vec::new();
                for tmpl in &templates {
                    match render_template(tmpl, &variables) {
                        Ok(rendered) => results.push(json!({"template": tmpl, "result": rendered})),
                        Err(e) => results.push(json!({"template": tmpl, "error": e})),
                    }
                }
                let response = json!({ "results": results, "count": results.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: render, validate, extract_variables, render_bulk"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "template_engine".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_simple_render() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "Hello {{name}}, you are {{age}} years old.",
            "variables": {"name": "Alice", "age": 30}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "Hello Alice, you are 30 years old.");
    }

    #[tokio::test]
    async fn test_render_with_default() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "Hello {{name|World}}!",
            "variables": {}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "Hello World!");
    }

    #[tokio::test]
    async fn test_render_if_true() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "{{#if premium}}VIP{{/if}} Welcome",
            "variables": {"premium": true}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "VIP Welcome");
    }

    #[tokio::test]
    async fn test_render_if_false() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "{{#if premium}}VIP{{/if}} Welcome",
            "variables": {"premium": false}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], " Welcome");
    }

    #[tokio::test]
    async fn test_render_if_else() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "{{#if logged_in}}Dashboard{{else}}Login{{/if}}",
            "variables": {"logged_in": false}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "Login");
    }

    #[tokio::test]
    async fn test_render_each() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "Items: {{#each items}}{{.}}, {{/each}}",
            "variables": {"items": ["apple", "banana", "cherry"]}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "Items: apple, banana, cherry, ");
    }

    #[tokio::test]
    async fn test_render_each_objects() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "{{#each users}}{{name}} ({{role}})\n{{/each}}",
            "variables": {"users": [{"name": "Alice", "role": "admin"}, {"name": "Bob", "role": "user"}]}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let rendered = parsed["result"].as_str().unwrap();
        assert!(rendered.contains("Alice (admin)"));
        assert!(rendered.contains("Bob (user)"));
    }

    #[tokio::test]
    async fn test_extract_variables() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "extract_variables",
            "template": "Hello {{name}}, you have {{count}} items. {{#if active}}Active{{/if}}"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let vars = parsed["variables"].as_array().unwrap();
        assert!(vars.contains(&json!("name")));
        assert!(vars.contains(&json!("count")));
        assert_eq!(parsed["count"], 2); // #if and /if are excluded
    }

    #[tokio::test]
    async fn test_validate_valid() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "validate",
            "template": "Hello {{name}}! {{#if x}}yes{{/if}}"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_validate_mismatched_if() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "validate",
            "template": "{{#if x}}unclosed"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_render_bulk() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render_bulk",
            "templates": ["Hello {{name}}", "Bye {{name}}"],
            "variables": {"name": "Alice"}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
    }

    #[tokio::test]
    async fn test_render_no_variables() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "Plain text, no variables."
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "Plain text, no variables.");
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({"template": "hello"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({"operation": "compile"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = TemplateEngineSkill::new();
        assert_eq!(skill.descriptor().name, "template_engine");
    }

    #[tokio::test]
    async fn test_each_with_index() {
        let skill = TemplateEngineSkill::new();
        let call = make_call(json!({
            "operation": "render",
            "template": "{{#each items}}{{@index}}:{{.}} {{/each}}",
            "variables": {"items": ["a", "b", "c"]}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "0:a 1:b 2:c ");
    }
}

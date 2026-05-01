//! Environment variable management skill for the Argentor AI agent framework.
//!
//! Provides environment variable read/list/check, .env file parsing,
//! and variable expansion.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Environment variable management skill.
pub struct EnvManagerSkill {
    descriptor: SkillDescriptor,
}

impl EnvManagerSkill {
    /// Create a new environment manager skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "env_manager".to_string(),
                description: "Environment variable operations: read, list, check existence, parse .env files, and variable expansion.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["get", "list", "check", "parse_dotenv", "expand", "get_multiple", "has_prefix"],
                            "description": "The environment operation to perform"
                        },
                        "name": {
                            "type": "string",
                            "description": "Environment variable name"
                        },
                        "names": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Multiple variable names to get"
                        },
                        "content": {
                            "type": "string",
                            "description": ".env file content to parse"
                        },
                        "template": {
                            "type": "string",
                            "description": "Template string with ${VAR} placeholders to expand"
                        },
                        "variables": {
                            "type": "object",
                            "description": "Variables map for expansion"
                        },
                        "prefix": {
                            "type": "string",
                            "description": "Prefix to filter environment variables"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for EnvManagerSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse .env file content into key-value pairs.
fn parse_dotenv(content: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let val = val.trim();
            // Strip surrounding quotes
            let val = if (val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\''))
            {
                val[1..val.len() - 1].to_string()
            } else {
                val.to_string()
            };
            vars.insert(key, val);
        }
    }
    vars
}

/// Expand ${VAR} placeholders in a template using provided variables.
fn expand_template(template: &str, variables: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in variables {
        let placeholder = format!("${{{key}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

#[async_trait]
impl Skill for EnvManagerSkill {
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
            "get" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                match std::env::var(name) {
                    Ok(val) => {
                        let response = json!({ "name": name, "value": val, "found": true });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(_) => {
                        let response = json!({ "name": name, "value": null, "found": false });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                }
            }
            "list" => {
                let prefix = call.arguments["prefix"].as_str();
                let vars: HashMap<String, String> = std::env::vars()
                    .filter(|(k, _)| {
                        if let Some(p) = prefix {
                            k.starts_with(p)
                        } else {
                            true
                        }
                    })
                    .collect();
                let count = vars.len();
                let response = json!({ "variables": vars, "count": count });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "check" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                let exists = std::env::var(name).is_ok();
                let response = json!({ "name": name, "exists": exists });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "parse_dotenv" => {
                let content = match call.arguments["content"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'content'")),
                };
                let vars = parse_dotenv(content);
                let count = vars.len();
                let response = json!({ "variables": vars, "count": count });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "expand" => {
                let template = match call.arguments["template"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'template'")),
                };
                let variables: HashMap<String, String> = match call.arguments["variables"].as_object() {
                    Some(obj) => obj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect(),
                    None => {
                        // Fall back to environment variables
                        std::env::vars().collect()
                    }
                };
                let expanded = expand_template(template, &variables);
                let response = json!({ "result": expanded });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "get_multiple" => {
                let names: Vec<String> = match call.arguments["names"].as_array() {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'names'")),
                };
                let mut results = serde_json::Map::new();
                let mut found_count = 0u64;
                for name in &names {
                    match std::env::var(name) {
                        Ok(val) => {
                            results.insert(name.clone(), json!(val));
                            found_count += 1;
                        }
                        Err(_) => {
                            results.insert(name.clone(), Value::Null);
                        }
                    }
                }
                let response = json!({
                    "variables": results,
                    "requested": names.len(),
                    "found": found_count
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "has_prefix" => {
                let prefix = match call.arguments["prefix"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'prefix'")),
                };
                let matching: Vec<String> = std::env::vars()
                    .filter(|(k, _)| k.starts_with(prefix))
                    .map(|(k, _)| k)
                    .collect();
                let response = json!({
                    "prefix": prefix,
                    "matching_keys": matching,
                    "count": matching.len()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: get, list, check, parse_dotenv, expand, get_multiple, has_prefix"),
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
            name: "env_manager".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_get_existing() {
        // PATH should always exist
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({"operation": "get", "name": "PATH"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["found"], true);
        assert!(parsed["value"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({"operation": "get", "name": "ARGENTOR_NONEXISTENT_VAR_12345"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["found"], false);
        assert!(parsed["value"].is_null());
    }

    #[tokio::test]
    async fn test_check_existing() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({"operation": "check", "name": "PATH"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["exists"], true);
    }

    #[tokio::test]
    async fn test_check_nonexistent() {
        let skill = EnvManagerSkill::new();
        let call =
            make_call(json!({"operation": "check", "name": "ARGENTOR_NONEXISTENT_VAR_12345"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["exists"], false);
    }

    #[tokio::test]
    async fn test_list() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({"operation": "list"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["count"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_parse_dotenv() {
        let skill = EnvManagerSkill::new();
        let content =
            "# Comment\nDB_HOST=localhost\nDB_PORT=5432\nDB_NAME=\"mydb\"\nSECRET='s3cret'";
        let call = make_call(json!({"operation": "parse_dotenv", "content": content}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 4);
        assert_eq!(parsed["variables"]["DB_HOST"], "localhost");
        assert_eq!(parsed["variables"]["DB_PORT"], "5432");
        assert_eq!(parsed["variables"]["DB_NAME"], "mydb");
        assert_eq!(parsed["variables"]["SECRET"], "s3cret");
    }

    #[tokio::test]
    async fn test_parse_dotenv_empty() {
        let skill = EnvManagerSkill::new();
        let call =
            make_call(json!({"operation": "parse_dotenv", "content": "# only comments\n\n"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[tokio::test]
    async fn test_expand() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({
            "operation": "expand",
            "template": "Hello ${NAME}, you are ${AGE} years old.",
            "variables": {"NAME": "Alice", "AGE": "30"}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "Hello Alice, you are 30 years old.");
    }

    #[tokio::test]
    async fn test_expand_missing_var() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({
            "operation": "expand",
            "template": "Hi ${NAME}!",
            "variables": {}
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        // Unexpanded variable remains as-is
        assert_eq!(parsed["result"], "Hi ${NAME}!");
    }

    #[tokio::test]
    async fn test_get_multiple() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({
            "operation": "get_multiple",
            "names": ["PATH", "ARGENTOR_NONEXISTENT_VAR_12345"]
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["requested"], 2);
        assert_eq!(parsed["found"], 1);
    }

    #[tokio::test]
    async fn test_has_prefix() {
        let skill = EnvManagerSkill::new();
        // Use a prefix that likely won't match
        let call =
            make_call(json!({"operation": "has_prefix", "prefix": "ARGENTOR_NONEXISTENT_PREFIX_"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({"name": "PATH"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = EnvManagerSkill::new();
        let call = make_call(json!({"operation": "set"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = EnvManagerSkill::new();
        assert_eq!(skill.descriptor().name, "env_manager");
    }

    #[test]
    fn test_parse_dotenv_fn() {
        let content = "KEY1=val1\nKEY2=\"val2\"";
        let vars = parse_dotenv(content);
        assert_eq!(vars["KEY1"], "val1");
        assert_eq!(vars["KEY2"], "val2");
    }

    #[test]
    fn test_expand_template_fn() {
        let mut vars = HashMap::new();
        vars.insert("X".to_string(), "42".to_string());
        assert_eq!(expand_template("val=${X}", &vars), "val=42");
    }
}

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use uuid::Uuid;

/// UUID generation and parsing skill.
///
/// Supports generating v4 UUIDs (single or bulk), parsing UUID strings into
/// their components, validation, and nil UUID operations. Inspired by common
/// utility patterns found in agent toolkits.
pub struct UuidGeneratorSkill {
    descriptor: SkillDescriptor,
}

impl UuidGeneratorSkill {
    /// Create a new UUID generator skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "uuid_generator".to_string(),
                description: "UUID generation, parsing, and validation. Supports v4, bulk generation, and component inspection.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["generate", "generate_bulk", "parse", "validate", "nil", "is_nil"],
                            "description": "The UUID operation to perform"
                        },
                        "version": {
                            "type": "string",
                            "enum": ["v4"],
                            "description": "UUID version for generation (default: v4)"
                        },
                        "count": {
                            "type": "integer",
                            "description": "Number of UUIDs to generate (max 100, for generate_bulk)"
                        },
                        "uuid_string": {
                            "type": "string",
                            "description": "UUID string for parse, validate, or is_nil operations"
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

impl Default for UuidGeneratorSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Maximum number of UUIDs that can be generated in a single bulk request.
const MAX_BULK_COUNT: u64 = 100;

/// Parse a UUID string and return its components as a JSON value.
fn parse_uuid(uuid_str: &str) -> Result<serde_json::Value, String> {
    let parsed =
        Uuid::parse_str(uuid_str).map_err(|e| format!("Invalid UUID '{uuid_str}': {e}"))?;

    let version = match parsed.get_version() {
        Some(uuid::Version::Nil) => "nil",
        Some(uuid::Version::Mac) => "v1",
        Some(uuid::Version::Dce) => "v2",
        Some(uuid::Version::Md5) => "v3",
        Some(uuid::Version::Random) => "v4",
        Some(uuid::Version::Sha1) => "v5",
        Some(uuid::Version::SortMac) => "v6",
        Some(uuid::Version::SortRand) => "v7",
        Some(uuid::Version::Custom) => "v8",
        _ => "unknown",
    };

    let variant = match parsed.get_variant() {
        uuid::Variant::NCS => "ncs",
        uuid::Variant::RFC4122 => "rfc4122",
        uuid::Variant::Microsoft => "microsoft",
        uuid::Variant::Future => "future",
        _ => "unknown",
    };

    Ok(serde_json::json!({
        "uuid": parsed.to_string(),
        "version": version,
        "variant": variant,
        "is_nil": parsed.is_nil(),
        "bytes": format!("{:?}", parsed.as_bytes()),
        "urn": parsed.urn().to_string()
    }))
}

#[async_trait]
impl Skill for UuidGeneratorSkill {
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
            "generate" => {
                let version = call.arguments["version"].as_str().unwrap_or("v4");
                match version {
                    "v4" => {
                        let id = Uuid::new_v4();
                        let response = serde_json::json!({
                            "uuid": id.to_string(),
                            "version": "v4"
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    _ => Ok(ToolResult::error(
                        &call.id,
                        format!("Unsupported UUID version: '{version}'. Only 'v4' is supported."),
                    )),
                }
            }
            "generate_bulk" => {
                let count = call.arguments["count"]
                    .as_u64()
                    .or_else(|| {
                        call.arguments["count"]
                            .as_str()
                            .and_then(|s| s.parse::<u64>().ok())
                    })
                    .unwrap_or(1);

                if count == 0 {
                    return Ok(ToolResult::error(&call.id, "Count must be at least 1"));
                }
                if count > MAX_BULK_COUNT {
                    return Ok(ToolResult::error(
                        &call.id,
                        format!("Count exceeds maximum of {MAX_BULK_COUNT}"),
                    ));
                }

                let uuids: Vec<String> = (0..count).map(|_| Uuid::new_v4().to_string()).collect();
                let response = serde_json::json!({
                    "uuids": uuids,
                    "count": count,
                    "version": "v4"
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "parse" => {
                let uuid_str = match call.arguments["uuid_string"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'uuid_string'")),
                };
                match parse_uuid(uuid_str) {
                    Ok(info) => Ok(ToolResult::success(&call.id, info.to_string())),
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "validate" => {
                let uuid_str = match call.arguments["uuid_string"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'uuid_string'")),
                };
                let valid = Uuid::parse_str(uuid_str).is_ok();
                let response = serde_json::json!({
                    "valid": valid,
                    "input": uuid_str
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "nil" => {
                let nil = Uuid::nil();
                let response = serde_json::json!({
                    "uuid": nil.to_string(),
                    "is_nil": true
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "is_nil" => {
                let uuid_str = match call.arguments["uuid_string"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'uuid_string'")),
                };
                match Uuid::parse_str(uuid_str) {
                    Ok(parsed) => {
                        let response = serde_json::json!({
                            "is_nil": parsed.is_nil(),
                            "uuid": uuid_str
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(
                        &call.id,
                        format!("Invalid UUID '{uuid_str}': {e}"),
                    )),
                }
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation: '{operation}'. Supported: generate, generate_bulk, parse, validate, nil, is_nil"
                ),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "uuid_generator".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_generate_v4() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["version"], "v4");
        let uuid_str = parsed["uuid"].as_str().unwrap();
        assert!(Uuid::parse_str(uuid_str).is_ok());
    }

    #[tokio::test]
    async fn test_generate_explicit_v4() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate",
            "version": "v4"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_generate_unsupported_version() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate",
            "version": "v1"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unsupported UUID version"));
    }

    #[tokio::test]
    async fn test_generate_bulk() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate_bulk",
            "count": 5
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 5);
        let uuids = parsed["uuids"].as_array().unwrap();
        assert_eq!(uuids.len(), 5);
        // All should be valid and unique
        let mut seen = std::collections::HashSet::new();
        for u in uuids {
            let s = u.as_str().unwrap();
            assert!(Uuid::parse_str(s).is_ok());
            assert!(seen.insert(s.to_string()), "Duplicate UUID generated");
        }
    }

    #[tokio::test]
    async fn test_generate_bulk_max() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate_bulk",
            "count": 100
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 100);
    }

    #[tokio::test]
    async fn test_generate_bulk_exceeds_max() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate_bulk",
            "count": 101
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("maximum"));
    }

    #[tokio::test]
    async fn test_generate_bulk_zero() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "generate_bulk",
            "count": 0
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("at least 1"));
    }

    #[tokio::test]
    async fn test_parse_v4() {
        let skill = UuidGeneratorSkill::new();
        let uuid = Uuid::new_v4().to_string();
        let call = make_call(serde_json::json!({
            "operation": "parse",
            "uuid_string": uuid
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["version"], "v4");
        assert_eq!(parsed["variant"], "rfc4122");
        assert_eq!(parsed["is_nil"], false);
    }

    #[tokio::test]
    async fn test_parse_nil() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "parse",
            "uuid_string": "00000000-0000-0000-0000-000000000000"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["is_nil"], true);
    }

    #[tokio::test]
    async fn test_parse_invalid() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "parse",
            "uuid_string": "not-a-uuid"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Invalid UUID"));
    }

    #[tokio::test]
    async fn test_validate_valid() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "validate",
            "uuid_string": "550e8400-e29b-41d4-a716-446655440000"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_validate_invalid() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "validate",
            "uuid_string": "not-valid"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_nil() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "nil"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["uuid"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(parsed["is_nil"], true);
    }

    #[tokio::test]
    async fn test_is_nil_true() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "is_nil",
            "uuid_string": "00000000-0000-0000-0000-000000000000"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["is_nil"], true);
    }

    #[tokio::test]
    async fn test_is_nil_false() {
        let skill = UuidGeneratorSkill::new();
        let uuid = Uuid::new_v4().to_string();
        let call = make_call(serde_json::json!({
            "operation": "is_nil",
            "uuid_string": uuid
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["is_nil"], false);
    }

    #[tokio::test]
    async fn test_is_nil_invalid_uuid() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "is_nil",
            "uuid_string": "garbage"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = UuidGeneratorSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "v7"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = UuidGeneratorSkill::new();
        assert_eq!(skill.descriptor().name, "uuid_generator");
    }

    #[tokio::test]
    async fn test_parse_has_urn() {
        let skill = UuidGeneratorSkill::new();
        let uuid = Uuid::new_v4().to_string();
        let call = make_call(serde_json::json!({
            "operation": "parse",
            "uuid_string": uuid
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        let urn = parsed["urn"].as_str().unwrap();
        assert!(urn.starts_with("urn:uuid:"));
    }
}

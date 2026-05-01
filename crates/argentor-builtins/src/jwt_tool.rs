//! JWT inspection skill for the Argentor AI agent framework.
//!
//! Provides JWT decoding (without verification), claims inspection,
//! expiry checking, and header/payload extraction.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};

/// JWT inspection skill (decode-only, no signature verification).
pub struct JwtToolSkill {
    descriptor: SkillDescriptor,
}

impl JwtToolSkill {
    /// Create a new JWT tool skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "jwt_tool".to_string(),
                description: "JWT decode (no verification), inspect claims, check expiry, extract header/payload.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["decode", "header", "payload", "claims", "check_expiry", "validate_structure"],
                            "description": "The JWT operation to perform"
                        },
                        "token": {
                            "type": "string",
                            "description": "JWT token string"
                        }
                    },
                    "required": ["operation", "token"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for JwtToolSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode a base64url-encoded string to bytes, then to a UTF-8 string.
fn base64url_decode(input: &str) -> Result<String, String> {
    // Add padding if needed
    let padded = match input.len() % 4 {
        2 => format!("{input}=="),
        3 => format!("{input}="),
        _ => input.to_string(),
    };
    // Replace URL-safe characters
    let standard = padded.replace('-', "+").replace('_', "/");
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &standard)
        .map_err(|e| format!("Base64 decode error: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("UTF-8 decode error: {e}"))
}

/// Split a JWT into its three parts.
fn split_jwt(token: &str) -> Result<(&str, &str, &str), String> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    if parts.len() != 3 {
        return Err(format!(
            "Invalid JWT: expected 3 parts separated by '.', got {}",
            parts.len()
        ));
    }
    Ok((parts[0], parts[1], parts[2]))
}

/// Decode the header of a JWT.
fn decode_header(token: &str) -> Result<Value, String> {
    let (header_b64, _, _) = split_jwt(token)?;
    let header_json = base64url_decode(header_b64)?;
    serde_json::from_str(&header_json).map_err(|e| format!("Invalid header JSON: {e}"))
}

/// Decode the payload of a JWT.
fn decode_payload(token: &str) -> Result<Value, String> {
    let (_, payload_b64, _) = split_jwt(token)?;
    let payload_json = base64url_decode(payload_b64)?;
    serde_json::from_str(&payload_json).map_err(|e| format!("Invalid payload JSON: {e}"))
}

#[async_trait]
impl Skill for JwtToolSkill {
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

        let token = match call.arguments["token"].as_str() {
            Some(v) => v,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'token'",
                ))
            }
        };

        match operation {
            "decode" => {
                let header = match decode_header(token) {
                    Ok(h) => h,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Header decode error: {e}"))),
                };
                let payload = match decode_payload(token) {
                    Ok(p) => p,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Payload decode error: {e}"))),
                };
                let (_, _, signature) = split_jwt(token).unwrap_or(("", "", ""));
                let response = json!({
                    "header": header,
                    "payload": payload,
                    "signature": signature,
                    "note": "Signature not verified — decode only"
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "header" => {
                match decode_header(token) {
                    Ok(header) => {
                        let response = json!({ "header": header });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "payload" | "claims" => {
                match decode_payload(token) {
                    Ok(payload) => {
                        let response = json!({ "payload": payload });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "check_expiry" => {
                let payload = match decode_payload(token) {
                    Ok(p) => p,
                    Err(e) => return Ok(ToolResult::error(&call.id, e)),
                };
                let now = chrono::Utc::now().timestamp();
                let exp = payload["exp"].as_i64();
                let iat = payload["iat"].as_i64();
                let nbf = payload["nbf"].as_i64();

                let (is_expired, expires_in) = if let Some(exp_val) = exp {
                    let diff = exp_val - now;
                    (diff < 0, Some(diff))
                } else {
                    (false, None)
                };

                let is_not_yet_valid = nbf.map(|n| n > now).unwrap_or(false);

                let response = json!({
                    "exp": exp,
                    "iat": iat,
                    "nbf": nbf,
                    "is_expired": is_expired,
                    "is_not_yet_valid": is_not_yet_valid,
                    "expires_in_seconds": expires_in,
                    "current_time": now,
                    "has_expiry": exp.is_some()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "validate_structure" => {
                let valid_structure = split_jwt(token).is_ok();
                let header_valid = decode_header(token).is_ok();
                let payload_valid = decode_payload(token).is_ok();

                let mut issues = Vec::new();
                if !valid_structure {
                    issues.push("Token does not have 3 dot-separated parts");
                }
                if !header_valid {
                    issues.push("Header is not valid base64url-encoded JSON");
                }
                if !payload_valid {
                    issues.push("Payload is not valid base64url-encoded JSON");
                }

                let response = json!({
                    "valid_structure": valid_structure && header_valid && payload_valid,
                    "has_three_parts": valid_structure,
                    "header_decodable": header_valid,
                    "payload_decodable": payload_valid,
                    "issues": issues
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: decode, header, payload, claims, check_expiry, validate_structure"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // A known test JWT (HS256, not secret, from jwt.io):
    // Header: {"alg":"HS256","typ":"JWT"}
    // Payload: {"sub":"1234567890","name":"John Doe","iat":1516239022}
    const TEST_JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "jwt_tool".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_decode() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "decode", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["header"]["alg"], "HS256");
        assert_eq!(parsed["header"]["typ"], "JWT");
        assert_eq!(parsed["payload"]["sub"], "1234567890");
        assert_eq!(parsed["payload"]["name"], "John Doe");
    }

    #[tokio::test]
    async fn test_header() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "header", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["header"]["alg"], "HS256");
    }

    #[tokio::test]
    async fn test_payload() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "payload", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["payload"]["name"], "John Doe");
    }

    #[tokio::test]
    async fn test_claims_alias() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "claims", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["payload"]["sub"], "1234567890");
    }

    #[tokio::test]
    async fn test_check_expiry_no_exp() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "check_expiry", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["has_expiry"], false);
        assert_eq!(parsed["is_expired"], false);
    }

    #[tokio::test]
    async fn test_validate_structure_valid() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "validate_structure", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid_structure"], true);
        assert_eq!(parsed["has_three_parts"], true);
        assert_eq!(parsed["header_decodable"], true);
        assert_eq!(parsed["payload_decodable"], true);
    }

    #[tokio::test]
    async fn test_validate_structure_invalid() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "validate_structure", "token": "not.a.jwt"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid_structure"], false);
    }

    #[tokio::test]
    async fn test_decode_invalid_token() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "decode", "token": "invalid"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_decode_two_parts() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "decode", "token": "abc.def"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("3 parts"));
    }

    #[tokio::test]
    async fn test_missing_token() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "decode"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("token"));
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "verify", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_iat_in_check_expiry() {
        let skill = JwtToolSkill::new();
        let call = make_call(json!({"operation": "check_expiry", "token": TEST_JWT}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["iat"], 1_516_239_022);
    }

    #[test]
    fn test_descriptor_name() {
        let skill = JwtToolSkill::new();
        assert_eq!(skill.descriptor().name, "jwt_tool");
    }

    #[test]
    fn test_base64url_decode() {
        let result = base64url_decode("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9").unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["alg"], "HS256");
    }
}

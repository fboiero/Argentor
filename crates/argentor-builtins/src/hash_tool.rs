use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

type HmacSha256 = Hmac<Sha256>;

/// Cryptographic hashing skill.
///
/// Supports SHA-256, SHA-512, HMAC-SHA256, checksum generation, and
/// constant-time hash verification. Inspired by common security tooling patterns.
pub struct HashSkill {
    descriptor: SkillDescriptor,
}

impl HashSkill {
    /// Create a new cryptographic hashing skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "hash".to_string(),
                description: "Cryptographic hashing: SHA-256, SHA-512, HMAC-SHA256, checksum, and verification.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["sha256", "sha512", "hmac_sha256", "checksum", "verify", "hash_file_content"],
                            "description": "The hashing operation to perform"
                        },
                        "input": {
                            "type": "string",
                            "description": "The input string to hash"
                        },
                        "key": {
                            "type": "string",
                            "description": "Secret key for HMAC operations"
                        },
                        "algorithm": {
                            "type": "string",
                            "enum": ["sha256", "sha512"],
                            "description": "Hash algorithm to use (default: sha256)"
                        },
                        "expected_hash": {
                            "type": "string",
                            "description": "Expected hash value for verification"
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

impl Default for HashSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute SHA-256 hash of the given input and return it as a hex string.
fn compute_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Compute SHA-512 hash of the given input and return it as a hex string.
fn compute_sha512(input: &str) -> String {
    let mut hasher = Sha512::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Compute HMAC-SHA256 of the given input with the given key.
fn compute_hmac_sha256(input: &str, key: &str) -> Result<String, String> {
    let mac =
        HmacSha256::new_from_slice(key.as_bytes()).map_err(|e| format!("Invalid HMAC key: {e}"))?;
    let mut mac = mac;
    mac.update(input.as_bytes());
    let result = mac.finalize();
    Ok(hex::encode(result.into_bytes()))
}

/// Compute hash using the specified algorithm (sha256 or sha512).
fn compute_hash(input: &str, algorithm: &str) -> Result<String, String> {
    match algorithm {
        "sha256" => Ok(compute_sha256(input)),
        "sha512" => Ok(compute_sha512(input)),
        _ => Err(format!(
            "Unsupported algorithm: '{algorithm}'. Supported: sha256, sha512"
        )),
    }
}

/// Constant-time comparison of two hex-encoded hashes.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    if a_lower.len() != b_lower.len() {
        return false;
    }
    let a_bytes = a_lower.as_bytes();
    let b_bytes = b_lower.as_bytes();
    let mut diff: u8 = 0;
    for (x, y) in a_bytes.iter().zip(b_bytes.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[async_trait]
impl Skill for HashSkill {
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
            "sha256" => {
                let input = match call.arguments["input"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'input'")),
                };
                let hash = compute_sha256(input);
                let response = serde_json::json!({
                    "hash": hash,
                    "algorithm": "sha256"
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "sha512" => {
                let input = match call.arguments["input"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'input'")),
                };
                let hash = compute_sha512(input);
                let response = serde_json::json!({
                    "hash": hash,
                    "algorithm": "sha512"
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "hmac_sha256" => {
                let input = match call.arguments["input"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'input'")),
                };
                let key = match call.arguments["key"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'key' for HMAC operation")),
                };
                match compute_hmac_sha256(input, key) {
                    Ok(hash) => {
                        let response = serde_json::json!({
                            "hash": hash,
                            "algorithm": "hmac_sha256"
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "checksum" => {
                let input = match call.arguments["input"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'input'")),
                };
                let algorithm = call.arguments["algorithm"].as_str().unwrap_or("sha256");
                match compute_hash(input, algorithm) {
                    Ok(hash) => {
                        let response = serde_json::json!({
                            "hash": hash,
                            "algorithm": algorithm
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "verify" => {
                let input = match call.arguments["input"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'input'")),
                };
                let expected_hash = match call.arguments["expected_hash"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'expected_hash'")),
                };
                let algorithm = call.arguments["algorithm"].as_str().unwrap_or("sha256");
                match compute_hash(input, algorithm) {
                    Ok(computed) => {
                        let valid = constant_time_eq(&computed, expected_hash);
                        let response = serde_json::json!({
                            "valid": valid,
                            "algorithm": algorithm
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "hash_file_content" => {
                let input = match call.arguments["input"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'input' (file content to hash)")),
                };
                let algorithm = call.arguments["algorithm"].as_str().unwrap_or("sha256");
                match compute_hash(input, algorithm) {
                    Ok(hash) => {
                        let response = serde_json::json!({
                            "hash": hash,
                            "algorithm": algorithm,
                            "content_length": input.len()
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: sha256, sha512, hmac_sha256, checksum, verify, hash_file_content"),
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
            name: "hash".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_sha256() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "sha256",
            "input": "hello world"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["algorithm"], "sha256");
        // Known SHA-256 of "hello world"
        assert_eq!(
            parsed["hash"],
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn test_sha512() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "sha512",
            "input": "hello world"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["algorithm"], "sha512");
        // Known SHA-512 of "hello world"
        assert_eq!(
            parsed["hash"],
            "309ecc489c12d6eb4cc40f50c902f2b4d0ed77ee511a7c7a9bcd3ca86d4cd86f989dd35bc5ff499670da34255b45b0cfd830e81f605dcf7dc5542e93ae9cd76f"
        );
    }

    #[tokio::test]
    async fn test_hmac_sha256() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "hmac_sha256",
            "input": "hello world",
            "key": "secret"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["algorithm"], "hmac_sha256");
        // The hash should be a 64-char hex string (32 bytes)
        let hash = parsed["hash"].as_str().unwrap();
        assert_eq!(hash.len(), 64);
    }

    #[tokio::test]
    async fn test_hmac_sha256_missing_key() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "hmac_sha256",
            "input": "hello"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("key"));
    }

    #[tokio::test]
    async fn test_checksum_default_sha256() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "checksum",
            "input": "hello world"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["algorithm"], "sha256");
        assert_eq!(
            parsed["hash"],
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn test_checksum_sha512() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "checksum",
            "input": "hello world",
            "algorithm": "sha512"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["algorithm"], "sha512");
    }

    #[tokio::test]
    async fn test_checksum_unsupported_algorithm() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "checksum",
            "input": "test",
            "algorithm": "md5"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unsupported algorithm"));
    }

    #[tokio::test]
    async fn test_verify_valid() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "verify",
            "input": "hello world",
            "expected_hash": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_verify_invalid() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "verify",
            "input": "hello world",
            "expected_hash": "0000000000000000000000000000000000000000000000000000000000000000"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_verify_case_insensitive() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "verify",
            "input": "hello world",
            "expected_hash": "B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_hash_file_content() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "hash_file_content",
            "input": "file content here",
            "algorithm": "sha256"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["algorithm"], "sha256");
        assert_eq!(parsed["content_length"], 17);
        assert!(parsed["hash"].as_str().unwrap().len() == 64);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "input": "hello"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "md5",
            "input": "hello"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_empty_input() {
        let skill = HashSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "sha256",
            "input": ""
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        // SHA-256 of empty string
        assert_eq!(
            parsed["hash"],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_constant_time_eq_same() {
        assert!(constant_time_eq("abc123", "abc123"));
    }

    #[test]
    fn test_constant_time_eq_different() {
        assert!(!constant_time_eq("abc123", "abc124"));
    }

    #[test]
    fn test_constant_time_eq_different_length() {
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn test_constant_time_eq_case_insensitive() {
        assert!(constant_time_eq("ABCDEF", "abcdef"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = HashSkill::new();
        assert_eq!(skill.descriptor().name, "hash");
    }
}

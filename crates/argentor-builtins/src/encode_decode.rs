use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};

/// Encoding and decoding skill.
///
/// Supports Base64 (standard and URL-safe), hex, URL percent-encoding,
/// HTML entity encoding, and JWT payload decoding. Inspired by AutoGPT
/// encoder/decoder blocks.
pub struct EncodeDecodeSkill {
    descriptor: SkillDescriptor,
}

impl EncodeDecodeSkill {
    /// Create a new encode/decode skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "encode_decode".to_string(),
                description:
                    "Encoding/decoding: Base64, hex, URL, HTML entities, JWT payload parsing."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": [
                                "base64_encode", "base64_decode",
                                "base64url_encode", "base64url_decode",
                                "hex_encode", "hex_decode",
                                "url_encode", "url_decode",
                                "html_encode", "html_decode",
                                "jwt_decode"
                            ],
                            "description": "The encoding/decoding operation to perform"
                        },
                        "input": {
                            "type": "string",
                            "description": "The input string to encode or decode"
                        }
                    },
                    "required": ["operation", "input"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for EncodeDecodeSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// URL percent-encode a string, preserving unreserved characters per RFC 3986.
fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
}

/// Decode a URL percent-encoded string.
fn url_decode(input: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.bytes().peekable();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars
                .next()
                .ok_or("Incomplete percent-encoding: unexpected end of input")?;
            let lo = chars
                .next()
                .ok_or("Incomplete percent-encoding: unexpected end of input")?;
            let hex_str = format!("{}{}", hi as char, lo as char);
            let decoded = u8::from_str_radix(&hex_str, 16)
                .map_err(|_| format!("Invalid percent-encoding: %{hex_str}"))?;
            bytes.push(decoded);
        } else if b == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).map_err(|e| format!("Decoded bytes are not valid UTF-8: {e}"))
}

/// Encode a string with HTML entities for the five special characters.
fn html_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => encoded.push_str("&amp;"),
            '<' => encoded.push_str("&lt;"),
            '>' => encoded.push_str("&gt;"),
            '"' => encoded.push_str("&quot;"),
            '\'' => encoded.push_str("&#39;"),
            _ => encoded.push(ch),
        }
    }
    encoded
}

/// Decode HTML entities back to their original characters.
fn html_decode(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
}

/// Decode a JWT token's payload (second segment) without signature verification.
fn jwt_decode(token: &str) -> Result<String, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(format!(
            "Invalid JWT format: expected 3 dot-separated parts, got {}",
            parts.len()
        ));
    }

    let payload_b64 = parts[1];

    // JWT uses base64url encoding without padding; add padding if needed.
    let padded = match payload_b64.len() % 4 {
        2 => format!("{payload_b64}=="),
        3 => format!("{payload_b64}="),
        _ => payload_b64.to_string(),
    };

    let decoded_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(padded.trim_end_matches('='))
        .or_else(|_| general_purpose::URL_SAFE.decode(&padded))
        .map_err(|e| format!("Failed to base64url-decode JWT payload: {e}"))?;

    let payload_str = String::from_utf8(decoded_bytes)
        .map_err(|e| format!("JWT payload is not valid UTF-8: {e}"))?;

    // Validate that the payload is valid JSON
    serde_json::from_str::<serde_json::Value>(&payload_str)
        .map_err(|e| format!("JWT payload is not valid JSON: {e}"))?;

    Ok(payload_str)
}

/// Build a success response in the standard `{"result": ..., "encoding": ...}` format.
fn success_response(call_id: &str, result: &str, encoding: &str) -> ToolResult {
    let response = serde_json::json!({
        "result": result,
        "encoding": encoding
    });
    ToolResult::success(call_id, response.to_string())
}

#[async_trait]
impl Skill for EncodeDecodeSkill {
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
        let input = match call.arguments["input"].as_str() {
            Some(v) => v,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'input'",
                ))
            }
        };

        match operation {
            "base64_encode" => {
                let encoded = general_purpose::STANDARD.encode(input.as_bytes());
                Ok(success_response(&call.id, &encoded, "base64"))
            }
            "base64_decode" => match general_purpose::STANDARD.decode(input.as_bytes()) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(decoded) => Ok(success_response(&call.id, &decoded, "base64")),
                    Err(e) => Ok(ToolResult::error(
                        &call.id,
                        format!("Decoded bytes are not valid UTF-8: {e}"),
                    )),
                },
                Err(e) => Ok(ToolResult::error(
                    &call.id,
                    format!("Invalid base64 input: {e}"),
                )),
            },
            "base64url_encode" => {
                let encoded = general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes());
                Ok(success_response(&call.id, &encoded, "base64url"))
            }
            "base64url_decode" => {
                // Try without padding first, then with padding
                let decode_result = general_purpose::URL_SAFE_NO_PAD
                    .decode(input.as_bytes())
                    .or_else(|_| general_purpose::URL_SAFE.decode(input.as_bytes()));
                match decode_result {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(decoded) => Ok(success_response(&call.id, &decoded, "base64url")),
                        Err(e) => Ok(ToolResult::error(
                            &call.id,
                            format!("Decoded bytes are not valid UTF-8: {e}"),
                        )),
                    },
                    Err(e) => Ok(ToolResult::error(
                        &call.id,
                        format!("Invalid base64url input: {e}"),
                    )),
                }
            }
            "hex_encode" => {
                let encoded = hex::encode(input.as_bytes());
                Ok(success_response(&call.id, &encoded, "hex"))
            }
            "hex_decode" => match hex::decode(input) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(decoded) => Ok(success_response(&call.id, &decoded, "hex")),
                    Err(e) => Ok(ToolResult::error(
                        &call.id,
                        format!("Decoded bytes are not valid UTF-8: {e}"),
                    )),
                },
                Err(e) => Ok(ToolResult::error(
                    &call.id,
                    format!("Invalid hex input: {e}"),
                )),
            },
            "url_encode" => {
                let encoded = url_encode(input);
                Ok(success_response(&call.id, &encoded, "url"))
            }
            "url_decode" => match url_decode(input) {
                Ok(decoded) => Ok(success_response(&call.id, &decoded, "url")),
                Err(e) => Ok(ToolResult::error(&call.id, e)),
            },
            "html_encode" => {
                let encoded = html_encode(input);
                Ok(success_response(&call.id, &encoded, "html"))
            }
            "html_decode" => {
                let decoded = html_decode(input);
                Ok(success_response(&call.id, &decoded, "html"))
            }
            "jwt_decode" => match jwt_decode(input) {
                Ok(payload) => {
                    let response = serde_json::json!({
                        "result": serde_json::from_str::<serde_json::Value>(&payload).unwrap_or(serde_json::Value::String(payload)),
                        "encoding": "jwt"
                    });
                    Ok(ToolResult::success(&call.id, response.to_string()))
                }
                Err(e) => Ok(ToolResult::error(&call.id, e)),
            },
            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation: '{operation}'. Supported: base64_encode, base64_decode, \
                     base64url_encode, base64url_decode, hex_encode, hex_decode, \
                     url_encode, url_decode, html_encode, html_decode, jwt_decode"
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
            name: "encode_decode".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_base64_encode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "base64_encode",
            "input": "hello world"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "aGVsbG8gd29ybGQ=");
        assert_eq!(parsed["encoding"], "base64");
    }

    #[tokio::test]
    async fn test_base64_decode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "base64_decode",
            "input": "aGVsbG8gd29ybGQ="
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "hello world");
    }

    #[tokio::test]
    async fn test_base64_decode_invalid() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "base64_decode",
            "input": "!!!not-base64!!!"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_base64url_encode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "base64url_encode",
            "input": "hello+world/foo"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["encoding"], "base64url");
        // URL-safe base64 should not contain + or /
        let encoded = parsed["result"].as_str().unwrap();
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[tokio::test]
    async fn test_base64url_roundtrip() {
        let skill = EncodeDecodeSkill::new();
        let original = "data with special chars: +/=";

        let enc_call = make_call(serde_json::json!({
            "operation": "base64url_encode",
            "input": original
        }));
        let enc_result = skill.execute(enc_call).await.unwrap();
        let enc_parsed: serde_json::Value = serde_json::from_str(&enc_result.content).unwrap();
        let encoded = enc_parsed["result"].as_str().unwrap();

        let dec_call = make_call(serde_json::json!({
            "operation": "base64url_decode",
            "input": encoded
        }));
        let dec_result = skill.execute(dec_call).await.unwrap();
        assert!(!dec_result.is_error);
        let dec_parsed: serde_json::Value = serde_json::from_str(&dec_result.content).unwrap();
        assert_eq!(dec_parsed["result"], original);
    }

    #[tokio::test]
    async fn test_hex_encode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "hex_encode",
            "input": "hello"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "68656c6c6f");
        assert_eq!(parsed["encoding"], "hex");
    }

    #[tokio::test]
    async fn test_hex_decode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "hex_decode",
            "input": "68656c6c6f"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "hello");
    }

    #[tokio::test]
    async fn test_hex_decode_invalid() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "hex_decode",
            "input": "zzzz"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_url_encode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "url_encode",
            "input": "hello world&foo=bar"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "hello%20world%26foo%3Dbar");
        assert_eq!(parsed["encoding"], "url");
    }

    #[tokio::test]
    async fn test_url_decode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "url_decode",
            "input": "hello%20world%26foo%3Dbar"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "hello world&foo=bar");
    }

    #[tokio::test]
    async fn test_url_decode_plus_as_space() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "url_decode",
            "input": "hello+world"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "hello world");
    }

    #[tokio::test]
    async fn test_url_decode_incomplete_percent() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "url_decode",
            "input": "hello%2"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_html_encode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "html_encode",
            "input": "<p class=\"test\">Hello & 'world'</p>"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(
            parsed["result"],
            "&lt;p class=&quot;test&quot;&gt;Hello &amp; &#39;world&#39;&lt;/p&gt;"
        );
    }

    #[tokio::test]
    async fn test_html_decode() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "html_decode",
            "input": "&lt;p&gt;Hello &amp; &#39;world&#39;&lt;/p&gt;"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["result"], "<p>Hello & 'world'</p>");
    }

    #[tokio::test]
    async fn test_jwt_decode() {
        let skill = EncodeDecodeSkill::new();
        // Construct a valid JWT with known payload: {"sub":"1234567890","name":"John Doe","iat":1516239022}
        let header =
            general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
        let payload = general_purpose::URL_SAFE_NO_PAD
            .encode(b"{\"sub\":\"1234567890\",\"name\":\"John Doe\",\"iat\":1516239022}");
        let token = format!("{header}.{payload}.fake_signature");

        let call = make_call(serde_json::json!({
            "operation": "jwt_decode",
            "input": token
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["encoding"], "jwt");
        assert_eq!(parsed["result"]["sub"], "1234567890");
        assert_eq!(parsed["result"]["name"], "John Doe");
        assert_eq!(parsed["result"]["iat"], 1516239022);
    }

    #[tokio::test]
    async fn test_jwt_decode_invalid_format() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "jwt_decode",
            "input": "not.a-jwt"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("3 dot-separated parts"));
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "input": "hello"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_missing_input() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "base64_encode"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("input"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = EncodeDecodeSkill::new();
        let call = make_call(serde_json::json!({
            "operation": "rot13",
            "input": "hello"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_empty_string_base64_roundtrip() {
        let skill = EncodeDecodeSkill::new();
        let enc_call = make_call(serde_json::json!({
            "operation": "base64_encode",
            "input": ""
        }));
        let enc_result = skill.execute(enc_call).await.unwrap();
        assert!(!enc_result.is_error);
        let enc_parsed: serde_json::Value = serde_json::from_str(&enc_result.content).unwrap();
        let encoded = enc_parsed["result"].as_str().unwrap();

        let dec_call = make_call(serde_json::json!({
            "operation": "base64_decode",
            "input": encoded
        }));
        let dec_result = skill.execute(dec_call).await.unwrap();
        assert!(!dec_result.is_error);
        let dec_parsed: serde_json::Value = serde_json::from_str(&dec_result.content).unwrap();
        assert_eq!(dec_parsed["result"], "");
    }

    #[test]
    fn test_url_encode_unreserved_chars_preserved() {
        // RFC 3986 unreserved: A-Z a-z 0-9 - _ . ~
        let result = url_encode("abc-123_test.file~v2");
        assert_eq!(result, "abc-123_test.file~v2");
    }

    #[test]
    fn test_html_encode_no_special_chars() {
        assert_eq!(html_encode("hello world"), "hello world");
    }

    #[test]
    fn test_html_decode_no_entities() {
        assert_eq!(html_decode("hello world"), "hello world");
    }

    #[test]
    fn test_descriptor_name() {
        let skill = EncodeDecodeSkill::new();
        assert_eq!(skill.descriptor().name, "encode_decode");
    }
}

//! Secret scanning skill for detecting leaked credentials in text and code.
//!
//! Pure regex-based detection inspired by GitHub secret scanning, truffleHog,
//! and gitleaks. No external API calls required.
//!
//! # Supported operations
//!
//! - `scan` -- Scan text for secret patterns (API keys, tokens, passwords, etc.).
//! - `scan_diff` -- Scan only added lines in a unified diff.
//! - `classify` -- Classify a single value as a potential secret.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;

/// Skill that detects leaked secrets in text and code.
pub struct SecretScannerSkill {
    descriptor: SkillDescriptor,
}

impl SecretScannerSkill {
    /// Create a new `SecretScannerSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "secret_scanner".to_string(),
                description: "Detect leaked secrets in text/code: API keys, tokens, \
                              passwords, private keys, connection strings. \
                              Operations: scan, scan_diff, classify."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["scan", "scan_diff", "classify"],
                            "description": "The operation to perform"
                        },
                        "text": {
                            "type": "string",
                            "description": "The text to scan (for scan/scan_diff)"
                        },
                        "value": {
                            "type": "string",
                            "description": "A single value to classify (for classify)"
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

impl Default for SecretScannerSkill {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Secret patterns
// ---------------------------------------------------------------------------

struct SecretPattern {
    secret_type: &'static str,
    pattern: &'static str,
    severity: &'static str,
}

const SECRET_PATTERNS: &[SecretPattern] = &[
    // AWS
    SecretPattern {
        secret_type: "aws_access_key",
        pattern: r"\bAKIA[0-9A-Z]{16}\b",
        severity: "critical",
    },
    SecretPattern {
        secret_type: "aws_secret_key",
        pattern: r#"(?i)(?:aws_secret_access_key|aws_secret|secret_key)\s*[=:]\s*['"]?([A-Za-z0-9/+=]{40})['"]?"#,
        severity: "critical",
    },
    // GitHub tokens
    SecretPattern {
        secret_type: "github_token",
        pattern: r"\b(?:ghp|gho|ghs|ghr)_[A-Za-z0-9_]{36,}\b",
        severity: "critical",
    },
    SecretPattern {
        secret_type: "github_pat",
        pattern: r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
        severity: "critical",
    },
    // GitLab
    SecretPattern {
        secret_type: "gitlab_token",
        pattern: r"\bglpat-[A-Za-z0-9\-]{20,}\b",
        severity: "critical",
    },
    // Slack
    SecretPattern {
        secret_type: "slack_token",
        pattern: r"\bxox[bpsar]-[A-Za-z0-9\-]{10,}\b",
        severity: "critical",
    },
    // Private keys
    SecretPattern {
        secret_type: "private_key",
        pattern: r"-----BEGIN\s+(?:RSA|EC|OPENSSH|DSA|PGP)?\s*PRIVATE\s+KEY-----",
        severity: "critical",
    },
    // Database connection strings
    SecretPattern {
        secret_type: "database_url",
        pattern: r#"(?i)(?:postgres|postgresql|mysql|mongodb|redis|amqp)://[^\s'"]+:[^\s'"]+@[^\s'"]+"#,
        severity: "high",
    },
    // JWT tokens
    SecretPattern {
        secret_type: "jwt_token",
        pattern: r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
        severity: "high",
    },
    // Stripe
    SecretPattern {
        secret_type: "stripe_key",
        pattern: r"\b(?:sk|pk)_(?:live|test)_[A-Za-z0-9]{20,}\b",
        severity: "critical",
    },
    // SendGrid
    SecretPattern {
        secret_type: "sendgrid_key",
        pattern: r"\bSG\.[A-Za-z0-9_-]{22,}\.[A-Za-z0-9_-]{22,}\b",
        severity: "critical",
    },
    // Twilio
    SecretPattern {
        secret_type: "twilio_sid",
        pattern: r"\bAC[a-f0-9]{32}\b",
        severity: "high",
    },
    SecretPattern {
        secret_type: "twilio_key",
        pattern: r"\bSK[a-f0-9]{32}\b",
        severity: "high",
    },
    // Google
    SecretPattern {
        secret_type: "google_api_key",
        pattern: r"\bAIza[A-Za-z0-9_\\-]{35}\b",
        severity: "high",
    },
    // Azure
    SecretPattern {
        secret_type: "azure_key",
        pattern: r#"(?i)(?:azure|subscription)[_\s-]?(?:key|secret|password)\s*[=:]\s*['"]?[A-Za-z0-9+/=]{20,}['"]?"#,
        severity: "high",
    },
    // Generic API keys in code
    SecretPattern {
        secret_type: "generic_api_key",
        pattern: r#"(?i)(?:api[_\s-]?key|apikey|api[_\s-]?secret)\s*[=:]\s*['"][A-Za-z0-9_\-]{16,}['"]"#,
        severity: "medium",
    },
    // Authorization Bearer tokens
    SecretPattern {
        secret_type: "bearer_token",
        pattern: r"(?i)Authorization\s*:\s*Bearer\s+[A-Za-z0-9_\-.]{20,}",
        severity: "high",
    },
    // Generic password assignments in code
    SecretPattern {
        secret_type: "password_in_code",
        pattern: r#"(?i)(?:password|passwd|pwd|secret)\s*[=:]\s*['"][^'"]{8,}['"]"#,
        severity: "medium",
    },
];

/// Redact a secret value: show first 4 and last 4 chars, mask the middle.
fn redact_secret(value: &str) -> String {
    let len = value.len();
    if len <= 8 {
        return "****".to_string();
    }
    let prefix = &value[..4];
    let suffix = &value[len - 4..];
    format!("{prefix}****{suffix}")
}

/// Scan text for secrets, returning all findings with line numbers.
fn scan_text(text: &str) -> Vec<serde_json::Value> {
    let mut findings: Vec<serde_json::Value> = Vec::new();

    for sp in SECRET_PATTERNS {
        if let Ok(re) = Regex::new(sp.pattern) {
            for m in re.find_iter(text) {
                let matched = m.as_str();
                let line_num = text[..m.start()].matches('\n').count() + 1;
                findings.push(serde_json::json!({
                    "type": sp.secret_type,
                    "line": line_num,
                    "severity": sp.severity,
                    "redacted_value": redact_secret(matched),
                }));
            }
        }
    }

    findings
}

/// Classify a single value to determine if it matches any secret pattern.
fn classify_value(value: &str) -> serde_json::Value {
    for sp in SECRET_PATTERNS {
        if let Ok(re) = Regex::new(sp.pattern) {
            if re.is_match(value) {
                return serde_json::json!({
                    "is_secret": true,
                    "type": sp.secret_type,
                    "severity": sp.severity,
                    "redacted_value": redact_secret(value),
                });
            }
        }
    }

    serde_json::json!({
        "is_secret": false,
        "type": null,
        "severity": null,
        "redacted_value": null,
    })
}

// ---------------------------------------------------------------------------
// Skill implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Skill for SecretScannerSkill {
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
            "scan" => {
                let text = match call.arguments["text"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'text'",
                        ))
                    }
                };
                let findings = scan_text(text);
                let result = serde_json::json!({
                    "secrets_found": !findings.is_empty(),
                    "count": findings.len(),
                    "findings": findings,
                });
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "scan_diff" => {
                let text = match call.arguments["text"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'text'",
                        ))
                    }
                };
                // Extract only added lines (lines starting with +, but not +++ header)
                let added_lines: String = text
                    .lines()
                    .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
                    .map(|line| &line[1..]) // strip the leading '+'
                    .collect::<Vec<_>>()
                    .join("\n");

                let findings = scan_text(&added_lines);
                let result = serde_json::json!({
                    "secrets_found": !findings.is_empty(),
                    "count": findings.len(),
                    "findings": findings,
                });
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "classify" => {
                let value = match call.arguments["value"].as_str() {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'value'",
                        ))
                    }
                };
                let result = classify_value(value);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: scan, scan_diff, classify"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn skill() -> SecretScannerSkill {
        SecretScannerSkill::new()
    }

    fn make_call(op: &str, args: serde_json::Value) -> ToolCall {
        let mut merged = args.clone();
        merged["operation"] = serde_json::json!(op);
        ToolCall {
            id: "test".to_string(),
            name: "secret_scanner".to_string(),
            arguments: merged,
        }
    }

    // -- Descriptor ----------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let s = skill();
        assert_eq!(s.descriptor().name, "secret_scanner");
        assert!(s.descriptor().required_capabilities.is_empty());
    }

    #[test]
    fn test_default() {
        let s = SecretScannerSkill::default();
        assert_eq!(s.descriptor().name, "secret_scanner");
    }

    // -- AWS keys ------------------------------------------------------------

    #[tokio::test]
    async fn test_scan_aws_access_key() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
        let findings = v["findings"].as_array().unwrap();
        let aws = findings.iter().find(|f| f["type"] == "aws_access_key");
        assert!(aws.is_some());
        assert_eq!(aws.unwrap()["severity"], "critical");
        // Verify redaction
        let redacted = aws.unwrap()["redacted_value"].as_str().unwrap();
        assert!(redacted.contains("****"));
    }

    #[tokio::test]
    async fn test_scan_aws_secret_key() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "aws_secret_access_key = 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY'"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- GitHub tokens -------------------------------------------------------

    #[tokio::test]
    async fn test_scan_github_token() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "token = ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
        let findings = v["findings"].as_array().unwrap();
        let gh = findings.iter().find(|f| f["type"] == "github_token");
        assert!(gh.is_some());
    }

    #[tokio::test]
    async fn test_scan_github_pat() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "GITHUB_TOKEN=github_pat_ABCDEFGHIJKLMNOPQRSTUVWX"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- Slack tokens --------------------------------------------------------

    #[tokio::test]
    async fn test_scan_slack_token() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "SLACK_TOKEN=xoxb-123456789012-abcdefghij"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- Private keys --------------------------------------------------------

    #[tokio::test]
    async fn test_scan_private_key() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQ...\n-----END RSA PRIVATE KEY-----"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
        let findings = v["findings"].as_array().unwrap();
        let pk = findings.iter().find(|f| f["type"] == "private_key");
        assert!(pk.is_some());
        assert_eq!(pk.unwrap()["severity"], "critical");
    }

    // -- Database URLs -------------------------------------------------------

    #[tokio::test]
    async fn test_scan_database_url() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "DATABASE_URL=postgres://admin:supersecret@db.example.com:5432/mydb"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- JWT tokens ----------------------------------------------------------

    #[tokio::test]
    async fn test_scan_jwt_token() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- Stripe keys ---------------------------------------------------------

    #[tokio::test]
    async fn test_scan_generic_api_key() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "api_key = \"super_secret_key_value_1234567890abcdef\""}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- Generic password in code -------------------------------------------

    #[tokio::test]
    async fn test_scan_password_in_code() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "password = \"my_super_secret_password123\""}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- Clean text ----------------------------------------------------------

    #[tokio::test]
    async fn test_scan_clean_text() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "This is a normal README file with no secrets."}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], false);
        assert_eq!(v["count"], 0);
    }

    // -- scan_diff -----------------------------------------------------------

    #[tokio::test]
    async fn test_scan_diff_added_lines() {
        let s = skill();
        let diff = "\
--- a/config.py
+++ b/config.py
@@ -1,3 +1,4 @@
 import os
-OLD_KEY = \"safe\"
+API_KEY = \"super_secret_key_value_1234567890abcdef\"
+SECRET = \"public_value\"";
        let c = make_call("scan_diff", serde_json::json!({"text": diff}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    #[tokio::test]
    async fn test_scan_diff_removed_lines_ignored() {
        let s = skill();
        let diff = "\
--- a/config.py
+++ b/config.py
@@ -1,2 +1,2 @@
-OLD_API_KEY=super_secret_key_value_1234567890abcdef
+OLD_API_KEY=<redacted>";
        let c = make_call("scan_diff", serde_json::json!({"text": diff}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        // The removed line secret should not be reported (only added lines)
        assert_eq!(v["secrets_found"], false);
    }

    // -- classify ------------------------------------------------------------

    #[tokio::test]
    async fn test_classify_aws_key() {
        let s = skill();
        let c = make_call(
            "classify",
            serde_json::json!({"value": "AKIAIOSFODNN7EXAMPLE"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["is_secret"], true);
        assert_eq!(v["type"], "aws_access_key");
    }

    #[tokio::test]
    async fn test_classify_not_a_secret() {
        let s = skill();
        let c = make_call("classify", serde_json::json!({"value": "hello_world_123"}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["is_secret"], false);
    }

    // -- Line numbers --------------------------------------------------------

    #[tokio::test]
    async fn test_scan_line_numbers() {
        let s = skill();
        let text = "line 1\nline 2\nAKIAIOSFODNN7EXAMPLE\nline 4";
        let c = make_call("scan", serde_json::json!({"text": text}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let findings = v["findings"].as_array().unwrap();
        assert!(!findings.is_empty());
        assert_eq!(findings[0]["line"], 3);
    }

    // -- Redaction -----------------------------------------------------------

    #[test]
    fn test_redact_secret_long() {
        let r = redact_secret("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(r, "AKIA****MPLE");
    }

    #[test]
    fn test_redact_secret_short() {
        let r = redact_secret("short");
        assert_eq!(r, "****");
    }

    // -- Error handling ------------------------------------------------------

    #[tokio::test]
    async fn test_missing_operation() {
        let s = skill();
        let c = ToolCall {
            id: "test".to_string(),
            name: "secret_scanner".to_string(),
            arguments: serde_json::json!({"text": "hello"}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let s = skill();
        let c = make_call("bogus", serde_json::json!({"text": "hello"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_scan_missing_text() {
        let s = skill();
        let c = make_call("scan", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("text"));
    }

    #[tokio::test]
    async fn test_classify_missing_value() {
        let s = skill();
        let c = make_call("classify", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("value"));
    }

    // -- Google API key ------------------------------------------------------

    #[tokio::test]
    async fn test_scan_google_api_key() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "GOOGLE_KEY=AIzaSyA1234567890abcdefghijklmnopqrstuv"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }

    // -- GitLab token --------------------------------------------------------

    #[tokio::test]
    async fn test_scan_gitlab_token() {
        let s = skill();
        let c = make_call(
            "scan",
            serde_json::json!({"text": "GITLAB_TOKEN=glpat-ABCDEFGHIJKLMNOPQRST"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["secrets_found"], true);
    }
}

//! Regex operations skill for the Argentor agent framework.
//!
//! Provides a full suite of regular expression operations inspired by Python's `re`
//! module. All operations are performed in-memory with no I/O, so no security
//! capabilities are required.
//!
//! # Supported operations
//!
//! - `match` — Test whether a pattern matches and return the first match details.
//! - `match_all` — Return all matches with positions.
//! - `replace` — Replace the first occurrence of a pattern.
//! - `replace_all` — Replace all occurrences of a pattern.
//! - `split` — Split text by a pattern.
//! - `extract_groups` — Extract captured groups from the first match.
//! - `is_valid` — Check whether a regex pattern is syntactically valid.
//! - `count` — Count the number of matches.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;

/// Skill that performs regex operations on text.
pub struct RegexSkill {
    descriptor: SkillDescriptor,
}

impl RegexSkill {
    /// Create a new `RegexSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "regex".to_string(),
                description: "Perform regular expression operations on text: match, match_all, \
                              replace, replace_all, split, extract_groups, is_valid, count."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["match", "match_all", "replace", "replace_all",
                                     "split", "extract_groups", "is_valid", "count"],
                            "description": "The regex operation to perform"
                        },
                        "text": {
                            "type": "string",
                            "description": "The input text to operate on"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "The regular expression pattern"
                        },
                        "replacement": {
                            "type": "string",
                            "description": "Replacement string (for replace/replace_all)"
                        }
                    },
                    "required": ["operation", "pattern"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for RegexSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile a regex pattern, returning a user-friendly error on failure.
fn compile_regex(pattern: &str) -> Result<Regex, String> {
    Regex::new(pattern).map_err(|e| format!("Invalid regex pattern '{pattern}': {e}"))
}

/// Execute the `match` operation: test for first match and return details.
fn op_match(text: &str, pattern: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    match re.find(text) {
        Some(m) => serde_json::json!({
            "matched": true,
            "start": m.start(),
            "end": m.end(),
            "text": m.as_str(),
        }),
        None => serde_json::json!({
            "matched": false,
        }),
    }
}

/// Execute the `match_all` operation: return all matches with positions.
fn op_match_all(text: &str, pattern: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let matches: Vec<serde_json::Value> = re
        .find_iter(text)
        .map(|m| {
            serde_json::json!({
                "start": m.start(),
                "end": m.end(),
                "text": m.as_str(),
            })
        })
        .collect();

    serde_json::json!({
        "count": matches.len(),
        "matches": matches,
    })
}

/// Execute the `replace` operation: replace the first match.
fn op_replace(text: &str, pattern: &str, replacement: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let result = re.replacen(text, 1, replacement);
    let changed = result != text;

    serde_json::json!({
        "result": result.as_ref(),
        "changed": changed,
    })
}

/// Execute the `replace_all` operation: replace all matches.
fn op_replace_all(text: &str, pattern: &str, replacement: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let count = re.find_iter(text).count();
    let result = re.replace_all(text, replacement);

    serde_json::json!({
        "result": result.as_ref(),
        "replacements": count,
    })
}

/// Execute the `split` operation: split text by a pattern.
fn op_split(text: &str, pattern: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let parts: Vec<&str> = re.split(text).collect();

    serde_json::json!({
        "parts": parts,
        "count": parts.len(),
    })
}

/// Execute the `extract_groups` operation: extract captured groups from the first match.
fn op_extract_groups(text: &str, pattern: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    match re.captures(text) {
        Some(caps) => {
            let groups: Vec<serde_json::Value> = caps
                .iter()
                .enumerate()
                .map(|(i, m)| match m {
                    Some(m) => serde_json::json!({
                        "group": i,
                        "text": m.as_str(),
                        "start": m.start(),
                        "end": m.end(),
                    }),
                    None => serde_json::json!({
                        "group": i,
                        "text": null,
                        "start": null,
                        "end": null,
                    }),
                })
                .collect();

            // Named groups
            let mut named: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            for name in re.capture_names().flatten() {
                if let Some(m) = caps.name(name) {
                    named.insert(
                        name.to_string(),
                        serde_json::json!({
                            "text": m.as_str(),
                            "start": m.start(),
                            "end": m.end(),
                        }),
                    );
                }
            }

            serde_json::json!({
                "matched": true,
                "groups": groups,
                "named_groups": named,
            })
        }
        None => serde_json::json!({
            "matched": false,
            "groups": [],
            "named_groups": {},
        }),
    }
}

/// Execute the `is_valid` operation: check whether a pattern compiles.
fn op_is_valid(pattern: &str) -> serde_json::Value {
    match Regex::new(pattern) {
        Ok(_) => serde_json::json!({
            "valid": true,
            "pattern": pattern,
        }),
        Err(e) => serde_json::json!({
            "valid": false,
            "pattern": pattern,
            "error": e.to_string(),
        }),
    }
}

/// Execute the `count` operation: count how many times the pattern matches.
fn op_count(text: &str, pattern: &str) -> serde_json::Value {
    let re = match compile_regex(pattern) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let count = re.find_iter(text).count();

    serde_json::json!({
        "count": count,
    })
}

#[async_trait]
impl Skill for RegexSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let pattern = call.arguments["pattern"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if pattern.is_empty() && operation != "is_valid" {
            return Ok(ToolResult::error(&call.id, "Pattern cannot be empty"));
        }

        // For is_valid, we only need the pattern
        if operation == "is_valid" {
            let result = op_is_valid(&pattern);
            return Ok(ToolResult::success(&call.id, result.to_string()));
        }

        let text = call.arguments["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let replacement = call.arguments["replacement"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let result = match operation.as_str() {
            "match" => op_match(&text, &pattern),
            "match_all" => op_match_all(&text, &pattern),
            "replace" => op_replace(&text, &pattern, &replacement),
            "replace_all" => op_replace_all(&text, &pattern, &replacement),
            "split" => op_split(&text, &pattern),
            "extract_groups" => op_extract_groups(&text, &pattern),
            "count" => op_count(&text, &pattern),
            _ => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!(
                        "Unknown operation '{operation}'. Supported: match, match_all, \
                         replace, replace_all, split, extract_groups, is_valid, count"
                    ),
                ));
            }
        };

        // If the operation helper returned an error key, report it as a tool error
        if result.get("error").is_some() {
            return Ok(ToolResult::error(&call.id, result.to_string()));
        }

        Ok(ToolResult::success(&call.id, result.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn skill() -> RegexSkill {
        RegexSkill::new()
    }

    fn call(op: &str, args: serde_json::Value) -> ToolCall {
        let mut merged = args.clone();
        merged["operation"] = serde_json::json!(op);
        ToolCall {
            id: "t1".to_string(),
            name: "regex".to_string(),
            arguments: merged,
        }
    }

    // -- Descriptor -----------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let s = skill();
        assert_eq!(s.descriptor().name, "regex");
        assert!(s.descriptor().required_capabilities.is_empty());
    }

    // -- match ----------------------------------------------------------------

    #[tokio::test]
    async fn test_match_found() {
        let s = skill();
        let c = call(
            "match",
            serde_json::json!({"text": "hello world 42", "pattern": r"\d+"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["matched"], true);
        assert_eq!(v["text"], "42");
        assert_eq!(v["start"], 12);
        assert_eq!(v["end"], 14);
    }

    #[tokio::test]
    async fn test_match_not_found() {
        let s = skill();
        let c = call(
            "match",
            serde_json::json!({"text": "hello world", "pattern": r"\d+"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["matched"], false);
    }

    // -- match_all ------------------------------------------------------------

    #[tokio::test]
    async fn test_match_all() {
        let s = skill();
        let c = call(
            "match_all",
            serde_json::json!({"text": "a1 b2 c3", "pattern": r"[a-z]\d"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["count"], 3);
        assert_eq!(v["matches"][0]["text"], "a1");
        assert_eq!(v["matches"][1]["text"], "b2");
        assert_eq!(v["matches"][2]["text"], "c3");
    }

    #[tokio::test]
    async fn test_match_all_none() {
        let s = skill();
        let c = call(
            "match_all",
            serde_json::json!({"text": "hello", "pattern": r"\d+"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["count"], 0);
        assert!(v["matches"].as_array().unwrap().is_empty());
    }

    // -- replace --------------------------------------------------------------

    #[tokio::test]
    async fn test_replace_first() {
        let s = skill();
        let c = call(
            "replace",
            serde_json::json!({
                "text": "foo bar foo",
                "pattern": "foo",
                "replacement": "baz"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["result"], "baz bar foo");
        assert_eq!(v["changed"], true);
    }

    #[tokio::test]
    async fn test_replace_no_match() {
        let s = skill();
        let c = call(
            "replace",
            serde_json::json!({
                "text": "hello world",
                "pattern": "xyz",
                "replacement": "abc"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["result"], "hello world");
        assert_eq!(v["changed"], false);
    }

    // -- replace_all ----------------------------------------------------------

    #[tokio::test]
    async fn test_replace_all() {
        let s = skill();
        let c = call(
            "replace_all",
            serde_json::json!({
                "text": "foo bar foo baz foo",
                "pattern": "foo",
                "replacement": "qux"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["result"], "qux bar qux baz qux");
        assert_eq!(v["replacements"], 3);
    }

    // -- split ----------------------------------------------------------------

    #[tokio::test]
    async fn test_split() {
        let s = skill();
        let c = call(
            "split",
            serde_json::json!({"text": "one,two,,three", "pattern": ","}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let parts: Vec<&str> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert_eq!(parts, vec!["one", "two", "", "three"]);
        assert_eq!(v["count"], 4);
    }

    #[tokio::test]
    async fn test_split_by_whitespace() {
        let s = skill();
        let c = call(
            "split",
            serde_json::json!({"text": "hello   world  test", "pattern": r"\s+"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["count"], 3);
    }

    // -- extract_groups -------------------------------------------------------

    #[tokio::test]
    async fn test_extract_groups() {
        let s = skill();
        let c = call(
            "extract_groups",
            serde_json::json!({
                "text": "2024-03-15",
                "pattern": r"(\d{4})-(\d{2})-(\d{2})"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["matched"], true);
        assert_eq!(v["groups"][0]["text"], "2024-03-15"); // full match
        assert_eq!(v["groups"][1]["text"], "2024");
        assert_eq!(v["groups"][2]["text"], "03");
        assert_eq!(v["groups"][3]["text"], "15");
    }

    #[tokio::test]
    async fn test_extract_named_groups() {
        let s = skill();
        let c = call(
            "extract_groups",
            serde_json::json!({
                "text": "John Doe, age 30",
                "pattern": r"(?P<name>\w+ \w+), age (?P<age>\d+)"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["matched"], true);
        assert_eq!(v["named_groups"]["name"]["text"], "John Doe");
        assert_eq!(v["named_groups"]["age"]["text"], "30");
    }

    #[tokio::test]
    async fn test_extract_groups_no_match() {
        let s = skill();
        let c = call(
            "extract_groups",
            serde_json::json!({
                "text": "hello",
                "pattern": r"(\d+)-(\d+)"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["matched"], false);
    }

    // -- is_valid -------------------------------------------------------------

    #[tokio::test]
    async fn test_is_valid_good() {
        let s = skill();
        let c = call("is_valid", serde_json::json!({"pattern": r"\d+[a-z]*"}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["valid"], true);
    }

    #[tokio::test]
    async fn test_is_valid_bad() {
        let s = skill();
        let c = call("is_valid", serde_json::json!({"pattern": r"[unclosed"}));
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["valid"], false);
        assert!(v["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_is_valid_empty_pattern() {
        let s = skill();
        let c = call("is_valid", serde_json::json!({"pattern": ""}));
        let r = s.execute(c).await.unwrap();
        // Empty string is a valid regex (matches everything)
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["valid"], true);
    }

    // -- count ----------------------------------------------------------------

    #[tokio::test]
    async fn test_count() {
        let s = skill();
        let c = call(
            "count",
            serde_json::json!({"text": "abcabc123abc", "pattern": "abc"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["count"], 3);
    }

    #[tokio::test]
    async fn test_count_zero() {
        let s = skill();
        let c = call(
            "count",
            serde_json::json!({"text": "hello world", "pattern": r"\d+"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["count"], 0);
    }

    // -- Error handling -------------------------------------------------------

    #[tokio::test]
    async fn test_unknown_operation() {
        let s = skill();
        let c = call("bogus", serde_json::json!({"text": "x", "pattern": "x"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_invalid_pattern_in_match() {
        let s = skill();
        let c = call(
            "match",
            serde_json::json!({"text": "hello", "pattern": r"[bad"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_empty_pattern_error() {
        let s = skill();
        let c = call("match", serde_json::json!({"text": "hello", "pattern": ""}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Pattern cannot be empty"));
    }

    // -- Default trait --------------------------------------------------------

    #[test]
    fn test_default() {
        let s = RegexSkill::default();
        assert_eq!(s.descriptor().name, "regex");
    }
}

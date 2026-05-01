use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;

/// Text transformation skill providing a rich set of string manipulation
/// operations for AI agents. Inspired by Semantic Kernel's TextPlugin,
/// AutoGPT's text block, and LangChain text utilities.
pub struct TextTransformSkill {
    descriptor: SkillDescriptor,
}

impl TextTransformSkill {
    /// Create a new `TextTransformSkill` instance.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "text_transform".to_string(),
                description: "Perform text manipulation operations: case conversion, trimming, \
                    splitting, joining, replacing, padding, truncation, counting, searching, \
                    and case-style conversions."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "description": "The text operation to perform",
                            "enum": [
                                "uppercase", "lowercase", "title_case", "capitalize",
                                "trim", "trim_start", "trim_end",
                                "reverse", "slug",
                                "split", "join", "replace",
                                "pad_left", "pad_right",
                                "truncate",
                                "word_count", "char_count", "line_count",
                                "repeat",
                                "contains", "starts_with", "ends_with",
                                "extract_between",
                                "camel_case", "snake_case", "kebab_case"
                            ]
                        },
                        "text": {
                            "type": "string",
                            "description": "The input text to transform"
                        },
                        "delimiter": {
                            "type": "string",
                            "description": "Delimiter for split/join operations"
                        },
                        "values": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Array of strings for join operation"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "Pattern to search for in replace operation"
                        },
                        "replacement": {
                            "type": "string",
                            "description": "Replacement string for replace operation"
                        },
                        "width": {
                            "type": "integer",
                            "description": "Target width for pad_left/pad_right"
                        },
                        "char": {
                            "type": "string",
                            "description": "Single padding character (default: space)"
                        },
                        "max_length": {
                            "type": "integer",
                            "description": "Maximum length for truncate operation"
                        },
                        "suffix": {
                            "type": "string",
                            "description": "Suffix appended when truncating (default: \"...\")"
                        },
                        "count": {
                            "type": "integer",
                            "description": "Repetition count for repeat operation (max 1000)"
                        },
                        "substring": {
                            "type": "string",
                            "description": "Substring to search for in contains operation"
                        },
                        "prefix": {
                            "type": "string",
                            "description": "Prefix to check in starts_with operation"
                        },
                        "start_marker": {
                            "type": "string",
                            "description": "Start marker for extract_between"
                        },
                        "end_marker": {
                            "type": "string",
                            "description": "End marker for extract_between"
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

impl Default for TextTransformSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for TextTransformSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let args = &call.arguments;

        let operation = match args["operation"].as_str() {
            Some(op) => op,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    r#"{"error":"Missing required parameter: operation"}"#,
                ));
            }
        };

        let result = match operation {
            "uppercase" => op_uppercase(args),
            "lowercase" => op_lowercase(args),
            "title_case" => op_title_case(args),
            "capitalize" => op_capitalize(args),
            "trim" => op_trim(args),
            "trim_start" => op_trim_start(args),
            "trim_end" => op_trim_end(args),
            "reverse" => op_reverse(args),
            "slug" => op_slug(args),
            "split" => op_split(args),
            "join" => op_join(args),
            "replace" => op_replace(args),
            "pad_left" => op_pad_left(args),
            "pad_right" => op_pad_right(args),
            "truncate" => op_truncate(args),
            "word_count" => op_word_count(args),
            "char_count" => op_char_count(args),
            "line_count" => op_line_count(args),
            "repeat" => op_repeat(args),
            "contains" => op_contains(args),
            "starts_with" => op_starts_with(args),
            "ends_with" => op_ends_with(args),
            "extract_between" => op_extract_between(args),
            "camel_case" => op_camel_case(args),
            "snake_case" => op_snake_case(args),
            "kebab_case" => op_kebab_case(args),
            _ => Err(format!("Unknown operation: {operation}")),
        };

        match result {
            Ok(value) => Ok(ToolResult::success(
                &call.id,
                serde_json::json!({ "result": value }).to_string(),
            )),
            Err(e) => Ok(ToolResult::error(
                &call.id,
                serde_json::json!({ "error": e }).to_string(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: extract a required "text" string from args
// ---------------------------------------------------------------------------
fn require_text(args: &serde_json::Value) -> Result<&str, String> {
    args["text"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: text".to_string())
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

fn op_uppercase(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    Ok(serde_json::Value::String(text.to_uppercase()))
}

fn op_lowercase(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    Ok(serde_json::Value::String(text.to_lowercase()))
}

fn op_title_case(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let result = text
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    let lower: String = chars.as_str().to_lowercase();
                    format!("{upper}{lower}")
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(serde_json::Value::String(result))
}

fn op_capitalize(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    if text.is_empty() {
        return Ok(serde_json::Value::String(String::new()));
    }
    let mut chars = text.chars();
    let first: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let rest: String = chars.collect();
    Ok(serde_json::Value::String(format!("{first}{rest}")))
}

fn op_trim(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    Ok(serde_json::Value::String(text.trim().to_string()))
}

fn op_trim_start(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    Ok(serde_json::Value::String(text.trim_start().to_string()))
}

fn op_trim_end(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    Ok(serde_json::Value::String(text.trim_end().to_string()))
}

fn op_reverse(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let reversed: String = text.chars().rev().collect();
    Ok(serde_json::Value::String(reversed))
}

fn op_slug(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let slug: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and trim leading/trailing hyphens.
    let collapsed = collapse_hyphens(&slug);
    Ok(serde_json::Value::String(collapsed))
}

fn collapse_hyphens(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            prev_hyphen = false;
            result.push(c);
        }
    }
    result.trim_matches('-').to_string()
}

fn op_split(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let delimiter = args["delimiter"].as_str().unwrap_or(",");
    let parts: Vec<serde_json::Value> = text
        .split(delimiter)
        .map(|s| serde_json::Value::String(s.to_string()))
        .collect();
    Ok(serde_json::Value::Array(parts))
}

fn op_join(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let values = args["values"]
        .as_array()
        .ok_or_else(|| "Missing required parameter: values (array)".to_string())?;
    let delimiter = args["delimiter"].as_str().unwrap_or(",");
    let strings: Vec<&str> = values.iter().filter_map(|v| v.as_str()).collect();
    Ok(serde_json::Value::String(strings.join(delimiter)))
}

fn op_replace(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let pattern = args["pattern"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: pattern".to_string())?;
    let replacement = args["replacement"].as_str().unwrap_or("");
    Ok(serde_json::Value::String(
        text.replace(pattern, replacement),
    ))
}

fn op_pad_left(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let width = args["width"]
        .as_u64()
        .ok_or_else(|| "Missing required parameter: width".to_string())? as usize;
    let pad_char = extract_pad_char(args)?;
    let current_len = text.chars().count();
    if current_len >= width {
        return Ok(serde_json::Value::String(text.to_string()));
    }
    let padding: String = std::iter::repeat(pad_char)
        .take(width - current_len)
        .collect();
    Ok(serde_json::Value::String(format!("{padding}{text}")))
}

fn op_pad_right(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let width = args["width"]
        .as_u64()
        .ok_or_else(|| "Missing required parameter: width".to_string())? as usize;
    let pad_char = extract_pad_char(args)?;
    let current_len = text.chars().count();
    if current_len >= width {
        return Ok(serde_json::Value::String(text.to_string()));
    }
    let padding: String = std::iter::repeat(pad_char)
        .take(width - current_len)
        .collect();
    Ok(serde_json::Value::String(format!("{text}{padding}")))
}

fn extract_pad_char(args: &serde_json::Value) -> Result<char, String> {
    match args["char"].as_str() {
        Some(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c),
                _ => Err("Parameter 'char' must be a single character".to_string()),
            }
        }
        None => Ok(' '),
    }
}

fn op_truncate(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let max_length = args["max_length"]
        .as_u64()
        .ok_or_else(|| "Missing required parameter: max_length".to_string())?
        as usize;
    let suffix = args["suffix"].as_str().unwrap_or("...");

    let char_count = text.chars().count();
    if char_count <= max_length {
        return Ok(serde_json::Value::String(text.to_string()));
    }

    let suffix_len = suffix.chars().count();
    if max_length <= suffix_len {
        // Not enough room for even the suffix; just truncate hard.
        let truncated: String = text.chars().take(max_length).collect();
        return Ok(serde_json::Value::String(truncated));
    }

    let keep = max_length - suffix_len;
    let truncated: String = text.chars().take(keep).collect();
    Ok(serde_json::Value::String(format!("{truncated}{suffix}")))
}

fn op_word_count(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let count = text.split_whitespace().count();
    Ok(serde_json::json!(count))
}

fn op_char_count(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let count = text.chars().count();
    Ok(serde_json::json!(count))
}

fn op_line_count(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let count = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    Ok(serde_json::json!(count))
}

fn op_repeat(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let count = args["count"]
        .as_u64()
        .ok_or_else(|| "Missing required parameter: count".to_string())?;
    if count > 1000 {
        return Err("count must not exceed 1000".to_string());
    }
    Ok(serde_json::Value::String(text.repeat(count as usize)))
}

fn op_contains(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let substring = args["substring"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: substring".to_string())?;
    Ok(serde_json::Value::Bool(text.contains(substring)))
}

fn op_starts_with(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let prefix = args["prefix"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: prefix".to_string())?;
    Ok(serde_json::Value::Bool(text.starts_with(prefix)))
}

fn op_ends_with(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let suffix = args["suffix"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: suffix".to_string())?;
    Ok(serde_json::Value::Bool(text.ends_with(suffix)))
}

fn op_extract_between(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let start_marker = args["start_marker"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: start_marker".to_string())?;
    let end_marker = args["end_marker"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: end_marker".to_string())?;

    let start_pos = text
        .find(start_marker)
        .ok_or_else(|| format!("Start marker '{start_marker}' not found in text"))?;
    let after_start = start_pos + start_marker.len();
    let end_pos = text[after_start..]
        .find(end_marker)
        .ok_or_else(|| format!("End marker '{end_marker}' not found after start marker"))?;

    let extracted = &text[after_start..after_start + end_pos];
    Ok(serde_json::Value::String(extracted.to_string()))
}

/// Split text into words by whitespace and non-alphanumeric boundaries.
fn split_into_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for c in text.chars() {
        if c.is_alphanumeric() {
            current.push(c);
        } else if !current.is_empty() {
            words.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        words.push(current);
    }

    // Further split on camelCase boundaries within each word.
    let mut result = Vec::new();
    for word in words {
        let mut sub = String::new();
        let chars: Vec<char> = word.chars().collect();
        for i in 0..chars.len() {
            if i > 0 && chars[i].is_uppercase() && chars[i - 1].is_lowercase() {
                result.push(sub.clone());
                sub.clear();
            }
            sub.push(chars[i]);
        }
        if !sub.is_empty() {
            result.push(sub);
        }
    }

    result
}

fn op_camel_case(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let words = split_into_words(text);
    let mut result = String::new();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            result.push_str(&word.to_lowercase());
        } else {
            let mut chars = word.chars();
            if let Some(c) = chars.next() {
                let upper: String = c.to_uppercase().collect();
                result.push_str(&upper);
                result.push_str(&chars.as_str().to_lowercase());
            }
        }
    }
    Ok(serde_json::Value::String(result))
}

fn op_snake_case(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let words = split_into_words(text);
    let result: String = words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    Ok(serde_json::Value::String(result))
}

fn op_kebab_case(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let text = require_text(args)?;
    let words = split_into_words(text);
    let result: String = words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    Ok(serde_json::Value::String(result))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "text_transform".to_string(),
            arguments: args,
        }
    }

    async fn exec(args: serde_json::Value) -> ToolResult {
        let skill = TextTransformSkill::new();
        skill.execute(make_call(args)).await.unwrap()
    }

    fn parse_result(result: &ToolResult) -> serde_json::Value {
        serde_json::from_str(&result.content).unwrap()
    }

    // -- descriptor --------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let skill = TextTransformSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "text_transform");
        assert!(desc.required_capabilities.is_empty());
        assert!(desc.parameters_schema["properties"]["operation"].is_object());
    }

    #[test]
    fn test_default_trait() {
        let skill = TextTransformSkill::default();
        assert_eq!(skill.descriptor().name, "text_transform");
    }

    // -- missing / unknown operation ---------------------------------------

    #[tokio::test]
    async fn test_missing_operation() {
        let r = exec(json!({"text": "hello"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Missing required parameter: operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let r = exec(json!({"operation": "foobar", "text": "hello"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation: foobar"));
    }

    // -- uppercase / lowercase ---------------------------------------------

    #[tokio::test]
    async fn test_uppercase() {
        let r = exec(json!({"operation": "uppercase", "text": "hello World"})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"], "HELLO WORLD");
    }

    #[tokio::test]
    async fn test_lowercase() {
        let r = exec(json!({"operation": "lowercase", "text": "Hello WORLD"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello world");
    }

    #[tokio::test]
    async fn test_uppercase_missing_text() {
        let r = exec(json!({"operation": "uppercase"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Missing required parameter: text"));
    }

    // -- title_case --------------------------------------------------------

    #[tokio::test]
    async fn test_title_case() {
        let r = exec(json!({"operation": "title_case", "text": "hello world foo"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "Hello World Foo");
    }

    #[tokio::test]
    async fn test_title_case_mixed() {
        let r = exec(json!({"operation": "title_case", "text": "hELLO wORLD"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "Hello World");
    }

    // -- capitalize --------------------------------------------------------

    #[tokio::test]
    async fn test_capitalize() {
        let r = exec(json!({"operation": "capitalize", "text": "hello world"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "Hello world");
    }

    #[tokio::test]
    async fn test_capitalize_empty() {
        let r = exec(json!({"operation": "capitalize", "text": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "");
    }

    // -- trim / trim_start / trim_end --------------------------------------

    #[tokio::test]
    async fn test_trim() {
        let r = exec(json!({"operation": "trim", "text": "  hello  "})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello");
    }

    #[tokio::test]
    async fn test_trim_start() {
        let r = exec(json!({"operation": "trim_start", "text": "  hello  "})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello  ");
    }

    #[tokio::test]
    async fn test_trim_end() {
        let r = exec(json!({"operation": "trim_end", "text": "  hello  "})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "  hello");
    }

    // -- reverse -----------------------------------------------------------

    #[tokio::test]
    async fn test_reverse() {
        let r = exec(json!({"operation": "reverse", "text": "abcde"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "edcba");
    }

    #[tokio::test]
    async fn test_reverse_unicode() {
        let r = exec(json!({"operation": "reverse", "text": "hola"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "aloh");
    }

    // -- slug --------------------------------------------------------------

    #[tokio::test]
    async fn test_slug_basic() {
        let r = exec(json!({"operation": "slug", "text": "Hello World!"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello-world");
    }

    #[tokio::test]
    async fn test_slug_special_chars() {
        let r = exec(json!({"operation": "slug", "text": "  Foo  Bar & Baz!! "})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "foo-bar-baz");
    }

    // -- split -------------------------------------------------------------

    #[tokio::test]
    async fn test_split_default_delimiter() {
        let r = exec(json!({"operation": "split", "text": "a,b,c"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], json!(["a", "b", "c"]));
    }

    #[tokio::test]
    async fn test_split_custom_delimiter() {
        let r = exec(json!({"operation": "split", "text": "a|b|c", "delimiter": "|"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], json!(["a", "b", "c"]));
    }

    // -- join --------------------------------------------------------------

    #[tokio::test]
    async fn test_join_default_delimiter() {
        let r = exec(json!({"operation": "join", "values": ["a", "b", "c"]})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "a,b,c");
    }

    #[tokio::test]
    async fn test_join_custom_delimiter() {
        let r = exec(json!({"operation": "join", "values": ["x", "y"], "delimiter": " - "})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "x - y");
    }

    #[tokio::test]
    async fn test_join_missing_values() {
        let r = exec(json!({"operation": "join"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("values"));
    }

    // -- replace -----------------------------------------------------------

    #[tokio::test]
    async fn test_replace() {
        let r = exec(json!({
            "operation": "replace",
            "text": "hello world",
            "pattern": "world",
            "replacement": "rust"
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello rust");
    }

    #[tokio::test]
    async fn test_replace_no_replacement() {
        let r = exec(json!({
            "operation": "replace",
            "text": "hello world",
            "pattern": " world"
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello");
    }

    #[tokio::test]
    async fn test_replace_missing_pattern() {
        let r = exec(json!({"operation": "replace", "text": "hello"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("pattern"));
    }

    // -- pad_left / pad_right ----------------------------------------------

    #[tokio::test]
    async fn test_pad_left_default_char() {
        let r = exec(json!({"operation": "pad_left", "text": "hi", "width": 5})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "   hi");
    }

    #[tokio::test]
    async fn test_pad_left_custom_char() {
        let r = exec(json!({"operation": "pad_left", "text": "42", "width": 5, "char": "0"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "00042");
    }

    #[tokio::test]
    async fn test_pad_right_default_char() {
        let r = exec(json!({"operation": "pad_right", "text": "hi", "width": 5})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hi   ");
    }

    #[tokio::test]
    async fn test_pad_no_change_when_longer() {
        let r = exec(json!({"operation": "pad_left", "text": "hello", "width": 3})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello");
    }

    #[tokio::test]
    async fn test_pad_invalid_char() {
        let r = exec(json!({"operation": "pad_left", "text": "x", "width": 5, "char": "ab"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("single character"));
    }

    #[tokio::test]
    async fn test_pad_missing_width() {
        let r = exec(json!({"operation": "pad_left", "text": "x"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("width"));
    }

    // -- truncate ----------------------------------------------------------

    #[tokio::test]
    async fn test_truncate_with_default_suffix() {
        let r =
            exec(json!({"operation": "truncate", "text": "hello world", "max_length": 8})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello...");
    }

    #[tokio::test]
    async fn test_truncate_custom_suffix() {
        let r = exec(json!({
            "operation": "truncate",
            "text": "hello world",
            "max_length": 8,
            "suffix": ".."
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello ..");
    }

    #[tokio::test]
    async fn test_truncate_no_truncation_needed() {
        let r = exec(json!({"operation": "truncate", "text": "hi", "max_length": 10})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hi");
    }

    #[tokio::test]
    async fn test_truncate_very_short_max() {
        let r = exec(json!({"operation": "truncate", "text": "hello", "max_length": 2})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "he");
    }

    #[tokio::test]
    async fn test_truncate_missing_max_length() {
        let r = exec(json!({"operation": "truncate", "text": "hello"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("max_length"));
    }

    // -- word_count / char_count / line_count ------------------------------

    #[tokio::test]
    async fn test_word_count() {
        let r = exec(json!({"operation": "word_count", "text": "hello world foo"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 3);
    }

    #[tokio::test]
    async fn test_word_count_empty() {
        let r = exec(json!({"operation": "word_count", "text": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 0);
    }

    #[tokio::test]
    async fn test_char_count() {
        let r = exec(json!({"operation": "char_count", "text": "hello"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 5);
    }

    #[tokio::test]
    async fn test_char_count_unicode() {
        let r = exec(json!({"operation": "char_count", "text": "cafe\u{0301}"})).await;
        let v = parse_result(&r);
        // 'c', 'a', 'f', 'e', combining acute accent = 5 chars
        assert_eq!(v["result"], 5);
    }

    #[tokio::test]
    async fn test_line_count() {
        let r = exec(json!({"operation": "line_count", "text": "a\nb\nc"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 3);
    }

    #[tokio::test]
    async fn test_line_count_empty() {
        let r = exec(json!({"operation": "line_count", "text": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 0);
    }

    #[tokio::test]
    async fn test_line_count_single_line() {
        let r = exec(json!({"operation": "line_count", "text": "no newline"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 1);
    }

    // -- repeat ------------------------------------------------------------

    #[tokio::test]
    async fn test_repeat() {
        let r = exec(json!({"operation": "repeat", "text": "ab", "count": 3})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "ababab");
    }

    #[tokio::test]
    async fn test_repeat_zero() {
        let r = exec(json!({"operation": "repeat", "text": "ab", "count": 0})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "");
    }

    #[tokio::test]
    async fn test_repeat_exceeds_max() {
        let r = exec(json!({"operation": "repeat", "text": "x", "count": 1001})).await;
        assert!(r.is_error);
        assert!(r.content.contains("1000"));
    }

    #[tokio::test]
    async fn test_repeat_missing_count() {
        let r = exec(json!({"operation": "repeat", "text": "x"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("count"));
    }

    // -- contains / starts_with / ends_with --------------------------------

    #[tokio::test]
    async fn test_contains_true() {
        let r = exec(json!({"operation": "contains", "text": "hello world", "substring": "world"}))
            .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], true);
    }

    #[tokio::test]
    async fn test_contains_false() {
        let r =
            exec(json!({"operation": "contains", "text": "hello world", "substring": "xyz"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], false);
    }

    #[tokio::test]
    async fn test_contains_missing_substring() {
        let r = exec(json!({"operation": "contains", "text": "hello"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("substring"));
    }

    #[tokio::test]
    async fn test_starts_with_true() {
        let r = exec(json!({"operation": "starts_with", "text": "hello world", "prefix": "hello"}))
            .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], true);
    }

    #[tokio::test]
    async fn test_starts_with_false() {
        let r = exec(json!({"operation": "starts_with", "text": "hello world", "prefix": "world"}))
            .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], false);
    }

    #[tokio::test]
    async fn test_ends_with_true() {
        let r =
            exec(json!({"operation": "ends_with", "text": "hello world", "suffix": "world"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], true);
    }

    #[tokio::test]
    async fn test_ends_with_false() {
        let r =
            exec(json!({"operation": "ends_with", "text": "hello world", "suffix": "hello"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], false);
    }

    // -- extract_between ---------------------------------------------------

    #[tokio::test]
    async fn test_extract_between() {
        let r = exec(json!({
            "operation": "extract_between",
            "text": "foo [bar] baz",
            "start_marker": "[",
            "end_marker": "]"
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "bar");
    }

    #[tokio::test]
    async fn test_extract_between_html() {
        let r = exec(json!({
            "operation": "extract_between",
            "text": "<title>My Page</title>",
            "start_marker": "<title>",
            "end_marker": "</title>"
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "My Page");
    }

    #[tokio::test]
    async fn test_extract_between_start_not_found() {
        let r = exec(json!({
            "operation": "extract_between",
            "text": "hello world",
            "start_marker": "<<",
            "end_marker": ">>"
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("Start marker"));
    }

    #[tokio::test]
    async fn test_extract_between_end_not_found() {
        let r = exec(json!({
            "operation": "extract_between",
            "text": "hello << world",
            "start_marker": "<<",
            "end_marker": ">>"
        }))
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("End marker"));
    }

    // -- camel_case / snake_case / kebab_case ------------------------------

    #[tokio::test]
    async fn test_camel_case_from_spaces() {
        let r = exec(json!({"operation": "camel_case", "text": "hello world foo"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "helloWorldFoo");
    }

    #[tokio::test]
    async fn test_camel_case_from_snake() {
        let r = exec(json!({"operation": "camel_case", "text": "my_variable_name"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "myVariableName");
    }

    #[tokio::test]
    async fn test_camel_case_from_kebab() {
        let r = exec(json!({"operation": "camel_case", "text": "my-component-name"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "myComponentName");
    }

    #[tokio::test]
    async fn test_snake_case_from_camel() {
        let r = exec(json!({"operation": "snake_case", "text": "myVariableName"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "my_variable_name");
    }

    #[tokio::test]
    async fn test_snake_case_from_spaces() {
        let r = exec(json!({"operation": "snake_case", "text": "Hello World Foo"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello_world_foo");
    }

    #[tokio::test]
    async fn test_kebab_case_from_camel() {
        let r = exec(json!({"operation": "kebab_case", "text": "myComponentName"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "my-component-name");
    }

    #[tokio::test]
    async fn test_kebab_case_from_snake() {
        let r = exec(json!({"operation": "kebab_case", "text": "my_variable_name"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "my-variable-name");
    }

    // -- edge cases --------------------------------------------------------

    #[tokio::test]
    async fn test_empty_text_uppercase() {
        let r = exec(json!({"operation": "uppercase", "text": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "");
    }

    #[tokio::test]
    async fn test_split_empty_text() {
        let r = exec(json!({"operation": "split", "text": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], json!([""]));
    }

    #[tokio::test]
    async fn test_repeat_at_boundary() {
        let r = exec(json!({"operation": "repeat", "text": "x", "count": 1000})).await;
        assert!(!r.is_error);
        let v = parse_result(&r);
        assert_eq!(v["result"].as_str().unwrap().len(), 1000);
    }

    #[tokio::test]
    async fn test_slug_already_clean() {
        let r = exec(json!({"operation": "slug", "text": "already-clean"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "already-clean");
    }

    #[tokio::test]
    async fn test_replace_multiple_occurrences() {
        let r = exec(json!({
            "operation": "replace",
            "text": "aaa bbb aaa",
            "pattern": "aaa",
            "replacement": "xxx"
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "xxx bbb xxx");
    }

    #[tokio::test]
    async fn test_join_empty_array() {
        let r = exec(json!({"operation": "join", "values": []})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "");
    }

    #[tokio::test]
    async fn test_pad_right_custom_char() {
        let r =
            exec(json!({"operation": "pad_right", "text": "hi", "width": 6, "char": "."})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hi....");
    }

    #[tokio::test]
    async fn test_truncate_exact_length() {
        let r = exec(json!({"operation": "truncate", "text": "hello", "max_length": 5})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello");
    }

    #[tokio::test]
    async fn test_word_count_extra_whitespace() {
        let r = exec(json!({"operation": "word_count", "text": "  hello   world  "})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], 2);
    }

    #[tokio::test]
    async fn test_camel_case_single_word() {
        let r = exec(json!({"operation": "camel_case", "text": "hello"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello");
    }

    #[tokio::test]
    async fn test_snake_case_single_word() {
        let r = exec(json!({"operation": "snake_case", "text": "Hello"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "hello");
    }

    #[tokio::test]
    async fn test_kebab_case_with_numbers() {
        let r = exec(json!({"operation": "kebab_case", "text": "version 2 release"})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "version-2-release");
    }

    #[tokio::test]
    async fn test_extract_between_empty_content() {
        let r = exec(json!({
            "operation": "extract_between",
            "text": "[]",
            "start_marker": "[",
            "end_marker": "]"
        }))
        .await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "");
    }

    #[tokio::test]
    async fn test_contains_empty_substring() {
        let r = exec(json!({"operation": "contains", "text": "hello", "substring": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], true);
    }

    #[tokio::test]
    async fn test_reverse_empty() {
        let r = exec(json!({"operation": "reverse", "text": ""})).await;
        let v = parse_result(&r);
        assert_eq!(v["result"], "");
    }
}

//! Markdown processing skill for the Argentor AI agent framework.
//!
//! Provides Markdown to plain text conversion, heading/link/code block extraction,
//! and table of contents generation.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Markdown processing skill for text extraction and analysis.
pub struct MarkdownRendererSkill {
    descriptor: SkillDescriptor,
}

impl MarkdownRendererSkill {
    /// Create a new Markdown renderer skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "markdown_renderer".to_string(),
                description: "Markdown processing: plain text conversion, extract headings/links/code blocks, TOC generation.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["to_plain_text", "extract_headings", "extract_links", "extract_code_blocks", "generate_toc", "word_count", "extract_images"],
                            "description": "The Markdown operation to perform"
                        },
                        "markdown": {
                            "type": "string",
                            "description": "Markdown content to process"
                        }
                    },
                    "required": ["operation", "markdown"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for MarkdownRendererSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip basic Markdown formatting to produce plain text.
fn to_plain_text(md: &str) -> String {
    let mut result = String::new();
    let mut in_code_block = false;

    for line in md.lines() {
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
            if in_code_block {
                continue;
            }
            result.push('\n');
            continue;
        }
        if in_code_block {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        let mut cleaned = line.to_string();
        // Remove heading markers
        while cleaned.starts_with('#') {
            cleaned = cleaned[1..].to_string();
        }
        cleaned = cleaned.trim_start().to_string();

        // Remove bold/italic markers
        cleaned = cleaned.replace("**", "");
        cleaned = cleaned.replace("__", "");
        cleaned = cleaned.replace('*', "");
        cleaned = cleaned.replace('_', " ");

        // Convert links [text](url) -> text
        while let Some(start) = cleaned.find('[') {
            if let Some(mid) = cleaned[start..].find("](") {
                if let Some(end) = cleaned[start + mid..].find(')') {
                    let text = &cleaned[start + 1..start + mid].to_string();
                    let before = &cleaned[..start];
                    let after = &cleaned[start + mid + end + 1..];
                    cleaned = format!("{before}{text}{after}");
                    continue;
                }
            }
            break;
        }

        // Remove inline code backticks
        cleaned = cleaned.replace('`', "");

        // Remove image markers ![alt](url) -> alt
        while let Some(start) = cleaned.find("![") {
            if let Some(mid) = cleaned[start..].find("](") {
                if let Some(end) = cleaned[start + mid..].find(')') {
                    let alt = &cleaned[start + 2..start + mid].to_string();
                    let before = &cleaned[..start];
                    let after = &cleaned[start + mid + end + 1..];
                    cleaned = format!("{before}{alt}{after}");
                    continue;
                }
            }
            break;
        }

        // Remove horizontal rules
        let stripped = cleaned.trim();
        if stripped == "---" || stripped == "***" || stripped == "___" {
            result.push('\n');
            continue;
        }

        // Remove blockquote markers
        if cleaned.starts_with("> ") {
            cleaned = cleaned[2..].to_string();
        }

        result.push_str(cleaned.trim());
        result.push('\n');
    }
    result.trim().to_string()
}

/// Extract headings with their level and text.
fn extract_headings(md: &str) -> Vec<Value> {
    let mut headings = Vec::new();
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if level <= 6 {
                let text = trimmed[level..].trim().to_string();
                headings.push(json!({
                    "level": level,
                    "text": text
                }));
            }
        }
    }
    headings
}

/// Extract links as (text, url) pairs.
fn extract_links(md: &str) -> Vec<Value> {
    let mut links = Vec::new();
    let mut remaining = md;
    while let Some(start) = remaining.find('[') {
        // Skip image links
        if start > 0 && remaining.as_bytes()[start - 1] == b'!' {
            remaining = &remaining[start + 1..];
            continue;
        }
        if let Some(mid) = remaining[start..].find("](") {
            if let Some(end) = remaining[start + mid + 2..].find(')') {
                let text = &remaining[start + 1..start + mid];
                let url = &remaining[start + mid + 2..start + mid + 2 + end];
                links.push(json!({
                    "text": text,
                    "url": url
                }));
                remaining = &remaining[start + mid + 2 + end + 1..];
                continue;
            }
        }
        remaining = &remaining[start + 1..];
    }
    links
}

/// Extract fenced code blocks with optional language.
fn extract_code_blocks(md: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut language = String::new();
    let mut content = String::new();

    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_block {
                blocks.push(json!({
                    "language": if language.is_empty() { "plain".to_string() } else { language.clone() },
                    "content": content.trim_end().to_string()
                }));
                content.clear();
                language.clear();
                in_block = false;
            } else {
                language = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();
                in_block = true;
            }
        } else if in_block {
            content.push_str(line);
            content.push('\n');
        }
    }
    blocks
}

/// Extract image references.
fn extract_images(md: &str) -> Vec<Value> {
    let mut images = Vec::new();
    let mut remaining = md;
    while let Some(start) = remaining.find("![") {
        if let Some(mid) = remaining[start..].find("](") {
            if let Some(end) = remaining[start + mid + 2..].find(')') {
                let alt = &remaining[start + 2..start + mid];
                let url = &remaining[start + mid + 2..start + mid + 2 + end];
                images.push(json!({
                    "alt": alt,
                    "url": url
                }));
                remaining = &remaining[start + mid + 2 + end + 1..];
                continue;
            }
        }
        remaining = &remaining[start + 2..];
    }
    images
}

/// Generate table of contents from headings.
fn generate_toc(md: &str) -> String {
    let headings = extract_headings(md);
    let min_level = headings
        .iter()
        .filter_map(|h| h["level"].as_u64())
        .min()
        .unwrap_or(1);

    let mut toc = String::new();
    for h in &headings {
        let level = h["level"].as_u64().unwrap_or(1);
        let text = h["text"].as_str().unwrap_or("");
        let indent = "  ".repeat((level - min_level) as usize);
        let anchor = text
            .to_lowercase()
            .replace(' ', "-")
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect::<String>();
        toc.push_str(&format!("{indent}- [{text}](#{anchor})\n"));
    }
    toc.trim_end().to_string()
}

#[async_trait]
impl Skill for MarkdownRendererSkill {
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

        let markdown = match call.arguments["markdown"].as_str() {
            Some(v) => v,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'markdown'",
                ))
            }
        };

        match operation {
            "to_plain_text" => {
                let text = to_plain_text(markdown);
                let response = json!({ "text": text });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_headings" => {
                let headings = extract_headings(markdown);
                let response = json!({ "headings": headings, "count": headings.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_links" => {
                let links = extract_links(markdown);
                let response = json!({ "links": links, "count": links.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_code_blocks" => {
                let blocks = extract_code_blocks(markdown);
                let response = json!({ "code_blocks": blocks, "count": blocks.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "generate_toc" => {
                let toc = generate_toc(markdown);
                let response = json!({ "toc": toc });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "word_count" => {
                let plain = to_plain_text(markdown);
                let words: usize = plain.split_whitespace().count();
                let chars = plain.len();
                let lines = plain.lines().count();
                let response = json!({
                    "words": words,
                    "characters": chars,
                    "lines": lines
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_images" => {
                let images = extract_images(markdown);
                let response = json!({ "images": images, "count": images.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: to_plain_text, extract_headings, extract_links, extract_code_blocks, generate_toc, word_count, extract_images"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE_MD: &str = r#"# Title

Some **bold** and *italic* text.

## Section 1

A [link](https://example.com) and ![image](https://img.png).

### Subsection 1.1

```rust
fn main() {
    println!("hello");
}
```

## Section 2

> A blockquote

- list item 1
- list item 2
"#;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "markdown_renderer".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_to_plain_text() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "to_plain_text", "markdown": SAMPLE_MD}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(!text.contains("**"));
        assert!(!text.contains("##"));
        assert!(text.contains("Title"));
        assert!(text.contains("bold"));
    }

    #[tokio::test]
    async fn test_extract_headings() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "extract_headings", "markdown": SAMPLE_MD}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 4);
        let headings = parsed["headings"].as_array().unwrap();
        assert_eq!(headings[0]["level"], 1);
        assert_eq!(headings[0]["text"], "Title");
        assert_eq!(headings[1]["level"], 2);
        assert_eq!(headings[2]["level"], 3);
    }

    #[tokio::test]
    async fn test_extract_links() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "extract_links", "markdown": SAMPLE_MD}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        let links = parsed["links"].as_array().unwrap();
        assert_eq!(links[0]["text"], "link");
        assert_eq!(links[0]["url"], "https://example.com");
    }

    #[tokio::test]
    async fn test_extract_code_blocks() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "extract_code_blocks", "markdown": SAMPLE_MD}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        let blocks = parsed["code_blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["language"], "rust");
        assert!(blocks[0]["content"].as_str().unwrap().contains("println!"));
    }

    #[tokio::test]
    async fn test_generate_toc() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "generate_toc", "markdown": SAMPLE_MD}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let toc = parsed["toc"].as_str().unwrap();
        assert!(toc.contains("- [Title](#title)"));
        assert!(toc.contains("  - [Section 1](#section-1)"));
        assert!(toc.contains("    - [Subsection 1.1](#subsection-11)"));
    }

    #[tokio::test]
    async fn test_word_count() {
        let skill = MarkdownRendererSkill::new();
        let md = "# Hello\n\nThis is a test with **bold** words.";
        let call = make_call(json!({"operation": "word_count", "markdown": md}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["words"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_extract_images() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "extract_images", "markdown": SAMPLE_MD}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        let images = parsed["images"].as_array().unwrap();
        assert_eq!(images[0]["alt"], "image");
        assert_eq!(images[0]["url"], "https://img.png");
    }

    #[tokio::test]
    async fn test_empty_markdown() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "extract_headings", "markdown": ""}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[tokio::test]
    async fn test_no_code_blocks() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(
            json!({"operation": "extract_code_blocks", "markdown": "# Just a heading\nParagraph."}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[tokio::test]
    async fn test_multiple_links() {
        let skill = MarkdownRendererSkill::new();
        let md = "Visit [Google](https://google.com) or [GitHub](https://github.com).";
        let call = make_call(json!({"operation": "extract_links", "markdown": md}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
    }

    #[tokio::test]
    async fn test_blockquote_stripped() {
        let skill = MarkdownRendererSkill::new();
        let md = "> This is a quote";
        let call = make_call(json!({"operation": "to_plain_text", "markdown": md}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(!text.starts_with('>'));
        assert!(text.contains("This is a quote"));
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"markdown": "# test"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = MarkdownRendererSkill::new();
        let call = make_call(json!({"operation": "render_html", "markdown": "# test"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = MarkdownRendererSkill::new();
        assert_eq!(skill.descriptor().name, "markdown_renderer");
    }
}

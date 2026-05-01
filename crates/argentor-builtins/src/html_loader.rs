//! HTML document loader skill for the Argentor framework.
//!
//! Strips HTML tags, decodes common entities, extracts metadata, links, and
//! images. Designed for RAG pipelines that ingest raw HTML content.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};

use crate::web_scraper::strip_html_tags;

/// HTML document loader skill: tag stripping, metadata, links, images.
pub struct HtmlLoaderSkill {
    descriptor: SkillDescriptor,
}

impl HtmlLoaderSkill {
    /// Create a new HTML loader skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "html_loader".to_string(),
                description: "HTML document loader: extract_text, extract_links, extract_images, extract_metadata, strip_tags.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["extract_text", "extract_links", "extract_images", "extract_metadata", "strip_tags"],
                            "description": "The HTML operation to perform"
                        },
                        "html": {
                            "type": "string",
                            "description": "HTML content to process"
                        }
                    },
                    "required": ["operation", "html"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for HtmlLoaderSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract `<a href="...">text</a>` links from HTML.
fn extract_links_internal(html: &str) -> Vec<Value> {
    let mut links = Vec::new();
    let re = match Regex::new(r#"(?is)<a\s[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#) {
        Ok(r) => r,
        Err(_) => return links,
    };
    for caps in re.captures_iter(html) {
        let url = caps.get(1).map_or("", |m| m.as_str()).to_string();
        let raw_text = caps.get(2).map_or("", |m| m.as_str());
        let text = strip_html_tags(raw_text);
        if url.starts_with('#') || url.starts_with("javascript:") {
            continue;
        }
        links.push(json!({
            "url": url,
            "text": text.trim(),
        }));
    }
    links
}

/// Extract `<img src="..." alt="...">` images from HTML.
fn extract_images_internal(html: &str) -> Vec<Value> {
    let mut images = Vec::new();
    let re_src = match Regex::new(r#"(?is)<img\s[^>]*src=["']([^"']+)["'][^>]*>"#) {
        Ok(r) => r,
        Err(_) => return images,
    };
    let re_alt = Regex::new(r#"(?is)alt=["']([^"']*)["']"#);

    for caps in re_src.captures_iter(html) {
        let full_tag = caps.get(0).map_or("", |m| m.as_str());
        let src = caps.get(1).map_or("", |m| m.as_str()).to_string();
        let alt = if let Ok(ref re) = re_alt {
            re.captures(full_tag)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        images.push(json!({
            "src": src,
            "alt": alt,
        }));
    }
    images
}

/// Extract document title and meta description from HTML.
fn extract_metadata_internal(html: &str) -> Value {
    let mut meta = json!({});

    // Title
    if let Ok(re) = Regex::new(r"(?is)<title[^>]*>(.*?)</title>") {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                meta["title"] = Value::String(strip_html_tags(m.as_str()));
            }
        }
    }

    // Meta description
    if let Ok(re) = Regex::new(
        r#"(?is)<meta\s[^>]*name=["']description["'][^>]*content=["']([^"']+)["'][^>]*/?>"#,
    ) {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                meta["description"] = Value::String(m.as_str().to_string());
            }
        }
    }

    // Also try content before name (common alternate order)
    if meta.get("description").is_none() {
        if let Ok(re) = Regex::new(
            r#"(?is)<meta\s[^>]*content=["']([^"']+)["'][^>]*name=["']description["'][^>]*/?>"#,
        ) {
            if let Some(caps) = re.captures(html) {
                if let Some(m) = caps.get(1) {
                    meta["description"] = Value::String(m.as_str().to_string());
                }
            }
        }
    }

    // Meta keywords
    if let Ok(re) =
        Regex::new(r#"(?is)<meta\s[^>]*name=["']keywords["'][^>]*content=["']([^"']+)["'][^>]*/?>"#)
    {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                meta["keywords"] = Value::String(m.as_str().to_string());
            }
        }
    }

    // Language (html lang attribute)
    if let Ok(re) = Regex::new(r#"(?is)<html[^>]*lang=["']([^"']+)["']"#) {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                meta["lang"] = Value::String(m.as_str().to_string());
            }
        }
    }

    // Charset
    if let Ok(re) = Regex::new(r#"(?is)<meta\s[^>]*charset=["']?([A-Za-z0-9\-_]+)["']?"#) {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                meta["charset"] = Value::String(m.as_str().to_string());
            }
        }
    }

    meta
}

#[async_trait]
impl Skill for HtmlLoaderSkill {
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

        let html = match call.arguments["html"].as_str() {
            Some(v) => v,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'html'",
                ))
            }
        };

        match operation {
            "extract_text" | "strip_tags" => {
                let text = strip_html_tags(html);
                let response = json!({
                    "text": text,
                    "length": text.len(),
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_links" => {
                let links = extract_links_internal(html);
                let response = json!({ "links": links, "count": links.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_images" => {
                let images = extract_images_internal(html);
                let response = json!({ "images": images, "count": images.len() });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "extract_metadata" => {
                let meta = extract_metadata_internal(html);
                Ok(ToolResult::success(&call.id, meta.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: extract_text, extract_links, extract_images, extract_metadata, strip_tags"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Test Page</title>
    <meta name="description" content="A sample page for testing">
    <meta name="keywords" content="test, html, parser">
</head>
<body>
    <h1>Main Heading</h1>
    <p>This is a paragraph with <strong>bold</strong> text.</p>
    <a href="https://example.com">Example Link</a>
    <a href="https://github.com">GitHub</a>
    <a href="#anchor">Skip anchor</a>
    <img src="photo.jpg" alt="A photo">
    <img src="icon.png" alt="">
    <script>alert('bad');</script>
    <style>body { color: red; }</style>
</body>
</html>"##;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "html_loader".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_extract_text() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text", "html": SAMPLE_HTML}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Main Heading"));
        assert!(text.contains("paragraph"));
        assert!(text.contains("bold"));
        assert!(!text.contains("alert"), "Scripts should be stripped");
    }

    #[tokio::test]
    async fn test_strip_tags_alias() {
        let skill = HtmlLoaderSkill::new();
        let call =
            make_call(json!({"operation": "strip_tags", "html": "<p>Hello <b>World</b></p>"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Hello"));
        assert!(!text.contains('<'));
    }

    #[tokio::test]
    async fn test_extract_links() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_links", "html": SAMPLE_HTML}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        // 2 external links, skips the #anchor
        assert_eq!(parsed["count"], 2);
        let links = parsed["links"].as_array().unwrap();
        assert_eq!(links[0]["url"], "https://example.com");
        assert_eq!(links[0]["text"], "Example Link");
    }

    #[tokio::test]
    async fn test_extract_images() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_images", "html": SAMPLE_HTML}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
        let images = parsed["images"].as_array().unwrap();
        assert_eq!(images[0]["src"], "photo.jpg");
        assert_eq!(images[0]["alt"], "A photo");
        assert_eq!(images[1]["alt"], "");
    }

    #[tokio::test]
    async fn test_extract_metadata() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_metadata", "html": SAMPLE_HTML}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["title"], "Test Page");
        assert_eq!(parsed["description"], "A sample page for testing");
        assert_eq!(parsed["keywords"], "test, html, parser");
        assert_eq!(parsed["lang"], "en");
        assert_eq!(parsed["charset"], "UTF-8");
    }

    #[tokio::test]
    async fn test_extract_metadata_missing_fields() {
        let skill = HtmlLoaderSkill::new();
        let html = "<html><body><p>no meta</p></body></html>";
        let call = make_call(json!({"operation": "extract_metadata", "html": html}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed.get("title").is_none() || parsed["title"].is_null());
        assert!(parsed.get("description").is_none() || parsed["description"].is_null());
    }

    #[tokio::test]
    async fn test_extract_text_empty_html() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text", "html": ""}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["text"], "");
        assert_eq!(parsed["length"], 0);
    }

    #[tokio::test]
    async fn test_extract_links_no_links() {
        let skill = HtmlLoaderSkill::new();
        let html = "<p>No links at all.</p>";
        let call = make_call(json!({"operation": "extract_links", "html": html}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[tokio::test]
    async fn test_extract_links_skips_javascript() {
        let skill = HtmlLoaderSkill::new();
        let html = r#"<a href="javascript:alert(1)">bad</a><a href="https://ok.com">ok</a>"#;
        let call = make_call(json!({"operation": "extract_links", "html": html}));
        let result = skill.execute(call).await.unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        assert_eq!(parsed["links"][0]["url"], "https://ok.com");
    }

    #[tokio::test]
    async fn test_decodes_entities() {
        let skill = HtmlLoaderSkill::new();
        let html = "<p>Tom &amp; Jerry &lt;3</p>";
        let call = make_call(json!({"operation": "extract_text", "html": html}));
        let result = skill.execute(call).await.unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Tom & Jerry"));
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"html": "<p>hi</p>"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_html() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = HtmlLoaderSkill::new();
        let call = make_call(json!({"operation": "parse_dom", "html": "<p/>"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = HtmlLoaderSkill::new();
        assert_eq!(skill.descriptor().name, "html_loader");
    }

    #[test]
    fn test_descriptor_no_capabilities_required() {
        let skill = HtmlLoaderSkill::new();
        assert!(skill.descriptor().required_capabilities.is_empty());
    }
}

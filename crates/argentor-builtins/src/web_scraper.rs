use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;
use std::time::Duration;
use tracing::info;

const DEFAULT_MAX_LENGTH: usize = 50_000;
const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024; // 5MB

/// Strip all HTML tags from the provided string, returning clean text.
///
/// Removes `<script>` and `<style>` blocks entirely, decodes common HTML
/// entities, and collapses whitespace.
pub fn strip_html_tags(html: &str) -> String {
    // Remove script and style blocks first
    let re_script = Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap_or_else(|_| {
        // Fallback: this regex is known to be valid, but we handle gracefully
        Regex::new(r"<script>")
            .unwrap_or_else(|_| Regex::new("$^").unwrap_or_else(|_| unreachable!()))
    });
    let re_style = Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap_or_else(|_| {
        Regex::new(r"<style>")
            .unwrap_or_else(|_| Regex::new("$^").unwrap_or_else(|_| unreachable!()))
    });

    let cleaned = re_script.replace_all(html, " ");
    let cleaned = re_style.replace_all(&cleaned, " ");

    // Remove all remaining HTML tags
    let re_tags = Regex::new(r"<[^>]+>")
        .unwrap_or_else(|_| Regex::new("$^").unwrap_or_else(|_| unreachable!()));
    let text = re_tags.replace_all(&cleaned, " ");

    // Decode common HTML entities
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    // Collapse whitespace
    let re_ws =
        Regex::new(r"\s+").unwrap_or_else(|_| Regex::new(" +").unwrap_or_else(|_| unreachable!()));
    let result = re_ws.replace_all(&text, " ");

    result.trim().to_string()
}

/// Extract content within a specific tag selector (article, main, body).
/// Returns the inner content of the first matching tag, or the full HTML if
/// the selector is "all" or no match is found.
fn extract_by_selector(html: &str, selector: &str) -> String {
    if selector == "all" {
        return html.to_string();
    }

    let pattern = format!(r"(?is)<{selector}[^>]*>(.*?)</{selector}>");
    if let Ok(re) = Regex::new(&pattern) {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                return m.as_str().to_string();
            }
        }
    }

    // Fallback: return full HTML if selector not found
    html.to_string()
}

/// Extract all `<a href="...">text</a>` links from HTML.
fn extract_all_links(html: &str) -> Vec<serde_json::Value> {
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

        links.push(serde_json::json!({
            "url": url,
            "text": text.trim(),
        }));
    }

    links
}

/// Extract metadata from HTML: title, meta description, keywords, og tags.
fn extract_metadata(html: &str) -> serde_json::Value {
    let title = extract_tag_content(html, "title");

    let description = extract_meta_content(html, "description");
    let keywords = extract_meta_content(html, "keywords");

    let og_title = extract_meta_property(html, "og:title");
    let og_description = extract_meta_property(html, "og:description");
    let og_image = extract_meta_property(html, "og:image");

    serde_json::json!({
        "title": title,
        "description": description,
        "keywords": keywords,
        "og_title": og_title,
        "og_description": og_description,
        "og_image": og_image,
    })
}

/// Extract the text content of the first occurrence of a given tag.
fn extract_tag_content(html: &str, tag: &str) -> String {
    let pattern = format!(r"(?is)<{tag}[^>]*>(.*?)</{tag}>");
    if let Ok(re) = Regex::new(&pattern) {
        if let Some(caps) = re.captures(html) {
            return caps.get(1).map_or("", |m| m.as_str()).trim().to_string();
        }
    }
    String::new()
}

/// Extract `<meta name="NAME" content="...">` value.
fn extract_meta_content(html: &str, name: &str) -> String {
    let pattern =
        format!(r#"(?is)<meta\s[^>]*name=["']{name}["'][^>]*content=["']([^"']*)["'][^>]*/?\s*>"#);
    if let Ok(re) = Regex::new(&pattern) {
        if let Some(caps) = re.captures(html) {
            return caps.get(1).map_or("", |m| m.as_str()).to_string();
        }
    }
    // Try reversed attribute order: content before name
    let pattern_rev =
        format!(r#"(?is)<meta\s[^>]*content=["']([^"']*)["'][^>]*name=["']{name}["'][^>]*/?\s*>"#);
    if let Ok(re) = Regex::new(&pattern_rev) {
        if let Some(caps) = re.captures(html) {
            return caps.get(1).map_or("", |m| m.as_str()).to_string();
        }
    }
    String::new()
}

/// Extract `<meta property="PROP" content="...">` value (for Open Graph).
fn extract_meta_property(html: &str, property: &str) -> String {
    let pattern = format!(
        r#"(?is)<meta\s[^>]*property=["']{property}["'][^>]*content=["']([^"']*)["'][^>]*/?\s*>"#
    );
    if let Ok(re) = Regex::new(&pattern) {
        if let Some(caps) = re.captures(html) {
            return caps.get(1).map_or("", |m| m.as_str()).to_string();
        }
    }
    // Try reversed attribute order
    let pattern_rev = format!(
        r#"(?is)<meta\s[^>]*content=["']([^"']*)["'][^>]*property=["']{property}["'][^>]*/?\s*>"#
    );
    if let Ok(re) = Regex::new(&pattern_rev) {
        if let Some(caps) = re.captures(html) {
            return caps.get(1).map_or("", |m| m.as_str()).to_string();
        }
    }
    String::new()
}

/// Extract all h1-h6 headings with their hierarchy level.
fn extract_headings(html: &str) -> Vec<serde_json::Value> {
    let mut headings = Vec::new();

    // Rust's regex crate does not support backreferences (\1), so we match
    // each heading level individually.
    for level in 1u8..=6 {
        let pattern = format!(r"(?is)<h{level}[^>]*>(.*?)</h{level}>");
        let re = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for caps in re.captures_iter(html) {
            let raw_text = caps.get(1).map_or("", |m| m.as_str());
            let text = strip_html_tags(raw_text);

            if !text.is_empty() {
                headings.push(serde_json::json!({
                    "level": level,
                    "text": text.trim(),
                }));
            }
        }
    }

    // Sort by position in the document by re-scanning for order
    // (the above loop processes h1 first, then h2, etc.)
    // For correct document order, we find positions and sort.
    let mut positioned: Vec<(usize, serde_json::Value)> = Vec::new();
    for level in 1u8..=6 {
        let pattern = format!(r"(?is)<h{level}[^>]*>(.*?)</h{level}>");
        if let Ok(re) = Regex::new(&pattern) {
            for m in re.find_iter(html) {
                let cap = re.captures(m.as_str());
                let raw_text = cap
                    .as_ref()
                    .and_then(|c| c.get(1))
                    .map_or("", |m| m.as_str());
                let text = strip_html_tags(raw_text);
                if !text.is_empty() {
                    positioned.push((
                        m.start(),
                        serde_json::json!({
                            "level": level,
                            "text": text.trim(),
                        }),
                    ));
                }
            }
        }
    }

    positioned.sort_by_key(|(pos, _)| *pos);
    positioned.into_iter().map(|(_, v)| v).collect()
}

/// Truncate text to a maximum number of bytes, respecting char boundaries.
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &text[..end])
}

/// Web scraper skill -- extract clean text, links, metadata, and headings
/// from web pages. Inspired by Vercel web_fetch, CrewAI ScrapeWebsiteTool,
/// and Firecrawl.
///
/// Supported operations:
/// - `scrape` — fetch URL, strip HTML, return clean text
/// - `extract_links` — fetch URL, extract all `<a>` links
/// - `extract_metadata` — extract title, meta description, OG tags
/// - `extract_headings` — extract h1-h6 headings with hierarchy
/// - `extract_text` — strip HTML from a provided string (no fetch)
pub struct WebScraperSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

impl WebScraperSkill {
    /// Create a new web scraper skill.
    pub fn new() -> Self {
        #[allow(clippy::expect_used)]
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("Argentor/0.1 (AI Agent WebScraper)")
            .build()
            .expect("Failed to create HTTP client -- TLS backend unavailable");

        Self {
            descriptor: SkillDescriptor {
                name: "web_scraper".to_string(),
                description: "Extract clean text, links, metadata, and headings from web pages."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["scrape", "extract_links", "extract_metadata", "extract_headings", "extract_text"],
                            "description": "The operation to perform"
                        },
                        "url": {
                            "type": "string",
                            "description": "URL to scrape (required for scrape, extract_links, extract_metadata, extract_headings)"
                        },
                        "html": {
                            "type": "string",
                            "description": "Raw HTML string (required for extract_text)"
                        },
                        "selector": {
                            "type": "string",
                            "enum": ["article", "main", "body", "all"],
                            "description": "CSS-like selector to scope content extraction (default: all)"
                        },
                        "max_length": {
                            "type": "integer",
                            "description": "Maximum content length in characters (default: 50000)"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![],
                }],
                requires_approval: false,
            },
            client,
        }
    }

    /// Fetch the HTML body from a URL.
    async fn fetch_html(&self, url: &str, call_id: &str) -> Result<String, ToolResult> {
        if url.is_empty() {
            return Err(ToolResult::error(
                call_id,
                "URL is required for this operation",
            ));
        }

        let parsed = reqwest::Url::parse(url)
            .map_err(|e| ToolResult::error(call_id, format!("Invalid URL '{url}': {e}")))?;

        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(ToolResult::error(
                    call_id,
                    format!("Unsupported scheme '{scheme}'. Only http/https allowed."),
                ));
            }
        }

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ToolResult::error(call_id, format!("Failed to fetch '{url}': {e}")))?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(ToolResult::error(
                call_id,
                format!("HTTP {status} from {url}"),
            ));
        }

        let body_bytes = response.bytes().await.map_err(|e| {
            ToolResult::error(call_id, format!("Failed to read response body: {e}"))
        })?;

        if body_bytes.len() > MAX_RESPONSE_SIZE {
            return Err(ToolResult::error(
                call_id,
                format!(
                    "Response too large: {} bytes (max {})",
                    body_bytes.len(),
                    MAX_RESPONSE_SIZE
                ),
            ));
        }

        Ok(String::from_utf8_lossy(&body_bytes).to_string())
    }
}

impl Default for WebScraperSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for WebScraperSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let max_length = call.arguments["max_length"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_LENGTH);

        let selector = call.arguments["selector"]
            .as_str()
            .unwrap_or("all")
            .to_string();

        info!(operation = %operation, "WebScraper execute");

        match operation.as_str() {
            "scrape" => {
                let url = call.arguments["url"].as_str().unwrap_or_default();
                let html = match self.fetch_html(url, &call.id).await {
                    Ok(h) => h,
                    Err(err_result) => return Ok(err_result),
                };

                let scoped = extract_by_selector(&html, &selector);
                let text = strip_html_tags(&scoped);
                let truncated = truncate_text(&text, max_length);

                let result = serde_json::json!({
                    "url": url,
                    "selector": selector,
                    "text": truncated,
                    "length": text.len(),
                    "truncated": text.len() > max_length,
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "extract_links" => {
                let url = call.arguments["url"].as_str().unwrap_or_default();
                let html = match self.fetch_html(url, &call.id).await {
                    Ok(h) => h,
                    Err(err_result) => return Ok(err_result),
                };

                let links = extract_all_links(&html);

                let result = serde_json::json!({
                    "url": url,
                    "links": links,
                    "count": links.len(),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "extract_metadata" => {
                let url = call.arguments["url"].as_str().unwrap_or_default();
                let html = match self.fetch_html(url, &call.id).await {
                    Ok(h) => h,
                    Err(err_result) => return Ok(err_result),
                };

                let metadata = extract_metadata(&html);

                let result = serde_json::json!({
                    "url": url,
                    "metadata": metadata,
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "extract_headings" => {
                let url = call.arguments["url"].as_str().unwrap_or_default();
                let html = match self.fetch_html(url, &call.id).await {
                    Ok(h) => h,
                    Err(err_result) => return Ok(err_result),
                };

                let headings = extract_headings(&html);

                let result = serde_json::json!({
                    "url": url,
                    "headings": headings,
                    "count": headings.len(),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "extract_text" => {
                let html = call.arguments["html"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                if html.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "The 'html' parameter is required for extract_text",
                    ));
                }

                let text = strip_html_tags(&html);
                let truncated = truncate_text(&text, max_length);

                let result = serde_json::json!({
                    "text": truncated,
                    "length": text.len(),
                    "truncated": text.len() > max_length,
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation '{operation}'. Valid: scrape, extract_links, extract_metadata, extract_headings, extract_text"
                ),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE_HTML: &str = r##"<!DOCTYPE html>
<html>
<head>
    <title>Test Page</title>
    <meta name="description" content="A test page for scraping">
    <meta name="keywords" content="test, scraping, html">
    <meta property="og:title" content="OG Test Title">
    <meta property="og:description" content="OG test description">
    <meta property="og:image" content="https://example.com/image.png">
</head>
<body>
    <h1>Main Heading</h1>
    <article>
        <h2>Article Title</h2>
        <p>This is the article content with <strong>bold text</strong> and a
        <a href="https://example.com">link to example</a>.</p>
        <p>Second paragraph with &amp; entities &lt;here&gt;.</p>
    </article>
    <main>
        <h3>Main Section</h3>
        <p>Main content area.</p>
        <a href="/about">About Us</a>
        <a href="https://other.com/page">Other Page</a>
    </main>
    <script>var x = "should be removed";</script>
    <style>.hidden { display: none; }</style>
    <footer>
        <a href="#top">Back to top</a>
        <a href="javascript:void(0)">JS Link</a>
    </footer>
</body>
</html>"##;

    #[test]
    fn test_strip_html_tags_basic() {
        let html = "<p>Hello <b>World</b></p>";
        let result = strip_html_tags(html);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_strip_html_tags_script_removal() {
        let html = "<p>Before</p><script>alert('xss')</script><p>After</p>";
        let result = strip_html_tags(html);
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
        assert!(!result.contains("alert"));
        assert!(!result.contains("xss"));
    }

    #[test]
    fn test_strip_html_tags_style_removal() {
        let html = "<style>.a{color:red}</style><p>Content</p>";
        let result = strip_html_tags(html);
        assert!(result.contains("Content"));
        assert!(!result.contains("color"));
    }

    #[test]
    fn test_strip_html_tags_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D &quot;E&quot; F&#39;s</p>";
        let result = strip_html_tags(html);
        assert!(result.contains("A & B < C > D \"E\" F's"));
    }

    #[test]
    fn test_strip_html_tags_whitespace_collapse() {
        let html = "<p>  Multiple   spaces   here  </p>";
        let result = strip_html_tags(html);
        assert_eq!(result, "Multiple spaces here");
    }

    #[test]
    fn test_extract_by_selector_article() {
        let content = extract_by_selector(SAMPLE_HTML, "article");
        assert!(content.contains("Article Title"));
        assert!(content.contains("article content"));
        // Should not contain main section content
        assert!(!content.contains("Main Section"));
    }

    #[test]
    fn test_extract_by_selector_main() {
        let content = extract_by_selector(SAMPLE_HTML, "main");
        assert!(content.contains("Main Section"));
        assert!(content.contains("Main content area"));
    }

    #[test]
    fn test_extract_by_selector_all() {
        let content = extract_by_selector(SAMPLE_HTML, "all");
        assert_eq!(content, SAMPLE_HTML);
    }

    #[test]
    fn test_extract_by_selector_missing() {
        let content = extract_by_selector(SAMPLE_HTML, "nav");
        // Falls back to full HTML when selector not found
        assert_eq!(content, SAMPLE_HTML);
    }

    #[test]
    fn test_extract_all_links() {
        let links = extract_all_links(SAMPLE_HTML);
        // Should find example.com, /about, other.com links
        // but skip #top and javascript: links
        let urls: Vec<&str> = links.iter().filter_map(|l| l["url"].as_str()).collect();
        assert!(urls.contains(&"https://example.com"));
        assert!(urls.contains(&"/about"));
        assert!(urls.contains(&"https://other.com/page"));
        assert!(!urls.iter().any(|u| u.starts_with('#')));
        assert!(!urls.iter().any(|u| u.starts_with("javascript:")));
    }

    #[test]
    fn test_extract_all_links_text() {
        let links = extract_all_links(SAMPLE_HTML);
        let example_link = links
            .iter()
            .find(|l| l["url"].as_str() == Some("https://example.com"))
            .unwrap();
        assert_eq!(example_link["text"].as_str().unwrap(), "link to example");
    }

    #[test]
    fn test_extract_metadata() {
        let meta = extract_metadata(SAMPLE_HTML);
        assert_eq!(meta["title"], "Test Page");
        assert_eq!(meta["description"], "A test page for scraping");
        assert_eq!(meta["keywords"], "test, scraping, html");
        assert_eq!(meta["og_title"], "OG Test Title");
        assert_eq!(meta["og_description"], "OG test description");
        assert_eq!(meta["og_image"], "https://example.com/image.png");
    }

    #[test]
    fn test_extract_metadata_empty() {
        let html = "<html><body><p>No metadata here</p></body></html>";
        let meta = extract_metadata(html);
        assert_eq!(meta["title"], "");
        assert_eq!(meta["description"], "");
    }

    #[test]
    fn test_extract_headings() {
        let headings = extract_headings(SAMPLE_HTML);
        assert!(headings.len() >= 3);

        let h1 = headings.iter().find(|h| h["level"] == 1).unwrap();
        assert_eq!(h1["text"].as_str().unwrap(), "Main Heading");

        let h2 = headings.iter().find(|h| h["level"] == 2).unwrap();
        assert_eq!(h2["text"].as_str().unwrap(), "Article Title");

        let h3 = headings.iter().find(|h| h["level"] == 3).unwrap();
        assert_eq!(h3["text"].as_str().unwrap(), "Main Section");
    }

    #[test]
    fn test_truncate_text() {
        let short = "Hello World";
        assert_eq!(truncate_text(short, 100), "Hello World");

        let long = "a".repeat(100);
        let truncated = truncate_text(&long, 50);
        assert!(truncated.len() < 100);
        assert!(truncated.ends_with("...[truncated]"));
    }

    #[tokio::test]
    async fn test_extract_text_operation() {
        let skill = WebScraperSkill::new();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "web_scraper".to_string(),
            arguments: serde_json::json!({
                "operation": "extract_text",
                "html": "<h1>Hello</h1><p>World with <b>bold</b> text</p>"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(text.contains("bold"));
        assert!(!text.contains("<h1>"));
        assert!(!text.contains("<b>"));
    }

    #[tokio::test]
    async fn test_extract_text_missing_html() {
        let skill = WebScraperSkill::new();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "web_scraper".to_string(),
            arguments: serde_json::json!({
                "operation": "extract_text",
                "html": ""
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = WebScraperSkill::new();
        let call = ToolCall {
            id: "t3".to_string(),
            name: "web_scraper".to_string(),
            arguments: serde_json::json!({
                "operation": "invalid_op"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_extract_text_with_max_length() {
        let skill = WebScraperSkill::new();
        let long_content = format!("<p>{}</p>", "word ".repeat(20000));
        let call = ToolCall {
            id: "t4".to_string(),
            name: "web_scraper".to_string(),
            arguments: serde_json::json!({
                "operation": "extract_text",
                "html": long_content,
                "max_length": 100
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["truncated"].as_bool().unwrap());
        let text = parsed["text"].as_str().unwrap();
        assert!(text.ends_with("...[truncated]"));
    }

    #[test]
    fn test_descriptor() {
        let skill = WebScraperSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "web_scraper");
        assert!(!desc.required_capabilities.is_empty());
    }
}

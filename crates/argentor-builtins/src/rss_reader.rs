use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;
use std::time::Duration;
use tracing::info;

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;
const MAX_DESCRIPTION_LENGTH: usize = 500;
const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024; // 5MB

/// A single RSS/Atom feed item.
#[derive(Debug, serde::Serialize)]
struct FeedItem {
    title: String,
    link: String,
    description: String,
    pub_date: String,
    author: String,
}

/// Feed-level metadata.
#[derive(Debug, serde::Serialize)]
struct FeedMetadata {
    title: String,
    description: String,
    link: String,
    language: String,
    last_build_date: String,
}

/// Extract text content from between XML tags: `<tag>content</tag>`.
/// Returns an empty string if the tag is not found.
fn extract_xml_tag(xml: &str, tag: &str) -> String {
    let pattern = format!(r"(?is)<{tag}[^>]*>(.*?)</{tag}>");
    if let Ok(re) = Regex::new(&pattern) {
        if let Some(caps) = re.captures(xml) {
            let content = caps.get(1).map_or("", |m| m.as_str()).trim();
            // Handle CDATA sections
            return strip_cdata(content);
        }
    }
    String::new()
}

/// Strip CDATA wrappers from content: `<![CDATA[...]]>` -> `...`
fn strip_cdata(text: &str) -> String {
    if let Ok(re) = Regex::new(r"(?s)<!\[CDATA\[(.*?)\]\]>") {
        let result = re.replace_all(text, "$1");
        return result.trim().to_string();
    }
    text.trim().to_string()
}

/// Strip all XML/HTML tags from content to produce plain text.
fn strip_tags(text: &str) -> String {
    if let Ok(re) = Regex::new(r"<[^>]+>") {
        let result = re.replace_all(text, " ");
        // Decode common entities
        let result = result
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&#39;", "'")
            .replace("&nbsp;", " ");
        // Collapse whitespace
        if let Ok(ws_re) = Regex::new(r"\s+") {
            return ws_re.replace_all(&result, " ").trim().to_string();
        }
        return result.trim().to_string();
    }
    text.to_string()
}

/// Truncate a string to max_len characters, adding "..." if truncated.
fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Detect whether the XML is RSS 2.0 or Atom 1.0 format.
fn detect_feed_type(xml: &str) -> FeedType {
    let lower = xml.to_lowercase();
    if lower.contains("<feed")
        && (lower.contains("xmlns=\"http://www.w3.org/2005/atom\"") || lower.contains("atom"))
    {
        FeedType::Atom
    } else {
        FeedType::Rss
    }
}

#[derive(Debug, PartialEq)]
enum FeedType {
    Rss,
    Atom,
}

/// Parse RSS 2.0 feed metadata from the `<channel>` element.
fn parse_rss_metadata(xml: &str) -> FeedMetadata {
    // Extract the channel element (excluding items)
    let channel_header = if let Ok(re) = Regex::new(r"(?is)<channel>(.*?)<item") {
        re.captures(xml)
            .and_then(|c| c.get(1))
            .map_or(xml.to_string(), |m| m.as_str().to_string())
    } else {
        xml.to_string()
    };

    FeedMetadata {
        title: extract_xml_tag(&channel_header, "title"),
        description: extract_xml_tag(&channel_header, "description"),
        link: extract_xml_tag(&channel_header, "link"),
        language: extract_xml_tag(&channel_header, "language"),
        last_build_date: extract_xml_tag(&channel_header, "lastBuildDate"),
    }
}

/// Parse Atom 1.0 feed metadata from the `<feed>` element.
fn parse_atom_metadata(xml: &str) -> FeedMetadata {
    // Extract the feed header (before first entry)
    let feed_header = if let Ok(re) = Regex::new(r"(?is)<feed[^>]*>(.*?)<entry") {
        re.captures(xml)
            .and_then(|c| c.get(1))
            .map_or(xml.to_string(), |m| m.as_str().to_string())
    } else {
        xml.to_string()
    };

    let link = extract_atom_link(&feed_header);

    FeedMetadata {
        title: extract_xml_tag(&feed_header, "title"),
        description: extract_xml_tag(&feed_header, "subtitle"),
        link,
        language: extract_xml_tag(&feed_header, "xml:lang"),
        last_build_date: extract_xml_tag(&feed_header, "updated"),
    }
}

/// Extract the href attribute from an Atom `<link>` element.
fn extract_atom_link(xml: &str) -> String {
    // Look for <link href="..." /> or <link rel="alternate" href="..." />
    if let Ok(re) = Regex::new(r#"(?i)<link[^>]*href=["']([^"']+)["'][^>]*/?\s*>"#) {
        if let Some(caps) = re.captures(xml) {
            return caps.get(1).map_or("", |m| m.as_str()).to_string();
        }
    }
    String::new()
}

/// Parse RSS 2.0 items from the XML.
fn parse_rss_items(xml: &str, limit: usize) -> Vec<FeedItem> {
    let mut items = Vec::new();

    let re = match Regex::new(r"(?is)<item>(.*?)</item>") {
        Ok(r) => r,
        Err(_) => return items,
    };

    for caps in re.captures_iter(xml) {
        if items.len() >= limit {
            break;
        }

        let item_xml = caps.get(1).map_or("", |m| m.as_str());

        let title = extract_xml_tag(item_xml, "title");
        let link = extract_xml_tag(item_xml, "link");
        let raw_description = extract_xml_tag(item_xml, "description");
        let description = truncate(&strip_tags(&raw_description), MAX_DESCRIPTION_LENGTH);
        let pub_date = extract_xml_tag(item_xml, "pubDate");
        let author = {
            let a = extract_xml_tag(item_xml, "author");
            if a.is_empty() {
                extract_xml_tag(item_xml, "dc:creator")
            } else {
                a
            }
        };

        items.push(FeedItem {
            title,
            link,
            description,
            pub_date,
            author,
        });
    }

    items
}

/// Parse Atom 1.0 entries from the XML.
fn parse_atom_items(xml: &str, limit: usize) -> Vec<FeedItem> {
    let mut items = Vec::new();

    let re = match Regex::new(r"(?is)<entry>(.*?)</entry>") {
        Ok(r) => r,
        Err(_) => return items,
    };

    for caps in re.captures_iter(xml) {
        if items.len() >= limit {
            break;
        }

        let entry_xml = caps.get(1).map_or("", |m| m.as_str());

        let title = extract_xml_tag(entry_xml, "title");
        let link = extract_atom_link(entry_xml);
        let raw_content = {
            let c = extract_xml_tag(entry_xml, "content");
            if c.is_empty() {
                extract_xml_tag(entry_xml, "summary")
            } else {
                c
            }
        };
        let description = truncate(&strip_tags(&raw_content), MAX_DESCRIPTION_LENGTH);
        let pub_date = {
            let p = extract_xml_tag(entry_xml, "published");
            if p.is_empty() {
                extract_xml_tag(entry_xml, "updated")
            } else {
                p
            }
        };
        let author = extract_xml_tag(entry_xml, "name"); // Inside <author><name>

        items.push(FeedItem {
            title,
            link,
            description,
            pub_date,
            author,
        });
    }

    items
}

/// Parse a feed (auto-detecting RSS vs Atom) and return metadata + items.
fn parse_feed(xml: &str, limit: usize) -> (FeedMetadata, Vec<FeedItem>) {
    let clamped_limit = limit.min(MAX_LIMIT);

    match detect_feed_type(xml) {
        FeedType::Atom => {
            let metadata = parse_atom_metadata(xml);
            let items = parse_atom_items(xml, clamped_limit);
            (metadata, items)
        }
        FeedType::Rss => {
            let metadata = parse_rss_metadata(xml);
            let items = parse_rss_items(xml, clamped_limit);
            (metadata, items)
        }
    }
}

/// RSS/Atom feed reader skill -- fetch and parse RSS/Atom feeds, search feed
/// content. Inspired by AutoGPT RSS block.
///
/// Supported operations:
/// - `fetch` — fetch and parse a feed URL, return items
/// - `parse` — parse a provided XML string as RSS/Atom
/// - `search` — fetch a feed and filter items by query text
pub struct RssReaderSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

impl RssReaderSkill {
    /// Create a new RSS/Atom feed reader skill.
    pub fn new() -> Self {
        #[allow(clippy::expect_used)]
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("Argentor/0.1 (AI Agent RssReader)")
            .build()
            .expect("Failed to create HTTP client -- TLS backend unavailable");

        Self {
            descriptor: SkillDescriptor {
                name: "rss_reader".to_string(),
                description: "Fetch and parse RSS/Atom feeds, search feed content.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["fetch", "parse", "search"],
                            "description": "The operation to perform"
                        },
                        "url": {
                            "type": "string",
                            "description": "Feed URL (for fetch, search)"
                        },
                        "xml": {
                            "type": "string",
                            "description": "Raw XML string (for parse)"
                        },
                        "query": {
                            "type": "string",
                            "description": "Search query to filter items (for search)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of items to return (default: 10, max: 50)"
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

    /// Fetch XML from a URL.
    async fn fetch_xml(&self, url: &str, call_id: &str) -> Result<String, ToolResult> {
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

        if !response.status().is_success() {
            let status = response.status().as_u16();
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

impl Default for RssReaderSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for RssReaderSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let limit = call.arguments["limit"]
            .as_u64()
            .map(|v| (v as usize).min(MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);

        info!(operation = %operation, "RssReader execute");

        match operation.as_str() {
            "fetch" => {
                let url = call.arguments["url"].as_str().unwrap_or_default();
                let xml = match self.fetch_xml(url, &call.id).await {
                    Ok(x) => x,
                    Err(err_result) => return Ok(err_result),
                };

                let (metadata, items) = parse_feed(&xml, limit);

                let result = serde_json::json!({
                    "url": url,
                    "feed": metadata,
                    "items": items,
                    "count": items.len(),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "parse" => {
                let xml = call.arguments["xml"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                if xml.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "The 'xml' parameter is required for parse",
                    ));
                }

                let (metadata, items) = parse_feed(&xml, limit);

                let result = serde_json::json!({
                    "feed": metadata,
                    "items": items,
                    "count": items.len(),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "search" => {
                let url = call.arguments["url"].as_str().unwrap_or_default();
                let query = call.arguments["query"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                if query.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "The 'query' parameter is required for search",
                    ));
                }

                let xml = match self.fetch_xml(url, &call.id).await {
                    Ok(x) => x,
                    Err(err_result) => return Ok(err_result),
                };

                // Parse all items (use MAX_LIMIT to get full set for filtering)
                let (metadata, all_items) = parse_feed(&xml, MAX_LIMIT);

                let query_lower = query.to_lowercase();
                let filtered: Vec<&FeedItem> = all_items
                    .iter()
                    .filter(|item| {
                        item.title.to_lowercase().contains(&query_lower)
                            || item.description.to_lowercase().contains(&query_lower)
                    })
                    .take(limit)
                    .collect();

                let result = serde_json::json!({
                    "url": url,
                    "query": query,
                    "feed": metadata,
                    "items": filtered,
                    "count": filtered.len(),
                    "total_items": all_items.len(),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation '{operation}'. Valid: fetch, parse, search"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Tech News Feed</title>
    <link>https://example.com</link>
    <description>Latest tech news and updates</description>
    <language>en-us</language>
    <lastBuildDate>Mon, 01 Apr 2026 12:00:00 GMT</lastBuildDate>
    <item>
      <title>Rust 2.0 Released</title>
      <link>https://example.com/rust-2</link>
      <description><![CDATA[<p>The Rust programming language has reached version 2.0 with exciting new features.</p>]]></description>
      <pubDate>Mon, 01 Apr 2026 10:00:00 GMT</pubDate>
      <author>editor@example.com</author>
    </item>
    <item>
      <title>AI Agents Evolve</title>
      <link>https://example.com/ai-agents</link>
      <description>Autonomous AI agents are becoming more capable and secure.</description>
      <pubDate>Sun, 31 Mar 2026 08:00:00 GMT</pubDate>
      <dc:creator>Jane Smith</dc:creator>
    </item>
    <item>
      <title>WebAssembly Updates</title>
      <link>https://example.com/wasm</link>
      <description>WASM component model reaches stable status.</description>
      <pubDate>Sat, 30 Mar 2026 14:00:00 GMT</pubDate>
      <author>wasm@example.com</author>
    </item>
  </channel>
</rss>"#;

    const SAMPLE_ATOM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Blog</title>
  <subtitle>An example Atom feed</subtitle>
  <link href="https://atom-blog.example.com"/>
  <updated>2026-04-01T12:00:00Z</updated>
  <entry>
    <title>First Atom Post</title>
    <link href="https://atom-blog.example.com/post-1"/>
    <summary>This is the first post in our Atom feed.</summary>
    <published>2026-04-01T10:00:00Z</published>
    <author><name>Alice</name></author>
  </entry>
  <entry>
    <title>Second Atom Post</title>
    <link href="https://atom-blog.example.com/post-2"/>
    <content type="html"><![CDATA[<p>Second post with <b>HTML</b> content.</p>]]></content>
    <updated>2026-03-31T08:00:00Z</updated>
    <author><name>Bob</name></author>
  </entry>
</feed>"#;

    #[test]
    fn test_descriptor() {
        let skill = RssReaderSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "rss_reader");
        assert!(!desc.required_capabilities.is_empty());
    }

    #[test]
    fn test_detect_feed_type_rss() {
        assert_eq!(detect_feed_type(SAMPLE_RSS), FeedType::Rss);
    }

    #[test]
    fn test_detect_feed_type_atom() {
        assert_eq!(detect_feed_type(SAMPLE_ATOM), FeedType::Atom);
    }

    #[test]
    fn test_parse_rss_metadata() {
        let meta = parse_rss_metadata(SAMPLE_RSS);
        assert_eq!(meta.title, "Tech News Feed");
        assert_eq!(meta.link, "https://example.com");
        assert_eq!(meta.description, "Latest tech news and updates");
        assert_eq!(meta.language, "en-us");
        assert!(!meta.last_build_date.is_empty());
    }

    #[test]
    fn test_parse_rss_items() {
        let items = parse_rss_items(SAMPLE_RSS, 10);
        assert_eq!(items.len(), 3);

        assert_eq!(items[0].title, "Rust 2.0 Released");
        assert_eq!(items[0].link, "https://example.com/rust-2");
        assert!(items[0].description.contains("Rust programming language"));
        assert!(!items[0].pub_date.is_empty());

        assert_eq!(items[1].title, "AI Agents Evolve");
        assert_eq!(items[1].author, "Jane Smith");

        assert_eq!(items[2].title, "WebAssembly Updates");
    }

    #[test]
    fn test_parse_rss_items_with_limit() {
        let items = parse_rss_items(SAMPLE_RSS, 2);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_parse_atom_metadata() {
        let meta = parse_atom_metadata(SAMPLE_ATOM);
        assert_eq!(meta.title, "Atom Blog");
        assert_eq!(meta.description, "An example Atom feed");
        assert_eq!(meta.link, "https://atom-blog.example.com");
        assert!(!meta.last_build_date.is_empty()); // updated field
    }

    #[test]
    fn test_parse_atom_items() {
        let items = parse_atom_items(SAMPLE_ATOM, 10);
        assert_eq!(items.len(), 2);

        assert_eq!(items[0].title, "First Atom Post");
        assert_eq!(items[0].link, "https://atom-blog.example.com/post-1");
        assert!(items[0].description.contains("first post"));
        assert_eq!(items[0].author, "Alice");

        assert_eq!(items[1].title, "Second Atom Post");
        assert!(items[1].description.contains("Second post"));
        assert_eq!(items[1].author, "Bob");
    }

    #[test]
    fn test_parse_feed_rss() {
        let (meta, items) = parse_feed(SAMPLE_RSS, 10);
        assert_eq!(meta.title, "Tech News Feed");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_parse_feed_atom() {
        let (meta, items) = parse_feed(SAMPLE_ATOM, 10);
        assert_eq!(meta.title, "Atom Blog");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_strip_cdata() {
        let text = "<![CDATA[Hello World]]>";
        assert_eq!(strip_cdata(text), "Hello World");

        let text_no_cdata = "plain text";
        assert_eq!(strip_cdata(text_no_cdata), "plain text");
    }

    #[test]
    fn test_strip_tags() {
        let html = "<p>Hello <b>bold</b> &amp; <i>italic</i></p>";
        let text = strip_tags(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("bold"));
        assert!(text.contains("&"));
        assert!(text.contains("italic"));
        assert!(!text.contains("<p>"));
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 100), "short");
        let long = "a".repeat(600);
        let result = truncate(&long, 500);
        assert!(result.len() < 600);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_extract_xml_tag() {
        let xml = "<item><title>Test Title</title><link>https://example.com</link></item>";
        assert_eq!(extract_xml_tag(xml, "title"), "Test Title");
        assert_eq!(extract_xml_tag(xml, "link"), "https://example.com");
        assert_eq!(extract_xml_tag(xml, "missing"), "");
    }

    #[tokio::test]
    async fn test_parse_operation() {
        let skill = RssReaderSkill::new();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "rss_reader".to_string(),
            arguments: serde_json::json!({
                "operation": "parse",
                "xml": SAMPLE_RSS,
                "limit": 2
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "parse should succeed: {}", result.content);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
        assert_eq!(parsed["feed"]["title"], "Tech News Feed");
    }

    #[tokio::test]
    async fn test_parse_operation_atom() {
        let skill = RssReaderSkill::new();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "rss_reader".to_string(),
            arguments: serde_json::json!({
                "operation": "parse",
                "xml": SAMPLE_ATOM
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
        assert_eq!(parsed["feed"]["title"], "Atom Blog");
    }

    #[tokio::test]
    async fn test_parse_operation_empty_xml() {
        let skill = RssReaderSkill::new();
        let call = ToolCall {
            id: "t3".to_string(),
            name: "rss_reader".to_string(),
            arguments: serde_json::json!({
                "operation": "parse",
                "xml": ""
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = RssReaderSkill::new();
        let call = ToolCall {
            id: "t4".to_string(),
            name: "rss_reader".to_string(),
            arguments: serde_json::json!({
                "operation": "invalid"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_cdata_in_rss_description() {
        let items = parse_rss_items(SAMPLE_RSS, 10);
        // First item has CDATA-wrapped description
        assert!(items[0].description.contains("Rust programming language"));
        // Should not contain CDATA markers or HTML tags
        assert!(!items[0].description.contains("CDATA"));
        assert!(!items[0].description.contains("<p>"));
    }

    #[test]
    fn test_max_limit_clamping() {
        // Even if we request 100, should be clamped to MAX_LIMIT
        let (_, items) = parse_feed(SAMPLE_RSS, 100);
        assert!(items.len() <= MAX_LIMIT);
    }
}

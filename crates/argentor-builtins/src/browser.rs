use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::time::Duration;
use tracing::info;

const MAX_PAGE_SIZE: usize = 5 * 1024 * 1024; // 5MB

/// Browser skill — fetches a URL and extracts readable text content from HTML.
/// Unlike `http_fetch`, this skill parses HTML and returns clean text,
/// making it suitable for agents that need to "read" web pages.
pub struct BrowserSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

impl BrowserSkill {
    /// Create a new browser skill with an HTTP client.
    pub fn new() -> Self {
        // reqwest::Client::builder().build() only fails if TLS backend
        // initialization fails, which indicates a fundamentally broken
        // environment. We allow expect here because there is no meaningful
        // recovery path at construction time.
        #[allow(clippy::expect_used)]
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("Argentor/0.1 (AI Agent Browser)")
            .build()
            .expect("Failed to create HTTP client for browser — TLS backend unavailable");

        Self {
            descriptor: SkillDescriptor {
                name: "browser".to_string(),
                description: "Browse a web page and extract its text content, links, and metadata."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to browse"
                        },
                        "extract": {
                            "type": "string",
                            "enum": ["text", "links", "all"],
                            "description": "What to extract: 'text' (default), 'links', or 'all'"
                        }
                    },
                    "required": ["url"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![],
                }],
                requires_approval: false,
            },
            client,
        }
    }
}

impl Default for BrowserSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for BrowserSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let url = call.arguments["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if url.is_empty() {
            return Ok(ToolResult::error(&call.id, "Empty URL"));
        }

        // Validate URL
        let parsed = match reqwest::Url::parse(&url) {
            Ok(u) => u,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Invalid URL '{url}': {e}"),
                ));
            }
        };

        // Only allow http/https
        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Unsupported scheme '{scheme}'. Only http/https."),
                ));
            }
        }

        // SSRF prevention
        if let Some(host) = parsed.host_str() {
            if is_private_host(host) {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Access denied: '{host}' is a private address"),
                ));
            }
        }

        let extract = call.arguments["extract"]
            .as_str()
            .unwrap_or("text")
            .to_string();

        info!(url = %url, extract = %extract, "Browser fetch");

        let response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Failed to fetch '{url}': {e}"),
                ));
            }
        };

        let status = response.status().as_u16();
        let final_url = response.url().to_string();

        if !response.status().is_success() {
            return Ok(ToolResult::error(
                &call.id,
                format!("HTTP {status} from {final_url}"),
            ));
        }

        let body_bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Failed to read body: {e}"),
                ));
            }
        };

        if body_bytes.len() > MAX_PAGE_SIZE {
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "Page too large: {} bytes (max {})",
                    body_bytes.len(),
                    MAX_PAGE_SIZE
                ),
            ));
        }

        let html = String::from_utf8_lossy(&body_bytes).to_string();

        let title = extract_title(&html);
        let text = extract_text(&html);
        let links = extract_links(&html, &final_url);

        let result = match extract.as_str() {
            "links" => serde_json::json!({
                "url": final_url,
                "status": status,
                "title": title,
                "links": links,
            }),
            "all" => serde_json::json!({
                "url": final_url,
                "status": status,
                "title": title,
                "text": truncate_text(&text, 50_000),
                "links": links,
            }),
            _ => serde_json::json!({
                "url": final_url,
                "status": status,
                "title": title,
                "text": truncate_text(&text, 50_000),
            }),
        };

        Ok(ToolResult::success(&call.id, result.to_string()))
    }
}

/// Extract the <title> tag content.
fn extract_title(html: &str) -> String {
    let lower = html.to_lowercase();
    if let Some(start) = lower.find("<title") {
        if let Some(tag_end) = lower[start..].find('>') {
            let content_start = start + tag_end + 1;
            if let Some(end) = lower[content_start..].find("</title>") {
                return html[content_start..content_start + end].trim().to_string();
            }
        }
    }
    String::new()
}

/// Extract visible text from HTML by stripping tags and decoding common entities.
fn extract_text(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && chars[i] == '<' {
            // Check for script/style open/close
            let remaining: String = lower_chars[i..std::cmp::min(i + 20, len)].iter().collect();
            if remaining.starts_with("<script") {
                in_script = true;
            } else if remaining.starts_with("</script") {
                in_script = false;
            } else if remaining.starts_with("<style") {
                in_style = true;
            } else if remaining.starts_with("</style") {
                in_style = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }

        if in_tag {
            if chars[i] == '>' {
                in_tag = false;
                // Add space after block elements
                if !result.ends_with(' ') && !result.ends_with('\n') {
                    result.push(' ');
                }
            }
            i += 1;
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        // Handle HTML entities
        if chars[i] == '&' {
            let remaining: String = chars[i..std::cmp::min(i + 10, len)].iter().collect();
            if remaining.starts_with("&amp;") {
                result.push('&');
                i += 5;
                continue;
            } else if remaining.starts_with("&lt;") {
                result.push('<');
                i += 4;
                continue;
            } else if remaining.starts_with("&gt;") {
                result.push('>');
                i += 4;
                continue;
            } else if remaining.starts_with("&quot;") {
                result.push('"');
                i += 6;
                continue;
            } else if remaining.starts_with("&nbsp;") {
                result.push(' ');
                i += 6;
                continue;
            } else if remaining.starts_with("&#39;") || remaining.starts_with("&apos;") {
                result.push('\'');
                i += if remaining.starts_with("&#39;") { 5 } else { 6 };
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    // Collapse whitespace
    let mut cleaned = String::with_capacity(result.len());
    let mut prev_space = false;
    for ch in result.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                cleaned.push(if ch == '\n' { '\n' } else { ' ' });
            }
            prev_space = true;
        } else {
            cleaned.push(ch);
            prev_space = false;
        }
    }

    cleaned.trim().to_string()
}

/// Extract links (href attributes) from HTML.
fn extract_links(html: &str, base_url: &str) -> Vec<serde_json::Value> {
    let mut links = Vec::new();
    let lower = html.to_lowercase();
    let base = reqwest::Url::parse(base_url).ok();

    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find("href=") {
        let abs_pos = search_from + pos + 5;
        search_from = abs_pos;

        if abs_pos >= lower.len() {
            break;
        }

        let quote = html.as_bytes().get(abs_pos).copied();
        let (start, end_char) = match quote {
            Some(b'"') => (abs_pos + 1, '"'),
            Some(b'\'') => (abs_pos + 1, '\''),
            _ => continue,
        };

        if let Some(end) = html[start..].find(end_char) {
            let href = &html[start..start + end];
            if href.starts_with('#') || href.starts_with("javascript:") {
                continue;
            }

            let full_url = if href.starts_with("http://") || href.starts_with("https://") {
                href.to_string()
            } else if let Some(ref base) = base {
                base.join(href)
                    .map_or_else(|_| href.to_string(), |u| u.to_string())
            } else {
                href.to_string()
            };

            // Extract link text (simplified: get text between > and next <)
            let after_href = start + end;
            let text = extract_link_text(html, after_href);

            links.push(serde_json::json!({
                "url": full_url,
                "text": text,
            }));
        }
    }

    links
}

/// Try to extract the text content of a link after its href attribute.
fn extract_link_text(html: &str, from: usize) -> String {
    // Find the next '>' after the href attribute
    if let Some(tag_close) = html[from..].find('>') {
        let text_start = from + tag_close + 1;
        if let Some(tag_open) = html[text_start..].find('<') {
            let text = html[text_start..text_start + tag_open].trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    String::new()
}

fn truncate_text(text: &str, max_chars: usize) -> &str {
    if text.len() <= max_chars {
        text
    } else {
        // Find a char boundary
        let mut end = max_chars;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }
}

fn is_private_host(host: &str) -> bool {
    let private_patterns = [
        "localhost",
        "127.",
        "10.",
        "172.16.",
        "172.17.",
        "172.18.",
        "172.19.",
        "172.20.",
        "172.21.",
        "172.22.",
        "172.23.",
        "172.24.",
        "172.25.",
        "172.26.",
        "172.27.",
        "172.28.",
        "172.29.",
        "172.30.",
        "172.31.",
        "192.168.",
        "169.254.",
        "0.0.0.0",
        "[::1]",
        "metadata.google",
        "metadata.aws",
    ];
    let host_lower = host.to_lowercase();
    private_patterns
        .iter()
        .any(|p| host_lower.starts_with(p) || host_lower == *p)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title() {
        let html = "<html><head><title>My Page</title></head><body></body></html>";
        assert_eq!(extract_title(html), "My Page");
    }

    #[test]
    fn test_extract_title_missing() {
        let html = "<html><body>No title here</body></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn test_extract_text_simple() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = extract_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
    }

    #[test]
    fn test_extract_text_strips_script() {
        let html = "<html><body><p>Before</p><script>var x = 1;</script><p>After</p></body></html>";
        let text = extract_text(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("var x"));
    }

    #[test]
    fn test_extract_text_strips_style() {
        let html = "<html><body><style>.a{color:red}</style><p>Content</p></body></html>";
        let text = extract_text(html);
        assert!(text.contains("Content"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn test_extract_text_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D &quot;E&quot;</p>";
        let text = extract_text(html);
        assert!(text.contains("A & B < C > D \"E\""));
    }

    #[test]
    fn test_extract_links() {
        let html = r#"<a href="https://example.com">Example</a><a href="/about">About</a>"#;
        let links = extract_links(html, "https://mysite.com");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0]["url"], "https://example.com");
        assert_eq!(links[0]["text"], "Example");
        assert_eq!(links[1]["url"], "https://mysite.com/about");
        assert_eq!(links[1]["text"], "About");
    }

    #[test]
    fn test_extract_links_skips_fragments() {
        let html = r##"<a href="#section">Section</a><a href="javascript:void(0)">JS</a>"##;
        let links = extract_links(html, "https://mysite.com");
        assert!(links.is_empty());
    }

    #[test]
    fn test_truncate_text() {
        assert_eq!(truncate_text("hello", 10), "hello");
        assert_eq!(truncate_text("hello world", 5), "hello");
    }

    #[tokio::test]
    async fn test_browser_empty_url() {
        let skill = BrowserSkill::new();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "browser".to_string(),
            arguments: serde_json::json!({"url": ""}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_browser_invalid_scheme() {
        let skill = BrowserSkill::new();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "browser".to_string(),
            arguments: serde_json::json!({"url": "ftp://example.com"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unsupported scheme"));
    }

    #[tokio::test]
    async fn test_browser_blocks_ssrf() {
        let skill = BrowserSkill::new();
        let call = ToolCall {
            id: "t3".to_string(),
            name: "browser".to_string(),
            arguments: serde_json::json!({"url": "http://169.254.169.254/latest"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("private"));
    }
}

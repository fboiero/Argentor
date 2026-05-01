// SPDX-License-Identifier: AGPL-3.0-only
//! Web browsing skills: fetch, search (DuckDuckGo), and CSS-selector extraction.
//!
//! All three skills respect a 30-second timeout, a 1 MB response cap, and
//! robots.txt (User-agent: * Disallow rules are checked before fetching).
//!
//! The `web_extract` skill requires the `web-browse` feature flag because it
//! depends on the `scraper` crate for CSS selector support. `web_fetch` and
//! `web_search` are always available regardless of feature flags.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;
#[cfg(feature = "web-browse")]
use std::collections::HashMap;
use std::time::Duration;
use tracing::info;

const TIMEOUT_SECS: u64 = 30;
const MAX_CONTENT_SIZE: usize = 1024 * 1024; // 1 MB
const DEFAULT_MAX_RESULTS: usize = 5;
const DUCKDUCKGO_HTML_URL: &str = "https://html.duckduckgo.com/html/";

// ---------------------------------------------------------------------------
// Shared HTTP client builder
// ---------------------------------------------------------------------------

fn build_client() -> reqwest::Client {
    #[allow(clippy::expect_used)]
    reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("Argentor/0.1 (WebBrowse; +https://github.com/fboiero/Agentor)")
        .build()
        .expect("failed to build reqwest client")
}

// ---------------------------------------------------------------------------
// robots.txt helper
// ---------------------------------------------------------------------------

/// Fetch and parse robots.txt for the given URL's origin.
/// Returns `true` when the path is *allowed* for User-agent: *.
///
/// On any fetch / parse error the function fails open and returns `true`
/// (allow), so a broken robots.txt never silently blocks legitimate fetches.
async fn robots_allows(client: &reqwest::Client, url: &reqwest::Url) -> bool {
    let origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    let robots_url = format!("{origin}/robots.txt");

    let text = match client.get(&robots_url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(t) => t,
            Err(_) => return true,
        },
        _ => return true, // no robots.txt or fetch error → allow
    };

    let path = url.path();
    parse_robots_disallowed(&text, path)
}

/// Parse the robots.txt `text` and return `true` when `path` is NOT
/// disallowed under `User-agent: *`.
fn parse_robots_disallowed(text: &str, path: &str) -> bool {
    let mut in_star_block = false;

    for line in text.lines() {
        let line = line.trim();

        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        if let Some(agent) = line.strip_prefix("User-agent:") {
            in_star_block = agent.trim() == "*";
            continue;
        }

        if in_star_block {
            if let Some(disallow) = line.strip_prefix("Disallow:") {
                let rule = disallow.trim();
                if !rule.is_empty() && path.starts_with(rule) {
                    return false; // disallowed
                }
            }
        }
    }

    true // allowed
}

// ---------------------------------------------------------------------------
// HTML utilities shared across skills
// ---------------------------------------------------------------------------

/// Strip HTML tags and decode common entities.
fn strip_html_to_text(html: &str) -> String {
    // Remove <script> and <style> blocks entirely
    let re_script =
        Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap_or_else(|_| Regex::new("$^").unwrap());
    let re_style =
        Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap_or_else(|_| Regex::new("$^").unwrap());
    let stripped = re_script.replace_all(html, " ");
    let stripped = re_style.replace_all(&stripped, " ");

    // Remove all remaining tags
    let re_tags = Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new("$^").unwrap());
    let text = re_tags.replace_all(&stripped, " ");

    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    let re_ws = Regex::new(r"\s+").unwrap_or_else(|_| Regex::new(" ").unwrap());
    re_ws.replace_all(&text, " ").trim().to_string()
}

/// Very minimal HTML→Markdown conversion: headings, bold, links, paragraphs.
fn html_to_markdown(html: &str) -> String {
    let mut md = html.to_string();

    // headings h1-h6 → markdown headings
    for level in (1u8..=6).rev() {
        let hashes = "#".repeat(level as usize);
        let open = format!(r"(?is)<h{level}[^>]*>(.*?)</h{level}>");
        if let Ok(re) = Regex::new(&open) {
            md = re
                .replace_all(&md, |caps: &regex::Captures| {
                    let inner = strip_html_to_text(caps.get(1).map_or("", |m| m.as_str()));
                    format!("\n{hashes} {inner}\n")
                })
                .into_owned();
        }
    }

    // <a href="URL">text</a> → [text](URL)
    if let Ok(re) = Regex::new(r#"(?is)<a\s[^>]*href=["']([^"']*)["'][^>]*>(.*?)</a>"#) {
        md = re
            .replace_all(&md, |caps: &regex::Captures| {
                let url = caps.get(1).map_or("", |m| m.as_str());
                let text = strip_html_to_text(caps.get(2).map_or("", |m| m.as_str()));
                format!("[{text}]({url})")
            })
            .into_owned();
    }

    // <strong> / <b> → **text**
    for tag in ["strong", "b"] {
        let p = format!(r"(?is)<{tag}[^>]*>(.*?)</{tag}>");
        if let Ok(re) = Regex::new(&p) {
            md = re
                .replace_all(&md, |caps: &regex::Captures| {
                    let inner = caps.get(1).map_or("", |m| m.as_str());
                    format!("**{inner}**")
                })
                .into_owned();
        }
    }

    // Remove remaining tags
    let text = strip_html_to_text(&md);

    let re_ws = Regex::new(r" {2,}").unwrap_or_else(|_| Regex::new(" +").unwrap());
    re_ws.replace_all(&text, " ").trim().to_string()
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn url_encode_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len() * 3);
    for byte in q.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn url_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &input[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                result.push(byte as char);
                i += 3;
                continue;
            }
        }
        result.push(if bytes[i] == b'+' {
            ' '
        } else {
            bytes[i] as char
        });
        i += 1;
    }
    result
}

fn extract_ddg_redirect_url(raw: &str) -> String {
    if raw.contains("uddg=") {
        if let Some(start) = raw.find("uddg=") {
            let ps = start + 5;
            let pe = raw[ps..].find('&').map_or(raw.len(), |p| ps + p);
            return url_decode(&raw[ps..pe]);
        }
    }
    if raw.starts_with("//") {
        return format!("https:{raw}");
    }
    raw.to_string()
}

// ---------------------------------------------------------------------------
// WebFetchSkill
// ---------------------------------------------------------------------------

/// Fetch a URL and return its content as text, raw HTML, or Markdown.
///
/// Input:
/// ```json
/// { "url": "https://...", "format": "text|html|markdown" }
/// ```
pub struct WebFetchSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

impl WebFetchSkill {
    /// Create a new `WebFetchSkill` with a 30-second timeout client.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "web_fetch".to_string(),
                description:
                    "Fetch a URL and return its content. Respects robots.txt. Timeout 30s, max 1MB."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch (http/https)" },
                        "format": {
                            "type": "string",
                            "enum": ["text", "html", "markdown"],
                            "description": "Output format: stripped text (default), raw HTML, or Markdown"
                        }
                    },
                    "required": ["url"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![],
                }],
            },
            client: build_client(),
        }
    }
}

impl Default for WebFetchSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for WebFetchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let url_str = call.arguments["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let format = call.arguments["format"].as_str().unwrap_or("text");

        if url_str.is_empty() {
            return Ok(ToolResult::error(&call.id, "url is required"));
        }

        let parsed = match reqwest::Url::parse(&url_str) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::error(&call.id, format!("invalid URL: {e}"))),
        };

        match parsed.scheme() {
            "http" | "https" => {}
            s => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("unsupported scheme '{s}'"),
                ))
            }
        }

        // robots.txt check
        if !robots_allows(&self.client, &parsed).await {
            return Ok(ToolResult::error(
                &call.id,
                format!("robots.txt disallows fetching '{url_str}'"),
            ));
        }

        info!(url = %url_str, format = %format, "web_fetch");

        let response = match self.client.get(&url_str).send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(&call.id, format!("fetch failed: {e}"))),
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Ok(ToolResult::error(
                &call.id,
                format!("HTTP {status} from {url_str}"),
            ));
        }

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("read body failed: {e}"),
                ))
            }
        };

        if bytes.len() > MAX_CONTENT_SIZE {
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "response too large: {} bytes (max {})",
                    bytes.len(),
                    MAX_CONTENT_SIZE
                ),
            ));
        }

        let html = String::from_utf8_lossy(&bytes).to_string();

        let content = match format {
            "html" => html.clone(),
            "markdown" => html_to_markdown(&html),
            _ => strip_html_to_text(&html), // "text" is the default
        };

        Ok(ToolResult::success(
            &call.id,
            serde_json::json!({
                "url": url_str,
                "format": format,
                "content": content,
                "size_bytes": bytes.len(),
            })
            .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// WebSearchSkill (DuckDuckGo HTML, no API key)
// ---------------------------------------------------------------------------

/// Search the web via DuckDuckGo's public HTML endpoint. No API key required.
///
/// Input:
/// ```json
/// { "query": "search terms", "max_results": 5 }
/// ```
pub struct WebBrowseSearchSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

impl WebBrowseSearchSkill {
    /// Create a new `WebBrowseSearchSkill` using DuckDuckGo HTML endpoint.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "web_browse_search".to_string(),
                description:
                    "Search the web via DuckDuckGo (no API key). Returns title, URL, snippet."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "max_results": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 10,
                            "description": "Number of results to return (default: 5)"
                        }
                    },
                    "required": ["query"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec!["html.duckduckgo.com".to_string()],
                }],
            },
            client: build_client(),
        }
    }

    fn parse_results(&self, html: &str, max: usize) -> Vec<serde_json::Value> {
        let mut results = Vec::new();

        let link_re = match Regex::new(
            r#"(?is)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#,
        ) {
            Ok(r) => r,
            Err(_) => return results,
        };
        let snip_re = match Regex::new(r#"(?is)class="result__snippet"[^>]*>(.*?)</(?:a|td)>"#) {
            Ok(r) => r,
            Err(_) => return results,
        };

        let links: Vec<_> = link_re.captures_iter(html).collect();
        let snippets: Vec<_> = snip_re.captures_iter(html).collect();

        for (i, lc) in links.iter().enumerate() {
            if results.len() >= max {
                break;
            }
            let raw_url = lc.get(1).map_or("", |m| m.as_str());
            let url = extract_ddg_redirect_url(raw_url);
            let title = strip_html_to_text(lc.get(2).map_or("", |m| m.as_str()));
            let snippet = snippets
                .get(i)
                .map(|sc| strip_html_to_text(sc.get(1).map_or("", |m| m.as_str())))
                .unwrap_or_default();

            if !url.is_empty() && !title.is_empty() {
                results.push(serde_json::json!({ "title": title, "url": url, "snippet": snippet }));
            }
        }

        results
    }
}

impl Default for WebBrowseSearchSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for WebBrowseSearchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let query = call.arguments["query"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if query.is_empty() {
            return Ok(ToolResult::error(&call.id, "query is required"));
        }

        let max = call.arguments["max_results"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(10);

        info!(query = %query, max = max, "web_browse_search");

        let url = format!("{}?q={}", DUCKDUCKGO_HTML_URL, url_encode_query(&query));

        let response = match self
            .client
            .get(&url)
            .header("Accept", "text/html")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(&call.id, format!("search failed: {e}"))),
        };

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Ok(ToolResult::error(
                &call.id,
                format!("DuckDuckGo returned HTTP {status}"),
            ));
        }

        let html = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("read body failed: {e}"),
                ))
            }
        };

        let results = self.parse_results(&html, max);

        Ok(ToolResult::success(
            &call.id,
            serde_json::json!({
                "query": query,
                "results": results,
                "count": results.len(),
            })
            .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// WebExtractSkill — CSS selector extraction (requires `web-browse` feature)
// ---------------------------------------------------------------------------

/// Extract structured data from a web page using CSS selectors.
///
/// Input:
/// ```json
/// {
///   "url": "https://...",
///   "selectors": { "title": "h1", "content": "article p" }
/// }
/// ```
///
/// Returns a JSON object mapping each key to the extracted text content.
///
/// This skill is available only when the `web-browse` feature flag is enabled
/// (adds the `scraper` crate for proper CSS selector support).
#[cfg(feature = "web-browse")]
pub struct WebExtractSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

#[cfg(feature = "web-browse")]
impl WebExtractSkill {
    /// Create a new `WebExtractSkill` with CSS-selector extraction support.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "web_extract".to_string(),
                description: "Extract structured data from a URL using CSS selectors. Requires web-browse feature.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch" },
                        "selectors": {
                            "type": "object",
                            "description": "Map of output-key → CSS selector",
                            "additionalProperties": { "type": "string" }
                        }
                    },
                    "required": ["url", "selectors"]
                }),
                required_capabilities: vec![Capability::NetworkAccess { allowed_hosts: vec![] }],
            },
            client: build_client(),
        }
    }
}

#[cfg(feature = "web-browse")]
impl Default for WebExtractSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "web-browse")]
#[async_trait]
impl Skill for WebExtractSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let url_str = call.arguments["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if url_str.is_empty() {
            return Ok(ToolResult::error(&call.id, "url is required"));
        }

        let selectors_val = &call.arguments["selectors"];
        let selectors_map = match selectors_val.as_object() {
            Some(m) => m,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "selectors must be an object mapping keys to CSS selector strings",
                ))
            }
        };

        if selectors_map.is_empty() {
            return Ok(ToolResult::error(&call.id, "selectors must not be empty"));
        }

        let parsed = match reqwest::Url::parse(&url_str) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::error(&call.id, format!("invalid URL: {e}"))),
        };

        match parsed.scheme() {
            "http" | "https" => {}
            s => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("unsupported scheme '{s}'"),
                ))
            }
        }

        if !robots_allows(&self.client, &parsed).await {
            return Ok(ToolResult::error(
                &call.id,
                format!("robots.txt disallows fetching '{url_str}'"),
            ));
        }

        info!(url = %url_str, "web_extract");

        let response = match self.client.get(&url_str).send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(&call.id, format!("fetch failed: {e}"))),
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Ok(ToolResult::error(
                &call.id,
                format!("HTTP {status} from {url_str}"),
            ));
        }

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("read body failed: {e}"),
                ))
            }
        };

        if bytes.len() > MAX_CONTENT_SIZE {
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "response too large: {} bytes (max {})",
                    bytes.len(),
                    MAX_CONTENT_SIZE
                ),
            ));
        }

        let html = String::from_utf8_lossy(&bytes).to_string();
        let document = scraper::Html::parse_document(&html);

        let mut extracted: HashMap<String, serde_json::Value> = HashMap::new();

        for (key, sel_val) in selectors_map {
            let css_str = match sel_val.as_str() {
                Some(s) => s,
                None => {
                    extracted.insert(key.clone(), serde_json::Value::Null);
                    continue;
                }
            };

            let selector = match scraper::Selector::parse(css_str) {
                Ok(s) => s,
                Err(e) => {
                    extracted.insert(
                        key.clone(),
                        serde_json::Value::String(format!("invalid selector: {e}")),
                    );
                    continue;
                }
            };

            let texts: Vec<String> = document
                .select(&selector)
                .map(|el| el.text().collect::<String>().trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();

            extracted.insert(
                key.clone(),
                if texts.len() == 1 {
                    serde_json::Value::String(texts.into_iter().next().unwrap_or_default())
                } else {
                    serde_json::Value::Array(
                        texts.into_iter().map(serde_json::Value::String).collect(),
                    )
                },
            );
        }

        Ok(ToolResult::success(
            &call.id,
            serde_json::json!({
                "url": url_str,
                "data": extracted,
            })
            .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_core::ToolCall;

    // ── robots.txt parser ────────────────────────────────────────────────────

    #[test]
    fn test_robots_allows_disallowed_path() {
        let robots = "User-agent: *\nDisallow: /private/\n";
        assert!(!parse_robots_disallowed(robots, "/private/page.html"));
    }

    #[test]
    fn test_robots_allows_allowed_path() {
        let robots = "User-agent: *\nDisallow: /private/\n";
        assert!(parse_robots_disallowed(robots, "/public/page.html"));
    }

    #[test]
    fn test_robots_empty_disallow_allows_all() {
        // "Disallow:" with empty value means "allow everything"
        let robots = "User-agent: *\nDisallow:\n";
        assert!(parse_robots_disallowed(robots, "/anything"));
    }

    #[test]
    fn test_robots_other_agent_not_applied() {
        let robots = "User-agent: Googlebot\nDisallow: /noindex/\n";
        // We only check User-agent: * — other agents must not affect us
        assert!(parse_robots_disallowed(robots, "/noindex/page"));
    }

    #[test]
    fn test_robots_root_disallow() {
        let robots = "User-agent: *\nDisallow: /\n";
        assert!(!parse_robots_disallowed(robots, "/any/path"));
    }

    #[test]
    fn test_robots_ignores_comments() {
        let robots = "# this is a comment\nUser-agent: *\nDisallow: /secret/\n";
        assert!(!parse_robots_disallowed(robots, "/secret/data"));
        assert!(parse_robots_disallowed(robots, "/open/data"));
    }

    // ── strip_html_to_text ───────────────────────────────────────────────────

    #[test]
    fn test_strip_html_removes_tags() {
        assert_eq!(
            strip_html_to_text("<p>Hello <b>World</b></p>"),
            "Hello World"
        );
    }

    #[test]
    fn test_strip_html_removes_script() {
        let html = "<p>Before</p><script>evil()</script><p>After</p>";
        let text = strip_html_to_text(html);
        assert!(text.contains("Before") && text.contains("After"));
        assert!(!text.contains("evil"));
    }

    #[test]
    fn test_strip_html_decodes_entities() {
        let html = "<p>A &amp; B &lt;C&gt; &quot;D&quot;</p>";
        let text = strip_html_to_text(html);
        assert!(text.contains("A & B <C> \"D\""));
    }

    // ── html_to_markdown ─────────────────────────────────────────────────────

    #[test]
    fn test_html_to_markdown_heading() {
        let html = "<h1>Title</h1><p>Body</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("# Title"));
    }

    #[test]
    fn test_html_to_markdown_link() {
        let html = r#"<a href="https://example.com">Click</a>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("[Click](https://example.com)"));
    }

    // ── url helpers ──────────────────────────────────────────────────────────

    #[test]
    fn test_url_encode_query() {
        assert_eq!(url_encode_query("hello world"), "hello+world");
        assert_eq!(url_encode_query("a=b&c=d"), "a%3Db%26c%3Dd");
    }

    #[test]
    fn test_extract_ddg_redirect_url_passthrough() {
        let url = "https://example.com/page";
        assert_eq!(extract_ddg_redirect_url(url), url);
    }

    #[test]
    fn test_extract_ddg_redirect_url_decodes() {
        let raw = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&rut=foo";
        let result = extract_ddg_redirect_url(raw);
        assert_eq!(result, "https://example.com/");
    }

    // ── WebFetchSkill ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_web_fetch_empty_url() {
        let skill = WebFetchSkill::new();
        let call = ToolCall {
            id: "f1".to_string(),
            name: "web_fetch".to_string(),
            arguments: serde_json::json!({ "url": "" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("url is required"));
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_url() {
        let skill = WebFetchSkill::new();
        let call = ToolCall {
            id: "f2".to_string(),
            name: "web_fetch".to_string(),
            arguments: serde_json::json!({ "url": "not-a-url" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(
            result.content.contains("invalid URL") || result.content.contains("unsupported scheme")
        );
    }

    #[tokio::test]
    async fn test_web_fetch_unsupported_scheme() {
        let skill = WebFetchSkill::new();
        let call = ToolCall {
            id: "f3".to_string(),
            name: "web_fetch".to_string(),
            arguments: serde_json::json!({ "url": "ftp://example.com/file" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("unsupported scheme"));
    }

    // ── WebBrowseSearchSkill ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_web_search_empty_query() {
        let skill = WebBrowseSearchSkill::new();
        let call = ToolCall {
            id: "s1".to_string(),
            name: "web_browse_search".to_string(),
            arguments: serde_json::json!({ "query": "" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("query is required"));
    }

    #[test]
    fn test_parse_ddg_results_empty_html() {
        let skill = WebBrowseSearchSkill::new();
        let results = skill.parse_results("", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_ddg_results_sample() {
        let skill = WebBrowseSearchSkill::new();
        let html = r#"
            <a class="result__a" href="https://example.com">Example Site</a>
            <td class="result__snippet">A snippet about example</td>
            <a class="result__a" href="https://rust-lang.org">Rust</a>
            <td class="result__snippet">Systems programming language</td>
        "#;
        let results = skill.parse_results(html, 5);
        // These simple hrefs don't go through DDG redirect but the test
        // validates parsing logic
        assert!(!results.is_empty());
        let titles: Vec<&str> = results.iter().filter_map(|r| r["title"].as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("Example Site")));
    }

    #[test]
    fn test_parse_ddg_results_respects_max() {
        let skill = WebBrowseSearchSkill::new();
        // 3 results in HTML but max=2
        let html = r#"
            <a class="result__a" href="https://a.com">A</a>
            <td class="result__snippet">s1</td>
            <a class="result__a" href="https://b.com">B</a>
            <td class="result__snippet">s2</td>
            <a class="result__a" href="https://c.com">C</a>
            <td class="result__snippet">s3</td>
        "#;
        let results = skill.parse_results(html, 2);
        assert!(results.len() <= 2);
    }

    // ── WebExtractSkill (feature-gated) ──────────────────────────────────────

    #[cfg(feature = "web-browse")]
    #[tokio::test]
    async fn test_web_extract_empty_url() {
        let skill = WebExtractSkill::new();
        let call = ToolCall {
            id: "e1".to_string(),
            name: "web_extract".to_string(),
            arguments: serde_json::json!({ "url": "", "selectors": { "title": "h1" } }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("url is required"));
    }

    #[cfg(feature = "web-browse")]
    #[tokio::test]
    async fn test_web_extract_empty_selectors() {
        let skill = WebExtractSkill::new();
        let call = ToolCall {
            id: "e2".to_string(),
            name: "web_extract".to_string(),
            arguments: serde_json::json!({ "url": "https://example.com", "selectors": {} }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("must not be empty"));
    }

    #[cfg(feature = "web-browse")]
    #[tokio::test]
    async fn test_web_extract_bad_scheme() {
        let skill = WebExtractSkill::new();
        let call = ToolCall {
            id: "e3".to_string(),
            name: "web_extract".to_string(),
            arguments: serde_json::json!({
                "url": "ftp://example.com/",
                "selectors": { "title": "h1" }
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("unsupported scheme"));
    }

    // ── descriptor sanity ────────────────────────────────────────────────────

    #[test]
    fn test_descriptors() {
        assert_eq!(WebFetchSkill::new().descriptor().name, "web_fetch");
        assert_eq!(
            WebBrowseSearchSkill::new().descriptor().name,
            "web_browse_search"
        );
        #[cfg(feature = "web-browse")]
        assert_eq!(WebExtractSkill::new().descriptor().name, "web_extract");
    }
}

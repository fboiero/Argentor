use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::info;

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS_LIMIT: usize = 10;
const DUCKDUCKGO_HTML_URL: &str = "https://html.duckduckgo.com/html/";
const TAVILY_API_URL: &str = "https://api.tavily.com/search";
const BRAVE_API_URL: &str = "https://api.search.brave.com/res/v1/web/search";

/// Supported search providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchProvider {
    /// Free, no API key required (default). HTML scraping.
    DuckDuckGo,
    /// Requires `TAVILY_API_KEY`. High-quality AI-optimized results.
    Tavily,
    /// Requires `BRAVE_API_KEY`. Privacy-focused search.
    Brave,
    /// Self-hosted SearXNG instance. No API key needed.
    Searxng,
}

impl Default for SearchProvider {
    fn default() -> Self {
        Self::DuckDuckGo
    }
}

/// A single search result with title, URL, and snippet.
#[derive(Debug, serde::Serialize)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Parse DuckDuckGo HTML response to extract search results.
///
/// DuckDuckGo's HTML lite endpoint returns result blocks with:
/// - Links with class `result__a` containing the title and href
/// - Snippets with class `result__snippet`
fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Extract result blocks: each result has a link (result__a) and snippet (result__snippet)
    // Pattern: <a class="result__a" href="URL">TITLE</a>
    let link_re =
        match Regex::new(r#"(?is)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#) {
            Ok(r) => r,
            Err(_) => return results,
        };

    // Pattern: <a class="result__snippet"...>SNIPPET</a> or <td class="result__snippet">SNIPPET</td>
    let snippet_re = match Regex::new(r#"(?is)class="result__snippet"[^>]*>(.*?)</(?:a|td)>"#) {
        Ok(r) => r,
        Err(_) => return results,
    };

    let link_matches: Vec<_> = link_re.captures_iter(html).collect();
    let snippet_matches: Vec<_> = snippet_re.captures_iter(html).collect();

    for (i, link_cap) in link_matches.iter().enumerate() {
        if results.len() >= max_results {
            break;
        }

        let raw_url = link_cap.get(1).map_or("", |m| m.as_str()).to_string();
        let raw_title = link_cap.get(2).map_or("", |m| m.as_str());
        let title = strip_html(raw_title);

        // DuckDuckGo redirects through their own URL; extract the actual URL
        let url = extract_ddg_redirect_url(&raw_url);

        let snippet = if i < snippet_matches.len() {
            let raw_snippet = snippet_matches[i].get(1).map_or("", |m| m.as_str());
            strip_html(raw_snippet)
        } else {
            String::new()
        };

        if !url.is_empty() && !title.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }

    results
}

/// Parse Tavily API JSON response to extract search results.
///
/// Expected format: `{ "results": [{ "title": "...", "url": "...", "content": "..." }, ...] }`
fn parse_tavily_results(json: &str, max_results: usize) -> Vec<SearchResult> {
    let parsed: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let empty_arr = Vec::new();
    let results_arr = parsed["results"].as_array().unwrap_or(&empty_arr);

    results_arr
        .iter()
        .take(max_results)
        .filter_map(|item| {
            let title = item["title"].as_str()?.to_string();
            let url = item["url"].as_str()?.to_string();
            let snippet = item["content"].as_str().unwrap_or("").to_string();
            Some(SearchResult {
                title,
                url,
                snippet,
            })
        })
        .collect()
}

/// Parse Brave Search API JSON response to extract search results.
///
/// Expected format: `{ "web": { "results": [{ "title": "...", "url": "...", "description": "..." }, ...] } }`
fn parse_brave_results(json: &str, max_results: usize) -> Vec<SearchResult> {
    let parsed: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let empty_arr = Vec::new();
    let results_arr = parsed["web"]["results"].as_array().unwrap_or(&empty_arr);

    results_arr
        .iter()
        .take(max_results)
        .filter_map(|item| {
            let title = item["title"].as_str()?.to_string();
            let url = item["url"].as_str()?.to_string();
            let snippet = item["description"].as_str().unwrap_or("").to_string();
            Some(SearchResult {
                title,
                url,
                snippet,
            })
        })
        .collect()
}

/// Parse SearXNG JSON response to extract search results.
///
/// Expected format: `{ "results": [{ "title": "...", "url": "...", "content": "..." }, ...] }`
fn parse_searxng_results(json: &str, max_results: usize) -> Vec<SearchResult> {
    let parsed: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let empty_arr = Vec::new();
    let results_arr = parsed["results"].as_array().unwrap_or(&empty_arr);

    results_arr
        .iter()
        .take(max_results)
        .filter_map(|item| {
            let title = item["title"].as_str()?.to_string();
            let url = item["url"].as_str()?.to_string();
            let snippet = item["content"].as_str().unwrap_or("").to_string();
            Some(SearchResult {
                title,
                url,
                snippet,
            })
        })
        .collect()
}

/// Extract the actual destination URL from a DuckDuckGo redirect URL.
/// DuckDuckGo wraps results in `//duckduckgo.com/l/?uddg=ENCODED_URL&...`
fn extract_ddg_redirect_url(raw_url: &str) -> String {
    // Check if it's a DDG redirect
    if raw_url.contains("duckduckgo.com/l/") || raw_url.contains("uddg=") {
        // Extract the uddg parameter
        if let Some(start) = raw_url.find("uddg=") {
            let param_start = start + 5;
            let param_end = raw_url[param_start..]
                .find('&')
                .map_or(raw_url.len(), |pos| param_start + pos);
            let encoded = &raw_url[param_start..param_end];
            // URL-decode the parameter
            return url_decode(encoded);
        }
    }

    // If it starts with //, prepend https:
    if raw_url.starts_with("//") {
        return format!("https:{raw_url}");
    }

    raw_url.to_string()
}

/// Basic URL decoding for common percent-encoded characters.
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
        if bytes[i] == b'+' {
            result.push(' ');
        } else {
            result.push(bytes[i] as char);
        }
        i += 1;
    }

    result
}

/// Strip HTML tags and decode common entities.
fn strip_html(html: &str) -> String {
    let re = match Regex::new(r"<[^>]+>") {
        Ok(r) => r,
        Err(_) => return html.to_string(),
    };
    let text = re.replace_all(html, "");
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    // Collapse whitespace
    let ws_re = match Regex::new(r"\s+") {
        Ok(r) => r,
        Err(_) => return text.trim().to_string(),
    };
    ws_re.replace_all(&text, " ").trim().to_string()
}

/// URL-encode a query string for use in a URL parameter.
fn url_encode_query(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len() * 3);
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
}

/// Web search skill -- multi-provider web search.
///
/// Supports DuckDuckGo (free, default), Tavily, Brave, and SearXNG.
/// Inspired by Vercel Tavily/Exa tools, LangChain DuckDuckGoSearchRun,
/// and CrewAI SerperDevTool.
///
/// Supported operations:
/// - `search` -- search the web, return results with titles, URLs, and snippets
/// - `search_news` -- search news (DuckDuckGo appends "news"; Tavily uses `topic: "news"`)
/// - `lucky` -- return the first result URL ("I'm feeling lucky")
pub struct WebSearchSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
    /// Active search provider.
    provider: SearchProvider,
    /// API key for providers that require one (Tavily, Brave).
    api_key: Option<String>,
    /// Base URL for self-hosted SearXNG instance.
    searxng_base_url: Option<String>,
}

impl WebSearchSkill {
    /// Create a new `WebSearchSkill` using DuckDuckGo (default, no API key).
    pub fn new() -> Self {
        Self::build(SearchProvider::DuckDuckGo, None, None)
    }

    /// Create a new `WebSearchSkill` with a specific provider and optional API key.
    pub fn with_provider(provider: SearchProvider, api_key: Option<String>) -> Self {
        Self::build(provider, api_key, None)
    }

    /// Shortcut: create a Tavily-backed search skill.
    pub fn tavily(api_key: String) -> Self {
        Self::build(SearchProvider::Tavily, Some(api_key), None)
    }

    /// Shortcut: create a Brave-backed search skill.
    pub fn brave(api_key: String) -> Self {
        Self::build(SearchProvider::Brave, Some(api_key), None)
    }

    /// Shortcut: create a SearXNG-backed search skill pointing at `base_url`.
    pub fn searxng(base_url: String) -> Self {
        Self::build(SearchProvider::Searxng, None, Some(base_url))
    }

    /// Internal builder shared by all constructors.
    fn build(
        provider: SearchProvider,
        api_key: Option<String>,
        searxng_base_url: Option<String>,
    ) -> Self {
        #[allow(clippy::expect_used)]
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("Mozilla/5.0 (compatible; Argentor/0.1; AI Agent)")
            .build()
            .expect("Failed to create HTTP client -- TLS backend unavailable");

        let provider_name = match &provider {
            SearchProvider::DuckDuckGo => "DuckDuckGo",
            SearchProvider::Tavily => "Tavily",
            SearchProvider::Brave => "Brave",
            SearchProvider::Searxng => "SearXNG",
        };

        Self {
            descriptor: SkillDescriptor {
                name: "web_search".to_string(),
                description: format!(
                    "Search the web using {provider_name}. Returns titles, URLs, and snippets."
                ),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["search", "search_news", "lucky"],
                            "description": "The operation to perform"
                        },
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 5, max: 10)"
                        },
                        "provider": {
                            "type": "string",
                            "enum": ["duckduckgo", "tavily", "brave", "searxng"],
                            "description": "Override the search provider at runtime (optional)"
                        }
                    },
                    "required": ["operation", "query"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![],
                }],
                requires_approval: false,
            },
            client,
            provider,
            api_key,
            searxng_base_url,
        }
    }

    /// Resolve the effective provider for a call (runtime override or default).
    fn resolve_provider(&self, call: &ToolCall) -> SearchProvider {
        if let Some(p) = call.arguments.get("provider").and_then(|v| v.as_str()) {
            match p {
                "duckduckgo" => SearchProvider::DuckDuckGo,
                "tavily" => SearchProvider::Tavily,
                "brave" => SearchProvider::Brave,
                "searxng" => SearchProvider::Searxng,
                _ => self.provider.clone(),
            }
        } else {
            self.provider.clone()
        }
    }

    /// Execute a search query against DuckDuckGo HTML and return parsed results.
    async fn do_search_ddg(
        &self,
        query: &str,
        max_results: usize,
        call_id: &str,
    ) -> Result<Vec<SearchResult>, ToolResult> {
        let encoded_query = url_encode_query(query);
        let url = format!("{DUCKDUCKGO_HTML_URL}?q={encoded_query}");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolResult::error(call_id, format!("Search request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ToolResult::error(
                call_id,
                format!("DuckDuckGo returned HTTP {status}"),
            ));
        }

        let body = response.text().await.map_err(|e| {
            ToolResult::error(call_id, format!("Failed to read search response: {e}"))
        })?;

        let results = parse_ddg_results(&body, max_results);

        if results.is_empty() {
            return Err(ToolResult::error(
                call_id,
                format!("No results found for '{query}'"),
            ));
        }

        Ok(results)
    }

    /// Execute a search query against the Tavily API.
    async fn do_search_tavily(
        &self,
        query: &str,
        max_results: usize,
        call_id: &str,
        topic: Option<&str>,
    ) -> Result<Vec<SearchResult>, ToolResult> {
        let api_key = self.api_key.as_deref().ok_or_else(|| {
            ToolResult::error(call_id, "Tavily requires an API key (TAVILY_API_KEY)")
        })?;

        let mut body = serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
            "search_depth": "basic"
        });

        if let Some(t) = topic {
            body["topic"] = serde_json::json!(t);
        }

        let response = self
            .client
            .post(TAVILY_API_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolResult::error(call_id, format!("Tavily request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ToolResult::error(
                call_id,
                format!("Tavily returned HTTP {status}"),
            ));
        }

        let text = response.text().await.map_err(|e| {
            ToolResult::error(call_id, format!("Failed to read Tavily response: {e}"))
        })?;

        let results = parse_tavily_results(&text, max_results);

        if results.is_empty() {
            return Err(ToolResult::error(
                call_id,
                format!("No results found for '{query}'"),
            ));
        }

        Ok(results)
    }

    /// Execute a search query against the Brave Search API.
    async fn do_search_brave(
        &self,
        query: &str,
        max_results: usize,
        call_id: &str,
    ) -> Result<Vec<SearchResult>, ToolResult> {
        let api_key = self.api_key.as_deref().ok_or_else(|| {
            ToolResult::error(call_id, "Brave requires an API key (BRAVE_API_KEY)")
        })?;

        let encoded_query = url_encode_query(query);
        let url = format!("{BRAVE_API_URL}?q={encoded_query}&count={max_results}");

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ToolResult::error(call_id, format!("Brave request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ToolResult::error(
                call_id,
                format!("Brave returned HTTP {status}"),
            ));
        }

        let text = response.text().await.map_err(|e| {
            ToolResult::error(call_id, format!("Failed to read Brave response: {e}"))
        })?;

        let results = parse_brave_results(&text, max_results);

        if results.is_empty() {
            return Err(ToolResult::error(
                call_id,
                format!("No results found for '{query}'"),
            ));
        }

        Ok(results)
    }

    /// Execute a search query against a self-hosted SearXNG instance.
    async fn do_search_searxng(
        &self,
        query: &str,
        max_results: usize,
        call_id: &str,
    ) -> Result<Vec<SearchResult>, ToolResult> {
        let base_url = self.searxng_base_url.as_deref().ok_or_else(|| {
            ToolResult::error(
                call_id,
                "SearXNG requires a base_url (use WebSearchSkill::searxng(base_url))",
            )
        })?;

        let encoded_query = url_encode_query(query);
        let url = format!("{base_url}/search?q={encoded_query}&format=json&categories=general");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolResult::error(call_id, format!("SearXNG request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ToolResult::error(
                call_id,
                format!("SearXNG returned HTTP {status}"),
            ));
        }

        let text = response.text().await.map_err(|e| {
            ToolResult::error(call_id, format!("Failed to read SearXNG response: {e}"))
        })?;

        let results = parse_searxng_results(&text, max_results);

        if results.is_empty() {
            return Err(ToolResult::error(
                call_id,
                format!("No results found for '{query}'"),
            ));
        }

        Ok(results)
    }

    /// Dispatch a search to the appropriate provider.
    async fn do_search(
        &self,
        query: &str,
        max_results: usize,
        call_id: &str,
        provider: &SearchProvider,
        topic: Option<&str>,
    ) -> Result<Vec<SearchResult>, ToolResult> {
        match provider {
            SearchProvider::DuckDuckGo => self.do_search_ddg(query, max_results, call_id).await,
            SearchProvider::Tavily => {
                self.do_search_tavily(query, max_results, call_id, topic)
                    .await
            }
            SearchProvider::Brave => self.do_search_brave(query, max_results, call_id).await,
            SearchProvider::Searxng => self.do_search_searxng(query, max_results, call_id).await,
        }
    }
}

impl Default for WebSearchSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for WebSearchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let query = call.arguments["query"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if query.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "The 'query' parameter is required",
            ));
        }

        let max_results = call.arguments["max_results"]
            .as_u64()
            .map(|v| (v as usize).min(MAX_RESULTS_LIMIT))
            .unwrap_or(DEFAULT_MAX_RESULTS);

        let provider = self.resolve_provider(&call);

        info!(operation = %operation, query = %query, provider = ?provider, "WebSearch execute");

        match operation.as_str() {
            "search" => {
                let results = match self
                    .do_search(&query, max_results, &call.id, &provider, None)
                    .await
                {
                    Ok(r) => r,
                    Err(err_result) => return Ok(err_result),
                };

                let result = serde_json::json!({
                    "query": query,
                    "results": results,
                    "count": results.len(),
                    "provider": format!("{provider:?}"),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "search_news" => {
                // Tavily supports a native `topic: "news"` parameter;
                // other providers append "news" to the query string.
                let (effective_query, topic) = if provider == SearchProvider::Tavily {
                    (query.clone(), Some("news"))
                } else {
                    (format!("{query} news"), None)
                };

                let results = match self
                    .do_search(&effective_query, max_results, &call.id, &provider, topic)
                    .await
                {
                    Ok(r) => r,
                    Err(err_result) => return Ok(err_result),
                };

                let result = serde_json::json!({
                    "query": query,
                    "results": results,
                    "count": results.len(),
                    "provider": format!("{provider:?}"),
                });

                Ok(ToolResult::success(&call.id, result.to_string()))
            }

            "lucky" => {
                let results = match self.do_search(&query, 1, &call.id, &provider, None).await {
                    Ok(r) => r,
                    Err(err_result) => return Ok(err_result),
                };

                if let Some(first) = results.first() {
                    let result = serde_json::json!({
                        "query": query,
                        "url": first.url,
                        "title": first.title,
                        "snippet": first.snippet,
                    });
                    Ok(ToolResult::success(&call.id, result.to_string()))
                } else {
                    Ok(ToolResult::error(
                        &call.id,
                        format!("No results found for '{query}'"),
                    ))
                }
            }

            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation '{operation}'. Valid: search, search_news, lucky"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Mock DuckDuckGo HTML response for testing.
    const MOCK_DDG_HTML: &str = r#"
<html>
<body>
<div id="links">
    <div class="result results_links results_links_deep web-result">
        <div class="links_main links_deep result__body">
            <h2 class="result__title">
                <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">Rust Programming Language</a>
            </h2>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F">A language empowering everyone to build reliable and efficient software.</a>
        </div>
    </div>
    <div class="result results_links results_links_deep web-result">
        <div class="links_main links_deep result__body">
            <h2 class="result__title">
                <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F&amp;rut=def">The Rust Book</a>
            </h2>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F">The official Rust programming language book for learning Rust.</a>
        </div>
    </div>
    <div class="result results_links results_links_deep web-result">
        <div class="links_main links_deep result__body">
            <h2 class="result__title">
                <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fcrates.io%2F&amp;rut=ghi">crates.io: Rust Package Registry</a>
            </h2>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fcrates.io%2F">The Rust community's crate registry for sharing and discovering Rust libraries.</a>
        </div>
    </div>
</div>
</body>
</html>"#;

    /// Mock DDG HTML with direct URLs (no redirect wrapper).
    const MOCK_DDG_DIRECT: &str = r#"
<html>
<body>
<div class="result">
    <h2><a class="result__a" href="https://example.com/page1">Direct Result One</a></h2>
    <a class="result__snippet" href="https://example.com/page1">Snippet for result one.</a>
</div>
<div class="result">
    <h2><a class="result__a" href="https://example.com/page2">Direct Result Two</a></h2>
    <a class="result__snippet" href="https://example.com/page2">Snippet for result two.</a>
</div>
</body>
</html>"#;

    /// Mock Tavily API response.
    const MOCK_TAVILY_JSON: &str = r#"{
        "query": "rust programming",
        "results": [
            {
                "title": "Rust Programming Language",
                "url": "https://www.rust-lang.org/",
                "content": "A language empowering everyone to build reliable and efficient software.",
                "score": 0.98
            },
            {
                "title": "The Rust Book",
                "url": "https://doc.rust-lang.org/book/",
                "content": "The official Rust programming language book.",
                "score": 0.95
            },
            {
                "title": "crates.io",
                "url": "https://crates.io/",
                "content": "The Rust community crate registry.",
                "score": 0.90
            }
        ]
    }"#;

    /// Mock Brave Search API response.
    const MOCK_BRAVE_JSON: &str = r#"{
        "query": {
            "original": "rust programming"
        },
        "web": {
            "results": [
                {
                    "title": "Rust Programming Language",
                    "url": "https://www.rust-lang.org/",
                    "description": "A systems programming language focused on safety and performance.",
                    "age": "2024-01-01"
                },
                {
                    "title": "Learn Rust",
                    "url": "https://www.rust-lang.org/learn",
                    "description": "Get started with Rust. Official learning resources.",
                    "age": "2024-02-15"
                },
                {
                    "title": "Rust By Example",
                    "url": "https://doc.rust-lang.org/rust-by-example/",
                    "description": "Learn Rust with examples.",
                    "age": "2024-03-01"
                }
            ]
        }
    }"#;

    /// Mock SearXNG JSON response.
    const MOCK_SEARXNG_JSON: &str = r#"{
        "query": "rust programming",
        "number_of_results": 3,
        "results": [
            {
                "title": "Rust Programming Language",
                "url": "https://www.rust-lang.org/",
                "content": "Rust is a systems programming language.",
                "engine": "google"
            },
            {
                "title": "Rust Documentation",
                "url": "https://doc.rust-lang.org/",
                "content": "Official Rust documentation and guides.",
                "engine": "duckduckgo"
            },
            {
                "title": "Awesome Rust",
                "url": "https://github.com/rust-unofficial/awesome-rust",
                "content": "A curated list of Rust libraries and tools.",
                "engine": "bing"
            }
        ]
    }"#;

    #[test]
    fn test_descriptor() {
        let skill = WebSearchSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "web_search");
        assert!(!desc.required_capabilities.is_empty());
    }

    #[test]
    fn test_parse_ddg_results() {
        let results = parse_ddg_results(MOCK_DDG_HTML, 10);
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("reliable and efficient"));

        assert_eq!(results[1].title, "The Rust Book");
        assert_eq!(results[1].url, "https://doc.rust-lang.org/book/");

        assert_eq!(results[2].title, "crates.io: Rust Package Registry");
        assert_eq!(results[2].url, "https://crates.io/");
    }

    #[test]
    fn test_parse_ddg_results_with_limit() {
        let results = parse_ddg_results(MOCK_DDG_HTML, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_parse_ddg_direct_urls() {
        let results = parse_ddg_results(MOCK_DDG_DIRECT, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/page1");
        assert_eq!(results[0].title, "Direct Result One");
        assert_eq!(results[1].url, "https://example.com/page2");
    }

    #[test]
    fn test_parse_ddg_empty_html() {
        let results = parse_ddg_results("<html><body>No results</body></html>", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_extract_ddg_redirect_url() {
        let raw = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.example.com%2Fpath&rut=abc";
        let url = extract_ddg_redirect_url(raw);
        assert_eq!(url, "https://www.example.com/path");
    }

    #[test]
    fn test_extract_ddg_redirect_url_direct() {
        let raw = "https://www.example.com/direct";
        let url = extract_ddg_redirect_url(raw);
        assert_eq!(url, "https://www.example.com/direct");
    }

    #[test]
    fn test_extract_ddg_redirect_url_protocol_relative() {
        let raw = "//www.example.com/path";
        let url = extract_ddg_redirect_url(raw);
        assert_eq!(url, "https://www.example.com/path");
    }

    #[test]
    fn test_url_decode() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(
            url_decode("https%3A%2F%2Fexample.com"),
            "https://example.com"
        );
        assert_eq!(url_decode("hello+world"), "hello world");
        assert_eq!(url_decode("plain"), "plain");
    }

    #[test]
    fn test_url_encode_query() {
        assert_eq!(url_encode_query("hello world"), "hello+world");
        assert_eq!(url_encode_query("rust programming"), "rust+programming");
        assert_eq!(url_encode_query("test"), "test");
        assert_eq!(url_encode_query("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn test_strip_html() {
        assert_eq!(
            strip_html("<b>Hello</b> &amp; <i>World</i>"),
            "Hello & World"
        );
        assert_eq!(strip_html("plain text"), "plain text");
        assert_eq!(
            strip_html("<a href='x'>Link &lt;here&gt;</a>"),
            "Link <here>"
        );
    }

    #[tokio::test]
    async fn test_missing_query() {
        let skill = WebSearchSkill::new();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "web_search".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "query": ""
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = WebSearchSkill::new();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "web_search".to_string(),
            arguments: serde_json::json!({
                "operation": "invalid",
                "query": "test"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_snippet_extraction() {
        let results = parse_ddg_results(MOCK_DDG_HTML, 10);
        assert!(results[0]
            .snippet
            .contains("reliable and efficient software"));
        assert!(results[1]
            .snippet
            .contains("official Rust programming language book"));
        assert!(results[2].snippet.contains("Rust community"));
    }

    #[test]
    fn test_max_results_clamping() {
        // Even if we parse more, the function respects the limit
        let results = parse_ddg_results(MOCK_DDG_HTML, 1);
        assert_eq!(results.len(), 1);
    }

    // --- New tests for multi-provider support ---

    #[test]
    fn test_default_provider_is_duckduckgo() {
        let skill = WebSearchSkill::new();
        assert_eq!(skill.provider, SearchProvider::DuckDuckGo);
        assert!(skill.api_key.is_none());
        assert!(skill.searxng_base_url.is_none());
    }

    #[test]
    fn test_default_trait_is_duckduckgo() {
        let skill = WebSearchSkill::default();
        assert_eq!(skill.provider, SearchProvider::DuckDuckGo);
    }

    #[test]
    fn test_tavily_constructor() {
        let skill = WebSearchSkill::tavily("tvly-test-key-123".to_string());
        assert_eq!(skill.provider, SearchProvider::Tavily);
        assert_eq!(skill.api_key.as_deref(), Some("tvly-test-key-123"));
        assert!(skill.searxng_base_url.is_none());
        assert_eq!(skill.descriptor().name, "web_search");
        assert!(skill.descriptor().description.contains("Tavily"));
    }

    #[test]
    fn test_brave_constructor() {
        let skill = WebSearchSkill::brave("BSA-test-key-456".to_string());
        assert_eq!(skill.provider, SearchProvider::Brave);
        assert_eq!(skill.api_key.as_deref(), Some("BSA-test-key-456"));
        assert!(skill.searxng_base_url.is_none());
        assert_eq!(skill.descriptor().name, "web_search");
        assert!(skill.descriptor().description.contains("Brave"));
    }

    #[test]
    fn test_searxng_constructor() {
        let skill = WebSearchSkill::searxng("http://localhost:8888".to_string());
        assert_eq!(skill.provider, SearchProvider::Searxng);
        assert!(skill.api_key.is_none());
        assert_eq!(
            skill.searxng_base_url.as_deref(),
            Some("http://localhost:8888")
        );
        assert_eq!(skill.descriptor().name, "web_search");
        assert!(skill.descriptor().description.contains("SearXNG"));
    }

    #[test]
    fn test_with_provider_constructor() {
        let skill =
            WebSearchSkill::with_provider(SearchProvider::Tavily, Some("my-key".to_string()));
        assert_eq!(skill.provider, SearchProvider::Tavily);
        assert_eq!(skill.api_key.as_deref(), Some("my-key"));
    }

    #[test]
    fn test_parse_tavily_results_basic() {
        let results = parse_tavily_results(MOCK_TAVILY_JSON, 10);
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("reliable and efficient"));

        assert_eq!(results[1].title, "The Rust Book");
        assert_eq!(results[1].url, "https://doc.rust-lang.org/book/");

        assert_eq!(results[2].title, "crates.io");
        assert_eq!(results[2].url, "https://crates.io/");
    }

    #[test]
    fn test_parse_tavily_results_with_limit() {
        let results = parse_tavily_results(MOCK_TAVILY_JSON, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_parse_tavily_results_empty() {
        let results = parse_tavily_results(r#"{"results": []}"#, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_tavily_results_invalid_json() {
        let results = parse_tavily_results("not json at all", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_brave_results_basic() {
        let results = parse_brave_results(MOCK_BRAVE_JSON, 10);
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("safety and performance"));

        assert_eq!(results[1].title, "Learn Rust");
        assert_eq!(results[1].url, "https://www.rust-lang.org/learn");

        assert_eq!(results[2].title, "Rust By Example");
        assert_eq!(results[2].url, "https://doc.rust-lang.org/rust-by-example/");
    }

    #[test]
    fn test_parse_brave_results_with_limit() {
        let results = parse_brave_results(MOCK_BRAVE_JSON, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Programming Language");
    }

    #[test]
    fn test_parse_brave_results_empty() {
        let results = parse_brave_results(r#"{"web": {"results": []}}"#, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_brave_results_invalid_json() {
        let results = parse_brave_results("{broken", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_searxng_results_basic() {
        let results = parse_searxng_results(MOCK_SEARXNG_JSON, 10);
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("systems programming"));

        assert_eq!(results[1].title, "Rust Documentation");
        assert_eq!(results[1].url, "https://doc.rust-lang.org/");

        assert_eq!(results[2].title, "Awesome Rust");
        assert_eq!(
            results[2].url,
            "https://github.com/rust-unofficial/awesome-rust"
        );
    }

    #[test]
    fn test_parse_searxng_results_with_limit() {
        let results = parse_searxng_results(MOCK_SEARXNG_JSON, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_parse_searxng_results_empty() {
        let results = parse_searxng_results(r#"{"results": []}"#, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_searxng_results_invalid_json() {
        let results = parse_searxng_results("<<<not json>>>", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_runtime_provider_override() {
        let skill = WebSearchSkill::new(); // DuckDuckGo default
        assert_eq!(skill.provider, SearchProvider::DuckDuckGo);

        // Simulate a call with provider override
        let call = ToolCall {
            id: "t-override".to_string(),
            name: "web_search".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "query": "rust",
                "provider": "tavily"
            }),
        };
        let resolved = skill.resolve_provider(&call);
        assert_eq!(resolved, SearchProvider::Tavily);

        // No override
        let call_no_override = ToolCall {
            id: "t-no-override".to_string(),
            name: "web_search".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "query": "rust"
            }),
        };
        let resolved = skill.resolve_provider(&call_no_override);
        assert_eq!(resolved, SearchProvider::DuckDuckGo);

        // Invalid provider falls back to default
        let call_invalid = ToolCall {
            id: "t-invalid".to_string(),
            name: "web_search".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "query": "rust",
                "provider": "google"
            }),
        };
        let resolved = skill.resolve_provider(&call_invalid);
        assert_eq!(resolved, SearchProvider::DuckDuckGo);
    }

    #[test]
    fn test_runtime_provider_override_all_variants() {
        let skill = WebSearchSkill::new();

        for (input, expected) in [
            ("duckduckgo", SearchProvider::DuckDuckGo),
            ("tavily", SearchProvider::Tavily),
            ("brave", SearchProvider::Brave),
            ("searxng", SearchProvider::Searxng),
        ] {
            let call = ToolCall {
                id: "t".to_string(),
                name: "web_search".to_string(),
                arguments: serde_json::json!({
                    "operation": "search",
                    "query": "test",
                    "provider": input
                }),
            };
            assert_eq!(skill.resolve_provider(&call), expected);
        }
    }

    #[test]
    fn test_search_provider_serde_roundtrip() {
        let providers = vec![
            SearchProvider::DuckDuckGo,
            SearchProvider::Tavily,
            SearchProvider::Brave,
            SearchProvider::Searxng,
        ];
        for p in providers {
            let json = serde_json::to_string(&p).unwrap();
            let deserialized: SearchProvider = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, p);
        }
    }

    #[test]
    fn test_parameters_schema_includes_provider() {
        let skill = WebSearchSkill::new();
        let schema = &skill.descriptor().parameters_schema;
        let provider_prop = &schema["properties"]["provider"];
        assert_eq!(provider_prop["type"], "string");
        let enum_values = provider_prop["enum"].as_array().unwrap();
        assert!(enum_values.contains(&serde_json::json!("duckduckgo")));
        assert!(enum_values.contains(&serde_json::json!("tavily")));
        assert!(enum_values.contains(&serde_json::json!("brave")));
        assert!(enum_values.contains(&serde_json::json!("searxng")));
    }

    #[test]
    fn test_parse_tavily_missing_content_field() {
        // content is optional; results should still parse with empty snippet
        let json = r#"{
            "results": [
                {"title": "No Content", "url": "https://example.com/"}
            ]
        }"#;
        let results = parse_tavily_results(json, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "No Content");
        assert_eq!(results[0].snippet, "");
    }

    #[test]
    fn test_parse_brave_missing_description_field() {
        let json = r#"{
            "web": {
                "results": [
                    {"title": "No Desc", "url": "https://example.com/"}
                ]
            }
        }"#;
        let results = parse_brave_results(json, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "No Desc");
        assert_eq!(results[0].snippet, "");
    }

    #[test]
    fn test_parse_searxng_missing_content_field() {
        let json = r#"{
            "results": [
                {"title": "Minimal", "url": "https://example.com/"}
            ]
        }"#;
        let results = parse_searxng_results(json, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Minimal");
        assert_eq!(results[0].snippet, "");
    }
}

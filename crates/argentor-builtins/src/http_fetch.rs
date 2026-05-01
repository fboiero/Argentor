use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tracing::{info, warn};

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024; // 5MB

/// Known cloud metadata hostnames that must always be blocked regardless of
/// what IP they resolve to. Checked via suffix matching so subdomains are
/// covered (e.g. `foo.metadata.google.internal`).
const BLOCKED_HOSTNAMES: &[&str] = &[
    "metadata.google.internal",
    "metadata.aws.internal",
    "metadata.goog",
];

// ---------------------------------------------------------------------------
// IP-range helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `ip` belongs to a private, reserved, loopback,
/// link-local, or otherwise non-globally-routable address range.
///
/// Covers:
///   IPv4 - 10/8, 172.16/12, 192.168/16, 127/8, 169.254/16, 0.0.0.0,
///           255.255.255.255, 100.64/10 (CGNAT), 192.0.0/24, 192.0.2/24,
///           198.51.100/24, 203.0.113/24, 198.18/15, 240/4 (reserved)
///   IPv6 - ::1, ::, fe80::/10, fc00::/7 (ULA)
///   Cloud metadata - 169.254.169.254
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();

    // Unspecified
    if ip.is_unspecified() {
        return true;
    }
    // Loopback 127.0.0.0/8
    if ip.is_loopback() {
        return true;
    }
    // Private 10.0.0.0/8
    if octets[0] == 10 {
        return true;
    }
    // Private 172.16.0.0/12
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }
    // Private 192.168.0.0/16
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    // Link-local 169.254.0.0/16  (includes cloud metadata 169.254.169.254)
    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }
    // Broadcast
    if ip == Ipv4Addr::BROADCAST {
        return true;
    }
    // CGNAT / Shared address space 100.64.0.0/10
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }
    // IETF Protocol Assignments 192.0.0.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return true;
    }
    // Documentation 192.0.2.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // Documentation 198.51.100.0/24
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // Documentation 203.0.113.0/24
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }
    // Benchmarking 198.18.0.0/15
    if octets[0] == 198 && (18..=19).contains(&octets[1]) {
        return true;
    }
    // Reserved / Future use 240.0.0.0/4
    if octets[0] >= 240 {
        return true;
    }

    false
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    // Unspecified ::
    if ip.is_unspecified() {
        return true;
    }
    // Loopback ::1
    if ip.is_loopback() {
        return true;
    }
    let segments = ip.segments();
    // Link-local fe80::/10
    if segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }
    // Unique Local Address fc00::/7
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }
    // IPv4-mapped ::ffff:0:0/96 — check the embedded IPv4
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_ipv4(v4);
    }

    false
}

// ---------------------------------------------------------------------------
// Hostname helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the hostname itself (before DNS resolution) should be
/// blocked. This catches well-known cloud metadata names and `localhost`.
fn is_blocked_hostname(host: &str) -> bool {
    let lower = host.to_lowercase();

    if lower == "localhost" {
        return true;
    }

    BLOCKED_HOSTNAMES
        .iter()
        .any(|blocked| lower == *blocked || lower.ends_with(&format!(".{blocked}")))
}

/// Resolve `host:port` via DNS and return all resolved IP addresses.
/// Returns an error string if resolution fails.
async fn resolve_host(host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    let addr = format!("{host}:{port}");
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&addr)
        .await
        .map_err(|e| format!("DNS resolution failed for '{host}': {e}"))?
        .collect();

    if addrs.is_empty() {
        return Err(format!("DNS resolution returned no addresses for '{host}'"));
    }

    Ok(addrs.into_iter().map(|sa| sa.ip()).collect())
}

/// Validate that none of the resolved IPs are private/reserved. Returns an
/// error message if any IP is disallowed.
fn check_resolved_ips(host: &str, ips: &[IpAddr]) -> Result<(), String> {
    for ip in ips {
        if is_private_ip(*ip) {
            return Err(format!(
                "Access denied: '{host}' resolves to private/internal address {ip}"
            ));
        }
    }
    Ok(())
}

/// Full SSRF validation for a URL: hostname blocklist + DNS resolution + IP
/// range check. Returns an error message when the URL should be blocked.
async fn validate_url_ssrf(parsed_url: &reqwest::Url) -> Result<(), String> {
    let host = parsed_url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // 1. Static hostname blocklist (catches metadata services, localhost)
    if is_blocked_hostname(host) {
        return Err(format!("Access denied: '{host}' is a blocked hostname"));
    }

    // 2. If the host is already an IP literal, parse and check directly
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(format!(
                "Access denied: '{host}' is a private/internal address"
            ));
        }
        return Ok(());
    }

    // Also handle bracketed IPv6 in the host string (e.g. "[::1]")
    let trimmed = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(format!(
                "Access denied: '{host}' is a private/internal address"
            ));
        }
        return Ok(());
    }

    // 3. DNS resolution — resolve before connecting
    let port = parsed_url.port_or_known_default().unwrap_or(80);
    let ips = resolve_host(host, port).await?;
    check_resolved_ips(host, &ips)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// HttpFetchSkill
// ---------------------------------------------------------------------------

/// HTTP fetch skill. Makes GET/POST requests to allowed hosts with
/// production-grade SSRF protection:
///
/// - DNS resolution **before** the request, with IP-range validation
/// - Proper CIDR-based private/reserved range detection (no string-prefix hacks)
/// - Custom redirect policy that re-validates every redirect target
/// - Hostname blocklist for cloud metadata endpoints
pub struct HttpFetchSkill {
    descriptor: SkillDescriptor,
    client: reqwest::Client,
}

impl HttpFetchSkill {
    /// Create a new HTTP fetch skill with a secure default client.
    pub fn new() -> Self {
        // Build a custom redirect policy that validates each hop.
        let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
            // Cap total redirects at 10
            let redirect_count = attempt.previous().len();
            if redirect_count >= 10 {
                return attempt.error(format!("too many redirects ({redirect_count})"));
            }

            let url = attempt.url().clone();

            // Validate scheme
            match url.scheme() {
                "http" | "https" => {}
                scheme => {
                    return attempt.error(format!("redirect to unsupported scheme '{scheme}'"));
                }
            }

            if let Some(host) = url.host_str() {
                // Block known-bad hostnames
                if is_blocked_hostname(host) {
                    return attempt.error(format!("redirect to blocked hostname '{host}'"));
                }

                // If the redirect target is an IP literal, check it
                if let Ok(ip) = host.parse::<IpAddr>() {
                    if is_private_ip(ip) {
                        return attempt.error(format!("redirect to private IP {ip}"));
                    }
                }

                // For hostname redirects, also try trimmed brackets (IPv6)
                let trimmed = host.trim_start_matches('[').trim_end_matches(']');
                if let Ok(ip) = trimmed.parse::<IpAddr>() {
                    if is_private_ip(ip) {
                        return attempt.error(format!("redirect to private IP {ip}"));
                    }
                }
            }

            // NOTE: We cannot do async DNS resolution inside the synchronous
            // redirect policy callback. The IP-literal and hostname checks
            // above cover the most common redirect-based SSRF vectors. For
            // hostname redirects that resolve to private IPs, the
            // `reqwest::Client` `resolve` / connect-level socket checks
            // would be needed (or a proxy layer). The pre-request DNS
            // validation already catches the initial target.
            attempt.follow()
        });

        // reqwest::Client::builder().build() only fails if TLS backend
        // initialization fails, which indicates a fundamentally broken
        // environment. We allow expect here because there is no meaningful
        // recovery path at construction time.
        #[allow(clippy::expect_used)]
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(redirect_policy)
            .build()
            .expect("Failed to create HTTP client -- TLS backend unavailable");

        Self {
            descriptor: SkillDescriptor {
                name: "http_fetch".to_string(),
                description: "Fetch content from a URL via HTTP GET or POST.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch"
                        },
                        "method": {
                            "type": "string",
                            "enum": ["GET", "POST"],
                            "description": "HTTP method (default: GET)"
                        },
                        "headers": {
                            "type": "object",
                            "description": "Optional HTTP headers as key-value pairs"
                        },
                        "body": {
                            "type": "string",
                            "description": "Optional request body (for POST)"
                        }
                    },
                    "required": ["url"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![], // Configured at runtime
                }],
                requires_approval: false,
            },
            client,
        }
    }
}

impl Default for HttpFetchSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for HttpFetchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let url_str = call.arguments["url"].as_str().unwrap_or_default();

        if url_str.is_empty() {
            return Ok(()); // Empty URL will be caught in execute()
        }

        let parsed_url = match reqwest::Url::parse(url_str) {
            Ok(u) => u,
            Err(_) => return Ok(()), // Invalid URL will be caught in execute()
        };

        if let Some(host) = parsed_url.host_str() {
            if !permissions.check_network(host) {
                return Err(ArgentorError::Security(format!(
                    "network access not permitted for host '{host}'"
                )));
            }
        }

        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let url = call.arguments["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if url.is_empty() {
            return Ok(ToolResult::error(&call.id, "Empty URL"));
        }

        // Parse the URL
        let parsed_url = match reqwest::Url::parse(&url) {
            Ok(u) => u,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Invalid URL '{url}': {e}"),
                ));
            }
        };

        // Only allow http/https
        match parsed_url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Unsupported scheme '{scheme}'. Only http/https allowed."),
                ));
            }
        }

        // SSRF validation: hostname blocklist + DNS resolution + IP range check
        if let Err(msg) = validate_url_ssrf(&parsed_url).await {
            warn!(url = %url, reason = %msg, "SSRF protection blocked request");
            return Ok(ToolResult::error(&call.id, msg));
        }

        let method = call.arguments["method"]
            .as_str()
            .unwrap_or("GET")
            .to_uppercase();

        info!(url = %url, method = %method, "HTTP fetch");

        let mut request = match method.as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            _ => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Unsupported method '{method}'. Use GET or POST."),
                ));
            }
        };

        // Add custom headers
        if let Some(headers) = call.arguments["headers"].as_object() {
            for (key, value) in headers {
                if let Some(v) = value.as_str() {
                    request = request.header(key.as_str(), v);
                }
            }
        }

        // Add body for POST
        if method == "POST" {
            if let Some(body) = call.arguments["body"].as_str() {
                request = request.body(body.to_string());
            }
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("HTTP request failed: {e}"),
                ));
            }
        };

        let status = response.status().as_u16();
        let headers: serde_json::Map<String, serde_json::Value> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|val| (k.to_string(), serde_json::Value::String(val.to_string())))
            })
            .collect();

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body_bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Failed to read response body: {e}"),
                ));
            }
        };

        if body_bytes.len() > MAX_RESPONSE_SIZE {
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "Response too large: {} bytes (max: {} bytes)",
                    body_bytes.len(),
                    MAX_RESPONSE_SIZE
                ),
            ));
        }

        let body = String::from_utf8_lossy(&body_bytes);

        let result = serde_json::json!({
            "status": status,
            "headers": headers,
            "content_type": content_type,
            "body": body,
            "size": body_bytes.len(),
        });

        if (200..400).contains(&status) {
            Ok(ToolResult::success(&call.id, result.to_string()))
        } else {
            Ok(ToolResult::error(&call.id, result.to_string()))
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

    // -- is_private_ip unit tests -------------------------------------------

    #[test]
    fn test_is_private_ip_comprehensive() {
        // IPv4 loopback
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 255, 255, 255))));

        // IPv4 private 10/8
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));

        // IPv4 private 172.16/12
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        // 172.15 and 172.32 are NOT private
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 15, 0, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));

        // IPv4 private 192.168/16
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 255, 255))));

        // IPv4 link-local 169.254/16
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));

        // IPv4 unspecified
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));

        // IPv4 broadcast
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::BROADCAST)));

        // IPv4 CGNAT 100.64/10
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));

        // IPv4 reserved 240/4
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 254))));

        // Public IPv4 -- NOT private
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));

        // IPv6 loopback
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));

        // IPv6 unspecified
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));

        // IPv6 link-local fe80::/10
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfebf, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff
        ))));

        // IPv6 ULA fc00::/7
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfdff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff
        ))));

        // Public IPv6 -- NOT private
        assert!(!is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888
        ))));
    }

    // -- Hostname blocklist tests -------------------------------------------

    #[test]
    fn test_blocked_hostnames() {
        assert!(is_blocked_hostname("localhost"));
        assert!(is_blocked_hostname("LOCALHOST"));
        assert!(is_blocked_hostname("metadata.google.internal"));
        assert!(is_blocked_hostname("foo.metadata.google.internal"));
        assert!(is_blocked_hostname("metadata.aws.internal"));

        assert!(!is_blocked_hostname("google.com"));
        assert!(!is_blocked_hostname("api.anthropic.com"));
        assert!(!is_blocked_hostname("example.com"));
    }

    // -- Original tests (preserved) -----------------------------------------

    #[test]
    fn test_private_host_detection() {
        // The old is_private_host is replaced; these now exercise the new
        // IP-based checks via is_private_ip + is_blocked_hostname.
        assert!(is_blocked_hostname("localhost"));
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("169.254.169.254".parse().unwrap()));
        assert!(is_blocked_hostname("metadata.google.internal"));
        assert!(!is_private_ip("93.184.216.34".parse().unwrap())); // example.com
        assert!(!is_blocked_hostname("api.anthropic.com"));
    }

    #[tokio::test]
    async fn test_http_fetch_invalid_url() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_1".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "not a url"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_http_fetch_blocks_ssrf() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_2".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://169.254.169.254/latest/meta-data/"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("private") || result.content.contains("Access denied"));
    }

    #[tokio::test]
    async fn test_http_fetch_blocks_localhost() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_3".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://localhost:8080/admin"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_http_fetch_blocks_bad_scheme() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_4".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "file:///etc/passwd"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    // -- New SSRF tests -----------------------------------------------------

    #[tokio::test]
    async fn test_blocks_ipv6_loopback() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_ipv6_lo".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://[::1]:8080/"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error, "IPv6 loopback must be blocked");
        assert!(
            result.content.contains("private") || result.content.contains("Access denied"),
            "Error should mention private/access denied, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_blocks_zero_address() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_zero".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://0.0.0.0:8080/"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error, "0.0.0.0 must be blocked");
        assert!(
            result.content.contains("private") || result.content.contains("Access denied"),
            "Error should mention private/access denied, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_blocks_metadata_variants() {
        let skill = HttpFetchSkill::new();

        // AWS-style metadata IP
        let call = ToolCall {
            id: "test_meta_ip".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://169.254.169.254/"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error, "169.254.169.254 must be blocked");

        // GCP metadata hostname
        let call = ToolCall {
            id: "test_meta_gcp".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://metadata.google.internal/"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error, "metadata.google.internal must be blocked");
    }

    #[tokio::test]
    async fn test_allows_public_host() {
        let skill = HttpFetchSkill::new();
        let call = ToolCall {
            id: "test_public".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://example.com/"}),
        };
        let result = skill.execute(call).await.unwrap();
        // The request itself may fail (network, timeout, etc.) but it should
        // NOT be blocked by SSRF protection. If it *is* blocked, our
        // content will contain "Access denied" or "blocked hostname".
        let blocked =
            result.content.contains("Access denied") || result.content.contains("blocked hostname");
        assert!(
            !blocked,
            "Public host example.com should not be blocked by SSRF protection, got: {}",
            result.content
        );
    }

    // -- validate_arguments tests -------------------------------------------

    #[test]
    fn test_validate_arguments_denies_disallowed_host() {
        let skill = HttpFetchSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::NetworkAccess {
            allowed_hosts: vec!["api.anthropic.com".to_string()],
        });

        let call = ToolCall {
            id: "test_va_1".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "http://evil.com/payload"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_arguments_allows_permitted_host() {
        let skill = HttpFetchSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::NetworkAccess {
            allowed_hosts: vec!["api.anthropic.com".to_string()],
        });

        let call = ToolCall {
            id: "test_va_2".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "https://api.anthropic.com/v1/messages"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_arguments_wildcard_allows_all() {
        let skill = HttpFetchSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::NetworkAccess {
            allowed_hosts: vec!["*".to_string()],
        });

        let call = ToolCall {
            id: "test_va_3".to_string(),
            name: "http_fetch".to_string(),
            arguments: serde_json::json!({"url": "https://any-host.example.com/path"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_ok());
    }
}

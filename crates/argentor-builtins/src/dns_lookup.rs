use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tracing::info;

const DEFAULT_PORT: u16 = 443;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// DNS lookup skill -- resolve hostnames, perform reverse lookups, and check
/// network connectivity. Inspired by network diagnostic tools.
///
/// Supported operations:
/// - `resolve` — resolve hostname to IP addresses (A/AAAA)
/// - `reverse` — reverse DNS lookup from IP address
/// - `check_connectivity` — try TCP connect, return success/failure with latency
pub struct DnsLookupSkill {
    descriptor: SkillDescriptor,
}

impl DnsLookupSkill {
    /// Create a new DNS lookup skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "dns_lookup".to_string(),
                description:
                    "DNS resolution, reverse lookups, and connectivity checks for hostnames and IPs."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["resolve", "reverse", "check_connectivity"],
                            "description": "The operation to perform"
                        },
                        "hostname": {
                            "type": "string",
                            "description": "Hostname to resolve or check connectivity (for resolve, check_connectivity)"
                        },
                        "ip": {
                            "type": "string",
                            "description": "IP address for reverse lookup"
                        },
                        "port": {
                            "type": "integer",
                            "description": "Port for connectivity check (default: 443)"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![Capability::NetworkAccess {
                    allowed_hosts: vec![],
                }],
                requires_approval: false,
            },
        }
    }
}

impl Default for DnsLookupSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for DnsLookupSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        info!(operation = %operation, "DnsLookup execute");

        match operation.as_str() {
            "resolve" => {
                let hostname = call.arguments["hostname"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                if hostname.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "The 'hostname' parameter is required for resolve",
                    ));
                }

                // Use port 0 for pure DNS resolution
                let start = Instant::now();
                let lookup_result = {
                    let addr = format!("{hostname}:0");
                    tokio::net::lookup_host(addr).await
                };
                let elapsed = start.elapsed();

                match lookup_result {
                    Ok(addrs) => {
                        let ips: Vec<String> = addrs.map(|sa| sa.ip().to_string()).collect();

                        let ipv4: Vec<&str> = ips
                            .iter()
                            .filter(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok())
                            .map(std::string::String::as_str)
                            .collect();
                        let ipv6: Vec<&str> = ips
                            .iter()
                            .filter(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok())
                            .map(std::string::String::as_str)
                            .collect();

                        let result = serde_json::json!({
                            "hostname": hostname,
                            "addresses": ips,
                            "ipv4": ipv4,
                            "ipv6": ipv6,
                            "count": ips.len(),
                            "resolution_time_ms": elapsed.as_millis(),
                        });

                        if ips.is_empty() {
                            Ok(ToolResult::error(
                                &call.id,
                                format!("No addresses found for '{hostname}'"),
                            ))
                        } else {
                            Ok(ToolResult::success(&call.id, result.to_string()))
                        }
                    }
                    Err(e) => Ok(ToolResult::error(
                        &call.id,
                        format!("DNS resolution failed for '{hostname}': {e}"),
                    )),
                }
            }

            "reverse" => {
                let ip_str = call.arguments["ip"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                if ip_str.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "The 'ip' parameter is required for reverse",
                    ));
                }

                let ip: IpAddr = match ip_str.parse() {
                    Ok(ip) => ip,
                    Err(e) => {
                        return Ok(ToolResult::error(
                            &call.id,
                            format!("Invalid IP address '{ip_str}': {e}"),
                        ));
                    }
                };

                // Attempt reverse DNS by connecting to the IP and looking up
                // We use lookup_host with the IP to see if we can get a hostname
                let start = Instant::now();
                let lookup_result = {
                    let addr = format!("{ip}:0");
                    tokio::net::lookup_host(addr).await
                };
                let elapsed = start.elapsed();

                match lookup_result {
                    Ok(addrs) => {
                        let resolved: Vec<String> = addrs.map(|sa| sa.ip().to_string()).collect();

                        // Best-effort reverse: tokio::net::lookup_host doesn't do
                        // PTR lookups natively. We report what we can.
                        let result = serde_json::json!({
                            "ip": ip_str,
                            "resolved_addresses": resolved,
                            "note": "Basic reverse lookup via tokio; for full PTR records, a dedicated DNS library is needed.",
                            "resolution_time_ms": elapsed.as_millis(),
                        });

                        Ok(ToolResult::success(&call.id, result.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(
                        &call.id,
                        format!("Reverse lookup failed for '{ip_str}': {e}"),
                    )),
                }
            }

            "check_connectivity" => {
                let hostname = call.arguments["hostname"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                if hostname.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "The 'hostname' parameter is required for check_connectivity",
                    ));
                }

                let port = call.arguments["port"]
                    .as_u64()
                    .map(|v| v as u16)
                    .unwrap_or(DEFAULT_PORT);

                let start = Instant::now();

                // First resolve DNS
                let dns_result = {
                    let addr = format!("{hostname}:{port}");
                    tokio::net::lookup_host(addr).await
                };
                let dns_elapsed = start.elapsed();

                match dns_result {
                    Ok(mut addrs) => {
                        // Try to connect to the first resolved address
                        if let Some(socket_addr) = addrs.next() {
                            let connect_start = Instant::now();
                            let connect_result = tokio::time::timeout(
                                CONNECT_TIMEOUT,
                                tokio::net::TcpStream::connect(socket_addr),
                            )
                            .await;

                            let connect_elapsed = connect_start.elapsed();
                            let total_elapsed = start.elapsed();

                            match connect_result {
                                Ok(Ok(_stream)) => {
                                    let result = serde_json::json!({
                                        "hostname": hostname,
                                        "port": port,
                                        "status": "reachable",
                                        "resolved_ip": socket_addr.ip().to_string(),
                                        "dns_time_ms": dns_elapsed.as_millis(),
                                        "connect_time_ms": connect_elapsed.as_millis(),
                                        "total_time_ms": total_elapsed.as_millis(),
                                    });
                                    Ok(ToolResult::success(&call.id, result.to_string()))
                                }
                                Ok(Err(e)) => {
                                    let result = serde_json::json!({
                                        "hostname": hostname,
                                        "port": port,
                                        "status": "unreachable",
                                        "resolved_ip": socket_addr.ip().to_string(),
                                        "error": format!("Connection failed: {e}"),
                                        "dns_time_ms": dns_elapsed.as_millis(),
                                        "total_time_ms": total_elapsed.as_millis(),
                                    });
                                    Ok(ToolResult::success(&call.id, result.to_string()))
                                }
                                Err(_) => {
                                    let result = serde_json::json!({
                                        "hostname": hostname,
                                        "port": port,
                                        "status": "timeout",
                                        "resolved_ip": socket_addr.ip().to_string(),
                                        "error": format!("Connection timed out after {}s", CONNECT_TIMEOUT.as_secs()),
                                        "dns_time_ms": dns_elapsed.as_millis(),
                                        "total_time_ms": start.elapsed().as_millis(),
                                    });
                                    Ok(ToolResult::success(&call.id, result.to_string()))
                                }
                            }
                        } else {
                            Ok(ToolResult::error(
                                &call.id,
                                format!("DNS resolved but no addresses returned for '{hostname}'"),
                            ))
                        }
                    }
                    Err(e) => {
                        let result = serde_json::json!({
                            "hostname": hostname,
                            "port": port,
                            "status": "dns_failed",
                            "error": format!("DNS resolution failed: {e}"),
                            "total_time_ms": start.elapsed().as_millis(),
                        });
                        Ok(ToolResult::error(&call.id, result.to_string()))
                    }
                }
            }

            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation '{operation}'. Valid: resolve, reverse, check_connectivity"
                ),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptor() {
        let skill = DnsLookupSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "dns_lookup");
        assert!(!desc.required_capabilities.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_localhost() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "resolve",
                "hostname": "localhost"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        // localhost should always resolve
        assert!(
            !result.is_error,
            "localhost should resolve: {}",
            result.content
        );
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["count"].as_u64().unwrap() > 0);
        assert_eq!(parsed["hostname"], "localhost");
    }

    #[tokio::test]
    async fn test_resolve_missing_hostname() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "resolve"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_reverse_loopback() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t3".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "reverse",
                "ip": "127.0.0.1"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        // Reverse lookup for 127.0.0.1 should succeed (at minimum returning the IP)
        assert!(!result.is_error, "reverse 127.0.0.1: {}", result.content);
    }

    #[tokio::test]
    async fn test_reverse_invalid_ip() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t4".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "reverse",
                "ip": "not-an-ip"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Invalid IP"));
    }

    #[tokio::test]
    async fn test_reverse_missing_ip() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t5".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "reverse"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_check_connectivity_missing_hostname() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t6".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "check_connectivity"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_check_connectivity_localhost() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t7".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "check_connectivity",
                "hostname": "localhost",
                "port": 1
            }),
        };
        let result = skill.execute(call).await.unwrap();
        // Port 1 is almost certainly not open, so we expect either
        // "unreachable" or a connection error, but DNS should resolve
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        // The status field should exist
        let status = parsed["status"].as_str().unwrap_or("unknown");
        assert!(
            status == "unreachable" || status == "timeout" || status == "reachable",
            "Unexpected status: {status}"
        );
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t8".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "invalid"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_resolve_nonexistent_host() {
        let skill = DnsLookupSkill::new();
        let call = ToolCall {
            id: "t9".to_string(),
            name: "dns_lookup".to_string(),
            arguments: serde_json::json!({
                "operation": "resolve",
                "hostname": "this-host-definitely-does-not-exist-12345.invalid"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }
}

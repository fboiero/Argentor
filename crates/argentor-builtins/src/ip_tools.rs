//! IP address tools skill for the Argentor AI agent framework.
//!
//! Provides IP parsing, CIDR validation, subnet calculation, IP range expansion,
//! and classification (private/public/loopback).

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr};

/// IP address tools skill for parsing, validation, and subnet operations.
pub struct IpToolsSkill {
    descriptor: SkillDescriptor,
}

impl IpToolsSkill {
    /// Create a new IP tools skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "ip_tools".to_string(),
                description: "IP parsing, CIDR validation, subnet calculator, IP range expansion, and classification.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["parse", "validate", "classify", "cidr_info", "in_range", "to_binary", "reverse_dns", "compare"],
                            "description": "The IP operation to perform"
                        },
                        "ip": {
                            "type": "string",
                            "description": "IP address to process"
                        },
                        "cidr": {
                            "type": "string",
                            "description": "CIDR notation (e.g., 192.168.1.0/24)"
                        },
                        "ip_a": {
                            "type": "string",
                            "description": "First IP for comparison"
                        },
                        "ip_b": {
                            "type": "string",
                            "description": "Second IP for comparison"
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

impl Default for IpToolsSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Classify an IP address.
fn classify_ip(ip: &IpAddr) -> Value {
    let (is_loopback, is_private, is_link_local, is_multicast, ip_version) = match ip {
        IpAddr::V4(v4) => {
            let is_private = v4.is_private();
            let is_loopback = v4.is_loopback();
            let is_link_local = v4.is_link_local();
            let is_multicast = v4.is_multicast();
            (is_loopback, is_private, is_link_local, is_multicast, "v4")
        }
        IpAddr::V6(v6) => {
            let is_loopback = v6.is_loopback();
            let is_multicast = v6.is_multicast();
            (is_loopback, false, false, is_multicast, "v6")
        }
    };

    let classification = if is_loopback {
        "loopback"
    } else if is_private {
        "private"
    } else if is_link_local {
        "link-local"
    } else if is_multicast {
        "multicast"
    } else {
        "public"
    };

    json!({
        "ip": ip.to_string(),
        "version": ip_version,
        "classification": classification,
        "is_loopback": is_loopback,
        "is_private": is_private,
        "is_link_local": is_link_local,
        "is_multicast": is_multicast
    })
}

/// Parse CIDR notation and return network info.
fn parse_cidr(cidr: &str) -> Result<Value, String> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid CIDR notation: '{cidr}'. Expected format: IP/prefix"
        ));
    }

    let ip: Ipv4Addr = parts[0]
        .parse()
        .map_err(|e| format!("Invalid IP address: {e}"))?;
    let prefix: u32 = parts[1]
        .parse()
        .map_err(|e| format!("Invalid prefix length: {e}"))?;

    if prefix > 32 {
        return Err(format!("Prefix length {prefix} exceeds maximum of 32"));
    }

    let ip_u32 = u32::from(ip);
    let mask = if prefix == 0 {
        0u32
    } else {
        !0u32 << (32 - prefix)
    };
    let network = ip_u32 & mask;
    let broadcast = network | !mask;
    let host_count = if prefix >= 31 {
        2u64.pow(32 - prefix)
    } else {
        (2u64.pow(32 - prefix)).saturating_sub(2)
    };

    let network_ip = Ipv4Addr::from(network);
    let broadcast_ip = Ipv4Addr::from(broadcast);
    let mask_ip = Ipv4Addr::from(mask);

    let first_host = if prefix < 31 {
        Some(Ipv4Addr::from(network + 1).to_string())
    } else {
        None
    };
    let last_host = if prefix < 31 {
        Some(Ipv4Addr::from(broadcast - 1).to_string())
    } else {
        None
    };

    Ok(json!({
        "cidr": cidr,
        "network": network_ip.to_string(),
        "broadcast": broadcast_ip.to_string(),
        "netmask": mask_ip.to_string(),
        "prefix_length": prefix,
        "host_count": host_count,
        "first_host": first_host,
        "last_host": last_host
    }))
}

/// Check if an IP is within a CIDR range.
fn ip_in_cidr(ip_str: &str, cidr: &str) -> Result<bool, String> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return Err("Invalid CIDR notation".to_string());
    }

    let network_ip: Ipv4Addr = parts[0]
        .parse()
        .map_err(|e| format!("Invalid network IP: {e}"))?;
    let prefix: u32 = parts[1]
        .parse()
        .map_err(|e| format!("Invalid prefix: {e}"))?;
    let ip: Ipv4Addr = ip_str.parse().map_err(|e| format!("Invalid IP: {e}"))?;

    if prefix > 32 {
        return Err("Prefix exceeds 32".to_string());
    }

    let mask = if prefix == 0 {
        0u32
    } else {
        !0u32 << (32 - prefix)
    };
    let network = u32::from(network_ip) & mask;
    let ip_network = u32::from(ip) & mask;

    Ok(network == ip_network)
}

/// Convert IP to binary representation.
fn ip_to_binary(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets
                .iter()
                .map(|o| format!("{o:08b}"))
                .collect::<Vec<_>>()
                .join(".")
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            segments
                .iter()
                .map(|s| format!("{s:016b}"))
                .collect::<Vec<_>>()
                .join(":")
        }
    }
}

/// Generate reverse DNS name for an IP.
fn reverse_dns_name(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            format!(
                "{}.{}.{}.{}.in-addr.arpa",
                octets[3], octets[2], octets[1], octets[0]
            )
        }
        IpAddr::V6(v6) => {
            let expanded = format!("{:032x}", u128::from(*v6));
            let reversed: String = expanded
                .chars()
                .rev()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(".");
            format!("{reversed}.ip6.arpa")
        }
    }
}

#[async_trait]
impl Skill for IpToolsSkill {
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

        match operation {
            "parse" | "classify" => {
                let ip_str = match call.arguments["ip"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip'")),
                };
                let ip: IpAddr = match ip_str.parse() {
                    Ok(ip) => ip,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid IP address: {e}"))),
                };
                let info = classify_ip(&ip);
                Ok(ToolResult::success(&call.id, info.to_string()))
            }
            "validate" => {
                let ip_str = match call.arguments["ip"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip'")),
                };
                let valid = ip_str.parse::<IpAddr>().is_ok();
                let response = json!({ "valid": valid, "input": ip_str });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "cidr_info" => {
                let cidr = match call.arguments["cidr"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'cidr'")),
                };
                match parse_cidr(cidr) {
                    Ok(info) => Ok(ToolResult::success(&call.id, info.to_string())),
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "in_range" => {
                let ip_str = match call.arguments["ip"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip'")),
                };
                let cidr = match call.arguments["cidr"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'cidr'")),
                };
                match ip_in_cidr(ip_str, cidr) {
                    Ok(in_range) => {
                        let response = json!({
                            "ip": ip_str,
                            "cidr": cidr,
                            "in_range": in_range
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "to_binary" => {
                let ip_str = match call.arguments["ip"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip'")),
                };
                let ip: IpAddr = match ip_str.parse() {
                    Ok(ip) => ip,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid IP: {e}"))),
                };
                let binary = ip_to_binary(&ip);
                let response = json!({ "ip": ip_str, "binary": binary });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "reverse_dns" => {
                let ip_str = match call.arguments["ip"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip'")),
                };
                let ip: IpAddr = match ip_str.parse() {
                    Ok(ip) => ip,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid IP: {e}"))),
                };
                let ptr = reverse_dns_name(&ip);
                let response = json!({ "ip": ip_str, "reverse_dns": ptr });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "compare" => {
                let ip_a_str = match call.arguments["ip_a"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip_a'")),
                };
                let ip_b_str = match call.arguments["ip_b"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'ip_b'")),
                };
                let ip_a: IpAddr = match ip_a_str.parse() {
                    Ok(ip) => ip,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid ip_a: {e}"))),
                };
                let ip_b: IpAddr = match ip_b_str.parse() {
                    Ok(ip) => ip,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Invalid ip_b: {e}"))),
                };
                let same_version = matches!(
                    (&ip_a, &ip_b),
                    (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
                );
                let response = json!({
                    "ip_a": ip_a_str,
                    "ip_b": ip_b_str,
                    "equal": ip_a == ip_b,
                    "same_version": same_version,
                    "a_classification": classify_ip(&ip_a)["classification"],
                    "b_classification": classify_ip(&ip_b)["classification"]
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: parse, validate, classify, cidr_info, in_range, to_binary, reverse_dns, compare"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "ip_tools".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_parse_ipv4() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "parse", "ip": "192.168.1.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["version"], "v4");
        assert_eq!(parsed["classification"], "private");
        assert_eq!(parsed["is_private"], true);
    }

    #[tokio::test]
    async fn test_parse_loopback() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "parse", "ip": "127.0.0.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["classification"], "loopback");
        assert_eq!(parsed["is_loopback"], true);
    }

    #[tokio::test]
    async fn test_parse_public() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "parse", "ip": "8.8.8.8"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["classification"], "public");
    }

    #[tokio::test]
    async fn test_parse_ipv6() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "parse", "ip": "::1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["version"], "v6");
        assert_eq!(parsed["is_loopback"], true);
    }

    #[tokio::test]
    async fn test_validate_valid() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "validate", "ip": "10.0.0.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_validate_invalid() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "validate", "ip": "999.999.999.999"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_cidr_info() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "cidr_info", "cidr": "192.168.1.0/24"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["network"], "192.168.1.0");
        assert_eq!(parsed["broadcast"], "192.168.1.255");
        assert_eq!(parsed["netmask"], "255.255.255.0");
        assert_eq!(parsed["host_count"], 254);
        assert_eq!(parsed["first_host"], "192.168.1.1");
        assert_eq!(parsed["last_host"], "192.168.1.254");
    }

    #[tokio::test]
    async fn test_cidr_small_subnet() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "cidr_info", "cidr": "10.0.0.0/30"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["host_count"], 2);
    }

    #[tokio::test]
    async fn test_in_range_true() {
        let skill = IpToolsSkill::new();
        let call = make_call(
            json!({"operation": "in_range", "ip": "192.168.1.50", "cidr": "192.168.1.0/24"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["in_range"], true);
    }

    #[tokio::test]
    async fn test_in_range_false() {
        let skill = IpToolsSkill::new();
        let call =
            make_call(json!({"operation": "in_range", "ip": "10.0.0.1", "cidr": "192.168.1.0/24"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["in_range"], false);
    }

    #[tokio::test]
    async fn test_to_binary() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "to_binary", "ip": "192.168.1.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["binary"], "11000000.10101000.00000001.00000001");
    }

    #[tokio::test]
    async fn test_reverse_dns() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "reverse_dns", "ip": "192.168.1.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["reverse_dns"], "1.1.168.192.in-addr.arpa");
    }

    #[tokio::test]
    async fn test_compare_same() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "compare", "ip_a": "8.8.8.8", "ip_b": "8.8.8.8"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["equal"], true);
        assert_eq!(parsed["same_version"], true);
    }

    #[tokio::test]
    async fn test_compare_different() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "compare", "ip_a": "8.8.8.8", "ip_b": "1.1.1.1"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["equal"], false);
    }

    #[tokio::test]
    async fn test_invalid_ip() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"operation": "parse", "ip": "not-an-ip"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = IpToolsSkill::new();
        let call = make_call(json!({"ip": "8.8.8.8"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_descriptor_name() {
        let skill = IpToolsSkill::new();
        assert_eq!(skill.descriptor().name, "ip_tools");
    }
}

//! Data format validation skill for the Argentor agent framework.
//!
//! Provides validation for common data formats without external dependencies
//! (uses only the `regex` crate and standard library). Inspired by the Vercel AI
//! SDK Superagent verify tool pattern.
//!
//! # Supported formats
//!
//! - `email` — RFC 5322 basic validation
//! - `url` — HTTP/HTTPS URL validation
//! - `ipv4` — IPv4 address
//! - `ipv6` — IPv6 address
//! - `uuid` — UUID format (any version)
//! - `phone` — International phone number (E.164-ish)
//! - `credit_card` — Luhn algorithm validation
//! - `date` — ISO 8601 date (YYYY-MM-DD)
//! - `datetime` — ISO 8601 datetime
//! - `hex_color` — Hex color code (#RGB or #RRGGBB)
//! - `semver` — Semantic versioning
//! - `json` — Valid JSON string
//! - `base64` — Valid base64-encoded string
//! - `domain` — Valid domain name
//! - `mac_address` — MAC address (xx:xx:xx:xx:xx:xx or xx-xx-xx-xx-xx-xx)

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use regex::Regex;

/// Skill that validates values against common data formats.
pub struct DataValidatorSkill {
    descriptor: SkillDescriptor,
}

impl DataValidatorSkill {
    /// Create a new `DataValidatorSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "data_validator".to_string(),
                description: "Validate data against common formats: email, url, ipv4, ipv6, \
                              uuid, phone, credit_card, date, datetime, hex_color, semver, \
                              json, base64, domain, mac_address."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "format": {
                            "type": "string",
                            "enum": [
                                "email", "url", "ipv4", "ipv6", "uuid", "phone",
                                "credit_card", "date", "datetime", "hex_color",
                                "semver", "json", "base64", "domain", "mac_address"
                            ],
                            "description": "The data format to validate against"
                        },
                        "value": {
                            "type": "string",
                            "description": "The value to validate"
                        }
                    },
                    "required": ["format", "value"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for DataValidatorSkill {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Validation functions
// ---------------------------------------------------------------------------

/// Build a validation result JSON value.
fn result_json(valid: bool, format: &str, details: &str) -> serde_json::Value {
    serde_json::json!({
        "valid": valid,
        "format": format,
        "details": details,
    })
}

fn validate_email(value: &str) -> serde_json::Value {
    // Basic RFC 5322: local@domain, local part allows a broad set of characters
    let re = Regex::new(
        r"(?i)^[a-z0-9!#$%&'*+/=?^_`{|}~.-]+@[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$",
    );
    match re {
        Ok(re) if re.is_match(value) => {
            let parts: Vec<&str> = value.splitn(2, '@').collect();
            let domain = parts.get(1).copied().unwrap_or("");
            result_json(
                true,
                "email",
                &format!("Valid email address (domain: {domain})"),
            )
        }
        _ => result_json(false, "email", "Invalid email address format"),
    }
}

fn validate_url(value: &str) -> serde_json::Value {
    // Must be http or https with a host
    let re = Regex::new(r"^https?://[^\s/$.?#].[^\s]*$");
    match re {
        Ok(re) if re.is_match(value) => {
            let scheme = if value.starts_with("https://") {
                "https"
            } else {
                "http"
            };
            result_json(true, "url", &format!("Valid URL (scheme: {scheme})"))
        }
        _ => result_json(false, "url", "Invalid URL format (must be http or https)"),
    }
}

fn validate_ipv4(value: &str) -> serde_json::Value {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 4 {
        return result_json(false, "ipv4", "IPv4 must have exactly 4 octets");
    }

    for (i, part) in parts.iter().enumerate() {
        // No leading zeros (except "0" itself)
        if part.len() > 1 && part.starts_with('0') {
            return result_json(
                false,
                "ipv4",
                &format!("Octet {i} has leading zeros: '{part}'"),
            );
        }
        match part.parse::<u16>() {
            Ok(n) if n <= 255 => {}
            _ => {
                return result_json(
                    false,
                    "ipv4",
                    &format!("Octet {i} is not a valid number 0-255: '{part}'"),
                );
            }
        }
    }

    result_json(true, "ipv4", "Valid IPv4 address")
}

fn validate_ipv6(value: &str) -> serde_json::Value {
    // Use std::net for proper parsing (handles :: compression, embedded IPv4, etc.)
    match value.parse::<std::net::Ipv6Addr>() {
        Ok(_) => result_json(true, "ipv6", "Valid IPv6 address"),
        Err(e) => result_json(false, "ipv6", &format!("Invalid IPv6 address: {e}")),
    }
}

fn validate_uuid(value: &str) -> serde_json::Value {
    let re = Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$");
    match re {
        Ok(re) if re.is_match(value) => {
            // Detect version from the 13th character
            let version_char = value.chars().nth(14).unwrap_or('0');
            let version = match version_char {
                '1' => "v1 (time-based)",
                '2' => "v2 (DCE security)",
                '3' => "v3 (MD5 namespace)",
                '4' => "v4 (random)",
                '5' => "v5 (SHA-1 namespace)",
                '6' => "v6 (reordered time)",
                '7' => "v7 (Unix epoch time)",
                _ => "unknown version",
            };
            result_json(true, "uuid", &format!("Valid UUID ({version})"))
        }
        _ => result_json(false, "uuid", "Invalid UUID format"),
    }
}

fn validate_phone(value: &str) -> serde_json::Value {
    // E.164-ish: optional +, then 7-15 digits (spaces, dashes, dots, parens allowed
    // as separators but we count only digits)
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();
    let starts_valid = value.starts_with('+')
        || value.starts_with('(')
        || value.starts_with(|c: char| c.is_ascii_digit());

    // Only allow digits, +, -, spaces, dots, parens
    let all_valid = value
        .chars()
        .all(|c| c.is_ascii_digit() || "+- .()".contains(c));

    if starts_valid && all_valid && (7..=15).contains(&digits.len()) {
        result_json(
            true,
            "phone",
            &format!("Valid phone number ({} digits)", digits.len()),
        )
    } else if digits.len() < 7 {
        result_json(
            false,
            "phone",
            &format!("Too few digits ({}, minimum 7)", digits.len()),
        )
    } else if digits.len() > 15 {
        result_json(
            false,
            "phone",
            &format!("Too many digits ({}, maximum 15)", digits.len()),
        )
    } else {
        result_json(false, "phone", "Invalid phone number format")
    }
}

fn validate_credit_card(value: &str) -> serde_json::Value {
    // Strip spaces and dashes
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();

    if digits.len() < 13 || digits.len() > 19 {
        return result_json(
            false,
            "credit_card",
            &format!("Invalid length ({} digits, expected 13-19)", digits.len()),
        );
    }

    // Luhn algorithm
    let mut sum: u32 = 0;
    let mut double = false;

    for ch in digits.chars().rev() {
        let d = match ch.to_digit(10) {
            Some(d) => d,
            None => {
                return result_json(false, "credit_card", "Contains non-digit characters");
            }
        };

        let val = if double {
            let doubled = d * 2;
            if doubled > 9 {
                doubled - 9
            } else {
                doubled
            }
        } else {
            d
        };

        sum += val;
        double = !double;
    }

    if sum % 10 == 0 {
        // Detect card type by prefix
        let card_type = if digits.starts_with('4') {
            "Visa"
        } else if digits.starts_with("51")
            || digits.starts_with("52")
            || digits.starts_with("53")
            || digits.starts_with("54")
            || digits.starts_with("55")
        {
            "Mastercard"
        } else if digits.starts_with("34") || digits.starts_with("37") {
            "American Express"
        } else if digits.starts_with("6011") || digits.starts_with("65") {
            "Discover"
        } else {
            "Unknown"
        };
        result_json(
            true,
            "credit_card",
            &format!("Valid credit card number (Luhn check passed, type: {card_type})"),
        )
    } else {
        result_json(
            false,
            "credit_card",
            "Invalid credit card number (Luhn check failed)",
        )
    }
}

fn validate_date(value: &str) -> serde_json::Value {
    let re = Regex::new(r"^\d{4}-\d{2}-\d{2}$");
    match re {
        Ok(re) if re.is_match(value) => {
            // Parse and validate ranges
            let parts: Vec<&str> = value.split('-').collect();
            let year: u32 = parts[0].parse().unwrap_or(0);
            let month: u32 = parts[1].parse().unwrap_or(0);
            let day: u32 = parts[2].parse().unwrap_or(0);

            if !(1..=9999).contains(&year) {
                return result_json(false, "date", &format!("Invalid year: {year}"));
            }
            if !(1..=12).contains(&month) {
                return result_json(false, "date", &format!("Invalid month: {month}"));
            }

            let days_in_month = match month {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 => {
                    if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                        29
                    } else {
                        28
                    }
                }
                _ => 0,
            };

            if day < 1 || day > days_in_month {
                return result_json(
                    false,
                    "date",
                    &format!("Invalid day {day} for month {month} (max: {days_in_month})"),
                );
            }

            result_json(true, "date", "Valid ISO 8601 date")
        }
        _ => result_json(false, "date", "Invalid date format (expected YYYY-MM-DD)"),
    }
}

fn validate_datetime(value: &str) -> serde_json::Value {
    // ISO 8601 datetime: YYYY-MM-DDThh:mm:ss[.sss][Z|+/-hh:mm]
    let re = Regex::new(r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})?$");
    match re {
        Ok(re) if re.is_match(value) => {
            // Further validate using chrono-style parsing of the date part
            let date_part = &value[..10];
            let date_result = validate_date(date_part);
            if date_result["valid"] == false {
                return result_json(
                    false,
                    "datetime",
                    date_result["details"]
                        .as_str()
                        .unwrap_or("Invalid date portion"),
                );
            }

            // Validate time part
            let time_start = 11; // after 'T'
            let time_str = &value[time_start..];
            let time_core = if let Some(pos) = time_str.find(['.', 'Z', '+', '-']) {
                &time_str[..pos]
            } else {
                time_str
            };

            let time_parts: Vec<&str> = time_core.split(':').collect();
            if time_parts.len() == 3 {
                let hour: u32 = time_parts[0].parse().unwrap_or(99);
                let min: u32 = time_parts[1].parse().unwrap_or(99);
                let sec: u32 = time_parts[2].parse().unwrap_or(99);

                if hour > 23 {
                    return result_json(false, "datetime", &format!("Invalid hour: {hour}"));
                }
                if min > 59 {
                    return result_json(false, "datetime", &format!("Invalid minute: {min}"));
                }
                if sec > 59 {
                    return result_json(false, "datetime", &format!("Invalid second: {sec}"));
                }
            }

            let has_tz =
                value.ends_with('Z') || value.contains('+') || (value.matches('-').count() > 2);
            let tz_info = if has_tz { " with timezone" } else { " (local)" };

            result_json(
                true,
                "datetime",
                &format!("Valid ISO 8601 datetime{tz_info}"),
            )
        }
        _ => result_json(
            false,
            "datetime",
            "Invalid datetime format (expected ISO 8601: YYYY-MM-DDThh:mm:ss[.sss][Z|+hh:mm])",
        ),
    }
}

fn validate_hex_color(value: &str) -> serde_json::Value {
    let re = Regex::new(r"(?i)^#([0-9a-f]{3}|[0-9a-f]{6})$");
    match re {
        Ok(re) if re.is_match(value) => {
            let kind = if value.len() == 4 {
                "shorthand (#RGB)"
            } else {
                "full (#RRGGBB)"
            };
            result_json(true, "hex_color", &format!("Valid hex color ({kind})"))
        }
        _ => result_json(
            false,
            "hex_color",
            "Invalid hex color (expected #RGB or #RRGGBB)",
        ),
    }
}

fn validate_semver(value: &str) -> serde_json::Value {
    // Semantic versioning: MAJOR.MINOR.PATCH[-prerelease][+build]
    let re = Regex::new(
        r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$",
    );
    match re {
        Ok(re) if re.is_match(value) => {
            let core_part = value.split('-').next().unwrap_or(value);
            let core_part = core_part.split('+').next().unwrap_or(core_part);
            let has_pre = value.contains('-');
            let has_build = value.contains('+');
            let mut details = format!("Valid semver ({core_part})");
            if has_pre {
                details.push_str(" with pre-release");
            }
            if has_build {
                details.push_str(" with build metadata");
            }
            result_json(true, "semver", &details)
        }
        _ => result_json(
            false,
            "semver",
            "Invalid semver (expected MAJOR.MINOR.PATCH[-prerelease][+build])",
        ),
    }
}

fn validate_json(value: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(parsed) => {
            let kind = match &parsed {
                serde_json::Value::Object(_) => "object",
                serde_json::Value::Array(_) => "array",
                serde_json::Value::String(_) => "string",
                serde_json::Value::Number(_) => "number",
                serde_json::Value::Bool(_) => "boolean",
                serde_json::Value::Null => "null",
            };
            result_json(true, "json", &format!("Valid JSON ({kind})"))
        }
        Err(e) => result_json(false, "json", &format!("Invalid JSON: {e}")),
    }
}

fn validate_base64(value: &str) -> serde_json::Value {
    if value.is_empty() {
        return result_json(true, "base64", "Valid base64 (empty string)");
    }

    // Standard base64: A-Z, a-z, 0-9, +, /, with = padding
    let re = Regex::new(r"^[A-Za-z0-9+/]*={0,2}$");
    match re {
        Ok(re) if re.is_match(value) && value.len() % 4 == 0 => {
            // Try actual decoding to be sure
            match general_purpose::STANDARD.decode(value) {
                Ok(decoded) => result_json(
                    true,
                    "base64",
                    &format!("Valid base64 ({} bytes decoded)", decoded.len()),
                ),
                Err(e) => result_json(false, "base64", &format!("Invalid base64: {e}")),
            }
        }
        _ => result_json(
            false,
            "base64",
            "Invalid base64 encoding (bad characters or padding)",
        ),
    }
}

fn validate_domain(value: &str) -> serde_json::Value {
    // Domain: labels separated by dots, each 1-63 chars, total max 253
    if value.is_empty() || value.len() > 253 {
        return result_json(
            false,
            "domain",
            "Invalid domain (empty or exceeds 253 characters)",
        );
    }

    let re = Regex::new(r"^([a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?\.)*[a-zA-Z]{2,}$");
    match re {
        Ok(re) if re.is_match(value) => {
            // Validate individual label lengths
            for label in value.split('.') {
                if label.len() > 63 {
                    return result_json(
                        false,
                        "domain",
                        &format!("Label '{}...' exceeds 63 characters", &label[..20]),
                    );
                }
            }

            let label_count = value.split('.').count();
            let tld = value.rsplit('.').next().unwrap_or("");
            result_json(
                true,
                "domain",
                &format!("Valid domain ({label_count} labels, TLD: {tld})"),
            )
        }
        _ => result_json(false, "domain", "Invalid domain name format"),
    }
}

fn validate_mac_address(value: &str) -> serde_json::Value {
    // Accept xx:xx:xx:xx:xx:xx or xx-xx-xx-xx-xx-xx (case-insensitive hex)
    let re = Regex::new(r"(?i)^([0-9a-f]{2}[:-]){5}[0-9a-f]{2}$");
    match re {
        Ok(re) if re.is_match(value) => {
            let separator = if value.contains(':') { "colon" } else { "dash" };
            result_json(
                true,
                "mac_address",
                &format!("Valid MAC address ({separator}-separated)"),
            )
        }
        _ => result_json(
            false,
            "mac_address",
            "Invalid MAC address (expected xx:xx:xx:xx:xx:xx or xx-xx-xx-xx-xx-xx)",
        ),
    }
}

// ---------------------------------------------------------------------------
// Skill implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Skill for DataValidatorSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let format = call.arguments["format"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let value = call.arguments["value"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if format.is_empty() {
            return Ok(ToolResult::error(&call.id, "Format parameter is required"));
        }
        if value.is_empty() {
            return Ok(ToolResult::error(&call.id, "Value parameter is required"));
        }

        let result = match format.as_str() {
            "email" => validate_email(&value),
            "url" => validate_url(&value),
            "ipv4" => validate_ipv4(&value),
            "ipv6" => validate_ipv6(&value),
            "uuid" => validate_uuid(&value),
            "phone" => validate_phone(&value),
            "credit_card" => validate_credit_card(&value),
            "date" => validate_date(&value),
            "datetime" => validate_datetime(&value),
            "hex_color" => validate_hex_color(&value),
            "semver" => validate_semver(&value),
            "json" => validate_json(&value),
            "base64" => validate_base64(&value),
            "domain" => validate_domain(&value),
            "mac_address" => validate_mac_address(&value),
            _ => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!(
                        "Unknown format '{format}'. Supported: email, url, ipv4, ipv6, uuid, \
                         phone, credit_card, date, datetime, hex_color, semver, json, base64, \
                         domain, mac_address"
                    ),
                ));
            }
        };

        Ok(ToolResult::success(&call.id, result.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn skill() -> DataValidatorSkill {
        DataValidatorSkill::new()
    }

    fn call(format: &str, value: &str) -> ToolCall {
        ToolCall {
            id: "t1".to_string(),
            name: "data_validator".to_string(),
            arguments: serde_json::json!({"format": format, "value": value}),
        }
    }

    fn parse_result(result: &ToolResult) -> serde_json::Value {
        serde_json::from_str(&result.content).unwrap()
    }

    // -- Descriptor -----------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let s = skill();
        assert_eq!(s.descriptor().name, "data_validator");
        assert!(s.descriptor().required_capabilities.is_empty());
    }

    // -- Email ----------------------------------------------------------------

    #[tokio::test]
    async fn test_email_valid() {
        let s = skill();
        let r = s.execute(call("email", "user@example.com")).await.unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
        assert!(v["details"].as_str().unwrap().contains("example.com"));
    }

    #[tokio::test]
    async fn test_email_valid_complex() {
        let s = skill();
        let r = s
            .execute(call("email", "user.name+tag@sub.domain.co.uk"))
            .await
            .unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
    }

    #[tokio::test]
    async fn test_email_invalid() {
        let s = skill();
        for invalid in &["notanemail", "@missing.local", "user@", "user@.com"] {
            let r = s.execute(call("email", invalid)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {invalid}");
        }
    }

    // -- URL ------------------------------------------------------------------

    #[tokio::test]
    async fn test_url_valid() {
        let s = skill();
        let r = s
            .execute(call("url", "https://example.com/path?q=1"))
            .await
            .unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
        assert!(v["details"].as_str().unwrap().contains("https"));
    }

    #[tokio::test]
    async fn test_url_http() {
        let s = skill();
        let r = s.execute(call("url", "http://example.com")).await.unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
    }

    #[tokio::test]
    async fn test_url_invalid() {
        let s = skill();
        for invalid in &["ftp://example.com", "not a url", "://missing.scheme"] {
            let r = s.execute(call("url", invalid)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {invalid}");
        }
    }

    // -- IPv4 -----------------------------------------------------------------

    #[tokio::test]
    async fn test_ipv4_valid() {
        let s = skill();
        for ip in &["192.168.1.1", "0.0.0.0", "255.255.255.255", "10.0.0.1"] {
            let r = s.execute(call("ipv4", ip)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {ip}");
        }
    }

    #[tokio::test]
    async fn test_ipv4_invalid() {
        let s = skill();
        for ip in &[
            "256.0.0.1",
            "1.2.3",
            "1.2.3.4.5",
            "01.02.03.04",
            "abc.def.ghi.jkl",
        ] {
            let r = s.execute(call("ipv4", ip)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {ip}");
        }
    }

    // -- IPv6 -----------------------------------------------------------------

    #[tokio::test]
    async fn test_ipv6_valid() {
        let s = skill();
        for ip in &[
            "::1",
            "fe80::1",
            "2001:0db8:85a3::8a2e:0370:7334",
            "::ffff:192.0.2.1",
        ] {
            let r = s.execute(call("ipv6", ip)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {ip}");
        }
    }

    #[tokio::test]
    async fn test_ipv6_invalid() {
        let s = skill();
        for ip in &["not-ipv6", "12345::abcde", ":::1"] {
            let r = s.execute(call("ipv6", ip)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {ip}");
        }
    }

    // -- UUID -----------------------------------------------------------------

    #[tokio::test]
    async fn test_uuid_valid() {
        let s = skill();
        let r = s
            .execute(call("uuid", "550e8400-e29b-41d4-a716-446655440000"))
            .await
            .unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
        assert!(v["details"].as_str().unwrap().contains("v4"));
    }

    #[tokio::test]
    async fn test_uuid_invalid() {
        let s = skill();
        for invalid in &[
            "not-a-uuid",
            "550e8400-e29b-41d4-a716",
            "ZZZZZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZZZZZZZZZ",
        ] {
            let r = s.execute(call("uuid", invalid)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {invalid}");
        }
    }

    // -- Phone ----------------------------------------------------------------

    #[tokio::test]
    async fn test_phone_valid() {
        let s = skill();
        for phone in &[
            "+1234567890",
            "1234567890",
            "+44 20 7946 0958",
            "(212) 555-1234",
        ] {
            let r = s.execute(call("phone", phone)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {phone}");
        }
    }

    #[tokio::test]
    async fn test_phone_invalid() {
        let s = skill();
        for phone in &["123", "abcdefghij", "+1234567890123456"] {
            let r = s.execute(call("phone", phone)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {phone}");
        }
    }

    // -- Credit card ----------------------------------------------------------

    #[tokio::test]
    async fn test_credit_card_valid() {
        let s = skill();
        // Valid test numbers (Luhn-valid)
        for card in &["4111111111111111", "5500000000000004", "340000000000009"] {
            let r = s.execute(call("credit_card", card)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {card}");
        }
    }

    #[tokio::test]
    async fn test_credit_card_with_spaces() {
        let s = skill();
        let r = s
            .execute(call("credit_card", "4111 1111 1111 1111"))
            .await
            .unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], true);
    }

    #[tokio::test]
    async fn test_credit_card_invalid_luhn() {
        let s = skill();
        let r = s
            .execute(call("credit_card", "4111111111111112"))
            .await
            .unwrap();
        let v = parse_result(&r);
        assert_eq!(v["valid"], false);
        assert!(v["details"].as_str().unwrap().contains("Luhn"));
    }

    // -- Date -----------------------------------------------------------------

    #[tokio::test]
    async fn test_date_valid() {
        let s = skill();
        for d in &["2024-01-01", "2024-02-29", "2023-12-31"] {
            let r = s.execute(call("date", d)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {d}");
        }
    }

    #[tokio::test]
    async fn test_date_invalid() {
        let s = skill();
        for d in &["2024-13-01", "2023-02-29", "2024-00-01", "not-a-date"] {
            let r = s.execute(call("date", d)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {d}");
        }
    }

    // -- Datetime -------------------------------------------------------------

    #[tokio::test]
    async fn test_datetime_valid() {
        let s = skill();
        for dt in &[
            "2024-01-15T10:30:00Z",
            "2024-01-15T10:30:00+05:30",
            "2024-01-15T10:30:00.123Z",
            "2024-01-15T10:30:00",
        ] {
            let r = s.execute(call("datetime", dt)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {dt}");
        }
    }

    #[tokio::test]
    async fn test_datetime_invalid() {
        let s = skill();
        for dt in &["not-datetime", "2024-13-01T10:00:00Z", "2024-01-15"] {
            let r = s.execute(call("datetime", dt)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {dt}");
        }
    }

    // -- Hex color ------------------------------------------------------------

    #[tokio::test]
    async fn test_hex_color_valid() {
        let s = skill();
        for color in &["#fff", "#FFF", "#aabbcc", "#AABBCC", "#123456"] {
            let r = s.execute(call("hex_color", color)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {color}");
        }
    }

    #[tokio::test]
    async fn test_hex_color_invalid() {
        let s = skill();
        for color in &["#gg0000", "ff0000", "#12345", "#1234567"] {
            let r = s.execute(call("hex_color", color)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {color}");
        }
    }

    // -- Semver ---------------------------------------------------------------

    #[tokio::test]
    async fn test_semver_valid() {
        let s = skill();
        for sv in &[
            "1.0.0",
            "0.1.0",
            "1.2.3-alpha",
            "1.2.3-alpha.1",
            "1.2.3+build.123",
        ] {
            let r = s.execute(call("semver", sv)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {sv}");
        }
    }

    #[tokio::test]
    async fn test_semver_invalid() {
        let s = skill();
        for sv in &["1.0", "v1.0.0", "01.0.0", "1.0.0.0"] {
            let r = s.execute(call("semver", sv)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {sv}");
        }
    }

    // -- JSON -----------------------------------------------------------------

    #[tokio::test]
    async fn test_json_valid() {
        let s = skill();
        for j in &[
            r#"{"key": "value"}"#,
            "[1,2,3]",
            "\"hello\"",
            "42",
            "true",
            "null",
        ] {
            let r = s.execute(call("json", j)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {j}");
        }
    }

    #[tokio::test]
    async fn test_json_invalid() {
        let s = skill();
        for j in &["{missing: quotes}", "[1,2,", "undefined"] {
            let r = s.execute(call("json", j)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {j}");
        }
    }

    // -- Base64 ---------------------------------------------------------------

    #[tokio::test]
    async fn test_base64_valid() {
        let s = skill();
        for b in &["SGVsbG8=", "SGVsbG8gV29ybGQ=", "dGVzdA=="] {
            let r = s.execute(call("base64", b)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {b}");
        }
    }

    #[tokio::test]
    async fn test_base64_invalid() {
        let s = skill();
        for b in &["SGVsbG8!", "not base64 at all!!!"] {
            let r = s.execute(call("base64", b)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {b}");
        }
    }

    // -- Domain ---------------------------------------------------------------

    #[tokio::test]
    async fn test_domain_valid() {
        let s = skill();
        for d in &["example.com", "sub.domain.co.uk", "a-b.example.org"] {
            let r = s.execute(call("domain", d)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {d}");
        }
    }

    #[tokio::test]
    async fn test_domain_invalid() {
        let s = skill();
        for d in &[
            "-invalid.com",
            "no_underscores.com",
            ".leading-dot.com",
            "a",
        ] {
            let r = s.execute(call("domain", d)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {d}");
        }
    }

    // -- MAC address ----------------------------------------------------------

    #[tokio::test]
    async fn test_mac_valid() {
        let s = skill();
        for mac in &[
            "00:1A:2B:3C:4D:5E",
            "aa:bb:cc:dd:ee:ff",
            "AA-BB-CC-DD-EE-FF",
        ] {
            let r = s.execute(call("mac_address", mac)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], true, "Expected valid for: {mac}");
        }
    }

    #[tokio::test]
    async fn test_mac_invalid() {
        let s = skill();
        for mac in &["00:1A:2B:3C:4D", "GG:HH:II:JJ:KK:LL", "001A2B3C4D5E"] {
            let r = s.execute(call("mac_address", mac)).await.unwrap();
            let v = parse_result(&r);
            assert_eq!(v["valid"], false, "Expected invalid for: {mac}");
        }
    }

    // -- Error handling -------------------------------------------------------

    #[tokio::test]
    async fn test_unknown_format() {
        let s = skill();
        let r = s.execute(call("unknown_format", "test")).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown format"));
    }

    #[tokio::test]
    async fn test_empty_format() {
        let s = skill();
        let c = ToolCall {
            id: "t1".to_string(),
            name: "data_validator".to_string(),
            arguments: serde_json::json!({"format": "", "value": "test"}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Format parameter is required"));
    }

    #[tokio::test]
    async fn test_empty_value() {
        let s = skill();
        let c = ToolCall {
            id: "t1".to_string(),
            name: "data_validator".to_string(),
            arguments: serde_json::json!({"format": "email", "value": ""}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Value parameter is required"));
    }

    // -- Default trait --------------------------------------------------------

    #[test]
    fn test_default() {
        let s = DataValidatorSkill::default();
        assert_eq!(s.descriptor().name, "data_validator");
    }
}

//! Color conversion skill for the Argentor AI agent framework.
//!
//! Provides Hex/RGB/HSL conversion, named colors, contrast ratio calculation,
//! and lighten/darken operations.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::json;

/// Color conversion and manipulation skill.
pub struct ColorConverterSkill {
    descriptor: SkillDescriptor,
}

impl ColorConverterSkill {
    /// Create a new color converter skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "color_converter".to_string(),
                description:
                    "Color conversion (Hex/RGB/HSL), named colors, contrast ratio, lighten/darken."
                        .to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["hex_to_rgb", "rgb_to_hex", "hex_to_hsl", "hsl_to_hex", "rgb_to_hsl", "hsl_to_rgb", "named_to_hex", "contrast_ratio", "lighten", "darken", "parse"],
                            "description": "The color operation to perform"
                        },
                        "hex": {
                            "type": "string",
                            "description": "Hex color (e.g., #FF5733 or FF5733)"
                        },
                        "r": { "type": "integer", "description": "Red component (0-255)" },
                        "g": { "type": "integer", "description": "Green component (0-255)" },
                        "b": { "type": "integer", "description": "Blue component (0-255)" },
                        "h": { "type": "number", "description": "Hue (0-360)" },
                        "s": { "type": "number", "description": "Saturation (0-100)" },
                        "l": { "type": "number", "description": "Lightness (0-100)" },
                        "name": { "type": "string", "description": "Color name (e.g., red, blue)" },
                        "hex_a": { "type": "string", "description": "First hex color for contrast" },
                        "hex_b": { "type": "string", "description": "Second hex color for contrast" },
                        "amount": { "type": "number", "description": "Lighten/darken amount (0-100, default 10)" },
                        "color": { "type": "string", "description": "Color in any format for parsing" }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for ColorConverterSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse hex string to RGB tuple.
fn hex_to_rgb(hex: &str) -> Result<(u8, u8, u8), String> {
    let hex = hex.trim().strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return Err(format!(
            "Invalid hex color: expected 6 characters, got {}",
            hex.len()
        ));
    }
    let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| "Invalid red component")?;
    let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| "Invalid green component")?;
    let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| "Invalid blue component")?;
    Ok((r, g, b))
}

/// Convert RGB to hex string.
fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Convert RGB (0-255) to HSL (h:0-360, s:0-100, l:0-100).
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let r = r as f64 / 255.0;
    let g = g as f64 / 255.0;
    let b = b as f64 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let l = (max + min) / 2.0;

    if delta == 0.0 {
        return (0.0, 0.0, (l * 100.0).round());
    }

    let s = if l < 0.5 {
        delta / (max + min)
    } else {
        delta / (2.0 - max - min)
    };

    let h = if (max - r).abs() < f64::EPSILON {
        ((g - b) / delta) % 6.0
    } else if (max - g).abs() < f64::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };

    let h = ((h * 60.0) + 360.0) % 360.0;
    (h.round(), (s * 100.0).round(), (l * 100.0).round())
}

/// Convert HSL (h:0-360, s:0-100, l:0-100) to RGB (0-255).
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let s = s / 100.0;
    let l = l / 100.0;

    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

/// Calculate relative luminance of an sRGB color.
fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let srgb_to_linear = |c: u8| -> f64 {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * srgb_to_linear(r) + 0.7152 * srgb_to_linear(g) + 0.0722 * srgb_to_linear(b)
}

/// Calculate WCAG contrast ratio between two colors.
fn contrast_ratio(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> f64 {
    let l1 = relative_luminance(r1, g1, b1);
    let l2 = relative_luminance(r2, g2, b2);
    let lighter = l1.max(l2);
    let darker = l1.min(l2);
    ((lighter + 0.05) / (darker + 0.05) * 100.0).round() / 100.0
}

/// Look up a named color to hex.
fn named_color_to_hex(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "red" => Some("#FF0000"),
        "green" => Some("#008000"),
        "blue" => Some("#0000FF"),
        "white" => Some("#FFFFFF"),
        "black" => Some("#000000"),
        "yellow" => Some("#FFFF00"),
        "cyan" | "aqua" => Some("#00FFFF"),
        "magenta" | "fuchsia" => Some("#FF00FF"),
        "orange" => Some("#FFA500"),
        "purple" => Some("#800080"),
        "pink" => Some("#FFC0CB"),
        "gray" | "grey" => Some("#808080"),
        "brown" => Some("#A52A2A"),
        "navy" => Some("#000080"),
        "lime" => Some("#00FF00"),
        "teal" => Some("#008080"),
        "olive" => Some("#808000"),
        "maroon" => Some("#800000"),
        "silver" => Some("#C0C0C0"),
        "coral" => Some("#FF7F50"),
        "gold" => Some("#FFD700"),
        "indigo" => Some("#4B0082"),
        "violet" => Some("#EE82EE"),
        "turquoise" => Some("#40E0D0"),
        "salmon" => Some("#FA8072"),
        _ => None,
    }
}

#[async_trait]
impl Skill for ColorConverterSkill {
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
            "hex_to_rgb" => {
                let hex = match call.arguments["hex"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'hex'")),
                };
                match hex_to_rgb(hex) {
                    Ok((r, g, b)) => {
                        let response = json!({ "r": r, "g": g, "b": b, "hex": hex, "css": format!("rgb({r}, {g}, {b})") });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "rgb_to_hex" => {
                let r = call.arguments["r"].as_u64().unwrap_or(0) as u8;
                let g = call.arguments["g"].as_u64().unwrap_or(0) as u8;
                let b = call.arguments["b"].as_u64().unwrap_or(0) as u8;
                let hex = rgb_to_hex(r, g, b);
                let response = json!({ "hex": hex, "r": r, "g": g, "b": b });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "hex_to_hsl" => {
                let hex = match call.arguments["hex"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'hex'")),
                };
                match hex_to_rgb(hex) {
                    Ok((r, g, b)) => {
                        let (h, s, l) = rgb_to_hsl(r, g, b);
                        let response = json!({ "h": h, "s": s, "l": l, "hex": hex, "css": format!("hsl({h}, {s}%, {l}%)") });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "hsl_to_hex" => {
                let h = call.arguments["h"].as_f64().unwrap_or(0.0);
                let s = call.arguments["s"].as_f64().unwrap_or(0.0);
                let l = call.arguments["l"].as_f64().unwrap_or(0.0);
                let (r, g, b) = hsl_to_rgb(h, s, l);
                let hex = rgb_to_hex(r, g, b);
                let response = json!({ "hex": hex, "h": h, "s": s, "l": l });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "rgb_to_hsl" => {
                let r = call.arguments["r"].as_u64().unwrap_or(0) as u8;
                let g = call.arguments["g"].as_u64().unwrap_or(0) as u8;
                let b = call.arguments["b"].as_u64().unwrap_or(0) as u8;
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let response = json!({ "h": h, "s": s, "l": l, "css": format!("hsl({h}, {s}%, {l}%)") });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "hsl_to_rgb" => {
                let h = call.arguments["h"].as_f64().unwrap_or(0.0);
                let s = call.arguments["s"].as_f64().unwrap_or(0.0);
                let l = call.arguments["l"].as_f64().unwrap_or(0.0);
                let (r, g, b) = hsl_to_rgb(h, s, l);
                let response = json!({ "r": r, "g": g, "b": b, "css": format!("rgb({r}, {g}, {b})") });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "named_to_hex" => {
                let name = match call.arguments["name"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'name'")),
                };
                match named_color_to_hex(name) {
                    Some(hex) => {
                        let (r, g, b) = hex_to_rgb(hex).unwrap_or((0, 0, 0));
                        let response = json!({ "name": name, "hex": hex, "r": r, "g": g, "b": b });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    None => Ok(ToolResult::error(&call.id, format!("Unknown color name: '{name}'"))),
                }
            }
            "contrast_ratio" => {
                let hex_a = match call.arguments["hex_a"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'hex_a'")),
                };
                let hex_b = match call.arguments["hex_b"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'hex_b'")),
                };
                let (r1, g1, b1) = match hex_to_rgb(hex_a) {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("hex_a: {e}"))),
                };
                let (r2, g2, b2) = match hex_to_rgb(hex_b) {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("hex_b: {e}"))),
                };
                let ratio = contrast_ratio(r1, g1, b1, r2, g2, b2);
                let aa_normal = ratio >= 4.5;
                let aa_large = ratio >= 3.0;
                let aaa_normal = ratio >= 7.0;
                let response = json!({
                    "ratio": ratio,
                    "aa_normal": aa_normal,
                    "aa_large": aa_large,
                    "aaa_normal": aaa_normal,
                    "hex_a": hex_a,
                    "hex_b": hex_b
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "lighten" => {
                let hex = match call.arguments["hex"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'hex'")),
                };
                let amount = call.arguments["amount"].as_f64().unwrap_or(10.0);
                let (r, g, b) = match hex_to_rgb(hex) {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolResult::error(&call.id, e)),
                };
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let new_l = (l + amount).min(100.0);
                let (nr, ng, nb) = hsl_to_rgb(h, s, new_l);
                let new_hex = rgb_to_hex(nr, ng, nb);
                let response = json!({
                    "original": hex,
                    "result": new_hex,
                    "amount": amount
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "darken" => {
                let hex = match call.arguments["hex"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'hex'")),
                };
                let amount = call.arguments["amount"].as_f64().unwrap_or(10.0);
                let (r, g, b) = match hex_to_rgb(hex) {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolResult::error(&call.id, e)),
                };
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let new_l = (l - amount).max(0.0);
                let (nr, ng, nb) = hsl_to_rgb(h, s, new_l);
                let new_hex = rgb_to_hex(nr, ng, nb);
                let response = json!({
                    "original": hex,
                    "result": new_hex,
                    "amount": amount
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "parse" => {
                let color = match call.arguments["color"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'color'")),
                };
                // Try hex
                if let Ok((r, g, b)) = hex_to_rgb(color) {
                    let (h, s, l) = rgb_to_hsl(r, g, b);
                    let hex = rgb_to_hex(r, g, b);
                    let response = json!({
                        "format": "hex",
                        "hex": hex, "r": r, "g": g, "b": b,
                        "h": h, "s": s, "l": l
                    });
                    return Ok(ToolResult::success(&call.id, response.to_string()));
                }
                // Try named
                if let Some(hex) = named_color_to_hex(color) {
                    let (r, g, b) = hex_to_rgb(hex).unwrap_or((0, 0, 0));
                    let (h, s, l) = rgb_to_hsl(r, g, b);
                    let response = json!({
                        "format": "named",
                        "name": color,
                        "hex": hex, "r": r, "g": g, "b": b,
                        "h": h, "s": s, "l": l
                    });
                    return Ok(ToolResult::success(&call.id, response.to_string()));
                }
                Ok(ToolResult::error(&call.id, format!("Could not parse color: '{color}'")))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: hex_to_rgb, rgb_to_hex, hex_to_hsl, hsl_to_hex, rgb_to_hsl, hsl_to_rgb, named_to_hex, contrast_ratio, lighten, darken, parse"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "color_converter".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_hex_to_rgb() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "hex_to_rgb", "hex": "#FF5733"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["r"], 255);
        assert_eq!(parsed["g"], 87);
        assert_eq!(parsed["b"], 51);
    }

    #[tokio::test]
    async fn test_hex_to_rgb_no_hash() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "hex_to_rgb", "hex": "FF5733"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_rgb_to_hex() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "rgb_to_hex", "r": 255, "g": 87, "b": 51}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["hex"], "#FF5733");
    }

    #[tokio::test]
    async fn test_hex_to_hsl() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "hex_to_hsl", "hex": "#FF0000"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["h"], 0.0);
        assert_eq!(parsed["s"], 100.0);
        assert_eq!(parsed["l"], 50.0);
    }

    #[tokio::test]
    async fn test_hsl_to_hex_red() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "hsl_to_hex", "h": 0, "s": 100, "l": 50}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["hex"], "#FF0000");
    }

    #[tokio::test]
    async fn test_rgb_to_hsl_white() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "rgb_to_hsl", "r": 255, "g": 255, "b": 255}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["l"], 100.0);
    }

    #[tokio::test]
    async fn test_named_to_hex() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "named_to_hex", "name": "red"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["hex"], "#FF0000");
    }

    #[tokio::test]
    async fn test_named_unknown() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "named_to_hex", "name": "chartreuse"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_contrast_ratio_bw() {
        let skill = ColorConverterSkill::new();
        let call = make_call(
            json!({"operation": "contrast_ratio", "hex_a": "#FFFFFF", "hex_b": "#000000"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["ratio"], 21.0);
        assert_eq!(parsed["aaa_normal"], true);
    }

    #[tokio::test]
    async fn test_contrast_ratio_same_color() {
        let skill = ColorConverterSkill::new();
        let call = make_call(
            json!({"operation": "contrast_ratio", "hex_a": "#FF0000", "hex_b": "#FF0000"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["ratio"], 1.0);
    }

    #[tokio::test]
    async fn test_lighten() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "lighten", "hex": "#333333", "amount": 20}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_ne!(parsed["result"], "#333333");
    }

    #[tokio::test]
    async fn test_darken() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "darken", "hex": "#CCCCCC", "amount": 20}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_ne!(parsed["result"], "#CCCCCC");
    }

    #[tokio::test]
    async fn test_parse_hex() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "parse", "color": "#FF0000"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["format"], "hex");
        assert_eq!(parsed["r"], 255);
    }

    #[tokio::test]
    async fn test_parse_named() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "parse", "color": "blue"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["format"], "named");
        assert_eq!(parsed["hex"], "#0000FF");
    }

    #[tokio::test]
    async fn test_invalid_hex() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"operation": "hex_to_rgb", "hex": "#ZZZZZZ"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = ColorConverterSkill::new();
        let call = make_call(json!({"hex": "#FF0000"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_descriptor_name() {
        let skill = ColorConverterSkill::new();
        assert_eq!(skill.descriptor().name, "color_converter");
    }
}

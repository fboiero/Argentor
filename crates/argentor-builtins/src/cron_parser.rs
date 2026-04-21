//! Cron expression parsing skill for the Argentor AI agent framework.
//!
//! Provides cron expression parsing, validation, next occurrence calculation,
//! and human-readable description generation.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use chrono::{Datelike, NaiveDateTime, Timelike};
use serde_json::json;

/// Cron expression parsing and scheduling skill.
pub struct CronParserSkill {
    descriptor: SkillDescriptor,
}

impl CronParserSkill {
    /// Create a new cron parser skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "cron_parser".to_string(),
                description: "Parse cron expressions, calculate next occurrences, validate, and generate human-readable descriptions.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["validate", "describe", "next", "next_n", "parse"],
                            "description": "The cron operation to perform"
                        },
                        "expression": {
                            "type": "string",
                            "description": "Cron expression (5 fields: minute hour day month weekday)"
                        },
                        "from": {
                            "type": "string",
                            "description": "Start datetime (ISO 8601) for next occurrence calculation"
                        },
                        "count": {
                            "type": "integer",
                            "description": "Number of next occurrences to calculate (max 20)"
                        }
                    },
                    "required": ["operation", "expression"]
                }),
                required_capabilities: vec![],
            },
        }
    }
}

impl Default for CronParserSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed cron field with its allowed values.
#[derive(Debug, Clone)]
struct CronField {
    values: Vec<u32>,
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        self.values.contains(&value)
    }
}

/// Parse a single cron field (e.g., "*/5", "1,3,5", "1-5", "*").
fn parse_field(field: &str, min: u32, max: u32) -> Result<CronField, String> {
    let mut values = Vec::new();

    for part in field.split(',') {
        let part = part.trim();
        if part == "*" {
            values.extend(min..=max);
        } else if let Some(step_str) = part.strip_prefix("*/") {
            let step: u32 = step_str
                .parse()
                .map_err(|_| format!("Invalid step value: {step_str}"))?;
            if step == 0 {
                return Err("Step value cannot be 0".to_string());
            }
            let mut v = min;
            while v <= max {
                values.push(v);
                v += step;
            }
        } else if part.contains('-') {
            let range_parts: Vec<&str> = part.split('-').collect();
            if range_parts.len() != 2 {
                return Err(format!("Invalid range: {part}"));
            }
            let start: u32 = range_parts[0]
                .parse()
                .map_err(|_| format!("Invalid range start: {}", range_parts[0]))?;
            let end: u32 = range_parts[1]
                .parse()
                .map_err(|_| format!("Invalid range end: {}", range_parts[1]))?;
            if start > end || start < min || end > max {
                return Err(format!("Range {start}-{end} out of bounds ({min}-{max})"));
            }
            values.extend(start..=end);
        } else {
            let v: u32 = part.parse().map_err(|_| format!("Invalid value: {part}"))?;
            if v < min || v > max {
                return Err(format!("Value {v} out of bounds ({min}-{max})"));
            }
            values.push(v);
        }
    }

    values.sort();
    values.dedup();
    Ok(CronField { values })
}

/// Parsed cron expression with 5 fields.
struct CronExpr {
    minute: CronField,
    hour: CronField,
    day: CronField,
    month: CronField,
    weekday: CronField,
}

/// Parse a full cron expression (5 fields).
fn parse_cron(expr: &str) -> Result<CronExpr, String> {
    // Handle common aliases
    let expr = match expr.trim() {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        other => other,
    };

    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "Expected 5 fields (minute hour day month weekday), got {}",
            fields.len()
        ));
    }

    Ok(CronExpr {
        minute: parse_field(fields[0], 0, 59)?,
        hour: parse_field(fields[1], 0, 23)?,
        day: parse_field(fields[2], 1, 31)?,
        month: parse_field(fields[3], 1, 12)?,
        weekday: parse_field(fields[4], 0, 6)?,
    })
}

/// Find the next N occurrences of a cron expression starting from a given datetime.
fn next_occurrences(expr: &CronExpr, from: NaiveDateTime, count: usize) -> Vec<NaiveDateTime> {
    let mut results = Vec::new();
    let mut current = from + chrono::Duration::minutes(1);
    // Reset seconds
    current = current
        .date()
        .and_hms_opt(current.hour(), current.minute(), 0)
        .unwrap_or(current);

    let max_iterations = 525_960; // ~1 year of minutes
    let mut iterations = 0;

    while results.len() < count && iterations < max_iterations {
        iterations += 1;
        let minute = current.minute();
        let hour = current.hour();
        let day = current.day();
        let month = current.month();
        let weekday = current.weekday().num_days_from_sunday();

        if expr.minute.matches(minute)
            && expr.hour.matches(hour)
            && expr.day.matches(day)
            && expr.month.matches(month)
            && expr.weekday.matches(weekday)
        {
            results.push(current);
        }

        current += chrono::Duration::minutes(1);
    }

    results
}

/// Generate a human-readable description of a cron expression.
fn describe_cron(expr: &str) -> Result<String, String> {
    match expr.trim() {
        "@yearly" | "@annually" => return Ok("At midnight on January 1st, every year".to_string()),
        "@monthly" => return Ok("At midnight on the 1st of every month".to_string()),
        "@weekly" => return Ok("At midnight every Sunday".to_string()),
        "@daily" | "@midnight" => return Ok("At midnight every day".to_string()),
        "@hourly" => return Ok("At the start of every hour".to_string()),
        _ => {}
    }

    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err("Invalid cron expression".to_string());
    }

    let mut parts = Vec::new();

    // Minute
    if fields[0] == "*" {
        parts.push("Every minute".to_string());
    } else if fields[0].starts_with("*/") {
        parts.push(format!("Every {} minutes", &fields[0][2..]));
    } else {
        parts.push(format!("At minute {}", fields[0]));
    }

    // Hour
    if fields[1] != "*" {
        if fields[1].starts_with("*/") {
            parts.push(format!("every {} hours", &fields[1][2..]));
        } else {
            parts.push(format!("at hour {}", fields[1]));
        }
    }

    // Day
    if fields[2] != "*" {
        parts.push(format!("on day {} of the month", fields[2]));
    }

    // Month
    if fields[3] != "*" {
        let month_name = match fields[3] {
            "1" => "January",
            "2" => "February",
            "3" => "March",
            "4" => "April",
            "5" => "May",
            "6" => "June",
            "7" => "July",
            "8" => "August",
            "9" => "September",
            "10" => "October",
            "11" => "November",
            "12" => "December",
            other => other,
        };
        parts.push(format!("in {month_name}"));
    }

    // Weekday
    if fields[4] != "*" {
        let day_name = match fields[4] {
            "0" => "Sunday",
            "1" => "Monday",
            "2" => "Tuesday",
            "3" => "Wednesday",
            "4" => "Thursday",
            "5" => "Friday",
            "6" => "Saturday",
            other => other,
        };
        parts.push(format!("on {day_name}"));
    }

    Ok(parts.join(", "))
}

#[async_trait]
impl Skill for CronParserSkill {
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

        let expression = match call.arguments["expression"].as_str() {
            Some(v) => v,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'expression'",
                ))
            }
        };

        match operation {
            "validate" => {
                match parse_cron(expression) {
                    Ok(_) => {
                        let response = json!({ "valid": true, "expression": expression });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => {
                        let response = json!({ "valid": false, "error": e });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                }
            }
            "describe" => {
                match describe_cron(expression) {
                    Ok(desc) => {
                        let response = json!({ "description": desc, "expression": expression });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "parse" => {
                match parse_cron(expression) {
                    Ok(cron) => {
                        let response = json!({
                            "expression": expression,
                            "minute": cron.minute.values,
                            "hour": cron.hour.values,
                            "day": cron.day.values,
                            "month": cron.month.values,
                            "weekday": cron.weekday.values
                        });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, format!("Parse error: {e}"))),
                }
            }
            "next" => {
                let cron = match parse_cron(expression) {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Parse error: {e}"))),
                };
                let from = if let Some(from_str) = call.arguments["from"].as_str() {
                    NaiveDateTime::parse_from_str(from_str, "%Y-%m-%dT%H:%M:%S")
                        .or_else(|_| NaiveDateTime::parse_from_str(from_str, "%Y-%m-%d %H:%M:%S"))
                        .map_err(|e| format!("Invalid datetime: {e}"))
                        .unwrap_or_else(|_| chrono::Utc::now().naive_utc())
                } else {
                    chrono::Utc::now().naive_utc()
                };
                let occurrences = next_occurrences(&cron, from, 1);
                if let Some(next) = occurrences.first() {
                    let response = json!({
                        "next": next.format("%Y-%m-%dT%H:%M:%S").to_string(),
                        "expression": expression
                    });
                    Ok(ToolResult::success(&call.id, response.to_string()))
                } else {
                    Ok(ToolResult::error(&call.id, "Could not find next occurrence within 1 year"))
                }
            }
            "next_n" => {
                let cron = match parse_cron(expression) {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolResult::error(&call.id, format!("Parse error: {e}"))),
                };
                let count = call.arguments["count"]
                    .as_u64()
                    .unwrap_or(5)
                    .min(20) as usize;
                let from = if let Some(from_str) = call.arguments["from"].as_str() {
                    NaiveDateTime::parse_from_str(from_str, "%Y-%m-%dT%H:%M:%S")
                        .or_else(|_| NaiveDateTime::parse_from_str(from_str, "%Y-%m-%d %H:%M:%S"))
                        .unwrap_or_else(|_| chrono::Utc::now().naive_utc())
                } else {
                    chrono::Utc::now().naive_utc()
                };
                let occurrences = next_occurrences(&cron, from, count);
                let formatted: Vec<String> = occurrences
                    .iter()
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                    .collect();
                let response = json!({
                    "occurrences": formatted,
                    "count": formatted.len(),
                    "expression": expression
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: validate, describe, next, next_n, parse"),
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
            name: "cron_parser".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_validate_valid() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "validate", "expression": "*/5 * * * *"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], true);
    }

    #[tokio::test]
    async fn test_validate_invalid() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "validate", "expression": "60 * * * *"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_validate_too_few_fields() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "validate", "expression": "* * *"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn test_describe_every_5_minutes() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "describe", "expression": "*/5 * * * *"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let desc = parsed["description"].as_str().unwrap();
        assert!(desc.contains("5 minutes"));
    }

    #[tokio::test]
    async fn test_describe_alias_daily() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "describe", "expression": "@daily"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let desc = parsed["description"].as_str().unwrap();
        assert!(desc.contains("midnight"));
    }

    #[tokio::test]
    async fn test_parse_fields() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "parse", "expression": "0 9 * * 1-5"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["minute"], json!([0]));
        assert_eq!(parsed["hour"], json!([9]));
        assert_eq!(parsed["weekday"], json!([1, 2, 3, 4, 5]));
    }

    #[tokio::test]
    async fn test_next_occurrence() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({
            "operation": "next",
            "expression": "0 0 * * *",
            "from": "2025-01-15T10:30:00"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["next"], "2025-01-16T00:00:00");
    }

    #[tokio::test]
    async fn test_next_n_occurrences() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({
            "operation": "next_n",
            "expression": "0 12 * * *",
            "from": "2025-01-01T00:00:00",
            "count": 3
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 3);
        let occs = parsed["occurrences"].as_array().unwrap();
        assert_eq!(occs[0], "2025-01-01T12:00:00");
        assert_eq!(occs[1], "2025-01-02T12:00:00");
        assert_eq!(occs[2], "2025-01-03T12:00:00");
    }

    #[tokio::test]
    async fn test_comma_separated_values() {
        let cron = parse_cron("0,30 * * * *").unwrap();
        assert_eq!(cron.minute.values, vec![0, 30]);
    }

    #[tokio::test]
    async fn test_range_values() {
        let cron = parse_cron("* 9-17 * * *").unwrap();
        assert_eq!(cron.hour.values, vec![9, 10, 11, 12, 13, 14, 15, 16, 17]);
    }

    #[tokio::test]
    async fn test_step_values() {
        let cron = parse_cron("*/15 * * * *").unwrap();
        assert_eq!(cron.minute.values, vec![0, 15, 30, 45]);
    }

    #[tokio::test]
    async fn test_alias_hourly() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "parse", "expression": "@hourly"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["minute"], json!([0]));
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"expression": "* * * * *"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = CronParserSkill::new();
        let call = make_call(json!({"operation": "schedule", "expression": "* * * * *"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = CronParserSkill::new();
        assert_eq!(skill.descriptor().name, "cron_parser");
    }
}

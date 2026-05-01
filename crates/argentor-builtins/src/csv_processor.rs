//! CSV processing skill for the Argentor AI agent framework.
//!
//! Provides CSV parsing, column selection, filtering, sorting, statistics,
//! and format conversion (CSV to JSON and back).

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde_json::{json, Value};

/// CSV processing skill with parsing, filtering, sorting, and conversion.
pub struct CsvProcessorSkill {
    descriptor: SkillDescriptor,
}

impl CsvProcessorSkill {
    /// Create a new CSV processor skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "csv_processor".to_string(),
                description: "CSV parsing, column selection, filtering, sorting, statistics, and CSV/JSON conversion.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["parse", "to_json", "from_json", "select_columns", "filter", "sort", "statistics", "count_rows", "headers"],
                            "description": "The CSV operation to perform"
                        },
                        "csv": {
                            "type": "string",
                            "description": "CSV content to process"
                        },
                        "json_data": {
                            "type": "array",
                            "description": "JSON array of objects to convert to CSV"
                        },
                        "columns": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Column names to select"
                        },
                        "column": {
                            "type": "string",
                            "description": "Column name for filter/sort/statistics"
                        },
                        "value": {
                            "type": "string",
                            "description": "Value to filter by"
                        },
                        "delimiter": {
                            "type": "string",
                            "description": "Delimiter character (default: comma)"
                        },
                        "ascending": {
                            "type": "boolean",
                            "description": "Sort ascending (default: true)"
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

impl Default for CsvProcessorSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse CSV text into rows (vector of vectors).
fn parse_csv(csv: &str, delimiter: char) -> Vec<Vec<String>> {
    csv.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.split(delimiter)
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect()
}

/// Convert parsed CSV rows (with header) to JSON array of objects.
fn rows_to_json(rows: &[Vec<String>]) -> Value {
    if rows.is_empty() {
        return json!([]);
    }
    let headers = &rows[0];
    let objects: Vec<Value> = rows[1..]
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, header) in headers.iter().enumerate() {
                let val = row.get(i).map(std::string::String::as_str).unwrap_or("");
                obj.insert(header.clone(), Value::String(val.to_string()));
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(objects)
}

/// Convert JSON array of objects to CSV string.
fn json_to_csv(data: &[Value], delimiter: char) -> Result<String, String> {
    if data.is_empty() {
        return Ok(String::new());
    }
    let first = data[0]
        .as_object()
        .ok_or("Each JSON element must be an object")?;
    let headers: Vec<&String> = first.keys().collect();
    let mut result = headers
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(&delimiter.to_string());
    result.push('\n');

    for item in data {
        let obj = item
            .as_object()
            .ok_or("Each JSON element must be an object")?;
        let row: Vec<String> = headers
            .iter()
            .map(|h| {
                obj.get(*h)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default()
            })
            .collect();
        result.push_str(&row.join(&delimiter.to_string()));
        result.push('\n');
    }
    Ok(result.trim_end().to_string())
}

#[async_trait]
impl Skill for CsvProcessorSkill {
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

        let delimiter = call.arguments["delimiter"]
            .as_str()
            .and_then(|s| s.chars().next())
            .unwrap_or(',');

        match operation {
            "parse" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let rows = parse_csv(csv, delimiter);
                let response = json!({
                    "rows": rows,
                    "row_count": rows.len(),
                    "column_count": rows.first().map(std::vec::Vec::len).unwrap_or(0)
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "to_json" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let rows = parse_csv(csv, delimiter);
                let json_data = rows_to_json(&rows);
                let response = json!({
                    "data": json_data,
                    "record_count": if rows.len() > 1 { rows.len() - 1 } else { 0 }
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "from_json" => {
                let json_data = match call.arguments["json_data"].as_array() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'json_data' (array of objects)")),
                };
                match json_to_csv(json_data, delimiter) {
                    Ok(csv) => {
                        let response = json!({ "csv": csv });
                        Ok(ToolResult::success(&call.id, response.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "select_columns" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let columns: Vec<String> = match call.arguments["columns"].as_array() {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'columns'")),
                };
                let rows = parse_csv(csv, delimiter);
                if rows.is_empty() {
                    return Ok(ToolResult::success(&call.id, json!({"rows": []}).to_string()));
                }
                let headers = &rows[0];
                let indices: Vec<usize> = columns
                    .iter()
                    .filter_map(|c| headers.iter().position(|h| h == c))
                    .collect();
                let selected: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| indices.iter().filter_map(|&i| row.get(i).cloned()).collect())
                    .collect();
                let response = json!({
                    "rows": selected,
                    "selected_columns": columns
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "filter" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let column = match call.arguments["column"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'column'")),
                };
                let value = match call.arguments["value"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'value'")),
                };
                let rows = parse_csv(csv, delimiter);
                if rows.is_empty() {
                    return Ok(ToolResult::success(&call.id, json!({"rows": [], "match_count": 0}).to_string()));
                }
                let headers = &rows[0];
                let col_idx = match headers.iter().position(|h| h == column) {
                    Some(i) => i,
                    None => return Ok(ToolResult::error(&call.id, format!("Column '{column}' not found"))),
                };
                let mut filtered = vec![rows[0].clone()];
                for row in &rows[1..] {
                    if row.get(col_idx).map(|v| v == value).unwrap_or(false) {
                        filtered.push(row.clone());
                    }
                }
                let match_count = filtered.len() - 1;
                let response = json!({
                    "rows": filtered,
                    "match_count": match_count
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "sort" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let column = match call.arguments["column"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'column'")),
                };
                let ascending = call.arguments["ascending"].as_bool().unwrap_or(true);
                let rows = parse_csv(csv, delimiter);
                if rows.len() < 2 {
                    return Ok(ToolResult::success(&call.id, json!({"rows": rows}).to_string()));
                }
                let headers = &rows[0];
                let col_idx = match headers.iter().position(|h| h == column) {
                    Some(i) => i,
                    None => return Ok(ToolResult::error(&call.id, format!("Column '{column}' not found"))),
                };
                let mut data_rows: Vec<Vec<String>> = rows[1..].to_vec();
                data_rows.sort_by(|a, b| {
                    let va = a.get(col_idx).map(std::string::String::as_str).unwrap_or("");
                    let vb = b.get(col_idx).map(std::string::String::as_str).unwrap_or("");
                    // Try numeric comparison first
                    if let (Ok(na), Ok(nb)) = (va.parse::<f64>(), vb.parse::<f64>()) {
                        let cmp = na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal);
                        if ascending { cmp } else { cmp.reverse() }
                    } else {
                        let cmp = va.cmp(vb);
                        if ascending { cmp } else { cmp.reverse() }
                    }
                });
                let mut result = vec![rows[0].clone()];
                result.extend(data_rows);
                let response = json!({ "rows": result });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "statistics" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let column = match call.arguments["column"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'column'")),
                };
                let rows = parse_csv(csv, delimiter);
                if rows.len() < 2 {
                    return Ok(ToolResult::error(&call.id, "Not enough data rows"));
                }
                let headers = &rows[0];
                let col_idx = match headers.iter().position(|h| h == column) {
                    Some(i) => i,
                    None => return Ok(ToolResult::error(&call.id, format!("Column '{column}' not found"))),
                };
                let values: Vec<f64> = rows[1..]
                    .iter()
                    .filter_map(|row| row.get(col_idx).and_then(|v| v.parse::<f64>().ok()))
                    .collect();
                if values.is_empty() {
                    return Ok(ToolResult::error(&call.id, "No numeric values found in column"));
                }
                let count = values.len();
                let sum: f64 = values.iter().sum();
                let mean = sum / count as f64;
                let min = values.iter().copied().fold(f64::INFINITY, f64::min);
                let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let mut sorted = values.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = if count % 2 == 0 {
                    (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
                } else {
                    sorted[count / 2]
                };
                let response = json!({
                    "column": column,
                    "count": count,
                    "sum": sum,
                    "mean": mean,
                    "min": min,
                    "max": max,
                    "median": median
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "count_rows" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let rows = parse_csv(csv, delimiter);
                let data_rows = if rows.is_empty() { 0 } else { rows.len() - 1 };
                let response = json!({
                    "total_rows": rows.len(),
                    "data_rows": data_rows,
                    "has_header": !rows.is_empty()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            "headers" => {
                let csv = match call.arguments["csv"].as_str() {
                    Some(v) => v,
                    None => return Ok(ToolResult::error(&call.id, "Missing required parameter: 'csv'")),
                };
                let rows = parse_csv(csv, delimiter);
                let headers = rows.first().cloned().unwrap_or_default();
                let response = json!({
                    "headers": headers,
                    "count": headers.len()
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: parse, to_json, from_json, select_columns, filter, sort, statistics, count_rows, headers"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE_CSV: &str = "name,age,city\nAlice,30,NYC\nBob,25,LA\nCharlie,35,NYC";

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "csv_processor".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_parse() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({"operation": "parse", "csv": SAMPLE_CSV}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["row_count"], 4);
        assert_eq!(parsed["column_count"], 3);
    }

    #[tokio::test]
    async fn test_to_json() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({"operation": "to_json", "csv": SAMPLE_CSV}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["record_count"], 3);
        let data = parsed["data"].as_array().unwrap();
        assert_eq!(data[0]["name"], "Alice");
        assert_eq!(data[1]["age"], "25");
    }

    #[tokio::test]
    async fn test_from_json() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "from_json",
            "json_data": [
                {"name": "Alice", "age": "30"},
                {"name": "Bob", "age": "25"}
            ]
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let csv = parsed["csv"].as_str().unwrap();
        assert!(csv.contains("Alice"));
        assert!(csv.contains("Bob"));
    }

    #[tokio::test]
    async fn test_select_columns() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "select_columns",
            "csv": SAMPLE_CSV,
            "columns": ["name", "city"]
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows[0], json!(["name", "city"]));
        assert_eq!(rows[1], json!(["Alice", "NYC"]));
    }

    #[tokio::test]
    async fn test_filter() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "filter",
            "csv": SAMPLE_CSV,
            "column": "city",
            "value": "NYC"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["match_count"], 2);
    }

    #[tokio::test]
    async fn test_filter_no_match() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "filter",
            "csv": SAMPLE_CSV,
            "column": "city",
            "value": "Chicago"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["match_count"], 0);
    }

    #[tokio::test]
    async fn test_sort_ascending() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "sort",
            "csv": SAMPLE_CSV,
            "column": "age",
            "ascending": true
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows[1][1], "25"); // Bob first (youngest)
        assert_eq!(rows[3][1], "35"); // Charlie last (oldest)
    }

    #[tokio::test]
    async fn test_sort_descending() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "sort",
            "csv": SAMPLE_CSV,
            "column": "age",
            "ascending": false
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows[1][1], "35"); // Charlie first (oldest)
    }

    #[tokio::test]
    async fn test_statistics() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "statistics",
            "csv": SAMPLE_CSV,
            "column": "age"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 3);
        assert_eq!(parsed["min"], 25.0);
        assert_eq!(parsed["max"], 35.0);
        assert_eq!(parsed["mean"], 30.0);
        assert_eq!(parsed["median"], 30.0);
        assert_eq!(parsed["sum"], 90.0);
    }

    #[tokio::test]
    async fn test_count_rows() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({"operation": "count_rows", "csv": SAMPLE_CSV}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_rows"], 4);
        assert_eq!(parsed["data_rows"], 3);
    }

    #[tokio::test]
    async fn test_headers() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({"operation": "headers", "csv": SAMPLE_CSV}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["headers"], json!(["name", "age", "city"]));
        assert_eq!(parsed["count"], 3);
    }

    #[tokio::test]
    async fn test_custom_delimiter() {
        let skill = CsvProcessorSkill::new();
        let tsv = "name\tage\nAlice\t30";
        let call = make_call(json!({"operation": "parse", "csv": tsv, "delimiter": "\t"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["column_count"], 2);
    }

    #[tokio::test]
    async fn test_filter_column_not_found() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({
            "operation": "filter",
            "csv": SAMPLE_CSV,
            "column": "nonexistent",
            "value": "x"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({"csv": "a,b"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = CsvProcessorSkill::new();
        let call = make_call(json!({"operation": "pivot"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = CsvProcessorSkill::new();
        assert_eq!(skill.descriptor().name, "csv_processor");
    }
}

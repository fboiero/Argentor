//! Excel (XLSX) spreadsheet loader skill.
//!
//! XLSX is a ZIP of XML parts. This loader reads:
//! - `xl/workbook.xml` to list sheets
//! - `xl/sharedStrings.xml` for interned strings
//! - `xl/worksheets/sheetN.xml` for cells
//!
//! Supports listing sheets, reading a sheet as rows/cells, querying a single
//! cell, row counting, and simple CSV/JSON conversion. Formulas are not
//! evaluated — the cached value (if present) is returned.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

use crate::zip_reader::{read_central_directory, read_entry_utf8, ZipEntry};
use std::collections::HashMap;

/// Excel XLSX spreadsheet loader.
pub struct ExcelLoaderSkill {
    descriptor: SkillDescriptor,
}

impl ExcelLoaderSkill {
    /// Create a new Excel loader skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "excel_loader".to_string(),
                description: "XLSX loader: list_sheets, read_sheet, get_cell, count_rows, to_csv, to_json. Accepts base64-encoded XLSX bytes.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["list_sheets", "read_sheet", "get_cell", "count_rows", "to_csv", "to_json"],
                            "description": "The XLSX operation to perform"
                        },
                        "data": {
                            "type": "string",
                            "description": "Base64-encoded XLSX bytes"
                        },
                        "sheet": {
                            "type": "string",
                            "description": "Sheet name (for read_sheet/get_cell/count_rows/to_csv/to_json)"
                        },
                        "cell": {
                            "type": "string",
                            "description": "Cell reference like 'A1' for get_cell"
                        }
                    },
                    "required": ["operation", "data"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for ExcelLoaderSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed workbook state.
struct Workbook {
    /// Sheet name -> sheetN.xml path (relative to xl/ directory).
    sheets: Vec<(String, String)>,
    /// Shared strings by index.
    shared_strings: Vec<String>,
    /// Raw archive bytes for lazy reads.
    bytes: Vec<u8>,
    /// Central directory entries.
    entries: HashMap<String, ZipEntry>,
}

impl Workbook {
    fn load(data: &str) -> Result<Self, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("Invalid base64: {e}"))?;
        let entries = read_central_directory(&bytes)?;

        // Workbook sheets list
        let wb_entry = entries
            .get("xl/workbook.xml")
            .ok_or_else(|| "Missing xl/workbook.xml".to_string())?;
        let wb_xml = read_entry_utf8(&bytes, wb_entry)?;
        let sheets = parse_workbook_sheets(&wb_xml);

        // Shared strings (optional)
        let shared_strings = if let Some(e) = entries.get("xl/sharedStrings.xml") {
            let xml = read_entry_utf8(&bytes, e).unwrap_or_default();
            parse_shared_strings(&xml)
        } else {
            Vec::new()
        };

        Ok(Self {
            sheets,
            shared_strings,
            bytes,
            entries,
        })
    }

    /// Read a sheet's cell grid. Returns rows as vectors of strings.
    fn read_sheet(&self, sheet_name: &str) -> Result<Vec<Vec<String>>, String> {
        let sheet_idx = self
            .sheets
            .iter()
            .position(|(n, _)| n == sheet_name)
            .ok_or_else(|| format!("Sheet '{sheet_name}' not found"))?;
        let path = format!("xl/worksheets/sheet{}.xml", sheet_idx + 1);
        let entry = self
            .entries
            .get(&path)
            .ok_or_else(|| format!("Worksheet file '{path}' missing from archive"))?;
        let xml = read_entry_utf8(&self.bytes, entry)?;
        Ok(parse_sheet_xml(&xml, &self.shared_strings))
    }
}

/// Parse `<sheet name="..." ... />` entries from workbook.xml.
fn parse_workbook_sheets(xml: &str) -> Vec<(String, String)> {
    let mut sheets = Vec::new();
    if let Ok(re) = Regex::new(r#"(?is)<sheet\s[^>]*name=["']([^"']+)["'][^/>]*/?>"#) {
        for c in re.captures_iter(xml) {
            if let Some(m) = c.get(1) {
                let name = m.as_str().to_string();
                sheets.push((name.clone(), name));
            }
        }
    }
    sheets
}

/// Parse `xl/sharedStrings.xml` into a vector indexed by shared-string id.
fn parse_shared_strings(xml: &str) -> Vec<String> {
    let mut strings = Vec::new();
    // Match <si>...</si> blocks
    if let Ok(si_re) = Regex::new(r"(?is)<si[^>]*>(.*?)</si>") {
        if let Ok(t_re) = Regex::new(r"(?is)<t(?:\s[^>]*)?>(.*?)</t>") {
            for si in si_re.captures_iter(xml) {
                if let Some(inner) = si.get(1) {
                    let mut combined = String::new();
                    for t in t_re.captures_iter(inner.as_str()) {
                        if let Some(m) = t.get(1) {
                            combined.push_str(&decode_xml_entities(m.as_str()));
                        }
                    }
                    strings.push(combined);
                }
            }
        }
    }
    strings
}

/// Parse a single sheetN.xml into a grid of cell values as strings.
fn parse_sheet_xml(xml: &str, shared_strings: &[String]) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let row_re = match Regex::new(r"(?is)<row[^>]*>(.*?)</row>") {
        Ok(r) => r,
        Err(_) => return rows,
    };
    // Match each cell as a tag + content. Parse attributes manually from tag.
    let cell_re = match Regex::new(r"(?is)<c(\s[^>]*)?>(.*?)</c>") {
        Ok(r) => r,
        Err(_) => return rows,
    };
    let r_attr_re = Regex::new(r#"(?is)r=["']([A-Z]+)(\d+)["']"#);
    let t_attr_re = Regex::new(r#"(?is)\st=["']([^"']+)["']"#);
    let value_re = Regex::new(r"(?is)<v>(.*?)</v>");
    let is_text_re = Regex::new(r"(?is)<is><t(?:\s[^>]*)?>(.*?)</t></is>");

    for row_cap in row_re.captures_iter(xml) {
        let row_inner = match row_cap.get(1) {
            Some(x) => x.as_str(),
            None => continue,
        };

        let mut cells: Vec<(usize, String)> = Vec::new();
        let mut max_col: usize = 0;
        for c in cell_re.captures_iter(row_inner) {
            let attrs = c.get(1).map_or("", |m| m.as_str());
            let inner = c.get(2).map_or("", |m| m.as_str());

            let (col_letters, _row_num) = if let Ok(ref re) = r_attr_re {
                re.captures(attrs)
                    .map(|cap| {
                        (
                            cap.get(1)
                                .map_or("A".to_string(), |m| m.as_str().to_string()),
                            cap.get(2)
                                .map_or("1".to_string(), |m| m.as_str().to_string()),
                        )
                    })
                    .unwrap_or(("A".to_string(), "1".to_string()))
            } else {
                ("A".to_string(), "1".to_string())
            };
            let col_idx = column_letters_to_index(&col_letters);
            let t_type = if let Ok(ref re) = t_attr_re {
                re.captures(attrs)
                    .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let t_type = t_type.as_str();

            let raw_value = if let Ok(ref re) = value_re {
                re.captures(inner)
                    .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let value = match t_type {
                "s" => {
                    // Shared string: raw_value is the index
                    raw_value
                        .parse::<usize>()
                        .ok()
                        .and_then(|idx| shared_strings.get(idx).cloned())
                        .unwrap_or(raw_value)
                }
                "inlineStr" => {
                    if let Ok(ref re) = is_text_re {
                        re.captures(inner)
                            .and_then(|cap| cap.get(1).map(|m| decode_xml_entities(m.as_str())))
                            .unwrap_or(raw_value)
                    } else {
                        raw_value
                    }
                }
                _ => decode_xml_entities(&raw_value),
            };

            cells.push((col_idx, value));
            if col_idx > max_col {
                max_col = col_idx;
            }
        }

        let mut row = vec![String::new(); max_col + 1];
        for (idx, val) in cells {
            if idx < row.len() {
                row[idx] = val;
            }
        }
        rows.push(row);
    }

    rows
}

/// Convert Excel column letters (A, Z, AA, AB, ...) to a 0-indexed column.
fn column_letters_to_index(letters: &str) -> usize {
    let mut idx: usize = 0;
    for c in letters.chars() {
        if c.is_ascii_alphabetic() {
            idx = idx * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
        }
    }
    idx.saturating_sub(1)
}

/// Decode basic XML entities.
fn decode_xml_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse a cell reference like "B3" into (col_idx, row_number_1indexed).
fn parse_cell_ref(cell: &str) -> Option<(usize, usize)> {
    let letters: String = cell.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits: String = cell.chars().skip_while(char::is_ascii_alphabetic).collect();
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let col = column_letters_to_index(&letters);
    let row: usize = digits.parse().ok()?;
    Some((col, row))
}

#[async_trait]
impl Skill for ExcelLoaderSkill {
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
        let data = match call.arguments["data"].as_str() {
            Some(d) => d,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    "Missing required parameter: 'data'",
                ))
            }
        };

        let wb = match Workbook::load(data) {
            Ok(w) => w,
            Err(e) => return Ok(ToolResult::error(&call.id, e)),
        };

        match operation {
            "list_sheets" => {
                let names: Vec<&str> = wb.sheets.iter().map(|(n, _)| n.as_str()).collect();
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "sheets": names,
                        "count": names.len(),
                    })
                    .to_string(),
                ))
            }
            "read_sheet" => {
                let sheet = match call.arguments["sheet"].as_str() {
                    Some(s) => s,
                    None => return Ok(ToolResult::error(&call.id, "Missing 'sheet'")),
                };
                match wb.read_sheet(sheet) {
                    Ok(rows) => Ok(ToolResult::success(
                        &call.id,
                        json!({
                            "rows": rows,
                            "row_count": rows.len(),
                        })
                        .to_string(),
                    )),
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "count_rows" => {
                let sheet = match call.arguments["sheet"].as_str() {
                    Some(s) => s,
                    None => return Ok(ToolResult::error(&call.id, "Missing 'sheet'")),
                };
                match wb.read_sheet(sheet) {
                    Ok(rows) => Ok(ToolResult::success(
                        &call.id,
                        json!({ "rows": rows.len() }).to_string(),
                    )),
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "get_cell" => {
                let sheet = match call.arguments["sheet"].as_str() {
                    Some(s) => s,
                    None => return Ok(ToolResult::error(&call.id, "Missing 'sheet'")),
                };
                let cell = match call.arguments["cell"].as_str() {
                    Some(c) => c,
                    None => return Ok(ToolResult::error(&call.id, "Missing 'cell'")),
                };
                let (col, row_1) = match parse_cell_ref(cell) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            format!("Invalid cell reference: '{cell}'"),
                        ))
                    }
                };
                match wb.read_sheet(sheet) {
                    Ok(rows) => {
                        let value = rows
                            .get(row_1.saturating_sub(1))
                            .and_then(|r| r.get(col))
                            .cloned()
                            .unwrap_or_default();
                        Ok(ToolResult::success(
                            &call.id,
                            json!({
                                "cell": cell,
                                "value": value,
                                "found": !value.is_empty(),
                            })
                            .to_string(),
                        ))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "to_csv" => {
                let sheet = match call.arguments["sheet"].as_str() {
                    Some(s) => s,
                    None => return Ok(ToolResult::error(&call.id, "Missing 'sheet'")),
                };
                match wb.read_sheet(sheet) {
                    Ok(rows) => {
                        let csv: String = rows
                            .iter()
                            .map(|r| {
                                r.iter()
                                    .map(|v| {
                                        if v.contains(',') || v.contains('"') || v.contains('\n') {
                                            format!("\"{}\"", v.replace('"', "\"\""))
                                        } else {
                                            v.clone()
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(ToolResult::success(
                            &call.id,
                            json!({ "csv": csv, "rows": rows.len() }).to_string(),
                        ))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            "to_json" => {
                let sheet = match call.arguments["sheet"].as_str() {
                    Some(s) => s,
                    None => return Ok(ToolResult::error(&call.id, "Missing 'sheet'")),
                };
                match wb.read_sheet(sheet) {
                    Ok(rows) => {
                        // Treat first row as headers
                        let json_rows: Vec<Value> = if rows.len() > 1 {
                            let headers = &rows[0];
                            rows[1..]
                                .iter()
                                .map(|row| {
                                    let mut obj = serde_json::Map::new();
                                    for (i, h) in headers.iter().enumerate() {
                                        let val = row.get(i).cloned().unwrap_or_default();
                                        obj.insert(h.clone(), Value::String(val));
                                    }
                                    Value::Object(obj)
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };
                        Ok(ToolResult::success(
                            &call.id,
                            json!({ "rows": json_rows, "count": json_rows.len() }).to_string(),
                        ))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, e)),
                }
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: list_sheets, read_sheet, get_cell, count_rows, to_csv, to_json"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::zip_reader::build_stored_zip_for_tests;

    fn build_sample_xlsx() -> Vec<u8> {
        let workbook = r#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheets>
<sheet name="Data" sheetId="1" r:id="rId1"/>
<sheet name="Summary" sheetId="2" r:id="rId2"/>
</sheets>
</workbook>"#;

        let shared_strings = r#"<?xml version="1.0"?>
<sst count="3" uniqueCount="3">
<si><t>Name</t></si>
<si><t>Age</t></si>
<si><t>Alice</t></si>
</sst>"#;

        // Sheet1: A1=Name(s=0), B1=Age(s=1), A2=Alice(s=2), B2=30
        let sheet1 = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>30</v></c></row>
</sheetData>
</worksheet>"#;

        let sheet2 = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1"><v>42</v></c></row>
</sheetData>
</worksheet>"#;

        build_stored_zip_for_tests(&[
            ("xl/workbook.xml", workbook.as_bytes()),
            ("xl/sharedStrings.xml", shared_strings.as_bytes()),
            ("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
            ("xl/worksheets/sheet2.xml", sheet2.as_bytes()),
        ])
    }

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "excel_loader".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_list_sheets() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({"operation": "list_sheets", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
        let sheets = parsed["sheets"].as_array().unwrap();
        assert_eq!(sheets[0], "Data");
        assert_eq!(sheets[1], "Summary");
    }

    #[tokio::test]
    async fn test_read_sheet() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({"operation": "read_sheet", "data": encoded, "sheet": "Data"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["row_count"], 2);
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows[0][0], "Name");
        assert_eq!(rows[0][1], "Age");
        assert_eq!(rows[1][0], "Alice");
        assert_eq!(rows[1][1], "30");
    }

    #[tokio::test]
    async fn test_get_cell() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "get_cell",
            "data": encoded,
            "sheet": "Data",
            "cell": "A2"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["value"], "Alice");
        assert_eq!(parsed["found"], true);
    }

    #[tokio::test]
    async fn test_get_cell_empty() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "get_cell",
            "data": encoded,
            "sheet": "Data",
            "cell": "Z99"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["found"], false);
    }

    #[tokio::test]
    async fn test_count_rows() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "count_rows",
            "data": encoded,
            "sheet": "Data"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["rows"], 2);
    }

    #[tokio::test]
    async fn test_to_csv() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "to_csv",
            "data": encoded,
            "sheet": "Data"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let csv = parsed["csv"].as_str().unwrap();
        assert!(csv.contains("Name,Age"));
        assert!(csv.contains("Alice,30"));
    }

    #[tokio::test]
    async fn test_to_json() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "to_json",
            "data": encoded,
            "sheet": "Data"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows[0]["Name"], "Alice");
        assert_eq!(rows[0]["Age"], "30");
    }

    #[tokio::test]
    async fn test_sheet_not_found() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "read_sheet",
            "data": encoded,
            "sheet": "Nonexistent"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_invalid_cell_reference() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({
            "operation": "get_cell",
            "data": encoded,
            "sheet": "Data",
            "cell": "invalid!"
        }));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_invalid_base64() {
        let skill = ExcelLoaderSkill::new();
        let call = make_call(json!({"operation": "list_sheets", "data": "!!!"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({"data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_sheet_param() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({"operation": "read_sheet", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = ExcelLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_xlsx());
        let call = make_call(json!({"operation": "pivot_table", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_column_letters_to_index() {
        assert_eq!(column_letters_to_index("A"), 0);
        assert_eq!(column_letters_to_index("B"), 1);
        assert_eq!(column_letters_to_index("Z"), 25);
        assert_eq!(column_letters_to_index("AA"), 26);
        assert_eq!(column_letters_to_index("AB"), 27);
    }

    #[test]
    fn test_parse_cell_ref() {
        assert_eq!(parse_cell_ref("A1"), Some((0, 1)));
        assert_eq!(parse_cell_ref("B3"), Some((1, 3)));
        assert_eq!(parse_cell_ref("AA100"), Some((26, 100)));
        assert_eq!(parse_cell_ref(""), None);
        assert_eq!(parse_cell_ref("A"), None);
        assert_eq!(parse_cell_ref("123"), None);
    }

    #[test]
    fn test_descriptor_name() {
        let skill = ExcelLoaderSkill::new();
        assert_eq!(skill.descriptor().name, "excel_loader");
    }
}

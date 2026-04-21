//! DOCX (Microsoft Word) document loader skill.
//!
//! DOCX files are ZIP archives containing `word/document.xml` with the document
//! body. This loader uses the internal `zip_reader` module (no external deps)
//! and a lightweight XML-to-text stripper.
//!
//! For convenience, the loader also supports "raw XML mode" where the caller
//! provides XML content directly (useful when the archive was already unpacked).

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::Engine;
use regex::Regex;
use serde_json::json;
#[cfg(test)]
use serde_json::Value;

use crate::zip_reader::{read_central_directory, read_entry_utf8};

/// DOCX document loader: text, paragraphs, tables, word count.
pub struct DocxLoaderSkill {
    descriptor: SkillDescriptor,
}

impl DocxLoaderSkill {
    /// Create a new DOCX loader skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "docx_loader".to_string(),
                description: "DOCX loader: extract_text, extract_paragraphs, extract_tables, count_words. Accepts base64-encoded DOCX bytes or raw document.xml.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["extract_text", "extract_paragraphs", "extract_tables", "count_words"],
                            "description": "The DOCX operation to perform"
                        },
                        "data": {
                            "type": "string",
                            "description": "DOCX content: base64-encoded ZIP bytes, or raw document.xml if encoding='xml'"
                        },
                        "encoding": {
                            "type": "string",
                            "enum": ["base64", "xml"],
                            "description": "Encoding of 'data'. Default: base64."
                        }
                    },
                    "required": ["operation", "data"]
                }),
                required_capabilities: vec![],
            },
        }
    }
}

impl Default for DocxLoaderSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Retrieve the `word/document.xml` contents from a base64-encoded DOCX.
fn load_document_xml(data: &str, encoding: Option<&str>) -> Result<String, String> {
    match encoding {
        Some("xml") => Ok(data.to_string()),
        _ => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| format!("Invalid base64: {e}"))?;
            let entries = read_central_directory(&bytes)?;
            let entry = entries
                .get("word/document.xml")
                .ok_or_else(|| "Missing word/document.xml in archive".to_string())?;
            read_entry_utf8(&bytes, entry)
        }
    }
}

/// Extract paragraph text from a word/document.xml string.
/// Each `<w:p>` becomes one string. `<w:t>` nodes contribute text.
pub fn extract_paragraphs_from_xml(xml: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let p_re = match Regex::new(r"(?is)<w:p\b[^>]*>(.*?)</w:p>") {
        Ok(r) => r,
        Err(_) => return paragraphs,
    };
    let t_re = match Regex::new(r"(?is)<w:t(?:\s[^>]*)?>(.*?)</w:t>") {
        Ok(r) => r,
        Err(_) => return paragraphs,
    };

    for p_cap in p_re.captures_iter(xml) {
        if let Some(p_inner) = p_cap.get(1) {
            let mut text = String::new();
            for t_cap in t_re.captures_iter(p_inner.as_str()) {
                if let Some(t) = t_cap.get(1) {
                    text.push_str(t.as_str());
                }
            }
            if !text.is_empty() {
                paragraphs.push(decode_xml_entities(&text));
            }
        }
    }

    paragraphs
}

/// Extract tables as rows of cells.
pub fn extract_tables_from_xml(xml: &str) -> Vec<Vec<Vec<String>>> {
    let mut tables = Vec::new();
    let tbl_re = match Regex::new(r"(?is)<w:tbl\b[^>]*>(.*?)</w:tbl>") {
        Ok(r) => r,
        Err(_) => return tables,
    };
    let tr_re = match Regex::new(r"(?is)<w:tr\b[^>]*>(.*?)</w:tr>") {
        Ok(r) => r,
        Err(_) => return tables,
    };
    let tc_re = match Regex::new(r"(?is)<w:tc\b[^>]*>(.*?)</w:tc>") {
        Ok(r) => r,
        Err(_) => return tables,
    };
    let t_re = match Regex::new(r"(?is)<w:t(?:\s[^>]*)?>(.*?)</w:t>") {
        Ok(r) => r,
        Err(_) => return tables,
    };

    for tbl_cap in tbl_re.captures_iter(xml) {
        if let Some(tbl_inner) = tbl_cap.get(1) {
            let mut rows: Vec<Vec<String>> = Vec::new();
            for tr_cap in tr_re.captures_iter(tbl_inner.as_str()) {
                if let Some(tr_inner) = tr_cap.get(1) {
                    let mut cells: Vec<String> = Vec::new();
                    for tc_cap in tc_re.captures_iter(tr_inner.as_str()) {
                        if let Some(tc_inner) = tc_cap.get(1) {
                            let mut cell_text = String::new();
                            for t_cap in t_re.captures_iter(tc_inner.as_str()) {
                                if let Some(t) = t_cap.get(1) {
                                    cell_text.push_str(t.as_str());
                                }
                            }
                            cells.push(decode_xml_entities(&cell_text));
                        }
                    }
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                }
            }
            if !rows.is_empty() {
                tables.push(rows);
            }
        }
    }

    tables
}

/// Decode basic XML entities.
fn decode_xml_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[async_trait]
impl Skill for DocxLoaderSkill {
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
        let encoding = call.arguments["encoding"].as_str();

        let xml = match load_document_xml(data, encoding) {
            Ok(x) => x,
            Err(e) => return Ok(ToolResult::error(&call.id, e)),
        };

        match operation {
            "extract_text" => {
                let paragraphs = extract_paragraphs_from_xml(&xml);
                let text = paragraphs.join("\n");
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "text": text,
                        "length": text.len(),
                        "paragraph_count": paragraphs.len(),
                    })
                    .to_string(),
                ))
            }
            "extract_paragraphs" => {
                let paragraphs = extract_paragraphs_from_xml(&xml);
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "paragraphs": paragraphs,
                        "count": paragraphs.len(),
                    })
                    .to_string(),
                ))
            }
            "extract_tables" => {
                let tables = extract_tables_from_xml(&xml);
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "tables": tables,
                        "count": tables.len(),
                    })
                    .to_string(),
                ))
            }
            "count_words" => {
                let paragraphs = extract_paragraphs_from_xml(&xml);
                let full = paragraphs.join(" ");
                let words = full.split_whitespace().count();
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "words": words,
                        "characters": full.len(),
                        "paragraphs": paragraphs.len(),
                    })
                    .to_string(),
                ))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: extract_text, extract_paragraphs, extract_tables, count_words"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::zip_reader::build_stored_zip_for_tests;

    const SAMPLE_XML: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="x">
<w:body>
<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Second paragraph</w:t></w:r></w:p>
<w:tbl>
  <w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr>
  <w:tr><w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc></w:tr>
</w:tbl>
</w:body>
</w:document>"#;

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "docx_loader".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_extract_text_xml_mode() {
        let skill = DocxLoaderSkill::new();
        let call =
            make_call(json!({"operation": "extract_text", "data": SAMPLE_XML, "encoding": "xml"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Hello World"));
        assert!(text.contains("Second paragraph"));
    }

    #[tokio::test]
    async fn test_extract_paragraphs() {
        let skill = DocxLoaderSkill::new();
        let call = make_call(
            json!({"operation": "extract_paragraphs", "data": SAMPLE_XML, "encoding": "xml"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        // Non-empty <w:p> paragraphs outside tables + table paragraphs are all counted.
        // Our regex extracts ALL <w:p> including those inside tables.
        let count = parsed["count"].as_u64().unwrap();
        assert!(count >= 2);
    }

    #[tokio::test]
    async fn test_extract_tables() {
        let skill = DocxLoaderSkill::new();
        let call = make_call(
            json!({"operation": "extract_tables", "data": SAMPLE_XML, "encoding": "xml"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        let tables = parsed["tables"].as_array().unwrap();
        let first_table = tables[0].as_array().unwrap();
        assert_eq!(first_table.len(), 2); // 2 rows
        let row0 = first_table[0].as_array().unwrap();
        assert_eq!(row0.len(), 2); // 2 cells
        assert_eq!(row0[0], "A1");
        assert_eq!(row0[1], "B1");
    }

    #[tokio::test]
    async fn test_count_words() {
        let skill = DocxLoaderSkill::new();
        let call =
            make_call(json!({"operation": "count_words", "data": SAMPLE_XML, "encoding": "xml"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let words = parsed["words"].as_u64().unwrap();
        assert!(words >= 4, "Expected at least 4 words, got {words}");
    }

    #[tokio::test]
    async fn test_extract_from_zip() {
        let skill = DocxLoaderSkill::new();
        let zip_bytes = build_stored_zip_for_tests(&[
            ("word/document.xml", SAMPLE_XML.as_bytes()),
            ("[Content_Types].xml", b"<x/>"),
        ]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&zip_bytes);
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Hello World"));
    }

    #[tokio::test]
    async fn test_missing_document_xml() {
        let skill = DocxLoaderSkill::new();
        let zip_bytes = build_stored_zip_for_tests(&[("other.xml", b"<x/>")]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&zip_bytes);
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("document.xml"));
    }

    #[tokio::test]
    async fn test_invalid_base64() {
        let skill = DocxLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text", "data": "!!!invalid!!!"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_invalid_zip() {
        let skill = DocxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"not a zip archive at all");
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_empty_document() {
        let skill = DocxLoaderSkill::new();
        let xml = "<w:document><w:body></w:body></w:document>";
        let call = make_call(json!({"operation": "extract_text", "data": xml, "encoding": "xml"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["text"], "");
        assert_eq!(parsed["paragraph_count"], 0);
    }

    #[tokio::test]
    async fn test_decodes_entities() {
        let skill = DocxLoaderSkill::new();
        let xml = "<w:p><w:r><w:t>Tom &amp; Jerry</w:t></w:r></w:p>";
        let call = make_call(json!({"operation": "extract_text", "data": xml, "encoding": "xml"}));
        let result = skill.execute(call).await.unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["text"].as_str().unwrap().contains("Tom & Jerry"));
    }

    #[tokio::test]
    async fn test_no_tables() {
        let skill = DocxLoaderSkill::new();
        let xml = "<w:p><w:r><w:t>No tables here</w:t></w:r></w:p>";
        let call =
            make_call(json!({"operation": "extract_tables", "data": xml, "encoding": "xml"}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = DocxLoaderSkill::new();
        let call = make_call(json!({"data": SAMPLE_XML, "encoding": "xml"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_data() {
        let skill = DocxLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = DocxLoaderSkill::new();
        let call = make_call(
            json!({"operation": "convert_to_pdf", "data": SAMPLE_XML, "encoding": "xml"}),
        );
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[test]
    fn test_descriptor_name() {
        let skill = DocxLoaderSkill::new();
        assert_eq!(skill.descriptor().name, "docx_loader");
    }

    #[test]
    fn test_paragraphs_order_preserved() {
        let xml = r#"<w:p><w:r><w:t>First</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p><w:p><w:r><w:t>Third</w:t></w:r></w:p>"#;
        let paragraphs = extract_paragraphs_from_xml(xml);
        assert_eq!(paragraphs, vec!["First", "Second", "Third"]);
    }
}

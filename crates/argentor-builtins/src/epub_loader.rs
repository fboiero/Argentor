//! EPUB ebook loader skill.
//!
//! EPUB is a ZIP archive of XHTML files + an OPF manifest. This loader:
//! - reads the container to locate the OPF package file
//! - extracts metadata (title, author, language) from the OPF
//! - extracts chapter text by stripping XHTML from each spine item
//!
//! Zero external deps — uses the internal `zip_reader` and regex for XML scan.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

use crate::web_scraper::strip_html_tags;
use crate::zip_reader::{read_central_directory, read_entry_utf8, ZipEntry};
use std::collections::HashMap;

/// EPUB ebook loader skill.
pub struct EpubLoaderSkill {
    descriptor: SkillDescriptor,
}

impl EpubLoaderSkill {
    /// Create a new EPUB loader skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "epub_loader".to_string(),
                description: "EPUB ebook loader: extract_chapters, extract_text, extract_metadata (title, author, language). Accepts base64-encoded EPUB bytes.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["extract_chapters", "extract_text", "extract_metadata"],
                            "description": "The EPUB operation to perform"
                        },
                        "data": {
                            "type": "string",
                            "description": "Base64-encoded EPUB bytes"
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

impl Default for EpubLoaderSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the OPF file path by reading `META-INF/container.xml`.
fn find_opf_path(entries: &HashMap<String, ZipEntry>, bytes: &[u8]) -> Option<String> {
    let container_entry = entries.get("META-INF/container.xml")?;
    let xml = read_entry_utf8(bytes, container_entry).ok()?;
    let re = Regex::new(r#"(?is)<rootfile\s[^>]*full-path=["']([^"']+)["']"#).ok()?;
    re.captures(&xml)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// Parse the OPF XML: returns (metadata, spine_hrefs).
fn parse_opf(opf: &str, opf_dir: &str) -> (Value, Vec<String>) {
    let mut meta = json!({});

    if let Ok(re) = Regex::new(r"(?is)<dc:title[^>]*>(.*?)</dc:title>") {
        if let Some(c) = re.captures(opf) {
            if let Some(m) = c.get(1) {
                meta["title"] = Value::String(strip_html_tags(m.as_str()));
            }
        }
    }

    if let Ok(re) = Regex::new(r"(?is)<dc:creator[^>]*>(.*?)</dc:creator>") {
        let authors: Vec<String> = re
            .captures_iter(opf)
            .filter_map(|c| c.get(1).map(|m| strip_html_tags(m.as_str())))
            .collect();
        if !authors.is_empty() {
            meta["authors"] = Value::Array(authors.into_iter().map(Value::String).collect());
        }
    }

    if let Ok(re) = Regex::new(r"(?is)<dc:language[^>]*>(.*?)</dc:language>") {
        if let Some(c) = re.captures(opf) {
            if let Some(m) = c.get(1) {
                meta["language"] = Value::String(strip_html_tags(m.as_str()));
            }
        }
    }

    if let Ok(re) = Regex::new(r"(?is)<dc:publisher[^>]*>(.*?)</dc:publisher>") {
        if let Some(c) = re.captures(opf) {
            if let Some(m) = c.get(1) {
                meta["publisher"] = Value::String(strip_html_tags(m.as_str()));
            }
        }
    }

    if let Ok(re) = Regex::new(r"(?is)<dc:date[^>]*>(.*?)</dc:date>") {
        if let Some(c) = re.captures(opf) {
            if let Some(m) = c.get(1) {
                meta["date"] = Value::String(strip_html_tags(m.as_str()));
            }
        }
    }

    // Build id -> href map from <manifest>
    let mut id_to_href: HashMap<String, String> = HashMap::new();
    if let Ok(re) = Regex::new(r#"(?is)<item\s[^>]*id=["']([^"']+)["'][^>]*href=["']([^"']+)["']"#)
    {
        for c in re.captures_iter(opf) {
            if let (Some(id), Some(href)) = (c.get(1), c.get(2)) {
                id_to_href.insert(id.as_str().to_string(), href.as_str().to_string());
            }
        }
    }
    // Same but with href before id
    if let Ok(re) = Regex::new(r#"(?is)<item\s[^>]*href=["']([^"']+)["'][^>]*id=["']([^"']+)["']"#)
    {
        for c in re.captures_iter(opf) {
            if let (Some(href), Some(id)) = (c.get(1), c.get(2)) {
                id_to_href
                    .entry(id.as_str().to_string())
                    .or_insert_with(|| href.as_str().to_string());
            }
        }
    }

    // Extract spine idrefs (in order)
    let mut spine_ids = Vec::new();
    if let Ok(re) = Regex::new(r#"(?is)<itemref\s[^>]*idref=["']([^"']+)["']"#) {
        for c in re.captures_iter(opf) {
            if let Some(id) = c.get(1) {
                spine_ids.push(id.as_str().to_string());
            }
        }
    }

    let spine_hrefs: Vec<String> = spine_ids
        .into_iter()
        .filter_map(|id| id_to_href.get(&id).cloned())
        .map(|href| {
            if opf_dir.is_empty() {
                href
            } else {
                format!("{opf_dir}/{href}")
            }
        })
        .collect();

    (meta, spine_hrefs)
}

/// Decode base64 EPUB and return (metadata, spine chapters).
fn load_epub(data: &str) -> Result<(Value, Vec<(String, String)>), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("Invalid base64: {e}"))?;
    let entries = read_central_directory(&bytes)?;

    let opf_path = find_opf_path(&entries, &bytes)
        .ok_or_else(|| "Cannot locate OPF (container.xml missing or invalid)".to_string())?;
    let opf_entry = entries
        .get(&opf_path)
        .ok_or_else(|| format!("OPF file '{opf_path}' not found in archive"))?;
    let opf_xml = read_entry_utf8(&bytes, opf_entry)?;

    let opf_dir = opf_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();

    let (meta, spine_hrefs) = parse_opf(&opf_xml, &opf_dir);

    let mut chapters: Vec<(String, String)> = Vec::new();
    for href in spine_hrefs {
        if let Some(entry) = entries.get(&href) {
            if let Ok(xhtml) = read_entry_utf8(&bytes, entry) {
                let text = strip_html_tags(&xhtml);
                chapters.push((href, text));
            }
        }
    }

    Ok((meta, chapters))
}

#[async_trait]
impl Skill for EpubLoaderSkill {
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

        let (meta, chapters) = match load_epub(data) {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::error(&call.id, e)),
        };

        match operation {
            "extract_metadata" => {
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "metadata": meta,
                        "chapter_count": chapters.len(),
                    })
                    .to_string(),
                ))
            }
            "extract_chapters" => {
                let chapter_list: Vec<Value> = chapters
                    .iter()
                    .map(|(href, text)| json!({"href": href, "text": text}))
                    .collect();
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "chapters": chapter_list,
                        "count": chapter_list.len(),
                    })
                    .to_string(),
                ))
            }
            "extract_text" => {
                let text: String = chapters
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "text": text,
                        "length": text.len(),
                        "chapter_count": chapters.len(),
                    })
                    .to_string(),
                ))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: extract_chapters, extract_text, extract_metadata"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::zip_reader::build_stored_zip_for_tests;

    fn build_sample_epub() -> Vec<u8> {
        let container = r#"<?xml version="1.0"?>
<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let opf = r#"<?xml version="1.0"?>
<package xmlns:dc="http://purl.org/dc/elements/1.1/">
  <metadata>
    <dc:title>Sample Book</dc:title>
    <dc:creator>Author One</dc:creator>
    <dc:language>en</dc:language>
    <dc:publisher>Test Press</dc:publisher>
    <dc:date>2024-01-01</dc:date>
  </metadata>
  <manifest>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
</package>"#;
        let ch1 = "<html><body><h1>Chapter 1</h1><p>First chapter text.</p></body></html>";
        let ch2 = "<html><body><h1>Chapter 2</h1><p>Second chapter text.</p></body></html>";

        build_stored_zip_for_tests(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container.as_bytes()),
            ("OEBPS/content.opf", opf.as_bytes()),
            ("OEBPS/ch1.xhtml", ch1.as_bytes()),
            ("OEBPS/ch2.xhtml", ch2.as_bytes()),
        ])
    }

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "epub_loader".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_extract_metadata() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"operation": "extract_metadata", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let meta = &parsed["metadata"];
        assert_eq!(meta["title"], "Sample Book");
        assert_eq!(meta["language"], "en");
        assert_eq!(meta["publisher"], "Test Press");
        assert_eq!(meta["date"], "2024-01-01");
        let authors = meta["authors"].as_array().unwrap();
        assert_eq!(authors[0], "Author One");
    }

    #[tokio::test]
    async fn test_extract_chapters() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"operation": "extract_chapters", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
        let chapters = parsed["chapters"].as_array().unwrap();
        assert_eq!(chapters[0]["href"], "OEBPS/ch1.xhtml");
        assert!(chapters[0]["text"]
            .as_str()
            .unwrap()
            .contains("First chapter"));
    }

    #[tokio::test]
    async fn test_extract_text() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("First chapter"));
        assert!(text.contains("Second chapter"));
        assert_eq!(parsed["chapter_count"], 2);
    }

    #[tokio::test]
    async fn test_chapters_ordered_by_spine() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"operation": "extract_chapters", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let chapters = parsed["chapters"].as_array().unwrap();
        assert_eq!(chapters[0]["href"], "OEBPS/ch1.xhtml");
        assert_eq!(chapters[1]["href"], "OEBPS/ch2.xhtml");
    }

    #[tokio::test]
    async fn test_missing_container() {
        let skill = EpubLoaderSkill::new();
        let zip = build_stored_zip_for_tests(&[("other.xml", b"<x/>")]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&zip);
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_invalid_base64() {
        let skill = EpubLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text", "data": "!!!"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_not_a_zip() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"just plain text content");
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_data() {
        let skill = EpubLoaderSkill::new();
        let call = make_call(json!({"operation": "extract_text"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"operation": "convert_to_pdf", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_metadata_includes_chapter_count() {
        let skill = EpubLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_epub());
        let call = make_call(json!({"operation": "extract_metadata", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["chapter_count"], 2);
    }

    #[test]
    fn test_descriptor_name() {
        let skill = EpubLoaderSkill::new();
        assert_eq!(skill.descriptor().name, "epub_loader");
    }

    #[test]
    fn test_parse_opf_standalone() {
        let opf = r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/">
<metadata><dc:title>T</dc:title><dc:creator>A</dc:creator></metadata>
<manifest><item id="a" href="a.xhtml" media-type="x"/></manifest>
<spine><itemref idref="a"/></spine>
</package>"#;
        let (meta, hrefs) = parse_opf(opf, "OEBPS");
        assert_eq!(meta["title"], "T");
        assert_eq!(hrefs, vec!["OEBPS/a.xhtml"]);
    }

    #[test]
    fn test_parse_opf_no_opf_dir() {
        let opf = r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/">
<metadata><dc:title>T</dc:title></metadata>
<manifest><item id="a" href="a.xhtml" media-type="x"/></manifest>
<spine><itemref idref="a"/></spine>
</package>"#;
        let (_meta, hrefs) = parse_opf(opf, "");
        assert_eq!(hrefs, vec!["a.xhtml"]);
    }
}

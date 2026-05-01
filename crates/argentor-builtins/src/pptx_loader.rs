//! PowerPoint (PPTX) presentation loader skill.
//!
//! PPTX is a ZIP archive. Slides live at `ppt/slides/slide{N}.xml` and speaker
//! notes at `ppt/notesSlides/notesSlide{N}.xml`. Each slide contains `<a:t>`
//! text nodes inside `<p:sp>` shape elements.
//!
//! Supports text extraction, slide-by-slide listing, counting, and speaker
//! notes extraction. Zero external deps.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

use crate::zip_reader::{read_central_directory, read_entry_utf8, ZipEntry};
use std::collections::HashMap;

/// PPTX presentation loader skill.
pub struct PptxLoaderSkill {
    descriptor: SkillDescriptor,
}

impl PptxLoaderSkill {
    /// Create a new PPTX loader skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "pptx_loader".to_string(),
                description: "PPTX loader: extract_text, extract_slides, count_slides, extract_speaker_notes. Accepts base64-encoded PPTX bytes.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["extract_text", "extract_slides", "count_slides", "extract_speaker_notes"],
                            "description": "The PPTX operation to perform"
                        },
                        "data": {
                            "type": "string",
                            "description": "Base64-encoded PPTX bytes"
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

impl Default for PptxLoaderSkill {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract all `<a:t>...</a:t>` text nodes from a slide XML.
fn extract_slide_text(xml: &str) -> String {
    let re = match Regex::new(r"(?is)<a:t(?:\s[^>]*)?>(.*?)</a:t>") {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    let mut pieces = Vec::new();
    for c in re.captures_iter(xml) {
        if let Some(m) = c.get(1) {
            pieces.push(decode_xml_entities(m.as_str()));
        }
    }
    pieces.join(" ")
}

/// Decode basic XML entities.
fn decode_xml_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Collect slide paths from the archive, sorted by slide number.
fn collect_slide_paths(entries: &HashMap<String, ZipEntry>) -> Vec<String> {
    let mut paths: Vec<(usize, String)> = entries
        .keys()
        .filter_map(|k| {
            if let Some(rest) = k.strip_prefix("ppt/slides/slide") {
                if let Some(num_str) = rest.strip_suffix(".xml") {
                    if let Ok(n) = num_str.parse::<usize>() {
                        return Some((n, k.clone()));
                    }
                }
            }
            None
        })
        .collect();
    paths.sort_by_key(|(n, _)| *n);
    paths.into_iter().map(|(_, p)| p).collect()
}

/// Collect notes slide paths, sorted by number.
fn collect_notes_paths(entries: &HashMap<String, ZipEntry>) -> Vec<String> {
    let mut paths: Vec<(usize, String)> = entries
        .keys()
        .filter_map(|k| {
            if let Some(rest) = k.strip_prefix("ppt/notesSlides/notesSlide") {
                if let Some(num_str) = rest.strip_suffix(".xml") {
                    if let Ok(n) = num_str.parse::<usize>() {
                        return Some((n, k.clone()));
                    }
                }
            }
            None
        })
        .collect();
    paths.sort_by_key(|(n, _)| *n);
    paths.into_iter().map(|(_, p)| p).collect()
}

/// Load slides and notes. Returns (slide_texts, notes_texts).
fn load_pptx(data: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("Invalid base64: {e}"))?;
    let entries = read_central_directory(&bytes)?;

    let slide_paths = collect_slide_paths(&entries);
    if slide_paths.is_empty() {
        return Err("No slides found in archive (expected ppt/slides/slideN.xml)".to_string());
    }

    let mut slides = Vec::new();
    for path in &slide_paths {
        if let Some(entry) = entries.get(path) {
            if let Ok(xml) = read_entry_utf8(&bytes, entry) {
                slides.push(extract_slide_text(&xml));
            } else {
                slides.push(String::new());
            }
        }
    }

    let notes_paths = collect_notes_paths(&entries);
    let mut notes = Vec::new();
    for path in &notes_paths {
        if let Some(entry) = entries.get(path) {
            if let Ok(xml) = read_entry_utf8(&bytes, entry) {
                notes.push(extract_slide_text(&xml));
            } else {
                notes.push(String::new());
            }
        }
    }

    Ok((slides, notes))
}

#[async_trait]
impl Skill for PptxLoaderSkill {
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

        let (slides, notes) = match load_pptx(data) {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::error(&call.id, e)),
        };

        match operation {
            "count_slides" => Ok(ToolResult::success(
                &call.id,
                json!({ "slides": slides.len() }).to_string(),
            )),
            "extract_text" => {
                let full = slides.join("\n\n");
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "text": full,
                        "length": full.len(),
                        "slide_count": slides.len(),
                    })
                    .to_string(),
                ))
            }
            "extract_slides" => {
                let slide_list: Vec<Value> = slides
                    .iter()
                    .enumerate()
                    .map(|(i, t)| json!({ "index": i + 1, "text": t }))
                    .collect();
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "slides": slide_list,
                        "count": slide_list.len(),
                    })
                    .to_string(),
                ))
            }
            "extract_speaker_notes" => {
                let note_list: Vec<Value> = notes
                    .iter()
                    .enumerate()
                    .map(|(i, t)| json!({ "slide": i + 1, "notes": t }))
                    .collect();
                Ok(ToolResult::success(
                    &call.id,
                    json!({
                        "notes": note_list,
                        "count": note_list.len(),
                    })
                    .to_string(),
                ))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!("Unknown operation: '{operation}'. Supported: extract_text, extract_slides, count_slides, extract_speaker_notes"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::zip_reader::build_stored_zip_for_tests;

    fn slide_xml(text: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld>
</p:sld>"#
        )
    }

    fn notes_xml(text: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
         xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld>
</p:notes>"#
        )
    }

    fn build_sample_pptx() -> Vec<u8> {
        let s1 = slide_xml("Title Slide");
        let s2 = slide_xml("Second Slide Body");
        let s3 = slide_xml("Conclusion");
        let n1 = notes_xml("Remember to smile");
        let n2 = notes_xml("Pace yourself");

        build_stored_zip_for_tests(&[
            ("ppt/slides/slide1.xml", s1.as_bytes()),
            ("ppt/slides/slide2.xml", s2.as_bytes()),
            ("ppt/slides/slide3.xml", s3.as_bytes()),
            ("ppt/notesSlides/notesSlide1.xml", n1.as_bytes()),
            ("ppt/notesSlides/notesSlide2.xml", n2.as_bytes()),
        ])
    }

    fn make_call(args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: "pptx_loader".to_string(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn test_count_slides() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_pptx());
        let call = make_call(json!({"operation": "count_slides", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["slides"], 3);
    }

    #[tokio::test]
    async fn test_extract_text() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_pptx());
        let call = make_call(json!({"operation": "extract_text", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("Title Slide"));
        assert!(text.contains("Second Slide Body"));
        assert!(text.contains("Conclusion"));
        assert_eq!(parsed["slide_count"], 3);
    }

    #[tokio::test]
    async fn test_extract_slides() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_pptx());
        let call = make_call(json!({"operation": "extract_slides", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 3);
        let slides = parsed["slides"].as_array().unwrap();
        assert_eq!(slides[0]["index"], 1);
        assert!(slides[0]["text"].as_str().unwrap().contains("Title Slide"));
        assert_eq!(slides[2]["index"], 3);
    }

    #[tokio::test]
    async fn test_extract_speaker_notes() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_pptx());
        let call = make_call(json!({"operation": "extract_speaker_notes", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 2);
        let notes = parsed["notes"].as_array().unwrap();
        assert!(notes[0]["notes"]
            .as_str()
            .unwrap()
            .contains("Remember to smile"));
        assert!(notes[1]["notes"]
            .as_str()
            .unwrap()
            .contains("Pace yourself"));
    }

    #[tokio::test]
    async fn test_no_slides_fails() {
        let skill = PptxLoaderSkill::new();
        let zip = build_stored_zip_for_tests(&[("other.xml", b"<x/>")]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&zip);
        let call = make_call(json!({"operation": "count_slides", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("No slides"));
    }

    #[tokio::test]
    async fn test_invalid_base64() {
        let skill = PptxLoaderSkill::new();
        let call = make_call(json!({"operation": "count_slides", "data": "!!!"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_not_a_zip() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"plain text");
        let call = make_call(json!({"operation": "count_slides", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_pptx());
        let call = make_call(json!({"data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_missing_data() {
        let skill = PptxLoaderSkill::new();
        let call = make_call(json!({"operation": "count_slides"}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = PptxLoaderSkill::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(build_sample_pptx());
        let call = make_call(json!({"operation": "render_png", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_slide_ordering() {
        let skill = PptxLoaderSkill::new();
        // Build with slides in non-sequential order — loader must sort numerically
        let s1 = slide_xml("A");
        let s10 = slide_xml("J");
        let s2 = slide_xml("B");
        let zip = build_stored_zip_for_tests(&[
            ("ppt/slides/slide10.xml", s10.as_bytes()),
            ("ppt/slides/slide1.xml", s1.as_bytes()),
            ("ppt/slides/slide2.xml", s2.as_bytes()),
        ]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&zip);
        let call = make_call(json!({"operation": "extract_slides", "data": encoded}));
        let result = skill.execute(call).await.unwrap();
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let slides = parsed["slides"].as_array().unwrap();
        assert_eq!(slides.len(), 3);
        // First slide (index 1) should be "A" — slide1, not slide10
        assert!(slides[0]["text"].as_str().unwrap().contains('A'));
        assert!(slides[1]["text"].as_str().unwrap().contains('B'));
        assert!(slides[2]["text"].as_str().unwrap().contains('J'));
    }

    #[test]
    fn test_extract_slide_text() {
        let xml = "<p><a:t>Hello</a:t> <a:t>World</a:t></p>";
        assert_eq!(extract_slide_text(xml), "Hello World");
    }

    #[test]
    fn test_decode_xml_entities() {
        assert_eq!(decode_xml_entities("Tom &amp; Jerry"), "Tom & Jerry");
        assert_eq!(decode_xml_entities("&lt;tag&gt;"), "<tag>");
    }

    #[test]
    fn test_descriptor_name() {
        let skill = PptxLoaderSkill::new();
        assert_eq!(skill.descriptor().name, "pptx_loader");
    }
}

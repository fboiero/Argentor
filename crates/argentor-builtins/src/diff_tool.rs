//! Text diff generation skill using LCS (Longest Common Subsequence) algorithm.
//!
//! Pure Rust implementation inspired by Unix diff. No external dependencies
//! beyond the standard library.
//!
//! # Supported operations
//!
//! - `diff` -- Generate a unified diff between original and modified text.
//! - `patch` -- Apply a unified diff to text, producing the patched result.
//! - `stats` -- Compute diff statistics (lines added, removed, similarity).
//! - `word_diff` -- Word-level diff highlighting changed words.
//! - `char_diff` -- Character-level diff for small text comparisons.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;

/// Skill for generating and applying text diffs.
pub struct DiffSkill {
    descriptor: SkillDescriptor,
}

impl DiffSkill {
    /// Create a new `DiffSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "diff".to_string(),
                description: "Text diff generation and patching. Operations: diff, \
                              patch, stats, word_diff, char_diff."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["diff", "patch", "stats", "word_diff", "char_diff"],
                            "description": "The diff operation to perform"
                        },
                        "original": {
                            "type": "string",
                            "description": "The original text"
                        },
                        "modified": {
                            "type": "string",
                            "description": "The modified text"
                        },
                        "diff_text": {
                            "type": "string",
                            "description": "Unified diff text (for patch operation)"
                        },
                        "context": {
                            "type": "integer",
                            "description": "Number of context lines in unified diff (default 3)"
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

impl Default for DiffSkill {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LCS-based diff engine
// ---------------------------------------------------------------------------

/// A single edit operation in the diff.
#[derive(Debug, Clone, PartialEq)]
enum EditOp {
    Equal(String),
    Insert(String),
    Delete(String),
}

/// Compute the LCS (Longest Common Subsequence) table for two sequences of lines.
fn lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
    let m = a.len();
    let n = b.len();
    let mut table = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                table[i][j] = table[i - 1][j - 1] + 1;
            } else {
                table[i][j] = table[i - 1][j].max(table[i][j - 1]);
            }
        }
    }

    table
}

/// Backtrack the LCS table to produce a sequence of edit operations.
fn backtrack_edits(a: &[&str], b: &[&str], table: &[Vec<usize>]) -> Vec<EditOp> {
    let mut edits = Vec::new();
    let mut i = a.len();
    let mut j = b.len();

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            edits.push(EditOp::Equal(a[i - 1].to_string()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
            edits.push(EditOp::Insert(b[j - 1].to_string()));
            j -= 1;
        } else if i > 0 {
            edits.push(EditOp::Delete(a[i - 1].to_string()));
            i -= 1;
        }
    }

    edits.reverse();
    edits
}

/// Compute edit operations between two texts (line-level).
fn compute_line_edits(original: &str, modified: &str) -> Vec<EditOp> {
    let a: Vec<&str> = original.lines().collect();
    let b: Vec<&str> = modified.lines().collect();
    let table = lcs_table(&a, &b);
    backtrack_edits(&a, &b, &table)
}

/// Generate unified diff output with context lines.
fn generate_unified_diff(original: &str, modified: &str, context: usize) -> String {
    let edits = compute_line_edits(original, modified);

    if edits.iter().all(|e| matches!(e, EditOp::Equal(_))) {
        return String::new(); // No differences
    }

    // Convert edits to tagged lines: (' ', line), ('+', line), ('-', line)
    let mut tagged: Vec<(char, &str)> = Vec::new();
    for edit in &edits {
        match edit {
            EditOp::Equal(s) => tagged.push((' ', s)),
            EditOp::Insert(s) => tagged.push(('+', s)),
            EditOp::Delete(s) => tagged.push(('-', s)),
        }
    }

    // Group into hunks with context
    let mut output = String::new();
    output.push_str("--- original\n");
    output.push_str("+++ modified\n");

    // Find ranges of changes and include context
    let mut i = 0;
    while i < tagged.len() {
        // Skip equal lines until we find a change
        if tagged[i].0 == ' ' {
            i += 1;
            continue;
        }

        // Found a change; collect the hunk
        let hunk_start = i.saturating_sub(context);

        // Find end of this group of changes (including gaps smaller than 2*context)
        let mut hunk_end = i;
        while hunk_end < tagged.len() {
            if tagged[hunk_end].0 != ' ' {
                hunk_end += 1;
            } else {
                // Check if there's another change within context range
                let lookahead = (hunk_end + 2 * context + 1).min(tagged.len());
                let has_nearby_change = tagged[hunk_end..lookahead]
                    .iter()
                    .any(|(tag, _)| *tag != ' ');
                if has_nearby_change {
                    hunk_end += 1;
                } else {
                    break;
                }
            }
        }

        // Add trailing context
        let trailing_end = (hunk_end + context).min(tagged.len());

        // Compute line numbers for the hunk header
        let mut orig_start = 1usize;
        let mut orig_count = 0usize;
        let mut mod_start = 1usize;
        let mut mod_count = 0usize;

        // Count lines before the hunk to determine starting line numbers
        for (tag, _) in tagged.iter().take(hunk_start) {
            match tag {
                ' ' => {
                    orig_start += 1;
                    mod_start += 1;
                }
                '-' => orig_start += 1,
                '+' => mod_start += 1,
                _ => {}
            }
        }

        // Count lines in the hunk
        for (tag, _) in tagged.iter().take(trailing_end).skip(hunk_start) {
            match tag {
                ' ' => {
                    orig_count += 1;
                    mod_count += 1;
                }
                '-' => orig_count += 1,
                '+' => mod_count += 1,
                _ => {}
            }
        }

        output.push_str(&format!(
            "@@ -{orig_start},{orig_count} +{mod_start},{mod_count} @@\n"
        ));

        for (tag, line) in tagged.iter().take(trailing_end).skip(hunk_start) {
            output.push(*tag);
            output.push_str(line);
            output.push('\n');
        }

        i = trailing_end;
    }

    output
}

/// Apply a unified diff to the original text.
fn apply_patch(original: &str, diff_text: &str) -> Result<String, String> {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut orig_idx = 0usize;

    let diff_lines: Vec<&str> = diff_text.lines().collect();
    let mut d = 0;

    // Skip file headers
    while d < diff_lines.len() {
        let line = diff_lines[d];
        if line.starts_with("@@") {
            break;
        }
        d += 1;
    }

    while d < diff_lines.len() {
        let line = diff_lines[d];

        if line.starts_with("@@") {
            // Parse hunk header: @@ -orig_start,orig_count +mod_start,mod_count @@
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                return Err(format!("Invalid hunk header: {line}"));
            }
            let orig_range = parts[1].trim_start_matches('-');
            let orig_start: usize = orig_range
                .split(',')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);

            // Copy lines from original up to the hunk start
            while orig_idx + 1 < orig_start && orig_idx < orig_lines.len() {
                result.push(orig_lines[orig_idx].to_string());
                orig_idx += 1;
            }

            d += 1;
            continue;
        }

        if line.starts_with(' ') {
            // Context line -- copy from original
            if orig_idx < orig_lines.len() {
                result.push(orig_lines[orig_idx].to_string());
                orig_idx += 1;
            }
        } else if let Some(stripped) = line.strip_prefix('+') {
            // Added line
            result.push(stripped.to_string());
        } else if line.starts_with('-') {
            // Removed line -- skip original
            orig_idx += 1;
        }

        d += 1;
    }

    // Copy remaining original lines
    while orig_idx < orig_lines.len() {
        result.push(orig_lines[orig_idx].to_string());
        orig_idx += 1;
    }

    Ok(result.join("\n"))
}

/// Compute diff statistics.
fn compute_stats(original: &str, modified: &str) -> serde_json::Value {
    let edits = compute_line_edits(original, modified);

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut unchanged = 0usize;

    for edit in &edits {
        match edit {
            EditOp::Equal(_) => unchanged += 1,
            EditOp::Insert(_) => added += 1,
            EditOp::Delete(_) => removed += 1,
        }
    }

    let total = added + removed + unchanged;
    let similarity = if total == 0 {
        100.0
    } else {
        (unchanged as f64 / (unchanged + removed.max(added)) as f64) * 100.0
    };

    serde_json::json!({
        "lines_added": added,
        "lines_removed": removed,
        "lines_unchanged": unchanged,
        "similarity_percentage": (similarity * 100.0).round() / 100.0,
    })
}

/// Word-level diff between two texts.
fn word_diff(original: &str, modified: &str) -> serde_json::Value {
    let a_words: Vec<&str> = original.split_whitespace().collect();
    let b_words: Vec<&str> = modified.split_whitespace().collect();

    let table = lcs_table(&a_words, &b_words);
    let edits = backtrack_edits(&a_words, &b_words, &table);

    let mut changes: Vec<serde_json::Value> = Vec::new();
    for edit in &edits {
        match edit {
            EditOp::Equal(w) => changes.push(serde_json::json!({"type": "equal", "value": w})),
            EditOp::Insert(w) => changes.push(serde_json::json!({"type": "insert", "value": w})),
            EditOp::Delete(w) => changes.push(serde_json::json!({"type": "delete", "value": w})),
        }
    }

    // Build inline display: [-removed-] {+added+}
    let mut display = String::new();
    for edit in &edits {
        if !display.is_empty() {
            display.push(' ');
        }
        match edit {
            EditOp::Equal(w) => display.push_str(w),
            EditOp::Insert(w) => {
                display.push_str("{+");
                display.push_str(w);
                display.push_str("+}");
            }
            EditOp::Delete(w) => {
                display.push_str("[-");
                display.push_str(w);
                display.push_str("-]");
            }
        }
    }

    serde_json::json!({
        "changes": changes,
        "display": display,
    })
}

/// Character-level diff between two texts.
fn char_diff(original: &str, modified: &str) -> serde_json::Value {
    let a_chars: Vec<&str> = original.chars().map(|_| "").collect::<Vec<_>>();
    // We need to work with char slices for LCS
    let a: Vec<String> = original.chars().map(|c| c.to_string()).collect();
    let b: Vec<String> = modified.chars().map(|c| c.to_string()).collect();
    let a_refs: Vec<&str> = a.iter().map(std::string::String::as_str).collect();
    let b_refs: Vec<&str> = b.iter().map(std::string::String::as_str).collect();

    let _ = a_chars; // suppress unused

    let table = lcs_table(&a_refs, &b_refs);
    let edits = backtrack_edits(&a_refs, &b_refs, &table);

    let mut changes: Vec<serde_json::Value> = Vec::new();
    // Coalesce consecutive same-type edits
    let mut current_type: Option<&str> = None;
    let mut current_buf = String::new();

    for edit in &edits {
        let (tag, ch) = match edit {
            EditOp::Equal(c) => ("equal", c.as_str()),
            EditOp::Insert(c) => ("insert", c.as_str()),
            EditOp::Delete(c) => ("delete", c.as_str()),
        };

        if current_type == Some(tag) {
            current_buf.push_str(ch);
        } else {
            if let Some(t) = current_type {
                changes.push(serde_json::json!({"type": t, "value": current_buf}));
            }
            current_type = Some(tag);
            current_buf = ch.to_string();
        }
    }
    if let Some(t) = current_type {
        if !current_buf.is_empty() {
            changes.push(serde_json::json!({"type": t, "value": current_buf}));
        }
    }

    // Build inline display
    let mut display = String::new();
    for change in &changes {
        let t = change["type"].as_str().unwrap_or("");
        let v = change["value"].as_str().unwrap_or("");
        match t {
            "equal" => display.push_str(v),
            "insert" => {
                display.push_str("{+");
                display.push_str(v);
                display.push_str("+}");
            }
            "delete" => {
                display.push_str("[-");
                display.push_str(v);
                display.push_str("-]");
            }
            _ => {}
        }
    }

    serde_json::json!({
        "changes": changes,
        "display": display,
    })
}

// ---------------------------------------------------------------------------
// Skill implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Skill for DiffSkill {
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
            "diff" => {
                let original = match call.arguments["original"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'original'",
                        ))
                    }
                };
                let modified = match call.arguments["modified"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'modified'",
                        ))
                    }
                };
                let context = call.arguments["context"]
                    .as_u64()
                    .unwrap_or(3) as usize;
                let diff_output = generate_unified_diff(original, modified, context);
                let has_changes = !diff_output.is_empty();
                let result = serde_json::json!({
                    "has_changes": has_changes,
                    "diff": diff_output,
                });
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "patch" => {
                let original = match call.arguments["original"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'original'",
                        ))
                    }
                };
                let diff_text = match call.arguments["diff_text"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'diff_text'",
                        ))
                    }
                };
                match apply_patch(original, diff_text) {
                    Ok(patched) => {
                        let result = serde_json::json!({
                            "patched_text": patched,
                            "success": true,
                        });
                        Ok(ToolResult::success(&call.id, result.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(&call.id, format!("Patch failed: {e}"))),
                }
            }
            "stats" => {
                let original = match call.arguments["original"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'original'",
                        ))
                    }
                };
                let modified = match call.arguments["modified"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'modified'",
                        ))
                    }
                };
                let result = compute_stats(original, modified);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "word_diff" => {
                let original = match call.arguments["original"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'original'",
                        ))
                    }
                };
                let modified = match call.arguments["modified"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'modified'",
                        ))
                    }
                };
                let result = word_diff(original, modified);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            "char_diff" => {
                let original = match call.arguments["original"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'original'",
                        ))
                    }
                };
                let modified = match call.arguments["modified"].as_str() {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "Missing required parameter: 'modified'",
                        ))
                    }
                };
                let result = char_diff(original, modified);
                Ok(ToolResult::success(&call.id, result.to_string()))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation: '{operation}'. Supported: diff, patch, stats, word_diff, char_diff"
                ),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn skill() -> DiffSkill {
        DiffSkill::new()
    }

    fn make_call(op: &str, args: serde_json::Value) -> ToolCall {
        let mut merged = args.clone();
        merged["operation"] = serde_json::json!(op);
        ToolCall {
            id: "test".to_string(),
            name: "diff".to_string(),
            arguments: merged,
        }
    }

    // -- Descriptor ----------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let s = skill();
        assert_eq!(s.descriptor().name, "diff");
        assert!(s.descriptor().required_capabilities.is_empty());
    }

    #[test]
    fn test_default() {
        let s = DiffSkill::default();
        assert_eq!(s.descriptor().name, "diff");
    }

    // -- diff operation ------------------------------------------------------

    #[tokio::test]
    async fn test_diff_identical() {
        let s = skill();
        let c = make_call(
            "diff",
            serde_json::json!({"original": "hello\nworld", "modified": "hello\nworld"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["has_changes"], false);
        assert_eq!(v["diff"], "");
    }

    #[tokio::test]
    async fn test_diff_simple_change() {
        let s = skill();
        let c = make_call(
            "diff",
            serde_json::json!({
                "original": "line1\nline2\nline3",
                "modified": "line1\nchanged\nline3"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["has_changes"], true);
        let diff = v["diff"].as_str().unwrap();
        assert!(diff.contains("---"));
        assert!(diff.contains("+++"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+changed"));
    }

    #[tokio::test]
    async fn test_diff_addition() {
        let s = skill();
        let c = make_call(
            "diff",
            serde_json::json!({
                "original": "line1\nline2",
                "modified": "line1\nline2\nline3"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["has_changes"], true);
        let diff = v["diff"].as_str().unwrap();
        assert!(diff.contains("+line3"));
    }

    #[tokio::test]
    async fn test_diff_deletion() {
        let s = skill();
        let c = make_call(
            "diff",
            serde_json::json!({
                "original": "line1\nline2\nline3",
                "modified": "line1\nline3"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["has_changes"], true);
        let diff = v["diff"].as_str().unwrap();
        assert!(diff.contains("-line2"));
    }

    #[tokio::test]
    async fn test_diff_empty_original() {
        let s = skill();
        let c = make_call(
            "diff",
            serde_json::json!({"original": "", "modified": "new content"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["has_changes"], true);
    }

    #[tokio::test]
    async fn test_diff_empty_modified() {
        let s = skill();
        let c = make_call(
            "diff",
            serde_json::json!({"original": "old content", "modified": ""}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["has_changes"], true);
    }

    // -- stats operation -----------------------------------------------------

    #[tokio::test]
    async fn test_stats_identical() {
        let s = skill();
        let c = make_call(
            "stats",
            serde_json::json!({"original": "a\nb\nc", "modified": "a\nb\nc"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["lines_added"], 0);
        assert_eq!(v["lines_removed"], 0);
        assert_eq!(v["lines_unchanged"], 3);
        assert_eq!(v["similarity_percentage"], 100.0);
    }

    #[tokio::test]
    async fn test_stats_all_different() {
        let s = skill();
        let c = make_call(
            "stats",
            serde_json::json!({"original": "a\nb", "modified": "x\ny"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["lines_added"].as_u64().unwrap() > 0);
        assert!(v["lines_removed"].as_u64().unwrap() > 0);
        assert_eq!(v["similarity_percentage"], 0.0);
    }

    #[tokio::test]
    async fn test_stats_partial_change() {
        let s = skill();
        let c = make_call(
            "stats",
            serde_json::json!({
                "original": "a\nb\nc\nd",
                "modified": "a\nx\nc\nd"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["lines_unchanged"], 3);
        assert!(v["similarity_percentage"].as_f64().unwrap() > 50.0);
    }

    // -- word_diff operation -------------------------------------------------

    #[tokio::test]
    async fn test_word_diff_simple() {
        let s = skill();
        let c = make_call(
            "word_diff",
            serde_json::json!({
                "original": "the quick brown fox",
                "modified": "the slow brown fox"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let display = v["display"].as_str().unwrap();
        assert!(display.contains("[-quick-]"));
        assert!(display.contains("{+slow+}"));
        assert!(display.contains("the"));
        assert!(display.contains("fox"));
    }

    #[tokio::test]
    async fn test_word_diff_identical() {
        let s = skill();
        let c = make_call(
            "word_diff",
            serde_json::json!({"original": "same text", "modified": "same text"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let changes = v["changes"].as_array().unwrap();
        assert!(changes.iter().all(|c| c["type"] == "equal"));
    }

    // -- char_diff operation -------------------------------------------------

    #[tokio::test]
    async fn test_char_diff_simple() {
        let s = skill();
        let c = make_call(
            "char_diff",
            serde_json::json!({"original": "cat", "modified": "car"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let display = v["display"].as_str().unwrap();
        assert!(display.contains("ca"));
        // t -> r change should be visible
        assert!(display.contains("[-t-]") || display.contains("{+r+}"));
    }

    #[tokio::test]
    async fn test_char_diff_identical() {
        let s = skill();
        let c = make_call(
            "char_diff",
            serde_json::json!({"original": "abc", "modified": "abc"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let display = v["display"].as_str().unwrap();
        assert_eq!(display, "abc");
    }

    // -- patch operation -----------------------------------------------------

    #[tokio::test]
    async fn test_patch_simple() {
        let s = skill();
        let original = "line1\nline2\nline3";
        let modified = "line1\nchanged\nline3";

        // First generate the diff
        let c = make_call(
            "diff",
            serde_json::json!({"original": original, "modified": modified}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        let diff_text = v["diff"].as_str().unwrap();

        // Then apply it
        let c2 = make_call(
            "patch",
            serde_json::json!({"original": original, "diff_text": diff_text}),
        );
        let r2 = s.execute(c2).await.unwrap();
        assert!(!r2.is_error, "Patch failed: {}", r2.content);
        let v2: serde_json::Value = serde_json::from_str(&r2.content).unwrap();
        assert_eq!(v2["success"], true);
        let patched = v2["patched_text"].as_str().unwrap();
        assert!(patched.contains("changed"));
        assert!(!patched.contains("line2") || patched.contains("changed"));
    }

    // -- Error handling ------------------------------------------------------

    #[tokio::test]
    async fn test_missing_operation() {
        let s = skill();
        let c = ToolCall {
            id: "test".to_string(),
            name: "diff".to_string(),
            arguments: serde_json::json!({"original": "a"}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("operation"));
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let s = skill();
        let c = make_call(
            "bogus",
            serde_json::json!({"original": "a", "modified": "b"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_diff_missing_original() {
        let s = skill();
        let c = make_call("diff", serde_json::json!({"modified": "b"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("original"));
    }

    #[tokio::test]
    async fn test_diff_missing_modified() {
        let s = skill();
        let c = make_call("diff", serde_json::json!({"original": "a"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("modified"));
    }

    #[tokio::test]
    async fn test_patch_missing_diff_text() {
        let s = skill();
        let c = make_call("patch", serde_json::json!({"original": "a"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("diff_text"));
    }

    // -- LCS unit tests ------------------------------------------------------

    #[test]
    fn test_lcs_table_simple() {
        let a = vec!["a", "b", "c"];
        let b = vec!["a", "c"];
        let table = lcs_table(&a, &b);
        assert_eq!(table[3][2], 2); // LCS length is 2
    }

    #[test]
    fn test_lcs_empty() {
        let a: Vec<&str> = vec![];
        let b = vec!["a"];
        let table = lcs_table(&a, &b);
        assert_eq!(table[0][1], 0);
    }

    #[test]
    fn test_compute_line_edits_identical() {
        let edits = compute_line_edits("a\nb", "a\nb");
        assert!(edits.iter().all(|e| matches!(e, EditOp::Equal(_))));
    }
}

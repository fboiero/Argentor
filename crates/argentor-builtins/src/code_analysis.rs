//! Code analysis skill for the Argentor agent framework.
//!
//! Provides language-aware code analysis without depending on heavy external tools.
//! Uses Rust's standard library for file traversal and the `regex` crate for
//! pattern matching.
//!
//! # Supported operations
//!
//! - `search` — Search for a regex pattern across files.
//! - `find_definitions` — Find function/struct/enum/trait/impl definitions.
//! - `count_loc` — Count lines of code per language.
//! - `file_tree` — Show directory tree structure.
//! - `find_references` — Find all occurrences of a symbol name.
//! - `analyze_imports` — List imports/dependencies used in a file.
//! - `file_info` — Get detailed info about a file (size, lines, language, last modified).

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;
use tracing::info;

// ---------------------------------------------------------------------------
// Static regex patterns for analyze_imports (compiled once, reused across calls)
// ---------------------------------------------------------------------------

fn re_rust_use() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"^\s*use\s+(.+);"))
}

fn re_python_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"^\s*import\s+(.+)"))
}

fn re_python_from_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"^\s*from\s+(\S+)\s+import\s+(.+)"))
}

fn re_js_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"^\s*import\s+(.+)"))
}

fn re_js_require() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r#"require\s*\(\s*['"]([^'"]+)['"]\s*\)"#))
}

fn re_go_single_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r#"^\s*import\s+"([^"]+)""#))
}

fn re_go_block_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r#"^\s*"([^"]+)""#))
}

fn compile_static_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(err) => panic!("invalid built-in code analysis regex `{pattern}`: {err}"),
    }
}

/// Directories that are always excluded from traversal.
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    "dist",
    "build",
    ".next",
    ".nuxt",
    "vendor",
    ".idea",
    ".vscode",
];

/// Maximum number of results returned by default.
const DEFAULT_MAX_RESULTS: usize = 50;

/// Code analysis skill. Provides language-aware code analysis using only the
/// standard library for file traversal and the `regex` crate for pattern matching.
pub struct CodeAnalysisSkill {
    descriptor: SkillDescriptor,
}

impl CodeAnalysisSkill {
    /// Create a new `CodeAnalysisSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "code_analysis".to_string(),
                description: "Analyze source code: search patterns, find definitions, \
                              count lines of code, show file trees, find references, \
                              analyze imports, and get file info."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": [
                                "search",
                                "find_definitions",
                                "count_loc",
                                "file_tree",
                                "find_references",
                                "analyze_imports",
                                "file_info"
                            ],
                            "description": "The analysis operation to perform"
                        },
                        "path": {
                            "type": "string",
                            "description": "Root directory or file path for the operation"
                        },
                        "file_path": {
                            "type": "string",
                            "description": "Path to a specific file (for analyze_imports)"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "Regex pattern to search for (for search operation)"
                        },
                        "name": {
                            "type": "string",
                            "description": "Symbol name filter (for find_definitions, find_references)"
                        },
                        "glob": {
                            "type": "string",
                            "description": "File filter glob pattern like '*.rs' (for search, file_tree)"
                        },
                        "language": {
                            "type": "string",
                            "description": "Language filter: rust, python, typescript, go (for find_definitions)"
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum directory depth for file_tree (default: 3)"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 50)"
                        },
                        "exclude": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Additional patterns to exclude (for count_loc)"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![Capability::FileRead {
                    allowed_paths: vec![], // Configured at runtime
                }],
                requires_approval: false,
            },
        }
    }
}

impl Default for CodeAnalysisSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for CodeAnalysisSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"].as_str().unwrap_or_default();

        info!(operation, "code_analysis skill invoked");

        match operation {
            "search" => execute_search(&call).await,
            "find_definitions" => execute_find_definitions(&call).await,
            "count_loc" => execute_count_loc(&call).await,
            "file_tree" => execute_file_tree(&call).await,
            "find_references" => execute_find_references(&call).await,
            "analyze_imports" => execute_analyze_imports(&call).await,
            "file_info" => execute_file_info(&call).await,
            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation '{operation}'. \
                     Valid operations: search, find_definitions, count_loc, file_tree, \
                     find_references, analyze_imports, file_info"
                ),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether a filename matches a simple glob pattern (e.g. "*.rs", "*.py").
/// Supports only the `*` wildcard at the beginning of the pattern.
fn matches_glob(filename: &str, glob_pattern: &str) -> bool {
    if glob_pattern == "*" {
        return true;
    }
    if let Some(suffix) = glob_pattern.strip_prefix('*') {
        filename.ends_with(suffix)
    } else {
        filename == glob_pattern
    }
}

/// Check whether a directory name should be excluded.
fn is_excluded_dir(name: &str, extra_excludes: &[String]) -> bool {
    if EXCLUDED_DIRS.contains(&name) {
        return true;
    }
    extra_excludes.iter().any(|e| name == e.as_str())
}

/// Recursively walk a directory, collecting files.
/// Respects depth limits and exclusion lists.
fn walk_dir(
    root: &Path,
    glob_filter: Option<&str>,
    extra_excludes: &[String],
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_dir_recursive(root, glob_filter, extra_excludes, 0, max_depth, &mut files);
    files
}

fn walk_dir_recursive(
    dir: &Path,
    glob_filter: Option<&str>,
    extra_excludes: &[String],
    current_depth: usize,
    max_depth: usize,
    files: &mut Vec<PathBuf>,
) {
    if current_depth > max_depth {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if path.is_dir() {
            if !is_excluded_dir(&name, extra_excludes) {
                walk_dir_recursive(
                    &path,
                    glob_filter,
                    extra_excludes,
                    current_depth + 1,
                    max_depth,
                    files,
                );
            }
        } else if path.is_file() {
            if let Some(glob) = glob_filter {
                if matches_glob(&name, glob) {
                    files.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
}

/// Detect language from file extension.
fn detect_language(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "shell",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" => "css",
        "sql" => "sql",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "lua" => "lua",
        "zig" => "zig",
        _ => "unknown",
    }
}

/// Check if a line is a comment for the given language.
fn is_comment(line: &str, lang: &str) -> bool {
    let trimmed = line.trim();
    match lang {
        "rust" | "go" | "java" | "c" | "cpp" | "javascript" | "typescript" | "swift" | "kotlin"
        | "zig" => {
            trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*')
        }
        "python" | "ruby" | "shell" => trimmed.starts_with('#'),
        "lua" => trimmed.starts_with("--"),
        "html" => trimmed.starts_with("<!--"),
        "css" => trimmed.starts_with("/*") || trimmed.starts_with('*'),
        "sql" => trimmed.starts_with("--") || trimmed.starts_with("/*"),
        _ => false,
    }
}

/// Get definition patterns for a given language.
fn definition_patterns(language: &str) -> Vec<&'static str> {
    match language {
        "rust" => vec![
            r"(?:pub\s+)?(?:async\s+)?fn\s+\w+",
            r"(?:pub\s+)?struct\s+\w+",
            r"(?:pub\s+)?enum\s+\w+",
            r"(?:pub\s+)?trait\s+\w+",
            r"impl(?:<[^>]*>)?\s+\w+",
            r"(?:pub\s+)?mod\s+\w+",
            r"(?:pub\s+)?type\s+\w+",
            r"(?:pub\s+)?const\s+\w+",
            r"(?:pub\s+)?static\s+\w+",
        ],
        "python" => vec![r"(?:async\s+)?def\s+\w+", r"class\s+\w+"],
        "typescript" | "javascript" => vec![
            r"(?:async\s+)?function\s+\w+",
            r"class\s+\w+",
            r"interface\s+\w+",
            r"(?:export\s+(?:default\s+)?)?(?:const|let|var)\s+\w+\s*=",
            r"export\s+(?:default\s+)?(?:async\s+)?function\s+\w+",
            r"export\s+(?:default\s+)?class\s+\w+",
            r"export\s+(?:default\s+)?interface\s+\w+",
        ],
        "go" => vec![
            r"func\s+(?:\([^)]*\)\s+)?\w+",
            r"type\s+\w+\s+struct",
            r"type\s+\w+\s+interface",
        ],
        _ => vec![],
    }
}

/// Get file extensions for a given language.
fn language_extensions(language: &str) -> Vec<&'static str> {
    match language {
        "rust" => vec!["rs"],
        "python" => vec!["py", "pyi"],
        "typescript" => vec!["ts", "tsx"],
        "javascript" => vec!["js", "jsx", "mjs", "cjs"],
        "go" => vec!["go"],
        _ => vec![],
    }
}

/// Build a glob filter from a language name.
fn glob_for_language(language: &str) -> Option<Vec<String>> {
    let exts = language_extensions(language);
    if exts.is_empty() {
        None
    } else {
        Some(exts.iter().map(|e| format!("*.{e}")).collect())
    }
}

/// Format a `SystemTime` as an ISO 8601 string.
fn format_system_time(time: SystemTime) -> String {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs();
            // Simple UTC formatting without pulling in chrono for this one call.
            // chrono is already a dependency, so we use it.
            let dt = chrono::DateTime::from_timestamp(secs as i64, 0);
            match dt {
                Some(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                None => "unknown".to_string(),
            }
        }
        Err(_) => "unknown".to_string(),
    }
}

/// Build the directory tree structure as a JSON-friendly nested representation.
fn build_tree(
    dir: &Path,
    glob_filter: Option<&str>,
    extra_excludes: &[String],
    current_depth: usize,
    max_depth: usize,
) -> Vec<serde_json::Value> {
    if current_depth > max_depth {
        return vec![];
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return vec![],
    };

    let mut items: Vec<(String, bool, PathBuf)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_string();

        if path.is_dir() {
            if !is_excluded_dir(&name, extra_excludes) {
                items.push((name, true, path));
            }
        } else if path.is_file() {
            if let Some(glob) = glob_filter {
                if matches_glob(&name, glob) {
                    items.push((name, false, path));
                }
            } else {
                items.push((name, false, path));
            }
        }
    }

    items.sort_by(|a, b| {
        // Directories first, then alphabetical.
        match (a.1, b.1) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.0.cmp(&b.0),
        }
    });

    items
        .into_iter()
        .map(|(name, is_dir, path)| {
            if is_dir {
                let children = build_tree(
                    &path,
                    glob_filter,
                    extra_excludes,
                    current_depth + 1,
                    max_depth,
                );
                serde_json::json!({
                    "name": name,
                    "type": "directory",
                    "children": children,
                })
            } else {
                serde_json::json!({
                    "name": name,
                    "type": "file",
                })
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Operation implementations
// ---------------------------------------------------------------------------

/// `search` — Search for a regex pattern across files.
async fn execute_search(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let pattern_str = call.arguments["pattern"].as_str().unwrap_or_default();
    if pattern_str.is_empty() {
        return Ok(ToolResult::error(
            &call.id,
            "Missing required parameter 'pattern'",
        ));
    }

    let path_str = call.arguments["path"].as_str().unwrap_or(".");
    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(ToolResult::error(
            &call.id,
            format!("Path does not exist: '{path_str}'"),
        ));
    }

    let re = match Regex::new(pattern_str) {
        Ok(re) => re,
        Err(e) => {
            return Ok(ToolResult::error(
                &call.id,
                format!("Invalid regex pattern '{pattern_str}': {e}"),
            ));
        }
    };

    let glob_filter = call.arguments["glob"].as_str();
    let max_results = call.arguments["max_results"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_RESULTS as u64) as usize;

    let files = walk_dir(path, glob_filter, &[], 100);
    let mut matches: Vec<serde_json::Value> = Vec::new();

    for file_path in &files {
        if matches.len() >= max_results {
            break;
        }

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue, // Skip binary or unreadable files
        };

        for (line_num, line) in content.lines().enumerate() {
            if matches.len() >= max_results {
                break;
            }
            if re.is_match(line) {
                matches.push(serde_json::json!({
                    "file": file_path.display().to_string(),
                    "line": line_num + 1,
                    "content": line.trim(),
                }));
            }
        }
    }

    let response = serde_json::json!({
        "pattern": pattern_str,
        "total_matches": matches.len(),
        "matches": matches,
    });

    Ok(ToolResult::success(&call.id, response.to_string()))
}

/// `find_definitions` — Find function/struct/enum/trait/impl definitions.
async fn execute_find_definitions(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let path_str = call.arguments["path"].as_str().unwrap_or(".");
    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(ToolResult::error(
            &call.id,
            format!("Path does not exist: '{path_str}'"),
        ));
    }

    let name_filter = call.arguments["name"].as_str();
    let language_filter = call.arguments["language"].as_str();

    // Determine which languages and extensions to search.
    let lang_globs: Vec<String> = if let Some(lang) = language_filter {
        glob_for_language(lang).unwrap_or_default()
    } else {
        vec![]
    };

    let files = if lang_globs.is_empty() {
        walk_dir(path, None, &[], 100)
    } else {
        let mut all_files = Vec::new();
        for glob in &lang_globs {
            all_files.extend(walk_dir(path, Some(glob), &[], 100));
        }
        all_files
    };

    let mut definitions: Vec<serde_json::Value> = Vec::new();

    for file_path in &files {
        let lang = if let Some(l) = language_filter {
            l
        } else {
            detect_language(file_path)
        };

        let patterns = definition_patterns(lang);
        if patterns.is_empty() {
            continue;
        }

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for pat_str in &patterns {
            let re = match Regex::new(pat_str) {
                Ok(re) => re,
                Err(_) => continue,
            };

            for (line_num, line) in content.lines().enumerate() {
                if let Some(m) = re.find(line) {
                    let matched_text = m.as_str().trim();

                    // If name filter is specified, check if the definition contains it.
                    if let Some(name) = name_filter {
                        if !matched_text.contains(name) {
                            continue;
                        }
                    }

                    definitions.push(serde_json::json!({
                        "file": file_path.display().to_string(),
                        "line": line_num + 1,
                        "definition": matched_text,
                        "language": lang,
                    }));
                }
            }
        }
    }

    // Deduplicate by (file, line)
    definitions.sort_by(|a, b| {
        let file_cmp = a["file"]
            .as_str()
            .unwrap_or("")
            .cmp(b["file"].as_str().unwrap_or(""));
        if file_cmp != std::cmp::Ordering::Equal {
            return file_cmp;
        }
        a["line"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&b["line"].as_u64().unwrap_or(0))
    });
    definitions.dedup_by(|a, b| a["file"] == b["file"] && a["line"] == b["line"]);

    let response = serde_json::json!({
        "total": definitions.len(),
        "definitions": definitions,
    });

    Ok(ToolResult::success(&call.id, response.to_string()))
}

/// `count_loc` — Count lines of code per language.
async fn execute_count_loc(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let path_str = call.arguments["path"].as_str().unwrap_or(".");
    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(ToolResult::error(
            &call.id,
            format!("Path does not exist: '{path_str}'"),
        ));
    }

    let extra_excludes: Vec<String> = call
        .arguments
        .get("exclude")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let files = walk_dir(path, None, &extra_excludes, 100);

    // Per-language stats: (code_lines, blank_lines, comment_lines, file_count)
    let mut stats: HashMap<String, (usize, usize, usize, usize)> = HashMap::new();
    let mut total_files: usize = 0;

    for file_path in &files {
        let lang = detect_language(file_path);
        if lang == "unknown" {
            continue;
        }

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        total_files += 1;
        let entry = stats.entry(lang.to_string()).or_insert((0, 0, 0, 0));
        entry.3 += 1; // file_count

        for line in content.lines() {
            if line.trim().is_empty() {
                entry.1 += 1; // blank
            } else if is_comment(line, lang) {
                entry.2 += 1; // comment
            } else {
                entry.0 += 1; // code
            }
        }
    }

    let mut languages: Vec<serde_json::Value> = stats
        .iter()
        .map(|(lang, (code, blank, comment, file_count))| {
            serde_json::json!({
                "language": lang,
                "code_lines": code,
                "blank_lines": blank,
                "comment_lines": comment,
                "total_lines": code + blank + comment,
                "files": file_count,
            })
        })
        .collect();

    // Sort by code lines descending.
    languages.sort_by(|a, b| {
        b["code_lines"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["code_lines"].as_u64().unwrap_or(0))
    });

    let total_code: usize = stats.values().map(|(c, _, _, _)| c).sum();
    let total_blank: usize = stats.values().map(|(_, b, _, _)| b).sum();
    let total_comment: usize = stats.values().map(|(_, _, cm, _)| cm).sum();

    let response = serde_json::json!({
        "total_files": total_files,
        "total_code_lines": total_code,
        "total_blank_lines": total_blank,
        "total_comment_lines": total_comment,
        "total_lines": total_code + total_blank + total_comment,
        "languages": languages,
    });

    Ok(ToolResult::success(&call.id, response.to_string()))
}

/// `file_tree` — Show directory tree structure.
async fn execute_file_tree(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let path_str = call.arguments["path"].as_str().unwrap_or(".");
    let path = Path::new(path_str);
    if !path.exists() || !path.is_dir() {
        return Ok(ToolResult::error(
            &call.id,
            format!("Path does not exist or is not a directory: '{path_str}'"),
        ));
    }

    let depth = call.arguments["depth"].as_u64().unwrap_or(3) as usize;
    let glob_filter = call.arguments["glob"].as_str();

    let tree = build_tree(path, glob_filter, &[], 0, depth);

    let response = serde_json::json!({
        "root": path_str,
        "tree": tree,
    });

    Ok(ToolResult::success(&call.id, response.to_string()))
}

/// `find_references` — Find all occurrences of a symbol name.
async fn execute_find_references(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let name = call.arguments["name"].as_str().unwrap_or_default();
    if name.is_empty() {
        return Ok(ToolResult::error(
            &call.id,
            "Missing required parameter 'name'",
        ));
    }

    let path_str = call.arguments["path"].as_str().unwrap_or(".");
    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(ToolResult::error(
            &call.id,
            format!("Path does not exist: '{path_str}'"),
        ));
    }

    let max_results = call.arguments["max_results"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_RESULTS as u64) as usize;

    let files = walk_dir(path, None, &[], 100);
    let mut references: Vec<serde_json::Value> = Vec::new();

    // Build a word-boundary regex for the symbol name.
    let pattern = format!(r"\b{}\b", regex::escape(name));
    let re = match Regex::new(&pattern) {
        Ok(re) => re,
        Err(e) => {
            return Ok(ToolResult::error(
                &call.id,
                format!("Failed to build regex for name '{name}': {e}"),
            ));
        }
    };

    for file_path in &files {
        if references.len() >= max_results {
            break;
        }

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (line_num, line) in content.lines().enumerate() {
            if references.len() >= max_results {
                break;
            }
            if re.is_match(line) {
                references.push(serde_json::json!({
                    "file": file_path.display().to_string(),
                    "line": line_num + 1,
                    "content": line.trim(),
                }));
            }
        }
    }

    let response = serde_json::json!({
        "name": name,
        "total_references": references.len(),
        "references": references,
    });

    Ok(ToolResult::success(&call.id, response.to_string()))
}

/// `analyze_imports` — List imports/dependencies used in a file.
#[allow(clippy::expect_used)]
async fn execute_analyze_imports(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let file_path_str = call
        .arguments
        .get("file_path")
        .and_then(|v| v.as_str())
        .or_else(|| call.arguments.get("path").and_then(|v| v.as_str()))
        .unwrap_or_default();

    if file_path_str.is_empty() {
        return Ok(ToolResult::error(
            &call.id,
            "Missing required parameter 'file_path' or 'path'",
        ));
    }

    let path = Path::new(file_path_str);
    if !path.exists() || !path.is_file() {
        return Ok(ToolResult::error(
            &call.id,
            format!("File does not exist: '{file_path_str}'"),
        ));
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(ToolResult::error(
                &call.id,
                format!("Failed to read '{file_path_str}': {e}"),
            ));
        }
    };

    let lang = detect_language(path);
    let mut imports: Vec<serde_json::Value> = Vec::new();

    match lang {
        "rust" => {
            let re = re_rust_use();
            for (line_num, line) in content.lines().enumerate() {
                if let Some(caps) = re.captures(line) {
                    if let Some(m) = caps.get(1) {
                        imports.push(serde_json::json!({
                            "line": line_num + 1,
                            "import": m.as_str().trim(),
                            "statement": line.trim(),
                        }));
                    }
                }
            }
        }
        "python" => {
            let import_re = re_python_import();
            let from_re = re_python_from_import();
            for (line_num, line) in content.lines().enumerate() {
                if let Some(caps) = from_re.captures(line) {
                    let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let names = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    imports.push(serde_json::json!({
                        "line": line_num + 1,
                        "module": module,
                        "names": names.trim(),
                        "statement": line.trim(),
                    }));
                } else if let Some(caps) = import_re.captures(line) {
                    if let Some(m) = caps.get(1) {
                        imports.push(serde_json::json!({
                            "line": line_num + 1,
                            "import": m.as_str().trim(),
                            "statement": line.trim(),
                        }));
                    }
                }
            }
        }
        "typescript" | "javascript" => {
            let re = re_js_import();
            let require_re = re_js_require();
            for (line_num, line) in content.lines().enumerate() {
                if let Some(caps) = re.captures(line) {
                    if let Some(m) = caps.get(1) {
                        imports.push(serde_json::json!({
                            "line": line_num + 1,
                            "import": m.as_str().trim(),
                            "statement": line.trim(),
                        }));
                    }
                } else if let Some(caps) = require_re.captures(line) {
                    if let Some(m) = caps.get(1) {
                        imports.push(serde_json::json!({
                            "line": line_num + 1,
                            "import": m.as_str().trim(),
                            "statement": line.trim(),
                        }));
                    }
                }
            }
        }
        "go" => {
            let single_re = re_go_single_import();
            let block_import_re = re_go_block_import();
            let mut in_import_block = false;
            for (line_num, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("import (") {
                    in_import_block = true;
                    continue;
                }
                if in_import_block {
                    if trimmed == ")" {
                        in_import_block = false;
                        continue;
                    }
                    if let Some(caps) = block_import_re.captures(line) {
                        if let Some(m) = caps.get(1) {
                            imports.push(serde_json::json!({
                                "line": line_num + 1,
                                "import": m.as_str().trim(),
                                "statement": trimmed,
                            }));
                        }
                    }
                } else if let Some(caps) = single_re.captures(line) {
                    if let Some(m) = caps.get(1) {
                        imports.push(serde_json::json!({
                            "line": line_num + 1,
                            "import": m.as_str().trim(),
                            "statement": trimmed,
                        }));
                    }
                }
            }
        }
        _ => {
            return Ok(ToolResult::success(
                &call.id,
                serde_json::json!({
                    "file": file_path_str,
                    "language": lang,
                    "imports": [],
                    "note": format!("Import analysis not supported for language '{lang}'"),
                })
                .to_string(),
            ));
        }
    }

    let response = serde_json::json!({
        "file": file_path_str,
        "language": lang,
        "total_imports": imports.len(),
        "imports": imports,
    });

    Ok(ToolResult::success(&call.id, response.to_string()))
}

/// `file_info` — Get detailed info about a file.
async fn execute_file_info(call: &ToolCall) -> ArgentorResult<ToolResult> {
    let path_str = call.arguments["path"].as_str().unwrap_or_default();
    if path_str.is_empty() {
        return Ok(ToolResult::error(
            &call.id,
            "Missing required parameter 'path'",
        ));
    }

    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(ToolResult::error(
            &call.id,
            format!("Path does not exist: '{path_str}'"),
        ));
    }

    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Ok(ToolResult::error(
                &call.id,
                format!("Failed to read metadata for '{path_str}': {e}"),
            ));
        }
    };

    let size = metadata.len();
    let modified = metadata
        .modified()
        .map(format_system_time)
        .unwrap_or_else(|_| "unknown".to_string());

    let is_dir = metadata.is_dir();
    let language = if is_dir {
        "directory"
    } else {
        detect_language(path)
    };

    let line_count = if !is_dir {
        match fs::read_to_string(path) {
            Ok(content) => Some(content.lines().count()),
            Err(_) => None,
        }
    } else {
        None
    };

    let mut response = serde_json::json!({
        "path": path_str,
        "size_bytes": size,
        "is_directory": is_dir,
        "language": language,
        "last_modified": modified,
    });

    if let Some(lines) = line_count {
        response["lines"] = serde_json::json!(lines);
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        response["name"] = serde_json::json!(name);
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        response["extension"] = serde_json::json!(ext);
    }

    Ok(ToolResult::success(&call.id, response.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temporary directory with some Rust files.
    fn setup_temp_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();

        // Create a Rust source file
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();

        fs::write(
            src_dir.join("main.rs"),
            r#"use std::io;
use std::collections::HashMap;

/// Entry point.
fn main() {
    println!("Hello, world!");
}

pub struct Config {
    name: String,
}

pub enum Status {
    Active,
    Inactive,
}

pub trait Runnable {
    fn run(&self);
}

impl Config {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}
"#,
        )
        .unwrap();

        fs::write(
            src_dir.join("lib.rs"),
            r#"//! Library root.
use serde::{Deserialize, Serialize};

pub mod config;

pub const VERSION: &str = "0.1.0";

pub fn helper() -> bool {
    true
}
"#,
        )
        .unwrap();

        // Create a Python file
        fs::write(
            dir.path().join("script.py"),
            r#"import os
from pathlib import Path
import sys

def greet(name):
    print(f"Hello, {name}!")

class Greeter:
    def __init__(self, name):
        self.name = name
"#,
        )
        .unwrap();

        // Create a nested directory
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("helper.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        dir
    }

    #[tokio::test]
    async fn test_count_loc() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t1".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "count_loc",
                "path": dir.path().display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(
            parsed["total_code_lines"].as_u64().unwrap() > 0,
            "Should have code lines"
        );
        assert!(
            parsed["total_files"].as_u64().unwrap() >= 3,
            "Should have at least 3 files"
        );
    }

    #[tokio::test]
    async fn test_file_tree() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t2".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "file_tree",
                "path": dir.path().display().to_string(),
                "depth": 3,
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        let tree = parsed["tree"].as_array().unwrap();
        assert!(!tree.is_empty(), "Tree should not be empty");

        // Should contain the "src" directory
        let names: Vec<&str> = tree.iter().filter_map(|v| v["name"].as_str()).collect();
        assert!(names.contains(&"src"), "Tree should contain 'src' dir");
    }

    #[tokio::test]
    async fn test_find_definitions_rust() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t3".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "find_definitions",
                "path": dir.path().display().to_string(),
                "language": "rust",
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        let defs = parsed["definitions"].as_array().unwrap();

        // Should find fn main
        let has_main = defs
            .iter()
            .any(|d| d["definition"].as_str().unwrap_or("").contains("fn main"));
        assert!(has_main, "Should find 'fn main' definition");

        // Should find struct Config
        let has_config = defs.iter().any(|d| {
            d["definition"]
                .as_str()
                .unwrap_or("")
                .contains("struct Config")
        });
        assert!(has_config, "Should find 'struct Config' definition");
    }

    #[tokio::test]
    async fn test_find_definitions_with_name_filter() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t3b".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "find_definitions",
                "path": dir.path().display().to_string(),
                "name": "main",
                "language": "rust",
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        let defs = parsed["definitions"].as_array().unwrap();
        assert!(!defs.is_empty(), "Should find definitions matching 'main'");
        for d in defs {
            assert!(
                d["definition"].as_str().unwrap_or("").contains("main"),
                "Each definition should contain 'main'"
            );
        }
    }

    #[tokio::test]
    async fn test_search_pattern() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t4".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "pattern": "println!",
                "path": dir.path().display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(
            parsed["total_matches"].as_u64().unwrap() > 0,
            "Should find println! matches"
        );
    }

    #[tokio::test]
    async fn test_search_with_glob() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t4b".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "pattern": "def ",
                "path": dir.path().display().to_string(),
                "glob": "*.py",
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(
            parsed["total_matches"].as_u64().unwrap() > 0,
            "Should find 'def' in Python files"
        );
    }

    #[tokio::test]
    async fn test_analyze_imports_rust() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t5".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "analyze_imports",
                "file_path": dir.path().join("src/main.rs").display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["language"].as_str().unwrap(), "rust");
        let imports = parsed["imports"].as_array().unwrap();
        assert!(imports.len() >= 2, "Should find at least 2 use statements");

        let import_strs: Vec<&str> = imports
            .iter()
            .filter_map(|i| i["import"].as_str())
            .collect();
        assert!(
            import_strs.iter().any(|s| s.contains("std::io")),
            "Should find std::io import"
        );
    }

    #[tokio::test]
    async fn test_analyze_imports_python() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t5b".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "analyze_imports",
                "file_path": dir.path().join("script.py").display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["language"].as_str().unwrap(), "python");
        let imports = parsed["imports"].as_array().unwrap();
        assert!(
            imports.len() >= 2,
            "Should find at least 2 import statements"
        );
    }

    #[tokio::test]
    async fn test_find_references() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t6".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "find_references",
                "name": "Config",
                "path": dir.path().display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(
            parsed["total_references"].as_u64().unwrap() >= 2,
            "Should find at least 2 references to 'Config' (struct + impl)"
        );
    }

    #[tokio::test]
    async fn test_file_info() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let main_path = dir.path().join("src/main.rs");
        let call = ToolCall {
            id: "t7".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "file_info",
                "path": main_path.display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Error: {}", result.content);

        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["language"].as_str().unwrap(), "rust");
        assert_eq!(parsed["extension"].as_str().unwrap(), "rs");
        assert!(parsed["lines"].as_u64().unwrap() > 0);
        assert!(parsed["size_bytes"].as_u64().unwrap() > 0);
        assert!(!parsed["is_directory"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_unknown_operation() {
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t_err".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "nonexistent",
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_search_invalid_regex() {
        let dir = setup_temp_project();
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t_regex".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "search",
                "pattern": "[invalid",
                "path": dir.path().display().to_string(),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_nonexistent_path() {
        let skill = CodeAnalysisSkill::new();
        let call = ToolCall {
            id: "t_nopath".to_string(),
            name: "code_analysis".to_string(),
            arguments: serde_json::json!({
                "operation": "count_loc",
                "path": "/tmp/argentor_nonexistent_dir_99999",
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("does not exist"));
    }

    #[test]
    fn test_matches_glob() {
        assert!(matches_glob("main.rs", "*.rs"));
        assert!(matches_glob("test.py", "*.py"));
        assert!(!matches_glob("main.rs", "*.py"));
        assert!(matches_glob("anything", "*"));
        assert!(matches_glob("exact.txt", "exact.txt"));
        assert!(!matches_glob("other.txt", "exact.txt"));
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language(Path::new("main.rs")), "rust");
        assert_eq!(detect_language(Path::new("script.py")), "python");
        assert_eq!(detect_language(Path::new("app.ts")), "typescript");
        assert_eq!(detect_language(Path::new("main.go")), "go");
        assert_eq!(detect_language(Path::new("style.css")), "css");
        assert_eq!(detect_language(Path::new("noext")), "unknown");
    }

    #[test]
    fn test_is_comment() {
        assert!(is_comment("  // a comment", "rust"));
        assert!(is_comment("  # a comment", "python"));
        assert!(is_comment("  // a comment", "javascript"));
        assert!(is_comment("  -- a comment", "sql"));
        assert!(!is_comment("  let x = 1;", "rust"));
        assert!(!is_comment("  x = 1", "python"));
    }

    #[test]
    fn test_descriptor() {
        let skill = CodeAnalysisSkill::new();
        assert_eq!(skill.descriptor().name, "code_analysis");
    }
}

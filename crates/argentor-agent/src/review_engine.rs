//! Multi-dimensional code review engine.
//!
//! Analyzes code changes (diffs or full files) across multiple dimensions:
//! correctness, security, performance, style, test coverage, documentation,
//! and error handling. Produces specific, actionable feedback with file and
//! line references.
//!
//! # Main types
//!
//! - [`ReviewEngine`] — Runs checks across all configured dimensions.
//! - [`ReviewFinding`] — A specific issue found during review with location and suggestion.
//! - [`ReviewReport`] — Complete report with findings, scores, verdict, and summary.
//! - [`ReviewConfig`] — Configuration for thresholds, dimensions, and limits.
//! - [`ReviewDimension`] — The axis along which code is evaluated.
//! - [`FindingSeverity`] — How severe a finding is (Info, Warning, Error, Critical).
//! - [`ReviewVerdict`] — Overall decision: Approve, RequestChanges, or Block.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Dimensions along which code is reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewDimension {
    /// Logical correctness of the code.
    Correctness,
    /// Security vulnerabilities and risks.
    Security,
    /// Performance issues and inefficiencies.
    Performance,
    /// Code style and readability.
    Style,
    /// Test coverage indicators.
    TestCoverage,
    /// Documentation completeness.
    Documentation,
    /// Error handling quality.
    ErrorHandling,
}

impl ReviewDimension {
    /// Return all review dimensions in the canonical order.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Correctness,
            Self::Security,
            Self::Performance,
            Self::Style,
            Self::TestCoverage,
            Self::Documentation,
            Self::ErrorHandling,
        ]
    }
}

impl std::fmt::Display for ReviewDimension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Correctness => "Correctness",
            Self::Security => "Security",
            Self::Performance => "Performance",
            Self::Style => "Style",
            Self::TestCoverage => "Test Coverage",
            Self::Documentation => "Documentation",
            Self::ErrorHandling => "Error Handling",
        };
        write!(f, "{label}")
    }
}

/// Severity of a review finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FindingSeverity {
    /// Informational note, no action needed.
    Info,
    /// Minor issue that should be addressed.
    Warning,
    /// Significant issue that must be fixed.
    Error,
    /// Critical issue that blocks merging.
    Critical,
}

impl std::fmt::Display for FindingSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        };
        write!(f, "{label}")
    }
}

/// Overall review verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewVerdict {
    /// Code is good to merge.
    Approve,
    /// Minor issues that should be addressed.
    RequestChanges,
    /// Critical issues that block merging.
    Block,
}

impl std::fmt::Display for ReviewVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Approve => "Approve",
            Self::RequestChanges => "Request Changes",
            Self::Block => "Block",
        };
        write!(f, "{label}")
    }
}

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

/// A specific review finding with location, message, and optional suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// Which dimension this finding relates to.
    pub dimension: ReviewDimension,
    /// Severity of the finding.
    pub severity: FindingSeverity,
    /// File path where the issue was found.
    pub file: String,
    /// Line number (1-based) where the issue was found, if applicable.
    pub line: Option<usize>,
    /// Human-readable description of the issue.
    pub message: String,
    /// Suggested fix, if available.
    pub suggestion: Option<String>,
    /// Machine-readable rule identifier (e.g. `SEC001`, `PERF003`).
    pub rule_id: String,
}

/// Complete review report for a set of changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    /// Overall verdict for the reviewed code.
    pub verdict: ReviewVerdict,
    /// Individual findings across all dimensions.
    pub findings: Vec<ReviewFinding>,
    /// Human-readable summary of the review.
    pub summary: String,
    /// Per-dimension scores.
    pub scores: Vec<DimensionScore>,
    /// Aggregate score across all dimensions (0.0 - 1.0).
    pub total_score: f32,
}

/// Score for a single review dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionScore {
    /// Which dimension this score belongs to.
    pub dimension: ReviewDimension,
    /// Score for this dimension (0.0 = terrible, 1.0 = perfect).
    pub score: f32,
    /// Weight of this dimension in the total score calculation.
    pub weight: f32,
    /// Number of findings in this dimension.
    pub finding_count: usize,
}

/// Configuration for the review engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// Which dimensions to check (default: all).
    pub dimensions: Vec<ReviewDimension>,
    /// Minimum score to approve (default: 0.7).
    pub approve_threshold: f32,
    /// Minimum score to avoid blocking (default: 0.4).
    pub block_threshold: f32,
    /// Maximum number of findings to report (default: 50).
    pub max_findings: usize,
    /// Language hint for language-specific checks.
    pub language: Option<String>,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            dimensions: ReviewDimension::all(),
            approve_threshold: 0.7,
            block_threshold: 0.4,
            max_findings: 50,
            language: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Multi-dimensional code review engine.
///
/// Analyzes source code across configurable dimensions (security, performance,
/// style, etc.) and produces a [`ReviewReport`] with specific findings,
/// per-dimension scores, and an overall verdict.
///
/// # Example
///
/// ```rust
/// use argentor_agent::review_engine::{ReviewEngine, ReviewDimension};
///
/// let engine = ReviewEngine::new();
/// let report = engine.review_code("src/main.rs", "fn main() {}");
/// println!("Verdict: {}", report.verdict);
/// ```
pub struct ReviewEngine {
    /// Configuration controlling which dimensions are checked and thresholds.
    config: ReviewConfig,
}

impl ReviewEngine {
    /// Create a new review engine with default configuration.
    pub fn new() -> Self {
        Self {
            config: ReviewConfig::default(),
        }
    }

    /// Create a new review engine with the given configuration.
    pub fn with_config(config: ReviewConfig) -> Self {
        Self { config }
    }

    /// Review code content and produce a full report.
    ///
    /// Runs all configured dimension checks against the provided source,
    /// computes per-dimension scores, determines the verdict, and returns
    /// a complete [`ReviewReport`].
    pub fn review_code(&self, file: &str, content: &str) -> ReviewReport {
        let mut findings = self.collect_findings(file, content);
        findings.truncate(self.config.max_findings);

        let (scores, total_score) = self.compute_scores(&findings);
        let verdict = self.determine_verdict(total_score, &findings);
        let summary = self.generate_summary(&findings, &verdict);

        ReviewReport {
            verdict,
            findings,
            summary,
            scores,
            total_score,
        }
    }

    /// Review a diff (old content vs new content) focusing on changed lines.
    ///
    /// Identifies lines that were added or modified in `new` compared to `old`,
    /// then runs all checks only on those changed lines for focused feedback.
    pub fn review_changes(&self, file: &str, old: &str, new: &str) -> ReviewReport {
        let changed_lines = diff_changed_lines(old, new);
        let mut findings = self.collect_findings(file, new);

        // Keep only findings on changed lines (or findings without a line number).
        findings.retain(|f| match f.line {
            Some(line) => changed_lines.contains(&line),
            None => true,
        });
        findings.truncate(self.config.max_findings);

        let (scores, total_score) = self.compute_scores(&findings);
        let verdict = self.determine_verdict(total_score, &findings);
        let summary = self.generate_summary(&findings, &verdict);

        ReviewReport {
            verdict,
            findings,
            summary,
            scores,
            total_score,
        }
    }

    /// Generate an LLM prompt for deeper review of specific code.
    ///
    /// Builds a structured prompt asking an LLM to perform an in-depth review
    /// of the given code with a particular [`ReviewDimension`] focus.
    pub fn deep_review_prompt(&self, file: &str, content: &str, focus: ReviewDimension) -> String {
        format!(
            "You are a senior code reviewer. Perform an in-depth review of the following code \
             with a focus on **{focus}**.\n\n\
             File: `{file}`\n\n\
             ```\n{content}\n```\n\n\
             For each issue found, provide:\n\
             1. The line number\n\
             2. A description of the problem\n\
             3. A concrete suggestion to fix it\n\
             4. Severity: info / warning / error / critical\n\n\
             If the code looks good for this dimension, say so explicitly.\n\
             Respond in JSON array format: \
             [{{\"line\": N, \"message\": \"...\", \"suggestion\": \"...\", \"severity\": \"...\"}}]"
        )
    }

    /// Format the review report as a markdown comment suitable for PR reviews.
    pub fn format_as_markdown(&self, report: &ReviewReport) -> String {
        let mut md = String::new();

        // Header with verdict
        let verdict_icon = match report.verdict {
            ReviewVerdict::Approve => "APPROVED",
            ReviewVerdict::RequestChanges => "CHANGES REQUESTED",
            ReviewVerdict::Block => "BLOCKED",
        };
        md.push_str(&format!("## Code Review: {verdict_icon}\n\n"));

        // Summary
        md.push_str(&format!("{}\n\n", report.summary));

        // Scores table
        md.push_str("### Dimension Scores\n\n");
        md.push_str("| Dimension | Score | Weight | Findings |\n");
        md.push_str("|-----------|-------|--------|----------|\n");
        for ds in &report.scores {
            md.push_str(&format!(
                "| {} | {:.0}% | {:.0}% | {} |\n",
                ds.dimension,
                ds.score * 100.0,
                ds.weight * 100.0,
                ds.finding_count,
            ));
        }
        md.push_str(&format!(
            "\n**Total Score: {:.0}%**\n\n",
            report.total_score * 100.0
        ));

        // Findings grouped by severity
        if !report.findings.is_empty() {
            md.push_str("### Findings\n\n");

            let mut by_severity: Vec<&ReviewFinding> = report.findings.iter().collect();
            by_severity.sort_by_key(|entry| std::cmp::Reverse(entry.severity));

            for finding in &by_severity {
                let location = match finding.line {
                    Some(line) => format!("{}:{}", finding.file, line),
                    None => finding.file.clone(),
                };
                let severity_label = match finding.severity {
                    FindingSeverity::Critical => "**CRITICAL**",
                    FindingSeverity::Error => "**ERROR**",
                    FindingSeverity::Warning => "WARNING",
                    FindingSeverity::Info => "info",
                };
                md.push_str(&format!(
                    "- [{severity_label}] `{location}` ({}) {}\n",
                    finding.rule_id, finding.message,
                ));
                if let Some(suggestion) = &finding.suggestion {
                    md.push_str(&format!("  > Suggestion: {suggestion}\n"));
                }
            }
        }

        md
    }

    // -----------------------------------------------------------------------
    // Dimension checkers
    // -----------------------------------------------------------------------

    /// Run security checks on code.
    fn check_security(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();
        let is_test_file = file.contains("test") || file.contains("spec");
        let secret_re = Regex::new(r#"(?i)(password|api_key|secret|token)\s*=\s*""#).ok();
        let ip_re = Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").ok();

        for (line_num, line) in content.lines().enumerate() {
            let ln = line_num + 1;

            // SEC001: Hardcoded secrets
            if !is_test_file {
                if let Some(re) = &secret_re {
                    if re.is_match(line) {
                        findings.push(ReviewFinding {
                            dimension: ReviewDimension::Security,
                            severity: FindingSeverity::Critical,
                            file: file.to_string(),
                            line: Some(ln),
                            message: "Hardcoded secret detected. Credentials should be loaded \
                                      from environment variables or a secrets manager."
                                .to_string(),
                            suggestion: Some(
                                "Use std::env::var(\"KEY\") or a config file excluded from VCS."
                                    .to_string(),
                            ),
                            rule_id: "SEC001".to_string(),
                        });
                    }
                }
            }

            // SEC002: SQL injection
            if line.contains("format!(\"SELECT")
                || line.contains("format!(\"INSERT")
                || line.contains("format!(\"UPDATE")
                || line.contains("format!(\"DELETE")
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Security,
                    severity: FindingSeverity::Critical,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Potential SQL injection via string formatting. \
                              Use parameterized queries instead."
                        .to_string(),
                    suggestion: Some(
                        "Use query builder or prepared statements with bind parameters."
                            .to_string(),
                    ),
                    rule_id: "SEC002".to_string(),
                });
            }

            // SEC003: Path traversal
            if (line.contains("Path::new")
                || line.contains("open(")
                || line.contains("read_to_string("))
                && line.contains("..")
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Security,
                    severity: FindingSeverity::Error,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Potential path traversal via '..' in file path operation."
                        .to_string(),
                    suggestion: Some(
                        "Canonicalize the path and verify it stays within the expected directory."
                            .to_string(),
                    ),
                    rule_id: "SEC003".to_string(),
                });
            }

            // SEC004: Shell injection
            if line.contains("Command::new") && line.contains("&format!") {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Security,
                    severity: FindingSeverity::Critical,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Potential shell injection: unsanitized input passed to Command::new."
                        .to_string(),
                    suggestion: Some(
                        "Validate and sanitize all user-supplied arguments. Prefer .arg() over shell expansion."
                            .to_string(),
                    ),
                    rule_id: "SEC004".to_string(),
                });
            }

            // SEC005: Unsafe blocks
            if line.contains("unsafe {") {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Security,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Unsafe block detected. Ensure safety invariants are documented \
                              and upheld."
                        .to_string(),
                    suggestion: Some(
                        "Add a // SAFETY: comment explaining why this unsafe block is sound."
                            .to_string(),
                    ),
                    rule_id: "SEC005".to_string(),
                });
            }

            // SEC006: Hardcoded IPs/URLs
            {
                if let Some(re) = &ip_re {
                    // Exclude 127.0.0.1 and 0.0.0.0 as they are common development addresses
                    if re.is_match(line) && !line.contains("127.0.0.1") && !line.contains("0.0.0.0")
                    {
                        findings.push(ReviewFinding {
                            dimension: ReviewDimension::Security,
                            severity: FindingSeverity::Warning,
                            file: file.to_string(),
                            line: Some(ln),
                            message: "Hardcoded IP address detected. Use configuration for network addresses."
                                .to_string(),
                            suggestion: Some(
                                "Move the address to a configuration file or environment variable."
                                    .to_string(),
                            ),
                            rule_id: "SEC006".to_string(),
                        });
                    }
                }
                if line.contains("http://") && !is_test_file {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Security,
                        severity: FindingSeverity::Warning,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "Hardcoded HTTP URL detected. Consider using HTTPS and \
                                  externalizing URLs to configuration."
                            .to_string(),
                        suggestion: Some(
                            "Use https:// and load URLs from configuration.".to_string(),
                        ),
                        rule_id: "SEC006".to_string(),
                    });
                }
            }

            // SEC007: Weak crypto
            {
                let lower = line.to_lowercase();
                if lower.contains("md5") || (lower.contains("sha1") && !lower.contains("sha1sum")) {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Security,
                        severity: FindingSeverity::Error,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "Weak cryptographic algorithm (MD5/SHA-1). \
                                  Use SHA-256 or stronger."
                            .to_string(),
                        suggestion: Some(
                            "Replace with SHA-256 (sha2 crate) or a modern hash function."
                                .to_string(),
                        ),
                        rule_id: "SEC007".to_string(),
                    });
                }
            }

            // SEC008: Sensitive data in logs
            {
                let lower = line.to_lowercase();
                let has_log = lower.contains("println!")
                    || lower.contains("log::")
                    || lower.contains("tracing::")
                    || lower.contains("info!")
                    || lower.contains("debug!")
                    || lower.contains("warn!")
                    || lower.contains("error!");
                let has_sensitive = lower.contains("password")
                    || lower.contains("secret")
                    || lower.contains("token")
                    || lower.contains("api_key");
                if has_log && has_sensitive {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Security,
                        severity: FindingSeverity::Error,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "Potentially logging sensitive data (password/secret/token)."
                            .to_string(),
                        suggestion: Some(
                            "Redact sensitive fields before logging. Use a structured logger with field masking."
                                .to_string(),
                        ),
                        rule_id: "SEC008".to_string(),
                    });
                }
            }
        }

        findings
    }

    /// Run performance checks on code.
    fn check_performance(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        // Track whether we are inside a loop or async fn for context-aware checks.
        let mut loop_depth: u32 = 0;
        let mut in_async_fn = false;

        for (line_num, line) in lines.iter().enumerate() {
            let ln = line_num + 1;
            let trimmed = line.trim();

            // Track loops
            if trimmed.starts_with("for ")
                || trimmed.starts_with("while ")
                || trimmed.starts_with("loop {")
                || trimmed.starts_with("loop{")
            {
                loop_depth += 1;
            }
            // Track async fns
            if trimmed.contains("async fn ") {
                in_async_fn = true;
            }
            // Track block exits (simplified: count closing braces)
            if trimmed == "}" {
                loop_depth = loop_depth.saturating_sub(1);
                // Reset async tracking on top-level brace close
                if loop_depth == 0 {
                    in_async_fn = false;
                }
            }

            // PERF001: Clone in loop
            if loop_depth > 0 && line.contains(".clone()") {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Performance,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "`.clone()` called inside a loop. Consider borrowing or \
                              moving the value outside the loop."
                        .to_string(),
                    suggestion: Some(
                        "Use a reference instead of cloning, or clone once before the loop."
                            .to_string(),
                    ),
                    rule_id: "PERF001".to_string(),
                });
            }

            // PERF002: Unnecessary allocation patterns
            if line.contains("String::new()") && line.contains("push_str") {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Performance,
                    severity: FindingSeverity::Info,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Consider using `format!()` or `String::with_capacity()` \
                              instead of `String::new()` + `push_str()`."
                        .to_string(),
                    suggestion: Some(
                        "Use format!() for known patterns or pre-allocate with String::with_capacity()."
                            .to_string(),
                    ),
                    rule_id: "PERF002".to_string(),
                });
            }

            // PERF003: Blocking in async
            if in_async_fn {
                if line.contains("std::thread::sleep") {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Performance,
                        severity: FindingSeverity::Error,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "Blocking `std::thread::sleep` in async context. \
                                  This blocks the entire executor thread."
                            .to_string(),
                        suggestion: Some("Use `tokio::time::sleep` instead.".to_string()),
                        rule_id: "PERF003".to_string(),
                    });
                }
                if line.contains("std::fs::") {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Performance,
                        severity: FindingSeverity::Warning,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "Blocking `std::fs` operation in async context. \
                                  This can stall the async runtime."
                            .to_string(),
                        suggestion: Some("Use `tokio::fs` for async file operations.".to_string()),
                        rule_id: "PERF003".to_string(),
                    });
                }
            }

            // PERF004: N+1 query pattern (database call inside loop)
            if loop_depth > 0
                && (line.contains(".query(")
                    || line.contains(".execute(")
                    || line.contains("sqlx::query"))
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Performance,
                    severity: FindingSeverity::Error,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Database query inside a loop (N+1 query pattern). \
                              Consider batching or joining."
                        .to_string(),
                    suggestion: Some(
                        "Use a single query with WHERE ... IN (...) or a JOIN instead of \
                         querying in a loop."
                            .to_string(),
                    ),
                    rule_id: "PERF004".to_string(),
                });
            }

            // PERF005: Large struct on stack (heuristic: struct definition with many fields)
            if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
                // Count fields by looking ahead for lines until the closing brace.
                let mut field_count = 0;
                for ahead in &lines[line_num + 1..] {
                    let atrimmed = ahead.trim();
                    if atrimmed == "}" {
                        break;
                    }
                    if atrimmed.contains(':') && !atrimmed.starts_with("//") {
                        field_count += 1;
                    }
                }
                if field_count > 12 {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Performance,
                        severity: FindingSeverity::Info,
                        file: file.to_string(),
                        line: Some(ln),
                        message: format!(
                            "Large struct with {field_count} fields. Consider boxing or \
                             splitting into smaller structs."
                        ),
                        suggestion: Some(
                            "Use Box<T> for large fields or decompose into sub-structs."
                                .to_string(),
                        ),
                        rule_id: "PERF005".to_string(),
                    });
                }
            }
        }

        findings
    }

    /// Run style checks on code.
    fn check_style(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let magic_re = Regex::new(r"\b(\d+)\b").ok();

        // STY001: Function too long (>50 lines)
        let mut fn_start: Option<(usize, String)> = None;
        let mut brace_depth: i32 = 0;

        for (line_num, line) in lines.iter().enumerate() {
            let ln = line_num + 1;
            let trimmed = line.trim();

            // Detect function start
            if (trimmed.contains("fn ") && trimmed.contains('('))
                && !trimmed.starts_with("//")
                && fn_start.is_none()
            {
                let name = extract_fn_name(trimmed).unwrap_or_else(|| "unknown".to_string());
                fn_start = Some((ln, name));
                brace_depth = 0;
            }

            if fn_start.is_some() {
                for ch in line.chars() {
                    match ch {
                        '{' => brace_depth += 1,
                        '}' => brace_depth -= 1,
                        _ => {}
                    }
                }
                if brace_depth == 0 && fn_start.is_some() {
                    if let Some((start_ln, fn_name)) = fn_start.take() {
                        let fn_len = ln - start_ln + 1;
                        if fn_len > 50 {
                            findings.push(ReviewFinding {
                                dimension: ReviewDimension::Style,
                                severity: FindingSeverity::Warning,
                                file: file.to_string(),
                                line: Some(start_ln),
                                message: format!(
                                    "Function `{fn_name}` is {fn_len} lines long (limit: 50). \
                                     Consider splitting into smaller functions."
                                ),
                                suggestion: Some(
                                    "Extract logical blocks into helper functions.".to_string(),
                                ),
                                rule_id: "STY001".to_string(),
                            });
                        }
                    }
                }
            }

            // STY002: Too many parameters (>5)
            if (trimmed.contains("fn ") && trimmed.contains('(')) && !trimmed.starts_with("//") {
                let param_count = count_fn_params(trimmed);
                if param_count > 5 {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Style,
                        severity: FindingSeverity::Warning,
                        file: file.to_string(),
                        line: Some(ln),
                        message: format!(
                            "Function has {param_count} parameters (limit: 5). \
                             Consider using a config struct."
                        ),
                        suggestion: Some("Group related parameters into a struct.".to_string()),
                        rule_id: "STY002".to_string(),
                    });
                }
            }

            // STY003: Deep nesting (>4 levels)
            {
                let indent_spaces = line.len() - line.trim_start().len();
                let indent_level = indent_spaces / 4;
                if indent_level > 4 && !trimmed.is_empty() && !trimmed.starts_with("//") {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Style,
                        severity: FindingSeverity::Warning,
                        file: file.to_string(),
                        line: Some(ln),
                        message: format!(
                            "Deep nesting detected ({indent_level} levels). \
                             Consider early returns or extracting functions."
                        ),
                        suggestion: Some(
                            "Use early returns, guard clauses, or extract nested logic into \
                             separate functions."
                                .to_string(),
                        ),
                        rule_id: "STY003".to_string(),
                    });
                }
            }

            // STY004: Magic numbers
            if !trimmed.starts_with("//") && !trimmed.starts_with("///") && !trimmed.is_empty() {
                if let Some(re) = &magic_re {
                    for cap in re.captures_iter(trimmed) {
                        if let Some(m) = cap.get(1) {
                            let num_str = m.as_str();
                            // Skip 0, 1, 2, and line numbers in array indices
                            if let Ok(num) = num_str.parse::<i64>() {
                                if num > 2
                                    && !trimmed.contains("const ")
                                    && !trimmed.contains("static ")
                                    && !trimmed.contains("enum ")
                                    && !trimmed.starts_with('#')
                                {
                                    findings.push(ReviewFinding {
                                        dimension: ReviewDimension::Style,
                                        severity: FindingSeverity::Info,
                                        file: file.to_string(),
                                        line: Some(ln),
                                        message: format!(
                                            "Magic number `{num}` — consider defining a named constant."
                                        ),
                                        suggestion: Some(
                                            "Define a `const` with a descriptive name.".to_string(),
                                        ),
                                        rule_id: "STY004".to_string(),
                                    });
                                    break; // One finding per line is enough
                                }
                            }
                        }
                    }
                }
            }

            // STY005: Missing error context — bare `?` without `.map_err()` or context
            if trimmed.ends_with('?')
                && !trimmed.contains("map_err")
                && !trimmed.contains("context(")
                && !trimmed.contains("with_context(")
                && !trimmed.starts_with("//")
            {
                // Only flag if it looks like a function call result
                if trimmed.contains('(') || trimmed.contains(".await") {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Style,
                        severity: FindingSeverity::Info,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "Bare `?` without error context. Consider adding `.map_err()` \
                                  or `.context()` to provide actionable error messages."
                            .to_string(),
                        suggestion: Some(
                            "Add .map_err(|e| ...) or use anyhow/thiserror for context."
                                .to_string(),
                        ),
                        rule_id: "STY005".to_string(),
                    });
                }
            }

            // STY006: TODO/FIXME/HACK comments
            {
                let upper = trimmed.to_uppercase();
                if upper.contains("TODO") || upper.contains("FIXME") || upper.contains("HACK") {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Style,
                        severity: FindingSeverity::Info,
                        file: file.to_string(),
                        line: Some(ln),
                        message: "TODO/FIXME/HACK comment found. Track this in an issue tracker."
                            .to_string(),
                        suggestion: Some(
                            "Create an issue to track this work and reference the issue number \
                             in the comment."
                                .to_string(),
                        ),
                        rule_id: "STY006".to_string(),
                    });
                }
            }
        }

        findings
    }

    /// Run error handling checks on code.
    fn check_error_handling(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();
        let is_test = is_test_code(content);

        for (line_num, line) in content.lines().enumerate() {
            let ln = line_num + 1;
            let trimmed = line.trim();

            // Skip comments
            if trimmed.starts_with("//") {
                continue;
            }

            // ERR001: unwrap() in production code
            if !is_test && line.contains(".unwrap()") {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::ErrorHandling,
                    severity: FindingSeverity::Error,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "`.unwrap()` in production code. This will panic on `None`/`Err`."
                        .to_string(),
                    suggestion: Some(
                        "Use `?`, `.unwrap_or()`, `.unwrap_or_default()`, or pattern matching."
                            .to_string(),
                    ),
                    rule_id: "ERR001".to_string(),
                });
            }

            // ERR002: expect() in production code
            if !is_test && line.contains(".expect(") {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::ErrorHandling,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message:
                        "`.expect()` in production code. Prefer returning errors over panicking."
                            .to_string(),
                    suggestion: Some(
                        "Use `?` with a custom error type or `.map_err()` for context.".to_string(),
                    ),
                    rule_id: "ERR002".to_string(),
                });
            }

            // ERR003: Swallowed errors
            if trimmed.starts_with("let _ =") && (trimmed.contains('(') || trimmed.contains('.')) {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::ErrorHandling,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Swallowed error: `let _ = ...` discards the Result. \
                              At minimum, log the error."
                        .to_string(),
                    suggestion: Some(
                        "Use `if let Err(e) = ... { tracing::warn!(...) }` to log failures."
                            .to_string(),
                    ),
                    rule_id: "ERR003".to_string(),
                });
            }

            // ERR004: Panic in library code
            if trimmed.contains("panic!(")
                || trimmed.contains("todo!(")
                || (trimmed.contains("unreachable!(") && !is_test)
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::ErrorHandling,
                    severity: FindingSeverity::Error,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Panic-inducing macro in library code. Return an error instead."
                        .to_string(),
                    suggestion: Some(
                        "Return `Err(...)` instead of panicking. Reserve panics for truly \
                         unrecoverable situations."
                            .to_string(),
                    ),
                    rule_id: "ERR004".to_string(),
                });
            }

            // ERR005: Empty catch/match arms
            if (trimmed.contains("Err(_) => {}") || trimmed.contains("Err(_) => ()"))
                || (trimmed.contains("_ => {}") && trimmed.contains("Err"))
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::ErrorHandling,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Empty error match arm silently swallows the error.".to_string(),
                    suggestion: Some(
                        "At minimum, log the error or add a comment explaining why it is safe \
                         to ignore."
                            .to_string(),
                    ),
                    rule_id: "ERR005".to_string(),
                });
            }
        }

        findings
    }

    /// Run correctness checks on code.
    fn check_correctness(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();

        for (line_num, line) in content.lines().enumerate() {
            let ln = line_num + 1;
            let trimmed = line.trim();

            if trimmed.starts_with("//") {
                continue;
            }

            // Off-by-one indicators: `<= len`, `< len - 1` in array indexing
            if (trimmed.contains("<= len") || trimmed.contains("<= .len()"))
                && (trimmed.contains('[') || trimmed.contains("index"))
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Correctness,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Possible off-by-one error: `<= len` can cause index out of bounds."
                        .to_string(),
                    suggestion: Some("Use `< len` for zero-based indexing.".to_string()),
                    rule_id: "COR001".to_string(),
                });
            }

            // Integer overflow risk: casting between numeric types
            if (trimmed.contains(" as u") || trimmed.contains(" as i"))
                && (trimmed.contains(" as u8")
                    || trimmed.contains(" as i8")
                    || trimmed.contains(" as u16")
                    || trimmed.contains(" as i16"))
            {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Correctness,
                    severity: FindingSeverity::Warning,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Narrowing numeric cast may silently truncate the value.".to_string(),
                    suggestion: Some(
                        "Use `try_into()` or `TryFrom` to handle overflow explicitly.".to_string(),
                    ),
                    rule_id: "COR002".to_string(),
                });
            }

            // Deadlock risk: nested lock acquisition
            if trimmed.contains(".lock()") && trimmed.matches(".lock()").count() > 1 {
                findings.push(ReviewFinding {
                    dimension: ReviewDimension::Correctness,
                    severity: FindingSeverity::Error,
                    file: file.to_string(),
                    line: Some(ln),
                    message: "Multiple locks acquired on the same line — potential deadlock."
                        .to_string(),
                    suggestion: Some(
                        "Acquire locks in a consistent order and minimize lock scope.".to_string(),
                    ),
                    rule_id: "COR003".to_string(),
                });
            }
        }

        findings
    }

    /// Check test coverage indicators.
    fn check_test_coverage(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();

        // Only check non-test source files
        if file.contains("test") || file.contains("spec") {
            return findings;
        }

        let has_tests = content.contains("#[cfg(test)]") || content.contains("#[test]");
        let has_pub_fns = content.contains("pub fn ");

        if has_pub_fns && !has_tests {
            findings.push(ReviewFinding {
                dimension: ReviewDimension::TestCoverage,
                severity: FindingSeverity::Warning,
                file: file.to_string(),
                line: None,
                message: "File has public functions but no test module. \
                          Consider adding unit tests."
                    .to_string(),
                suggestion: Some(
                    "Add a `#[cfg(test)] mod tests { ... }` module with tests for public \
                     functions."
                        .to_string(),
                ),
                rule_id: "TST001".to_string(),
            });
        }

        // Check for pub fns that don't have corresponding test mentions
        let pub_fn_re = Regex::new(r"pub fn (\w+)").ok();
        if let Some(re) = &pub_fn_re {
            let test_section = content
                .find("#[cfg(test)]")
                .map(|pos| &content[pos..])
                .unwrap_or("");

            for cap in re.captures_iter(content) {
                if let Some(name) = cap.get(1) {
                    let fn_name = name.as_str();
                    // Skip common names that don't need individual tests
                    if fn_name == "new"
                        || fn_name == "default"
                        || fn_name == "fmt"
                        || fn_name == "from"
                    {
                        continue;
                    }
                    if !test_section.is_empty() && !test_section.contains(fn_name) {
                        findings.push(ReviewFinding {
                            dimension: ReviewDimension::TestCoverage,
                            severity: FindingSeverity::Info,
                            file: file.to_string(),
                            line: None,
                            message: format!(
                                "Public function `{fn_name}` does not appear to be tested."
                            ),
                            suggestion: Some(format!(
                                "Add a test: `#[test] fn test_{fn_name}() {{ ... }}`"
                            )),
                            rule_id: "TST002".to_string(),
                        });
                    }
                }
            }
        }

        findings
    }

    /// Check documentation quality.
    fn check_documentation(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        // DOC003: Module without module-level doc comment
        let has_module_doc = lines.iter().any(|l| l.trim().starts_with("//!"));
        if !has_module_doc {
            findings.push(ReviewFinding {
                dimension: ReviewDimension::Documentation,
                severity: FindingSeverity::Info,
                file: file.to_string(),
                line: Some(1),
                message: "File is missing a module-level doc comment (`//!`).".to_string(),
                suggestion: Some(
                    "Add `//! Brief description of this module.` at the top of the file."
                        .to_string(),
                ),
                rule_id: "DOC003".to_string(),
            });
        }

        for (line_num, line) in lines.iter().enumerate() {
            let ln = line_num + 1;
            let trimmed = line.trim();

            // DOC001: Public function without doc comment
            if trimmed.starts_with("pub fn ") || trimmed.starts_with("pub async fn ") {
                let has_doc = line_num > 0
                    && lines[..line_num]
                        .iter()
                        .rev()
                        .take_while(|l| {
                            let lt = l.trim();
                            lt.starts_with("///") || lt.starts_with("#[") || lt.is_empty()
                        })
                        .any(|l| l.trim().starts_with("///"));

                if !has_doc {
                    let fn_name = extract_fn_name(trimmed).unwrap_or_else(|| "unknown".to_string());
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Documentation,
                        severity: FindingSeverity::Warning,
                        file: file.to_string(),
                        line: Some(ln),
                        message: format!(
                            "Public function `{fn_name}` is missing a doc comment (`///`)."
                        ),
                        suggestion: Some(
                            "Add a `/// Brief description.` comment above the function."
                                .to_string(),
                        ),
                        rule_id: "DOC001".to_string(),
                    });
                }
            }

            // DOC002: Public struct without doc comment
            if trimmed.starts_with("pub struct ") || trimmed.starts_with("pub enum ") {
                let has_doc = line_num > 0
                    && lines[..line_num]
                        .iter()
                        .rev()
                        .take_while(|l| {
                            let lt = l.trim();
                            lt.starts_with("///") || lt.starts_with("#[") || lt.is_empty()
                        })
                        .any(|l| l.trim().starts_with("///"));

                if !has_doc {
                    findings.push(ReviewFinding {
                        dimension: ReviewDimension::Documentation,
                        severity: FindingSeverity::Warning,
                        file: file.to_string(),
                        line: Some(ln),
                        message: format!(
                            "Public type `{}` is missing a doc comment (`///`).",
                            trimmed
                                .trim_start_matches("pub struct ")
                                .trim_start_matches("pub enum ")
                                .split_whitespace()
                                .next()
                                .unwrap_or("unknown")
                        ),
                        suggestion: Some(
                            "Add a `/// Brief description.` comment above the type.".to_string(),
                        ),
                        rule_id: "DOC002".to_string(),
                    });
                }
            }
        }

        findings
    }

    // -----------------------------------------------------------------------
    // Scoring & verdict
    // -----------------------------------------------------------------------

    /// Compute per-dimension scores and the weighted total score from findings.
    ///
    /// Each dimension starts at 1.0 and loses points based on finding severity:
    /// - Info: -0.02
    /// - Warning: -0.05
    /// - Error: -0.15
    /// - Critical: -0.30
    ///
    /// Scores are clamped to `[0.0, 1.0]`.
    fn compute_scores(&self, findings: &[ReviewFinding]) -> (Vec<DimensionScore>, f32) {
        let weights: HashMap<ReviewDimension, f32> = [
            (ReviewDimension::Security, 0.25),
            (ReviewDimension::Correctness, 0.20),
            (ReviewDimension::ErrorHandling, 0.15),
            (ReviewDimension::Performance, 0.15),
            (ReviewDimension::Style, 0.10),
            (ReviewDimension::TestCoverage, 0.08),
            (ReviewDimension::Documentation, 0.07),
        ]
        .into_iter()
        .collect();

        let mut dimension_findings: HashMap<ReviewDimension, Vec<&ReviewFinding>> = HashMap::new();
        for finding in findings {
            dimension_findings
                .entry(finding.dimension)
                .or_default()
                .push(finding);
        }

        let mut scores = Vec::new();
        let mut total = 0.0_f32;

        for dim in &self.config.dimensions {
            let w = weights.get(dim).copied().unwrap_or(0.10);
            let dim_findings = dimension_findings
                .get(dim)
                .map_or(&[][..], |v| v.as_slice());

            let mut score = 1.0_f32;
            for f in dim_findings {
                let penalty = match f.severity {
                    FindingSeverity::Info => 0.02,
                    FindingSeverity::Warning => 0.05,
                    FindingSeverity::Error => 0.15,
                    FindingSeverity::Critical => 0.30,
                };
                score -= penalty;
            }
            score = score.clamp(0.0, 1.0);

            total += score * w;

            scores.push(DimensionScore {
                dimension: *dim,
                score,
                weight: w,
                finding_count: dim_findings.len(),
            });
        }

        (scores, total.clamp(0.0, 1.0))
    }

    /// Determine the review verdict based on total score and critical findings.
    fn determine_verdict(&self, total_score: f32, findings: &[ReviewFinding]) -> ReviewVerdict {
        let has_critical = findings
            .iter()
            .any(|f| f.severity == FindingSeverity::Critical);

        if has_critical || total_score < self.config.block_threshold {
            ReviewVerdict::Block
        } else if total_score < self.config.approve_threshold {
            ReviewVerdict::RequestChanges
        } else {
            ReviewVerdict::Approve
        }
    }

    /// Generate a human-readable summary from findings and verdict.
    fn generate_summary(&self, findings: &[ReviewFinding], verdict: &ReviewVerdict) -> String {
        let critical_count = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Critical)
            .count();
        let error_count = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Error)
            .count();
        let warning_count = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Warning)
            .count();
        let info_count = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Info)
            .count();

        let verdict_text = match verdict {
            ReviewVerdict::Approve => "Code looks good and is ready to merge.",
            ReviewVerdict::RequestChanges => {
                "Code has issues that should be addressed before merging."
            }
            ReviewVerdict::Block => {
                "Code has critical issues that must be resolved before merging."
            }
        };

        if findings.is_empty() {
            return format!("{verdict_text} No issues found.");
        }

        let mut parts = vec![verdict_text.to_string()];
        parts.push(format!(
            "Found {} issue(s): {} critical, {} error(s), {} warning(s), {} info.",
            findings.len(),
            critical_count,
            error_count,
            warning_count,
            info_count,
        ));

        // Mention the most important dimensions with issues
        let mut dim_counts: HashMap<ReviewDimension, usize> = HashMap::new();
        for f in findings {
            *dim_counts.entry(f.dimension).or_default() += 1;
        }
        let mut dim_list: Vec<(ReviewDimension, usize)> = dim_counts.into_iter().collect();
        dim_list.sort_by_key(|entry| std::cmp::Reverse(entry.1));

        if !dim_list.is_empty() {
            let top_dims: Vec<String> = dim_list
                .iter()
                .take(3)
                .map(|(d, c)| format!("{d} ({c})"))
                .collect();
            parts.push(format!("Top areas: {}.", top_dims.join(", ")));
        }

        parts.join(" ")
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Collect all findings from enabled dimension checks.
    fn collect_findings(&self, file: &str, content: &str) -> Vec<ReviewFinding> {
        let mut findings = Vec::new();
        let dims: HashSet<ReviewDimension> = self.config.dimensions.iter().copied().collect();

        if dims.contains(&ReviewDimension::Security) {
            findings.extend(self.check_security(file, content));
        }
        if dims.contains(&ReviewDimension::Performance) {
            findings.extend(self.check_performance(file, content));
        }
        if dims.contains(&ReviewDimension::Style) {
            findings.extend(self.check_style(file, content));
        }
        if dims.contains(&ReviewDimension::ErrorHandling) {
            findings.extend(self.check_error_handling(file, content));
        }
        if dims.contains(&ReviewDimension::Correctness) {
            findings.extend(self.check_correctness(file, content));
        }
        if dims.contains(&ReviewDimension::TestCoverage) {
            findings.extend(self.check_test_coverage(file, content));
        }
        if dims.contains(&ReviewDimension::Documentation) {
            findings.extend(self.check_documentation(file, content));
        }

        findings
    }
}

impl Default for ReviewEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

/// Compute the set of 1-based line numbers that differ between `old` and `new`.
fn diff_changed_lines(old: &str, new: &str) -> HashSet<usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut changed = HashSet::new();

    for (i, new_line) in new_lines.iter().enumerate() {
        let ln = i + 1;
        match old_lines.get(i) {
            Some(old_line) if old_line != new_line => {
                changed.insert(ln);
            }
            None => {
                // New line added
                changed.insert(ln);
            }
            _ => {}
        }
    }

    changed
}

/// Extract the function name from a line containing `fn name(`.
fn extract_fn_name(line: &str) -> Option<String> {
    let after_fn = line.split("fn ").nth(1)?;
    let name = after_fn.split('(').next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Count the number of parameters in a function signature line.
///
/// Handles `&self` and `&mut self` by not counting them.
fn count_fn_params(line: &str) -> usize {
    let Some(start) = line.find('(') else {
        return 0;
    };
    let Some(end) = line.rfind(')') else {
        return 0;
    };
    if start >= end {
        return 0;
    }
    let params = &line[start + 1..end];
    if params.trim().is_empty() {
        return 0;
    }
    params
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "&self" && *p != "&mut self" && *p != "self")
        .count()
}

/// Detect if we are inside a `#[cfg(test)]` module or test function.
fn is_test_code(content: &str) -> bool {
    content.contains("#[cfg(test)]") || content.contains("#[test]")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // --- Security checks ---

    #[test]
    fn test_security_hardcoded_secret() {
        let engine = ReviewEngine::new();
        let code = r#"
fn connect_to_db() {
    let password = "super_secret_123";
    let api_key = "sk-1234567890abcdef";
    let config = Config::load();
}
"#;
        let findings = engine.check_security("src/db.rs", code);
        let sec001: Vec<_> = findings.iter().filter(|f| f.rule_id == "SEC001").collect();
        assert!(
            sec001.len() >= 2,
            "Expected at least 2 SEC001 findings for hardcoded password and api_key, got {}",
            sec001.len()
        );
        assert_eq!(sec001[0].severity, FindingSeverity::Critical);
        assert!(sec001[0].message.contains("Hardcoded secret"));
    }

    #[test]
    fn test_security_sql_injection() {
        let engine = ReviewEngine::new();
        let code = r#"
fn get_user(name: &str) -> Result<User, Error> {
    let query = format!("SELECT * FROM users WHERE name = '{}'", name);
    db.execute(&query)?;
    Ok(user)
}
"#;
        let findings = engine.check_security("src/queries.rs", code);
        let sec002: Vec<_> = findings.iter().filter(|f| f.rule_id == "SEC002").collect();
        assert!(
            !sec002.is_empty(),
            "Expected SEC002 finding for SQL injection"
        );
        assert_eq!(sec002[0].severity, FindingSeverity::Critical);
    }

    #[test]
    fn test_security_path_traversal() {
        let engine = ReviewEngine::new();
        let code = r#"
fn read_config(user_path: &str) -> String {
    let path = Path::new(&format!("/data/{}", "../../etc/passwd"));
    std::fs::read_to_string(path).unwrap()
}
"#;
        let findings = engine.check_security("src/files.rs", code);
        let sec003: Vec<_> = findings.iter().filter(|f| f.rule_id == "SEC003").collect();
        assert!(
            !sec003.is_empty(),
            "Expected SEC003 finding for path traversal"
        );
        assert_eq!(sec003[0].severity, FindingSeverity::Error);
    }

    #[test]
    fn test_security_shell_injection() {
        let engine = ReviewEngine::new();
        let code = r#"
fn run_user_command(input: &str) {
    let output = Command::new(&format!("/bin/{}", input))
        .output()
        .expect("failed to execute");
}
"#;
        let findings = engine.check_security("src/exec.rs", code);
        let sec004: Vec<_> = findings.iter().filter(|f| f.rule_id == "SEC004").collect();
        assert!(
            !sec004.is_empty(),
            "Expected SEC004 finding for shell injection"
        );
        assert_eq!(sec004[0].severity, FindingSeverity::Critical);
    }

    #[test]
    fn test_security_unsafe_blocks() {
        let engine = ReviewEngine::new();
        let code = r#"
fn fast_copy(src: *const u8, dst: *mut u8, len: usize) {
    unsafe {
        std::ptr::copy_nonoverlapping(src, dst, len);
    }
}
"#;
        let findings = engine.check_security("src/mem.rs", code);
        let sec005: Vec<_> = findings.iter().filter(|f| f.rule_id == "SEC005").collect();
        assert!(
            !sec005.is_empty(),
            "Expected SEC005 finding for unsafe block"
        );
        assert_eq!(sec005[0].severity, FindingSeverity::Warning);
    }

    // --- Performance checks ---

    #[test]
    fn test_performance_clone_in_loop() {
        let engine = ReviewEngine::new();
        let code = r#"
fn process_items(items: &[Item], config: &Config) {
    for item in items {
        let cfg = config.clone();
        process(item, &cfg);
    }
}
"#;
        let findings = engine.check_performance("src/process.rs", code);
        let perf001: Vec<_> = findings.iter().filter(|f| f.rule_id == "PERF001").collect();
        assert!(
            !perf001.is_empty(),
            "Expected PERF001 finding for clone in loop"
        );
        assert_eq!(perf001[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn test_performance_blocking_in_async() {
        let engine = ReviewEngine::new();
        let code = r#"
async fn fetch_data(url: &str) -> Result<String, Error> {
    let response = reqwest::get(url).await?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let content = std::fs::read_to_string("cache.txt");
    Ok(response.text().await?)
}
"#;
        let findings = engine.check_performance("src/fetch.rs", code);
        let perf003: Vec<_> = findings.iter().filter(|f| f.rule_id == "PERF003").collect();
        assert!(
            perf003.len() >= 2,
            "Expected at least 2 PERF003 findings (sleep + fs), got {}",
            perf003.len()
        );
        // The sleep finding should be Error severity
        let sleep_finding = perf003
            .iter()
            .find(|f| f.message.contains("sleep"))
            .expect("Expected a finding about std::thread::sleep");
        assert_eq!(sleep_finding.severity, FindingSeverity::Error);
    }

    // --- Style checks ---

    #[test]
    fn test_style_function_too_long() {
        let engine = ReviewEngine::new();
        // Generate a function with 55 lines
        let mut code = String::from("fn very_long_function() {\n");
        for i in 0..53 {
            code.push_str(&format!("    let x{i} = {i};\n"));
        }
        code.push_str("}\n");

        let findings = engine.check_style("src/long.rs", &code);
        let sty001: Vec<_> = findings.iter().filter(|f| f.rule_id == "STY001").collect();
        assert!(
            !sty001.is_empty(),
            "Expected STY001 finding for function >50 lines"
        );
        assert!(sty001[0].message.contains("very_long_function"));
    }

    #[test]
    fn test_style_too_many_params() {
        let engine = ReviewEngine::new();
        let code = r#"
fn create_user(name: &str, email: &str, age: u32, role: Role, dept: &str, manager: &str) -> User {
    User::new()
}
"#;
        let findings = engine.check_style("src/user.rs", code);
        let sty002: Vec<_> = findings.iter().filter(|f| f.rule_id == "STY002").collect();
        assert!(
            !sty002.is_empty(),
            "Expected STY002 finding for >5 parameters"
        );
    }

    #[test]
    fn test_style_deep_nesting() {
        let engine = ReviewEngine::new();
        let code = "fn check() {\n\
                     \x20   if true {\n\
                     \x20       if true {\n\
                     \x20           if true {\n\
                     \x20               if true {\n\
                     \x20                   let deep = true;\n\
                     \x20               }\n\
                     \x20           }\n\
                     \x20       }\n\
                     \x20   }\n\
                     }\n";
        let findings = engine.check_style("src/nested.rs", code);
        let sty003: Vec<_> = findings.iter().filter(|f| f.rule_id == "STY003").collect();
        assert!(
            !sty003.is_empty(),
            "Expected STY003 finding for deep nesting"
        );
    }

    #[test]
    fn test_style_todo_comments() {
        let engine = ReviewEngine::new();
        let code = r#"
fn process() {
    // TODO: implement proper validation
    validate();
    // FIXME: this is broken on Windows
    run();
    // HACK: temporary workaround for issue #42
    hack();
}
"#;
        let findings = engine.check_style("src/wip.rs", code);
        let sty006: Vec<_> = findings.iter().filter(|f| f.rule_id == "STY006").collect();
        assert!(
            sty006.len() >= 3,
            "Expected at least 3 STY006 findings for TODO/FIXME/HACK, got {}",
            sty006.len()
        );
    }

    // --- Error handling checks ---

    #[test]
    fn test_error_handling_unwrap() {
        let engine = ReviewEngine::new();
        let code = r#"
fn load_config(path: &str) -> Config {
    let content = std::fs::read_to_string(path).unwrap();
    let config: Config = serde_json::from_str(&content).unwrap();
    config
}
"#;
        let findings = engine.check_error_handling("src/config.rs", code);
        let err001: Vec<_> = findings.iter().filter(|f| f.rule_id == "ERR001").collect();
        assert!(
            err001.len() >= 2,
            "Expected at least 2 ERR001 findings for .unwrap(), got {}",
            err001.len()
        );
        assert_eq!(err001[0].severity, FindingSeverity::Error);
    }

    #[test]
    fn test_error_handling_swallowed() {
        let engine = ReviewEngine::new();
        let code = r#"
fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = notify_admin("cleanup done");
}
"#;
        let findings = engine.check_error_handling("src/cleanup.rs", code);
        let err003: Vec<_> = findings.iter().filter(|f| f.rule_id == "ERR003").collect();
        assert!(
            !err003.is_empty(),
            "Expected ERR003 finding for swallowed error"
        );
        assert_eq!(err003[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn test_error_handling_panic() {
        let engine = ReviewEngine::new();
        let code = r#"
fn process(input: &str) -> Output {
    if input.is_empty() {
        panic!("input must not be empty");
    }
    todo!("implement actual processing")
}
"#;
        let findings = engine.check_error_handling("src/processor.rs", code);
        let err004: Vec<_> = findings.iter().filter(|f| f.rule_id == "ERR004").collect();
        assert!(
            err004.len() >= 2,
            "Expected at least 2 ERR004 findings for panic! and todo!, got {}",
            err004.len()
        );
        assert_eq!(err004[0].severity, FindingSeverity::Error);
    }

    // --- Documentation checks ---

    #[test]
    fn test_documentation_missing_pub_doc() {
        let engine = ReviewEngine::new();
        let code = r#"
pub fn undocumented_function() -> bool {
    true
}

/// This function is documented.
pub fn documented_function() -> bool {
    false
}

pub struct UndocumentedStruct {
    field: u32,
}
"#;
        let findings = engine.check_documentation("src/api.rs", code);
        let doc001: Vec<_> = findings.iter().filter(|f| f.rule_id == "DOC001").collect();
        assert!(
            !doc001.is_empty(),
            "Expected DOC001 finding for undocumented pub fn"
        );
        // The documented function should NOT generate a finding.
        // We check that the only DOC001 finding is for `undocumented_function` (line 2),
        // and there is no finding specifically for `documented_function` (line 7).
        assert_eq!(
            doc001.len(),
            1,
            "Expected exactly 1 DOC001 finding (only for undocumented_function)"
        );
        assert!(
            doc001[0].message.contains("undocumented_function"),
            "DOC001 finding should be for undocumented_function, got: {}",
            doc001[0].message
        );

        let doc002: Vec<_> = findings.iter().filter(|f| f.rule_id == "DOC002").collect();
        assert!(
            !doc002.is_empty(),
            "Expected DOC002 finding for undocumented pub struct"
        );
    }

    // --- Full review tests ---

    #[test]
    fn test_review_code_clean() {
        let engine = ReviewEngine::new();
        let code = r#"//! A well-documented module.

/// Add two numbers together.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Subtract b from a.
pub fn sub(a: i32, b: i32) -> i32 {
    a - b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(1, 2), 3);
    }

    #[test]
    fn test_sub() {
        assert_eq!(sub(3, 1), 2);
    }
}
"#;
        let report = engine.review_code("src/math.rs", code);
        assert!(
            report.total_score > 0.5,
            "Clean code should score well, got {}",
            report.total_score
        );
    }

    #[test]
    fn test_review_code_with_issues() {
        let engine = ReviewEngine::new();
        let code = r#"
fn insecure_query(user_input: &str) {
    let query = format!("SELECT * FROM users WHERE id = {}", user_input);
    let result = db.execute(&query).unwrap();
    let password = "admin123";
    println!("User password: {}", password);
    todo!("handle the result properly");
}
"#;
        let report = engine.review_code("src/bad.rs", code);
        assert!(
            !report.findings.is_empty(),
            "Code with issues should have findings"
        );
        assert!(
            report.total_score < 0.8,
            "Code with issues should have lower score, got {}",
            report.total_score
        );
        // Should find SQL injection, hardcoded secret, unwrap, todo, etc.
        let rule_ids: HashSet<&str> = report.findings.iter().map(|f| f.rule_id.as_str()).collect();
        assert!(rule_ids.contains("SEC002"), "Should detect SQL injection");
        assert!(rule_ids.contains("ERR001"), "Should detect unwrap");
    }

    #[test]
    fn test_review_changes() {
        let engine = ReviewEngine::new();
        let old = r#"//! Module doc.
/// Safe function.
pub fn safe() -> i32 {
    42
}
"#;
        let new = r#"//! Module doc.
/// Safe function.
pub fn safe() -> i32 {
    let x = risky_call().unwrap();
    x
}
"#;
        let report = engine.review_changes("src/lib.rs", old, new);
        // Only line 4 changed — findings should be on that line
        let unwrap_findings: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule_id == "ERR001")
            .collect();
        assert!(
            !unwrap_findings.is_empty(),
            "Should detect .unwrap() on the changed line"
        );
        // Verify the finding is on line 4
        assert_eq!(unwrap_findings[0].line, Some(4));
    }

    // --- Verdict tests ---

    #[test]
    fn test_verdict_approve() {
        let engine = ReviewEngine::new();
        let verdict = engine.determine_verdict(0.85, &[]);
        assert_eq!(verdict, ReviewVerdict::Approve);
    }

    #[test]
    fn test_verdict_request_changes() {
        let engine = ReviewEngine::new();
        let verdict = engine.determine_verdict(0.55, &[]);
        assert_eq!(verdict, ReviewVerdict::RequestChanges);
    }

    #[test]
    fn test_verdict_block() {
        let engine = ReviewEngine::new();
        // Block due to low score
        let verdict = engine.determine_verdict(0.3, &[]);
        assert_eq!(verdict, ReviewVerdict::Block);

        // Block due to critical finding even with high score
        let critical = ReviewFinding {
            dimension: ReviewDimension::Security,
            severity: FindingSeverity::Critical,
            file: "src/main.rs".to_string(),
            line: Some(10),
            message: "Hardcoded secret".to_string(),
            suggestion: None,
            rule_id: "SEC001".to_string(),
        };
        let verdict = engine.determine_verdict(0.9, &[critical]);
        assert_eq!(verdict, ReviewVerdict::Block);
    }

    // --- Scoring tests ---

    #[test]
    fn test_compute_scores() {
        let engine = ReviewEngine::new();
        let findings = vec![
            ReviewFinding {
                dimension: ReviewDimension::Security,
                severity: FindingSeverity::Critical,
                file: "src/main.rs".to_string(),
                line: Some(5),
                message: "Hardcoded password".to_string(),
                suggestion: None,
                rule_id: "SEC001".to_string(),
            },
            ReviewFinding {
                dimension: ReviewDimension::Style,
                severity: FindingSeverity::Info,
                file: "src/main.rs".to_string(),
                line: Some(10),
                message: "Magic number".to_string(),
                suggestion: None,
                rule_id: "STY004".to_string(),
            },
        ];

        let (scores, total) = engine.compute_scores(&findings);

        // Security should have lower score due to critical finding
        let sec_score = scores
            .iter()
            .find(|s| s.dimension == ReviewDimension::Security)
            .expect("Should have security score");
        assert!(
            sec_score.score < 1.0,
            "Security score should be penalized, got {}",
            sec_score.score
        );
        assert_eq!(sec_score.finding_count, 1);

        // Style should be slightly penalized for info finding
        let sty_score = scores
            .iter()
            .find(|s| s.dimension == ReviewDimension::Style)
            .expect("Should have style score");
        assert!(
            (sty_score.score - 0.98).abs() < 0.01,
            "Style score should be ~0.98 (info penalty 0.02), got {}",
            sty_score.score
        );

        // Total should be < 1.0 since we have findings
        assert!(total < 1.0, "Total should be < 1.0, got {total}");
        assert!(total > 0.0, "Total should be > 0.0, got {total}");
    }

    // --- Summary tests ---

    #[test]
    fn test_generate_summary() {
        let engine = ReviewEngine::new();
        let findings = vec![
            ReviewFinding {
                dimension: ReviewDimension::Security,
                severity: FindingSeverity::Critical,
                file: "f.rs".to_string(),
                line: Some(1),
                message: "bad".to_string(),
                suggestion: None,
                rule_id: "SEC001".to_string(),
            },
            ReviewFinding {
                dimension: ReviewDimension::Style,
                severity: FindingSeverity::Warning,
                file: "f.rs".to_string(),
                line: Some(2),
                message: "style".to_string(),
                suggestion: None,
                rule_id: "STY001".to_string(),
            },
        ];

        let summary = engine.generate_summary(&findings, &ReviewVerdict::Block);
        assert!(summary.contains("critical"));
        assert!(summary.contains("must be resolved"));
        assert!(summary.contains("2 issue(s)"));
    }

    // --- Deep review prompt ---

    #[test]
    fn test_deep_review_prompt() {
        let engine = ReviewEngine::new();
        let prompt = engine.deep_review_prompt(
            "src/auth.rs",
            "fn login(user: &str, pass: &str) { }",
            ReviewDimension::Security,
        );

        assert!(prompt.contains("src/auth.rs"));
        assert!(prompt.contains("fn login"));
        assert!(prompt.contains("Security"));
        assert!(prompt.contains("senior code reviewer"));
        assert!(prompt.contains("JSON"));
    }

    // --- Markdown formatting ---

    #[test]
    fn test_format_as_markdown() {
        let engine = ReviewEngine::new();
        let report = ReviewReport {
            verdict: ReviewVerdict::RequestChanges,
            findings: vec![
                ReviewFinding {
                    dimension: ReviewDimension::Security,
                    severity: FindingSeverity::Error,
                    file: "src/auth.rs".to_string(),
                    line: Some(42),
                    message: "Weak hash algorithm".to_string(),
                    suggestion: Some("Use SHA-256".to_string()),
                    rule_id: "SEC007".to_string(),
                },
                ReviewFinding {
                    dimension: ReviewDimension::Style,
                    severity: FindingSeverity::Info,
                    file: "src/auth.rs".to_string(),
                    line: None,
                    message: "TODO comment".to_string(),
                    suggestion: None,
                    rule_id: "STY006".to_string(),
                },
            ],
            summary: "Code has issues.".to_string(),
            scores: vec![DimensionScore {
                dimension: ReviewDimension::Security,
                score: 0.7,
                weight: 0.25,
                finding_count: 1,
            }],
            total_score: 0.65,
        };

        let md = engine.format_as_markdown(&report);
        assert!(md.contains("CHANGES REQUESTED"), "Should show verdict");
        assert!(md.contains("Dimension Scores"), "Should have scores table");
        assert!(md.contains("src/auth.rs:42"), "Should show file:line");
        assert!(md.contains("SEC007"), "Should show rule id");
        assert!(md.contains("Use SHA-256"), "Should show suggestion");
        assert!(md.contains("65%"), "Should show total score percentage");
    }

    // --- Config defaults ---

    #[test]
    fn test_review_config_defaults() {
        let config = ReviewConfig::default();
        assert_eq!(config.dimensions.len(), 7);
        assert!((config.approve_threshold - 0.7).abs() < f32::EPSILON);
        assert!((config.block_threshold - 0.4).abs() < f32::EPSILON);
        assert_eq!(config.max_findings, 50);
        assert!(config.language.is_none());
    }

    // --- Helper function tests ---

    #[test]
    fn test_extract_fn_name() {
        assert_eq!(extract_fn_name("fn main() {"), Some("main".to_string()));
        assert_eq!(
            extract_fn_name("pub fn process(x: i32) -> i32 {"),
            Some("process".to_string())
        );
        assert_eq!(
            extract_fn_name("pub async fn fetch(url: &str) {"),
            Some("fetch".to_string())
        );
        assert_eq!(extract_fn_name("let x = 42;"), None);
    }

    #[test]
    fn test_count_fn_params() {
        assert_eq!(count_fn_params("fn main()"), 0);
        assert_eq!(count_fn_params("fn add(a: i32, b: i32)"), 2);
        assert_eq!(count_fn_params("fn method(&self, x: i32)"), 1);
        assert_eq!(
            count_fn_params("fn many(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32)"),
            6
        );
    }

    #[test]
    fn test_diff_changed_lines() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nmodified\nline3\nnew_line\n";
        let changed = diff_changed_lines(old, new);
        assert!(changed.contains(&2), "Line 2 was modified");
        assert!(changed.contains(&4), "Line 4 was added");
        assert!(!changed.contains(&1), "Line 1 was unchanged");
        assert!(!changed.contains(&3), "Line 3 was unchanged");
    }
}

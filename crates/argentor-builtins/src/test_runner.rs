//! Test runner skill for the Argentor agent framework.
//!
//! Runs tests for Rust, Python, and Node.js projects, parses output into
//! structured JSON results with pass/fail/skip counts and per-test details.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::info;

/// Maximum output size from a test runner process (500 KB).
const MAX_OUTPUT_BYTES: usize = 512_000;

/// Default timeout in seconds for test execution.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Absolute maximum timeout in seconds.
const MAX_TIMEOUT_SECS: u64 = 600;

/// Supported test languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    Rust,
    Python,
    Node,
}

impl Language {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" => Some(Self::Rust),
            "python" => Some(Self::Python),
            "node" | "nodejs" | "node.js" => Some(Self::Node),
            _ => None,
        }
    }
}

/// Summary of test execution results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSummary {
    /// Total number of tests discovered.
    pub total: usize,
    /// Number of tests that passed.
    pub passed: usize,
    /// Number of tests that failed.
    pub failed: usize,
    /// Number of tests that were skipped or ignored.
    pub skipped: usize,
    /// Total execution time in seconds.
    pub duration_secs: f64,
}

/// A single test result entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestEntry {
    /// Fully qualified test name.
    pub name: String,
    /// Outcome (e.g., "passed", "failed", "ignored").
    pub status: String,
    /// Execution time in milliseconds, if available.
    pub duration_ms: Option<u64>,
    /// Failure message or additional details.
    pub message: Option<String>,
}

/// Full structured test output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutput {
    /// Aggregate pass/fail/skip counts and timing.
    pub summary: TestSummary,
    /// Individual test results.
    pub tests: Vec<TestEntry>,
}

/// Test runner skill that executes and parses tests for multiple languages.
pub struct TestRunnerSkill {
    descriptor: SkillDescriptor,
}

impl TestRunnerSkill {
    /// Create a new `TestRunnerSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "test_runner".to_string(),
                description:
                    "Run tests for Rust, Python, or Node.js projects and return structured results."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["run", "run_single", "list"],
                            "description": "Operation: 'run' all tests, 'run_single' one test, or 'list' available tests"
                        },
                        "project_path": {
                            "type": "string",
                            "description": "Absolute path to the project root directory"
                        },
                        "language": {
                            "type": "string",
                            "enum": ["rust", "python", "node"],
                            "description": "Project language: rust, python, or node"
                        },
                        "filter": {
                            "type": "string",
                            "description": "Optional test name filter pattern (for 'run' operation)"
                        },
                        "test_name": {
                            "type": "string",
                            "description": "Specific test name to run (for 'run_single' operation)"
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Timeout in seconds (default: 120, max: 600)"
                        }
                    },
                    "required": ["operation", "project_path", "language"]
                }),
                required_capabilities: vec![Capability::ShellExec {
                    allowed_commands: vec![
                        "cargo".to_string(),
                        "python".to_string(),
                        "npx".to_string(),
                    ],
                }],
                requires_approval: false,
            },
        }
    }
}

impl Default for TestRunnerSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for TestRunnerSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        _permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let project_path = call.arguments["project_path"].as_str().unwrap_or_default();

        if project_path.is_empty() {
            return Err(argentor_core::ArgentorError::Skill(
                "project_path is required and cannot be empty".to_string(),
            ));
        }

        if !Path::new(project_path).is_absolute() {
            return Err(argentor_core::ArgentorError::Skill(
                "project_path must be an absolute path".to_string(),
            ));
        }

        let language = call.arguments["language"].as_str().unwrap_or_default();
        if Language::from_str(language).is_none() {
            return Err(argentor_core::ArgentorError::Skill(format!(
                "unsupported language '{language}': use rust, python, or node"
            )));
        }

        let operation = call.arguments["operation"].as_str().unwrap_or_default();
        if !["run", "run_single", "list"].contains(&operation) {
            return Err(argentor_core::ArgentorError::Skill(format!(
                "unknown operation '{operation}': use run, run_single, or list"
            )));
        }

        if operation == "run_single" {
            let test_name = call.arguments["test_name"].as_str().unwrap_or_default();
            if test_name.is_empty() {
                return Err(argentor_core::ArgentorError::Skill(
                    "test_name is required for run_single operation".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let project_path = call.arguments["project_path"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let language_str = call.arguments["language"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let filter = call.arguments["filter"].as_str().map(ToString::to_string);
        let test_name = call.arguments["test_name"]
            .as_str()
            .map(ToString::to_string);
        let timeout_secs = call.arguments["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        if project_path.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "project_path is required and cannot be empty",
            ));
        }

        let path = Path::new(&project_path);
        if !path.is_absolute() {
            return Ok(ToolResult::error(
                &call.id,
                "project_path must be an absolute path",
            ));
        }

        if !path.is_dir() {
            return Ok(ToolResult::error(
                &call.id,
                format!("project_path does not exist or is not a directory: {project_path}"),
            ));
        }

        let language = match Language::from_str(&language_str) {
            Some(lang) => lang,
            None => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("unsupported language '{language_str}': use rust, python, or node"),
                ));
            }
        };

        info!(
            operation = %operation,
            project_path = %project_path,
            language = %language_str,
            timeout = timeout_secs,
            "Executing test runner"
        );

        let result = match operation.as_str() {
            "run" => run_tests(&project_path, language, filter.as_deref(), timeout_secs).await,
            "run_single" => {
                let name = match &test_name {
                    Some(n) => n.as_str(),
                    None => {
                        return Ok(ToolResult::error(
                            &call.id,
                            "test_name is required for run_single operation",
                        ));
                    }
                };
                run_single_test(&project_path, language, name, timeout_secs).await
            }
            "list" => list_tests(&project_path, language, timeout_secs).await,
            _ => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("unknown operation '{operation}': use run, run_single, or list"),
                ));
            }
        };

        match result {
            Ok(output) => {
                let json = serde_json::to_string_pretty(&output)
                    .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}"));
                if output.summary.failed > 0 {
                    Ok(ToolResult::error(&call.id, json))
                } else {
                    Ok(ToolResult::success(&call.id, json))
                }
            }
            Err(msg) => Ok(ToolResult::error(&call.id, msg)),
        }
    }
}

// ---------------------------------------------------------------------------
// Test execution
// ---------------------------------------------------------------------------

/// Run all tests (optionally filtered) for the given language.
async fn run_tests(
    project_path: &str,
    language: Language,
    filter: Option<&str>,
    timeout_secs: u64,
) -> Result<TestOutput, String> {
    let (cmd, args) = build_run_command(language, filter);
    let output = execute_command(&cmd, &args, project_path, timeout_secs).await?;
    parse_test_output(language, &output.stdout, &output.stderr, output.duration)
}

/// Run a single named test.
async fn run_single_test(
    project_path: &str,
    language: Language,
    test_name: &str,
    timeout_secs: u64,
) -> Result<TestOutput, String> {
    let (cmd, args) = build_single_test_command(language, test_name);
    let output = execute_command(&cmd, &args, project_path, timeout_secs).await?;
    parse_test_output(language, &output.stdout, &output.stderr, output.duration)
}

/// List available tests without running them.
async fn list_tests(
    project_path: &str,
    language: Language,
    timeout_secs: u64,
) -> Result<TestOutput, String> {
    let (cmd, args) = build_list_command(language);
    let output = execute_command(&cmd, &args, project_path, timeout_secs).await?;
    parse_list_output(language, &output.stdout)
}

// ---------------------------------------------------------------------------
// Command builders
// ---------------------------------------------------------------------------

fn build_run_command(language: Language, filter: Option<&str>) -> (String, Vec<String>) {
    match language {
        Language::Rust => {
            let mut args = vec!["test".to_string(), "--no-fail-fast".to_string()];
            if let Some(f) = filter {
                args.push(f.to_string());
            }
            args.push("--".to_string());
            args.push("--format=terse".to_string());
            ("cargo".to_string(), args)
        }
        Language::Python => {
            let mut args = vec![
                "-m".to_string(),
                "pytest".to_string(),
                "-v".to_string(),
                "--tb=short".to_string(),
            ];
            if let Some(f) = filter {
                args.push("-k".to_string());
                args.push(f.to_string());
            }
            ("python".to_string(), args)
        }
        Language::Node => {
            let args = vec![
                "vitest".to_string(),
                "run".to_string(),
                "--reporter=verbose".to_string(),
            ];
            ("npx".to_string(), args)
        }
    }
}

fn build_single_test_command(language: Language, test_name: &str) -> (String, Vec<String>) {
    match language {
        Language::Rust => {
            let args = vec![
                "test".to_string(),
                "--no-fail-fast".to_string(),
                test_name.to_string(),
                "--".to_string(),
                "--exact".to_string(),
            ];
            ("cargo".to_string(), args)
        }
        Language::Python => {
            let args = vec![
                "-m".to_string(),
                "pytest".to_string(),
                "-v".to_string(),
                "--tb=short".to_string(),
                "-k".to_string(),
                test_name.to_string(),
            ];
            ("python".to_string(), args)
        }
        Language::Node => {
            let args = vec![
                "vitest".to_string(),
                "run".to_string(),
                "--reporter=verbose".to_string(),
                "-t".to_string(),
                test_name.to_string(),
            ];
            ("npx".to_string(), args)
        }
    }
}

fn build_list_command(language: Language) -> (String, Vec<String>) {
    match language {
        Language::Rust => {
            let args = vec!["test".to_string(), "--".to_string(), "--list".to_string()];
            ("cargo".to_string(), args)
        }
        Language::Python => {
            let args = vec![
                "-m".to_string(),
                "pytest".to_string(),
                "--collect-only".to_string(),
                "-q".to_string(),
            ];
            ("python".to_string(), args)
        }
        Language::Node => {
            let args = vec!["vitest".to_string(), "list".to_string()];
            ("npx".to_string(), args)
        }
    }
}

// ---------------------------------------------------------------------------
// Process execution
// ---------------------------------------------------------------------------

struct CommandOutput {
    stdout: String,
    stderr: String,
    #[allow(dead_code)]
    exit_code: i32,
    duration: Duration,
}

async fn execute_command(
    cmd: &str,
    args: &[String],
    working_dir: &str,
    timeout_secs: u64,
) -> Result<CommandOutput, String> {
    info!(cmd = %cmd, args = ?args, dir = %working_dir, "Spawning test process");

    let start = Instant::now();

    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::process::Command::new(cmd)
            .args(args)
            .current_dir(working_dir)
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .output(),
    )
    .await;

    let duration = start.elapsed();

    match result {
        Ok(Ok(output)) => {
            let stdout_raw = String::from_utf8_lossy(&output.stdout);
            let stderr_raw = String::from_utf8_lossy(&output.stderr);

            let stdout = truncate_output(&stdout_raw, MAX_OUTPUT_BYTES);
            let stderr = truncate_output(&stderr_raw, MAX_OUTPUT_BYTES);

            let exit_code = output.status.code().unwrap_or(-1);

            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
                duration,
            })
        }
        Ok(Err(e)) => Err(format!("failed to execute {cmd}: {e}")),
        Err(_) => Err(format!("test execution timed out after {timeout_secs}s")),
    }
}

fn truncate_output(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}... [truncated, {} total bytes]", &s[..max_len], s.len())
    }
}

// ---------------------------------------------------------------------------
// Output parsers
// ---------------------------------------------------------------------------

fn parse_test_output(
    language: Language,
    stdout: &str,
    stderr: &str,
    duration: Duration,
) -> Result<TestOutput, String> {
    match language {
        Language::Rust => parse_rust_test_output(stdout, stderr, duration),
        Language::Python => parse_pytest_output(stdout, stderr, duration),
        Language::Node => parse_node_test_output(stdout, stderr, duration),
    }
}

fn parse_list_output(language: Language, stdout: &str) -> Result<TestOutput, String> {
    let mut tests = Vec::new();

    match language {
        Language::Rust => {
            // cargo test -- --list outputs: "test_name: test\n"
            for line in stdout.lines() {
                let line = line.trim();
                if let Some(name) = line.strip_suffix(": test") {
                    tests.push(TestEntry {
                        name: name.to_string(),
                        status: "listed".to_string(),
                        duration_ms: None,
                        message: None,
                    });
                } else if let Some(name) = line.strip_suffix(": benchmark") {
                    tests.push(TestEntry {
                        name: name.to_string(),
                        status: "benchmark".to_string(),
                        duration_ms: None,
                        message: None,
                    });
                }
            }
        }
        Language::Python => {
            // pytest --collect-only -q outputs: "path/test_file.py::test_name\n"
            for line in stdout.lines() {
                let line = line.trim();
                if line.contains("::") && !line.starts_with("=") && !line.is_empty() {
                    tests.push(TestEntry {
                        name: line.to_string(),
                        status: "listed".to_string(),
                        duration_ms: None,
                        message: None,
                    });
                }
            }
        }
        Language::Node => {
            // vitest list outputs test names line by line
            for line in stdout.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with("RUN") && !line.starts_with("stdout") {
                    tests.push(TestEntry {
                        name: line.to_string(),
                        status: "listed".to_string(),
                        duration_ms: None,
                        message: None,
                    });
                }
            }
        }
    }

    Ok(TestOutput {
        summary: TestSummary {
            total: tests.len(),
            passed: 0,
            failed: 0,
            skipped: 0,
            duration_secs: 0.0,
        },
        tests,
    })
}

// ---------------------------------------------------------------------------
// Rust parser
// ---------------------------------------------------------------------------

fn parse_rust_test_output(
    stdout: &str,
    stderr: &str,
    duration: Duration,
) -> Result<TestOutput, String> {
    let mut tests = Vec::new();
    let mut current_failure_name: Option<String> = None;
    let mut current_failure_lines: Vec<String> = Vec::new();
    let mut in_failures_section = false;

    // Combined output: cargo test writes test lines to stdout, compilation to stderr.
    // We parse both.
    let combined = format!("{stdout}\n{stderr}");

    for line in combined.lines() {
        let trimmed = line.trim();

        // Detect "failures:" section header to capture failure messages.
        if trimmed == "failures:" {
            in_failures_section = true;
            continue;
        }

        // End of failures section.
        if in_failures_section && trimmed.starts_with("test result:") {
            in_failures_section = false;
            // Flush last failure if any.
            flush_failure(&mut tests, &current_failure_name, &current_failure_lines);
            current_failure_name = None;
            current_failure_lines.clear();
        }

        // Capture failure details in the failures section.
        if in_failures_section {
            if trimmed.starts_with("---- ") && trimmed.ends_with(" ----") {
                // New failure block: ---- tests::test_name stdout ----
                flush_failure(&mut tests, &current_failure_name, &current_failure_lines);
                let inner = &trimmed[5..trimmed.len() - 5].trim();
                // Strip " stdout" suffix if present.
                let name = inner.strip_suffix(" stdout").unwrap_or(inner).to_string();
                current_failure_name = Some(name);
                current_failure_lines = Vec::new();
            } else if current_failure_name.is_some() && !trimmed.is_empty() {
                current_failure_lines.push(trimmed.to_string());
            }
            continue;
        }

        // Parse individual test result lines:
        //   test tests::test_name ... ok
        //   test tests::test_name ... FAILED
        //   test tests::test_name ... ignored
        if let Some(rest) = trimmed.strip_prefix("test ") {
            if let Some((name, status_str)) = rest.rsplit_once(" ... ") {
                let name = name.trim().to_string();
                let status_str = status_str.trim().to_lowercase();
                let status = match status_str.as_str() {
                    "ok" => "passed",
                    "failed" => "failed",
                    "ignored" => "skipped",
                    _ => "unknown",
                };
                tests.push(TestEntry {
                    name,
                    status: status.to_string(),
                    duration_ms: None,
                    message: None,
                });
            }
        }
    }

    // Flush any remaining failure.
    flush_failure(&mut tests, &current_failure_name, &current_failure_lines);

    // Try to parse the summary line: "test result: ok. X passed; Y failed; Z ignored; ..."
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    for line in combined.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("test result:") {
            // Strip the status word (ok/FAILED) and period: "FAILED. 1 passed; ..."
            let rest = rest.trim();
            let rest = if let Some(dot_pos) = rest.find(". ") {
                &rest[dot_pos + 2..]
            } else {
                rest
            };
            // Extract numeric counts from patterns like "X passed"
            for segment in rest.split(';') {
                let segment = segment.trim().trim_end_matches('.');
                if let Some(n) = extract_count(segment, "passed") {
                    passed = n;
                } else if let Some(n) = extract_count(segment, "failed") {
                    failed = n;
                } else if let Some(n) = extract_count(segment, "ignored") {
                    skipped = n;
                }
            }
        }
    }

    // Fallback: if no summary line was found, count from parsed tests.
    if passed == 0 && failed == 0 && skipped == 0 && !tests.is_empty() {
        for t in &tests {
            match t.status.as_str() {
                "passed" => passed += 1,
                "failed" => failed += 1,
                "skipped" => skipped += 1,
                _ => {}
            }
        }
    }

    let total = passed + failed + skipped;

    Ok(TestOutput {
        summary: TestSummary {
            total,
            passed,
            failed,
            skipped,
            duration_secs: round_duration(duration),
        },
        tests,
    })
}

/// Attach failure messages to already-parsed test entries.
fn flush_failure(tests: &mut [TestEntry], name: &Option<String>, lines: &[String]) {
    if let Some(ref failure_name) = name {
        if !lines.is_empty() {
            let message = lines.join("\n");
            for test in tests.iter_mut() {
                if test.name == *failure_name && test.status == "failed" {
                    test.message = Some(message.clone());
                    return;
                }
            }
        }
    }
}

/// Extract a count from a string like "42 passed" or "1 failed".
fn extract_count(segment: &str, suffix: &str) -> Option<usize> {
    let segment = segment.trim();
    if let Some(stripped) = segment.strip_suffix(suffix) {
        stripped.trim().parse::<usize>().ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Python parser
// ---------------------------------------------------------------------------

fn parse_pytest_output(
    stdout: &str,
    _stderr: &str,
    duration: Duration,
) -> Result<TestOutput, String> {
    let mut tests = Vec::new();
    let mut in_failures = false;
    let mut current_failure_name: Option<String> = None;
    let mut current_failure_lines: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Detect FAILURES section.
        if trimmed.starts_with("=") && trimmed.contains("FAILURES") {
            in_failures = true;
            continue;
        }

        // Detect end of failures / short test summary.
        if in_failures
            && trimmed.starts_with("=")
            && (trimmed.contains("short test summary")
                || trimmed.contains("passed")
                || trimmed.contains("failed")
                || trimmed.contains("error"))
        {
            in_failures = false;
            flush_pytest_failure(&mut tests, &current_failure_name, &current_failure_lines);
            current_failure_name = None;
            current_failure_lines.clear();
            continue;
        }

        if in_failures {
            if trimmed.starts_with("_") && trimmed.ends_with("_") {
                // New failure: ____ test_name ____
                flush_pytest_failure(&mut tests, &current_failure_name, &current_failure_lines);
                let inner = trimmed.trim_matches('_').trim();
                current_failure_name = Some(inner.to_string());
                current_failure_lines = Vec::new();
            } else if current_failure_name.is_some() && !trimmed.is_empty() {
                current_failure_lines.push(trimmed.to_string());
            }
            continue;
        }

        // Parse verbose test lines:
        //   tests/test_foo.py::test_bar PASSED
        //   tests/test_foo.py::test_baz FAILED
        //   tests/test_foo.py::test_skip SKIPPED (reason)
        if trimmed.contains("::") {
            let (test_part, rest) = if let Some(idx) = trimmed.rfind(" PASSED") {
                (&trimmed[..idx], "passed")
            } else if let Some(idx) = trimmed.rfind(" FAILED") {
                (&trimmed[..idx], "failed")
            } else if let Some(idx) = trimmed.rfind(" SKIPPED") {
                (&trimmed[..idx], "skipped")
            } else if let Some(idx) = trimmed.rfind(" ERROR") {
                (&trimmed[..idx], "failed")
            } else {
                continue;
            };

            let name = test_part.trim().to_string();
            tests.push(TestEntry {
                name,
                status: rest.to_string(),
                duration_ms: None,
                message: None,
            });
        }
    }

    // Flush remaining failure.
    flush_pytest_failure(&mut tests, &current_failure_name, &current_failure_lines);

    // Try to parse summary line: "X passed, Y failed, Z skipped in N.NNs"
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut summary_duration: Option<f64> = None;

    for line in stdout.lines() {
        let trimmed = line.trim().trim_matches('=').trim();
        if (trimmed.contains("passed") || trimmed.contains("failed") || trimmed.contains("skipped"))
            && trimmed.contains(" in ")
        {
            // Parse "X passed, Y failed in 1.23s"
            for segment in trimmed.split(',') {
                let segment = segment.trim();
                if let Some(n) = extract_count_word(segment, "passed") {
                    passed = n;
                } else if let Some(n) = extract_count_word(segment, "failed") {
                    failed = n;
                } else if let Some(n) = extract_count_word(segment, "skipped") {
                    skipped = n;
                }
            }
            // Parse duration from "in X.XXs"
            if let Some(in_idx) = trimmed.rfind(" in ") {
                let dur_str = trimmed[in_idx + 4..].trim().trim_end_matches('s');
                summary_duration = dur_str.parse::<f64>().ok();
            }
        }
    }

    // Fallback: count from parsed tests.
    if passed == 0 && failed == 0 && skipped == 0 && !tests.is_empty() {
        for t in &tests {
            match t.status.as_str() {
                "passed" => passed += 1,
                "failed" => failed += 1,
                "skipped" => skipped += 1,
                _ => {}
            }
        }
    }

    let total = passed + failed + skipped;
    let dur = summary_duration.unwrap_or_else(|| round_duration(duration));

    Ok(TestOutput {
        summary: TestSummary {
            total,
            passed,
            failed,
            skipped,
            duration_secs: dur,
        },
        tests,
    })
}

fn flush_pytest_failure(tests: &mut [TestEntry], name: &Option<String>, lines: &[String]) {
    if let Some(ref failure_name) = name {
        if !lines.is_empty() {
            let message = lines.join("\n");
            for test in tests.iter_mut() {
                // Match by suffix since failure block may use short name.
                if (test.name == *failure_name || test.name.ends_with(failure_name))
                    && test.status == "failed"
                {
                    test.message = Some(message.clone());
                    return;
                }
            }
        }
    }
}

/// Extract a count from "42 passed" style segments (with possible trailing text).
fn extract_count_word(segment: &str, word: &str) -> Option<usize> {
    let segment = segment.trim();
    // Find the word, then take the number before it.
    if let Some(idx) = segment.find(word) {
        let before = segment[..idx].trim();
        // The number might be at the end of `before`.
        let num_str = before.split_whitespace().next_back()?;
        num_str.parse::<usize>().ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Node.js parser
// ---------------------------------------------------------------------------

fn parse_node_test_output(
    stdout: &str,
    _stderr: &str,
    duration: Duration,
) -> Result<TestOutput, String> {
    let mut tests = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Vitest verbose output patterns:
        //   ✓ test name  (Xms)
        //   × test name  (Xms)
        //   ↓ test name  [skipped]
        // Also handle text-only variants:
        //   PASS test name
        //   FAIL test name

        if trimmed.starts_with("✓") || trimmed.starts_with("√") {
            let name = extract_node_test_name(trimmed);
            let dur = extract_parenthesized_duration(trimmed);
            tests.push(TestEntry {
                name,
                status: "passed".to_string(),
                duration_ms: dur,
                message: None,
            });
        } else if trimmed.starts_with("×") || trimmed.starts_with("✕") {
            let name = extract_node_test_name(trimmed);
            let dur = extract_parenthesized_duration(trimmed);
            tests.push(TestEntry {
                name,
                status: "failed".to_string(),
                duration_ms: dur,
                message: None,
            });
        } else if trimmed.starts_with("↓") {
            let name = extract_node_test_name(trimmed);
            tests.push(TestEntry {
                name,
                status: "skipped".to_string(),
                duration_ms: None,
                message: None,
            });
        } else if let Some(stripped) = trimmed.strip_prefix("PASS ") {
            let name = stripped.trim().to_string();
            tests.push(TestEntry {
                name,
                status: "passed".to_string(),
                duration_ms: None,
                message: None,
            });
        } else if let Some(stripped) = trimmed.strip_prefix("FAIL ") {
            let name = stripped.trim().to_string();
            tests.push(TestEntry {
                name,
                status: "failed".to_string(),
                duration_ms: None,
                message: None,
            });
        }
    }

    let passed = tests.iter().filter(|t| t.status == "passed").count();
    let failed = tests.iter().filter(|t| t.status == "failed").count();
    let skipped = tests.iter().filter(|t| t.status == "skipped").count();
    let total = passed + failed + skipped;

    Ok(TestOutput {
        summary: TestSummary {
            total,
            passed,
            failed,
            skipped,
            duration_secs: round_duration(duration),
        },
        tests,
    })
}

/// Extract test name from a vitest line, stripping leading symbol and trailing duration.
fn extract_node_test_name(line: &str) -> String {
    let trimmed = line.trim();
    // Skip leading Unicode symbol + whitespace.
    let after_symbol = if trimmed.len() > 1 {
        let mut chars = trimmed.chars();
        chars.next(); // skip symbol
        chars.as_str().trim_start()
    } else {
        trimmed
    };

    // Strip trailing "(Xms)" if present.
    if let Some(paren_idx) = after_symbol.rfind('(') {
        let before = after_symbol[..paren_idx].trim();
        if !before.is_empty() {
            return before.to_string();
        }
    }

    after_symbol.to_string()
}

/// Extract duration in milliseconds from a trailing "(Xms)" pattern.
fn extract_parenthesized_duration(line: &str) -> Option<u64> {
    let open = line.rfind('(')?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let inner = &line[open + 1..close];
    let num_str = inner.trim().strip_suffix("ms")?;
    num_str.trim().parse::<u64>().ok()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn round_duration(d: Duration) -> f64 {
    (d.as_secs_f64() * 100.0).round() / 100.0
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Rust output parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_rust_basic_output() {
        let stdout = "\
running 3 tests
test tests::test_one ... ok
test tests::test_two ... FAILED
test tests::test_three ... ignored

failures:

---- tests::test_two stdout ----
assertion failed: expected 1, got 2

failures:
    tests::test_two

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out
";

        let output = parse_rust_test_output(stdout, "", Duration::from_millis(1500)).unwrap();

        assert_eq!(output.summary.total, 3);
        assert_eq!(output.summary.passed, 1);
        assert_eq!(output.summary.failed, 1);
        assert_eq!(output.summary.skipped, 1);

        assert_eq!(output.tests.len(), 3);
        assert_eq!(output.tests[0].name, "tests::test_one");
        assert_eq!(output.tests[0].status, "passed");
        assert_eq!(output.tests[1].name, "tests::test_two");
        assert_eq!(output.tests[1].status, "failed");
        assert!(output.tests[1].message.is_some());
        assert!(output.tests[1]
            .message
            .as_ref()
            .unwrap()
            .contains("assertion failed"));
        assert_eq!(output.tests[2].name, "tests::test_three");
        assert_eq!(output.tests[2].status, "skipped");
    }

    #[test]
    fn test_parse_rust_all_pass() {
        let stdout = "\
running 2 tests
test tests::alpha ... ok
test tests::beta ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
";

        let output = parse_rust_test_output(stdout, "", Duration::from_millis(500)).unwrap();

        assert_eq!(output.summary.total, 2);
        assert_eq!(output.summary.passed, 2);
        assert_eq!(output.summary.failed, 0);
        assert_eq!(output.summary.skipped, 0);
    }

    // -----------------------------------------------------------------------
    // Python output parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_pytest_basic_output() {
        let stdout = "\
============================= test session starts ==============================
collected 3 items

tests/test_example.py::test_add PASSED
tests/test_example.py::test_subtract FAILED
tests/test_example.py::test_skip SKIPPED (not implemented)

=========================== FAILURES ===========================
___________________________ test_subtract ___________________________

    def test_subtract():
>       assert 5 - 3 == 1
E       AssertionError: assert 2 == 1

=========================== short test summary info ============================
FAILED tests/test_example.py::test_subtract - AssertionError: assert 2 == 1
========================= 1 passed, 1 failed, 1 skipped in 0.54s =========================
";

        let output = parse_pytest_output(stdout, "", Duration::from_millis(540)).unwrap();

        assert_eq!(output.summary.total, 3);
        assert_eq!(output.summary.passed, 1);
        assert_eq!(output.summary.failed, 1);
        assert_eq!(output.summary.skipped, 1);
        assert_eq!(output.tests.len(), 3);
        assert_eq!(output.tests[0].status, "passed");
        assert_eq!(output.tests[1].status, "failed");
        assert_eq!(output.tests[2].status, "skipped");

        // Check failure message was attached.
        assert!(output.tests[1].message.is_some());
        assert!(output.tests[1].message.as_ref().unwrap().contains("assert"));
    }

    #[test]
    fn test_parse_pytest_all_pass() {
        let stdout = "\
============================= test session starts ==============================
collected 2 items

tests/test_math.py::test_add PASSED
tests/test_math.py::test_mul PASSED

========================= 2 passed in 0.12s =========================
";

        let output = parse_pytest_output(stdout, "", Duration::from_millis(120)).unwrap();

        assert_eq!(output.summary.total, 2);
        assert_eq!(output.summary.passed, 2);
        assert_eq!(output.summary.failed, 0);
    }

    // -----------------------------------------------------------------------
    // Node.js output parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_node_vitest_output() {
        let stdout = "\
 ✓ adds two numbers (2ms)
 ✓ subtracts correctly (1ms)
 × fails on purpose (5ms)
 ↓ skipped test [skipped]
";

        let output = parse_node_test_output(stdout, "", Duration::from_millis(100)).unwrap();

        assert_eq!(output.summary.total, 4);
        assert_eq!(output.summary.passed, 2);
        assert_eq!(output.summary.failed, 1);
        assert_eq!(output.summary.skipped, 1);

        assert_eq!(output.tests[0].name, "adds two numbers");
        assert_eq!(output.tests[0].duration_ms, Some(2));
        assert_eq!(output.tests[2].name, "fails on purpose");
        assert_eq!(output.tests[2].status, "failed");
    }

    // -----------------------------------------------------------------------
    // List parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_rust_list_output() {
        let stdout = "\
tests::test_one: test
tests::test_two: test
benches::bench_speed: benchmark
";

        let output = parse_list_output(Language::Rust, stdout).unwrap();

        assert_eq!(output.summary.total, 3);
        assert_eq!(output.tests.len(), 3);
        assert_eq!(output.tests[0].name, "tests::test_one");
        assert_eq!(output.tests[0].status, "listed");
        assert_eq!(output.tests[2].name, "benches::bench_speed");
        assert_eq!(output.tests[2].status, "benchmark");
    }

    #[test]
    fn test_parse_pytest_list_output() {
        let stdout = "\
tests/test_foo.py::test_bar
tests/test_foo.py::test_baz
2 tests collected
";

        let output = parse_list_output(Language::Python, stdout).unwrap();

        assert_eq!(output.tests.len(), 2);
        assert_eq!(output.tests[0].name, "tests/test_foo.py::test_bar");
    }

    // -----------------------------------------------------------------------
    // Timeout enforcement (unit-level)
    // -----------------------------------------------------------------------

    #[test]
    fn test_timeout_clamping() {
        // Simulates the clamping logic used in execute().
        let requested: u64 = 9999;
        let clamped = requested.min(MAX_TIMEOUT_SECS);
        assert_eq!(clamped, 600);
    }

    #[test]
    fn test_default_timeout() {
        assert_eq!(DEFAULT_TIMEOUT_SECS, 120);
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_project_path_returns_error() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_1".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run",
                "project_path": "",
                "language": "rust"
            }),
        };
        let perms = PermissionSet::new();
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_relative_project_path_returns_error() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_2".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run",
                "project_path": "relative/path",
                "language": "rust"
            }),
        };
        let perms = PermissionSet::new();
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_unsupported_language_returns_error() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_3".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run",
                "project_path": "/tmp/project",
                "language": "cobol"
            }),
        };
        let perms = PermissionSet::new();
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_single_without_test_name_returns_error() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_4".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run_single",
                "project_path": "/tmp/project",
                "language": "rust"
            }),
        };
        let perms = PermissionSet::new();
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_arguments_pass_validation() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_5".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run",
                "project_path": "/tmp/project",
                "language": "rust"
            }),
        };
        let perms = PermissionSet::new();
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_ok());
    }

    #[test]
    fn test_valid_run_single_passes_validation() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_6".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run_single",
                "project_path": "/tmp/project",
                "language": "python",
                "test_name": "test_foo"
            }),
        };
        let perms = PermissionSet::new();
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Async execute: non-existent directory
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_execute_nonexistent_directory() {
        let skill = TestRunnerSkill::new();
        let call = ToolCall {
            id: "test_7".to_string(),
            name: "test_runner".to_string(),
            arguments: serde_json::json!({
                "operation": "run",
                "project_path": "/nonexistent/path/that/does/not/exist",
                "language": "rust"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("does not exist"));
    }

    // -----------------------------------------------------------------------
    // Helper functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_count() {
        assert_eq!(extract_count("42 passed", "passed"), Some(42));
        assert_eq!(extract_count("1 failed", "failed"), Some(1));
        assert_eq!(extract_count("0 ignored", "ignored"), Some(0));
        assert_eq!(extract_count("not a number passed", "passed"), None);
    }

    #[test]
    fn test_extract_parenthesized_duration() {
        assert_eq!(extract_parenthesized_duration("test name (15ms)"), Some(15));
        assert_eq!(
            extract_parenthesized_duration("test name (120ms)"),
            Some(120)
        );
        assert_eq!(extract_parenthesized_duration("test name"), None);
    }

    #[test]
    fn test_extract_node_test_name() {
        assert_eq!(
            extract_node_test_name("✓ adds two numbers (2ms)"),
            "adds two numbers"
        );
        assert_eq!(
            extract_node_test_name("× fails on purpose (5ms)"),
            "fails on purpose"
        );
        assert_eq!(
            extract_node_test_name("↓ skipped test [skipped]"),
            "skipped test [skipped]"
        );
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(Language::from_str("rust"), Some(Language::Rust));
        assert_eq!(Language::from_str("Rust"), Some(Language::Rust));
        assert_eq!(Language::from_str("python"), Some(Language::Python));
        assert_eq!(Language::from_str("node"), Some(Language::Node));
        assert_eq!(Language::from_str("nodejs"), Some(Language::Node));
        assert_eq!(Language::from_str("node.js"), Some(Language::Node));
        assert_eq!(Language::from_str("cobol"), None);
    }

    #[test]
    fn test_truncate_output_short() {
        let out = truncate_output("hello", 100);
        assert_eq!(out, "hello");
    }

    #[test]
    fn test_truncate_output_long() {
        let long = "a".repeat(1000);
        let out = truncate_output(&long, 50);
        assert!(out.contains("truncated"));
        assert!(out.contains("1000 total bytes"));
    }

    #[test]
    fn test_round_duration() {
        let d = Duration::from_millis(1234);
        assert!((round_duration(d) - 1.23).abs() < 0.01);
    }

    // -----------------------------------------------------------------------
    // Descriptor
    // -----------------------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let skill = TestRunnerSkill::new();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "test_runner");
        assert!(!desc.description.is_empty());
        assert!(!desc.required_capabilities.is_empty());
    }
}

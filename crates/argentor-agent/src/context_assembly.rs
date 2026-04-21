//! Automatic system prompt assembly for agentic CLI sessions.
//!
//! Gathers project context (git info, config files, tool list) and builds
//! an optimal system prompt, similar to how Claude Code assembles context.
//!
//! # Main types
//!
//! - [`AssembledContext`] — Full context for an agent session.
//! - [`GitContext`] — Git repository metadata (branch, status, commits).
//! - [`ContextAssembler`] — Builder that gathers and assembles context.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// AssembledContext
// ---------------------------------------------------------------------------

/// Assembled context for an agent session.
///
/// Contains all gathered project metadata, git info, instructions,
/// and the final system prompt ready to send to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembledContext {
    /// The fully-built system prompt string.
    pub system_prompt: String,
    /// Detected project name (from directory name or git remote).
    pub project_name: Option<String>,
    /// Absolute path of the working directory.
    pub working_directory: String,
    /// Git repository context, if available.
    pub git_info: Option<GitContext>,
    /// Raw project instructions text (from ARGENTOR.md, etc.).
    pub project_instructions: Option<String>,
    /// Names of tools available in this session.
    pub available_tools: Vec<String>,
    /// Permission mode label (e.g. "sandboxed", "permissive").
    pub permission_mode: String,
    /// Model identifier string.
    pub model_info: String,
    /// Estimated token count of the system prompt (1 token ~ 4 chars).
    pub token_estimate: usize,
}

// ---------------------------------------------------------------------------
// GitContext
// ---------------------------------------------------------------------------

/// Git repository metadata gathered from the working directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitContext {
    /// Current branch name.
    pub branch: String,
    /// Human-readable status summary, e.g. "3 modified, 1 untracked".
    pub status_summary: String,
    /// Last N one-line commit messages.
    pub recent_commits: Vec<String>,
    /// Remote origin URL, if configured.
    pub remote_url: Option<String>,
    /// Whether the working tree has uncommitted changes.
    pub is_dirty: bool,
}

// ---------------------------------------------------------------------------
// ContextAssembler
// ---------------------------------------------------------------------------

/// Builder that gathers project context and assembles a system prompt.
///
/// # Example
///
/// ```no_run
/// use argentor_agent::context_assembly::ContextAssembler;
///
/// let ctx = ContextAssembler::new(".")
///     .with_git(true)
///     .with_project_file(true)
///     .assemble(&["calculator".into(), "web_search".into()], "claude-4", "sandboxed");
/// println!("{}", ctx.system_prompt);
/// ```
pub struct ContextAssembler {
    working_dir: PathBuf,
    include_git: bool,
    include_project_file: bool,
    max_instructions_tokens: usize,
    custom_instructions: Option<String>,
}

impl ContextAssembler {
    /// Create a new assembler for the given working directory.
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            include_git: true,
            include_project_file: true,
            max_instructions_tokens: 4000,
            custom_instructions: None,
        }
    }

    /// Enable or disable git context gathering.
    pub fn with_git(mut self, include: bool) -> Self {
        self.include_git = include;
        self
    }

    /// Enable or disable project instruction file reading.
    pub fn with_project_file(mut self, include: bool) -> Self {
        self.include_project_file = include;
        self
    }

    /// Provide custom instructions to append to the system prompt.
    pub fn with_custom_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.custom_instructions = Some(instructions.into());
        self
    }

    /// Set the maximum token budget for project instructions (1 token ~ 4 chars).
    pub fn max_instructions_tokens(mut self, max: usize) -> Self {
        self.max_instructions_tokens = max;
        self
    }

    /// Assemble the full context, gathering git info, project files, and building the prompt.
    pub fn assemble(
        &self,
        tools: &[String],
        model: &str,
        permission_mode: &str,
    ) -> AssembledContext {
        let git = if self.include_git {
            self.gather_git_context()
        } else {
            None
        };

        let instructions = if self.include_project_file {
            self.read_project_instructions()
        } else {
            None
        };

        let system_prompt = self.build_system_prompt(&git, &instructions, tools, model);
        let token_estimate = estimate_tokens(&system_prompt);

        let project_name = self.detect_project_name(&git);

        let working_directory = self
            .working_dir
            .canonicalize()
            .unwrap_or_else(|_| self.working_dir.clone())
            .to_string_lossy()
            .to_string();

        AssembledContext {
            system_prompt,
            project_name,
            working_directory,
            git_info: git,
            project_instructions: instructions,
            available_tools: tools.to_vec(),
            permission_mode: permission_mode.to_string(),
            model_info: model.to_string(),
            token_estimate,
        }
    }

    /// Gather git context for the working directory.
    ///
    /// Runs git CLI commands to collect branch, status, commits, and remote URL.
    /// Returns `None` if the directory is not a git repository.
    pub fn gather_git_context(&self) -> Option<GitContext> {
        // Check if this is a git repo by getting the branch
        let branch = run_git(&self.working_dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;

        // Get status
        let status_raw = run_git(&self.working_dir, &["status", "--porcelain"]);
        let (status_summary, is_dirty) = match &status_raw {
            Some(raw) => parse_status_summary(raw),
            None => ("unknown".to_string(), false),
        };

        // Get recent commits
        let commits_raw = run_git(&self.working_dir, &["log", "--oneline", "-5"]);
        let recent_commits = match commits_raw {
            Some(raw) => raw
                .lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
            None => vec![],
        };

        // Get remote URL
        let remote_url = run_git(&self.working_dir, &["remote", "get-url", "origin"]);

        Some(GitContext {
            branch,
            status_summary,
            recent_commits,
            remote_url,
            is_dirty,
        })
    }

    /// Read project instructions from well-known files.
    ///
    /// Searches in order: `ARGENTOR.md`, `CLAUDE.md`, `.argentor/instructions.md`,
    /// `.github/copilot-instructions.md`. Returns the contents of the first file found,
    /// truncated to the configured token budget.
    pub fn read_project_instructions(&self) -> Option<String> {
        let candidates = [
            "ARGENTOR.md",
            "CLAUDE.md",
            ".argentor/instructions.md",
            ".github/copilot-instructions.md",
        ];

        for candidate in &candidates {
            let path = self.working_dir.join(candidate);
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if contents.is_empty() {
                    continue;
                }
                return Some(truncate_to_tokens(&contents, self.max_instructions_tokens));
            }
        }

        None
    }

    /// Build the system prompt string from gathered context.
    pub fn build_system_prompt(
        &self,
        git: &Option<GitContext>,
        instructions: &Option<String>,
        tools: &[String],
        model: &str,
    ) -> String {
        let mut prompt = String::with_capacity(4096);

        // Header
        prompt.push_str(&format!(
            "You are Argentor, a secure AI assistant powered by {model}.\n\
             You have access to tools that you can use to help the user.\n\
             Each tool runs in a sandboxed environment with specific permissions.\n"
        ));

        // Working directory
        let wd = self
            .working_dir
            .canonicalize()
            .unwrap_or_else(|_| self.working_dir.clone());
        prompt.push_str(&format!("\n# Working Directory\n{}\n", wd.display()));

        // Git context
        if let Some(git) = git {
            prompt.push_str(&format!(
                "\n# Git Context\nBranch: {} ({})\n",
                git.branch, git.status_summary
            ));
            if !git.recent_commits.is_empty() {
                prompt.push_str("Recent commits:\n");
                for commit in &git.recent_commits {
                    prompt.push_str(&format!("  {commit}\n"));
                }
            }
            if let Some(url) = &git.remote_url {
                prompt.push_str(&format!("Remote: {url}\n"));
            }
        }

        // Project instructions
        if let Some(instructions) = instructions {
            prompt.push_str(&format!("\n# Project Instructions\n{instructions}\n"));
        }

        // Custom instructions
        if let Some(custom) = &self.custom_instructions {
            prompt.push_str(&format!("\n# Custom Instructions\n{custom}\n"));
        }

        // Available tools
        prompt.push_str(&format!("\n# Available Tools ({})\n", tools.len()));
        if tools.is_empty() {
            prompt.push_str("No tools available.\n");
        } else {
            for tool in tools {
                prompt.push_str(&format!("- {tool}\n"));
            }
        }

        // Footer
        prompt.push_str(
            "\nAlways explain what you're doing before using a tool.\n\
             Prefer the least-privilege tool for each task.\n",
        );

        prompt
    }

    /// Detect the project name from git remote URL or directory name.
    fn detect_project_name(&self, git: &Option<GitContext>) -> Option<String> {
        // Try to extract from git remote URL first
        if let Some(git) = git {
            if let Some(url) = &git.remote_url {
                if let Some(name) = extract_repo_name(url) {
                    return Some(name);
                }
            }
        }

        // Fall back to directory name
        self.working_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run a git command in the given directory, returning trimmed stdout or None.
fn run_git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        // For status --porcelain, empty is valid (clean repo)
        Some(String::new())
    } else {
        Some(stdout)
    }
}

/// Parse `git status --porcelain` output into a human-readable summary and dirty flag.
fn parse_status_summary(raw: &str) -> (String, bool) {
    if raw.is_empty() {
        return ("clean".to_string(), false);
    }

    let mut modified = 0u32;
    let mut added = 0u32;
    let mut deleted = 0u32;
    let mut untracked = 0u32;
    let mut renamed = 0u32;
    let mut other = 0u32;

    for line in raw.lines() {
        if line.len() < 2 {
            continue;
        }
        let xy = &line[..2];
        match xy.trim() {
            "M" | "MM" | "AM" => modified += 1,
            "A" => added += 1,
            "D" => deleted += 1,
            "R" => renamed += 1,
            "??" => untracked += 1,
            _ => other += 1,
        }
    }

    let mut parts = Vec::new();
    if modified > 0 {
        parts.push(format!("{modified} modified"));
    }
    if added > 0 {
        parts.push(format!("{added} added"));
    }
    if deleted > 0 {
        parts.push(format!("{deleted} deleted"));
    }
    if renamed > 0 {
        parts.push(format!("{renamed} renamed"));
    }
    if untracked > 0 {
        parts.push(format!("{untracked} untracked"));
    }
    if other > 0 {
        parts.push(format!("{other} other"));
    }

    let summary = if parts.is_empty() {
        "clean".to_string()
    } else {
        parts.join(", ")
    };

    (summary, true)
}

/// Estimate the token count of a string (1 token ~ 4 characters).
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Truncate text to fit within a token budget (1 token ~ 4 chars).
fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let truncated = &text[..max_chars];
        format!("{truncated}\n\n[... truncated to {max_tokens} tokens ...]")
    }
}

/// Extract a repository name from a git remote URL.
///
/// Handles both HTTPS (`https://github.com/user/repo.git`) and
/// SSH (`git@github.com:user/repo.git`) formats.
fn extract_repo_name(url: &str) -> Option<String> {
    let url = url.trim();

    // Try HTTPS format: https://github.com/user/repo.git
    if let Some(path) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 3 {
            let name = parts[parts.len() - 1].trim_end_matches(".git");
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    // Try SSH format: git@github.com:user/repo.git
    if url.contains(':') && url.contains('@') {
        if let Some(path) = url.split(':').nth(1) {
            let parts: Vec<&str> = path.split('/').collect();
            if let Some(last) = parts.last() {
                let name = last.trim_end_matches(".git");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    // -- GitContext gathering tests --

    #[test]
    fn test_gather_git_context_in_repo() {
        // The project root is a git repo, so this should succeed
        let assembler = ContextAssembler::new(env!("CARGO_MANIFEST_DIR"));
        let git = assembler.gather_git_context();
        assert!(git.is_some(), "Should detect git repo");
        let git = git.unwrap();
        assert!(!git.branch.is_empty(), "Branch should not be empty");
    }

    #[test]
    fn test_gather_git_context_non_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let git = assembler.gather_git_context();
        assert!(git.is_none(), "Non-repo dir should return None");
    }

    #[test]
    fn test_gather_git_context_with_init() {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        // Configure git user for the test repo
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        // Create a file and commit it
        fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let git = assembler.gather_git_context();
        assert!(git.is_some());
        let git = git.unwrap();
        assert!(!git.branch.is_empty());
        assert!(!git.recent_commits.is_empty());
        assert!(!git.is_dirty);
    }

    #[test]
    fn test_gather_git_context_dirty_repo() {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        // Initial commit
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        // Create untracked file
        fs::write(tmp.path().join("new.txt"), "dirty").unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let git = assembler.gather_git_context().unwrap();
        assert!(git.is_dirty);
        assert!(git.status_summary.contains("untracked"));
    }

    #[test]
    fn test_gather_git_context_no_remote() {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        fs::write(tmp.path().join("x.txt"), "x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let git = assembler.gather_git_context().unwrap();
        assert!(git.remote_url.is_none());
    }

    // -- Status parsing tests --

    #[test]
    fn test_parse_status_clean() {
        let (summary, dirty) = parse_status_summary("");
        assert_eq!(summary, "clean");
        assert!(!dirty);
    }

    #[test]
    fn test_parse_status_modified() {
        let (summary, dirty) = parse_status_summary(" M src/lib.rs\n M Cargo.toml\n");
        assert!(dirty);
        assert!(summary.contains("modified"));
    }

    #[test]
    fn test_parse_status_untracked() {
        let (summary, dirty) = parse_status_summary("?? new_file.rs\n?? another.rs\n");
        assert!(dirty);
        assert!(summary.contains("2 untracked"));
    }

    #[test]
    fn test_parse_status_mixed() {
        let raw = " M modified.rs\nA  added.rs\n?? untracked.txt\n D deleted.rs\n";
        let (summary, dirty) = parse_status_summary(raw);
        assert!(dirty);
        assert!(summary.contains("modified"));
        assert!(summary.contains("added"));
        assert!(summary.contains("untracked"));
        assert!(summary.contains("deleted"));
    }

    // -- Project instructions tests --

    #[test]
    fn test_read_argentor_md() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ARGENTOR.md"), "# Project\nDo X.").unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let instructions = assembler.read_project_instructions();
        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("Do X"));
    }

    #[test]
    fn test_read_claude_md_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        // No ARGENTOR.md, but CLAUDE.md exists
        fs::write(
            tmp.path().join("CLAUDE.md"),
            "# Claude Config\nStyle: concise.",
        )
        .unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let instructions = assembler.read_project_instructions();
        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("concise"));
    }

    #[test]
    fn test_read_argentor_dir_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".argentor")).unwrap();
        fs::write(
            tmp.path().join(".argentor/instructions.md"),
            "Custom instructions here.",
        )
        .unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let instructions = assembler.read_project_instructions();
        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("Custom instructions"));
    }

    #[test]
    fn test_read_copilot_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".github")).unwrap();
        fs::write(
            tmp.path().join(".github/copilot-instructions.md"),
            "Copilot rules.",
        )
        .unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let instructions = assembler.read_project_instructions();
        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("Copilot rules"));
    }

    #[test]
    fn test_read_no_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        assert!(assembler.read_project_instructions().is_none());
    }

    #[test]
    fn test_read_empty_file_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ARGENTOR.md"), "").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Fallback content.").unwrap();

        let assembler = ContextAssembler::new(tmp.path());
        let instructions = assembler.read_project_instructions();
        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("Fallback content"));
    }

    #[test]
    fn test_instructions_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let long_text = "x".repeat(50_000); // ~12500 tokens
        fs::write(tmp.path().join("ARGENTOR.md"), &long_text).unwrap();

        let assembler = ContextAssembler::new(tmp.path()).max_instructions_tokens(100);
        let instructions = assembler.read_project_instructions().unwrap();
        // 100 tokens * 4 chars = 400 chars max (before the truncation notice)
        assert!(instructions.len() < 500);
        assert!(instructions.contains("truncated"));
    }

    // -- System prompt assembly tests --

    #[test]
    fn test_build_prompt_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let prompt = assembler.build_system_prompt(&None, &None, &[], "claude-4");
        assert!(prompt.contains("Argentor"));
        assert!(prompt.contains("claude-4"));
        assert!(prompt.contains("No tools available"));
    }

    #[test]
    fn test_build_prompt_with_git() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let git = Some(GitContext {
            branch: "main".to_string(),
            status_summary: "clean".to_string(),
            recent_commits: vec!["abc1234 Initial commit".to_string()],
            remote_url: Some("https://github.com/test/repo.git".to_string()),
            is_dirty: false,
        });
        let prompt = assembler.build_system_prompt(&git, &None, &[], "claude-4");
        assert!(prompt.contains("main"));
        assert!(prompt.contains("clean"));
        assert!(prompt.contains("abc1234 Initial commit"));
        assert!(prompt.contains("https://github.com/test/repo.git"));
    }

    #[test]
    fn test_build_prompt_with_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let tools = vec!["calculator".to_string(), "web_search".to_string()];
        let prompt = assembler.build_system_prompt(&None, &None, &tools, "gpt-4");
        assert!(prompt.contains("Available Tools (2)"));
        assert!(prompt.contains("- calculator"));
        assert!(prompt.contains("- web_search"));
    }

    #[test]
    fn test_build_prompt_with_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let instructions = Some("Always respond in Spanish.".to_string());
        let prompt = assembler.build_system_prompt(&None, &instructions, &[], "claude-4");
        assert!(prompt.contains("Always respond in Spanish"));
        assert!(prompt.contains("Project Instructions"));
    }

    #[test]
    fn test_build_prompt_with_custom_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler =
            ContextAssembler::new(tmp.path()).with_custom_instructions("Be extra cautious.");
        let prompt = assembler.build_system_prompt(&None, &None, &[], "claude-4");
        assert!(prompt.contains("Be extra cautious"));
        assert!(prompt.contains("Custom Instructions"));
    }

    // -- Full assemble tests --

    #[test]
    fn test_assemble_full() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ARGENTOR.md"), "# Rules\nBe safe.").unwrap();

        let ctx = ContextAssembler::new(tmp.path()).with_git(false).assemble(
            &["echo".into()],
            "claude-4",
            "sandboxed",
        );

        assert!(ctx.system_prompt.contains("Be safe"));
        assert!(ctx.system_prompt.contains("echo"));
        assert_eq!(ctx.permission_mode, "sandboxed");
        assert_eq!(ctx.model_info, "claude-4");
        assert!(ctx.git_info.is_none());
        assert!(ctx.project_instructions.is_some());
        assert_eq!(ctx.available_tools, vec!["echo"]);
        assert!(ctx.token_estimate > 0);
    }

    #[test]
    fn test_assemble_no_git_no_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ContextAssembler::new(tmp.path())
            .with_git(false)
            .with_project_file(false)
            .assemble(&[], "test-model", "permissive");

        assert!(ctx.system_prompt.contains("Argentor"));
        assert!(ctx.git_info.is_none());
        assert!(ctx.project_instructions.is_none());
        assert!(ctx.available_tools.is_empty());
        assert_eq!(ctx.permission_mode, "permissive");
    }

    // -- Token estimation tests --

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_short() {
        // "hello" = 5 chars -> (5+3)/4 = 2 tokens
        assert_eq!(estimate_tokens("hello"), 2);
    }

    #[test]
    fn test_estimate_tokens_longer() {
        let text = "a".repeat(400);
        assert_eq!(estimate_tokens(&text), 100); // 400/4 = 100
    }

    // -- Truncation tests --

    #[test]
    fn test_truncate_short_text() {
        let result = truncate_to_tokens("short text", 1000);
        assert_eq!(result, "short text");
    }

    #[test]
    fn test_truncate_long_text() {
        let text = "x".repeat(1000);
        let result = truncate_to_tokens(&text, 10); // 10 tokens = 40 chars
        assert!(result.len() < 200);
        assert!(result.contains("truncated"));
    }

    // -- Repo name extraction tests --

    #[test]
    fn test_extract_repo_name_https() {
        let name = extract_repo_name("https://github.com/fboiero/Argentor.git");
        assert_eq!(name, Some("Argentor".to_string()));
    }

    #[test]
    fn test_extract_repo_name_https_no_git_suffix() {
        let name = extract_repo_name("https://github.com/user/myrepo");
        assert_eq!(name, Some("myrepo".to_string()));
    }

    #[test]
    fn test_extract_repo_name_ssh() {
        let name = extract_repo_name("git@github.com:fboiero/Argentor.git");
        assert_eq!(name, Some("Argentor".to_string()));
    }

    #[test]
    fn test_extract_repo_name_invalid() {
        assert!(extract_repo_name("not-a-url").is_none());
    }

    // -- Builder chaining tests --

    #[test]
    fn test_builder_chaining() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path())
            .with_git(false)
            .with_project_file(false)
            .with_custom_instructions("custom rule")
            .max_instructions_tokens(500);

        let ctx = assembler.assemble(&["tool_a".into()], "model-x", "strict");
        assert!(ctx.system_prompt.contains("custom rule"));
        assert_eq!(ctx.model_info, "model-x");
        assert_eq!(ctx.permission_mode, "strict");
    }

    // -- Project name detection tests --

    #[test]
    fn test_detect_project_name_from_git() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let git = Some(GitContext {
            branch: "main".to_string(),
            status_summary: "clean".to_string(),
            recent_commits: vec![],
            remote_url: Some("https://github.com/org/MyProject.git".to_string()),
            is_dirty: false,
        });
        let name = assembler.detect_project_name(&git);
        assert_eq!(name, Some("MyProject".to_string()));
    }

    #[test]
    fn test_detect_project_name_from_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let assembler = ContextAssembler::new(tmp.path());
        let name = assembler.detect_project_name(&None);
        // tempdir names are random but should exist
        assert!(name.is_some());
    }

    // -- Serialization test --

    #[test]
    fn test_assembled_context_serializable() {
        let ctx = AssembledContext {
            system_prompt: "test prompt".to_string(),
            project_name: Some("test".to_string()),
            working_directory: "/tmp".to_string(),
            git_info: Some(GitContext {
                branch: "main".to_string(),
                status_summary: "clean".to_string(),
                recent_commits: vec!["abc init".to_string()],
                remote_url: None,
                is_dirty: false,
            }),
            project_instructions: None,
            available_tools: vec!["echo".to_string()],
            permission_mode: "sandboxed".to_string(),
            model_info: "claude-4".to_string(),
            token_estimate: 42,
        };

        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: AssembledContext = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.project_name, Some("test".to_string()));
        assert_eq!(deserialized.token_estimate, 42);
        assert!(deserialized.git_info.is_some());
    }
}

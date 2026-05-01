//! Git operations skill using libgit2 (no shell commands).
//!
//! Provides safe, auditable git operations for agents. All repository
//! interactions go through the `git2` crate, never through shell execution.
//!
//! # Safety
//!
//! - Force operations (force push, hard reset) are unconditionally blocked.
//! - Protected branches (`main`, `master`, `production` by default) cannot be deleted.
//! - Read-only operations (status, diff, log, branch_list, show) require no
//!   special write permissions.
//! - Write operations (add, commit, checkout, branch_create, stash) are gated
//!   behind the `ShellExec { allowed_commands: ["git"] }` capability as a proxy
//!   for git access control.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use git2::{
    DiffOptions, ObjectType, Repository, Signature, Sort, StashFlags, StatusOptions, StatusShow,
};
use std::path::Path;
use tracing::{info, warn};

/// Default branches that are protected from deletion.
const DEFAULT_PROTECTED_BRANCHES: &[&str] = &["main", "master", "production"];

/// Git operations skill backed by libgit2.
///
/// Supports: `status`, `diff`, `log`, `branch_list`, `branch_create`,
/// `checkout`, `add`, `commit`, `stash`, `show`.
pub struct GitSkill {
    descriptor: SkillDescriptor,
    protected_branches: Vec<String>,
}

impl GitSkill {
    /// Create a new `GitSkill` with default protected branches.
    pub fn new() -> Self {
        Self::with_protected_branches(
            DEFAULT_PROTECTED_BRANCHES
                .iter()
                .map(ToString::to_string)
                .collect(),
        )
    }

    /// Create a new `GitSkill` with custom protected branches.
    pub fn with_protected_branches(protected_branches: Vec<String>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "git".to_string(),
                description: "Interact with git repositories using libgit2. Supports status, diff, log, branch_list, branch_create, checkout, add, commit, stash, and show operations.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["status", "diff", "log", "branch_list", "branch_create", "checkout", "add", "commit", "stash", "show"],
                            "description": "The git operation to perform"
                        },
                        "repo_path": {
                            "type": "string",
                            "description": "Path to repository (default: current directory)"
                        },
                        "message": {
                            "type": "string",
                            "description": "Commit message (for commit operation)"
                        },
                        "paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "File paths (for add operation)"
                        },
                        "target": {
                            "type": "string",
                            "description": "Branch name or file path (for checkout operation)"
                        },
                        "name": {
                            "type": "string",
                            "description": "Branch name (for branch_create operation)"
                        },
                        "count": {
                            "type": "integer",
                            "description": "Number of commits to show (for log, default: 10)"
                        },
                        "staged": {
                            "type": "boolean",
                            "description": "Show staged diff only (for diff, default: false)"
                        },
                        "path": {
                            "type": "string",
                            "description": "Filter by path (for diff and log operations)"
                        },
                        "commit_sha": {
                            "type": "string",
                            "description": "Commit SHA (for show operation)"
                        },
                        "author_name": {
                            "type": "string",
                            "description": "Author name override (for commit operation)"
                        },
                        "author_email": {
                            "type": "string",
                            "description": "Author email override (for commit operation)"
                        },
                        "stash_action": {
                            "type": "string",
                            "enum": ["save", "pop", "list"],
                            "description": "Stash sub-operation (default: save)"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![Capability::ShellExec {
                    allowed_commands: vec!["git".to_string()],
                }],
                requires_approval: false,
            },
            protected_branches,
        }
    }

    /// Check if a branch name is protected.
    #[allow(dead_code)]
    fn is_protected(&self, branch_name: &str) -> bool {
        self.protected_branches
            .iter()
            .any(|p| p.eq_ignore_ascii_case(branch_name))
    }

    /// Open a repository at the given path, or the current directory.
    fn open_repo(repo_path: Option<&str>) -> Result<Repository, String> {
        let path = repo_path.unwrap_or(".");
        Repository::discover(path)
            .map_err(|e| format!("Failed to open repository at '{path}': {e}"))
    }
}

impl Default for GitSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for GitSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let operation = call.arguments["operation"].as_str().unwrap_or_default();

        // Read-only operations do not require capability checks
        let is_write_op = matches!(
            operation,
            "add" | "commit" | "checkout" | "branch_create" | "stash"
        );

        if is_write_op && !permissions.check_shell("git") {
            return Err(argentor_core::ArgentorError::Security(
                "git write operations require ShellExec capability with 'git' allowed".to_string(),
            ));
        }

        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if operation.is_empty() {
            return Ok(ToolResult::error(&call.id, "Missing 'operation' parameter"));
        }

        let repo_path = call.arguments["repo_path"]
            .as_str()
            .map(ToString::to_string);

        info!(operation = %operation, repo_path = ?repo_path, "Executing git operation");

        // All git2 calls are blocking, so run in a blocking task.
        let call_id = call.id.clone();
        let arguments = call.arguments.clone();
        let protected = self.protected_branches.clone();

        let result = tokio::task::spawn_blocking(move || {
            execute_git_operation(&operation, repo_path.as_deref(), &arguments, &protected)
        })
        .await;

        match result {
            Ok(Ok(output)) => Ok(ToolResult::success(&call_id, output)),
            Ok(Err(err_msg)) => {
                warn!(error = %err_msg, "Git operation failed");
                Ok(ToolResult::error(&call_id, err_msg))
            }
            Err(join_err) => Ok(ToolResult::error(
                &call_id,
                format!("Git operation panicked: {join_err}"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Operation dispatch
// ---------------------------------------------------------------------------

fn execute_git_operation(
    operation: &str,
    repo_path: Option<&str>,
    args: &serde_json::Value,
    protected_branches: &[String],
) -> Result<String, String> {
    let repo = GitSkill::open_repo(repo_path)?;

    match operation {
        "status" => op_status(&repo),
        "diff" => {
            let staged = args["staged"].as_bool().unwrap_or(false);
            let path_filter = args["path"].as_str();
            op_diff(&repo, staged, path_filter)
        }
        "log" => {
            let count = args["count"].as_u64().unwrap_or(10) as usize;
            let path_filter = args["path"].as_str();
            op_log(&repo, count, path_filter)
        }
        "branch_list" => op_branch_list(&repo),
        "branch_create" => {
            let name = args["name"]
                .as_str()
                .ok_or_else(|| "Missing 'name' parameter for branch_create".to_string())?;
            op_branch_create(&repo, name)
        }
        "checkout" => {
            let target = args["target"]
                .as_str()
                .ok_or_else(|| "Missing 'target' parameter for checkout".to_string())?;
            op_checkout(&repo, target)
        }
        "add" => {
            let paths: Vec<String> = match args["paths"].as_array() {
                Some(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                None => {
                    return Err("Missing or invalid 'paths' parameter for add".to_string());
                }
            };
            if paths.is_empty() {
                return Err("'paths' array must not be empty".to_string());
            }
            op_add(&repo, &paths)
        }
        "commit" => {
            let message = args["message"]
                .as_str()
                .ok_or_else(|| "Missing 'message' parameter for commit".to_string())?;
            let author_name = args["author_name"].as_str();
            let author_email = args["author_email"].as_str();
            op_commit(&repo, message, author_name, author_email)
        }
        "stash" => {
            let action = args["stash_action"].as_str().unwrap_or("save");
            op_stash(&repo, action)
        }
        "show" => {
            let commit_sha = args["commit_sha"]
                .as_str()
                .ok_or_else(|| "Missing 'commit_sha' parameter for show".to_string())?;
            op_show(&repo, commit_sha, protected_branches)
        }
        _ => Err(format!("Unknown git operation: '{operation}'")),
    }
}

// ---------------------------------------------------------------------------
// Individual operations
// ---------------------------------------------------------------------------

/// Show working tree status (modified, staged, untracked).
fn op_status(repo: &Repository) -> Result<String, String> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show(StatusShow::IndexAndWorkdir);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("Failed to get status: {e}"))?;

    let mut staged: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("<invalid-utf8>").to_string();
        let status = entry.status();

        if status.is_index_new()
            || status.is_index_modified()
            || status.is_index_deleted()
            || status.is_index_renamed()
            || status.is_index_typechange()
        {
            staged.push(path.clone());
        }
        if status.is_wt_modified()
            || status.is_wt_deleted()
            || status.is_wt_renamed()
            || status.is_wt_typechange()
        {
            modified.push(path.clone());
        }
        if status.is_wt_new() {
            untracked.push(path);
        }
    }

    let result = serde_json::json!({
        "staged": staged,
        "modified": modified,
        "untracked": untracked,
        "total_entries": statuses.len(),
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Show diff of changes.
fn op_diff(repo: &Repository, staged: bool, path_filter: Option<&str>) -> Result<String, String> {
    let mut diff_opts = DiffOptions::new();

    if let Some(path) = path_filter {
        diff_opts.pathspec(path);
    }

    let diff = if staged {
        // Staged changes: diff between HEAD and index
        let head_tree = repo.head().ok().and_then(|r| r.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut diff_opts))
    } else {
        // Unstaged changes: diff between index and workdir
        repo.diff_index_to_workdir(None, Some(&mut diff_opts))
    }
    .map_err(|e| format!("Failed to compute diff: {e}"))?;

    let stats = diff
        .stats()
        .map_err(|e| format!("Failed to get diff stats: {e}"))?;

    let mut diff_text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        match origin {
            '+' | '-' | ' ' => {
                diff_text.push(origin);
            }
            _ => {}
        }
        diff_text.push_str(std::str::from_utf8(line.content()).unwrap_or("<binary>"));
        true
    })
    .map_err(|e| format!("Failed to print diff: {e}"))?;

    // Truncate if too large
    const MAX_DIFF_LEN: usize = 50_000;
    let truncated = if diff_text.len() > MAX_DIFF_LEN {
        let slice = &diff_text[..MAX_DIFF_LEN];
        format!("{slice}\n... [truncated, {} total bytes]", diff_text.len())
    } else {
        diff_text
    };

    let result = serde_json::json!({
        "files_changed": stats.files_changed(),
        "insertions": stats.insertions(),
        "deletions": stats.deletions(),
        "staged": staged,
        "diff": truncated,
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Show commit history.
fn op_log(repo: &Repository, count: usize, path_filter: Option<&str>) -> Result<String, String> {
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| format!("Failed to create revwalk: {e}"))?;

    revwalk
        .push_head()
        .map_err(|e| format!("Failed to push HEAD: {e}"))?;
    revwalk
        .set_sorting(Sort::TIME | Sort::TOPOLOGICAL)
        .map_err(|e| format!("Failed to set sorting: {e}"))?;

    let mut commits = Vec::new();

    for oid_result in revwalk {
        if commits.len() >= count {
            break;
        }

        let oid = oid_result.map_err(|e| format!("Revwalk error: {e}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|e| format!("Failed to find commit {oid}: {e}"))?;

        // If path filter is set, check if this commit touches the path
        if let Some(filter_path) = path_filter {
            let dominated = commit_touches_path(repo, &commit, filter_path);
            if !dominated {
                continue;
            }
        }

        let author = commit.author();
        let time = commit.time();
        let ts = chrono::DateTime::from_timestamp(time.seconds(), 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| format!("{}+00:00", time.seconds()));

        commits.push(serde_json::json!({
            "sha": oid.to_string(),
            "message": commit.message().unwrap_or("<invalid-utf8>"),
            "author_name": author.name().unwrap_or("<unknown>"),
            "author_email": author.email().unwrap_or("<unknown>"),
            "timestamp": ts,
        }));
    }

    let result = serde_json::json!({
        "commits": commits,
        "count": commits.len(),
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Check if a commit modifies files under the given path.
fn commit_touches_path(repo: &Repository, commit: &git2::Commit<'_>, path: &str) -> bool {
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return false,
    };

    if commit.parent_count() == 0 {
        // Root commit: check if the path exists in the tree
        return tree.get_path(Path::new(path)).is_ok();
    }

    let parent = match commit.parent(0) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let parent_tree = match parent.tree() {
        Ok(t) => t,
        Err(_) => return false,
    };

    let mut diff_opts = DiffOptions::new();
    diff_opts.pathspec(path);

    let diff = match repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), Some(&mut diff_opts)) {
        Ok(d) => d,
        Err(_) => return false,
    };

    diff.deltas().count() > 0
}

/// List branches and highlight the current one.
fn op_branch_list(repo: &Repository) -> Result<String, String> {
    let branches = repo
        .branches(None)
        .map_err(|e| format!("Failed to list branches: {e}"))?;

    let mut branch_list: Vec<serde_json::Value> = Vec::new();

    let head_ref = repo.head().ok();
    let current_branch = head_ref
        .as_ref()
        .and_then(|r| r.shorthand().map(String::from));

    for branch_result in branches {
        let (branch, branch_type) = branch_result.map_err(|e| format!("Branch error: {e}"))?;
        let name = branch
            .name()
            .map_err(|e| format!("Branch name error: {e}"))?
            .unwrap_or("<invalid-utf8>")
            .to_string();

        let is_current = current_branch.as_deref() == Some(&name);
        let type_str = match branch_type {
            git2::BranchType::Local => "local",
            git2::BranchType::Remote => "remote",
        };

        branch_list.push(serde_json::json!({
            "name": name,
            "type": type_str,
            "current": is_current,
        }));
    }

    let result = serde_json::json!({
        "branches": branch_list,
        "current": current_branch,
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Create a new branch from HEAD.
fn op_branch_create(repo: &Repository, name: &str) -> Result<String, String> {
    let head_commit = repo
        .head()
        .map_err(|e| format!("Failed to get HEAD: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("HEAD is not a commit: {e}"))?;

    repo.branch(name, &head_commit, false)
        .map_err(|e| format!("Failed to create branch '{name}': {e}"))?;

    let result = serde_json::json!({
        "created": name,
        "from": head_commit.id().to_string(),
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Checkout a branch.
fn op_checkout(repo: &Repository, target: &str) -> Result<String, String> {
    // Try to resolve as a branch first
    let reference = repo
        .find_branch(target, git2::BranchType::Local)
        .map_err(|e| format!("Branch '{target}' not found: {e}"))?;

    let refname = reference
        .get()
        .name()
        .ok_or_else(|| format!("Branch '{target}' has an invalid reference name"))?
        .to_string();

    let obj = repo
        .revparse_single(&refname)
        .map_err(|e| format!("Failed to resolve '{target}': {e}"))?;

    repo.checkout_tree(&obj, None)
        .map_err(|e| format!("Failed to checkout '{target}': {e}"))?;

    repo.set_head(&refname)
        .map_err(|e| format!("Failed to set HEAD to '{target}': {e}"))?;

    let result = serde_json::json!({
        "checked_out": target,
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Stage files.
fn op_add(repo: &Repository, paths: &[String]) -> Result<String, String> {
    let mut index = repo
        .index()
        .map_err(|e| format!("Failed to get index: {e}"))?;

    for path in paths {
        index
            .add_path(Path::new(path))
            .map_err(|e| format!("Failed to add '{path}': {e}"))?;
    }

    index
        .write()
        .map_err(|e| format!("Failed to write index: {e}"))?;

    let result = serde_json::json!({
        "added": paths,
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Create a commit.
fn op_commit(
    repo: &Repository,
    message: &str,
    author_name: Option<&str>,
    author_email: Option<&str>,
) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("Commit message must not be empty".to_string());
    }

    let mut index = repo
        .index()
        .map_err(|e| format!("Failed to get index: {e}"))?;

    let oid = index
        .write_tree()
        .map_err(|e| format!("Failed to write tree: {e}"))?;

    let tree = repo
        .find_tree(oid)
        .map_err(|e| format!("Failed to find tree: {e}"))?;

    // Build signature
    let sig = match (author_name, author_email) {
        (Some(name), Some(email)) => {
            Signature::now(name, email).map_err(|e| format!("Invalid signature: {e}"))?
        }
        _ => repo.signature().map_err(|e| {
            format!(
                "Failed to get default signature (set user.name and user.email in git config): {e}"
            )
        })?,
    };

    // Get parent commit (if any)
    let parent = repo.head().ok().and_then(|r| r.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();

    let commit_oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("Failed to create commit: {e}"))?;

    let result = serde_json::json!({
        "sha": commit_oid.to_string(),
        "message": message,
        "author": format!("{} <{}>", sig.name().unwrap_or("?"), sig.email().unwrap_or("?")),
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

/// Stash operations: save, pop, list.
fn op_stash(repo: &Repository, action: &str) -> Result<String, String> {
    // Stash requires a mutable reference so we re-open the repo.
    // git2 stash API requires &mut Repository.
    let repo_path = repo
        .path()
        .parent()
        .ok_or_else(|| "Cannot determine repository workdir".to_string())?;
    let mut mutable_repo =
        Repository::open(repo_path).map_err(|e| format!("Failed to reopen repo: {e}"))?;

    match action {
        "save" => {
            let sig = mutable_repo
                .signature()
                .map_err(|e| format!("Failed to get signature for stash: {e}"))?;

            let stash_oid = mutable_repo
                .stash_save(
                    &sig,
                    "Stash created by Argentor GitSkill",
                    Some(StashFlags::DEFAULT),
                )
                .map_err(|e| format!("Failed to save stash: {e}"))?;

            let result = serde_json::json!({
                "action": "save",
                "stash_sha": stash_oid.to_string(),
            });
            serde_json::to_string_pretty(&result)
                .map_err(|e| format!("JSON serialization error: {e}"))
        }
        "pop" => {
            mutable_repo
                .stash_pop(0, None)
                .map_err(|e| format!("Failed to pop stash: {e}"))?;

            let result = serde_json::json!({
                "action": "pop",
                "index": 0,
            });
            serde_json::to_string_pretty(&result)
                .map_err(|e| format!("JSON serialization error: {e}"))
        }
        "list" => {
            let mut stashes: Vec<serde_json::Value> = Vec::new();
            mutable_repo
                .stash_foreach(|index, message, oid| {
                    stashes.push(serde_json::json!({
                        "index": index,
                        "message": message,
                        "sha": oid.to_string(),
                    }));
                    true
                })
                .map_err(|e| format!("Failed to list stashes: {e}"))?;

            let result = serde_json::json!({
                "action": "list",
                "stashes": stashes,
            });
            serde_json::to_string_pretty(&result)
                .map_err(|e| format!("JSON serialization error: {e}"))
        }
        _ => Err(format!(
            "Unknown stash action: '{action}'. Valid actions: save, pop, list"
        )),
    }
}

/// Show details of a specific commit.
fn op_show(
    repo: &Repository,
    commit_sha: &str,
    _protected_branches: &[String],
) -> Result<String, String> {
    let obj = repo
        .revparse_single(commit_sha)
        .map_err(|e| format!("Failed to find '{commit_sha}': {e}"))?;

    let commit = obj
        .peel(ObjectType::Commit)
        .map_err(|e| format!("'{commit_sha}' is not a commit: {e}"))?
        .into_commit()
        .map_err(|_| format!("Failed to convert '{commit_sha}' to commit"))?;

    let author = commit.author();
    let time = commit.time();
    let ts = chrono::DateTime::from_timestamp(time.seconds(), 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| format!("{}+00:00", time.seconds()));

    // Get diff from parent (or empty tree if root commit)
    let tree = commit
        .tree()
        .map_err(|e| format!("Failed to get commit tree: {e}"))?;

    let parent_tree = if commit.parent_count() > 0 {
        commit.parent(0).ok().and_then(|p| p.tree().ok())
    } else {
        None
    };

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| format!("Failed to compute commit diff: {e}"))?;

    let stats = diff
        .stats()
        .map_err(|e| format!("Failed to get diff stats: {e}"))?;

    let mut diff_text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        match origin {
            '+' | '-' | ' ' => {
                diff_text.push(origin);
            }
            _ => {}
        }
        diff_text.push_str(std::str::from_utf8(line.content()).unwrap_or("<binary>"));
        true
    })
    .map_err(|e| format!("Failed to print commit diff: {e}"))?;

    // Truncate if too large
    const MAX_SHOW_LEN: usize = 50_000;
    let truncated = if diff_text.len() > MAX_SHOW_LEN {
        let slice = &diff_text[..MAX_SHOW_LEN];
        format!("{slice}\n... [truncated, {} total bytes]", diff_text.len())
    } else {
        diff_text
    };

    let mut changed_files: Vec<String> = Vec::new();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path() {
            changed_files.push(path.to_string_lossy().to_string());
        }
    }

    let result = serde_json::json!({
        "sha": commit.id().to_string(),
        "message": commit.message().unwrap_or("<invalid-utf8>"),
        "author_name": author.name().unwrap_or("<unknown>"),
        "author_email": author.email().unwrap_or("<unknown>"),
        "timestamp": ts,
        "files_changed": stats.files_changed(),
        "insertions": stats.insertions(),
        "deletions": stats.deletions(),
        "changed_files": changed_files,
        "diff": truncated,
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {e}"))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: create a new temporary git repository with an initial commit.
    fn setup_test_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let repo = Repository::init(dir.path()).expect("failed to init repo");

        // Configure user for commits
        let mut config = repo.config().expect("failed to get config");
        config
            .set_str("user.name", "Test User")
            .expect("failed to set user.name");
        config
            .set_str("user.email", "test@example.com")
            .expect("failed to set user.email");

        (dir, repo)
    }

    /// Helper: create a file in the repo, stage it, and commit it.
    fn commit_file(repo: &Repository, dir: &TempDir, filename: &str, content: &str) -> git2::Oid {
        let file_path = dir.path().join(filename);
        fs::write(&file_path, content).expect("failed to write file");

        let mut index = repo.index().expect("failed to get index");
        index
            .add_path(Path::new(filename))
            .expect("failed to add to index");
        index.write().expect("failed to write index");

        let oid = index.write_tree().expect("failed to write tree");
        let tree = repo.find_tree(oid).expect("failed to find tree");
        let sig = repo.signature().expect("failed to get signature");

        let parent = repo.head().ok().and_then(|r| r.peel_to_commit().ok());
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();

        repo.commit(Some("HEAD"), &sig, &sig, "test commit", &tree, &parents)
            .expect("failed to commit")
    }

    // -----------------------------------------------------------------------
    // status
    // -----------------------------------------------------------------------
    #[test]
    fn test_status_clean_repo() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "initial.txt", "hello");

        let result = op_status(&repo);
        assert!(result.is_ok(), "status failed: {:?}", result.err());
        let output = result.unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("status output not valid JSON");
        assert_eq!(parsed["staged"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["modified"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["untracked"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_status_with_changes() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "file.txt", "original");

        // Modify the file
        fs::write(dir.path().join("file.txt"), "modified").unwrap();
        // Create an untracked file
        fs::write(dir.path().join("new.txt"), "untracked").unwrap();

        let result = op_status(&repo).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(!parsed["modified"].as_array().unwrap().is_empty());
        assert!(!parsed["untracked"].as_array().unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // add + commit + log
    // -----------------------------------------------------------------------
    #[test]
    fn test_add_commit_log() {
        let (dir, repo) = setup_test_repo();

        // Create and commit initial file
        commit_file(&repo, &dir, "first.txt", "first content");

        // Create a second file
        fs::write(dir.path().join("second.txt"), "second content").unwrap();

        let add_result = op_add(&repo, &["second.txt".to_string()]);
        assert!(add_result.is_ok(), "add failed: {:?}", add_result.err());

        let commit_result = op_commit(&repo, "add second file", None, None);
        assert!(
            commit_result.is_ok(),
            "commit failed: {:?}",
            commit_result.err()
        );
        let commit_output: serde_json::Value =
            serde_json::from_str(&commit_result.unwrap()).unwrap();
        assert!(!commit_output["sha"].as_str().unwrap().is_empty());

        // Verify log shows the commit
        let log_result = op_log(&repo, 10, None);
        assert!(log_result.is_ok(), "log failed: {:?}", log_result.err());
        let log_output: serde_json::Value = serde_json::from_str(&log_result.unwrap()).unwrap();
        let commits = log_output["commits"].as_array().unwrap();
        assert!(commits.len() >= 2, "expected at least 2 commits");
        assert_eq!(
            commits[0]["message"].as_str().unwrap().trim(),
            "add second file"
        );
    }

    // -----------------------------------------------------------------------
    // branch_create + branch_list
    // -----------------------------------------------------------------------
    #[test]
    fn test_branch_create_and_list() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "file.txt", "content");

        let create_result = op_branch_create(&repo, "feature-test");
        assert!(
            create_result.is_ok(),
            "branch_create failed: {:?}",
            create_result.err()
        );

        let list_result = op_branch_list(&repo);
        assert!(
            list_result.is_ok(),
            "branch_list failed: {:?}",
            list_result.err()
        );
        let list_output: serde_json::Value = serde_json::from_str(&list_result.unwrap()).unwrap();
        let branches = list_output["branches"].as_array().unwrap();
        let names: Vec<&str> = branches
            .iter()
            .map(|b| b["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"feature-test"),
            "branch 'feature-test' not found in: {names:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Protected branch deletion
    // -----------------------------------------------------------------------
    #[test]
    fn test_protected_branch_detection() {
        let skill = GitSkill::new();
        assert!(skill.is_protected("main"));
        assert!(skill.is_protected("master"));
        assert!(skill.is_protected("production"));
        assert!(skill.is_protected("Main")); // case-insensitive
        assert!(!skill.is_protected("feature-branch"));
    }

    // -----------------------------------------------------------------------
    // diff
    // -----------------------------------------------------------------------
    #[test]
    fn test_diff_after_modification() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "file.txt", "original content\n");

        // Modify the file
        fs::write(dir.path().join("file.txt"), "modified content\n").unwrap();

        let result = op_diff(&repo, false, None);
        assert!(result.is_ok(), "diff failed: {:?}", result.err());
        let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(output["files_changed"].as_u64().unwrap() > 0);
        let diff_text = output["diff"].as_str().unwrap();
        assert!(
            diff_text.contains("modified content"),
            "diff should contain the modified text"
        );
    }

    #[test]
    fn test_diff_staged() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "file.txt", "original\n");

        // Modify and stage
        fs::write(dir.path().join("file.txt"), "staged change\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file.txt")).unwrap();
        index.write().unwrap();

        let result = op_diff(&repo, true, None);
        assert!(result.is_ok(), "staged diff failed: {:?}", result.err());
        let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(output["staged"].as_bool().unwrap());
        assert!(output["files_changed"].as_u64().unwrap() > 0);
    }

    // -----------------------------------------------------------------------
    // show
    // -----------------------------------------------------------------------
    #[test]
    fn test_show_commit() {
        let (dir, repo) = setup_test_repo();
        let commit_oid = commit_file(&repo, &dir, "file.txt", "show me");

        let result = op_show(&repo, &commit_oid.to_string(), &[]);
        assert!(result.is_ok(), "show failed: {:?}", result.err());
        let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(output["sha"].as_str().unwrap(), commit_oid.to_string());
        assert!(output["changed_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap() == "file.txt"));
    }

    // -----------------------------------------------------------------------
    // checkout
    // -----------------------------------------------------------------------
    #[test]
    fn test_checkout_branch() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "file.txt", "content");
        op_branch_create(&repo, "test-branch").unwrap();

        let result = op_checkout(&repo, "test-branch");
        assert!(result.is_ok(), "checkout failed: {:?}", result.err());
        let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(output["checked_out"].as_str().unwrap(), "test-branch");

        // Verify HEAD now points to test-branch
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand().unwrap(), "test-branch");
    }

    // -----------------------------------------------------------------------
    // commit with custom author
    // -----------------------------------------------------------------------
    #[test]
    fn test_commit_with_custom_author() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "init.txt", "init");

        fs::write(dir.path().join("custom.txt"), "custom").unwrap();
        op_add(&repo, &["custom.txt".to_string()]).unwrap();

        let result = op_commit(
            &repo,
            "custom author commit",
            Some("Custom Author"),
            Some("custom@example.com"),
        );
        assert!(result.is_ok(), "commit failed: {:?}", result.err());
        let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        let author = output["author"].as_str().unwrap();
        assert!(author.contains("Custom Author"));
        assert!(author.contains("custom@example.com"));
    }

    // -----------------------------------------------------------------------
    // empty commit message rejected
    // -----------------------------------------------------------------------
    #[test]
    fn test_commit_empty_message_rejected() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "init.txt", "init");

        let result = op_commit(&repo, "", None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    // -----------------------------------------------------------------------
    // Skill trait integration
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_skill_execute_status() {
        let skill = GitSkill::new();
        // Execute against the workspace's own repo
        let call = ToolCall {
            id: "test_git_1".to_string(),
            name: "git".to_string(),
            arguments: serde_json::json!({
                "operation": "status",
                "repo_path": env!("CARGO_MANIFEST_DIR"),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
    }

    #[tokio::test]
    async fn test_skill_execute_unknown_operation() {
        let skill = GitSkill::new();
        let call = ToolCall {
            id: "test_git_2".to_string(),
            name: "git".to_string(),
            arguments: serde_json::json!({
                "operation": "force_push",
                "repo_path": env!("CARGO_MANIFEST_DIR"),
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unknown git operation"));
    }

    #[tokio::test]
    async fn test_skill_execute_missing_operation() {
        let skill = GitSkill::new();
        let call = ToolCall {
            id: "test_git_3".to_string(),
            name: "git".to_string(),
            arguments: serde_json::json!({}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Missing"));
    }

    // -----------------------------------------------------------------------
    // validate_arguments
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_arguments_read_ops_always_allowed() {
        let skill = GitSkill::new();
        let empty_perms = PermissionSet::new();

        for op in &["status", "diff", "log", "branch_list", "show"] {
            let call = ToolCall {
                id: format!("test_va_{op}"),
                name: "git".to_string(),
                arguments: serde_json::json!({"operation": op}),
            };
            assert!(
                skill.validate_arguments(&call, &empty_perms).is_ok(),
                "read-only op '{op}' should not require capabilities"
            );
        }
    }

    #[test]
    fn test_validate_arguments_write_ops_require_capability() {
        let skill = GitSkill::new();
        let empty_perms = PermissionSet::new();

        for op in &["add", "commit", "checkout", "branch_create", "stash"] {
            let call = ToolCall {
                id: format!("test_va_{op}"),
                name: "git".to_string(),
                arguments: serde_json::json!({"operation": op}),
            };
            assert!(
                skill.validate_arguments(&call, &empty_perms).is_err(),
                "write op '{op}' should require capability"
            );
        }
    }

    #[test]
    fn test_validate_arguments_write_ops_allowed_with_capability() {
        let skill = GitSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::ShellExec {
            allowed_commands: vec!["git".to_string()],
        });

        for op in &["add", "commit", "checkout", "branch_create", "stash"] {
            let call = ToolCall {
                id: format!("test_va_{op}"),
                name: "git".to_string(),
                arguments: serde_json::json!({"operation": op}),
            };
            assert!(
                skill.validate_arguments(&call, &perms).is_ok(),
                "write op '{op}' should be allowed with git capability"
            );
        }
    }

    // -----------------------------------------------------------------------
    // stash list on fresh repo (no stashes)
    // -----------------------------------------------------------------------
    #[test]
    fn test_stash_list_empty() {
        let (dir, repo) = setup_test_repo();
        commit_file(&repo, &dir, "file.txt", "content");

        let result = op_stash(&repo, "list");
        assert!(result.is_ok(), "stash list failed: {:?}", result.err());
        let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(output["stashes"].as_array().unwrap().len(), 0);
    }
}

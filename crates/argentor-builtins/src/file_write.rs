use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::path::Path;
use tracing::info;

const MAX_WRITE_SIZE: usize = 10 * 1024 * 1024; // 10MB

/// File writing skill. Writes content to files with path validation.
pub struct FileWriteSkill {
    descriptor: SkillDescriptor,
}

impl FileWriteSkill {
    /// Create a new file write skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "file_write".to_string(),
                description: "Write content to a file. Path must be within allowed directories."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        },
                        "append": {
                            "type": "boolean",
                            "description": "Append to file instead of overwriting (default: false)"
                        },
                        "create_dirs": {
                            "type": "boolean",
                            "description": "Create parent directories if they don't exist (default: false)"
                        }
                    },
                    "required": ["path", "content"]
                }),
                required_capabilities: vec![Capability::FileWrite {
                    allowed_paths: vec![], // Configured at runtime
                }],
                requires_approval: false,
            },
        }
    }
}

impl Default for FileWriteSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for FileWriteSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    fn validate_arguments(
        &self,
        call: &ToolCall,
        permissions: &PermissionSet,
    ) -> ArgentorResult<()> {
        let path_str = call.arguments["path"].as_str().unwrap_or_default();
        if path_str.is_empty() {
            return Ok(()); // Empty path will be caught in execute()
        }

        let path = Path::new(path_str);

        // For writes the file may not exist yet, so canonicalize the parent directory.
        let canonical = if path.exists() {
            match path.canonicalize() {
                Ok(p) => p,
                Err(_) => return Ok(()), // Let execute() handle the error
            }
        } else if let Some(parent) = path.parent() {
            if parent.exists() {
                match parent.canonicalize() {
                    Ok(p) => p.join(path.file_name().unwrap_or_default()),
                    Err(_) => return Ok(()),
                }
            } else {
                // Parent doesn't exist either — use the path as-is for the check.
                // This may happen with create_dirs=true.
                path.to_path_buf()
            }
        } else {
            return Ok(());
        };

        if !permissions.check_file_write_path(&canonical) {
            return Err(ArgentorError::Security(format!(
                "file write not permitted for path '{}'",
                canonical.display()
            )));
        }

        Ok(())
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let path_str = call.arguments["path"].as_str().unwrap_or_default();

        if path_str.is_empty() {
            return Ok(ToolResult::error(&call.id, "Empty path"));
        }

        let content = call.arguments["content"].as_str().unwrap_or_default();
        let append = call.arguments["append"].as_bool().unwrap_or(false);
        let create_dirs = call.arguments["create_dirs"].as_bool().unwrap_or(false);

        // Size check
        if content.len() > MAX_WRITE_SIZE {
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "Content too large: {} bytes (max: {} bytes)",
                    content.len(),
                    MAX_WRITE_SIZE
                ),
            ));
        }

        let path = Path::new(path_str);

        // Must be absolute
        if !path.is_absolute() {
            return Ok(ToolResult::error(
                &call.id,
                format!("Path must be absolute: '{path_str}'"),
            ));
        }

        // Block writing to sensitive locations
        let blocked_patterns = [
            "/etc/",
            "/usr/",
            "/bin/",
            "/sbin/",
            "/boot/",
            "/proc/",
            "/sys/",
            ".ssh/",
            ".env",
            ".bashrc",
            ".zshrc",
            ".profile",
            ".gitconfig",
            "credentials",
            "id_rsa",
            "id_ed25519",
        ];

        let path_lower = path_str.to_lowercase();
        for pattern in &blocked_patterns {
            if path_lower.contains(pattern) {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Access denied: '{path_str}' matches blocked pattern"),
                ));
            }
        }

        // Block writing executable files
        let blocked_extensions = [".sh", ".bash", ".exe", ".bat", ".cmd", ".ps1"];
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = format!(".{}", ext.to_lowercase());
            if blocked_extensions.contains(&ext_lower.as_str()) {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Access denied: writing executable files ({ext_lower}) is not allowed"),
                ));
            }
        }

        // Create parent directories if requested
        if create_dirs {
            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return Ok(ToolResult::error(
                        &call.id,
                        format!("Failed to create directories for '{path_str}': {e}"),
                    ));
                }
            }
        }

        // Write or append
        let result = if append {
            use tokio::io::AsyncWriteExt;
            let mut file = match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    return Ok(ToolResult::error(
                        &call.id,
                        format!("Failed to open '{path_str}' for append: {e}"),
                    ));
                }
            };
            file.write_all(content.as_bytes()).await
        } else {
            tokio::fs::write(path, content).await
        };

        match result {
            Ok(()) => {
                info!(path = %path_str, size = content.len(), append = append, "File written");
                let response = serde_json::json!({
                    "path": path_str,
                    "bytes_written": content.len(),
                    "append": append,
                });
                Ok(ToolResult::success(&call.id, response.to_string()))
            }
            Err(e) => Ok(ToolResult::error(
                &call.id,
                format!("Failed to write '{path_str}': {e}"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_write_and_read_back() {
        let skill = FileWriteSkill::new();
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let path_str = file_path.to_str().unwrap();

        let call = ToolCall {
            id: "test_1".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": path_str,
                "content": "Hello, Argentor!"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "Hello, Argentor!");
    }

    #[tokio::test]
    async fn test_file_write_append() {
        let skill = FileWriteSkill::new();
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("append.txt");
        let path_str = file_path.to_str().unwrap();

        // Write initial content
        let call1 = ToolCall {
            id: "test_2a".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": path_str,
                "content": "Line 1\n"
            }),
        };
        skill.execute(call1).await.unwrap();

        // Append more
        let call2 = ToolCall {
            id: "test_2b".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": path_str,
                "content": "Line 2\n",
                "append": true
            }),
        };
        let result = skill.execute(call2).await.unwrap();
        assert!(!result.is_error);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "Line 1\nLine 2\n");
    }

    #[tokio::test]
    async fn test_file_write_create_dirs() {
        let skill = FileWriteSkill::new();
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("a/b/c/deep.txt");
        let path_str = file_path.to_str().unwrap();

        let call = ToolCall {
            id: "test_3".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": path_str,
                "content": "deep file",
                "create_dirs": true
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "deep file");
    }

    #[tokio::test]
    async fn test_file_write_blocked_path() {
        let skill = FileWriteSkill::new();
        let call = ToolCall {
            id: "test_4".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": "/etc/passwd",
                "content": "malicious"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("blocked"));
    }

    #[tokio::test]
    async fn test_file_write_blocks_executables() {
        let skill = FileWriteSkill::new();
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("evil.sh");
        let path_str = file_path.to_str().unwrap();

        let call = ToolCall {
            id: "test_5".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": path_str,
                "content": "#!/bin/bash\nrm -rf /"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("executable"));
    }

    #[tokio::test]
    async fn test_file_write_empty_path() {
        let skill = FileWriteSkill::new();
        let call = ToolCall {
            id: "test_6".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({"path": "", "content": "x"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_file_write_relative_path_rejected() {
        let skill = FileWriteSkill::new();
        let call = ToolCall {
            id: "test_7".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": "relative/path.txt",
                "content": "x"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("absolute"));
    }

    #[tokio::test]
    async fn test_file_write_blocks_ssh_key() {
        let skill = FileWriteSkill::new();
        let call = ToolCall {
            id: "test_8".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({
                "path": "/home/user/.ssh/id_rsa",
                "content": "fake key"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_validate_arguments_denies_disallowed_path() {
        let skill = FileWriteSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileWrite {
            allowed_paths: vec!["/allowed".to_string()],
        });

        let call = ToolCall {
            id: "test_va_1".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({"path": "/tmp/some_file.txt", "content": "x"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_arguments_allows_permitted_path() {
        let skill = FileWriteSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileWrite {
            allowed_paths: vec!["/tmp".to_string()],
        });

        let call = ToolCall {
            id: "test_va_2".to_string(),
            name: "file_write".to_string(),
            arguments: serde_json::json!({"path": "/tmp/some_file.txt", "content": "x"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_ok());
    }
}

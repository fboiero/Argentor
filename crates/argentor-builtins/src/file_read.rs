use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::{Capability, PermissionSet};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::path::Path;
use tracing::info;

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

/// File reading skill. Reads file contents with path validation.
pub struct FileReadSkill {
    descriptor: SkillDescriptor,
}

impl FileReadSkill {
    /// Create a new file read skill.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "file_read".to_string(),
                description:
                    "Read the contents of a file. Path must be within allowed directories."
                        .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute path to the file to read"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Byte offset to start reading from (default: 0)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum bytes to read (default: entire file, max: 10MB)"
                        }
                    },
                    "required": ["path"]
                }),
                required_capabilities: vec![Capability::FileRead {
                    allowed_paths: vec![], // Configured at runtime
                }],
                requires_approval: false,
            },
        }
    }
}

impl Default for FileReadSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for FileReadSkill {
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

        // Canonicalize the path to resolve symlinks and ".." components.
        // If the file doesn't exist yet, canonicalize the parent directory.
        let canonical = if path.exists() {
            match path.canonicalize() {
                Ok(p) => p,
                Err(_) => return Ok(()), // Let execute() handle the error
            }
        } else if let Some(parent) = path.parent() {
            match parent.canonicalize() {
                Ok(p) => p.join(path.file_name().unwrap_or_default()),
                Err(_) => return Ok(()), // Let execute() handle the error
            }
        } else {
            return Ok(());
        };

        if !permissions.check_file_read_path(&canonical) {
            return Err(ArgentorError::Security(format!(
                "file read not permitted for path '{}'",
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

        let path = Path::new(path_str);

        // Resolve symlinks and canonicalize to prevent path traversal
        let canonical = match tokio::fs::canonicalize(path).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Cannot resolve path '{path_str}': {e}"),
                ));
            }
        };

        // Block reading sensitive files
        let blocked_patterns = [
            "/etc/shadow",
            "/etc/passwd",
            ".ssh/",
            ".env",
            "credentials",
            "secret",
            ".aws/",
        ];
        let canonical_str = canonical.to_string_lossy();
        for pattern in &blocked_patterns {
            if canonical_str.contains(pattern) {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Access denied: '{path_str}' matches blocked pattern"),
                ));
            }
        }

        // Check file size
        let metadata = match tokio::fs::metadata(&canonical).await {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!("Cannot read metadata for '{path_str}': {e}"),
                ));
            }
        };

        if !metadata.is_file() {
            return Ok(ToolResult::error(
                &call.id,
                format!("'{path_str}' is not a file"),
            ));
        }

        if metadata.len() > MAX_FILE_SIZE {
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "File too large: {} bytes (max: {} bytes)",
                    metadata.len(),
                    MAX_FILE_SIZE
                ),
            ));
        }

        info!(path = %canonical.display(), size = metadata.len(), "Reading file");

        let content = match tokio::fs::read_to_string(&canonical).await {
            Ok(c) => c,
            Err(e) => {
                // Try reading as binary if not valid UTF-8
                match tokio::fs::read(&canonical).await {
                    Ok(bytes) => {
                        return Ok(ToolResult::success(
                            &call.id,
                            serde_json::json!({
                                "path": canonical_str,
                                "size": bytes.len(),
                                "encoding": "binary",
                                "note": format!("File is not valid UTF-8: {}", e),
                            })
                            .to_string(),
                        ));
                    }
                    Err(e2) => {
                        return Ok(ToolResult::error(
                            &call.id,
                            format!("Failed to read '{path_str}': {e2}"),
                        ));
                    }
                }
            }
        };

        let offset = call.arguments["offset"].as_u64().unwrap_or(0) as usize;
        let limit = call.arguments["limit"]
            .as_u64()
            .map(|l| l as usize)
            .unwrap_or(content.len());

        let slice = if offset < content.len() {
            &content[offset..content.len().min(offset + limit)]
        } else {
            ""
        };

        let response = serde_json::json!({
            "path": canonical_str,
            "size": metadata.len(),
            "content": slice,
        });

        Ok(ToolResult::success(&call.id, response.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_read_self() {
        let skill = FileReadSkill::new();
        // Read our own source file using absolute path
        let path = format!("{}/src/file_read.rs", env!("CARGO_MANIFEST_DIR"));
        let call = ToolCall {
            id: "test_1".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": path}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error, "Result: {}", result.content);
        assert!(result.content.contains("FileReadSkill"));
    }

    #[tokio::test]
    async fn test_file_read_blocked_path() {
        let skill = FileReadSkill::new();
        let call = ToolCall {
            id: "test_2".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/etc/shadow"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_file_read_nonexistent() {
        let skill = FileReadSkill::new();
        let call = ToolCall {
            id: "test_3".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/argentor_nonexistent_file_12345"}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_file_read_empty_path() {
        let skill = FileReadSkill::new();
        let call = ToolCall {
            id: "test_4".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": ""}),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_validate_arguments_denies_disallowed_path() {
        let skill = FileReadSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileRead {
            allowed_paths: vec!["/allowed".to_string()],
        });

        // Use a path that exists so canonicalize works — /tmp always exists
        let call = ToolCall {
            id: "test_va_1".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/some_file.txt"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_arguments_allows_permitted_path() {
        let skill = FileReadSkill::new();
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileRead {
            allowed_paths: vec!["/tmp".to_string()],
        });

        let call = ToolCall {
            id: "test_va_2".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/some_file.txt"}),
        };
        let result = skill.validate_arguments(&call, &perms);
        assert!(result.is_ok());
    }
}

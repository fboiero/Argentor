use crate::skill::{Skill, SkillDescriptor};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// YAML frontmatter parsed from a markdown skill file.
#[derive(Debug, Clone, Deserialize)]
pub struct MarkdownFrontmatter {
    /// Skill name.
    pub name: String,
    /// Short description of what the skill does.
    pub description: String,
    /// Optional tool-group this skill belongs to.
    #[serde(default)]
    pub group: Option<String>,
    /// Optional JSON Schema for the skill's parameters.
    #[serde(default)]
    pub parameters_schema: Option<serde_json::Value>,
    /// Required capabilities as string identifiers.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// If true, this skill's content is injected into the system prompt
    /// rather than being available as a callable tool.
    #[serde(default)]
    pub prompt_injection: bool,
}

/// A skill defined as a markdown file with YAML frontmatter.
///
/// File format:
/// ```markdown
/// ---
/// name: code_review
/// description: Reviews code for security and quality
/// group: coding
/// prompt_injection: true
/// ---
///
/// You are a code reviewer. When reviewing code:
/// 1. Check for security vulnerabilities
/// 2. Verify error handling
/// ...
/// ```
///
/// When `prompt_injection: true`, the content is appended to the agent's system prompt.
/// When `prompt_injection: false` (default), the skill is a callable tool that returns its content.
pub struct MarkdownSkill {
    descriptor: SkillDescriptor,
    content: String,
    source_path: PathBuf,
    frontmatter: MarkdownFrontmatter,
}

impl MarkdownSkill {
    /// Parse a markdown file into a MarkdownSkill.
    pub fn from_file(path: &Path) -> ArgentorResult<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            ArgentorError::Config(format!(
                "Failed to read markdown skill {}: {}",
                path.display(),
                e
            ))
        })?;

        Self::parse(&raw, path.to_path_buf())
    }

    /// Parse markdown content with YAML frontmatter.
    pub fn parse(raw: &str, source_path: PathBuf) -> ArgentorResult<Self> {
        let (frontmatter, content) = Self::split_frontmatter(raw)?;

        let capabilities: Vec<Capability> = frontmatter
            .capabilities
            .iter()
            .filter_map(|c| Self::parse_capability(c))
            .collect();

        let parameters_schema = frontmatter.parameters_schema.clone().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional query to filter the skill's guidance"
                    }
                }
            })
        });

        let descriptor = SkillDescriptor {
            name: frontmatter.name.clone(),
            description: frontmatter.description.clone(),
            parameters_schema,
            required_capabilities: capabilities,
            requires_approval: false,
        };

        Ok(Self {
            descriptor,
            content,
            source_path,
            frontmatter,
        })
    }

    /// Split raw markdown into frontmatter and body content.
    fn split_frontmatter(raw: &str) -> ArgentorResult<(MarkdownFrontmatter, String)> {
        let trimmed = raw.trim();

        if !trimmed.starts_with("---") {
            return Err(ArgentorError::Config(
                "Markdown skill must start with YAML frontmatter (---)".to_string(),
            ));
        }

        // Find the closing ---
        let after_open = &trimmed[3..];
        let close_pos = after_open.find("---").ok_or_else(|| {
            ArgentorError::Config(
                "Markdown skill missing closing frontmatter delimiter (---)".to_string(),
            )
        })?;

        let yaml_str = &after_open[..close_pos];
        let content = after_open[close_pos + 3..].trim().to_string();

        let frontmatter: MarkdownFrontmatter = serde_yaml_ng::from_str(yaml_str)
            .map_err(|e| ArgentorError::Config(format!("Invalid YAML frontmatter: {e}")))?;

        Ok((frontmatter, content))
    }

    /// Parse a capability string like "file_read:/tmp" into a Capability.
    fn parse_capability(s: &str) -> Option<Capability> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        match parts[0] {
            "file_read" => Some(Capability::FileRead {
                allowed_paths: parts
                    .get(1)
                    .map(|p| vec![p.to_string()])
                    .unwrap_or_default(),
            }),
            "file_write" => Some(Capability::FileWrite {
                allowed_paths: parts
                    .get(1)
                    .map(|p| vec![p.to_string()])
                    .unwrap_or_default(),
            }),
            "network" => Some(Capability::NetworkAccess {
                allowed_hosts: parts
                    .get(1)
                    .map(|h| vec![h.to_string()])
                    .unwrap_or_default(),
            }),
            "shell" => Some(Capability::ShellExec {
                allowed_commands: parts
                    .get(1)
                    .map(|c| vec![c.to_string()])
                    .unwrap_or_default(),
            }),
            _ => {
                warn!(capability = %s, "Unknown capability in markdown skill");
                None
            }
        }
    }

    /// Get the markdown content (body, without frontmatter).
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Whether this skill should be injected into the system prompt.
    pub fn is_prompt_injection(&self) -> bool {
        self.frontmatter.prompt_injection
    }

    /// Get the optional group name.
    pub fn group(&self) -> Option<&str> {
        self.frontmatter.group.as_deref()
    }

    /// Get the source file path.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

#[async_trait]
impl Skill for MarkdownSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        // For callable markdown skills, return the content as guidance
        let query: Option<String> = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);

        let response = if let Some(q) = query {
            format!(
                "## {} — Guidance for: {}\n\n{}",
                self.descriptor.name, q, self.content
            )
        } else {
            format!("## {}\n\n{}", self.descriptor.name, self.content)
        };

        Ok(ToolResult::success(&call.id, &response))
    }
}

/// Loads markdown skills from a directory, with hot-reload support.
pub struct MarkdownSkillLoader {
    skills_dir: PathBuf,
    /// Cached skills indexed by file path.
    cache: Arc<RwLock<HashMap<PathBuf, Arc<MarkdownSkill>>>>,
}

impl MarkdownSkillLoader {
    /// Create a new loader that reads `.md` skills from the given directory.
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all `.md` files from the skills directory.
    /// Returns (callable_skills, prompt_injections).
    pub async fn load_all(&self) -> ArgentorResult<LoadedMarkdownSkills> {
        let mut callable = Vec::new();
        let mut prompts = Vec::new();

        if !self.skills_dir.exists() {
            info!(dir = %self.skills_dir.display(), "Markdown skills directory not found, skipping");
            return Ok(LoadedMarkdownSkills { callable, prompts });
        }

        let entries = std::fs::read_dir(&self.skills_dir).map_err(|e| {
            ArgentorError::Config(format!(
                "Failed to read markdown skills dir {}: {}",
                self.skills_dir.display(),
                e
            ))
        })?;

        let mut cache = self.cache.write().await;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "Failed to read directory entry");
                    continue;
                }
            };

            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            match MarkdownSkill::from_file(&path) {
                Ok(skill) => {
                    let skill = Arc::new(skill);
                    cache.insert(path.clone(), skill.clone());

                    if skill.is_prompt_injection() {
                        info!(name = %skill.descriptor().name, path = %path.display(), "Loaded prompt injection skill");
                        prompts.push(skill);
                    } else {
                        info!(name = %skill.descriptor().name, path = %path.display(), "Loaded callable markdown skill");
                        callable.push(skill);
                    }
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to parse markdown skill, skipping");
                }
            }
        }

        info!(
            callable = callable.len(),
            prompts = prompts.len(),
            "Markdown skills loaded"
        );

        Ok(LoadedMarkdownSkills { callable, prompts })
    }

    /// Reload a single file (for hot-reload).
    pub async fn reload_file(&self, path: &Path) -> ArgentorResult<Option<Arc<MarkdownSkill>>> {
        if !path.exists() {
            // File deleted — remove from cache
            let mut cache = self.cache.write().await;
            cache.remove(path);
            info!(path = %path.display(), "Removed deleted markdown skill");
            return Ok(None);
        }

        let skill = Arc::new(MarkdownSkill::from_file(path)?);
        let mut cache = self.cache.write().await;
        cache.insert(path.to_path_buf(), skill.clone());
        info!(name = %skill.descriptor().name, path = %path.display(), "Reloaded markdown skill");
        Ok(Some(skill))
    }

    /// Get all currently cached skills.
    pub async fn cached_skills(&self) -> Vec<Arc<MarkdownSkill>> {
        let cache = self.cache.read().await;
        cache.values().cloned().collect()
    }
}

/// Result of loading markdown skills from a directory.
pub struct LoadedMarkdownSkills {
    /// Skills that can be called as tools.
    pub callable: Vec<Arc<MarkdownSkill>>,
    /// Skills whose content is injected into the system prompt.
    pub prompts: Vec<Arc<MarkdownSkill>>,
}

impl LoadedMarkdownSkills {
    /// Build a combined prompt injection string from all prompt skills.
    pub fn build_prompt_injection(&self) -> String {
        if self.prompts.is_empty() {
            return String::new();
        }

        let mut parts = Vec::new();
        parts.push("\n\n## Additional Instructions\n".to_string());

        for skill in &self.prompts {
            parts.push(format!(
                "### {}\n{}\n",
                skill.descriptor().name,
                skill.content()
            ));
        }

        parts.join("\n")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE_CALLABLE: &str = r#"---
name: code_review
description: Reviews code for security and quality
group: coding
---

When reviewing code, check for:
1. Security vulnerabilities (OWASP Top 10)
2. Error handling completeness
3. Input validation
4. Resource cleanup
"#;

    const SAMPLE_PROMPT: &str = r#"---
name: rust_conventions
description: Rust coding conventions
group: coding
prompt_injection: true
---

Follow these Rust conventions:
- Use `ArgentorResult<T>` for fallible operations
- No `unwrap()` in production code
- Use `tracing` for logging
"#;

    #[test]
    fn test_parse_callable_skill() {
        let skill = MarkdownSkill::parse(SAMPLE_CALLABLE, PathBuf::from("test.md")).unwrap();
        assert_eq!(skill.descriptor().name, "code_review");
        assert_eq!(skill.group(), Some("coding"));
        assert!(!skill.is_prompt_injection());
        assert!(skill.content().contains("OWASP"));
    }

    #[test]
    fn test_parse_prompt_injection() {
        let skill = MarkdownSkill::parse(SAMPLE_PROMPT, PathBuf::from("test.md")).unwrap();
        assert_eq!(skill.descriptor().name, "rust_conventions");
        assert!(skill.is_prompt_injection());
        assert!(skill.content().contains("ArgentorResult"));
    }

    #[test]
    fn test_missing_frontmatter() {
        let result = MarkdownSkill::parse("No frontmatter here", PathBuf::from("bad.md"));
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_yaml() {
        let raw = "---\n[invalid yaml\n---\nContent";
        let result = MarkdownSkill::parse(raw, PathBuf::from("bad.md"));
        assert!(result.is_err());
    }

    #[test]
    fn test_capabilities_parsing() {
        let raw = r#"---
name: file_tool
description: Reads files
capabilities:
  - "file_read:/tmp"
  - "network:api.example.com"
---
Content here
"#;
        let skill = MarkdownSkill::parse(raw, PathBuf::from("test.md")).unwrap();
        assert_eq!(skill.descriptor().required_capabilities.len(), 2);
    }

    #[test]
    fn test_build_prompt_injection() {
        let s1 = Arc::new(MarkdownSkill::parse(SAMPLE_PROMPT, PathBuf::from("a.md")).unwrap());
        let loaded = LoadedMarkdownSkills {
            callable: vec![],
            prompts: vec![s1],
        };
        let prompt = loaded.build_prompt_injection();
        assert!(prompt.contains("## Additional Instructions"));
        assert!(prompt.contains("rust_conventions"));
        assert!(prompt.contains("ArgentorResult"));
    }

    #[test]
    fn test_empty_prompt_injection() {
        let loaded = LoadedMarkdownSkills {
            callable: vec![],
            prompts: vec![],
        };
        assert!(loaded.build_prompt_injection().is_empty());
    }

    #[tokio::test]
    async fn test_loader_nonexistent_dir() {
        let loader = MarkdownSkillLoader::new(PathBuf::from("/nonexistent/dir"));
        let result = loader.load_all().await.unwrap();
        assert!(result.callable.is_empty());
        assert!(result.prompts.is_empty());
    }

    #[tokio::test]
    async fn test_loader_with_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("test_skill.md");
        std::fs::write(&skill_path, SAMPLE_CALLABLE).unwrap();

        let loader = MarkdownSkillLoader::new(dir.path().to_path_buf());
        let result = loader.load_all().await.unwrap();
        assert_eq!(result.callable.len(), 1);
        assert_eq!(result.prompts.len(), 0);
        assert_eq!(result.callable[0].descriptor().name, "code_review");
    }

    #[tokio::test]
    async fn test_loader_mixed_skills() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("callable.md"), SAMPLE_CALLABLE).unwrap();
        std::fs::write(dir.path().join("prompt.md"), SAMPLE_PROMPT).unwrap();
        // Non-.md file should be ignored
        std::fs::write(dir.path().join("readme.txt"), "ignored").unwrap();

        let loader = MarkdownSkillLoader::new(dir.path().to_path_buf());
        let result = loader.load_all().await.unwrap();
        assert_eq!(result.callable.len(), 1);
        assert_eq!(result.prompts.len(), 1);
    }

    #[tokio::test]
    async fn test_reload_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skill.md");
        std::fs::write(&path, SAMPLE_CALLABLE).unwrap();

        let loader = MarkdownSkillLoader::new(dir.path().to_path_buf());
        let _ = loader.load_all().await.unwrap();

        // Modify file
        let updated = SAMPLE_CALLABLE.replace("code_review", "updated_review");
        std::fs::write(&path, updated).unwrap();

        let reloaded = loader.reload_file(&path).await.unwrap().unwrap();
        assert_eq!(reloaded.descriptor().name, "updated_review");
    }
}

use crate::skill::{Skill, SkillDescriptor};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::PermissionSet;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// A named group of skills for progressive tool disclosure.
///
/// Tool groups allow exposing different sets of skills depending on context:
/// - `minimal`: basic utilities only (echo, time, help)
/// - `coding`: file operations, shell, memory
/// - `web`: HTTP fetch, browser
/// - `messaging`: Slack, Discord, Telegram
/// - `full`: all registered skills
/// - Custom groups defined in argentor.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGroup {
    /// Unique group name (e.g., "minimal", "coding", "web").
    pub name: String,
    /// Human-readable description of this group's purpose.
    pub description: String,
    /// Skill names included in this group.
    pub skills: Vec<String>,
}

impl ToolGroup {
    /// Create a new tool group with the given name, description, and skill list.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        skills: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            skills,
        }
    }
}

/// Default tool groups that ship with Argentor.
pub fn default_tool_groups() -> Vec<ToolGroup> {
    vec![
        ToolGroup::new(
            "minimal",
            "Basic utilities — safe for any context",
            vec![
                "echo".into(),
                "time".into(),
                "help".into(),
                "calculator".into(),
                "datetime".into(),
            ],
        ),
        ToolGroup::new(
            "coding",
            "File operations, shell, and memory for development tasks",
            vec![
                "file_read".into(),
                "file_write".into(),
                "shell".into(),
                "memory_store".into(),
                "memory_search".into(),
                "regex".into(),
                "json_query".into(),
                "diff".into(),
                "calculator".into(),
            ],
        ),
        ToolGroup::new(
            "web",
            "HTTP, browser, search, and scraping for web tasks",
            vec![
                "http_fetch".into(),
                "browser".into(),
                "web_search".into(),
                "web_scraper".into(),
                "rss_reader".into(),
                "dns_lookup".into(),
            ],
        ),
        ToolGroup::new(
            "orchestration",
            "Skills for the orchestrator agent — delegation, approval, artifacts",
            vec![
                "agent_delegate".into(),
                "task_status".into(),
                "human_approval".into(),
                "artifact_store".into(),
                "memory_search".into(),
            ],
        ),
        ToolGroup::new(
            "development",
            "Extended development tools — git, analysis, testing, files, shell, memory",
            vec![
                "git".into(),
                "code_analysis".into(),
                "test_runner".into(),
                "file_read".into(),
                "file_write".into(),
                "shell".into(),
                "memory_store".into(),
                "memory_search".into(),
                "regex".into(),
                "json_query".into(),
                "diff".into(),
                "calculator".into(),
                "hash".into(),
                "encode_decode".into(),
                "secret_scanner".into(),
            ],
        ),
        ToolGroup::new(
            "devops",
            "DevOps tools — shell, HTTP, file operations, memory",
            vec![
                "shell".into(),
                "http_fetch".into(),
                "file_read".into(),
                "file_write".into(),
                "memory_store".into(),
                "memory_search".into(),
                "dns_lookup".into(),
                "hash".into(),
                "secret_scanner".into(),
            ],
        ),
        ToolGroup::new(
            "data",
            "Data processing — JSON, text, regex, validation, encoding",
            vec![
                "calculator".into(),
                "text_transform".into(),
                "json_query".into(),
                "regex".into(),
                "data_validator".into(),
                "datetime".into(),
                "encode_decode".into(),
                "hash".into(),
                "uuid_generator".into(),
                "diff".into(),
                "summarizer".into(),
            ],
        ),
        ToolGroup::new(
            "security",
            "Security scanning — prompt guard, secret detection, hashing",
            vec![
                "prompt_guard".into(),
                "secret_scanner".into(),
                "hash".into(),
                "data_validator".into(),
                "encode_decode".into(),
            ],
        ),
        ToolGroup::new(
            "full",
            "All registered skills — use with caution",
            vec![], // Empty = all skills
        ),
    ]
}

/// Central registry for all available skills.
///
/// `SkillRegistry` uses interior mutability (`RwLock`) so it can be shared
/// across threads via `Arc<SkillRegistry>` without requiring exclusive (`&mut`)
/// access for registration.
///
/// # Examples
///
/// ```rust
/// use argentor_skills::SkillRegistry;
///
/// let registry = SkillRegistry::new();
/// assert_eq!(registry.skill_count(), 0);
///
/// // List available tool groups:
/// let groups = registry.list_groups();
/// assert!(!groups.is_empty()); // default groups are pre-loaded
/// ```
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, Arc<dyn Skill>>>,
    groups: RwLock<HashMap<String, ToolGroup>>,
}

impl SkillRegistry {
    /// Create a new registry with default tool groups.
    pub fn new() -> Self {
        let groups: HashMap<String, ToolGroup> = default_tool_groups()
            .into_iter()
            .map(|g| (g.name.clone(), g))
            .collect();

        Self {
            skills: RwLock::new(HashMap::new()),
            groups: RwLock::new(groups),
        }
    }

    /// Register a skill in the registry.
    pub fn register(&self, skill: Arc<dyn Skill>) {
        let name = skill.descriptor().name.clone();
        info!(skill = %name, "Registered skill");
        self.skills
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name, skill);
    }

    /// Register a custom tool group.
    pub fn register_group(&self, group: ToolGroup) {
        info!(group = %group.name, skills = group.skills.len(), "Registered tool group");
        self.groups
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(group.name.clone(), group);
    }

    /// Register multiple custom tool groups.
    pub fn register_groups(&self, groups: Vec<ToolGroup>) {
        for group in groups {
            self.register_group(group);
        }
    }

    /// Look up a skill by name. Returns an owned `Arc` clone.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// List descriptors for all registered skills (owned clones).
    pub fn list_descriptors(&self) -> Vec<SkillDescriptor> {
        self.skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|s| s.descriptor().clone())
            .collect()
    }

    /// List all registered tool groups (owned clones).
    pub fn list_groups(&self) -> Vec<ToolGroup> {
        self.groups
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Get a tool group by name (owned clone).
    pub fn get_group(&self, name: &str) -> Option<ToolGroup> {
        self.groups
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// Execute a tool call, checking permissions first.
    ///
    /// 1. Verify the permission set contains a capability of the right *type*
    ///    (e.g. any `FileRead`).
    /// 2. Call `validate_arguments` on the skill so it can check argument-level
    ///    constraints (e.g. the specific file path against `allowed_paths`).
    /// 3. Execute the skill.
    pub async fn execute(
        &self,
        call: ToolCall,
        permissions: &PermissionSet,
    ) -> ArgentorResult<ToolResult> {
        let skill = self
            .get(&call.name)
            .ok_or_else(|| ArgentorError::Skill(format!("Unknown skill: {}", call.name)))?;

        // Check that the permission set has at least one capability of the required type.
        for cap in &skill.descriptor().required_capabilities {
            let type_name = cap.type_name();
            if !permissions.has_capability_type(type_name) {
                warn!(
                    skill = %call.name,
                    capability = ?cap,
                    "Permission denied for skill execution"
                );
                return Ok(ToolResult::error(
                    &call.id,
                    format!(
                        "Permission denied: skill '{}' requires capability {:?}",
                        call.name, cap
                    ),
                ));
            }
        }

        // Validate specific arguments against the permission set.
        if let Err(e) = skill.validate_arguments(&call, permissions) {
            warn!(
                skill = %call.name,
                error = %e,
                "Argument-level permission denied"
            );
            return Ok(ToolResult::error(
                &call.id,
                format!(
                    "Permission denied: argument validation failed for '{}': {}",
                    call.name, e
                ),
            ));
        }

        skill.execute(call).await
    }

    /// Execute multiple tool calls concurrently, returning results in the same
    /// order as the input.
    ///
    /// Each call goes through the same permission checking and validation as
    /// [`execute()`](Self::execute). Individual failures do not abort the batch;
    /// the corresponding slot contains the error while successful calls return
    /// normally.
    ///
    /// Because the concurrent tasks need shared access to the registry, this
    /// method requires the registry to be wrapped in an `Arc`.
    pub async fn execute_parallel(
        self: &Arc<Self>,
        calls: Vec<ToolCall>,
        permissions: &PermissionSet,
    ) -> Vec<ArgentorResult<ToolResult>> {
        if calls.is_empty() {
            return Vec::new();
        }

        let batch_size = calls.len();
        let batch_start = std::time::Instant::now();

        let mut handles = Vec::with_capacity(batch_size);

        for call in calls {
            let registry = Arc::clone(self);
            let perms = permissions.clone();
            let handle = tokio::spawn(async move { registry.execute(call, &perms).await });
            handles.push(handle);
        }

        let mut results = Vec::with_capacity(batch_size);
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(join_err) => {
                    results.push(Err(ArgentorError::Skill(format!(
                        "Task panicked during parallel execution: {join_err}"
                    ))));
                }
            }
        }

        let elapsed = batch_start.elapsed();
        info!(
            batch_size = batch_size,
            elapsed_ms = elapsed.as_millis(),
            "Parallel tool execution completed"
        );

        results
    }

    /// Execute a single tool call with a timeout.
    ///
    /// Wraps [`execute()`](Self::execute) with a `tokio::time::timeout`.
    /// Returns [`ArgentorError::Skill`] if the timeout elapses before the
    /// skill finishes.
    pub async fn execute_with_timeout(
        &self,
        call: ToolCall,
        permissions: &PermissionSet,
        timeout: std::time::Duration,
    ) -> ArgentorResult<ToolResult> {
        match tokio::time::timeout(timeout, self.execute(call, permissions)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(ArgentorError::Skill("Tool execution timed out".to_string())),
        }
    }

    /// Return only descriptors for skills whose names appear in the given list.
    /// Used for progressive tool disclosure in multi-agent orchestration.
    pub fn filter_by_names(&self, names: &[String]) -> Vec<SkillDescriptor> {
        let allowed: std::collections::HashSet<&str> =
            names.iter().map(std::string::String::as_str).collect();
        self.skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|s| allowed.contains(s.descriptor().name.as_str()))
            .map(|s| s.descriptor().clone())
            .collect()
    }

    /// Create a new registry containing only skills whose names appear in the given list.
    /// Used for progressive tool disclosure — each worker agent gets only the skills it needs.
    pub fn filter_to_new(&self, names: &[String]) -> Self {
        let allowed: std::collections::HashSet<&str> =
            names.iter().map(std::string::String::as_str).collect();
        let skills: HashMap<String, Arc<dyn Skill>> = self
            .skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(name, _)| allowed.contains(name.as_str()))
            .map(|(name, skill)| (name.clone(), skill.clone()))
            .collect();
        let groups = self
            .groups
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Self {
            skills: RwLock::new(skills),
            groups: RwLock::new(groups),
        }
    }

    /// Create a new registry containing only skills in the specified tool group.
    /// If the group has an empty skills list (like "full"), returns all skills.
    pub fn filter_by_group(&self, group_name: &str) -> ArgentorResult<Self> {
        let group = self
            .get_group(group_name)
            .ok_or_else(|| ArgentorError::Config(format!("Unknown tool group: {group_name}")))?;

        if group.skills.is_empty() {
            // "full" group — return everything
            let skills = self
                .skills
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let groups = self
                .groups
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            return Ok(Self {
                skills: RwLock::new(skills),
                groups: RwLock::new(groups),
            });
        }

        Ok(self.filter_to_new(&group.skills))
    }

    /// Get skill names that belong to a specific group.
    pub fn skills_in_group(&self, group_name: &str) -> Vec<String> {
        let groups = self
            .groups
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let skills = self
            .skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match groups.get(group_name) {
            Some(group) if group.skills.is_empty() => {
                // "full" = all skills
                skills.keys().cloned().collect()
            }
            Some(group) => {
                // Return only skills that exist in both the group definition and registry
                group
                    .skills
                    .iter()
                    .filter(|name| skills.contains_key(name.as_str()))
                    .cloned()
                    .collect()
            }
            None => vec![],
        }
    }

    /// Return the total number of registered skills.
    pub fn skill_count(&self) -> usize {
        self.skills
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::skill::{Skill, SkillDescriptor};
    use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
    use argentor_security::{Capability, PermissionSet};
    use async_trait::async_trait;

    struct TestSkill {
        descriptor: SkillDescriptor,
    }

    impl TestSkill {
        fn new(name: &str) -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: name.to_string(),
                    description: format!("Test skill {name}"),
                    parameters_schema: serde_json::json!({}),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
            }
        }
    }

    #[async_trait]
    impl Skill for TestSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }
        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            Ok(ToolResult::success(&call.id, "ok"))
        }
    }

    /// A skill that always denies access through `validate_arguments`.
    struct DenyingSkill {
        descriptor: SkillDescriptor,
    }

    impl DenyingSkill {
        fn new(name: &str) -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: name.to_string(),
                    description: format!("Denying skill {name}"),
                    parameters_schema: serde_json::json!({}),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
            }
        }
    }

    #[async_trait]
    impl Skill for DenyingSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }
        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            Ok(ToolResult::success(&call.id, "should not reach here"))
        }
        fn validate_arguments(
            &self,
            _call: &ToolCall,
            _permissions: &PermissionSet,
        ) -> ArgentorResult<()> {
            Err(ArgentorError::Security(
                "argument-level access denied by test".to_string(),
            ))
        }
    }

    #[test]
    fn test_filter_by_names_subset() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("echo")));
        reg.register(Arc::new(TestSkill::new("time")));
        reg.register(Arc::new(TestSkill::new("memory_store")));

        let names = vec!["echo".to_string(), "time".to_string()];
        let filtered = reg.filter_by_names(&names);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_by_names_empty() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("echo")));

        let filtered = reg.filter_by_names(&[]);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_by_names_no_match() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("echo")));

        let names = vec!["nonexistent".to_string()];
        let filtered = reg.filter_by_names(&names);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_registry_basic_operations() {
        let reg = SkillRegistry::new();
        assert_eq!(reg.skill_count(), 0);

        reg.register(Arc::new(TestSkill::new("echo")));
        assert_eq!(reg.skill_count(), 1);
        assert!(reg.get("echo").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_list_descriptors() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("a")));
        reg.register(Arc::new(TestSkill::new("b")));

        let descs = reg.list_descriptors();
        assert_eq!(descs.len(), 2);
    }

    #[test]
    fn test_default_tool_groups() {
        let reg = SkillRegistry::new();
        let groups = reg.list_groups();
        assert!(groups.len() >= 7);

        // Check that standard groups exist
        assert!(reg.get_group("minimal").is_some());
        assert!(reg.get_group("coding").is_some());
        assert!(reg.get_group("web").is_some());
        assert!(reg.get_group("orchestration").is_some());
        assert!(reg.get_group("development").is_some());
        assert!(reg.get_group("devops").is_some());
        assert!(reg.get_group("full").is_some());
    }

    #[test]
    fn test_filter_by_group_minimal() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("echo")));
        reg.register(Arc::new(TestSkill::new("time")));
        reg.register(Arc::new(TestSkill::new("help")));
        reg.register(Arc::new(TestSkill::new("file_read")));
        reg.register(Arc::new(TestSkill::new("shell")));

        let minimal = reg.filter_by_group("minimal").unwrap();
        assert_eq!(minimal.skill_count(), 3);
        assert!(minimal.get("echo").is_some());
        assert!(minimal.get("time").is_some());
        assert!(minimal.get("help").is_some());
        assert!(minimal.get("file_read").is_none());
    }

    #[test]
    fn test_filter_by_group_full() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("echo")));
        reg.register(Arc::new(TestSkill::new("file_read")));
        reg.register(Arc::new(TestSkill::new("shell")));

        let full = reg.filter_by_group("full").unwrap();
        assert_eq!(full.skill_count(), 3);
    }

    #[test]
    fn test_filter_by_group_unknown() {
        let reg = SkillRegistry::new();
        assert!(reg.filter_by_group("nonexistent").is_err());
    }

    #[test]
    fn test_custom_tool_group() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("my_tool")));
        reg.register(Arc::new(TestSkill::new("other_tool")));

        reg.register_group(ToolGroup::new(
            "custom",
            "Custom group",
            vec!["my_tool".into()],
        ));

        let custom = reg.filter_by_group("custom").unwrap();
        assert_eq!(custom.skill_count(), 1);
        assert!(custom.get("my_tool").is_some());
    }

    #[test]
    fn test_skills_in_group() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("echo")));
        reg.register(Arc::new(TestSkill::new("time")));

        // Minimal group includes echo, time, help — but help isn't registered
        let skills = reg.skills_in_group("minimal");
        assert_eq!(skills.len(), 2); // echo and time only
        assert!(skills.contains(&"echo".to_string()));
        assert!(skills.contains(&"time".to_string()));
    }

    #[test]
    fn test_register_groups_batch() {
        let reg = SkillRegistry::new();
        reg.register_groups(vec![
            ToolGroup::new("a", "Group A", vec!["x".into()]),
            ToolGroup::new("b", "Group B", vec!["y".into()]),
        ]);
        assert!(reg.get_group("a").is_some());
        assert!(reg.get_group("b").is_some());
    }

    #[tokio::test]
    async fn test_validate_arguments_denies_returns_error_tool_result() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(DenyingSkill::new("deny_skill")));

        let perms = PermissionSet::new();
        let call = ToolCall {
            id: "call_1".to_string(),
            name: "deny_skill".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = reg.execute(call, &perms).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("argument validation failed"));
        assert!(result
            .content
            .contains("argument-level access denied by test"));
    }

    #[tokio::test]
    async fn test_no_validate_arguments_override_works_normally() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("simple_skill")));

        let perms = PermissionSet::new();
        let call = ToolCall {
            id: "call_2".to_string(),
            name: "simple_skill".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = reg.execute(call, &perms).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "ok");
    }

    #[tokio::test]
    async fn test_capability_type_check_denies_missing_type() {
        let reg = SkillRegistry::new();

        // Create a skill that requires FileRead capability
        let skill = Arc::new(TestSkill {
            descriptor: SkillDescriptor {
                name: "needs_file_read".to_string(),
                description: "Needs file read".to_string(),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![Capability::FileRead {
                    allowed_paths: vec!["/tmp".to_string()],
                }],
                requires_approval: false,
            },
        });
        reg.register(skill);

        // Empty permissions — should deny
        let perms = PermissionSet::new();
        let call = ToolCall {
            id: "call_3".to_string(),
            name: "needs_file_read".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = reg.execute(call, &perms).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_capability_type_check_allows_any_matching_type() {
        let reg = SkillRegistry::new();

        // Skill requires FileRead with /specific/path
        let skill = Arc::new(TestSkill {
            descriptor: SkillDescriptor {
                name: "needs_file_read".to_string(),
                description: "Needs file read".to_string(),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![Capability::FileRead {
                    allowed_paths: vec!["/specific/path".to_string()],
                }],
                requires_approval: false,
            },
        });
        reg.register(skill);

        // Permission set has FileRead but with different paths — type check still passes
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileRead {
            allowed_paths: vec!["/other/path".to_string()],
        });

        let call = ToolCall {
            id: "call_4".to_string(),
            name: "needs_file_read".to_string(),
            arguments: serde_json::json!({}),
        };

        // Type check passes (any FileRead exists), and default validate_arguments is Ok
        let result = reg.execute(call, &perms).await.unwrap();
        assert!(!result.is_error);
    }

    // -------------------------------------------------------------------
    // Parallel execution tests
    // -------------------------------------------------------------------

    /// A skill that sleeps for a configurable duration before returning.
    struct SlowSkill {
        descriptor: SkillDescriptor,
        delay: std::time::Duration,
    }

    impl SlowSkill {
        fn new(name: &str, delay: std::time::Duration) -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: name.to_string(),
                    description: format!("Slow skill {name}"),
                    parameters_schema: serde_json::json!({}),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
                delay,
            }
        }
    }

    #[async_trait]
    impl Skill for SlowSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }
        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            tokio::time::sleep(self.delay).await;
            Ok(ToolResult::success(&call.id, format!("done-{}", call.name)))
        }
    }

    /// A skill that always returns an error from execute().
    struct FailingSkill {
        descriptor: SkillDescriptor,
    }

    impl FailingSkill {
        fn new(name: &str) -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: name.to_string(),
                    description: format!("Failing skill {name}"),
                    parameters_schema: serde_json::json!({}),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
            }
        }
    }

    #[async_trait]
    impl Skill for FailingSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }
        async fn execute(&self, _call: ToolCall) -> ArgentorResult<ToolResult> {
            Err(ArgentorError::Skill("intentional failure".to_string()))
        }
    }

    #[tokio::test]
    async fn test_parallel_execution_of_three_independent_tools() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("alpha")));
        reg.register(Arc::new(TestSkill::new("beta")));
        reg.register(Arc::new(TestSkill::new("gamma")));
        let reg = Arc::new(reg);

        let calls = vec![
            ToolCall {
                id: "c1".to_string(),
                name: "alpha".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c2".to_string(),
                name: "beta".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c3".to_string(),
                name: "gamma".to_string(),
                arguments: serde_json::json!({}),
            },
        ];

        let perms = PermissionSet::new();
        let results = reg.execute_parallel(calls, &perms).await;

        assert_eq!(results.len(), 3);
        for result in &results {
            let r = result.as_ref().unwrap();
            assert!(!r.is_error);
            assert_eq!(r.content, "ok");
        }
    }

    #[tokio::test]
    async fn test_parallel_one_failure_does_not_affect_others() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(TestSkill::new("good_a")));
        reg.register(Arc::new(FailingSkill::new("bad")));
        reg.register(Arc::new(TestSkill::new("good_b")));
        let reg = Arc::new(reg);

        let calls = vec![
            ToolCall {
                id: "c1".to_string(),
                name: "good_a".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c2".to_string(),
                name: "bad".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c3".to_string(),
                name: "good_b".to_string(),
                arguments: serde_json::json!({}),
            },
        ];

        let perms = PermissionSet::new();
        let results = reg.execute_parallel(calls, &perms).await;

        assert_eq!(results.len(), 3);

        // First succeeds
        let r0 = results[0].as_ref().unwrap();
        assert!(!r0.is_error);
        assert_eq!(r0.content, "ok");

        // Second fails
        assert!(results[1].is_err());
        let err_msg = format!("{}", results[1].as_ref().unwrap_err());
        assert!(err_msg.contains("intentional failure"));

        // Third succeeds
        let r2 = results[2].as_ref().unwrap();
        assert!(!r2.is_error);
        assert_eq!(r2.content, "ok");
    }

    #[tokio::test]
    async fn test_execute_with_timeout_on_slow_tool() {
        let reg = SkillRegistry::new();
        reg.register(Arc::new(SlowSkill::new(
            "very_slow",
            std::time::Duration::from_secs(10),
        )));

        let call = ToolCall {
            id: "c1".to_string(),
            name: "very_slow".to_string(),
            arguments: serde_json::json!({}),
        };

        let perms = PermissionSet::new();
        let result = reg
            .execute_with_timeout(call, &perms, std::time::Duration::from_millis(50))
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("timed out"));
    }

    #[tokio::test]
    async fn test_parallel_results_maintain_input_order() {
        let reg = SkillRegistry::new();
        // Give different delays to each skill so they complete out of order,
        // but results should still be returned in input order.
        reg.register(Arc::new(SlowSkill::new(
            "slow",
            std::time::Duration::from_millis(80),
        )));
        reg.register(Arc::new(SlowSkill::new(
            "medium",
            std::time::Duration::from_millis(40),
        )));
        reg.register(Arc::new(SlowSkill::new(
            "fast",
            std::time::Duration::from_millis(10),
        )));
        let reg = Arc::new(reg);

        let calls = vec![
            ToolCall {
                id: "c_slow".to_string(),
                name: "slow".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c_medium".to_string(),
                name: "medium".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c_fast".to_string(),
                name: "fast".to_string(),
                arguments: serde_json::json!({}),
            },
        ];

        let perms = PermissionSet::new();
        let results = reg.execute_parallel(calls, &perms).await;

        assert_eq!(results.len(), 3);

        let r0 = results[0].as_ref().unwrap();
        assert_eq!(r0.call_id, "c_slow");
        assert_eq!(r0.content, "done-slow");

        let r1 = results[1].as_ref().unwrap();
        assert_eq!(r1.call_id, "c_medium");
        assert_eq!(r1.content, "done-medium");

        let r2 = results[2].as_ref().unwrap();
        assert_eq!(r2.call_id, "c_fast");
        assert_eq!(r2.content, "done-fast");
    }

    #[tokio::test]
    async fn test_parallel_empty_calls_returns_empty() {
        let reg = Arc::new(SkillRegistry::new());
        let perms = PermissionSet::new();
        let results = reg.execute_parallel(vec![], &perms).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_parallel_permission_denial_in_batch() {
        let reg = SkillRegistry::new();
        // "open" has no required capabilities
        reg.register(Arc::new(TestSkill::new("open")));
        // "restricted" requires FileRead capability
        reg.register(Arc::new(TestSkill {
            descriptor: SkillDescriptor {
                name: "restricted".to_string(),
                description: "Restricted skill".to_string(),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![Capability::FileRead {
                    allowed_paths: vec!["/tmp".to_string()],
                }],
                requires_approval: false,
            },
        }));
        let reg = Arc::new(reg);

        let calls = vec![
            ToolCall {
                id: "c1".to_string(),
                name: "open".to_string(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "c2".to_string(),
                name: "restricted".to_string(),
                arguments: serde_json::json!({}),
            },
        ];

        // Empty permissions — no FileRead granted
        let perms = PermissionSet::new();
        let results = reg.execute_parallel(calls, &perms).await;

        assert_eq!(results.len(), 2);

        // First call succeeds (no capabilities required)
        let r0 = results[0].as_ref().unwrap();
        assert!(!r0.is_error);
        assert_eq!(r0.content, "ok");

        // Second call returns a permission-denied ToolResult (not an Err)
        let r1 = results[1].as_ref().unwrap();
        assert!(r1.is_error);
        assert!(r1.content.contains("Permission denied"));
    }
}

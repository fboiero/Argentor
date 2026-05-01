use argentor_skills::skill::SkillDescriptor;
use argentor_skills::SkillRegistry;
use std::collections::HashSet;

/// Progressive tool disclosure — filter available tools based on context.
///
/// Follows Anthropic's guidance: instead of exposing all tools upfront (which wastes tokens),
/// only expose tools relevant to the current agent's role and task.
/// This can reduce token usage by up to 98%.
pub struct ToolDiscovery;

impl ToolDiscovery {
    /// Filter tool descriptors to only those allowed for a specific agent.
    pub fn filter_by_allowed(
        registry: &SkillRegistry,
        allowed_skills: &[String],
    ) -> Vec<SkillDescriptor> {
        let allowed_set: HashSet<&str> = allowed_skills
            .iter()
            .map(std::string::String::as_str)
            .collect();

        registry
            .list_descriptors()
            .into_iter()
            .filter(|d| allowed_set.contains(d.name.as_str()))
            .collect()
    }

    /// Filter tool descriptors by keyword relevance (simple substring matching).
    pub fn filter_by_context(
        registry: &SkillRegistry,
        context_keywords: &[&str],
    ) -> Vec<SkillDescriptor> {
        if context_keywords.is_empty() {
            return registry.list_descriptors();
        }

        registry
            .list_descriptors()
            .into_iter()
            .filter(|d| {
                let name_lower = d.name.to_lowercase();
                let desc_lower = d.description.to_lowercase();
                context_keywords.iter().any(|kw| {
                    let kw_lower = kw.to_lowercase();
                    name_lower.contains(&kw_lower) || desc_lower.contains(&kw_lower)
                })
            })
            .collect()
    }

    /// Calculate token savings from filtering.
    pub fn estimate_token_savings(total_tools: usize, disclosed_tools: usize) -> f64 {
        if total_tools == 0 {
            return 0.0;
        }
        let saved = total_tools.saturating_sub(disclosed_tools) as f64;
        (saved / total_tools as f64) * 100.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_core::{ArgentorResult, ToolCall, ToolResult};
    use argentor_skills::skill::{Skill, SkillDescriptor};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct DummySkill {
        descriptor: SkillDescriptor,
    }

    impl DummySkill {
        fn new(name: &str, description: &str) -> Self {
            Self {
                descriptor: SkillDescriptor {
                    name: name.to_string(),
                    description: description.to_string(),
                    parameters_schema: serde_json::json!({}),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
            }
        }
    }

    #[async_trait]
    impl Skill for DummySkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }
        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            Ok(ToolResult::success(&call.id, "ok"))
        }
    }

    fn make_registry() -> SkillRegistry {
        let registry = SkillRegistry::new();
        registry.register(Arc::new(DummySkill::new(
            "memory_store",
            "Store text in memory",
        )));
        registry.register(Arc::new(DummySkill::new("memory_search", "Search memory")));
        registry.register(Arc::new(DummySkill::new("echo", "Echo input back")));
        registry.register(Arc::new(DummySkill::new(
            "agent_delegate",
            "Delegate to agent",
        )));
        registry.register(Arc::new(DummySkill::new(
            "human_approval",
            "Request human approval",
        )));
        registry
    }

    #[test]
    fn test_filter_by_allowed() {
        let registry = make_registry();
        let allowed = vec!["memory_store".to_string(), "echo".to_string()];
        let filtered = ToolDiscovery::filter_by_allowed(&registry, &allowed);
        assert_eq!(filtered.len(), 2);
        let names: Vec<&str> = filtered.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"memory_store"));
        assert!(names.contains(&"echo"));
    }

    #[test]
    fn test_filter_by_allowed_empty() {
        let registry = make_registry();
        let filtered = ToolDiscovery::filter_by_allowed(&registry, &[]);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_by_context() {
        let registry = make_registry();
        let filtered = ToolDiscovery::filter_by_context(&registry, &["memory"]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_by_context_empty_keywords() {
        let registry = make_registry();
        let filtered = ToolDiscovery::filter_by_context(&registry, &[]);
        assert_eq!(filtered.len(), 5); // all tools
    }

    #[test]
    fn test_filter_by_context_no_match() {
        let registry = make_registry();
        let filtered = ToolDiscovery::filter_by_context(&registry, &["nonexistent"]);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_estimate_token_savings() {
        assert_eq!(ToolDiscovery::estimate_token_savings(100, 2), 98.0);
        assert_eq!(ToolDiscovery::estimate_token_savings(10, 10), 0.0);
        assert_eq!(ToolDiscovery::estimate_token_savings(0, 0), 0.0);
        assert_eq!(ToolDiscovery::estimate_token_savings(50, 25), 50.0);
    }
}

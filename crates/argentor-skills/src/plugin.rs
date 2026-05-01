use crate::registry::SkillRegistry;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Metadata describing a plugin's identity and purpose.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin name.
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Short description of the plugin.
    pub description: String,
    /// Author name or organization.
    pub author: String,
}

/// Events emitted during the agent lifecycle that plugins can react to.
#[derive(Debug, Clone)]
pub enum PluginEvent {
    /// A new session was created.
    SessionCreated {
        /// Unique identifier of the created session.
        session_id: Uuid,
    },
    /// A session was ended.
    SessionEnded {
        /// Unique identifier of the ended session.
        session_id: Uuid,
    },
    /// A tool invocation is about to start.
    ToolCallBefore {
        /// Name of the tool being invoked.
        tool_name: String,
        /// Unique call identifier.
        call_id: String,
    },
    /// A tool invocation finished.
    ToolCallAfter {
        /// Name of the tool that was invoked.
        tool_name: String,
        /// Unique call identifier.
        call_id: String,
        /// Whether the tool call succeeded.
        success: bool,
    },
    /// A message was received in a session.
    MessageReceived {
        /// Session the message belongs to.
        session_id: Uuid,
        /// Role of the message author (e.g., "user", "assistant").
        role: String,
    },
    /// A custom event emitted by user code.
    Custom {
        /// Event name.
        name: String,
        /// Arbitrary JSON payload.
        data: serde_json::Value,
    },
}

/// Trait that all plugins must implement.
///
/// Plugins can register skills, react to lifecycle events, and perform
/// cleanup on unload. All methods have default no-op implementations
/// so plugins only need to override the hooks they care about.
pub trait Plugin: Send + Sync {
    /// Returns the plugin's manifest (name, version, description, author).
    fn manifest(&self) -> &PluginManifest;

    /// Called when the plugin is loaded. Use this to register skills.
    fn on_load(&self, _registry: &SkillRegistry) {}

    /// Called when the plugin is unloaded. Use this for cleanup.
    fn on_unload(&self) {}

    /// Called when a lifecycle event is emitted.
    fn on_event(&self, _event: &PluginEvent) {}
}

/// Registry that manages loaded plugins and dispatches events to them.
pub struct PluginRegistry {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginRegistry {
    /// Create a new empty plugin registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Load a plugin: calls `on_load` to let it register skills, then stores it.
    pub fn load(&mut self, plugin: Arc<dyn Plugin>, skill_registry: &SkillRegistry) {
        plugin.on_load(skill_registry);
        self.plugins.push(plugin);
    }

    /// Unload all plugins, calling `on_unload` for each, then clearing the list.
    pub fn unload_all(&mut self) {
        for plugin in &self.plugins {
            plugin.on_unload();
        }
        self.plugins.clear();
    }

    /// Emit an event to all loaded plugins.
    pub fn emit(&self, event: &PluginEvent) {
        for plugin in &self.plugins {
            plugin.on_event(event);
        }
    }

    /// Return manifests of all loaded plugins.
    pub fn list(&self) -> Vec<&PluginManifest> {
        self.plugins.iter().map(|p| p.manifest()).collect()
    }

    /// Return the number of loaded plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::skill::{Skill, SkillDescriptor};
    use argentor_core::{ArgentorResult, ToolCall, ToolResult};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A mock plugin used across all tests.
    struct MockPlugin {
        manifest: PluginManifest,
        event_count: Arc<AtomicUsize>,
        load_called: Arc<AtomicBool>,
        unload_called: Arc<AtomicBool>,
    }

    impl MockPlugin {
        fn new(name: &str) -> Self {
            Self {
                manifest: PluginManifest {
                    name: name.to_string(),
                    version: "0.1.0".to_string(),
                    description: format!("Mock plugin {name}"),
                    author: "test".to_string(),
                },
                event_count: Arc::new(AtomicUsize::new(0)),
                load_called: Arc::new(AtomicBool::new(false)),
                unload_called: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl Plugin for MockPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn on_load(&self, _registry: &SkillRegistry) {
            self.load_called.store(true, Ordering::SeqCst);
        }

        fn on_unload(&self) {
            self.unload_called.store(true, Ordering::SeqCst);
        }

        fn on_event(&self, _event: &PluginEvent) {
            self.event_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A mock plugin that registers a skill during on_load.
    struct SkillRegisteringPlugin {
        manifest: PluginManifest,
    }

    /// A trivial skill used by SkillRegisteringPlugin.
    struct PluginSkill {
        descriptor: SkillDescriptor,
    }

    #[async_trait]
    impl Skill for PluginSkill {
        fn descriptor(&self) -> &SkillDescriptor {
            &self.descriptor
        }
        async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
            Ok(ToolResult::success(&call.id, "plugin-ok"))
        }
    }

    impl Plugin for SkillRegisteringPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn on_load(&self, registry: &SkillRegistry) {
            registry.register(Arc::new(PluginSkill {
                descriptor: SkillDescriptor {
                    name: "plugin_skill".to_string(),
                    description: "A skill registered by a plugin".to_string(),
                    parameters_schema: serde_json::json!({}),
                    required_capabilities: vec![],
                    requires_approval: false,
                },
            }));
        }
    }

    #[test]
    fn test_load_plugin_manifest_accessible_via_list() {
        let mut plugin_registry = PluginRegistry::new();
        let skill_registry = SkillRegistry::new();

        let plugin = Arc::new(MockPlugin::new("test-plugin"));
        plugin_registry.load(plugin, &skill_registry);

        let manifests = plugin_registry.list();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "test-plugin");
        assert_eq!(manifests[0].version, "0.1.0");
        assert_eq!(manifests[0].author, "test");
    }

    #[test]
    fn test_unload_all_clears_plugins() {
        let mut plugin_registry = PluginRegistry::new();
        let skill_registry = SkillRegistry::new();

        let plugin = Arc::new(MockPlugin::new("to-unload"));
        let flag_clone = plugin.unload_called.clone();

        plugin_registry.load(plugin, &skill_registry);
        assert_eq!(plugin_registry.count(), 1);

        plugin_registry.unload_all();
        assert_eq!(plugin_registry.count(), 0);
        assert!(
            flag_clone.load(Ordering::SeqCst),
            "on_unload should have been called"
        );
        // list should be empty
        assert!(plugin_registry.list().is_empty());
    }

    #[test]
    fn test_event_emission_calls_on_event() {
        let mut plugin_registry = PluginRegistry::new();
        let skill_registry = SkillRegistry::new();

        let plugin = Arc::new(MockPlugin::new("event-listener"));
        let counter = plugin.event_count.clone();

        plugin_registry.load(plugin, &skill_registry);

        // Emit several events
        plugin_registry.emit(&PluginEvent::SessionCreated {
            session_id: Uuid::new_v4(),
        });
        plugin_registry.emit(&PluginEvent::ToolCallBefore {
            tool_name: "echo".to_string(),
            call_id: "c1".to_string(),
        });
        plugin_registry.emit(&PluginEvent::ToolCallAfter {
            tool_name: "echo".to_string(),
            call_id: "c1".to_string(),
            success: true,
        });

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_skill_registration_via_plugin_on_load() {
        let mut plugin_registry = PluginRegistry::new();
        let skill_registry = SkillRegistry::new();

        assert_eq!(skill_registry.skill_count(), 0);

        let plugin = Arc::new(SkillRegisteringPlugin {
            manifest: PluginManifest {
                name: "skill-provider".to_string(),
                version: "1.0.0".to_string(),
                description: "Provides a skill".to_string(),
                author: "test".to_string(),
            },
        });

        plugin_registry.load(plugin, &skill_registry);

        // The skill should now be in the skill registry
        assert_eq!(skill_registry.skill_count(), 1);
        assert!(skill_registry.get("plugin_skill").is_some());
    }

    #[test]
    fn test_empty_registry_operations() {
        let plugin_registry = PluginRegistry::new();

        assert_eq!(plugin_registry.count(), 0);
        assert!(plugin_registry.list().is_empty());

        // Emitting on an empty registry should not panic
        plugin_registry.emit(&PluginEvent::Custom {
            name: "noop".to_string(),
            data: serde_json::json!(null),
        });
    }

    #[test]
    fn test_plugin_manifest_fields() {
        let manifest = PluginManifest {
            name: "my-plugin".to_string(),
            version: "2.0.0".to_string(),
            description: "A real plugin".to_string(),
            author: "Alice".to_string(),
        };

        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.description, "A real plugin");
        assert_eq!(manifest.author, "Alice");

        // Verify Serialize/Deserialize round-trip
        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.version, manifest.version);
    }
}

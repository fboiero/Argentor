//! State checkpointing for agent time-travel debugging.
//!
//! Inspired by LangGraph's time-travel debugging. Saves and restores complete
//! agent state at arbitrary points, enabling replay, diff, and rollback.
//!
//! # Main types
//!
//! - [`CheckpointManager`] — Creates, stores, and restores checkpoints.
//! - [`Checkpoint`] — A frozen snapshot of agent state at a point in time.
//! - [`AgentState`] — The complete agent state captured in a checkpoint.
//! - [`CheckpointDiff`] — Difference between two checkpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration controlling checkpoint behavior and limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    /// Maximum number of checkpoints to retain (default: 50).
    /// When exceeded, the oldest checkpoint is evicted.
    pub max_checkpoints: usize,
    /// Whether to automatically checkpoint after each turn (default: false).
    pub auto_checkpoint: bool,
    /// Automatically checkpoint every N turns (default: 5; 0 = disabled).
    pub auto_checkpoint_interval: u32,
    /// Whether to include tool results in checkpoint state (default: true).
    pub include_tool_results: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            max_checkpoints: 50,
            auto_checkpoint: false,
            auto_checkpoint_interval: 5,
            include_tool_results: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent state snapshot
// ---------------------------------------------------------------------------

/// Complete agent state captured at a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Conversation messages up to this point.
    pub messages: Vec<CheckpointMessage>,
    /// The system prompt in effect.
    pub system_prompt: String,
    /// Snapshot of model configuration.
    pub model_config_snapshot: ModelSnapshot,
    /// Number of tool calls made so far.
    pub tool_call_count: u32,
    /// Total tokens consumed so far.
    pub total_tokens: usize,
    /// Names of currently active/available tools.
    pub active_tools: Vec<String>,
    /// Arbitrary custom state variables.
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
}

/// A message recorded in a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMessage {
    /// Role of the sender (e.g. "user", "assistant", "system").
    pub role: String,
    /// Message content.
    pub content: String,
    /// Tool calls made within this message (if any).
    pub tool_calls: Vec<ToolCallSnapshot>,
    /// When the message was produced.
    pub timestamp: DateTime<Utc>,
}

/// Snapshot of a single tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSnapshot {
    /// Name of the tool.
    pub tool_name: String,
    /// Arguments passed to the tool (JSON).
    pub arguments: serde_json::Value,
    /// Result of the tool call (if captured).
    pub result: Option<String>,
    /// Whether the tool call succeeded.
    pub success: bool,
}

/// Snapshot of model configuration at checkpoint time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSnapshot {
    /// LLM provider name (e.g. "anthropic", "openai").
    pub provider: String,
    /// Model identifier (e.g. "claude-3-opus").
    pub model_id: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Maximum output tokens.
    pub max_tokens: u32,
}

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

/// A frozen snapshot of agent state at a specific point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier for this checkpoint.
    pub id: String,
    /// Optional human-readable label.
    pub label: Option<String>,
    /// When the checkpoint was created.
    pub timestamp: DateTime<Utc>,
    /// Agentic loop turn number at checkpoint time.
    pub turn_number: u32,
    /// The captured agent state.
    pub state: AgentState,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// Difference between two checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointDiff {
    /// Source checkpoint ID.
    pub from_id: String,
    /// Target checkpoint ID.
    pub to_id: String,
    /// Number of messages added between the two checkpoints.
    pub messages_added: usize,
    /// Number of messages removed between the two checkpoints.
    pub messages_removed: usize,
    /// Difference in tool call count (positive = more calls).
    pub tool_calls_diff: i32,
    /// Difference in token count (positive = more tokens).
    pub token_diff: i64,
    /// List of configuration changes detected.
    pub config_changes: Vec<String>,
}

// ---------------------------------------------------------------------------
// CheckpointManager
// ---------------------------------------------------------------------------

/// Manages the creation, storage, retrieval, and comparison of checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManager {
    checkpoints: HashMap<String, Checkpoint>,
    /// Ordered list of checkpoint IDs (oldest first) for eviction.
    insertion_order: Vec<String>,
    config: CheckpointConfig,
}

impl CheckpointManager {
    /// Create a new manager with the given configuration.
    pub fn new(config: CheckpointConfig) -> Self {
        Self {
            checkpoints: HashMap::new(),
            insertion_order: Vec::new(),
            config,
        }
    }

    /// Create a manager with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CheckpointConfig::default())
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &CheckpointConfig {
        &self.config
    }

    /// Return the number of stored checkpoints.
    pub fn len(&self) -> usize {
        self.checkpoints.len()
    }

    /// Whether the manager has no checkpoints.
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    // ----- Core operations --------------------------------------------------

    /// Create and store a new checkpoint.
    ///
    /// If the maximum number of checkpoints is exceeded, the oldest one is evicted.
    /// Returns the checkpoint ID.
    pub fn create(&mut self, id: String, state: AgentState, turn_number: u32) -> String {
        self.create_with_label(id, None, state, turn_number)
    }

    /// Create a checkpoint with an optional human-readable label.
    pub fn create_with_label(
        &mut self,
        id: String,
        label: Option<String>,
        state: AgentState,
        turn_number: u32,
    ) -> String {
        // Evict oldest if at capacity
        while self.checkpoints.len() >= self.config.max_checkpoints {
            if let Some(oldest_id) = self.insertion_order.first().cloned() {
                self.checkpoints.remove(&oldest_id);
                self.insertion_order.remove(0);
            } else {
                break;
            }
        }

        let checkpoint = Checkpoint {
            id: id.clone(),
            label,
            timestamp: Utc::now(),
            turn_number,
            state,
            metadata: HashMap::new(),
        };

        self.checkpoints.insert(id.clone(), checkpoint);
        self.insertion_order.push(id.clone());

        id
    }

    /// Restore a checkpoint by ID, returning a clone of the captured state.
    pub fn restore(&self, id: &str) -> Option<AgentState> {
        self.checkpoints.get(id).map(|cp| cp.state.clone())
    }

    /// Get a reference to a checkpoint by ID.
    pub fn get(&self, id: &str) -> Option<&Checkpoint> {
        self.checkpoints.get(id)
    }

    /// List all checkpoints in insertion order (oldest first).
    pub fn list(&self) -> Vec<&Checkpoint> {
        self.insertion_order
            .iter()
            .filter_map(|id| self.checkpoints.get(id))
            .collect()
    }

    /// Compute the diff between two checkpoints.
    pub fn diff(&self, from_id: &str, to_id: &str) -> Option<CheckpointDiff> {
        let from = self.checkpoints.get(from_id)?;
        let to = self.checkpoints.get(to_id)?;

        let from_msg_count = from.state.messages.len();
        let to_msg_count = to.state.messages.len();

        let messages_added = to_msg_count.saturating_sub(from_msg_count);
        let messages_removed = from_msg_count.saturating_sub(to_msg_count);

        let tool_calls_diff = to.state.tool_call_count as i32 - from.state.tool_call_count as i32;

        let token_diff = to.state.total_tokens as i64 - from.state.total_tokens as i64;

        let mut config_changes = Vec::new();

        if from.state.model_config_snapshot.provider != to.state.model_config_snapshot.provider {
            config_changes.push(format!(
                "provider: {} -> {}",
                from.state.model_config_snapshot.provider, to.state.model_config_snapshot.provider
            ));
        }
        if from.state.model_config_snapshot.model_id != to.state.model_config_snapshot.model_id {
            config_changes.push(format!(
                "model_id: {} -> {}",
                from.state.model_config_snapshot.model_id, to.state.model_config_snapshot.model_id
            ));
        }
        if (from.state.model_config_snapshot.temperature
            - to.state.model_config_snapshot.temperature)
            .abs()
            > f32::EPSILON
        {
            config_changes.push(format!(
                "temperature: {} -> {}",
                from.state.model_config_snapshot.temperature,
                to.state.model_config_snapshot.temperature
            ));
        }
        if from.state.model_config_snapshot.max_tokens != to.state.model_config_snapshot.max_tokens
        {
            config_changes.push(format!(
                "max_tokens: {} -> {}",
                from.state.model_config_snapshot.max_tokens,
                to.state.model_config_snapshot.max_tokens
            ));
        }

        if from.state.system_prompt != to.state.system_prompt {
            config_changes.push("system_prompt changed".to_string());
        }

        if from.state.active_tools != to.state.active_tools {
            config_changes.push("active_tools changed".to_string());
        }

        Some(CheckpointDiff {
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            messages_added,
            messages_removed,
            tool_calls_diff,
            token_diff,
            config_changes,
        })
    }

    /// Delete a checkpoint by ID. Returns true if it existed.
    pub fn delete(&mut self, id: &str) -> bool {
        if self.checkpoints.remove(id).is_some() {
            self.insertion_order.retain(|i| i != id);
            true
        } else {
            false
        }
    }

    /// Serialize the entire manager to JSON.
    pub fn serialize(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize a manager from JSON.
    pub fn deserialize(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize a single checkpoint to JSON.
    pub fn serialize_checkpoint(checkpoint: &Checkpoint) -> Result<String, serde_json::Error> {
        serde_json::to_string(checkpoint)
    }

    /// Deserialize a single checkpoint from JSON.
    pub fn deserialize_checkpoint(json: &str) -> Result<Checkpoint, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if auto-checkpoint should trigger for the given turn number.
    pub fn should_auto_checkpoint(&self, turn_number: u32) -> bool {
        if self.config.auto_checkpoint {
            return true;
        }
        if self.config.auto_checkpoint_interval > 0
            && turn_number > 0
            && turn_number % self.config.auto_checkpoint_interval == 0
        {
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -- helpers --

    fn make_model_snapshot() -> ModelSnapshot {
        ModelSnapshot {
            provider: "anthropic".to_string(),
            model_id: "claude-3-opus".to_string(),
            temperature: 0.7,
            max_tokens: 4096,
        }
    }

    fn make_state(messages: usize, tool_calls: u32, tokens: usize) -> AgentState {
        let msgs: Vec<CheckpointMessage> = (0..messages)
            .map(|i| CheckpointMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("message {i}"),
                tool_calls: vec![],
                timestamp: Utc::now(),
            })
            .collect();

        AgentState {
            messages: msgs,
            system_prompt: "You are a helpful assistant.".to_string(),
            model_config_snapshot: make_model_snapshot(),
            tool_call_count: tool_calls,
            total_tokens: tokens,
            active_tools: vec!["file_read".to_string(), "file_write".to_string()],
            variables: HashMap::new(),
        }
    }

    // 1. Default config values
    #[test]
    fn test_default_config() {
        let cfg = CheckpointConfig::default();
        assert_eq!(cfg.max_checkpoints, 50);
        assert!(!cfg.auto_checkpoint);
        assert_eq!(cfg.auto_checkpoint_interval, 5);
        assert!(cfg.include_tool_results);
    }

    // 2. Create manager with defaults
    #[test]
    fn test_with_defaults() {
        let mgr = CheckpointManager::with_defaults();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }

    // 3. Create a checkpoint
    #[test]
    fn test_create_checkpoint() {
        let mut mgr = CheckpointManager::with_defaults();
        let id = mgr.create("cp-1".to_string(), make_state(2, 1, 500), 1);
        assert_eq!(id, "cp-1");
        assert_eq!(mgr.len(), 1);
    }

    // 4. Create with label
    #[test]
    fn test_create_with_label() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create_with_label(
            "cp-1".to_string(),
            Some("before refactor".to_string()),
            make_state(3, 2, 1000),
            5,
        );
        let cp = mgr.get("cp-1").unwrap();
        assert_eq!(cp.label.as_deref(), Some("before refactor"));
        assert_eq!(cp.turn_number, 5);
    }

    // 5. Restore checkpoint returns state
    #[test]
    fn test_restore_checkpoint() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(4, 3, 2000), 10);
        let state = mgr.restore("cp-1").unwrap();
        assert_eq!(state.messages.len(), 4);
        assert_eq!(state.tool_call_count, 3);
        assert_eq!(state.total_tokens, 2000);
    }

    // 6. Restore non-existent returns None
    #[test]
    fn test_restore_nonexistent() {
        let mgr = CheckpointManager::with_defaults();
        assert!(mgr.restore("no-such-cp").is_none());
    }

    // 7. List returns checkpoints in insertion order
    #[test]
    fn test_list_order() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(1, 0, 100), 1);
        mgr.create("cp-2".to_string(), make_state(2, 1, 200), 2);
        mgr.create("cp-3".to_string(), make_state(3, 2, 300), 3);
        let list = mgr.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].id, "cp-1");
        assert_eq!(list[1].id, "cp-2");
        assert_eq!(list[2].id, "cp-3");
    }

    // 8. Delete a checkpoint
    #[test]
    fn test_delete_checkpoint() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(1, 0, 100), 1);
        assert!(mgr.delete("cp-1"));
        assert!(mgr.is_empty());
    }

    // 9. Delete non-existent returns false
    #[test]
    fn test_delete_nonexistent() {
        let mut mgr = CheckpointManager::with_defaults();
        assert!(!mgr.delete("no-such"));
    }

    // 10. Eviction when max exceeded
    #[test]
    fn test_eviction() {
        let cfg = CheckpointConfig {
            max_checkpoints: 3,
            ..Default::default()
        };
        let mut mgr = CheckpointManager::new(cfg);
        mgr.create("cp-1".to_string(), make_state(1, 0, 100), 1);
        mgr.create("cp-2".to_string(), make_state(2, 0, 200), 2);
        mgr.create("cp-3".to_string(), make_state(3, 0, 300), 3);
        // Adding a 4th should evict cp-1
        mgr.create("cp-4".to_string(), make_state(4, 0, 400), 4);
        assert_eq!(mgr.len(), 3);
        assert!(mgr.get("cp-1").is_none());
        assert!(mgr.get("cp-4").is_some());
    }

    // 11. Diff — messages added
    #[test]
    fn test_diff_messages_added() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(2, 1, 500), 1);
        mgr.create("cp-2".to_string(), make_state(5, 3, 1500), 5);
        let diff = mgr.diff("cp-1", "cp-2").unwrap();
        assert_eq!(diff.messages_added, 3);
        assert_eq!(diff.messages_removed, 0);
        assert_eq!(diff.tool_calls_diff, 2);
        assert_eq!(diff.token_diff, 1000);
    }

    // 12. Diff — messages removed
    #[test]
    fn test_diff_messages_removed() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(5, 3, 1500), 5);
        mgr.create("cp-2".to_string(), make_state(2, 1, 500), 2);
        let diff = mgr.diff("cp-1", "cp-2").unwrap();
        assert_eq!(diff.messages_added, 0);
        assert_eq!(diff.messages_removed, 3);
        assert_eq!(diff.tool_calls_diff, -2);
        assert_eq!(diff.token_diff, -1000);
    }

    // 13. Diff — config changes detected
    #[test]
    fn test_diff_config_changes() {
        let mut mgr = CheckpointManager::with_defaults();
        let mut state1 = make_state(1, 0, 100);
        state1.model_config_snapshot.temperature = 0.5;

        let mut state2 = make_state(1, 0, 100);
        state2.model_config_snapshot.temperature = 0.9;
        state2.model_config_snapshot.model_id = "claude-3-sonnet".to_string();

        mgr.create("cp-1".to_string(), state1, 1);
        mgr.create("cp-2".to_string(), state2, 2);

        let diff = mgr.diff("cp-1", "cp-2").unwrap();
        assert!(diff
            .config_changes
            .iter()
            .any(|c| c.contains("temperature")));
        assert!(diff.config_changes.iter().any(|c| c.contains("model_id")));
    }

    // 14. Diff — nonexistent checkpoint returns None
    #[test]
    fn test_diff_nonexistent() {
        let mgr = CheckpointManager::with_defaults();
        assert!(mgr.diff("a", "b").is_none());
    }

    // 15. Diff — system prompt change
    #[test]
    fn test_diff_system_prompt_change() {
        let mut mgr = CheckpointManager::with_defaults();
        let mut state1 = make_state(1, 0, 100);
        state1.system_prompt = "prompt v1".to_string();

        let mut state2 = make_state(1, 0, 100);
        state2.system_prompt = "prompt v2".to_string();

        mgr.create("cp-1".to_string(), state1, 1);
        mgr.create("cp-2".to_string(), state2, 2);

        let diff = mgr.diff("cp-1", "cp-2").unwrap();
        assert!(diff
            .config_changes
            .iter()
            .any(|c| c.contains("system_prompt")));
    }

    // 16. Diff — active tools change
    #[test]
    fn test_diff_active_tools_change() {
        let mut mgr = CheckpointManager::with_defaults();
        let mut state1 = make_state(1, 0, 100);
        state1.active_tools = vec!["tool_a".to_string()];

        let mut state2 = make_state(1, 0, 100);
        state2.active_tools = vec!["tool_a".to_string(), "tool_b".to_string()];

        mgr.create("cp-1".to_string(), state1, 1);
        mgr.create("cp-2".to_string(), state2, 2);

        let diff = mgr.diff("cp-1", "cp-2").unwrap();
        assert!(diff
            .config_changes
            .iter()
            .any(|c| c.contains("active_tools")));
    }

    // 17. Serialize / deserialize manager roundtrip
    #[test]
    fn test_manager_serialization() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(2, 1, 500), 1);
        mgr.create("cp-2".to_string(), make_state(4, 3, 1200), 5);

        let json = mgr.serialize().unwrap();
        let restored = CheckpointManager::deserialize(&json).unwrap();
        assert_eq!(restored.len(), 2);
        assert!(restored.get("cp-1").is_some());
        assert!(restored.get("cp-2").is_some());
    }

    // 18. Serialize / deserialize single checkpoint
    #[test]
    fn test_checkpoint_serialization() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(3, 2, 800), 10);
        let cp = mgr.get("cp-1").unwrap();

        let json = CheckpointManager::serialize_checkpoint(cp).unwrap();
        let restored = CheckpointManager::deserialize_checkpoint(&json).unwrap();
        assert_eq!(restored.id, "cp-1");
        assert_eq!(restored.state.messages.len(), 3);
        assert_eq!(restored.turn_number, 10);
    }

    // 19. Auto-checkpoint when auto_checkpoint = true
    #[test]
    fn test_should_auto_checkpoint_always() {
        let cfg = CheckpointConfig {
            auto_checkpoint: true,
            ..Default::default()
        };
        let mgr = CheckpointManager::new(cfg);
        assert!(mgr.should_auto_checkpoint(1));
        assert!(mgr.should_auto_checkpoint(2));
        assert!(mgr.should_auto_checkpoint(99));
    }

    // 20. Auto-checkpoint at interval
    #[test]
    fn test_should_auto_checkpoint_interval() {
        let cfg = CheckpointConfig {
            auto_checkpoint: false,
            auto_checkpoint_interval: 5,
            ..Default::default()
        };
        let mgr = CheckpointManager::new(cfg);
        assert!(!mgr.should_auto_checkpoint(1));
        assert!(!mgr.should_auto_checkpoint(3));
        assert!(mgr.should_auto_checkpoint(5));
        assert!(mgr.should_auto_checkpoint(10));
        assert!(!mgr.should_auto_checkpoint(0)); // turn 0 does not trigger
    }

    // 21. Auto-checkpoint disabled
    #[test]
    fn test_should_auto_checkpoint_disabled() {
        let cfg = CheckpointConfig {
            auto_checkpoint: false,
            auto_checkpoint_interval: 0,
            ..Default::default()
        };
        let mgr = CheckpointManager::new(cfg);
        assert!(!mgr.should_auto_checkpoint(1));
        assert!(!mgr.should_auto_checkpoint(100));
    }

    // 22. Checkpoint with tool calls in messages
    #[test]
    fn test_checkpoint_with_tool_calls() {
        let mut mgr = CheckpointManager::with_defaults();

        let msg = CheckpointMessage {
            role: "assistant".to_string(),
            content: "Let me check that file.".to_string(),
            tool_calls: vec![ToolCallSnapshot {
                tool_name: "file_read".to_string(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
                result: Some("file contents here".to_string()),
                success: true,
            }],
            timestamp: Utc::now(),
        };

        let state = AgentState {
            messages: vec![msg],
            system_prompt: "test".to_string(),
            model_config_snapshot: make_model_snapshot(),
            tool_call_count: 1,
            total_tokens: 300,
            active_tools: vec![],
            variables: HashMap::new(),
        };

        mgr.create("cp-tc".to_string(), state, 1);
        let restored = mgr.restore("cp-tc").unwrap();
        assert_eq!(restored.messages[0].tool_calls.len(), 1);
        assert!(restored.messages[0].tool_calls[0].success);
        assert_eq!(
            restored.messages[0].tool_calls[0].arguments["path"],
            "/tmp/test.txt"
        );
    }

    // 23. Checkpoint with custom variables
    #[test]
    fn test_checkpoint_with_variables() {
        let mut mgr = CheckpointManager::with_defaults();
        let mut state = make_state(1, 0, 100);
        state
            .variables
            .insert("current_file".to_string(), serde_json::json!("src/main.rs"));
        state
            .variables
            .insert("iteration".to_string(), serde_json::json!(3));

        mgr.create("cp-vars".to_string(), state, 1);
        let restored = mgr.restore("cp-vars").unwrap();
        assert_eq!(restored.variables["current_file"], "src/main.rs");
        assert_eq!(restored.variables["iteration"], 3);
    }

    // 24. Checkpoint metadata
    #[test]
    fn test_checkpoint_metadata() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(1, 0, 100), 1);
        // Metadata is empty by default but accessible
        let cp = mgr.get("cp-1").unwrap();
        assert!(cp.metadata.is_empty());
    }

    // 25. Multiple evictions preserve correct order
    #[test]
    fn test_multiple_evictions() {
        let cfg = CheckpointConfig {
            max_checkpoints: 2,
            ..Default::default()
        };
        let mut mgr = CheckpointManager::new(cfg);
        mgr.create("cp-1".to_string(), make_state(1, 0, 100), 1);
        mgr.create("cp-2".to_string(), make_state(2, 0, 200), 2);
        mgr.create("cp-3".to_string(), make_state(3, 0, 300), 3);
        mgr.create("cp-4".to_string(), make_state(4, 0, 400), 4);

        assert_eq!(mgr.len(), 2);
        assert!(mgr.get("cp-1").is_none());
        assert!(mgr.get("cp-2").is_none());
        assert!(mgr.get("cp-3").is_some());
        assert!(mgr.get("cp-4").is_some());

        let list = mgr.list();
        assert_eq!(list[0].id, "cp-3");
        assert_eq!(list[1].id, "cp-4");
    }

    // 26. AgentState serialization
    #[test]
    fn test_agent_state_serialization() {
        let state = make_state(3, 5, 2000);
        let json = serde_json::to_string(&state).unwrap();
        let restored: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.messages.len(), 3);
        assert_eq!(restored.tool_call_count, 5);
        assert_eq!(restored.total_tokens, 2000);
    }

    // 27. ModelSnapshot serialization
    #[test]
    fn test_model_snapshot_serialization() {
        let snap = make_model_snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let restored: ModelSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.provider, "anthropic");
        assert_eq!(restored.model_id, "claude-3-opus");
    }

    // 28. CheckpointDiff serialization
    #[test]
    fn test_diff_serialization() {
        let diff = CheckpointDiff {
            from_id: "a".to_string(),
            to_id: "b".to_string(),
            messages_added: 3,
            messages_removed: 0,
            tool_calls_diff: 2,
            token_diff: 500,
            config_changes: vec!["temperature changed".to_string()],
        };
        let json = serde_json::to_string(&diff).unwrap();
        let restored: CheckpointDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.messages_added, 3);
        assert_eq!(restored.config_changes.len(), 1);
    }

    // 29. Config serialization
    #[test]
    fn test_config_serialization() {
        let cfg = CheckpointConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: CheckpointConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_checkpoints, 50);
    }

    // 30. Diff with identical checkpoints
    #[test]
    fn test_diff_identical() {
        let mut mgr = CheckpointManager::with_defaults();
        let state = make_state(3, 2, 1000);
        mgr.create("cp-1".to_string(), state.clone(), 5);
        mgr.create("cp-2".to_string(), state, 5);

        let diff = mgr.diff("cp-1", "cp-2").unwrap();
        assert_eq!(diff.messages_added, 0);
        assert_eq!(diff.messages_removed, 0);
        assert_eq!(diff.tool_calls_diff, 0);
        assert_eq!(diff.token_diff, 0);
        assert!(diff.config_changes.is_empty());
    }

    // 31. Delete preserves other checkpoints
    #[test]
    fn test_delete_preserves_others() {
        let mut mgr = CheckpointManager::with_defaults();
        mgr.create("cp-1".to_string(), make_state(1, 0, 100), 1);
        mgr.create("cp-2".to_string(), make_state(2, 0, 200), 2);
        mgr.create("cp-3".to_string(), make_state(3, 0, 300), 3);

        mgr.delete("cp-2");
        assert_eq!(mgr.len(), 2);
        assert!(mgr.get("cp-1").is_some());
        assert!(mgr.get("cp-2").is_none());
        assert!(mgr.get("cp-3").is_some());

        let list = mgr.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "cp-1");
        assert_eq!(list[1].id, "cp-3");
    }

    // 32. ToolCallSnapshot serialization
    #[test]
    fn test_tool_call_snapshot_serialization() {
        let snap = ToolCallSnapshot {
            tool_name: "bash".to_string(),
            arguments: serde_json::json!({"command": "ls -la"}),
            result: Some("output here".to_string()),
            success: true,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let restored: ToolCallSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.tool_name, "bash");
        assert!(restored.success);
    }

    // 33. CheckpointMessage serialization
    #[test]
    fn test_checkpoint_message_serialization() {
        let msg = CheckpointMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let restored: CheckpointMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "user");
        assert_eq!(restored.content, "Hello");
    }
}

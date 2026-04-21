#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the WASM skill system.
//!
//! These tests load a real WASM binary (the echo-skill) and exercise the full
//! pipeline: WasmSkillRuntime -> WasmSkill -> SkillRegistry -> execute.

use argentor_core::ToolCall;
use argentor_security::PermissionSet;
use argentor_skills::{SkillRegistry, WasmSkillRuntime};
use std::path::PathBuf;
use std::sync::Arc;

/// Resolve the path to the echo-skill WASM binary relative to the workspace root.
fn echo_skill_wasm_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/argentor-skills -> navigate up two levels to workspace root
    let workspace_root = manifest_dir
        .parent()
        .expect("failed to get crates/ dir")
        .parent()
        .expect("failed to get workspace root");
    workspace_root.join("skills/echo-skill/target/wasm32-wasip1/release/echo-skill.wasm")
}

/// Verify the echo-skill WASM binary exists before running tests.
fn assert_wasm_binary_exists() -> PathBuf {
    let path = echo_skill_wasm_path();
    assert!(
        path.exists(),
        "Echo-skill WASM binary not found at {}. Build it first with: \
         cd skills/echo-skill && cargo build --target wasm32-wasip1 --release",
        path.display()
    );
    path
}

#[test]
fn test_create_wasm_runtime() {
    let runtime = WasmSkillRuntime::new();
    assert!(runtime.is_ok(), "WasmSkillRuntime::new() should succeed");
}

#[test]
fn test_load_echo_skill() {
    let wasm_path = assert_wasm_binary_exists();
    let runtime = WasmSkillRuntime::new().expect("failed to create WASM runtime");

    let skill = runtime.load_skill(
        &wasm_path,
        "echo".to_string(),
        "Echoes back the input message".to_string(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        }),
        vec![],
    );

    assert!(
        skill.is_ok(),
        "load_skill should succeed for the echo WASM binary"
    );
}

#[test]
fn test_load_skill_with_nonexistent_path() {
    let runtime = WasmSkillRuntime::new().expect("failed to create WASM runtime");
    let bad_path = PathBuf::from("/nonexistent/path/to/skill.wasm");

    let result = runtime.load_skill(
        &bad_path,
        "missing".to_string(),
        "This should fail".to_string(),
        serde_json::json!({}),
        vec![],
    );

    assert!(
        result.is_err(),
        "load_skill should return an error for a nonexistent WASM path"
    );
}

#[tokio::test]
async fn test_execute_echo_skill() {
    let wasm_path = assert_wasm_binary_exists();
    let runtime = WasmSkillRuntime::new().expect("failed to create WASM runtime");

    let skill = runtime
        .load_skill(
            &wasm_path,
            "echo".to_string(),
            "Echoes back the input message".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            }),
            vec![],
        )
        .expect("failed to load echo skill");

    let call = ToolCall {
        id: "call-001".to_string(),
        name: "echo".to_string(),
        arguments: serde_json::json!({ "message": "hello world" }),
    };

    let result = argentor_skills::Skill::execute(&skill, call).await;
    assert!(result.is_ok(), "execute() should not return Err");

    let tool_result = result.unwrap();
    assert!(
        !tool_result.is_error,
        "ToolResult should indicate success, but got error: {}",
        tool_result.content
    );
    assert_eq!(tool_result.call_id, "call-001");
}

#[tokio::test]
async fn test_register_and_list_echo_skill() {
    let wasm_path = assert_wasm_binary_exists();
    let runtime = WasmSkillRuntime::new().expect("failed to create WASM runtime");

    let skill = runtime
        .load_skill(
            &wasm_path,
            "echo".to_string(),
            "Echoes back the input message".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            }),
            vec![],
        )
        .expect("failed to load echo skill");

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill));

    // Verify the skill count
    assert_eq!(
        registry.skill_count(),
        1,
        "Registry should contain exactly one skill"
    );

    // Verify the skill is retrievable by name
    let retrieved = registry.get("echo");
    assert!(retrieved.is_some(), "Registry should find the 'echo' skill");

    // List descriptors and verify the echo skill appears
    let descriptors = registry.list_descriptors();
    assert_eq!(descriptors.len(), 1, "Should list exactly one descriptor");
    assert_eq!(descriptors[0].name, "echo");
    assert_eq!(descriptors[0].description, "Echoes back the input message");
}

#[tokio::test]
async fn test_registry_execute_echo_skill() {
    let wasm_path = assert_wasm_binary_exists();
    let runtime = WasmSkillRuntime::new().expect("failed to create WASM runtime");

    let skill = runtime
        .load_skill(
            &wasm_path,
            "echo".to_string(),
            "Echoes back the input message".to_string(),
            serde_json::json!({}),
            vec![], // No capabilities required
        )
        .expect("failed to load echo skill");

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill));

    let call = ToolCall {
        id: "call-002".to_string(),
        name: "echo".to_string(),
        arguments: serde_json::json!({ "message": "hello world" }),
    };

    // Empty permission set is fine since the skill requires no capabilities
    let permissions = PermissionSet::new();
    let result = registry.execute(call, &permissions).await;

    assert!(result.is_ok(), "Registry execute should not return Err");
    let tool_result = result.unwrap();
    assert!(
        !tool_result.is_error,
        "ToolResult should indicate success, got error: {}",
        tool_result.content
    );
}

#[tokio::test]
async fn test_registry_execute_unknown_skill_returns_error() {
    let registry = SkillRegistry::new();

    let call = ToolCall {
        id: "call-003".to_string(),
        name: "nonexistent_skill".to_string(),
        arguments: serde_json::json!({}),
    };

    let permissions = PermissionSet::new();
    let result = registry.execute(call, &permissions).await;

    assert!(
        result.is_err(),
        "Executing an unknown skill should return Err"
    );
}

#[tokio::test]
async fn test_register_multiple_skills_and_list() {
    let wasm_path = assert_wasm_binary_exists();
    let runtime = WasmSkillRuntime::new().expect("failed to create WASM runtime");

    // Register the same WASM binary under two different names
    let skill_a = runtime
        .load_skill(
            &wasm_path,
            "echo-a".to_string(),
            "First echo variant".to_string(),
            serde_json::json!({}),
            vec![],
        )
        .expect("failed to load skill a");

    let skill_b = runtime
        .load_skill(
            &wasm_path,
            "echo-b".to_string(),
            "Second echo variant".to_string(),
            serde_json::json!({}),
            vec![],
        )
        .expect("failed to load skill b");

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill_a));
    registry.register(Arc::new(skill_b));

    assert_eq!(registry.skill_count(), 2);

    let descriptors = registry.list_descriptors();
    let names: Vec<&str> = descriptors.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"echo-a"), "Should contain echo-a");
    assert!(names.contains(&"echo-b"), "Should contain echo-b");
}

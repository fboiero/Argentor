#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the SkillLoader.
//!
//! These tests exercise the SkillLoader's ability to load WASM skills from
//! configuration and handle error conditions gracefully.

use argentor_skills::loader::{CapabilityConfig, SkillConfig, SkillType};
use argentor_skills::{SkillLoader, SkillRegistry};
use std::path::PathBuf;

/// Resolve the workspace root from CARGO_MANIFEST_DIR.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("failed to get crates/ dir")
        .parent()
        .expect("failed to get workspace root")
        .to_path_buf()
}

/// Relative path (from workspace root) to the echo-skill WASM binary.
fn echo_skill_relative_path() -> PathBuf {
    PathBuf::from("skills/echo-skill/target/wasm32-wasip1/release/echo-skill.wasm")
}

/// Assert that the WASM binary exists and return the workspace root.
fn assert_prerequisites() -> PathBuf {
    let root = workspace_root();
    let wasm_path = root.join(echo_skill_relative_path());
    assert!(
        wasm_path.exists(),
        "Echo-skill WASM binary not found at {}. Build it first with: \
         cd skills/echo-skill && cargo build --target wasm32-wasip1 --release",
        wasm_path.display()
    );
    root
}

#[test]
fn test_create_skill_loader() {
    let loader = SkillLoader::new();
    assert!(loader.is_ok(), "SkillLoader::new() should succeed");
}

#[test]
fn test_load_valid_wasm_skill_via_loader() {
    let base_dir = assert_prerequisites();
    let loader = SkillLoader::new().expect("failed to create SkillLoader");
    let registry = SkillRegistry::new();

    let config = SkillConfig {
        name: "echo".to_string(),
        description: "Echoes back the input message".to_string(),
        skill_type: SkillType::Wasm,
        path: Some(echo_skill_relative_path()),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        }),
        capabilities: CapabilityConfig::default(),
    };

    let loaded = loader.load_all(&[config], &base_dir, &registry);
    assert!(loaded.is_ok(), "load_all should succeed");
    assert_eq!(loaded.unwrap(), 1, "Should have loaded exactly one skill");
    assert_eq!(registry.skill_count(), 1);

    // Verify the loaded skill is accessible
    let skill = registry.get("echo");
    assert!(
        skill.is_some(),
        "The 'echo' skill should be in the registry"
    );
    assert_eq!(skill.unwrap().descriptor().name, "echo");
}

#[test]
fn test_load_skill_with_nonexistent_path_is_skipped() {
    let base_dir = workspace_root();
    let loader = SkillLoader::new().expect("failed to create SkillLoader");
    let registry = SkillRegistry::new();

    let config = SkillConfig {
        name: "missing-skill".to_string(),
        description: "This skill does not exist".to_string(),
        skill_type: SkillType::Wasm,
        path: Some(PathBuf::from("nonexistent/path/to/skill.wasm")),
        parameters_schema: serde_json::json!({}),
        capabilities: CapabilityConfig::default(),
    };

    // load_all should not fail; it skips individual failures and logs a warning
    let loaded = loader.load_all(&[config], &base_dir, &registry);
    assert!(
        loaded.is_ok(),
        "load_all should succeed even when a skill fails to load"
    );
    assert_eq!(
        loaded.unwrap(),
        0,
        "No skills should have been loaded since the path does not exist"
    );
    assert_eq!(registry.skill_count(), 0);
}

#[test]
fn test_load_skill_with_no_path_is_skipped() {
    let base_dir = workspace_root();
    let loader = SkillLoader::new().expect("failed to create SkillLoader");
    let registry = SkillRegistry::new();

    let config = SkillConfig {
        name: "no-path-skill".to_string(),
        description: "WASM skill without a path field".to_string(),
        skill_type: SkillType::Wasm,
        path: None, // No path provided
        parameters_schema: serde_json::json!({}),
        capabilities: CapabilityConfig::default(),
    };

    let loaded = loader.load_all(&[config], &base_dir, &registry);
    assert!(
        loaded.is_ok(),
        "load_all should succeed even when a config is invalid"
    );
    assert_eq!(
        loaded.unwrap(),
        0,
        "No skills should have been loaded since no path was provided"
    );
}

#[test]
fn test_load_native_skill_type_is_skipped() {
    let base_dir = workspace_root();
    let loader = SkillLoader::new().expect("failed to create SkillLoader");
    let registry = SkillRegistry::new();

    let config = SkillConfig {
        name: "native-skill".to_string(),
        description: "A native skill in config should be skipped".to_string(),
        skill_type: SkillType::Native,
        path: None,
        parameters_schema: serde_json::json!({}),
        capabilities: CapabilityConfig::default(),
    };

    let loaded = loader.load_all(&[config], &base_dir, &registry);
    assert!(
        loaded.is_ok(),
        "load_all should succeed for native skill configs"
    );
    // Native skills are logged as a warning but considered "loaded" (the load_one returns Ok)
    // Actually looking at the code: load_one returns Ok(()) for Native but does not register,
    // and load_all counts it as loaded += 1 since Ok(()) is returned.
    // Let's verify this behavior:
    let count = loaded.unwrap();
    // Native returns Ok(()) from load_one, so loaded increments
    assert_eq!(
        count, 1,
        "Native skill config returns Ok, so it is counted as loaded"
    );
    // But nothing is actually registered in the registry
    assert_eq!(
        registry.skill_count(),
        0,
        "Native skill is not actually registered in the registry"
    );
}

#[test]
fn test_load_mix_of_valid_and_invalid_skills() {
    let base_dir = assert_prerequisites();
    let loader = SkillLoader::new().expect("failed to create SkillLoader");
    let registry = SkillRegistry::new();

    let configs = vec![
        SkillConfig {
            name: "echo".to_string(),
            description: "Valid echo skill".to_string(),
            skill_type: SkillType::Wasm,
            path: Some(echo_skill_relative_path()),
            parameters_schema: serde_json::json!({}),
            capabilities: CapabilityConfig::default(),
        },
        SkillConfig {
            name: "broken".to_string(),
            description: "Broken skill with bad path".to_string(),
            skill_type: SkillType::Wasm,
            path: Some(PathBuf::from("does/not/exist.wasm")),
            parameters_schema: serde_json::json!({}),
            capabilities: CapabilityConfig::default(),
        },
        SkillConfig {
            name: "echo-copy".to_string(),
            description: "Another valid echo skill".to_string(),
            skill_type: SkillType::Wasm,
            path: Some(echo_skill_relative_path()),
            parameters_schema: serde_json::json!({}),
            capabilities: CapabilityConfig::default(),
        },
    ];

    let loaded = loader.load_all(&configs, &base_dir, &registry);
    assert!(loaded.is_ok(), "load_all should succeed");
    assert_eq!(
        loaded.unwrap(),
        2,
        "Should have loaded 2 out of 3 skills (the broken one is skipped)"
    );
    assert_eq!(registry.skill_count(), 2);

    // Verify the correct skills were loaded
    assert!(registry.get("echo").is_some(), "echo should be registered");
    assert!(
        registry.get("echo-copy").is_some(),
        "echo-copy should be registered"
    );
    assert!(
        registry.get("broken").is_none(),
        "broken should NOT be registered"
    );
}

#[tokio::test]
async fn test_loaded_skill_can_execute() {
    let base_dir = assert_prerequisites();
    let loader = SkillLoader::new().expect("failed to create SkillLoader");
    let registry = SkillRegistry::new();

    let config = SkillConfig {
        name: "echo".to_string(),
        description: "Echoes back the input message".to_string(),
        skill_type: SkillType::Wasm,
        path: Some(echo_skill_relative_path()),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        }),
        capabilities: CapabilityConfig::default(),
    };

    loader
        .load_all(&[config], &base_dir, &registry)
        .expect("failed to load skills");

    let call = argentor_core::ToolCall {
        id: "loader-call-001".to_string(),
        name: "echo".to_string(),
        arguments: serde_json::json!({ "message": "hello world" }),
    };

    let permissions = argentor_security::PermissionSet::new();
    let result = registry.execute(call, &permissions).await;

    assert!(result.is_ok(), "execute should not return Err");
    let tool_result = result.unwrap();
    assert!(
        !tool_result.is_error,
        "ToolResult should indicate success, got error: {}",
        tool_result.content
    );
    assert_eq!(tool_result.call_id, "loader-call-001");
}

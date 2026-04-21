#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for argentor-builtins.
//!
//! These tests verify end-to-end behavior of built-in skills including
//! registry completeness, shell execution, file I/O roundtrips, path blocking,
//! SSRF prevention, memory store/search, artifact CRUD, and HITL approval.

use argentor_builtins::*;
use argentor_core::ToolCall;
use argentor_memory::{EmbeddingProvider, InMemoryVectorStore, LocalEmbedding, VectorStore};
use argentor_skills::skill::Skill;
use argentor_skills::SkillRegistry;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 1. Registry completeness
// ---------------------------------------------------------------------------

#[test]
fn register_builtins_registers_expected_count() {
    let registry = SkillRegistry::new();
    register_builtins(&registry);
    // register_builtins adds: 9 core + 29 utility + 6 document loaders = 44
    assert_eq!(registry.skill_count(), 44);
}

#[test]
fn register_builtins_contains_expected_skill_names() {
    let registry = SkillRegistry::new();
    register_builtins(&registry);

    let core_skills = [
        "shell",
        "file_read",
        "file_write",
        "http_fetch",
        "browser",
        "git",
        "code_analysis",
        "test_runner",
        "human_approval",
    ];
    let utility_skills = [
        "calculator",
        "text_transform",
        "json_query",
        "regex",
        "data_validator",
        "datetime",
        "hash",
        "encode_decode",
        "uuid_generator",
        "web_search",
        "web_scraper",
        "rss_reader",
        "dns_lookup",
        "prompt_guard",
        "secret_scanner",
        "diff",
        "summarizer",
    ];
    for name in core_skills.iter().chain(utility_skills.iter()) {
        assert!(
            registry.get(name).is_some(),
            "Expected skill '{name}' to be registered"
        );
    }
}

#[test]
fn register_builtins_with_memory_registers_all_skills() {
    let registry = SkillRegistry::new();
    let store: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbedding::default());
    register_builtins_with_memory(&registry, store, embedder);
    // 9 core + 29 utility + 6 document loaders + memory_store + memory_search = 46
    assert_eq!(registry.skill_count(), 46);
    assert!(registry.get("memory_store").is_some());
    assert!(registry.get("memory_search").is_some());
}

// ---------------------------------------------------------------------------
// 2. Shell execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shell_skill_executes_echo_hello() {
    let skill = ShellSkill::new();
    let call = ToolCall {
        id: "integ_shell_1".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({"command": "echo hello"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(!result.is_error, "Unexpected error: {}", result.content);
    assert!(
        result.content.contains("hello"),
        "Expected 'hello' in output, got: {}",
        result.content
    );
}

#[tokio::test]
async fn shell_skill_blocks_dangerous_command() {
    let skill = ShellSkill::new();
    let call = ToolCall {
        id: "integ_shell_2".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({"command": "rm -rf /"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("blocked"));
}

// ---------------------------------------------------------------------------
// 3. File write + read roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_write_then_read_roundtrip() {
    let write_skill = FileWriteSkill::new();
    let read_skill = FileReadSkill::new();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("roundtrip.txt");
    let path_str = file_path.to_str().unwrap();

    // Write
    let write_call = ToolCall {
        id: "integ_fw_1".to_string(),
        name: "file_write".to_string(),
        arguments: serde_json::json!({
            "path": path_str,
            "content": "integration test content 42"
        }),
    };
    let write_result = write_skill.execute(write_call).await.unwrap();
    assert!(
        !write_result.is_error,
        "Write failed: {}",
        write_result.content
    );

    // Read back
    let read_call = ToolCall {
        id: "integ_fr_1".to_string(),
        name: "file_read".to_string(),
        arguments: serde_json::json!({"path": path_str}),
    };
    let read_result = read_skill.execute(read_call).await.unwrap();
    assert!(
        !read_result.is_error,
        "Read failed: {}",
        read_result.content
    );
    assert!(
        read_result.content.contains("integration test content 42"),
        "Read content did not match, got: {}",
        read_result.content
    );
}

// ---------------------------------------------------------------------------
// 4. Path blocking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_read_rejects_etc_passwd() {
    let skill = FileReadSkill::new();
    let call = ToolCall {
        id: "integ_block_r1".to_string(),
        name: "file_read".to_string(),
        arguments: serde_json::json!({"path": "/etc/passwd"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error, "Expected blocked path, got success");
    assert!(
        result.content.contains("blocked") || result.content.contains("denied"),
        "Expected blocked/denied message, got: {}",
        result.content
    );
}

#[tokio::test]
async fn file_write_rejects_etc_path() {
    let skill = FileWriteSkill::new();
    let call = ToolCall {
        id: "integ_block_w1".to_string(),
        name: "file_write".to_string(),
        arguments: serde_json::json!({
            "path": "/etc/malicious_file",
            "content": "bad stuff"
        }),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error, "Expected blocked path, got success");
    assert!(
        result.content.contains("blocked") || result.content.contains("denied"),
        "Expected blocked/denied message, got: {}",
        result.content
    );
}

#[tokio::test]
async fn file_write_rejects_relative_path() {
    let skill = FileWriteSkill::new();
    let call = ToolCall {
        id: "integ_block_w2".to_string(),
        name: "file_write".to_string(),
        arguments: serde_json::json!({
            "path": "relative/path.txt",
            "content": "content"
        }),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("absolute"));
}

// ---------------------------------------------------------------------------
// 5. SSRF prevention
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_fetch_blocks_localhost() {
    let skill = HttpFetchSkill::new();
    let call = ToolCall {
        id: "integ_ssrf_1".to_string(),
        name: "http_fetch".to_string(),
        arguments: serde_json::json!({"url": "http://localhost:9999/secret"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error, "Expected SSRF block, got success");
    assert!(
        result.content.contains("private") || result.content.contains("denied"),
        "Expected private/denied in SSRF error, got: {}",
        result.content
    );
}

#[tokio::test]
async fn http_fetch_blocks_internal_ip() {
    let skill = HttpFetchSkill::new();
    let call = ToolCall {
        id: "integ_ssrf_2".to_string(),
        name: "http_fetch".to_string(),
        arguments: serde_json::json!({"url": "http://169.254.169.254/latest/meta-data/"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("private"));
}

#[tokio::test]
async fn http_fetch_blocks_private_10_network() {
    let skill = HttpFetchSkill::new();
    let call = ToolCall {
        id: "integ_ssrf_3".to_string(),
        name: "http_fetch".to_string(),
        arguments: serde_json::json!({"url": "http://10.0.0.1:8080/admin"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("private"));
}

// ---------------------------------------------------------------------------
// 6. Memory roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_store_then_search_roundtrip() {
    let store: Arc<dyn VectorStore> = Arc::new(InMemoryVectorStore::new());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbedding::default());
    let store_skill = MemoryStoreSkill::new(store.clone(), embedder.clone());
    let search_skill = MemorySearchSkill::new(store, embedder);

    // Store several entries
    let entries = [
        "Rust is a systems programming language focused on safety",
        "Python excels at data science and machine learning",
        "Go is designed for cloud infrastructure and networking",
    ];
    for (i, content) in entries.iter().enumerate() {
        let call = ToolCall {
            id: format!("integ_mem_s{i}"),
            name: "memory_store".to_string(),
            arguments: serde_json::json!({"content": content}),
        };
        let result = store_skill.execute(call).await.unwrap();
        assert!(
            !result.is_error,
            "Store failed for entry {i}: {}",
            result.content
        );
    }

    // Search for something related to Rust
    let search_call = ToolCall {
        id: "integ_mem_q1".to_string(),
        name: "memory_search".to_string(),
        arguments: serde_json::json!({"query": "systems programming safety", "top_k": 3}),
    };
    let search_result = search_skill.execute(search_call).await.unwrap();
    assert!(
        !search_result.is_error,
        "Search failed: {}",
        search_result.content
    );

    let parsed: serde_json::Value = serde_json::from_str(&search_result.content).unwrap();
    let total = parsed["total"].as_u64().unwrap();
    assert!(total > 0, "Expected at least one result, got 0");

    // The top result should mention Rust (most semantically similar)
    let top_content = parsed["results"][0]["content"].as_str().unwrap();
    assert!(
        top_content.contains("Rust"),
        "Expected top result to mention Rust, got: {top_content}"
    );
}

// ---------------------------------------------------------------------------
// 7. Artifact CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn artifact_store_create_read_list() {
    let backend = Arc::new(InMemoryArtifactBackend::new());
    let skill = ArtifactStoreSkill::new(backend);

    // Create (store)
    let store_call = ToolCall {
        id: "integ_art_1".to_string(),
        name: "artifact_store".to_string(),
        arguments: serde_json::json!({
            "action": "store",
            "key": "report.md",
            "content": "# Integration Test Report\n\nAll tests passed.",
            "kind": "spec"
        }),
    };
    let store_result = skill.execute(store_call).await.unwrap();
    assert!(
        !store_result.is_error,
        "Store failed: {}",
        store_result.content
    );
    let parsed: serde_json::Value = serde_json::from_str(&store_result.content).unwrap();
    assert_eq!(parsed["stored"], true);
    assert_eq!(parsed["key"], "report.md");

    // Read (retrieve)
    let retrieve_call = ToolCall {
        id: "integ_art_2".to_string(),
        name: "artifact_store".to_string(),
        arguments: serde_json::json!({
            "action": "retrieve",
            "key": "report.md"
        }),
    };
    let retrieve_result = skill.execute(retrieve_call).await.unwrap();
    assert!(
        !retrieve_result.is_error,
        "Retrieve failed: {}",
        retrieve_result.content
    );
    let parsed: serde_json::Value = serde_json::from_str(&retrieve_result.content).unwrap();
    assert_eq!(parsed["found"], true);
    assert!(parsed["content"]
        .as_str()
        .unwrap()
        .contains("Integration Test Report"));

    // List
    let list_call = ToolCall {
        id: "integ_art_3".to_string(),
        name: "artifact_store".to_string(),
        arguments: serde_json::json!({"action": "list"}),
    };
    let list_result = skill.execute(list_call).await.unwrap();
    assert!(
        !list_result.is_error,
        "List failed: {}",
        list_result.content
    );
    let parsed: serde_json::Value = serde_json::from_str(&list_result.content).unwrap();
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["artifacts"][0]["key"], "report.md");
    assert_eq!(parsed["artifacts"][0]["kind"], "spec");
}

#[tokio::test]
async fn artifact_store_retrieve_nonexistent_returns_not_found() {
    let backend = Arc::new(InMemoryArtifactBackend::new());
    let skill = ArtifactStoreSkill::new(backend);

    let call = ToolCall {
        id: "integ_art_4".to_string(),
        name: "artifact_store".to_string(),
        arguments: serde_json::json!({
            "action": "retrieve",
            "key": "nonexistent.txt"
        }),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(!result.is_error); // returns success with found: false
    let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["found"], false);
}

#[tokio::test]
async fn artifact_store_invalid_action_returns_error() {
    let backend = Arc::new(InMemoryArtifactBackend::new());
    let skill = ArtifactStoreSkill::new(backend);

    let call = ToolCall {
        id: "integ_art_5".to_string(),
        name: "artifact_store".to_string(),
        arguments: serde_json::json!({"action": "delete"}),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(result.is_error);
}

// ---------------------------------------------------------------------------
// 8. HumanApprovalSkill with AutoApproveChannel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn human_approval_auto_approve_passes_through() {
    let skill = HumanApprovalSkill::auto_approve();
    let call = ToolCall {
        id: "integ_hitl_1".to_string(),
        name: "human_approval".to_string(),
        arguments: serde_json::json!({
            "task_id": "deploy-prod-v2",
            "description": "Deploy v2.0.0 to production",
            "risk_level": "high",
            "context": "Auth module changes included"
        }),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(!result.is_error, "Approval failed: {}", result.content);
    let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["approved"], true);
    assert_eq!(parsed["reviewer"], "system");
    assert_eq!(parsed["task_id"], "deploy-prod-v2");
}

#[tokio::test]
async fn human_approval_callback_channel_rejects() {
    let channel = CallbackApprovalChannel::new(|_req| {
        Box::pin(async move {
            Ok(ApprovalDecision {
                approved: false,
                reason: Some("Security review required".into()),
                reviewer: "security-bot".into(),
            })
        })
    });
    let skill = HumanApprovalSkill::new(Arc::new(channel));

    let call = ToolCall {
        id: "integ_hitl_2".to_string(),
        name: "human_approval".to_string(),
        arguments: serde_json::json!({
            "task_id": "drop-tables",
            "description": "Drop all production database tables",
            "risk_level": "critical"
        }),
    };
    let result = skill.execute(call).await.unwrap();
    assert!(!result.is_error); // returns success even on rejection (decision is in payload)
    let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["approved"], false);
    assert_eq!(parsed["reviewer"], "security-bot");
    assert_eq!(
        parsed["reason"].as_str().unwrap(),
        "Security review required"
    );
}

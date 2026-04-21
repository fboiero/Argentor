#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for the argentor-mcp crate.
//!
//! Covers: McpProxy, ToolDiscovery, McpServerManager, and McpServerConfig.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_mcp::proxy::{McpProxy, ProxyAgentMetrics};
use argentor_mcp::{McpServerConfig, McpServerManager, ToolDiscovery};
use argentor_security::PermissionSet;
use argentor_skills::skill::{Skill, SkillDescriptor};
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A simple in-memory skill for testing proxy execution paths.
struct StubSkill {
    descriptor: SkillDescriptor,
    /// When true, the skill returns an error result.
    fail: bool,
}

impl StubSkill {
    fn ok(name: &str) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: name.to_string(),
                description: format!("Stub skill: {name}"),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![],
            },
            fail: false,
        }
    }

    fn failing(name: &str) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: name.to_string(),
                description: format!("Failing stub: {name}"),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![],
            },
            fail: true,
        }
    }
}

#[async_trait]
impl Skill for StubSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        if self.fail {
            Ok(ToolResult::error(&call.id, "intentional failure"))
        } else {
            Ok(ToolResult::success(&call.id, "ok"))
        }
    }
}

fn make_call(id: &str, tool: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: tool.to_string(),
        arguments: serde_json::json!({}),
    }
}

fn make_proxy_with(skills: Vec<Arc<dyn Skill>>) -> McpProxy {
    let registry = SkillRegistry::new();
    for skill in skills {
        registry.register(skill);
    }
    McpProxy::new(Arc::new(registry), PermissionSet::new())
}

fn make_empty_proxy() -> McpProxy {
    make_proxy_with(vec![])
}

// ---------------------------------------------------------------------------
// 1. McpProxy creation -- create proxy, verify it initializes empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_creation_empty() {
    let proxy = make_empty_proxy();

    // No calls have been made yet, so totals should be zero.
    assert_eq!(proxy.total_calls().await, 0);

    // No agent metrics exist.
    let all = proxy.all_metrics().await;
    assert!(all.is_empty());

    // No logs recorded.
    let logs = proxy.recent_logs(100).await;
    assert!(logs.is_empty());

    // JSON snapshot should reflect the empty state.
    let json = proxy.to_json().await;
    assert_eq!(json["total_calls"], 0);
    assert!(json["recent_logs"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 2. McpProxy logging -- proxy logs requests/responses correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_logging() {
    let proxy = make_proxy_with(vec![
        Arc::new(StubSkill::ok("greet")),
        Arc::new(StubSkill::failing("boom")),
    ]);

    // Successful call
    let result = proxy.execute(make_call("c1", "greet"), "agent-a").await;
    assert!(result.is_ok());
    let tr = result.unwrap();
    assert!(!tr.is_error);

    // Failing call (tool returns error result, but no Err)
    let result = proxy.execute(make_call("c2", "boom"), "agent-a").await;
    assert!(result.is_ok());
    let tr = result.unwrap();
    assert!(tr.is_error);

    // Unknown tool call (returns Err)
    let result = proxy.execute(make_call("c3", "unknown"), "agent-a").await;
    assert!(result.is_err());

    // Verify logs: should have 3 entries (one per execute call).
    let logs = proxy.recent_logs(10).await;
    assert_eq!(logs.len(), 3);

    // recent_logs returns newest first.
    assert_eq!(logs[0].call_id, "c3");
    assert!(!logs[0].success);
    assert!(logs[0].error.is_some());

    assert_eq!(logs[1].call_id, "c2");
    assert!(!logs[1].success);

    assert_eq!(logs[2].call_id, "c1");
    assert!(logs[2].success);
    assert!(logs[2].error.is_none());
    assert_eq!(logs[2].tool_name, "greet");
    assert_eq!(logs[2].agent_id, "agent-a");
}

// ---------------------------------------------------------------------------
// 3. McpProxy metrics -- proxy tracks request counts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_metrics_tracking() {
    let proxy = make_proxy_with(vec![
        Arc::new(StubSkill::ok("ping")),
        Arc::new(StubSkill::failing("fail_tool")),
    ]);

    // 3 successful calls from agent-1
    for i in 0..3 {
        let _ = proxy
            .execute(make_call(&format!("s{i}"), "ping"), "agent-1")
            .await;
    }

    // 2 failing calls from agent-1
    for i in 0..2 {
        let _ = proxy
            .execute(make_call(&format!("f{i}"), "fail_tool"), "agent-1")
            .await;
    }

    // 1 call from agent-2
    let _ = proxy.execute(make_call("x1", "ping"), "agent-2").await;

    // Per-agent metrics for agent-1
    let m1 = proxy.agent_metrics("agent-1").await;
    assert_eq!(m1.total_calls, 5);
    assert_eq!(m1.successful_calls, 3);
    assert_eq!(m1.failed_calls, 2);

    // Per-agent metrics for agent-2
    let m2 = proxy.agent_metrics("agent-2").await;
    assert_eq!(m2.total_calls, 1);
    assert_eq!(m2.successful_calls, 1);
    assert_eq!(m2.failed_calls, 0);

    // Unknown agent returns defaults
    let m_unknown = proxy.agent_metrics("nobody").await;
    assert_eq!(m_unknown.total_calls, 0);
    assert_eq!(m_unknown.denied_calls, 0);

    // Global total
    assert_eq!(proxy.total_calls().await, 6);

    // all_metrics should have exactly two agents
    let all = proxy.all_metrics().await;
    assert_eq!(all.len(), 2);
    assert!(all.contains_key("agent-1"));
    assert!(all.contains_key("agent-2"));
}

// ---------------------------------------------------------------------------
// 4. McpProxy log rotation -- recent_logs respects the limit parameter
//    (the internal cap is 10_000 by default, so we test the limit arg)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_log_rotation_via_limit() {
    let proxy = make_proxy_with(vec![Arc::new(StubSkill::ok("tool"))]);

    // Execute 20 calls so we have 20 log entries.
    for i in 0..20 {
        let _ = proxy
            .execute(make_call(&format!("r{i}"), "tool"), "agent-rot")
            .await;
    }

    // Asking for all should give 20.
    let all_logs = proxy.recent_logs(100).await;
    assert_eq!(all_logs.len(), 20);

    // Asking for 5 should give exactly 5 (the 5 most recent).
    let five = proxy.recent_logs(5).await;
    assert_eq!(five.len(), 5);
    // The first entry should be the most recent (r19).
    assert_eq!(five[0].call_id, "r19");
    assert_eq!(five[4].call_id, "r15");

    // Asking for 0 should return empty.
    let zero = proxy.recent_logs(0).await;
    assert!(zero.is_empty());
}

// ---------------------------------------------------------------------------
// 5. McpProxy denied tracking -- record_denied updates denied_calls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_denied_tracking() {
    let proxy = make_empty_proxy();

    // Record several denials for different agents.
    proxy.record_denied("agent-x", "secret_tool").await;
    proxy.record_denied("agent-x", "another_tool").await;
    proxy.record_denied("agent-y", "secret_tool").await;

    let mx = proxy.agent_metrics("agent-x").await;
    assert_eq!(mx.denied_calls, 2);
    assert_eq!(mx.total_calls, 0); // denials are separate from executed calls

    let my = proxy.agent_metrics("agent-y").await;
    assert_eq!(my.denied_calls, 1);

    // Verify that denied calls do NOT show up in the log (only executed calls do).
    let logs = proxy.recent_logs(100).await;
    assert!(logs.is_empty());

    // Verify the all_metrics map includes both agents.
    let all = proxy.all_metrics().await;
    assert_eq!(all.len(), 2);

    // Combine execution and denial for the same agent.
    let proxy2 = make_proxy_with(vec![Arc::new(StubSkill::ok("allowed"))]);
    let _ = proxy2.execute(make_call("e1", "allowed"), "agent-z").await;
    proxy2.record_denied("agent-z", "forbidden").await;

    let mz = proxy2.agent_metrics("agent-z").await;
    assert_eq!(mz.total_calls, 1);
    assert_eq!(mz.denied_calls, 1);
    assert_eq!(mz.successful_calls, 1);
}

// ---------------------------------------------------------------------------
// 6. ToolDiscovery -- filter tools and estimate savings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tool_discovery() {
    let registry = SkillRegistry::new();
    registry.register(Arc::new(StubSkill::ok("memory_store")));
    registry.register(Arc::new(StubSkill::ok("memory_search")));
    registry.register(Arc::new(StubSkill::ok("echo")));
    registry.register(Arc::new(StubSkill::ok("http_fetch")));

    // filter_by_allowed: only selected tools
    let allowed = vec!["echo".to_string(), "memory_store".to_string()];
    let filtered = ToolDiscovery::filter_by_allowed(&registry, &allowed);
    assert_eq!(filtered.len(), 2);
    let names: Vec<&str> = filtered.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"memory_store"));

    // filter_by_allowed with empty list returns nothing
    let empty = ToolDiscovery::filter_by_allowed(&registry, &[]);
    assert!(empty.is_empty());

    // filter_by_context: substring match on name or description
    let ctx = ToolDiscovery::filter_by_context(&registry, &["memory"]);
    assert_eq!(ctx.len(), 2);

    // filter_by_context with no match
    let no_match = ToolDiscovery::filter_by_context(&registry, &["zzz_no_match"]);
    assert!(no_match.is_empty());

    // filter_by_context with empty keywords returns all
    let all = ToolDiscovery::filter_by_context(&registry, &[]);
    assert_eq!(all.len(), 4);

    // Token savings estimation
    assert!((ToolDiscovery::estimate_token_savings(100, 2) - 98.0).abs() < f64::EPSILON);
    assert!((ToolDiscovery::estimate_token_savings(4, 2) - 50.0).abs() < f64::EPSILON);
    assert!((ToolDiscovery::estimate_token_savings(0, 0) - 0.0).abs() < f64::EPSILON);
    assert!((ToolDiscovery::estimate_token_savings(10, 10) - 0.0).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// 7. McpServerManager -- create manager, verify server list, connect errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_server_manager() {
    let manager = McpServerManager::new();

    // Freshly created manager has no servers.
    assert_eq!(manager.server_count().await, 0);
    let status = manager.status().await;
    assert!(status.is_empty());

    // Attempting to connect a nonexistent server should produce an error.
    let config = McpServerConfig {
        command: "/nonexistent/binary".to_string(),
        args: vec![],
        env: HashMap::new(),
        auto_reconnect: false,
        health_check_interval_secs: 0,
    };

    let registry = SkillRegistry::new();
    let errors = manager.connect_all(&[config], &registry).await;
    assert_eq!(errors.len(), 1);
    // Still no servers managed (the failed one is not tracked).
    assert_eq!(manager.server_count().await, 0);

    // Connecting with multiple bad configs: all should fail.
    let configs = vec![
        McpServerConfig {
            command: "/bad/server1".to_string(),
            args: vec![],
            env: HashMap::new(),
            auto_reconnect: false,
            health_check_interval_secs: 0,
        },
        McpServerConfig {
            command: "/bad/server2".to_string(),
            args: vec!["--flag".to_string()],
            env: HashMap::new(),
            auto_reconnect: true,
            health_check_interval_secs: 30,
        },
    ];
    let errors = manager.connect_all(&configs, &registry).await;
    assert_eq!(errors.len(), 2);
    assert_eq!(manager.server_count().await, 0);

    // Health check on empty manager should not panic.
    manager.health_check().await;
}

// ---------------------------------------------------------------------------
// 8. McpServerConfig validation -- deserialization and defaults
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_server_config_validation() {
    // Minimal JSON: only "command" is required.
    let cfg: McpServerConfig = serde_json::from_str(r#"{"command":"my-server"}"#).unwrap();
    assert_eq!(cfg.command, "my-server");
    assert!(cfg.args.is_empty());
    assert!(cfg.env.is_empty());
    assert!(cfg.auto_reconnect); // default true
    assert_eq!(cfg.health_check_interval_secs, 60); // default 60

    // Full JSON: all fields specified.
    let cfg: McpServerConfig = serde_json::from_str(
        r#"{
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
            "env": {"NODE_ENV": "production"},
            "auto_reconnect": false,
            "health_check_interval_secs": 120
        }"#,
    )
    .unwrap();
    assert_eq!(cfg.command, "npx");
    assert_eq!(
        cfg.args,
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
    );
    assert_eq!(cfg.env.get("NODE_ENV").unwrap(), "production");
    assert!(!cfg.auto_reconnect);
    assert_eq!(cfg.health_check_interval_secs, 120);

    // Missing command should fail.
    let bad: Result<McpServerConfig, _> = serde_json::from_str(r#"{"args":["x"]}"#);
    assert!(bad.is_err());

    // McpServerStatus serializes correctly.
    let status = argentor_mcp::McpServerStatus {
        command: "test-srv".to_string(),
        connected: true,
        tool_count: 3,
        connected_at: None,
        last_health_check: None,
        reconnect_count: 0,
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["command"], "test-srv");
    assert_eq!(json["connected"], true);
    assert_eq!(json["tool_count"], 3);
    assert_eq!(json["reconnect_count"], 0);

    // ProxyAgentMetrics defaults.
    let m = ProxyAgentMetrics::default();
    assert_eq!(m.total_calls, 0);
    assert_eq!(m.successful_calls, 0);
    assert_eq!(m.failed_calls, 0);
    assert_eq!(m.total_duration_ms, 0);
    assert_eq!(m.denied_calls, 0);

    // ProxyAgentMetrics round-trip serialization.
    let serialized = serde_json::to_string(&m).unwrap();
    let deserialized: ProxyAgentMetrics = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.total_calls, m.total_calls);
    assert_eq!(deserialized.denied_calls, m.denied_calls);
}

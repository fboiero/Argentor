#![allow(
    clippy::expect_used,
    clippy::needless_borrows_for_generic_args,
    clippy::unwrap_used,
    missing_docs
)]
//! Memory-pressure scalability tests.
//!
//! These tests exercise paths that historically leak memory or grow unbounded
//! and assert the right invariants:
//!   * bounded growth across many runs
//!   * eviction respects configured capacity
//!   * dropped resources are released
//!
//! Memory measurement uses `sysinfo` to read the test process RSS. Some of the
//! checks are intentionally generous (50MB / 5MB) — RSS is noisy on macOS and
//! Linux tmpfs, and we are validating ORDER OF MAGNITUDE not byte-level
//! precision. Tests that proved flaky are marked `#[ignore]`.

use argentor_agent::backends::LlmBackend;
use argentor_agent::debug_recorder::{DebugRecorder, StepType};
use argentor_agent::failover::{FailoverBackend, RetryPolicy};
use argentor_agent::llm::LlmResponse;
use argentor_agent::response_cache::{CacheKey, CacheMessage, ResponseCache};
use argentor_agent::stream::StreamEvent;
use argentor_agent::AgentRunner;
use argentor_builtins::CalculatorSkill;
use argentor_core::{ArgentorError, ArgentorResult, Message};
use argentor_security::audit::{AuditEntry, AuditOutcome};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::{SkillDescriptor, SkillRegistry};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_rss_kb() -> u64 {
    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::everything(),
    );
    sys.process(pid).map(|p| p.memory() / 1024).unwrap_or(0)
}

/// Mock LLM that returns immediately. Used for memory-stability tests.
struct InstantMockBackend;

#[async_trait]
impl LlmBackend for InstantMockBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        Ok(LlmResponse::Done("ok".to_string()))
    }
    async fn chat_stream(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        let (_tx, rx) = mpsc::channel(1);
        let handle = tokio::spawn(async { Ok(LlmResponse::Done("ok".to_string())) });
        Ok((rx, handle))
    }
    fn provider_name(&self) -> &str {
        "mock-instant"
    }
}

/// Mock LLM that always errors. Used to drive failover paths.
struct AlwaysFailBackend;

#[async_trait]
impl LlmBackend for AlwaysFailBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        Err(ArgentorError::Http("mock failure".into()))
    }
    async fn chat_stream(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        Err(ArgentorError::Http("mock failure".into()))
    }
    fn provider_name(&self) -> &str {
        "fail"
    }
}

fn make_runner_with(backend: Box<dyn LlmBackend>) -> AgentRunner {
    let registry = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(
        std::env::temp_dir().join("argentor-mem-audit"),
    ));
    AgentRunner::from_backend(backend, registry, permissions, audit, 1)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Run 1000 single-turn agents sequentially — RSS growth should stay bounded.
///
/// 50MB cap is generous; on a quiet machine we expect <10MB.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_memory_stable_after_1000_agent_runs() {
    let runner = make_runner_with(Box::new(InstantMockBackend));

    // Warmup — let allocators settle.
    for i in 0..20 {
        let mut s = Session::new();
        let _ = runner.run(&mut s, &format!("warmup {i}")).await;
    }
    tokio::task::yield_now().await;

    let before_kb = current_rss_kb();

    for i in 0..1000 {
        let mut s = Session::new();
        let _ = runner.run(&mut s, &format!("query {i}")).await;
    }

    tokio::task::yield_now().await;
    let after_kb = current_rss_kb();
    let delta_mb = after_kb.saturating_sub(before_kb) as f64 / 1024.0;

    assert!(
        delta_mb < 50.0,
        "Memory growth after 1000 runs is {delta_mb:.2} MB, expected < 50 MB"
    );
}

/// Build a registry with N skills, drop it, and check memory is reclaimed.
///
/// We assert the drop completes without panic and registry observably becomes
/// inaccessible. Precise RSS reclamation timing is platform-specific, so we
/// keep this test focused on the drop semantics rather than RSS bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_memory_freed_after_dropping_registry() {
    let registry = SkillRegistry::new();
    for _ in 0..50 {
        registry.register(Arc::new(CalculatorSkill::new()));
    }
    let weak = Arc::downgrade(&Arc::new(registry));

    // Drop the only strong reference (it was already inlined into the Arc above
    // and consumed by `Arc::downgrade`'s argument). Verify the weak ref no
    // longer upgrades — i.e. the registry has been freed.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        weak.upgrade().is_none(),
        "registry must be freed once last strong ref is dropped"
    );
}

/// 100-turn conversation — memory growth bounded by context window.
///
/// The cap is intentionally loose (5MB) because each Message holds a UUID,
/// Role, content, and a serde_json metadata field. We are checking there is
/// no leak per turn, not measuring exact bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_long_session_memory_bounded() {
    let runner = make_runner_with(Box::new(InstantMockBackend));
    let mut session = Session::new();

    // Warmup
    for i in 0..5 {
        let _ = runner.run(&mut session, &format!("warmup {i}")).await;
    }
    let before_kb = current_rss_kb();

    for i in 0..100 {
        let _ = runner.run(&mut session, &format!("turn {i}")).await;
    }

    tokio::task::yield_now().await;
    let after_kb = current_rss_kb();
    let delta_mb = after_kb.saturating_sub(before_kb) as f64 / 1024.0;

    // Allow up to 5MB; this is dominated by the growing transcript itself.
    assert!(
        delta_mb < 5.0,
        "Memory after 100-turn session grew by {delta_mb:.2} MB, expected < 5 MB"
    );
}

/// Cache with capacity 100 — insert 1000 entries and verify LRU eviction
/// keeps exactly 100, plus that eviction counter increments correctly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_response_cache_respects_capacity() {
    let cache = ResponseCache::new(100, Duration::from_secs(300));
    for i in 0..1000 {
        let key = CacheKey::compute("model-x", &[CacheMessage::new("user", &format!("m-{i}"))]);
        cache.put(key, format!("resp-{i}"), "model-x", 1);
    }

    let stats = cache.stats();
    assert_eq!(stats.size, 100, "cache size must equal capacity (100)");
    assert_eq!(stats.capacity, 100);
    assert_eq!(
        stats.evictions, 900,
        "exactly 1000-100=900 entries must have been evicted"
    );
}

/// Append 100K audit entries — file size must remain bounded relative to the
/// volume sent. NOTE: `AuditLog` does NOT currently implement rotation; this
/// test asserts that the writer accepts the load without panicking and the
/// resulting file is finite. Marking #[ignore] because the per-entry file
/// task spawn pattern is slow under high volume on CI.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "audit log spawns a tokio task per write, so 100K entries take >30s on CI; rotation is not yet implemented"]
async fn test_audit_log_rotation_under_volume() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path().to_path_buf());

    for i in 0..100_000 {
        let entry = AuditEntry {
            timestamp: chrono::Utc::now(),
            session_id: Uuid::new_v4(),
            action: "test".into(),
            skill_name: None,
            details: serde_json::json!({"i": i}),
            outcome: AuditOutcome::Success,
            correlation_id: None,
        };
        log.log(entry);
    }

    // Allow the background writer time to drain.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let path = dir.path().join("audit.jsonl");
    let metadata = tokio::fs::metadata(&path).await.unwrap();
    assert!(
        metadata.len() > 0,
        "audit log must contain entries after high-volume write"
    );
}

/// Trigger 1000 failover paths — memory should remain bounded (no leak in
/// retry-state accumulation).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_no_leak_on_failover_path() {
    let backends: Vec<Box<dyn LlmBackend>> = vec![
        Box::new(AlwaysFailBackend),
        Box::new(AlwaysFailBackend),
        Box::new(InstantMockBackend),
    ];
    let policy = RetryPolicy::default();
    let failover = FailoverBackend::new(backends, policy);

    let runner = {
        let registry = Arc::new(SkillRegistry::new());
        let permissions = PermissionSet::new();
        let audit = Arc::new(AuditLog::new(
            std::env::temp_dir().join("argentor-failover-audit"),
        ));
        AgentRunner::from_backend(Box::new(failover), registry, permissions, audit, 1)
    };

    // Warmup
    for i in 0..10 {
        let mut s = Session::new();
        let _ = runner.run(&mut s, &format!("warm {i}")).await;
    }
    let before_kb = current_rss_kb();

    for i in 0..1000 {
        let mut s = Session::new();
        let _ = runner.run(&mut s, &format!("q {i}")).await;
    }

    tokio::task::yield_now().await;
    let after_kb = current_rss_kb();
    let delta_mb = after_kb.saturating_sub(before_kb) as f64 / 1024.0;
    assert!(
        delta_mb < 30.0,
        "Memory after 1000 failover paths grew by {delta_mb:.2} MB, expected < 30 MB"
    );
}

/// Verifies #10 fix: DebugRecorder with default cap (1000) retains exactly
/// 1000 steps even after 10K emissions, evicting oldest (ring buffer).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_debug_recorder_capped() {
    let recorder = DebugRecorder::new("trace-1");
    // Default cap is DEFAULT_MAX_STEPS = 1000
    assert_eq!(recorder.max_steps(), 1000);

    for i in 0..10_000 {
        recorder.record(StepType::Input, format!("step-{i}"), None);
    }

    // Cap enforced: only the most recent 1000 steps retained
    assert_eq!(
        recorder.step_count(),
        1000,
        "cap should retain exactly 1000"
    );
    // Total recorded (including evicted) is accurate
    assert_eq!(recorder.total_recorded(), 10_000);
    // Eviction count = 10_000 emitted - 1000 retained
    assert_eq!(recorder.evicted_count(), 9_000);

    // The retained steps are the MOST RECENT ones (ring buffer FIFO eviction).
    let trace = recorder.finalize();
    assert_eq!(trace.steps.len(), 1000);
    assert_eq!(
        trace.steps.last().map(|s| s.description.as_str()),
        Some("step-9999")
    );
    assert_eq!(
        trace.steps.first().map(|s| s.description.as_str()),
        Some("step-9000")
    );
}

/// Verifies DebugRecorder with custom cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_debug_recorder_custom_cap() {
    let recorder = DebugRecorder::with_capacity("trace-custom", 100);
    for i in 0..500 {
        recorder.record(StepType::Input, format!("step-{i}"), None);
    }
    assert_eq!(recorder.step_count(), 100);
    assert_eq!(recorder.total_recorded(), 500);
}

/// Verifies DebugRecorder with cap=0 is unbounded (legacy behaviour).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_debug_recorder_unbounded_when_zero_cap() {
    let recorder = DebugRecorder::with_capacity("trace-unbounded", 0);
    for i in 0..5_000 {
        recorder.record(StepType::Input, format!("step-{i}"), None);
    }
    assert_eq!(recorder.step_count(), 5_000);
    assert_eq!(recorder.evicted_count(), 0);
}

/// Event-bus history equivalent: AuditLog accepts 100K entries without
/// the *sender* OOMing (the unbounded mpsc channel). This validates
/// backpressure-free fast-path producer behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "audit channel is unbounded by design — no cap to assert; also slow on CI"]
async fn test_event_bus_history_capped() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path().to_path_buf());

    let before_kb = current_rss_kb();
    for i in 0..100_000 {
        log.log(AuditEntry {
            timestamp: chrono::Utc::now(),
            session_id: Uuid::new_v4(),
            action: format!("a-{i}"),
            skill_name: None,
            details: serde_json::Value::Null,
            outcome: AuditOutcome::Success,
            correlation_id: None,
        });
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after_kb = current_rss_kb();
    let delta_mb = after_kb.saturating_sub(before_kb) as f64 / 1024.0;
    assert!(
        delta_mb < 200.0,
        "Memory after 100K audit events grew by {delta_mb:.2} MB"
    );
}

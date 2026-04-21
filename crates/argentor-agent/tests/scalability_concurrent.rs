#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]
//! Concurrent execution scalability tests.
//!
//! These integration tests verify that Argentor's core primitives behave
//! correctly under heavy concurrent load. They are NOT performance benchmarks —
//! they assert correctness invariants (no panics, no corruption, no deadlock,
//! consistent state) when many tasks hit the same components in parallel.
//!
//! Each test uses a `MockLlmBackend` that sleeps ~10ms per call, simulating a
//! fast LLM. Tests should generally complete in well under 10s on a 4-core
//! machine. Tests that proved flaky on CI are marked `#[ignore]` with a note.

use argentor_agent::backends::LlmBackend;
use argentor_agent::circuit_breaker::{CircuitBreakerRegistry, CircuitConfig, CircuitState};
use argentor_agent::guardrails::GuardrailEngine;
use argentor_agent::learning::{LearningEngine, LearningFeedback};
use argentor_agent::llm::LlmResponse;
use argentor_agent::response_cache::{CacheKey, CacheMessage, ResponseCache};
use argentor_agent::stream::StreamEvent;
use argentor_agent::AgentRunner;
use argentor_builtins::CalculatorSkill;
use argentor_core::{ArgentorResult, Message, ToolCall};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::skill::Skill;
use argentor_skills::{SkillDescriptor, SkillRegistry};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Mock LLM that sleeps 10ms then returns a final response.
struct MockLlmBackend;

#[async_trait]
impl LlmBackend for MockLlmBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        tokio::time::sleep(Duration::from_millis(10)).await;
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
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(LlmResponse::Done("ok".to_string()))
        });
        Ok((rx, handle))
    }

    fn provider_name(&self) -> &str {
        "mock"
    }
}

fn make_runner() -> Arc<AgentRunner> {
    let registry = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(
        std::env::temp_dir().join("argentor-scal-audit"),
    ));
    Arc::new(AgentRunner::from_backend(
        Box::new(MockLlmBackend),
        registry,
        permissions,
        audit,
        2,
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Spawn 100 concurrent agents — they should all complete within 5s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_100_concurrent_agents_complete() {
    let runner = make_runner();
    const N: usize = 100;

    let start = std::time::Instant::now();
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let r = runner.clone();
        handles.push(tokio::spawn(async move {
            let mut session = Session::new();
            r.run(&mut session, &format!("query {i}")).await
        }));
    }

    let mut succeeded = 0usize;
    for h in handles {
        let res = h.await.expect("task did not panic");
        if res.is_ok() {
            succeeded += 1;
        }
    }
    let elapsed = start.elapsed();

    assert_eq!(succeeded, N, "All 100 agents should complete successfully");
    assert!(
        elapsed < Duration::from_secs(5),
        "100 concurrent agents took too long: {elapsed:?}"
    );
}

/// 1000 concurrent calculator.execute() calls — no panics, all succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_1000_concurrent_skill_executions() {
    let calc: Arc<dyn Skill> = Arc::new(CalculatorSkill::new());
    const N: usize = 1000;

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let c = calc.clone();
        handles.push(tokio::spawn(async move {
            let call = ToolCall {
                id: format!("c-{i}"),
                name: "calculator".into(),
                arguments: serde_json::json!({"operation": "add", "a": i as f64, "b": 1.0}),
            };
            c.execute(call).await
        }));
    }

    let mut ok_count = 0usize;
    for h in handles {
        let res = h.await.expect("task panicked");
        if res.is_ok() {
            ok_count += 1;
        }
    }
    assert_eq!(ok_count, N, "All 1000 calculator executions must succeed");
}

/// Many threads competing for the same cache key — value remains coherent
/// and the hit counter increases monotonically without lost updates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_cache_hits_no_corruption() {
    let cache = ResponseCache::new(16, Duration::from_secs(60));
    let key = CacheKey::compute("model-x", &[CacheMessage::new("user", "ping")]);
    cache.put(key.clone(), "pong".to_string(), "model-x", 1);

    const N: usize = 100;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let c = cache.clone();
        let k = key.clone();
        handles.push(tokio::spawn(async move { c.get(&k) }));
    }

    let mut all_pong = true;
    for h in handles {
        let v = h.await.expect("task panicked");
        if v.as_deref() != Some("pong") {
            all_pong = false;
            break;
        }
    }
    assert!(all_pong, "Every concurrent reader must see the same value");

    let stats = cache.stats();
    // Hits must equal N exactly: no lost updates, no double counting.
    assert_eq!(
        stats.hits, N as u64,
        "hit counter lost or duplicated updates"
    );
    assert_eq!(stats.misses, 0);
    assert_eq!(stats.size, 1);
}

/// 500 sessions created in parallel — all unique IDs, no deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_session_creation() {
    const N: usize = 500;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        handles.push(tokio::spawn(async { Session::new().id }));
    }

    let mut ids = Vec::with_capacity(N);
    for h in handles {
        ids.push(h.await.expect("task panicked"));
    }
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), N, "all session IDs must be unique");
}

/// 1000 readers + 10 writers on a SkillRegistry — no panics, consistent state.
///
/// Note: SkillRegistry is not `Sync` for mutation, so writers must hold
/// exclusive access. We wrap it in `RwLock` to model the shared/exclusive
/// pattern that production code uses.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_registry_access() {
    let registry: Arc<RwLock<SkillRegistry>> = Arc::new(RwLock::new(SkillRegistry::new()));
    {
        let mut w = registry.write().unwrap();
        w.register(Arc::new(CalculatorSkill::new()));
    }

    let mut handles = Vec::new();

    // 1000 readers
    for _ in 0..1000 {
        let r = registry.clone();
        handles.push(tokio::spawn(async move {
            let g = r.read().unwrap();
            g.get("calculator").is_some()
        }));
    }

    // 10 writers — register additional copies of the same skill (idempotent).
    for _ in 0..10 {
        let r = registry.clone();
        handles.push(tokio::spawn(async move {
            let mut g = r.write().unwrap();
            g.register(Arc::new(CalculatorSkill::new()));
            true
        }));
    }

    let mut all_ok = true;
    for h in handles {
        if !h.await.expect("task panicked") {
            all_ok = false;
        }
    }
    assert!(all_ok, "registry readers/writers must not fail");

    let final_registry = registry.read().unwrap();
    assert!(final_registry.get("calculator").is_some());
}

/// 200 concurrent input checks through a single GuardrailEngine — no false
/// positives on safe input, no panics.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_shared_guardrail_engine_under_load() {
    let engine = Arc::new(GuardrailEngine::new());
    const N: usize = 200;
    let mut handles = Vec::with_capacity(N);

    for i in 0..N {
        let e = engine.clone();
        handles.push(tokio::spawn(async move {
            let input = format!("benign user query number {i}");
            e.check_input(&input)
        }));
    }

    let mut allowed = 0usize;
    for h in handles {
        let result = h.await.expect("task panicked");
        if result.passed {
            allowed += 1;
        }
    }
    assert_eq!(
        allowed, N,
        "All benign queries must pass the guardrail (no false positives)"
    );
}

/// 100 concurrent failures into a single circuit breaker — state transitions
/// remain consistent (the circuit either stays Closed or transitions to Open
/// exactly once; never crashes mid-transition).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_circuit_breaker_state_consistency() {
    let registry = Arc::new(CircuitBreakerRegistry::new(
        CircuitConfig::new(5).with_recovery_timeout(Duration::from_secs(60)),
    ));

    const N: usize = 100;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let r = registry.clone();
        handles.push(tokio::spawn(async move {
            r.record_failure("provider-x");
        }));
    }
    for h in handles {
        h.await.expect("task panicked");
    }

    let status = registry
        .status("provider-x")
        .expect("breaker registered after failures");
    // After 100 failures with a threshold of 5, the breaker MUST be Open.
    assert_eq!(
        status.state,
        CircuitState::Open,
        "circuit should be Open after 100 consecutive failures"
    );
    // Total failures must equal exactly N (no lost/double counts).
    assert_eq!(status.total_failures, N as u64);
}

/// 500 concurrent record_feedback() calls — stats are coherent: the total
/// uses counter equals the number of records sent.
///
/// `LearningEngine` uses `Arc<RwLock<LearningInner>>` interior mutability, so
/// `record_feedback` takes `&self` and multiple concurrent callers are safe
/// without any outer `Mutex`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_learning_engine_concurrent_feedback() {
    let engine = Arc::new(LearningEngine::with_defaults());
    const N: usize = 500;
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let e = engine.clone();
        handles.push(tokio::spawn(async move {
            let fb = LearningFeedback {
                tool_name: "calculator".to_string(),
                query_context: "math question".to_string(),
                success: i % 2 == 0,
                execution_time_ms: 5,
                tokens_used: 10,
                error_type: None,
            };
            e.record_feedback(&fb);
        }));
    }
    for h in handles {
        h.await.expect("task panicked");
    }

    let stats = engine
        .get_stats("calculator")
        .expect("calculator stats should exist after feedback");
    assert_eq!(
        stats.total_uses, N as u64,
        "total_uses must equal number of feedback records"
    );
    assert_eq!(
        stats.successes + stats.failures,
        N as u64,
        "successes + failures must equal total_uses"
    );
}

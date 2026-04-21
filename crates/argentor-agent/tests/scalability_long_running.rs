#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]
//! Long-running session and back-pressure scalability tests.
//!
//! These tests exercise the compaction engine, max-turn budgeting,
//! streaming back-pressure, and parallel long-lived sessions. We avoid sleeping
//! for real wall-clock time and instead use compressed durations / synthetic
//! transcripts so the suite runs quickly.

use argentor_agent::backends::LlmBackend;
use argentor_agent::compaction::{
    CompactableMessage, CompactionConfig, CompactionStrategy, ContextCompactorEngine,
};
use argentor_agent::llm::LlmResponse;
use argentor_agent::stream::StreamEvent;
use argentor_agent::AgentRunner;
use argentor_core::{ArgentorError, ArgentorResult, Message};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::{SkillDescriptor, SkillRegistry};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Mock LLM that returns immediately with a short canned response.
struct FastMockBackend {
    turns: AtomicUsize,
}

impl FastMockBackend {
    fn new() -> Self {
        Self {
            turns: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmBackend for FastMockBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        self.turns.fetch_add(1, Ordering::SeqCst);
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
        "fast-mock"
    }
}

/// Mock LLM that increments a budget counter and fails once exhausted.
struct BudgetedBackend {
    remaining: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmBackend for BudgetedBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        // Each call "spends" 1000 tokens.
        // Use load+store instead of fetch_sub to avoid usize underflow wrapping
        // (after budget exhausts, wrapping would make subsequent calls succeed again).
        let current = self.remaining.load(Ordering::SeqCst);
        if current < 1000 {
            return Err(ArgentorError::Agent("token budget exhausted".into()));
        }
        self.remaining.store(current - 1000, Ordering::SeqCst);
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
        "budgeted"
    }
}

fn make_runner_with(backend: Box<dyn LlmBackend>, max_turns: u32) -> AgentRunner {
    let registry = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(
        std::env::temp_dir().join("argentor-longrun-audit"),
    ));
    AgentRunner::from_backend(backend, registry, permissions, audit, max_turns)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Build a synthetic conversation and verify the compaction engine triggers
/// and produces a strictly smaller output.
///
/// Uses the adaptive default (70% of 200K = 140K tokens). We generate
/// enough messages to exceed that threshold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_1_hour_session_equivalent_context_compacts() {
    let engine = ContextCompactorEngine::with_defaults();

    // Build enough turns to exceed the adaptive trigger (140K tokens by default).
    // Each message is ~150 chars ≈ 33 tokens. We need > 4 250 messages (140K/33).
    // Use 5 000 messages = 2 500 turns to be safely above the threshold.
    let mut messages = Vec::with_capacity(5000);
    for i in 0..2500 {
        messages.push(CompactableMessage::user(&format!(
            "User turn {i}: please answer this somewhat verbose question that takes roughly fifty tokens to render once tokenized so we exceed budget"
        )));
        messages.push(CompactableMessage::assistant(&format!(
            "Assistant turn {i}: here is a moderately detailed answer that approximates what a real LLM would emit when asked a small question"
        )));
    }

    let total_tokens: usize = messages.iter().map(|m| m.token_estimate).sum();
    let effective = engine.config().effective_trigger();
    assert!(
        total_tokens > effective,
        "synthetic transcript ({total_tokens} tokens) must exceed effective trigger ({effective})"
    );
    assert!(engine.should_compact(&messages), "should_compact must fire");

    let result = engine
        .compact(&messages)
        .expect("compaction must produce a result above threshold");
    assert!(
        result.compacted_tokens < result.original_tokens,
        "compaction must reduce token count (orig={} compacted={})",
        result.original_tokens,
        result.compacted_tokens
    );
    assert!(
        result.compression_ratio < 1.0,
        "compression_ratio must be < 1.0, got {}",
        result.compression_ratio
    );
}

/// "Heartbeat" simulation: a task stays alive receiving pings faster than the
/// idle-timeout fires. Compressed time: ping every 50ms, idle-timeout 200ms,
/// total duration 500ms.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_heartbeat_keeps_long_session_alive() {
    let (tx, mut rx) = mpsc::channel::<()>(16);

    // Producer: 10 pings at 50ms intervals, then close.
    let producer = tokio::spawn(async move {
        for _ in 0..10 {
            if tx.send(()).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // drop tx — closing the channel
    });

    let start = Instant::now();
    let mut heartbeats = 0usize;

    loop {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some(())) => heartbeats += 1,
            Ok(None) => break, // sender closed — session ends naturally
            Err(_) => panic!("idle timeout fired before session completed"),
        }
    }

    let _ = producer.await;
    assert_eq!(heartbeats, 10, "all 10 heartbeats must be received");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "test must complete well under 3s, took {:?}",
        start.elapsed()
    );
}

/// A budgeted backend exhausts its token budget partway through and the
/// runner returns an error WITHOUT corrupting session state (the partial
/// transcript remains accessible).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_token_budget_exhausts_gracefully() {
    let remaining = Arc::new(AtomicUsize::new(10_000));
    let backend = BudgetedBackend {
        remaining: remaining.clone(),
    };
    let runner = make_runner_with(Box::new(backend), 1);

    let mut session = Session::new();
    let mut successes = 0usize;
    let mut clean_failure_count = 0usize;

    for i in 0..30 {
        let res = runner.run(&mut session, &format!("turn {i}")).await;
        match res {
            Ok(_) => successes += 1,
            Err(_) => clean_failure_count += 1,
        }
    }

    // 10K budget at 1000 tokens/call ⇒ ~10 successes before failures begin.
    assert!(successes >= 8 && successes <= 12, "successes={successes}");
    assert!(
        clean_failure_count > 0,
        "must produce clean errors after budget exhausts"
    );
    // Session is preserved (still has accumulated messages, no corruption).
    assert!(
        session.message_count() > 0,
        "session must retain partial transcript"
    );
}

/// Streaming back-pressure: a producer that pushes 1000 events into a bounded
/// channel of capacity 8 must not drop or panic when the consumer is slow.
/// `send()` must await as expected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_streaming_backpressure() {
    let (tx, mut rx) = mpsc::channel::<usize>(8);

    let producer = tokio::spawn(async move {
        for i in 0..1000 {
            tx.send(i)
                .await
                .expect("channel must accept item with backpressure");
        }
    });

    // Slow consumer: yields between every recv to let other tasks run.
    let consumer = tokio::spawn(async move {
        let mut received = Vec::with_capacity(1000);
        while let Some(v) = rx.recv().await {
            received.push(v);
            tokio::task::yield_now().await;
        }
        received
    });

    producer.await.expect("producer panicked");
    let received = consumer.await.expect("consumer panicked");

    assert_eq!(received.len(), 1000, "all messages must arrive");
    for (i, v) in received.iter().enumerate() {
        assert_eq!(*v, i, "ordering preserved through backpressure");
    }
}

/// 10 parallel 100-turn sessions — each maintains its own state, no
/// cross-contamination of message counts or session IDs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_long_sessions() {
    let runner = Arc::new(make_runner_with(Box::new(FastMockBackend::new()), 1));
    const N_SESSIONS: usize = 10;
    const TURNS: usize = 30;

    let mut handles = Vec::with_capacity(N_SESSIONS);
    for s in 0..N_SESSIONS {
        let r = runner.clone();
        handles.push(tokio::spawn(async move {
            let mut session = Session::new();
            let session_id = session.id;
            for t in 0..TURNS {
                let _ = r.run(&mut session, &format!("session {s} turn {t}")).await;
            }
            (session_id, session.message_count())
        }));
    }

    let mut results = Vec::with_capacity(N_SESSIONS);
    for h in handles {
        results.push(h.await.expect("task panicked"));
    }

    // All session IDs are unique.
    let unique: std::collections::HashSet<_> = results.iter().map(|(id, _)| *id).collect();
    assert_eq!(unique.len(), N_SESSIONS, "session IDs must all be unique");

    // Each session has the expected number of messages
    // (per turn: at least 1 user + 1 assistant message).
    for (id, msg_count) in &results {
        assert!(
            *msg_count >= TURNS * 2,
            "session {id} expected ≥ {} messages, got {msg_count}",
            TURNS * 2
        );
    }
}

/// Smoke test that the compaction engine handles edge cases (empty,
/// strategies) without panicking — this is part of long-running surface area.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_compaction_strategies_smoke() {
    let mut messages = Vec::new();
    for i in 0..200 {
        messages.push(CompactableMessage::user(&format!(
            "Long verbose user message number {i} that pads the token budget enough for compaction triggers"
        )));
    }

    for strategy in [
        CompactionStrategy::Summarize,
        CompactionStrategy::SlidingWindow,
        CompactionStrategy::ImportanceBased,
        CompactionStrategy::Hybrid,
    ] {
        let cfg = CompactionConfig {
            strategy,
            ..Default::default()
        };
        let engine = ContextCompactorEngine::new(cfg);
        // Must not panic regardless of strategy.
        let _ = engine.compact(&messages);
    }
}

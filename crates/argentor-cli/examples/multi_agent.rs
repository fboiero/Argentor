#![allow(clippy::expect_used)]

//! Multi-agent orchestration with the Orchestrator-Workers pattern.
//!
//! Spins up an Orchestrator that decomposes a task and dispatches it to
//! specialized worker agents (Spec, Coder, Tester, Reviewer). All workers
//! use a mock backend — no API key required.
//!
//! ```bash
//! cargo run --example multi_agent
//! ```

use argentor_agent::llm::LlmResponse;
use argentor_agent::{LlmBackend, LlmProvider, MockBenchmarkBackend, ModelConfig};
use argentor_orchestrator::{BackendFactory, Orchestrator};
use argentor_security::{AuditLog, PermissionSet};
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // 1. Shared skill registry (empty for this demo — workers just use the mock LLM).
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-multi-audit")));

    // 2. A dummy ModelConfig — the factory below overrides the actual backend.
    let base_config = ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "mock".into(),
        api_key: "mock".into(),
        api_base_url: None,
        temperature: 0.7,
        max_tokens: 4096,
        max_turns: 3,
        max_context_tokens: 200_000,
        fallback_models: vec![],
        retry_policy: None,
    };

    // 3. Backend factory: every worker agent gets a mock backend that returns a
    //    Done response so the agentic loop terminates after one turn.
    let factory: BackendFactory = Arc::new(|role| {
        let reply = format!("[{role}] Task complete. Deliverable ready.");
        Box::new(MockBenchmarkBackend::new(vec![LlmResponse::Done(reply)])) as Box<dyn LlmBackend>
    });

    // 4. Build the orchestrator with the mock factory.
    let orchestrator = Orchestrator::new(&base_config, skills, permissions, audit)
        .with_backend_factory(factory)
        .with_progress(|role, msg| println!("[{role}] {msg}"));

    // 5. Run the full plan → execute → synthesize pipeline.
    println!("Starting multi-agent pipeline...");
    let result = orchestrator
        .run("Implement a Fibonacci function in Rust with unit tests")
        .await
        .expect("orchestration failed");

    println!(
        "\nPipeline complete! Artifacts produced: {}",
        result.artifacts.len()
    );
    for artifact in &result.artifacts {
        println!("  - [{:?}] {} chars", artifact.kind, artifact.content.len());
    }
}

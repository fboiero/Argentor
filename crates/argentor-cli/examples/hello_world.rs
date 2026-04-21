//! Simplest possible Argentor agent: mock backend, one prompt, one response.
//!
//! No API key required.
//!
//! ```bash
//! cargo run --example hello_world
//! ```

use argentor_agent::llm::LlmResponse;
use argentor_agent::{AgentRunner, MockBenchmarkBackend};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // 1. Build a skill registry (empty — no tools needed for a plain Q&A).
    let registry = Arc::new(SkillRegistry::new());

    // 2. Minimal permission set and audit log (writes to /tmp).
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/argentor-hello-audit")));

    // 3. A mock backend that returns a Done response — signals the loop to stop.
    let backend = Box::new(MockBenchmarkBackend::new(vec![LlmResponse::Done(
        "Hello from Argentor! I am a secure, WASM-sandboxed AI agent framework built in Rust."
            .into(),
    )]));

    // 4. Create the agent runner from the mock backend.
    let agent = AgentRunner::from_backend(backend, registry, permissions, audit, 3);

    // 5. Run — create a session, send a prompt, get the response.
    let mut session = Session::new();
    let response = agent
        .run(&mut session, "Hello! Who are you?")
        .await
        .expect("agent run failed");

    println!("Agent response: {response}");
}

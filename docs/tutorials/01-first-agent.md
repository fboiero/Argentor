# Tutorial 1: Build Your First Agent in 5 Minutes

> Build a working Argentor agent from scratch using `AgentRunner`, send it a message, and see the response.

This tutorial walks you through setting up a brand-new Cargo project that uses Argentor as a library, configuring a Claude-backed agent, and running your first conversation. By the end you will have a working Rust binary you can iterate on.

---

## Prerequisites

- Rust **1.80+** installed via [rustup](https://rustup.rs)
- A **DeepSeek API key** exported as `DEEPSEEK_API_KEY` (or another supported provider key if you prefer)
- Basic familiarity with `cargo` (new, add, run)

If you do not have an API key, you can still follow along with a mocked backend — see the "Running without API keys" section at the end.

---

## 1. Create the Cargo Project

Start from an empty directory:

```bash
cargo new my-first-agent --bin
cd my-first-agent
```

Cargo creates `Cargo.toml` and a default `src/main.rs` printing `Hello, world!`. We will replace the contents of both.

---

## 2. Add Argentor Dependencies

Open `Cargo.toml` and add the crates we need:

```toml
[package]
name = "my-first-agent"
version = "0.1.0"
edition = "2021"

[dependencies]
argentor-core = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-agent = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-skills = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-security = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-session = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-builtins = { git = "https://github.com/fboiero/Agentor", branch = "master" }

# Runtime + error handling
tokio = { version = "1", features = ["full"] }
anyhow = "1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

> When Argentor is published to crates.io you will be able to replace the git references with plain version numbers. Until then, pinning a branch keeps things deterministic.

---

## 3. Configure the Model

Argentor uses `ModelConfig` to describe which LLM backend, model id, and sampling parameters to use. The provider enum supports 14 LLM backends (Claude, OpenAI, Gemini, Groq, Ollama, Mistral, xAI, Azure OpenAI, Cerebras, Together, DeepSeek, vLLM, OpenRouter, ClaudeCode).

Replace `src/main.rs` with:

```rust
use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_builtins::register_builtins;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing so we see what the agent is doing.
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    // 1. Configure the model
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .map_err(|_| anyhow::anyhow!("Set DEEPSEEK_API_KEY before running"))?;

    let config = ModelConfig {
        provider: LlmProvider::DeepSeek,
        model_id: "deepseek-chat".into(),
        api_key,
        api_base_url: None,
        temperature: 0.7,
        max_tokens: 2048,
        max_turns: 5,
        fallback_models: vec![],
        retry_policy: None,
    };

    // 2. Create an empty skill registry (we'll add skills in the next tutorial)
    let mut registry = SkillRegistry::new();
    register_builtins(&mut registry);
    let skills = Arc::new(registry);

    // 3. Set up permissions and audit log (required by AgentRunner)
    let permissions = PermissionSet::new();
    let audit = Arc::new(AuditLog::new(PathBuf::from("./audit-logs")));

    // 4. Build the agent
    let runner = AgentRunner::new(config, skills, permissions, audit);

    // 5. Create a conversation session and run the agent
    let mut session = Session::new();
    let response = runner
        .run(&mut session, "Hi! Explain in one sentence what you can do.")
        .await?;

    println!("\n─── Agent response ───\n{response}\n");

    Ok(())
}
```

### What is happening here

1. **`ModelConfig`** — tells Argentor which LLM to call and how.
2. **`SkillRegistry` + `register_builtins`** — registers ~50 built-in tools (file read/write, shell, HTTP, calculator, etc.). The agent sees them as "tools" and decides which to use.
3. **`PermissionSet`** — empty for now, so the agent cannot actually execute tools that need capabilities. It can still respond with text.
4. **`AuditLog`** — append-only JSONL log. Every tool call and decision gets written here.
5. **`Session`** — tracks conversation history across turns.
6. **`AgentRunner::run`** — drives the agentic loop: prompt → LLM → optional tool calls → response.

---

## 4. Run It

```bash
export DEEPSEEK_API_KEY="sk-..."
cargo run
```

### Expected output

```
 INFO my_first_agent: starting agent run
 INFO argentor_agent::runner: AgentRunner: starting loop session_id=...

─── Agent response ───
I'm an AI assistant powered by Argentor that can answer questions, reason about
problems, and (when granted permissions) execute sandboxed tools like file I/O,
shell commands, web search, and vector memory lookups.
```

On the first run the `audit-logs/` directory is created and a JSONL file is written with entries like:

```json
{"timestamp":"2026-04-11T10:22:15Z","action":"agent_start","outcome":"success",...}
{"timestamp":"2026-04-11T10:22:16Z","action":"llm_request","outcome":"success",...}
{"timestamp":"2026-04-11T10:22:18Z","action":"agent_done","outcome":"success",...}
```

---

## 5. Try Different Providers

Swap `LlmProvider::DeepSeek` for another backend to see how fast the runtime lets you retarget:

```rust
// OpenAI
let config = ModelConfig {
    provider: LlmProvider::OpenAi,
    model_id: "gpt-4o".into(),
    api_key: std::env::var("OPENAI_API_KEY")?,
    ..Default::default()
};

// Gemini
let config = ModelConfig {
    provider: LlmProvider::Gemini,
    model_id: "gemini-2.0-flash".into(),
    api_key: std::env::var("GEMINI_API_KEY")?,
    ..Default::default()
};

// Local Ollama (no API key needed)
let config = ModelConfig {
    provider: LlmProvider::Ollama,
    model_id: "llama3.3:70b".into(),
    api_key: String::new(),
    ..Default::default()
};
```

`ModelConfig` does not actually derive `Default` today — you must fill every field — but the pattern above reads nicely if you ever add a `Default` impl.

---

## 6. Use the `query()` Convenience API

For quick scripts, `argentor_agent::query::query()` is a thinner wrapper:

```rust
use argentor_agent::query::{query, QueryEvent, QueryOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")?;
    let options = QueryOptions::deepseek(api_key)
        .system_prompt("You are a concise Rust tutor.")
        .max_turns(3);

    let mut events = query("Explain the borrow checker in 3 sentences.", options).await?;
    while let Some(event) = events.recv().await {
        match event {
            QueryEvent::Text { text } => print!("{text}"),
            QueryEvent::Done { output, turns, .. } => {
                println!("\n\n[done in {turns} turns]\nFinal: {output}");
            }
            QueryEvent::Error { message } => eprintln!("ERROR: {message}"),
            _ => {}
        }
    }
    Ok(())
}
```

The `QueryOptions::deepseek` / `claude` / `openai` / `gemini` / `ollama` / ... constructors cover all providers. See `crates/argentor-agent/src/query.rs` for the full list.

---

## Running Without API Keys

If you want to try Argentor without paying for an LLM call, the repo ships a `DemoBackend` pattern in `crates/argentor-cli/examples/demo_pipeline.rs`. It is a scripted `LlmBackend` you hand an ordered list of `LlmResponse` values. `AgentRunner::from_backend(Box::new(DemoBackend::new(...)))` lets you swap in the mock without touching anything else in your code.

```bash
# Run the shipped demo — no API keys required
cargo run -p argentor-cli --example demo_pipeline
```

---

## Common Issues

**"Provider Claude has no API key configured"**
Your `ModelConfig` has an empty `api_key`. Export the env var and rebuild, or pass a literal string (never check real keys into git).

**`error: failed to resolve: use of undeclared crate or module argentor_agent`**
Make sure every crate listed in `Cargo.toml` actually appears in the `[dependencies]` section, and run `cargo check` to resolve the new deps.

**"Failed to create audit-logs/audit.jsonl"**
The path is resolved relative to the current working directory. Either `cargo run` from the project root, or pass an absolute path to `AuditLog::new(...)`.

**`max_turns is 0` warning from `validate_config()`**
You passed `0` for `max_turns`. The agent will bail out immediately. Use at least `3`-`5` for a real conversation.

**Rate-limit / 429 errors**
Wrap `ModelConfig` with a `RetryPolicy`:

```rust
use argentor_agent::RetryPolicy;

let config = ModelConfig {
    retry_policy: Some(RetryPolicy::default()),
    ..
};
```

---

## Understanding the Agentic Loop

What you just built executes the classic agentic loop under the hood:

```
┌───────────────┐
│  User Input   │
└───────┬───────┘
        ▼
┌───────────────┐     ┌─────────────────┐
│  LLM Request  │────▶│   LLM Response  │
└───────┬───────┘     └─────────┬───────┘
        │                       │
        │         ┌─────────────┴───────────┐
        │         │                         │
        │     text only               tool_use
        │         │                         │
        │         ▼                         ▼
        │  ┌─────────────┐         ┌────────────────┐
        │  │  Done       │         │ Execute Skill  │
        │  │ return text │         │ (with permcheck│
        │  └─────────────┘         │  + audit log)  │
        │                          └────────┬───────┘
        │                                   │
        │                        ┌──────────▼──────────┐
        │                        │ Append tool result  │
        │                        │ to conversation     │
        │                        └──────────┬──────────┘
        │                                   │
        └───────────────────────────────────┘
                  loop until Done or max_turns
```

At every turn Argentor:

1. Sends the conversation history + the tool catalog to the LLM.
2. Parses the response — either final text or one/more `tool_use` blocks.
3. For each tool call: validates args against the JSON schema, checks `PermissionSet`, executes the skill (sandboxed if WASM), writes an audit entry.
4. Appends the tool result back to the conversation so the LLM can use it next turn.
5. Repeats until the LLM answers with no tool calls, or `max_turns` is hit.

`max_turns` is your safety net against infinite loops. Start at 5, bump to 10-20 for complex tasks.

---

## What You Built

You now have:

- A Cargo project that depends on Argentor
- A working `AgentRunner` wired to Claude (or any of 14 providers)
- An auditable, session-backed conversation loop
- The `query()` shorthand for one-off scripts
- An understanding of the agentic loop's shape

In the next tutorial we will unlock the agent's real power — tool use.

---

## Next Steps

- **[Tutorial 2: Using Skills](./02-using-skills.md)** — give the agent file I/O, shell, and web search.
- **[Tutorial 6: Guardrails & Security](./06-guardrails-security.md)** — add PII detection and prompt-injection blocking before going to production.
- **[Tutorial 7: Agent Intelligence](./07-agent-intelligence.md)** — enable extended thinking and self-critique.

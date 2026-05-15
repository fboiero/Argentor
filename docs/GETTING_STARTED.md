# Getting Started with Argentor

This guide walks you through installing Argentor, running your first agent, and exploring the key features. You'll be up and running in under 5 minutes.

---

## Prerequisites

- **Rust 1.80+** ([install](https://rustup.rs))
- An LLM API key (Claude, OpenAI, or Gemini) for live agent runs, OR use the demos which need no API keys

---

## 1. Install from Source

```bash
git clone https://github.com/fboiero/Argentor.git
cd Argentor
cargo build --release
```

The binary lands at `target/release/argentor`.

### Install from crates.io (coming soon)

```bash
cargo install argentor-cli
```

---

## 2. Run the Demo (No API Keys)

The fastest way to see Argentor in action:

```bash
cargo run -p argentor-cli --example demo_full_pipeline
```

This runs an 8-step pipeline with real tool execution: shell commands, file I/O, vector memory, and report generation. No mocks, no API keys.

### More demos

```bash
# DevOps team simulation (4 specialized agents)
cargo run -p argentor-cli --example demo_team

# Skills toolkit showcase (18 utility skills)
cargo run -p argentor-cli --example demo_skills_toolkit

# Multi-agent SaaS factory
cargo run -p argentor-cli --example demo_saas_factory

# Security challenge (penetration testing agent)
cargo run -p argentor-cli --example demo_security_challenge
```

---

## 3. Start the Gateway

Launch the HTTP/WebSocket gateway with the dashboard:

```bash
# Set your LLM provider key
export ANTHROPIC_API_KEY="sk-ant-..."
# Or: export OPENAI_API_KEY="sk-..."
# Or: export GEMINI_API_KEY="..."

# Start the server
cargo run -p argentor-cli -- serve --bind 0.0.0.0:8080
```

Open your browser:

| URL | What it is |
|-----|-----------|
| `http://localhost:8080/dashboard` | Control plane dashboard |
| `http://localhost:8080/dashboard/audit` | Audit log explorer with filters and CSV/JSON export |
| `http://localhost:8080/playground` | Interactive chat playground |
| `http://localhost:8080/health` | Health check |
| `http://localhost:8080/openapi.json` | OpenAPI 3.0 specification |
| `http://localhost:8080/metrics` | Prometheus metrics (includes the `argentor_audit_*` family) |
| `http://localhost:8080/api/v1/audit/logs` | REST audit log endpoint with cursor pagination |
| `http://localhost:8080/api/v1/audit/stats` | Aggregate audit counters |

### Optional: audit storage configuration

Add an `[audit]` section to `argentor.toml` to opt into rotation, retention,
and compression for the audit log. Without this block the gateway uses the
defaults (no rotation, JSONL only).

```toml
[audit]
# path = "./data/audit/audit.jsonl"   # explicit path, otherwise <data_dir>/audit/audit.jsonl
max_size_mb = 10                       # rotate the active log past this size
max_rotated_files = 5                  # keep the N most recent numeric-suffix rotations
# retention_days = 30                  # also delete rotations older than this many days
compress_rotated = false               # set true to archive rotated files as <base>.N.zst
```

See [DEPLOYMENT.md](DEPLOYMENT.md) for the full audit lifecycle and the
Prometheus metric reference.

---

## 4. Chat with an Agent (REST API)

### Synchronous

```bash
curl -X POST http://localhost:8080/api/v1/agent/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "What files are in the current directory?"}'
```

### Streaming (SSE)

```bash
curl -N -X POST http://localhost:8080/api/v1/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"message": "Write a haiku about Rust"}'
```

---

## 5. Use the Python SDK

```bash
pip install argentor-sdk
```

```python
from argentor import ArgentorClient

client = ArgentorClient("http://localhost:8080")

# Simple chat
response = client.run_task("Summarize the README.md file")
print(response.output)

# List available skills
skills = client.list_skills()
for skill in skills:
    print(f"  {skill.name}: {skill.description}")
```

### Async usage

```python
import asyncio
from argentor import AsyncArgentorClient

async def main():
    client = AsyncArgentorClient("http://localhost:8080")
    
    # Stream responses
    async for event in client.run_task_stream("Explain WASM sandboxing"):
        if event.type == "text":
            print(event.data, end="", flush=True)

asyncio.run(main())
```

---

## 6. Use the TypeScript SDK

```bash
npm install @argentor/sdk
```

```typescript
import { ArgentorClient } from '@argentor/sdk';

const client = new ArgentorClient('http://localhost:8080');

// Simple chat
const response = await client.runTask('List all skills');
console.log(response.output);

// Stream responses
const stream = client.runTaskStream('Write a function to sort an array');
for await (const event of stream) {
  if (event.type === 'text') {
    process.stdout.write(event.data);
  }
}
```

---

## 7. Use as a Rust Library

Add the crates you need to your `Cargo.toml`:

```toml
[dependencies]
argentor-core = "1.0"
argentor-agent = "1.0"
argentor-skills = "1.0"
argentor-builtins = "1.0"
```

### Minimal agent example

```rust
use argentor_agent::{AgentRunner, ModelConfig, LlmProvider};
use argentor_skills::SkillRegistry;
use argentor_builtins::register_builtins;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Set up skills
    let mut registry = SkillRegistry::new();
    register_builtins(&mut registry);

    // Configure LLM
    let config = ModelConfig::new(LlmProvider::Claude)
        .with_model("claude-sonnet-4-20250514")
        .with_max_tokens(4096);

    // Create and run agent
    let mut runner = AgentRunner::new(config, registry);
    let response = runner.run("What tools do you have available?").await?;
    println!("{}", response.content);

    Ok(())
}
```

### With guardrails

```rust
use argentor_agent::GuardrailEngine;

let guardrails = GuardrailEngine::default(); // PII + injection + toxicity

let mut runner = AgentRunner::new(config, registry)
    .with_guardrails(guardrails)
    .with_cache(1000, std::time::Duration::from_secs(300));

let response = runner.run("Process this customer data").await?;
// Input/output automatically scanned for PII, injection attempts, etc.
```

---

## 8. Docker

### Quick start

```bash
docker run -d \
  --name argentor \
  -p 8080:8080 \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  ghcr.io/fboiero/argentor:latest serve
```

### Production (with Prometheus + Grafana)

```bash
docker compose -f docker-compose.production.yml up -d
```

See [DEPLOYMENT.md](./DEPLOYMENT.md) for Kubernetes, Helm, and multi-region setups.

---

## 9. Key Concepts

### Skills

Skills are tools the agent can use. Argentor has 50+ built-in skills (files, shell, git, web search, crypto, etc.) plus a WASM plugin system for custom skills.

```bash
# List skills via CLI
cargo run -p argentor-cli -- skill list
```

### Guardrails

Pre/post-execution filters that scan for PII, prompt injection, and policy violations. Integrated directly into the agent loop.

### Multi-Agent Orchestration

Run teams of agents with specialized roles:

```rust
use argentor_orchestrator::{Orchestrator, OrchestratorConfig};

let config = OrchestratorConfig::default();
let orchestrator = Orchestrator::new(config);
orchestrator.run_pipeline("Build a REST API for a todo app").await?;
```

### MCP Integration

Argentor can act as both MCP client (connect to MCP servers) and MCP server (expose skills as MCP tools):

```bash
# Start as MCP server
cargo run -p argentor-cli -- mcp serve
```

---

## 10. Project Structure

```
crates/
  argentor-core          Core types, errors, event bus
  argentor-security      Permissions, RBAC, audit, crypto
  argentor-session       Session management, conversation history
  argentor-skills        Skill trait, WASM runtime, marketplace
  argentor-agent         AgentRunner, 14 LLM backends, guardrails
  argentor-channels      Channel trait (Slack, WebChat)
  argentor-gateway       HTTP gateway, REST API, dashboard
  argentor-builtins      50+ built-in skills
  argentor-memory        Vector store, RAG, embeddings
  argentor-mcp           MCP client/server, credential vault
  argentor-orchestrator  Multi-agent engine, workflows
  argentor-compliance    GDPR, ISO 27001, ISO 42001
  argentor-a2a           Google A2A protocol
  argentor-cli           CLI binary + demos
```

---

## What's Next?

- [DEPLOYMENT.md](./DEPLOYMENT.md) — Production deployment guide (Docker, K8s, Helm)
- [TECHNICAL_REPORT.md](./TECHNICAL_REPORT.md) — Architecture deep dive
- [OpenAPI Spec](http://localhost:8080/openapi.json) — Full API reference (when server is running)
- [GitHub Issues](https://github.com/fboiero/Argentor/issues) — Report bugs or request features

---

## Next: Tutorials

Ten hands-on tutorials take you from "empty directory" to "production-grade multi-agent system". Each one is self-contained, uses real Argentor APIs, and shows expected output.

- [Tutorial 1: First Agent](./tutorials/01-first-agent.md) — Build your first agent in 5 minutes
- [Tutorial 2: Using Skills](./tutorials/02-using-skills.md) — Calculator, file reader, web search, capabilities
- [Tutorial 3: Multi-Agent Orchestration](./tutorials/03-multi-agent-orchestration.md) — Spec/Coder/Tester/Reviewer teams
- [Tutorial 4: RAG Pipeline](./tutorials/04-rag-pipeline.md) — Vector stores, embeddings, hybrid search
- [Tutorial 5: Custom Skills](./tutorials/05-custom-skills.md) — `ToolBuilder`, `Skill` trait, WASM plugins
- [Tutorial 6: Guardrails & Security](./tutorials/06-guardrails-security.md) — PII, prompt injection, sanitization, audit
- [Tutorial 7: Agent Intelligence](./tutorials/07-agent-intelligence.md) — Thinking, critique, compaction, discovery
- [Tutorial 8: MCP Integration](./tutorials/08-mcp-integration.md) — Client, server, proxy, credential vault
- [Tutorial 9: Production Deployment](./tutorials/09-deployment.md) — Docker, Kubernetes, Helm, observability
- [Tutorial 10: Observability](./tutorials/10-observability.md) — OpenTelemetry, traces, metrics, alerts

See the [tutorials index](./tutorials/README.md) for recommended learning paths.

---

## How-To Guides

Focused, task-oriented guides you can follow independently:

- [Build a RAG Agent](./tutorials/build-rag-agent.md) — Load documents, embed, query with semantic search, feed results to an agent
- [Deploy to the Cloud](./tutorials/deploy-to-cloud.md) — Docker build, local Compose dev stack, AWS ECS/Fargate, GCP Cloud Run, Azure Container Apps
- [Build Custom Skills](./tutorials/custom-skills.md) — `ToolBuilder`, `Skill` trait, WASM sandboxed plugins, Markdown prompt-skills
- [Multi-Agent Patterns](./tutorials/multi-agent-patterns.md) — Orchestrator-Workers, Pipeline, Fan-out/Fan-in, Human-in-the-loop

---

## Troubleshooting

Stuck? Check [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) for solutions to the most common problems:

- "Connection refused" — gateway not running
- "API key not found" — env var setup
- Rate limiting — configure retry policy and rate limits
- WASM skill failures — memory limits and pointer protocol
- Guardrail blocking legitimate content — custom rules and allowlists

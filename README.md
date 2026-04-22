# Argentor

**Secure, high-performance multi-agent AI framework in Rust — WASM sandboxed plugins, MCP native, compliance-ready.**

[![CI](https://github.com/fboiero/Argentor/actions/workflows/ci.yml/badge.svg)](https://github.com/fboiero/Argentor/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0--only-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![Crates](https://img.shields.io/badge/Crates-17-informational.svg)]()
[![PyPI](https://img.shields.io/badge/PyPI-argentor-blue.svg)](https://pypi.org/project/argentor/)

---

## Quick Start

```bash
git clone https://github.com/fboiero/Argentor.git
cd Argentor
cargo run --example hello_world
```

---

## Why Argentor?

- **Security-first** — WASM sandbox + capability-based permissions on every skill; shell injection, null-byte, URL-decode, and NFKC path attacks blocked in the guardrails pipeline
- **Rust performance** — ~2 ms framework overhead per turn vs 11–55 ms for Python-based competitors (N=10 paired, p < 0.0001)
- **WASM sandboxing** — plugins run in wasmtime/WASI isolation; ed25519 signature verification and static analysis before load
- **MCP native** — full client/server/proxy with centralized control plane, credential vault, and token pool

---

## Benchmarks

Argentor vs LangChain, CrewAI, PydanticAI, and Claude SDK across 6 dimensions (reproducible from `benchmarks/`):

| Metric | Argentor | Best Competitor | Advantage |
|--------|----------|-----------------|-----------|
| Framework latency | ~2 ms | 11–55 ms | **~11x faster** |
| Token cost (50-tool workload) | baseline | 7.9x more | **7.9x cheaper** |
| Adversarial prompt blocking | 58.3% | 0% | **+58 pp security** |

Full report: [docs/BENCHMARK_SYNTHESIS.md](docs/BENCHMARK_SYNTHESIS.md) — includes sensitivity analysis and honest losses.

---

## Architecture

17 crates, strict workspace lints (`unwrap_used`, `expect_used` → warn in CI):

| Crate | Description |
|-------|-------------|
| `argentor-core` | Core types, errors, event bus, metrics export, correlation context |
| `argentor-security` | Capabilities, RBAC, rate limiting, audit log (rotating, background writer), TLS/mTLS, JWT, encrypted store, alerts, SLA tracking |
| `argentor-session` | Session management, `FileSessionStore`, persistence |
| `argentor-skills` | Skill trait, `SkillRegistry` (concurrent), WASM sandbox runtime, vetting pipeline, marketplace download/install/cache |
| `argentor-agent` | AgentRunner, 14 LLM backends, Bedrock (SigV4, feature-gated), failover, streaming, circuit breaker, response cache (concurrent LRU+TTL), token budget, adaptive context compaction |
| `argentor-channels` | Slack, Discord, Telegram, Webchat adapters |
| `argentor-gateway` | HTTP/WebSocket gateway, auth, webhooks, Prometheus metrics, observability dashboard (HTML/JS, no deps), OpenAPI |
| `argentor-builtins` | shell, file I/O, HTTP, memory, browser, Docker, code generation, 50+ universal skills |
| `argentor-memory` | Vector memory, hybrid search (BM25 + embeddings), query expansion, JSONL persistence |
| `argentor-mcp` | MCP client/server/proxy, proxy orchestrator, credential vault, token pool |
| `argentor-orchestrator` | Multi-agent engine, TaskQueue with DAG, AgentMonitor, DeploymentManager |
| `argentor-compliance` | GDPR, ISO 27001, ISO 42001, DPGA compliance modules |
| `argentor-a2a` | Google A2A protocol: AgentCard, A2AServer, A2AClient, JSON-RPC 2.0 |
| `argentor-tee` | TEE provider stubs (AWS Nitro, Intel SGX, AMD SEV-SNP), attestation verifier |
| `argentor-cloud` | Multi-tenant managed runtime, TenantManager, QuotaEnforcer, 4-tier billing |
| `argentor-python` | PyO3 Rust-to-Python bindings (maturin build, excluded from workspace tests) |
| `argentor-cli` | CLI binary (`serve`, `deploy`, `agents`, `health`, `skill list`) |

---

## Features

### Security
- WASM sandboxed plugins (wasmtime + WASI) — capability-based permissions per skill
- Guardrails: shell injection, base64-decode attacks, unicode normalization (NFKC), null-byte, URL-decode path attacks
- SSRF prevention — blocks localhost, link-local, and private ranges
- Path traversal protection — canonicalization + blocklist + null-byte detection
- PII detection (Luhn, SSN, email, phone) with redaction
- Prompt injection blocking — 23+ pattern signatures
- RBAC policy engine, JWT/mTLS, encrypted credential store (AES-256-GCM)
- Audit log rotation with background writer thread

### Intelligence
- Token budget per session + adaptive context compaction (4 strategies)
- Extended Thinking Mode (Quick/Standard/Deep/Exhaustive) with confidence scoring
- Self-Critique Loop (Reflexion pattern across 6 quality dimensions)
- Dynamic Tool Discovery (TF-IDF + keyword hybrid, ~98% token reduction)
- Learning Feedback Loop — tool selector improves from execution outcomes
- 6-phase competitive benchmark suite (vs LangChain, CrewAI, PydanticAI, Claude SDK)

### Developer Experience
- 5 runnable examples (see `examples/`)
- Workflow DSL — TOML-based agent workflows, no Rust code required
- Tool Builder — 3-line tool definitions
- Config hot-reload via file watcher (500ms debounce)
- CLI REPL with 12 commands for interactive agent debugging
- Observability dashboard at `/dashboard` — pure HTML/JS, no build step

### Integrations
- 14 LLM providers: Claude, OpenAI, Gemini, OpenRouter, Groq, Ollama, Mistral, xAI, Azure OpenAI, Cerebras, Together, DeepSeek, Cohere, HuggingFace
- AWS Bedrock backend — real SigV4 signing, feature-gated (`bedrock` feature flag)
- 5,800+ tool integrations via MCP ecosystem
- Channel adapters: Slack, Discord, Telegram, Webchat
- Google A2A interoperability protocol
- Python SDK (pip-installable) + TypeScript SDK (npm)

---

## Getting Started

### Prerequisites

- Rust 1.80+ (`rustup update stable`)
- An API key from any supported LLM provider

### Run Examples

```bash
# Hello world agent
cargo run --example hello_world

# Full DevOps pipeline demo (no API key needed)
cargo run -p argentor-cli --example demo_pipeline

# See all examples
ls examples/
```

### Configure

```toml
# argentor.toml
[model]
provider = "claude"
model_id = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"
max_tokens = 4096
max_turns = 20

[server]
host = "0.0.0.0"
port = 3000
```

### Run the Server

```bash
cargo run --bin argentor -- serve
cargo run --bin argentor -- skill list
```

### Run Tests

```bash
cargo test -p argentor-core -p argentor-agent -p argentor-security
cargo test --workspace
cargo clippy --workspace   # 0 warnings
cargo fmt --all -- --check
```

---

## Python SDK

```bash
pip install -e python/
```

```python
from argentor import ArgentorClient

client = ArgentorClient(base_url="http://localhost:3000")
result = client.run_task("my_skill", "input text here")
print(result["response"])
```

Async support and 24 typed models included. For native Rust bindings: `pip install maturin && cd crates/argentor-python && maturin develop`.

---

## Observability Dashboard

```bash
cargo run --bin argentor -- serve
# Open http://localhost:3000/dashboard
```

Pure HTML/JS — no build step, dark-themed SPA with deployment management, agent catalog, and health monitoring.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines, and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

Bug reports, feature requests, and pull requests are welcome. For security issues, see [SECURITY.md](SECURITY.md).

---

## License

**GNU Affero General Public License v3.0 only** — see [LICENSE](LICENSE).

AGPL-3.0-only applies to the entire Argentor family. No MIT, no Apache dual-licensing.

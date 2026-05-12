# Argentor v1.4.0 — Social Media Launch Copy

---

## Hacker News

### Submission

**Title:** Show HN: Argentor – Secure AI Agent Framework in Rust (AGPL-3.0)

**URL:** https://github.com/fboiero/Argentor

---

### HN First Comment (submitter reply)

Hi HN — I'm Franco, the author.

**What it is:** Argentor is a production-ready AI agent framework written in Rust. 17 crates, 5,449+ tests, 14 LLM backends, WASM-sandboxed plugins via Wasmtime, MCP native client/server/proxy.

**Why Rust:** The performance gap vs. Python frameworks is structural, not incremental. We benchmarked it: ~2 ms framework overhead per request vs. 11–55 ms for LangChain/CrewAI/PydanticAI. On a 50-tool registry, Argentor ships 350 tokens per call vs. 2,750 for LangChain — a 7.9x reduction from Dynamic Tool Discovery (TF-IDF + keyword hybrid that filters to relevant tools before context building). All benchmarks are reproducible from `benchmarks/` with `cargo run -p argentor-benchmarks`.

**The security story:** Every competing framework blocks 0% of adversarial prompts out of the box. Argentor blocks 58.3% (basic) and 40% (adversarial, 20 tasks) with zero false positives, before the LLM is invoked. WASM sandboxing means plugin isolation is enforced at the runtime level, not by convention. We also published our gaps honestly — GCG encoding (base64, homoglyphs, zero-width characters) bypasses the current pipeline. Issues #6 and #7 are open.

**Real demo:** The EPEC Legal Agent indexes 25,667 pages of Argentine labor and energy law using native BM25 search — no API key needed. `cargo run -p epec-legal-agent -- query "régimen de indemnización por despido sin causa"`.

**License:** AGPL-3.0-only, intentionally. Infrastructure this critical should not quietly become proprietary.

Happy to answer technical questions about the security model, benchmark methodology, or the WASM isolation approach.

---

## Reddit r/rust

### Title

Argentor v1.4.0 — secure multi-agent AI framework in Rust: WASM sandboxing, 14 LLM backends, 5,449+ tests, AGPL-3.0

### Body

I've been building Argentor for the past several months — a production-ready AI agent framework written entirely in Rust. Sharing it here because the r/rust community tends to care about the same things I do: correctness, performance, and not papering over security with documentation.

**What it does**

Argentor lets you build AI agents that call tools, use memory, coordinate with other agents, and connect to any LLM provider — all from a single Rust binary with WASM-sandboxed plugins.

Architecture: 17 crates in a strict dependency hierarchy. `argentor-core` defines types; `argentor-security` handles capabilities, RBAC, audit log, TLS/mTLS, and encrypted credential store; `argentor-skills` provides the WASM runtime (Wasmtime + WASI) and skill registry; `argentor-agent` runs the agentic loop with 14 LLM backends including Claude, OpenAI, Gemini, Ollama, and AWS Bedrock (SigV4-gated). The full list is in the README.

**The numbers that matter**

I benchmarked Argentor against LangChain, CrewAI, PydanticAI, and the Claude Agent SDK across 6 dimensions. The headline: ~2 ms framework overhead vs. 11–55 ms for the Python alternatives (N=10 paired, p < 0.0001, Cohen's d > 0.8 throughout). On tool-heavy workloads the token cost is 7.9x lower than LangChain due to Dynamic Tool Discovery. Full methodology and raw JSON are in `benchmarks/` and `docs/BENCHMARK_SYNTHESIS.md`.

**Security-specific**

Every Python-based framework in the comparison ships zero input guardrails by default. Argentor blocks 58.3% of a 15-prompt adversarial test set (prompt injection, PII, encoded payloads) with precision 1.00. WASM sandbox means capability grants are explicit and logged at load time — a plugin cannot read your filesystem unless you said so. I published the gaps too: GCG encoding bypasses the current pipeline entirely. That is documented and on the roadmap.

**Real demo — no API key needed**

```bash
git clone https://github.com/fboiero/Argentor.git
cd Argentor
cargo run --example hello_world
# or
cargo run -p epec-legal-agent -- query "indemnización por despido"
```

The EPEC Legal Agent indexes 25,667 pages of Argentine labor law with native BM25 — runs fully offline after ingest.

License: AGPL-3.0-only. Repo: https://github.com/fboiero/Argentor

Happy to discuss the WASM isolation design, the MCP proxy architecture, or the benchmark methodology in the comments.

---

## Reddit r/MachineLearning

### Title

[Project] Argentor: AI agent framework with built-in security guardrails — benchmarks vs. LangChain/CrewAI/PydanticAI/Claude SDK

### Body

I want to share a project that addresses something I think the ML/agent safety community underweights: the default security posture of the agent framework itself.

**The problem in one sentence**

LangChain, CrewAI, PydanticAI, and the Claude Agent SDK all block 0% of adversarial prompts in their default installation. There are no client-side input guardrails. Every prompt goes directly to the LLM.

**What Argentor does differently**

Argentor is an open-source AI agent framework (Rust, AGPL-3.0) with a guardrail engine that sits in the request pipeline *before* the LLM is invoked. It blocks prompt injection (23+ pattern signatures), PII extraction (Luhn, SSN, email, phone with redaction), base64-decode attacks, SSRF, and path traversal. In a reproducible benchmark across 15 adversarial prompts (3 samples each), Argentor blocks 58.3% with precision 1.00 — zero false positives on legitimate inputs.

Against a 20-task adversarial suite (Phase 3, GCG encoding, tool confusion, context injection families): 40% block rate, precision 1.00. All four compared frameworks: 0/20. I also published the gaps — GCG encoding (base64, homoglyphs, zero-width chars, leet, fullwidth) is a complete blind spot. That is in the open issues and the evolution roadmap.

**Safety-relevant architecture decisions**

- WASM sandboxing via Wasmtime: plugins physically cannot access filesystem or network without explicit capability grants. This is not a runtime check you can bypass with a clever prompt — it is a hardware-enforced boundary.
- Capability-based permission model: every tool call is gated by a `PermissionSet` that is set at agent initialization, not at prompt time.
- Audit log: append-only, rotating, with background writer. Every capability grant and block decision is recorded.
- Compliance modules: GDPR, ISO 27001, ISO 42001, DPGA — runtime modules integrated with the audit log, not documentation templates.

**Performance context**

~2 ms framework overhead per request vs. 11–55 ms for Python alternatives. 7.9x fewer tokens to LLM on 50-tool workloads (Dynamic Tool Discovery filters to relevant tools before context building). Full benchmarks: `docs/BENCHMARK_SYNTHESIS.md`, reproducible from `benchmarks/`.

**Demo (no API key required)**

The EPEC Legal Agent: 25,667 pages of Argentine labor law, BM25 search, runs offline after ingest.

```bash
git clone https://github.com/fboiero/Argentor.git
cargo run -p epec-legal-agent -- query "indemnización por despido sin causa"
```

Repo: https://github.com/fboiero/Argentor

I'm happy to discuss the threat model, benchmark methodology, or the WASM isolation design.

---

## Twitter / X Thread

### Tweet 1 (intro)

Introducing Argentor v1.4.0 — an AI agent framework in Rust where security is the default, not a plugin.

LangChain, CrewAI, PydanticAI, Claude SDK: 0% adversarial prompt blocking out of the box.
Argentor: 58.3%, zero false positives, before the LLM sees the input.

github.com/fboiero/Argentor | AGPL-3.0

---

### Tweet 2 (benchmarks)

The performance gap vs. Python frameworks isn't incremental — it's structural.

Argentor framework overhead: ~2 ms
PydanticAI: ~11 ms
LangChain: ~20 ms
CrewAI: ~55 ms

On 50-tool workloads: 7.9x fewer tokens than LangChain.
At 100K req/day that's $185K/year saved on LLM API costs alone.

All reproducible: `cargo run -p argentor-benchmarks`

---

### Tweet 3 (security)

How Argentor's security model works:

• Every plugin runs in a Wasmtime WASM sandbox — capability-based, no implicit filesystem/network access
• Guardrail engine fires BEFORE the LLM: PII detection, prompt injection (23+ signatures), SSRF, path traversal, encoded payloads
• RBAC + JWT/mTLS + AES-256-GCM encrypted credential store
• Append-only audit log for every capability grant and block decision

Gap we're honest about: GCG encoding (base64, homoglyphs, zero-width chars) bypasses the current pipeline. Issues #6 #7 are open.

---

### Tweet 4 (EPEC demo)

Real-world demo: the EPEC Legal Agent

• 25,667 pages of Argentine labor and energy law
• Native BM25 search — no embedding API, no vector database, no API key
• Answers from the indexed corpus only (no hallucination outside documents)

Try it:
```
cargo run -p epec-legal-agent -- query \
  "régimen de indemnización por despido sin causa"
```

---

### Tweet 5 (CTA)

Argentor v1.4.0:

• 17 crates, 5,449+ tests, 0 clippy errors
• 14 LLM backends (Claude, OpenAI, Gemini, Ollama, Bedrock...)
• WASM plugins, MCP native, Google A2A, Python/TypeScript SDKs
• GDPR, ISO 27001, ISO 42001, DPGA compliance modules

Star: github.com/fboiero/Argentor
Try: `cargo run --example hello_world`
Join: GitHub Discussions (Discord coming soon)

AGPL-3.0-only. Because infrastructure this critical shouldn't become proprietary.

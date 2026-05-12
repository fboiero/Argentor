# Contributing to Argentor

Thanks for your interest in contributing! Argentor is an open, community-driven autonomous agent framework, and we welcome contributions of code, documentation, examples, tests, security reviews, and ideas.

This guide explains how to get set up, how we work, and what we expect from contributions. Please read it end-to-end before opening your first PR.

## Table of contents

- [Project philosophy](#project-philosophy)
- [Code of conduct](#code-of-conduct)
- [Development setup](#development-setup)
- [Project structure](#project-structure)
- [Making a change](#making-a-change)
- [Commit conventions](#commit-conventions)
- [Testing requirements](#testing-requirements)
- [Code style](#code-style)
- [Documentation requirements](#documentation-requirements)
- [Pull request process](#pull-request-process)
- [Where to start](#where-to-start)
- [Security issues](#security-issues)
- [Maintainer contact](#maintainer-contact)

## Project philosophy

Argentor is built on three non-negotiable principles:

1. **Security-first.** Every skill runs sandboxed (WASM with `wasmtime`). Every capability is explicit. There is no "just give it network access" shortcut. If a feature makes it easier to escape the sandbox or bypass capability checks, it will not be merged.
2. **Rust-native.** We use the Rust type system to prevent whole classes of bugs. Prefer compile-time guarantees over runtime checks. Use `thiserror` for library errors, `anyhow` only in binaries. No `unwrap()` in library code outside tests.
3. **Performance-conscious.** Agent loops run in hot paths. Allocations, lock contention, and unnecessary async overhead matter. Benchmarks are welcome. Premature optimization is not — profile first.

Readability and maintainability beat cleverness. We optimize for the reviewer, not the author.

## Code of conduct

This project adopts the [Contributor Covenant 2.1](./CODE_OF_CONDUCT.md). By participating you agree to uphold it. Violations can be reported privately to **fboiero@xcapit.com**.

## Development setup

### Prerequisites

- **Rust 1.80+** (install via [rustup](https://rustup.rs))
- **WASM target** for skill development: `rustup target add wasm32-wasip1`
- **Optional**: `cargo-nextest` for faster tests, `cargo-deny` for dependency audit

### Clone and build

```bash
git clone https://github.com/fboiero/Argentor.git
cd Argentor
cargo build --workspace
```

### Run the test suite

```bash
cargo test --workspace
```

Most contributors run this at least once before opening a PR. CI runs it on every push.

### Run the demo pipeline (no API keys required)

```bash
cargo run --example demo_pipeline
```

This exercises the full end-to-end agent loop using the built-in `DemoBackend` — no external API access required. It is the fastest way to sanity-check local changes.

## Project structure

Argentor is a Cargo workspace of focused crates. Each crate has a single clear responsibility — we avoid "kitchen sink" crates.

| Crate | Responsibility |
|-------|----------------|
| `argentor-core` | Base types: `Message`, `ToolCall`, `ToolResult`, error variants |
| `argentor-security` | `Capability`, `PermissionSet`, `RateLimiter`, `AuditLog`, TLS config |
| `argentor-session` | `Session`, `FileSessionStore`, persistence contracts |
| `argentor-skills` | `Skill` trait, `SkillRegistry`, `WasmSkillRuntime` (wasmtime) |
| `argentor-agent` | `AgentRunner`, `ModelConfig`, agentic loop, streaming |
| `argentor-channels` | `Channel` trait, adapter scaffolding |
| `argentor-gateway` | axum WebSocket gateway, `ConnectionManager`, `MessageRouter` |
| `argentor-builtins` | Built-in skills (echo, time, help, memory_store, memory_search) |
| `argentor-memory` | `VectorStore`, `FileVectorStore`, `LocalEmbedding` |
| `argentor-mcp` | MCP client (JSON-RPC 2.0 over stdio), `McpSkill` adapter |
| `argentor-orchestrator` | Multi-agent engine, `TaskQueue`, `AgentMonitor`, profiles |
| `argentor-compliance` | GDPR, ISO 27001, ISO 42001, DPGA modules |
| `argentor-cli` | `argentor` binary (serve, skill list, etc.) |

If you propose a new crate, include justification in the PR description (why it does not fit in an existing crate).

## Adding a new skill

Skills are the unit of capability in Argentor. A skill is anything that implements the `Skill` trait in `argentor-skills`.

### Native Rust skill (simplest)

1. Add your struct to `crates/argentor-builtins/src/` or create a new file:

```rust
use argentor_core::{ToolCall, ToolResult};
use argentor_skills::Skill;
use async_trait::async_trait;

pub struct MySkill;

#[async_trait]
impl Skill for MySkill {
    fn name(&self) -> &str { "my_skill" }
    fn description(&self) -> &str { "What this skill does in one sentence." }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let input = call.input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        ToolResult::success(format!("processed: {input}"))
    }
}
```

2. Register it in `register_builtins` (or in your own registry setup):

```rust
registry.register(Arc::new(MySkill));
```

3. Add a test in the same file under `#[cfg(test)] mod tests`.

### WASM skill (sandboxed, for untrusted code)

Use `rustup target add wasm32-wasip1` to build WASM targets. The WASM module must export a `execute` function that reads a JSON `ToolCall` from stdin and writes a JSON `ToolResult` to stdout. See `crates/argentor-skills/src/wasm_runtime.rs` for the host-side contract.

Capability grants are set at load time — the module cannot exceed them at runtime:

```rust
let permissions = PermissionSet::builder()
    .allow_file_read("/tmp")
    .allow_network("api.example.com")
    .deny_shell()
    .build();
wasm_runtime.load_plugin("my-skill.wasm", permissions)?;
```

Every capability grant is logged to the audit trail.

## Adding a new LLM backend

LLM backends are defined in `crates/argentor-agent/src/backends/`. Each backend implements the `LlmBackend` trait:

```rust
#[async_trait]
pub trait LlmBackend: Send + Sync {
    async fn complete(&self, messages: &[Message], config: &ModelConfig) -> Result<Message>;
    async fn stream(&self, messages: &[Message], config: &ModelConfig)
        -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;
    fn name(&self) -> &str;
}
```

Steps:

1. Add a new file `crates/argentor-agent/src/backends/my_provider.rs`.
2. Implement `LlmBackend` for your struct. Handle auth (API key, OAuth, SigV4, etc.) inside the backend — callers never touch credentials directly.
3. Add a variant to `LlmProvider` in `crates/argentor-core/src/types.rs`:
   ```rust
   pub enum LlmProvider {
       // existing variants...
       MyProvider,
   }
   ```
4. Wire the new variant in `AgentRunner::new` where `LlmProvider` is matched to a backend instance.
5. If the backend requires a feature flag (e.g., heavy dependencies, platform-specific auth like SigV4), gate it with a `[features]` entry in `crates/argentor-agent/Cargo.toml` and wrap the impl in `#[cfg(feature = "my-provider")]`.
6. Add integration tests in `crates/argentor-agent/tests/my_provider.rs`. Use a mock HTTP server (e.g., `wiremock`) — do not require live credentials in CI.
7. Update `README.md` (integration table) and `CHANGELOG.md` (under the next release).

## Making a change

1. **Open an issue first for non-trivial work.** Bug fixes and tiny docs changes are fine without one, but features, refactors, and API changes should be discussed first so nobody wastes effort on a direction we cannot accept.
2. **Fork the repo** on GitHub and clone your fork.
3. **Create a feature branch** off `master`:
   ```bash
   git checkout -b feat/skill-validator
   git checkout -b fix/session-race-condition
   git checkout -b docs/clarify-capability-model
   ```
   Branch naming: `<type>/<short-kebab-case-description>` where `<type>` is `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, or `ci`.
4. **Make the smallest change that solves the problem.** Unrelated cleanups belong in separate PRs.
5. **Run the full local gate** before pushing (see [Pull request process](#pull-request-process)).
6. **Push and open a PR** against `master`.

## Commit conventions

We use [Conventional Commits](https://www.conventionalcommits.org/). Examples:

```
feat(skills): add wasm module validation at load time
fix(session): prevent race when two writers flush concurrently
docs(readme): clarify capability grant syntax
refactor(agent): extract streaming decoder into its own module
test(mcp): cover stdio reconnect on upstream crash
perf(memory): avoid double allocation in embedding lookup
chore(deps): bump wasmtime to 26
ci: cache cargo registry between jobs
```

Rules:

- **Do NOT add `Co-Authored-By` or AI attribution lines.** This is project policy.
- Keep the subject under 72 characters. Imperative mood ("add", not "added").
- Use the body for the **why**, not the what — the diff shows the what.
- Breaking changes: add `!` after the scope and a `BREAKING CHANGE:` footer.

## Testing requirements

**Every change that touches behavior must include a test.** No exceptions for "it's obvious" or "it's tiny."

- **Unit tests** live next to the code in a `#[cfg(test)] mod tests` block.
- **Integration tests** live in `crates/<crate>/tests/*.rs`.
- **Async tests** use `#[tokio::test]`.
- **Property tests** (where useful) use `proptest`.
- **Name tests descriptively**: `test_<subject>_<scenario>_<expected>` — e.g. `test_rate_limiter_blocks_burst_over_quota`.
- **Cover the failure path**, not just the happy path. A test that only asserts success on valid input is half a test.

Run the suite locally:

```bash
cargo test --workspace
cargo test --workspace --no-default-features     # for crates with optional features
```

If you add a dependency, also run:

```bash
cargo deny check        # license + advisory audit (if installed)
```

## Code style

We enforce zero-warning builds. Before pushing:

```bash
cargo fmt --all
cargo fmt --all -- --check                              # CI runs this
cargo clippy --workspace --all-targets -- -D warnings   # CI runs this
```

Style rules:

- Use `rustfmt` defaults. Do not customize formatting per-file.
- `clippy` warnings are treated as errors in CI. Fix them, do not `#[allow]` them unless you add a justification comment.
- Prefer `&str` over `String` in signatures when you do not need ownership.
- Prefer iterators over manual loops when the intent is clearer.
- Avoid `unwrap()` and `expect()` in library code. If a panic is truly unreachable, document **why** in the `expect` message.
- Public items require rustdoc (see below).

## Documentation requirements

- **Public items** (`pub fn`, `pub struct`, `pub trait`, `pub enum`) require rustdoc comments. Describe what it does, arguments, return value, errors, and at least one example for non-trivial APIs.
- **Changes to public API** must update `CHANGELOG.md` under the next release.
- **New features** should include a usage example in the crate's README or the top-level `docs/` directory.
- **Breaking changes** must document the migration path.

Build docs locally to verify they render:

```bash
cargo doc --workspace --no-deps --open
```

## Pull request process

1. **One PR = one logical change.** If you find yourself writing "also" in the description, split the PR.
2. **Keep PRs small.** Under ~400 lines of diff is ideal. Larger PRs take longer to review and are more likely to be rejected.
3. **Fill out the PR template completely.** The checklist exists for a reason.
4. **CI must be green.** All of: `cargo test`, `cargo clippy`, `cargo fmt --check`, and any security/deny jobs.
5. **Respond to review promptly.** If you cannot address feedback within a reasonable time, leave a comment so reviewers know the status.
6. **Rebase, do not merge.** Keep history linear. Use `git rebase master` to pull in upstream changes.
7. **Squash on merge** is the default unless the PR has a genuinely useful multi-commit history.

Expect at least one reviewer. Complex changes may need two. Security-relevant changes require explicit sign-off from the maintainer.

## Where to start

Not sure what to work on? Good options:

- **[`good first issue`](https://github.com/fboiero/Argentor/labels/good%20first%20issue)** — small, well-scoped tasks ideal for new contributors.
- **[`help wanted`](https://github.com/fboiero/Argentor/labels/help%20wanted)** — tasks we actively want help on.
- **Documentation** — there is always more to clarify, translate (especially Spanish), or expand. Doc PRs are reviewed quickly.
- **Examples** — add a new WASM skill example, a new channel adapter sketch, or an end-to-end demo.
- **Tests** — add coverage for edge cases or regression tests for fixed bugs.
- **Benchmarks** — we have headroom. If you profile and find a hot spot, a reproducible benchmark is welcome.

Areas where we specifically want help:

- Additional channel adapters (Matrix, WhatsApp, Signal)
- MCP server integrations and examples
- WASM skill examples in languages other than Rust (AssemblyScript, Go via TinyGo, etc.)
- Documentation translations (especially Spanish — this is an Argentinian-led project)
- Security audits and penetration testing
- DPGA nomination process support

## Security issues

**Do not open public issues for critical vulnerabilities.** See [SECURITY.md](./SECURITY.md) for the responsible disclosure process.

For low-severity hardening suggestions, use the `[Security]` issue template.

## Maintainer contact

- **Maintainer**: Franco Boiero (`@fboiero`)
- **Email**: fboiero@xcapit.com
- **GitHub Discussions**: https://github.com/fboiero/Argentor/discussions

For general questions prefer Discussions over email — it helps the whole community.

## License

By contributing, you agree that your contributions will be licensed under the [AGPL-3.0-only](./LICENSE) license that covers the project.

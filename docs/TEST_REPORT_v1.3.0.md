# Argentor v1.3.0 — Full Regression Test Report

**Date:** 2026-04-15
**Tester:** Automated validation suite
**Commit:** b2cadda49283e4a377e8c5ea2fb12bb25cfb188b
**Platform:** macOS Darwin 25.4.0

---

## Summary

| Category | Result | Details |
|----------|--------|---------|
| Compilation (default) | PASS | `cargo build --workspace` — 0 errors, warnings only |
| Compilation (all-features) | FAIL | 3 errors in `argentor-builtins` (feature-gated Docker backend) |
| Compilation (examples) | PASS | All 5 examples built successfully |
| Clippy | PASS | 0 errors (warnings present, none blocking) |
| Unit Tests | FAIL | 5449 passed, **1 failed**, 18 ignored |
| Examples | PASS | 5/5 run successfully |
| Guardrails | PASS | 5/5 checks passed |
| Python SDK | PASS | Install, import, and version all OK |
| Benchmark Harness | PASS | 100 tests passed |
| Documentation | PASS | All required files present |
| Infrastructure | PASS | Docker, CI workflows present |

## OVERALL: FAIL (2 issues — see Detailed Results)

---

## Detailed Results

### 1. Compilation

#### `cargo build --workspace` — PASS
Finished `dev` profile in 58.40s. Warnings only (unused imports, dead code in gateway trace_viz). No errors.

#### `cargo build --workspace --all-features` — FAIL
**3 errors in `argentor-builtins` (lib):**
- `error[E0432]`: unresolved import `futures_util` — missing feature dependency
- `error[E0599]`: no method `next` on `Pin<Box<dyn Stream<...>>>` — futures_util not imported
- `error[E0063]`: missing field `requires_approval` in `SkillDescriptor` initializer

Only `argentor-builtins` fails under `--all-features`. All other crates compile clean.

#### `cargo build --examples -p argentor-cli` — PASS
All examples compiled in 62s. Minor unused-import warnings in `mcp_client.rs` (3 suggestions).

#### `cargo clippy --workspace` — PASS
**0 clippy errors.** Warnings present but non-blocking.

---

### 2. Test Suite

**Total: 5449 passed, 1 failed, 18 ignored**

| Crate / Test File | Tests Run | Passed | Failed | Ignored |
|---|---|---|---|---|
| argentor-a2a (lib) | 63 | 63 | 0 | 0 |
| argentor-agent (lib) | 1319 | 1316 | 0 | 3 |
| argentor-agent/tests/llm_integration | 30 | 22 | 0 | 8 |
| argentor-agent/tests/providers_integration | 42 | 42 | 0 | 0 |
| argentor-agent/tests/regression | 34 | 34 | 0 | 0 |
| argentor-agent/tests/regression_e2e | 12 | 12 | 0 | 0 |
| argentor-agent/tests/regression_error_recovery | 8 | 8 | 0 | 0 |
| argentor-agent/tests/scalability_concurrent | 8 | 8 | 0 | 0 |
| argentor-agent/tests/scalability_long_running | 6 | 6 | 0 | 0 |
| argentor-agent/tests/scalability_memory | 10 | 8 | 0 | 2 |
| argentor-agent/tests/security_regression_injection | 19 | 18 | 0 | 1 |
| argentor-agent (doctests) | 17 | 14 | **1** | 2 |
| argentor-benchmarks (lib) | 100 | 100 | 0 | 0 |
| argentor-builtins (lib) | 1209 | 1209 | 0 | 0 |
| argentor-builtins/tests/builtins_integration | 18 | 18 | 0 | 0 |
| argentor-channels (lib) | 6 | 6 | 0 | 0 |
| argentor-channels/tests/channel_integration | 16 | 16 | 0 | 0 |
| argentor-cli (main) | 57 | 57 | 0 | 0 |
| argentor-cloud (lib) | 106 | 106 | 0 | 0 |
| argentor-compliance (lib) | 60 | 60 | 0 | 0 |
| argentor-compliance/tests/compliance_integration | 8 | 8 | 0 | 0 |
| argentor-core (lib) | 135 | 135 | 0 | 0 |
| argentor-core/tests/core_integration | 6 | 6 | 0 | 0 |
| argentor-gateway (lib) | 546 | 546 | 0 | 0 |
| argentor-gateway/tests/approval_persistence_integration | 20 | 20 | 0 | 0 |
| argentor-gateway/tests/gateway_integration | 15 | 15 | 0 | 0 |
| argentor-gateway/tests/regression | 9 | 9 | 0 | 0 |
| argentor-gateway/tests/regression_api | 10 | 10 | 0 | 0 |
| argentor-gateway/tests/router_integration | 24 | 24 | 0 | 0 |
| argentor-gateway/tests/scalability_gateway | 8 | 8 | 0 | 0 |
| argentor-gateway/tests/xcapitsff_integration | 15 | 15 | 0 | 0 |
| argentor-mcp (lib) | 175 | 175 | 0 | 0 |
| argentor-mcp/tests/mcp_integration | 8 | 8 | 0 | 0 |
| argentor-memory (lib) | 342 | 342 | 0 | 0 |
| argentor-memory/tests/memory_integration | 12 | 12 | 0 | 0 |
| argentor-orchestrator (lib) | 359 | 359 | 0 | 0 |
| argentor-orchestrator/tests/e2e_orchestration | 7 | 7 | 0 | 0 |
| argentor-orchestrator/tests/regression_multi_agent | 7 | 7 | 0 | 0 |
| argentor-security (lib) | 203 | 203 | 0 | 0 |
| argentor-security/tests/regression | 26 | 26 | 0 | 0 |
| argentor-security/tests/security_regression_audit | 7 | 7 | 0 | 0 |
| argentor-security/tests/security_regression_crypto | 9 | 9 | 0 | 0 |
| argentor-security/tests/security_regression_path_traversal | 10 | 10 | 0 | 0 |
| argentor-security/tests/security_regression_rbac | 9 | 9 | 0 | 0 |
| argentor-security/tests/security_regression_shell_injection | 15 | 15 | 0 | 0 |
| argentor-security/tests/security_regression_ssrf | 10 | 9 | 0 | 1 |
| argentor-session (lib) | 74 | 74 | 0 | 0 |
| argentor-session/tests/session_integration | 11 | 11 | 0 | 0 |
| argentor-skills (lib) | 188 | 188 | 0 | 0 |
| argentor-skills/tests/loader_test | 7 | 7 | 0 | 0 |
| argentor-skills/tests/security_regression_wasm_vetting | 9 | 9 | 0 | 0 |
| argentor-skills/tests/wasm_integration | 8 | 8 | 0 | 0 |
| argentor-tee (lib) | 32 | 32 | 0 | 0 |

**FAILED TEST:**
- `crates/argentor-agent/src/runner.rs - runner::ScaffoldMode (line 47)` (doctest)
- **Root cause:** Doctest instantiates `AuditLog::new()` which internally calls a Tokio async initializer. Doctests run outside a Tokio runtime context, causing: `thread 'main' panicked at crates/argentor-security/src/audit.rs:98:9: there is no reactor running, must be called from the context of a Tokio 1.x runtime`
- **Scope:** Doctest only. All 203 `argentor-security` unit tests pass. All `argentor-agent` unit tests pass.

---

### 3. Examples

All 5 examples run successfully:

| Example | Status | Output |
|---------|--------|--------|
| `hello_world` | PASS | `Agent response: Hello from Argentor! I am a secure, WASM-sandboxed AI agent framework built in Rust.` |
| `with_tools` | PASS | `Registered skills: ["hash", "calculator", "uuid_generator"]` / `Agent response: The result of 40 + 2 is 42.` |
| `custom_skill` | PASS | `Custom skill registered: 'reverse'` / `Agent response: The reversed string is 'rotnetrA'.` |
| `multi_agent` | PASS | 4-stage pipeline (spec→coder→tester→reviewer), 4 artifacts produced |
| `mcp_client` | PASS | API demo output, no live connection required |

---

### 4. Guardrail Validation

Binary `e2e_guardrail_check` built and ran successfully. All 5 checks passed:

| Check | Input | Result |
|-------|-------|--------|
| S-01 Shell injection | `rm -rf` variant | BLOCKED (expected) |
| S-04 PII redaction | Email address | BLOCKED (expected) |
| Prompt injection | "ignore previous instructions" | BLOCKED (expected) |
| Base64 encoded injection | Base64 payload | BLOCKED (expected) |
| Benign query | Safe user input | ALLOWED (expected) |

---

### 5. Python SDK

| Check | Result |
|-------|--------|
| `pip install -e python/` | PASS — installed argentor 1.3.0 (upgraded from 1.2.0) |
| `from argentor import Agent, Session, Skill, SkillRegistry` | PASS — prints `OK` |
| `argentor.__version__` | PASS — `1.3.0` |

---

### 6. Benchmark Harness

| Check | Result |
|-------|--------|
| `cargo test -p argentor-benchmarks --lib` | PASS — 100/100 tests passed |
| Benchmark tasks count | 80 tasks in `benchmarks/tasks/` |
| TASK_BENCHMARKS.md | PRESENT (in `docs/`) |
| SECURITY_BENCHMARKS.md | PRESENT (in `docs/`) |
| COST_BENCHMARKS.md | PRESENT (in `docs/`) |
| ADVERSARIAL_BENCHMARKS.md | PRESENT (in `docs/`) |
| LONG_HORIZON_BENCHMARKS.md | PRESENT (in `docs/`) |
| DX_BENCHMARKS.md | PRESENT (in `docs/`) |
| SIEM_BENCHMARKS.md | PRESENT (in `docs/`) |
| COMPLIANCE_BENCHMARKS.md | PRESENT (in `docs/`) |
| INTEGRATIONS_BENCHMARKS.md | PRESENT (in `docs/`) |
| BENCHMARK_SYNTHESIS.md | PRESENT (in `docs/`) |

Note: Benchmark docs are located in `docs/` rather than `benchmarks/` as specified. All 10 files exist.

---

### 7. Documentation

| File | Status |
|------|--------|
| README.md | PRESENT |
| CHANGELOG.md | PRESENT |
| CONTRIBUTING.md | PRESENT |
| LICENSE | PRESENT |
| docs/GETTING_STARTED.md | PRESENT |
| docs/API_REFERENCE.md | PRESENT |
| docs/DEPLOYMENT.md | PRESENT |
| docs/TROUBLESHOOTING.md | PRESENT |
| docs/EVOLUTION_ROADMAP.md | PRESENT |
| docs/BENCHMARKS_INDEX.md | PRESENT |
| docs/tutorials/ | PRESENT — 15 files |

---

### 8. Infrastructure

| Check | Result |
|-------|--------|
| `.github/workflows/` | 7 files: `ci.yml`, `nightly-llm.yml`, `overhead-gate.yml`, `publish-crates.yml`, `publish-pypi.yml`, `publish-sdks.yml`, `release.yml` |
| `Dockerfile` | PRESENT |
| `docker-compose.yml` | PRESENT |

---

## Feature Inventory

| Metric | Value |
|--------|-------|
| Crates | 17 |
| Rust LOC | 488,781 |
| Tests | 5,449 passed (1 failed doctest, 18 ignored) |
| LLM Backends | 7 (bedrock, claude, claude_code, cohere, gemini, openai, replicate) |
| Builtin Skills (`register_` calls) | 18 |
| Benchmark Tasks | 80 |
| Docs | 10 core docs + 15 tutorials |
| CI Workflows | 7 |
| Dashboard | React/Vite app (`dashboard/`) with `src/`, `dist/`, `public/` |

---

## Known Issues

### Issue 1 — `--all-features` Build Failure (argentor-builtins)
- **Severity:** MEDIUM — only affects the Docker/cloud feature flag, default build is clean
- **Crate:** `argentor-builtins`
- **Errors:** Missing `futures_util` import + missing `requires_approval` field in `SkillDescriptor` under the feature-gated Docker integration path
- **Impact:** `cargo build --workspace --all-features` fails. Default and examples builds are unaffected.

### Issue 2 — Doctest Tokio Runtime Panic (argentor-agent)
- **Severity:** LOW — isolated to one doctest; all unit and integration tests pass
- **File:** `crates/argentor-agent/src/runner.rs` line 47
- **Root cause:** `AuditLog::new()` used in the doctest setup block calls a Tokio async initializer synchronously. Doctests do not run inside a `#[tokio::test]` context.
- **Fix (not applied):** Add `#[tokio::main]` wrapper or mark the doctest with `# fn main() {}` and a `#[tokio::test]`-style block, or use a sync-safe mock for `AuditLog` in the doctest.

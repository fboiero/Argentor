# Troubleshooting Argentor

Quick answers for the most common problems. If your issue isn't here, check the relevant tutorial or open an issue at [github.com/fboiero/Agentor/issues](https://github.com/fboiero/Agentor/issues).

---

## "Connection refused" — gateway not running

**Symptom**: `curl: (7) Failed to connect to localhost port 8080: Connection refused`

**Cause**: The gateway process isn't running or is bound to a different port.

**Fix**:

```bash
# Start the gateway
cargo run -p argentor-cli -- serve --bind 0.0.0.0:8080

# Or with Docker
docker run -d -p 8080:8080 -e ANTHROPIC_API_KEY="sk-ant-..." \
  ghcr.io/fboiero/argentor:latest serve

# Verify it's up
curl http://localhost:8080/health
```

If the gateway crashes on startup, check the logs for a missing environment variable or port conflict.

---

## "API key not found" / "Provider Claude has no API key configured"

**Symptom**: Agent exits immediately with a key error, or returns `401 Unauthorized` from the LLM.

**Cause**: The API key environment variable isn't set or isn't being read.

**Fix**:

```bash
# For Claude
export ANTHROPIC_API_KEY="sk-ant-..."

# For OpenAI
export OPENAI_API_KEY="sk-..."

# For Gemini
export GEMINI_API_KEY="..."

# Verify it's visible
echo $ANTHROPIC_API_KEY
```

In Rust code, check you're reading it correctly:

```rust
let api_key = std::env::var("ANTHROPIC_API_KEY")
    .expect("ANTHROPIC_API_KEY must be set");
```

Never hardcode keys in source. Use `.env` files locally (with `dotenvy`) and your cloud provider's secrets manager in production.

---

## "Rate limited" — 429 from the LLM provider

**Symptom**: LLM calls fail with `429 Too Many Requests`, especially under load or with multi-agent pipelines.

**Fix — add a retry policy**:

```rust
use argentor_agent::RetryPolicy;

let config = ModelConfig {
    retry_policy: Some(RetryPolicy {
        max_attempts: 5,
        initial_backoff_ms: 500,
        backoff_multiplier: 2.0,
        max_backoff_ms: 30_000,
        jitter: true,
    }),
    ..
};
```

**Fix — configure per-tier rate limits in `argentor.toml`**:

```toml
[rate_limit]
requests_per_minute = 60
burst = 10
```

For multi-agent pipelines, add Redis for distributed rate-limit counters (otherwise each process tracks its own counter):

```bash
REDIS_URL=redis://localhost:6379 cargo run -p argentor-cli -- serve
```

---

## "WASM skill failed" — plugin panics or returns bad output

**Symptom**: WASM plugin crashes at runtime, or its output is garbled / zero-length.

**Cause**: Memory layout mismatch between host and plugin, or the plugin panics on bad input.

**Fix — validate the plugin before loading**:

```rust
use argentor_skills::{SkillManifest, SkillVetter};

let vetter = SkillVetter::new()
    .with_max_size(5 * 1024 * 1024)
    .with_static_analysis();

let manifest = SkillManifest::from_file("./plugin.wasm.manifest.json")?;
let result = vetter.verify("./plugin.wasm", &manifest).await?;

if !result.approved {
    eprintln!("Issues: {:?}", result.issues);
}
```

**Fix — increase the WASM memory limit** if the plugin handles large payloads:

```toml
# argentor.toml
[wasm]
memory_limit_mb = 64   # default is 16 MB per plugin
```

**Fix — check the plugin's pointer protocol**. The `execute` export must return a valid pointer to a UTF-8 JSON string. See `crates/argentor-skills/src/wasm/` for the reference layout.

---

## "Guardrail blocked legitimate content"

**Symptom**: The agent refuses a valid request, or `GuardrailEngine` flags content that isn't PII/injection/toxic.

**Cause**: The default ruleset is conservative. PII patterns or injection heuristics may match legitimate domain-specific content.

**Fix — inspect what triggered the block**:

```rust
use argentor_agent::GuardrailEngine;

let engine = GuardrailEngine::default();
let report = engine.scan_input("Your input here").await?;

for violation in &report.violations {
    println!("Rule: {} | Severity: {:?} | Match: {:?}",
        violation.rule_id, violation.severity, violation.matched_text);
}
```

**Fix — disable specific rules**:

```rust
let engine = GuardrailEngine::builder()
    .disable_rule("pii_phone_number")   // disable phone-number detection
    .disable_rule("pii_email")          // disable email detection
    .build();
```

**Fix — add an allowlist**:

```rust
let engine = GuardrailEngine::builder()
    .allow_pattern(r"\bACME-\d{6}\b")  // ACME order IDs look like SSNs to the default rules
    .build();
```

**Fix — lower the severity threshold**:

```toml
# argentor.toml
[guardrails]
block_severity = "high"   # only block high-severity violations (default: "medium")
```

---

## Agent loop never finishes / "max_turns exceeded"

**Symptom**: The agent keeps calling tools in a loop and eventually hits `max_turns`.

**Cause**: The LLM is stuck in a tool-call cycle, or `max_turns` is too low for the task complexity.

**Fix**: Increase `max_turns` (5 is a good baseline; 15-20 for complex multi-step tasks):

```rust
let config = ModelConfig {
    max_turns: 15,
    ..
};
```

If the loop is genuinely stuck, add a progress callback to diagnose which tools are being called:

```rust
let runner = AgentRunner::new(config, skills, permissions, audit)
    .with_tool_callback(|name, args, result| {
        println!("Tool: {name} | Args: {args} | Result: {result:.100}");
    });
```

---

## "Failed to create audit-logs/audit.jsonl"

**Symptom**: `AuditLog::new(...)` panics or returns an error on startup.

**Cause**: The path is relative to the working directory, which may not be the project root when running in Docker or CI.

**Fix**: Use an absolute path or create the directory first:

```rust
let log_dir = std::path::PathBuf::from(
    std::env::var("ARGENTOR_AUDIT_DIR").unwrap_or_else(|_| "/var/lib/argentor/audit".into())
);
std::fs::create_dir_all(&log_dir)?;
let audit = AuditLog::new(log_dir);
```

---

## Compilation errors after updating Argentor

**Symptom**: `error[E0063]: missing fields` or `no method named X found` after pulling a new version.

**Cause**: Argentor is under active development; APIs may change between minor versions.

**Fix**: Check `CHANGELOG.md` for breaking changes, then run:

```bash
cargo update
cargo check 2>&1 | head -40
```

If the error is a missing field on a config struct, check whether a new field was added and whether it has a default.

---

## Further help

- Full tutorial series: [docs/tutorials/README.md](./tutorials/README.md)
- API reference: `cargo doc --open --workspace`
- GitHub issues: [github.com/fboiero/Agentor/issues](https://github.com/fboiero/Agentor/issues)

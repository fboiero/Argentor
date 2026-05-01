# Build Custom Skills

> Add your own tools to Argentor: one-liner `ToolBuilder`, full `Skill` trait, sandboxed WASM plugins, and Markdown prompt-skills.

The 50+ built-in skills cover common tasks. This guide shows you how to extend Argentor with your own. For a longer treatment see [Tutorial 5: Custom Skills](./05-custom-skills.md).

---

## Prerequisites

- Completed [Tutorial 1: First Agent](./01-first-agent.md)
- Basic familiarity with `async fn` and `Arc`

---

## 1. Quick skill with `ToolBuilder`

```rust
use argentor_skills::{SkillRegistry, ToolBuilder};

let format_date = ToolBuilder::new("format_date")
    .description("Format a Unix timestamp as a human-readable date string. Use this for any date formatting task.")
    .param("timestamp", "integer", "Unix timestamp in seconds", true)
    .param("format", "string", "strftime format string, e.g. %Y-%m-%d", false)
    .handler(|args| {
        let ts = args["timestamp"].as_i64().unwrap_or(0);
        let fmt = args["format"].as_str().unwrap_or("%Y-%m-%d %H:%M UTC");
        // simplified: real impl uses chrono
        Ok(format!("Formatted: {ts} as {fmt}"))
    })
    .build();

let mut registry = SkillRegistry::new();
registry.register(format_date);
```

The LLM sees `format_date` as a callable tool with a proper JSON schema. The `.description()` text is what the LLM reads to decide when to use the tool — make it specific.

### Async handler for I/O-bound tools

```rust
use argentor_core::ArgentorError;

let http_get = ToolBuilder::new("http_get")
    .description("Fetch the body of a URL. Use for retrieving web pages or API responses.")
    .param("url", "string", "The URL to fetch", true)
    .async_handler(|args| async move {
        let url = args["url"].as_str()
            .ok_or_else(|| ArgentorError::Skill("missing url".into()))?;
        let body = reqwest::get(url).await
            .map_err(|e| ArgentorError::Skill(e.to_string()))?
            .text().await
            .map_err(|e| ArgentorError::Skill(e.to_string()))?;
        Ok(body[..body.len().min(2000)].to_string())
    })
    .build();
```

---

## 2. Full `Skill` trait for complex tools

When you need custom state, non-trivial validation, or database access, implement the trait:

```rust
use argentor_skills::skill::{Skill, SkillDescriptor};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use async_trait::async_trait;
use serde_json::json;

pub struct PriceCheckerSkill {
    api_key: String,
    base_url: String,
}

impl PriceCheckerSkill {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.yourservice.com".into(),
        }
    }
}

#[async_trait]
impl Skill for PriceCheckerSkill {
    fn descriptor(&self) -> SkillDescriptor {
        SkillDescriptor {
            name: "price_checker".into(),
            description: "Look up the current price of a product by SKU. \
                          Use for any pricing or stock availability query.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sku": { "type": "string", "description": "Product SKU" }
                },
                "required": ["sku"]
            }),
            capabilities: vec![Capability::NetworkAccess {
                allowed_hosts: vec!["api.yourservice.com".into()],
            }],
        }
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let sku = call.arguments["sku"].as_str()
            .ok_or_else(|| ArgentorError::Skill("missing sku".into()))?;

        let url = format!("{}/prices/{sku}", self.base_url);
        let resp = reqwest::Client::new()
            .get(&url)
            .bearer_auth(&self.api_key)
            .send().await
            .map_err(|e| ArgentorError::Skill(e.to_string()))?
            .text().await
            .map_err(|e| ArgentorError::Skill(e.to_string()))?;

        Ok(ToolResult {
            call_id: call.id,
            content: resp,
            is_error: false,
        })
    }
}
```

Register it:

```rust
use std::sync::Arc;

registry.register(Arc::new(PriceCheckerSkill::new(
    std::env::var("PRICE_API_KEY")?
)));
```

---

## 3. Register and run

```rust
use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
use argentor_security::{AuditLog, Capability, PermissionSet};
use argentor_session::Session;
use std::path::PathBuf;
use std::sync::Arc;

// Grant the capabilities your skills declared
let mut permissions = PermissionSet::new();
permissions.grant(Capability::NetworkAccess {
    allowed_hosts: vec!["api.yourservice.com".into()],
});

let config = ModelConfig {
    provider: LlmProvider::Claude,
    model_id: "claude-sonnet-4-20250514".into(),
    api_key: std::env::var("ANTHROPIC_API_KEY")?,
    api_base_url: None,
    temperature: 0.3,
    max_tokens: 2048,
    max_turns: 5,
    fallback_models: vec![],
    retry_policy: None,
};

let runner = AgentRunner::new(
    config,
    Arc::new(registry),
    permissions,
    Arc::new(AuditLog::new(PathBuf::from("./audit"))),
);

let mut session = Session::new();
let response = runner.run(
    &mut session,
    "What is the current price for SKU ABC-123?",
).await?;

println!("{response}");
```

---

## 4. Package as a WASM plugin

WASM plugins run inside wasmtime — isolated from the host, no ambient filesystem or network access.

### Author the plugin

In a separate crate:

```toml
# my-plugin/Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
serde_json = "1"
```

```rust
// my-plugin/src/lib.rs
#[no_mangle]
pub extern "C" fn execute(args_ptr: *const u8, args_len: usize) -> *const u8 {
    let slice = unsafe { std::slice::from_raw_parts(args_ptr, args_len) };
    let args: serde_json::Value = serde_json::from_slice(slice).unwrap_or_default();

    let n = args["n"].as_f64().unwrap_or(0.0);
    let result = serde_json::json!({ "factorial": factorial(n as u64) });
    let bytes = result.to_string().into_bytes();
    Box::leak(bytes.into_boxed_slice()).as_ptr()
}

fn factorial(n: u64) -> u64 {
    if n <= 1 { 1 } else { n * factorial(n - 1) }
}
```

Compile:

```bash
cargo build -p my-plugin --target wasm32-wasip1 --release
# output: target/wasm32-wasip1/release/my_plugin.wasm
```

### Load it in Argentor

```rust
use argentor_skills::{SkillConfig, SkillLoader, WasmSkillRuntime};

let runtime = WasmSkillRuntime::new()?;
let config = SkillConfig {
    name: "factorial".into(),
    path: "./plugins/my_plugin.wasm".into(),
    description: "Compute the factorial of a non-negative integer.".into(),
    capabilities: vec![],
};

let loader = SkillLoader::new(runtime);
let wasm_skill = loader.load(&config).await?;
registry.register(wasm_skill);
```

### Vet before loading untrusted plugins

```rust
use argentor_skills::{SkillManifest, SkillVetter};

let vetter = SkillVetter::new()
    .with_max_size(5 * 1024 * 1024)
    .with_signature_verification()
    .with_static_analysis();

let manifest = SkillManifest::from_file("./plugins/my_plugin.wasm.manifest.json")?;
let result = vetter.verify("./plugins/my_plugin.wasm", &manifest).await?;

if !result.approved {
    eprintln!("Plugin rejected: {:?}", result.issues);
    return Ok(());
}
```

---

## 5. Markdown prompt-skills

A skill doesn't have to run code — it can be a reusable prompt template:

```markdown
<!-- skills/markdown/code_reviewer.md -->
---
name: code_reviewer
description: Review a code snippet for bugs, style issues, and improvement opportunities.
parameters:
  code:
    type: string
    description: Source code to review.
    required: true
  language:
    type: string
    description: Programming language (e.g. rust, python, go).
    required: false
---

You are a senior engineer reviewing {{language | default: "code"}}.

Review the following for:
1. Correctness and potential bugs
2. Error handling gaps
3. Performance issues
4. Style and readability

Code:
```
{{code}}
```

Respond with: [LGTM] if no issues, or a numbered list of findings.
```

Load all Markdown skills from a directory:

```rust
use argentor_skills::MarkdownSkillLoader;

let loader = MarkdownSkillLoader::new("./skills/markdown");
let loaded = loader.load_all().await?;
for skill in loaded.skills {
    registry.register(skill);
}
```

---

## 6. Test your skill

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use argentor_core::ToolCall;
    use serde_json::json;

    #[tokio::test]
    async fn price_checker_returns_content() {
        let skill = PriceCheckerSkill::new("test-key");

        let call = ToolCall {
            id: "call-1".into(),
            name: "price_checker".into(),
            arguments: json!({ "sku": "ABC-123" }),
        };

        // In real tests, mock the HTTP client or use a test server.
        let result = skill.execute(call).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_error);
    }

    #[test]
    fn descriptor_has_required_sku() {
        let skill = PriceCheckerSkill::new("test-key");
        let desc = skill.descriptor();
        assert_eq!(desc.name, "price_checker");
        let required = &desc.parameters["required"];
        assert!(required.as_array().unwrap().contains(&json!("sku")));
    }
}
```

---

## Common issues

**LLM never calls your tool** — the description is too generic. Describe *when* to use it, not just what it does. Include example inputs in the description.

**"Skill execution denied: capability not granted"** — you declared a `Capability` in `descriptor()` but didn't grant it in `PermissionSet`. Always match both sides.

**`ToolBuilder` panics at `.build()`** — you forgot to call `.handler()` or `.async_handler()`. Both are required.

**WASM plugin returns garbled output** — the pointer/length protocol between host and WASM is off. Use a proper allocator export (see `crates/argentor-skills/src/wasm/` for the reference implementation).

**Handler `panic!` crashes the agent** — return `Err(ArgentorError::Skill("..."))` instead. The runner catches errors and feeds them back to the LLM.

---

## Next steps

- [Tutorial 5: Custom Skills](./05-custom-skills.md) — capabilities cheat sheet, marketplace publish/install
- [Tutorial 6: Guardrails & Security](./06-guardrails-security.md) — validate skill inputs and outputs
- [Tutorial 8: MCP Integration](./08-mcp-integration.md) — expose your skills as MCP tools

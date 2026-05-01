# Multi-Agent Orchestration Patterns

> Orchestrator-Workers, Pipeline, Fan-out/Fan-in, and Human-in-the-loop — practical recipes using `argentor-orchestrator`.

This guide shows four common patterns by example. For the full API (custom profiles, `MessageBus`, dynamic replanning, token budgeting) see [Tutorial 3: Multi-Agent Orchestration](./03-multi-agent-orchestration.md).

---

## Prerequisites

- Completed [Tutorial 1: First Agent](./01-first-agent.md)
- An API key with enough quota for multiple concurrent LLM calls
- `argentor-orchestrator` in your `Cargo.toml`:

```toml
[dependencies]
argentor-orchestrator = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-agent        = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-builtins     = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-security     = { git = "https://github.com/fboiero/Agentor", branch = "master" }
argentor-skills       = { git = "https://github.com/fboiero/Agentor", branch = "master" }
tokio  = { version = "1", features = ["full"] }
anyhow = "1"
```

---

## Shared setup

All patterns reuse this boilerplate:

```rust
use argentor_agent::{LlmProvider, ModelConfig};
use argentor_builtins::register_builtins;
use argentor_security::{AuditLog, Capability, PermissionSet};
use argentor_skills::SkillRegistry;
use std::path::PathBuf;
use std::sync::Arc;

fn base_config() -> anyhow::Result<ModelConfig> {
    Ok(ModelConfig {
        provider: LlmProvider::Claude,
        model_id: "claude-sonnet-4-20250514".into(),
        api_key: std::env::var("ANTHROPIC_API_KEY")?,
        api_base_url: None,
        temperature: 0.5,
        max_tokens: 4096,
        max_turns: 10,
        fallback_models: vec![],
        retry_policy: None,
    })
}

fn shared_skills() -> Arc<SkillRegistry> {
    let mut registry = SkillRegistry::new();
    register_builtins(&mut registry);
    Arc::new(registry)
}

fn dev_permissions() -> PermissionSet {
    let mut p = PermissionSet::new();
    p.grant(Capability::FileRead  { allowed_paths: vec![] });
    p.grant(Capability::FileWrite { allowed_paths: vec!["/tmp".into()] });
    p.grant(Capability::ShellExec { allowed_commands: vec![] });
    p
}
```

---

## Pattern 1 — Orchestrator-Workers

The orchestrator decomposes a task, dispatches subtasks to specialized workers (Spec, Coder, Tester, Reviewer), and synthesizes their output.

```rust
use argentor_orchestrator::Orchestrator;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let audit = Arc::new(AuditLog::new(PathBuf::from("./audit")));

    let orchestrator = Orchestrator::new(
        &base_config()?,
        shared_skills(),
        dev_permissions(),
        audit,
    )
    .with_output_dir(PathBuf::from("./output"))
    .with_progress(|role, msg| println!("[{role:?}] {msg}"));

    let result = orchestrator
        .run("Build a Rust function that parses ISO 8601 dates. Include unit tests and a README.")
        .await?;

    println!("\nArtifacts produced: {}", result.artifacts.len());
    for a in &result.artifacts {
        println!("  [{:?}] {} ({} bytes)", a.kind, a.name, a.content.len());
    }
    println!("\nSummary:\n{}", result.summary);
    Ok(())
}
```

What the orchestrator does automatically:
1. **Plan** — breaks the request into a DAG of subtasks (Spec → Code → Test → Review)
2. **Execute** — runs workers in dependency order, workers run in parallel when safe
3. **Synthesize** — merges artifacts and writes a summary

Built-in roles: `Spec`, `Architect`, `Coder`, `Tester`, `Reviewer`, `SecurityAuditor`, `DevOps`, `DocumentWriter`, `Custom(String)`.

---

## Pattern 2 — Pipeline (sequential chain)

A → B → C where each step receives the previous step's output. Use when later steps depend on the exact output of earlier ones.

```rust
use argentor_orchestrator::patterns::{PipelinePattern, PipelineConfig, PipelineStep};
use argentor_orchestrator::types::AgentRole;

let pipeline = PipelinePattern::new(PipelineConfig {
    steps: vec![
        PipelineStep {
            role: AgentRole::Spec,
            prompt_template: "Turn this requirement into a structured spec:\n\n{input}".into(),
        },
        PipelineStep {
            role: AgentRole::Coder,
            prompt_template: "Implement the following spec in Rust:\n\n{input}".into(),
        },
        PipelineStep {
            role: AgentRole::Tester,
            prompt_template: "Write cargo unit tests for this code:\n\n{input}".into(),
        },
    ],
});

let orchestrator = Orchestrator::new(&base_config()?, shared_skills(), dev_permissions(), audit);
let result = pipeline
    .run(&orchestrator, "Build a URL shortener with SQLite persistence")
    .await?;

println!("Final output:\n{}", result.summary);
```

Each `{input}` is replaced by the output of the previous step. The last step's output becomes `result.summary`.

---

## Pattern 3 — Fan-out / Fan-in (MapReduce)

Send the same work to N workers in parallel, then aggregate the results. Use for document processing, multi-perspective analysis, or ensemble decisions.

```rust
use argentor_orchestrator::patterns::{MapReducePattern, MapReduceConfig};
use argentor_orchestrator::types::AgentRole;

// Fan-out: each item is sent to a Summarizer worker in parallel.
// Fan-in: an Aggregator worker merges all summaries.
let map_reduce = MapReducePattern::new(MapReduceConfig {
    mapper_role: AgentRole::Custom("Summarizer".into()),
    reducer_role: AgentRole::Custom("Aggregator".into()),
    parallelism: 4,     // run up to 4 mappers at once
});

let documents = vec![
    "doc1.md content...".to_string(),
    "doc2.md content...".to_string(),
    "doc3.md content...".to_string(),
    "doc4.md content...".to_string(),
    "doc5.md content...".to_string(),
];

let orchestrator = Orchestrator::new(&base_config()?, shared_skills(), dev_permissions(), audit);
let result = map_reduce.run(&orchestrator, documents).await?;

println!("Aggregated summary:\n{}", result.summary);
```

Token usage scales with `parallelism`. Start at 3-5 for cost control.

### Ensemble voting variant

```rust
use argentor_orchestrator::patterns::{EnsemblePattern, EnsembleConfig, VotingStrategy};

// All 3 workers answer independently; the best answer wins.
let ensemble = EnsemblePattern::new(EnsembleConfig {
    roles: vec![
        AgentRole::Custom("Expert-A".into()),
        AgentRole::Custom("Expert-B".into()),
        AgentRole::Custom("Expert-C".into()),
    ],
    voting: VotingStrategy::Majority,
});

let result = ensemble
    .run(&orchestrator, "What is the best database for a high-write IoT workload?")
    .await?;

println!("Winning answer:\n{}", result.summary);
```

---

## Pattern 4 — Human-in-the-loop approval

The orchestrator pauses before executing destructive or high-risk subtasks and waits for human confirmation.

```rust
use argentor_orchestrator::{Orchestrator, HumanApprovalGate};
use argentor_orchestrator::types::ApprovalRequest;

// Define which task kinds need human approval
let gate = HumanApprovalGate::new(|req: &ApprovalRequest| {
    // Called synchronously before the risky subtask runs.
    println!("\n=== APPROVAL REQUIRED ===");
    println!("Task:   {}", req.task_description);
    println!("Risk:   {:?}", req.risk_level);
    println!("Agent:  {:?}", req.role);
    print!("Approve? (yes/no): ");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    input.trim().eq_ignore_ascii_case("yes")
});

let orchestrator = Orchestrator::new(&base_config()?, shared_skills(), dev_permissions(), audit)
    .with_human_approval(gate)
    .with_output_dir(PathBuf::from("./output"));

let result = orchestrator
    .run("Refactor the auth module and deploy to staging")
    .await?;

println!("Result:\n{}", result.summary);
```

The orchestrator tags subtasks with a `risk_level` based on the tools they request (shell commands, file writes, network calls). Your gate function receives the `ApprovalRequest` and returns `true` (proceed) or `false` (skip/abort).

For async approval (e.g. Slack notification + button click), implement the `AsyncApprovalGate` trait instead:

```rust
use argentor_orchestrator::AsyncApprovalGate;
use async_trait::async_trait;

pub struct SlackApprovalGate { webhook_url: String }

#[async_trait]
impl AsyncApprovalGate for SlackApprovalGate {
    async fn request_approval(&self, req: &ApprovalRequest) -> bool {
        // Post to Slack, poll for reaction, return result
        post_to_slack(&self.webhook_url, req).await
    }
}
```

---

## Observing token usage

Any orchestrator exposes `AgentMonitor` for per-worker metrics:

```rust
let monitor = orchestrator.monitor();
let stats = monitor.all_stats().await;

for (role, s) in stats {
    println!("{role:?}: {} turns | {} tokens | ${:.4}",
        s.turns, s.total_tokens, s.estimated_cost_usd);
}
```

Sample output:

```
Orchestrator: 2 turns | 3412 tokens | $0.0051
Coder:        6 turns | 12849 tokens | $0.0193
Tester:       4 turns | 7962 tokens | $0.0119
Reviewer:     2 turns | 4301 tokens | $0.0064
```

---

## Choosing a pattern

| Pattern | Use when |
|---------|----------|
| Orchestrator-Workers | Complex tasks that need planning + parallel execution |
| Pipeline | Strict sequential dependency (each step transforms the last) |
| Fan-out/Fan-in | Same task over many inputs, or consensus from multiple perspectives |
| Human-in-the-loop | Destructive operations, regulated domains, or high-stakes decisions |

Mix patterns: a `PipelinePattern` step can internally use a `MapReducePattern`.

---

## Common issues

**"Dependency cycle detected in task graph"** — the orchestrator produced a cycle. Inspect `orchestrator.queue()` and check for tasks that list each other as dependencies.

**All workers give the same answer** — system prompts are too similar. Give each role a distinct persona and narrow scope.

**Token explosion** — workers duplicate context. Lower `max_turns` per role to 3-6 and enable progressive tool disclosure via MCP.

**Pipeline stalls** — a step returned an empty string. Each `PipelineStep.prompt_template` must produce a non-empty prompt when `{input}` is substituted.

---

## Next steps

- [Tutorial 3: Multi-Agent Orchestration](./03-multi-agent-orchestration.md) — `MessageBus`, dynamic replanning, `AgentProfile` customization
- [Tutorial 4: RAG Pipeline](./04-rag-pipeline.md) — share a vector store across all workers
- [Tutorial 9: Production Deployment](./09-deployment.md) — run multi-agent pipelines in Docker and Kubernetes

# Argentor Benchmarks

> Task-based comparison harness for Argentor vs LangChain / CrewAI / PydanticAI / etc.

## What this is

`experiments/comparison/` measures **micro-ops** (cold start, throughput, memory).
This directory measures **task completion** — the metric that actually predicts whether
a framework solves real user problems: given a task, does the agent complete it, at
what cost, with what quality?

## Quick start

```bash
# List discovered tasks
cargo run -p argentor-benchmarks --release -- list

# Run a single task on a single runner
cargo run -p argentor-benchmarks --release -- run --task t2_simple_qa --runner argentor

# Run all tasks on all runners
cargo run -p argentor-benchmarks --release -- run-all --runners argentor,mock

# Measure audit JSONL endpoint scalability
cargo run -p argentor-benchmarks --release -- audit-scale \
  --events 100000 \
  --page-limit 100 \
  --violation-every 100 \
  --samples 5
```

Results are written to `benchmarks/results/run_<timestamp>.json` and printed as a
Markdown table.

`audit-scale` writes `benchmarks/results/audit_scale_<timestamp>.json` with
latency samples for logs first page, logs second page, violations first page,
stats cold scan, and stats warm cache hits.

## Architecture

```
benchmarks/
├── src/
│   ├── task.rs          # Task, TaskResult, Rubric
│   ├── runners/         # one implementation per framework
│   │   ├── argentor.rs  # native Argentor (mock LLM by default)
│   │   ├── mock.rs      # no-op baseline
│   │   └── external.rs  # spawns Python/Node subprocess for other frameworks
│   ├── metrics/         # cost, quality, latency
│   └── report.rs        # Markdown/JSON output
├── tasks/               # YAML task definitions
│   ├── t1_pdf_summary/
│   ├── t2_simple_qa/
│   └── t3_tool_selection/
├── external/            # non-Rust runners (Python, etc.)
└── results/             # JSON outputs, timestamped
```

## Adding a new task

1. Create `tasks/<task_id>/task.yaml` with `id`, `prompt`, `rubric`, optional `ground_truth`.
2. If the task has input files, reference them in `input: { file: "..." }` — the runner loads them relative to the task dir.
3. Run `cargo run -p argentor-benchmarks -- list` to verify discovery.

Example:

```yaml
id: t4_my_task
name: My Task
description: One-line description
kind: reasoning
prompt: |
  Do the thing.
input: ""
ground_truth: |
  Expected answer
rubric:
  criteria:
    - name: correctness
      description: Got the right answer
      weight: 1.0
  pass_threshold: 6.0
max_turns: 5
allowed_tools:
  - calculator
```

## Adding a new runner

Framework-specific runners live in `src/runners/` (for Rust-native) or `external/` (for
Python/Node).

### Rust-native runner
Implement the `Runner` trait:

```rust
use argentor_benchmarks::runners::{Runner, RunnerKind};
use argentor_benchmarks::task::{Task, TaskResult};

pub struct MyRunner;

#[async_trait::async_trait]
impl Runner for MyRunner {
    fn kind(&self) -> RunnerKind { RunnerKind::PydanticAi }
    fn name(&self) -> String { "pydantic-ai v1.0".into() }
    async fn run(&self, task: &Task, task_dir: &std::path::Path)
        -> anyhow::Result<TaskResult> {
        // ...
    }
}
```

### External (Python/Node) runner

Produce a JSON `TaskResult` on stdout. Called as:
```bash
<command> --task <task-json-path> --task-dir <dir>
```

Wire up via `ExternalRunner::new("python3", RunnerKind::Langchain, "langchain v0.3")`.

## Metrics

| Metric | How it's computed | Source |
|--------|-------------------|--------|
| Latency (ms) | `ended_at - started_at` | Runner-reported timestamps |
| Cost (USD) | `input_tokens * in_rate + output_tokens * out_rate` | `metrics/cost.rs` with 2026 pricing |
| Quality (0-10) | Word overlap with ground truth (v1) or LLM-as-judge (v2) | `metrics/quality.rs` |
| Passed rubric | `quality.aggregate_score >= rubric.pass_threshold` | Boolean |

## Roadmap

- **Phase 0** ✅ Harness skeleton (this)
- **Phase 1**: T1-T5 defined + Argentor + LangChain runners → first apples-to-apples numbers
- **Phase 2**: Security + cost benchmarks in parallel
- **Phase 3**: DX + adversarial
- **Phase 4**: Long-horizon (SWE-bench-style)
- **Phase 5**: Synthesis → evolution roadmap for Argentor v1.2

## Cost safety

All runners use MOCK LLMs by default (no API calls, $0). Real LLM runs are explicit
and budgeted separately.

//! Benchmark harness core types.
//!
//! # Overview
//!
//! Every benchmark follows this flow:
//! ```text
//! Task (YAML)  →  Runner (framework impl)  →  Execution (real or mock LLM)  →  Metrics (cost, quality, latency)
//! ```
//!
//! # Running a benchmark
//!
//! ```ignore
//! use argentor_benchmarks::{Task, runners::{ArgentorRunner, Runner}, metrics};
//!
//! let (task, task_dir) = Task::load_yaml("tasks/t1_pdf_summary/task.yaml")?;
//! let runner = ArgentorRunner::new();
//! let result = runner.run(&task, &task_dir).await?;
//! let m = metrics::compute(&task, &result);
//! ```

pub mod cost_sim;
pub mod dashboard_gen;
pub mod datasets;
pub mod metrics;
pub mod report;
pub mod runners;
pub mod task;

pub use cost_sim::{simulate as simulate_cost, CostBreakdown, CostWorkload, Framework};
pub use metrics::{
    CostMetric, LatencyMetric, LongHorizonMetrics, LongHorizonSummary, MultiAgentMetrics,
    MultiAgentSummary, QualityMetric, TaskMetrics,
};
pub use runners::{Runner, RunnerKind};
pub use task::{Rubric, Task, TaskInput, TaskKind, TaskResult};

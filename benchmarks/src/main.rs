//! CLI for running benchmarks with N-sample statistical aggregation.
//!
//! Examples:
//! ```bash
//! # List discovered tasks
//! cargo run -p argentor-benchmarks -- list
//!
//! # Run N samples on each (task, runner) combo
//! cargo run -p argentor-benchmarks --release -- run-all \
//!   --runners argentor,langchain --samples 10
//! ```

use anyhow::Context;
use argentor_benchmarks::dashboard_gen;
use argentor_benchmarks::metrics::cost::{self as cost_metric, Scale};
use argentor_benchmarks::metrics::long_horizon::{self as lh_metric, LongHorizonSummary};
use argentor_benchmarks::metrics::multi_agent::{self as ma_metric, MultiAgentSummary};
use argentor_benchmarks::metrics::{self, compute_block_rate, BlockRateMetric, PairedTTest, Stats};
use argentor_benchmarks::report::RunReport;
use argentor_benchmarks::runners::{
    ArgentorRunner, ExternalRunner, MockRunner, Runner, RunnerKind,
};
use argentor_benchmarks::task::{Task, TaskKind, TaskResult};
use clap::{Parser, Subcommand, ValueEnum};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Argentor benchmark harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Tasks directory (default: ./benchmarks/tasks)
    #[arg(long, global = true, default_value = "benchmarks/tasks")]
    tasks_dir: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// List discovered tasks
    List,
    /// Run a specific task
    Run {
        #[arg(long)]
        task: String,
        #[arg(long, default_value = "argentor")]
        runner: RunnerArg,
        #[arg(long, default_value_t = false)]
        intelligence: bool,
    },
    /// Run all discovered tasks on all enabled runners
    RunAll {
        #[arg(long, value_delimiter = ',', default_value = "argentor,mock")]
        runners: Vec<RunnerArg>,
        /// Number of samples per (task, runner) pair. Default 1 for quick dev,
        /// use 10+ for statistically meaningful reports.
        #[arg(long, default_value_t = 1)]
        samples: usize,
    },
    /// Run security-track only: discover tasks with `kind: security` and
    /// compute block-rate / precision / recall / F1 per runner.
    Security {
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "argentor,langchain,crewai,pydantic-ai,claude-agent-sdk"
        )]
        runners: Vec<RunnerArg>,
        /// Number of samples per (task, runner) pair.
        #[arg(long, default_value_t = 1)]
        samples: usize,
    },
    /// Run cost-track only: discover `kind: cost` tasks and compute
    /// prompt-tokens-sent + dollar-cost per runner + scale projections.
    Cost {
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "argentor,langchain,crewai,pydantic-ai,claude-agent-sdk"
        )]
        runners: Vec<RunnerArg>,
        /// Number of samples per (task, runner) pair. Cost simulation is
        /// deterministic so 1 is plenty — higher values validate consistency.
        #[arg(long, default_value_t = 1)]
        samples: usize,
        /// Workload scale for dollar projections (small | mid | large | enterprise).
        #[arg(long, default_value = "mid")]
        scale: String,
        /// Pricing model (used for $/task, $/day, $/month, $/year).
        #[arg(long, default_value = "claude-sonnet-4")]
        pricing_model: String,
    },
    /// Run long-horizon-track only: discover `kind: long_horizon` tasks and
    /// measure turns-to-completion, token accumulation, goal drift, and
    /// memory recall rate per runner.
    LongHorizon {
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "argentor,langchain,crewai,pydantic-ai,claude-agent-sdk"
        )]
        runners: Vec<RunnerArg>,
        /// Number of samples per (task, runner) pair. 1 is sufficient for the
        /// deterministic simulation path.
        #[arg(long, default_value_t = 1)]
        samples: usize,
    },
    /// Run multi-agent track only: discover `kind: multi_agent` tasks and
    /// measure completion rate, total turns, total tokens, and coordination
    /// overhead per runner.
    MultiAgent {
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "argentor,langchain,crewai,pydantic-ai,claude-agent-sdk"
        )]
        runners: Vec<RunnerArg>,
        /// Number of samples per (task, runner) pair. 1 is sufficient for
        /// the deterministic simulation path.
        #[arg(long, default_value_t = 1)]
        samples: usize,
    },
    /// Generate the static benchmark dashboard from `benchmarks/results/*.json`.
    /// Writes `benchmarks/dashboard/index.html`.
    Dashboard {
        /// Directory containing benchmark result JSON files.
        #[arg(long, default_value = "benchmarks/results")]
        results_dir: PathBuf,
        /// Output path for the generated HTML file.
        #[arg(long, default_value = "benchmarks/dashboard/index.html")]
        output: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RunnerArg {
    Argentor,
    Langchain,
    Crewai,
    PydanticAi,
    ClaudeAgentSdk,
    Mock,
}

impl RunnerArg {
    /// Whether this runner is Argentor (used to flip intelligence on for cost).
    fn is_argentor(&self) -> bool {
        matches!(self, RunnerArg::Argentor)
    }

    #[allow(dead_code)]
    fn kind(&self) -> RunnerKind {
        match self {
            RunnerArg::Argentor => RunnerKind::Argentor,
            RunnerArg::Langchain => RunnerKind::Langchain,
            RunnerArg::Crewai => RunnerKind::Crewai,
            RunnerArg::PydanticAi => RunnerKind::PydanticAi,
            RunnerArg::ClaudeAgentSdk => RunnerKind::ClaudeAgentSdk,
            RunnerArg::Mock => RunnerKind::Mock,
        }
    }

    fn build(&self, intelligence: bool) -> Box<dyn Runner> {
        match self {
            RunnerArg::Argentor => {
                let r = ArgentorRunner::new();
                if intelligence {
                    Box::new(r.with_intelligence())
                } else {
                    Box::new(r)
                }
            }
            RunnerArg::Langchain => {
                let cmd = std::env::var("ARGENTOR_LC_RUNNER")
                    .unwrap_or_else(|_| "argentor-lc-runner".to_string());
                Box::new(ExternalRunner::new(
                    cmd,
                    RunnerKind::Langchain,
                    "langchain v0.3 (mock-llm)",
                ))
            }
            RunnerArg::Crewai => {
                let cmd = std::env::var("ARGENTOR_CREWAI_RUNNER")
                    .unwrap_or_else(|_| "argentor-crewai-runner".to_string());
                Box::new(ExternalRunner::new(
                    cmd,
                    RunnerKind::Crewai,
                    "crewai v0.100 (mock-llm)",
                ))
            }
            RunnerArg::PydanticAi => {
                let cmd = std::env::var("ARGENTOR_PYDANTIC_AI_RUNNER")
                    .unwrap_or_else(|_| "argentor-pydantic-ai-runner".to_string());
                Box::new(ExternalRunner::new(
                    cmd,
                    RunnerKind::PydanticAi,
                    "pydantic-ai v0.5 (mock-llm)",
                ))
            }
            RunnerArg::ClaudeAgentSdk => {
                let cmd = std::env::var("ARGENTOR_CLAUDE_AGENT_SDK_RUNNER")
                    .unwrap_or_else(|_| "argentor-claude-agent-sdk-runner".to_string());
                Box::new(ExternalRunner::new(
                    cmd,
                    RunnerKind::ClaudeAgentSdk,
                    "claude-agent-sdk v0.2 (mock-llm)",
                ))
            }
            RunnerArg::Mock => Box::new(MockRunner::new()),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::List => {
            let tasks = Task::discover(&cli.tasks_dir)
                .with_context(|| format!("discovering tasks in {:?}", cli.tasks_dir))?;
            if tasks.is_empty() {
                println!("No tasks found in {:?}", cli.tasks_dir);
            } else {
                println!("Discovered {} tasks:", tasks.len());
                for (t, _) in &tasks {
                    println!("  {:<24} — {}", t.id, t.description);
                }
            }
        }
        Command::Run {
            task,
            runner,
            intelligence,
        } => {
            let task_yaml = cli.tasks_dir.join(&task).join("task.yaml");
            let (t, dir) =
                Task::load_yaml(&task_yaml).with_context(|| format!("loading {:?}", task_yaml))?;
            let r = runner.build(intelligence);
            println!("Running {} on {}", t.id, r.name());
            let result = r.run(&t, &dir).await?;
            let m = metrics::compute(&t, &result);
            let report = RunReport::new(vec![m]);
            println!("\n{}", report.to_markdown());
        }
        Command::RunAll { runners, samples } => {
            let tasks = Task::discover(&cli.tasks_dir)?;
            if tasks.is_empty() {
                anyhow::bail!("no tasks found in {:?}", cli.tasks_dir);
            }

            println!(
                "Running {} tasks × {} runners × {} samples = {} total runs",
                tasks.len(),
                runners.len(),
                samples,
                tasks.len() * runners.len() * samples
            );

            let mut all_metrics = Vec::new();
            let mut latency_by_combo: HashMap<(String, String), Vec<f64>> = HashMap::new();

            for (task, dir) in &tasks {
                for r_arg in &runners {
                    let runner_box = r_arg.build(false);
                    let runner_name = runner_box.name();
                    println!("▶ {}  [{}] × {}", task.id, runner_name, samples);

                    for sample_idx in 0..samples {
                        let r = r_arg.build(false);
                        let result = r.run(task, dir).await?;
                        let m = metrics::compute(task, &result);

                        latency_by_combo
                            .entry((task.id.clone(), runner_name.clone()))
                            .or_default()
                            .push(m.latency.wall_ms as f64);

                        if sample_idx == 0 {
                            all_metrics.push(m);
                        }
                    }
                }
            }

            let report = RunReport::new(all_metrics);
            println!("\n{}", report.to_markdown());

            if samples > 1 {
                println!("\n## Latency stats (N={samples})\n");
                println!("| Task | Runner | Mean | Median | Stddev | Min | Max | P95 | P99 |");
                println!("|------|--------|------|--------|--------|-----|-----|-----|-----|");
                let mut keys: Vec<_> = latency_by_combo.keys().collect();
                keys.sort();
                for (task_id, runner_name) in keys {
                    let samples = &latency_by_combo[&(task_id.clone(), runner_name.clone())];
                    let s = Stats::from_samples(samples);
                    println!(
                        "| `{}` | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
                        task_id,
                        runner_name,
                        s.mean,
                        s.median,
                        s.stddev,
                        s.min,
                        s.max,
                        s.p95,
                        s.p99,
                    );
                }

                // Paired t-tests: Argentor vs each other runner, per task
                let argentor_samples: HashMap<String, &Vec<f64>> = latency_by_combo
                    .iter()
                    .filter(|((_, r), _)| r.starts_with("argentor"))
                    .map(|((t, _), v)| (t.clone(), v))
                    .collect();

                if !argentor_samples.is_empty() {
                    println!("\n## Paired t-test (Argentor vs competitors)\n");
                    println!(
                        "| Task | Competitor | N | Argentor mean | Competitor mean | Diff | p-value | Signif | Effect |"
                    );
                    println!(
                        "|------|------------|---|---------------|-----------------|------|---------|--------|--------|"
                    );
                    for ((task_id, runner_name), samples) in &latency_by_combo {
                        if runner_name.starts_with("argentor") {
                            continue;
                        }
                        if let Some(ag_samples) = argentor_samples.get(task_id) {
                            if let Some(t) = PairedTTest::compute(ag_samples, samples) {
                                let sig = if t.is_significant() { "✓" } else { "✗" };
                                println!(
                                    "| `{}` | {} | {} | {:.1} | {:.1} | {:+.1} | {:.4} | {} | {} |",
                                    task_id,
                                    runner_name,
                                    t.n,
                                    ag_samples.iter().sum::<f64>() / t.n as f64,
                                    samples.iter().sum::<f64>() / t.n as f64,
                                    t.mean_diff,
                                    t.p_value,
                                    sig,
                                    t.effect_label(),
                                );
                            }
                        }
                    }
                }
            }

            let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let out = cli
                .tasks_dir
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("results")
                .join(format!("run_{ts}.json"));
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            // Flatten tuple keys into strings so JSON serialization works
            let flat_samples: serde_json::Map<String, serde_json::Value> = latency_by_combo
                .iter()
                .map(|((task_id, runner_name), samples)| {
                    (
                        format!("{task_id} :: {runner_name}"),
                        serde_json::to_value(samples).unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            let payload = serde_json::json!({
                "summary": report,
                "samples_per_combo": samples,
                "latency_samples_ms": flat_samples,
            });
            std::fs::write(&out, serde_json::to_string_pretty(&payload)?)?;
            println!("\nResults written to {}", out.display());
        }
        Command::Security { runners, samples } => {
            run_security(&cli.tasks_dir, &runners, samples).await?;
        }
        Command::Cost {
            runners,
            samples,
            scale,
            pricing_model,
        } => {
            run_cost(&cli.tasks_dir, &runners, samples, &scale, &pricing_model).await?;
        }
        Command::LongHorizon { runners, samples } => {
            run_long_horizon(&cli.tasks_dir, &runners, samples).await?;
        }
        Command::MultiAgent { runners, samples } => {
            run_multi_agent(&cli.tasks_dir, &runners, samples).await?;
        }
        Command::Dashboard {
            results_dir,
            output,
        } => {
            println!("Generating dashboard from {:?} ...", results_dir);
            dashboard_gen::generate(&results_dir, &output)?;
            println!("Dashboard written to {}", output.display());
        }
    }

    Ok(())
}

/// Security-track runner. Discovers security tasks (kind == Security) and
/// computes block-rate / precision / recall / F1 per runner.
async fn run_security(
    tasks_dir: &std::path::Path,
    runners: &[RunnerArg],
    samples: usize,
) -> anyhow::Result<()> {
    let all_tasks =
        Task::discover(tasks_dir).with_context(|| format!("discovering tasks in {tasks_dir:?}"))?;
    let security_tasks: Vec<_> = all_tasks
        .into_iter()
        .filter(|(t, _)| t.kind == TaskKind::Security)
        .collect();

    if security_tasks.is_empty() {
        anyhow::bail!(
            "no security tasks found in {:?} (looking for kind: security)",
            tasks_dir
        );
    }

    println!(
        "Running {} security tasks × {} runners × {} samples = {} total runs",
        security_tasks.len(),
        runners.len(),
        samples,
        security_tasks.len() * runners.len() * samples
    );

    // Collect all raw results per runner (across all samples) so we can
    // compute the aggregate BlockRateMetric afterwards.
    let mut results_by_runner: HashMap<String, Vec<TaskResult>> = HashMap::new();
    // Track which category each task falls into for the per-category breakdown.
    let mut category_of: HashMap<String, &'static str> = HashMap::new();

    for (task, dir) in &security_tasks {
        let category = if task.id.starts_with("sec_inj_") {
            "injection"
        } else if task.id.starts_with("sec_pii_") {
            "pii"
        } else if task.id.starts_with("sec_cmd_") {
            "command"
        } else {
            "other"
        };
        category_of.insert(task.id.clone(), category);

        for r_arg in runners {
            let runner_box = r_arg.build(false);
            let runner_name = runner_box.name();
            println!("▶ {}  [{}] × {}", task.id, runner_name, samples);
            for _ in 0..samples {
                let r = r_arg.build(false);
                let result = r.run(task, dir).await?;
                results_by_runner
                    .entry(runner_name.clone())
                    .or_default()
                    .push(result);
            }
        }
    }

    // Print the overall per-runner table.
    println!("\n## Security block-rate results\n");
    println!(
        "| Runner | Tasks | TP | TN | FP | FN | Block rate | Precision | Recall | F1 | Accuracy |"
    );
    println!(
        "|--------|-------|----|----|----|----|-----------|-----------|--------|----|----------|"
    );

    let mut runner_names: Vec<_> = results_by_runner.keys().cloned().collect();
    runner_names.sort();

    for runner_name in &runner_names {
        let results = &results_by_runner[runner_name];
        // Compute aggregate by treating each result individually as a
        // classification attempt. Build a task lookup so we don't lose the
        // `expected_blocked` label.
        let tasks_only: Vec<Task> = security_tasks.iter().map(|(t, _)| t.clone()).collect();
        let metric = aggregate_block_rate(&tasks_only, results);
        println!(
            "| {} | {} | {} | {} | {} | {} | {:.1}% | {:.2} | {:.2} | {:.2} | {:.2} |",
            runner_name,
            metric.total(),
            metric.blocked_correctly,
            metric.allowed_correctly,
            metric.false_positives,
            metric.false_negatives,
            metric.block_rate_pct(),
            metric.precision(),
            metric.recall(),
            metric.f1(),
            metric.accuracy(),
        );
    }

    // Per-category breakdown.
    println!("\n## Per-category breakdown\n");
    println!("| Runner | Category | TP | FN | Block rate |");
    println!("|--------|----------|----|----|-----------|");
    for runner_name in &runner_names {
        let results = &results_by_runner[runner_name];
        for category in ["injection", "pii", "command"] {
            let (tp, fn_count) = category_stats(&security_tasks, results, &category_of, category);
            let denom = tp + fn_count;
            let rate = if denom == 0 {
                0.0
            } else {
                tp as f32 / denom as f32 * 100.0
            };
            println!(
                "| {} | {} | {} | {} | {:.1}% |",
                runner_name, category, tp, fn_count, rate
            );
        }
    }

    // Per-task detail (first sample only).
    println!("\n## Per-task detail (first sample)\n");
    println!("| Task | Expected | Runner | Was blocked | Correct | Reason |");
    println!("|------|----------|--------|-------------|---------|--------|");
    for (task, _) in &security_tasks {
        let expected = task.expected_blocked.unwrap_or(false);
        for runner_name in &runner_names {
            let results = &results_by_runner[runner_name];
            if let Some(r) = results.iter().find(|r| r.task_id == task.id) {
                let correct = r.was_blocked == expected;
                let reason = r
                    .block_reason
                    .clone()
                    .unwrap_or_else(|| "-".to_string())
                    .replace('|', "\\|");
                println!(
                    "| `{}` | {} | {} | {} | {} | {} |",
                    task.id,
                    if expected { "block" } else { "allow" },
                    runner_name,
                    if r.was_blocked { "yes" } else { "no" },
                    if correct { "✓" } else { "✗" },
                    reason,
                );
            }
        }
    }

    // Persist JSON results.
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let out = tasks_dir
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("results")
        .join(format!("security_{ts}.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let payload = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "results_by_runner": results_by_runner,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&payload)?)?;
    println!("\nResults written to {}", out.display());

    Ok(())
}

/// Aggregate block-rate across all samples (each sample is one classification).
fn aggregate_block_rate(tasks: &[Task], results: &[TaskResult]) -> BlockRateMetric {
    let mut agg = BlockRateMetric::default();
    for res in results {
        // Find the matching task to read expected_blocked.
        let Some(task) = tasks.iter().find(|t| t.id == res.task_id) else {
            continue;
        };
        let Some(expected) = task.expected_blocked else {
            continue;
        };
        match (expected, res.was_blocked) {
            (true, true) => agg.blocked_correctly += 1,
            (true, false) => agg.false_negatives += 1,
            (false, false) => agg.allowed_correctly += 1,
            (false, true) => agg.false_positives += 1,
        }
    }
    agg
}

/// Count TP / FN for adversarial inputs in a given category across all samples.
fn category_stats(
    tasks: &[(Task, std::path::PathBuf)],
    results: &[TaskResult],
    category_of: &HashMap<String, &'static str>,
    category: &str,
) -> (u32, u32) {
    let mut tp: u32 = 0;
    let mut fn_count: u32 = 0;
    for res in results {
        let Some(cat) = category_of.get(&res.task_id) else {
            continue;
        };
        if *cat != category {
            continue;
        }
        let Some((task, _)) = tasks.iter().find(|(t, _)| t.id == res.task_id) else {
            continue;
        };
        match (task.expected_blocked, res.was_blocked) {
            (Some(true), true) => tp += 1,
            (Some(true), false) => fn_count += 1,
            _ => {}
        }
    }
    (tp, fn_count)
}

// Silence unused-import warning when compute_block_rate is unused in main
// (kept re-exported at the library level for programmatic callers).
#[allow(dead_code)]
fn _ensure_compute_block_rate_is_exported() -> BlockRateMetric {
    compute_block_rate(&[], &[])
}

/// Cost-track runner. Discovers `kind: cost` tasks, runs each runner (which
/// short-circuits into the deterministic cost simulator), then prints a
/// per-task breakdown plus a $/task × scale projection table.
async fn run_cost(
    tasks_dir: &std::path::Path,
    runners: &[RunnerArg],
    samples: usize,
    scale_str: &str,
    pricing_model: &str,
) -> anyhow::Result<()> {
    let scale = Scale::parse(scale_str).with_context(|| {
        format!("invalid scale '{scale_str}' (expected small|mid|large|enterprise)")
    })?;

    let all_tasks =
        Task::discover(tasks_dir).with_context(|| format!("discovering tasks in {tasks_dir:?}"))?;
    let cost_tasks: Vec<_> = all_tasks
        .into_iter()
        .filter(|(t, _)| t.kind == TaskKind::Cost)
        .collect();

    if cost_tasks.is_empty() {
        anyhow::bail!(
            "no cost tasks found in {:?} (looking for kind: cost)",
            tasks_dir
        );
    }

    println!(
        "Running {} cost tasks × {} runners × {} samples = {} runs  (scale: {})",
        cost_tasks.len(),
        runners.len(),
        samples,
        cost_tasks.len() * runners.len() * samples,
        scale.label()
    );

    // Results: (task_id, runner_name) -> list of TaskResult across samples.
    let mut results_by_combo: HashMap<(String, String), Vec<TaskResult>> = HashMap::new();
    // Ordered list of runner display names so the output tables are stable.
    let mut runner_display: Vec<String> = Vec::new();

    for (task, dir) in &cost_tasks {
        for r_arg in runners {
            let runner_box = r_arg.build(r_arg.is_argentor());
            let runner_name = runner_box.name();
            if !runner_display.contains(&runner_name) {
                runner_display.push(runner_name.clone());
            }
            println!("▶ {}  [{}] × {}", task.id, runner_name, samples);
            for _ in 0..samples {
                let r = r_arg.build(r_arg.is_argentor());
                let result = r.run(task, dir).await?;
                results_by_combo
                    .entry((task.id.clone(), runner_name.clone()))
                    .or_default()
                    .push(result);
            }
        }
    }

    // Per-task breakdown table: for each task × runner, mean prompt tokens
    // sent (across samples) + component breakdown + $/task.
    println!("\n## Per-task cost breakdown\n");
    println!("Pricing model: `{pricing_model}`  (input rate applied to prompt_tokens_sent)\n");
    println!(
        "| Task | Runner | Turns | Tools | Ctx(KB) | Tokens sent | Tool tok | History tok | $/task |"
    );
    println!(
        "|------|--------|-------|-------|---------|-------------|----------|-------------|--------|"
    );

    // Sort tasks by id for stable output.
    let mut sorted_tasks: Vec<_> = cost_tasks.iter().collect();
    sorted_tasks.sort_by(|a, b| a.0.id.cmp(&b.0.id));

    // Aggregate per-runner sums (across all tasks) for the scale projection.
    let mut total_tokens_per_runner: HashMap<String, u64> = HashMap::new();
    let mut total_output_per_runner: HashMap<String, u64> = HashMap::new();
    let mut total_dollars_per_runner: HashMap<String, f64> = HashMap::new();

    for (task, _) in &sorted_tasks {
        for runner_name in &runner_display {
            let Some(results) = results_by_combo.get(&(task.id.clone(), runner_name.clone()))
            else {
                continue;
            };
            if results.is_empty() {
                continue;
            }
            let n = results.len() as f64;
            let mean_tokens = results.iter().map(|r| r.prompt_tokens_sent).sum::<u64>() as f64 / n;
            let mean_tool = results
                .iter()
                .map(|r| r.tool_description_tokens)
                .sum::<u64>() as f64
                / n;
            let mean_hist = results
                .iter()
                .map(|r| r.context_history_tokens)
                .sum::<u64>() as f64
                / n;
            let mean_output = results.iter().map(|r| r.output_tokens).sum::<u64>() as f64 / n;

            let cost = cost_metric::compute(pricing_model, mean_tokens as u64, mean_output as u64);

            println!(
                "| `{}` | {} | {} | {} | {:.1} | {:.0} | {:.0} | {:.0} | ${:.6} |",
                task.id,
                runner_name,
                task.simulated_turns,
                task.tool_count,
                task.context_size_bytes as f64 / 1024.0,
                mean_tokens,
                mean_tool,
                mean_hist,
                cost.total_usd,
            );

            *total_tokens_per_runner
                .entry(runner_name.clone())
                .or_insert(0) += mean_tokens as u64;
            *total_output_per_runner
                .entry(runner_name.clone())
                .or_insert(0) += mean_output as u64;
            *total_dollars_per_runner
                .entry(runner_name.clone())
                .or_insert(0.0) += cost.total_usd;
        }
    }

    // Scale projection: $/task (mean across the whole suite), scaled up.
    let task_count = sorted_tasks.len() as f64;
    let rpd = scale.requests_per_day();

    println!(
        "\n## Scale projection — {} ({} req/day)\n",
        scale.label(),
        rpd
    );
    println!("| Runner | tokens/task (mean) | $/task | $/day | $/month | $/year |");
    println!("|--------|-------------------|--------|-------|---------|--------|");

    // Sort runners so Argentor shows first.
    let mut runners_sorted = runner_display.clone();
    runners_sorted.sort_by(|a, b| {
        let a_arg = a.starts_with("argentor");
        let b_arg = b.starts_with("argentor");
        b_arg.cmp(&a_arg).then(a.cmp(b))
    });

    for runner_name in &runners_sorted {
        let total_tokens = *total_tokens_per_runner.get(runner_name).unwrap_or(&0);
        let total_dollars = *total_dollars_per_runner.get(runner_name).unwrap_or(&0.0);
        let mean_tokens = total_tokens as f64 / task_count;
        let mean_dollars = total_dollars / task_count;

        let per_day = cost_metric::project_daily(mean_dollars, rpd);
        let per_month = cost_metric::project_monthly(mean_dollars, rpd);
        let per_year = cost_metric::project_annual(mean_dollars, rpd);

        println!(
            "| {} | {:.0} | ${:.6} | ${:.2} | ${:.2} | ${:.2} |",
            runner_name, mean_tokens, mean_dollars, per_day, per_month, per_year,
        );
    }

    // Argentor-vs-competitor ratios (tokens / dollars).
    let argentor_tokens: Option<f64> = runners_sorted
        .iter()
        .find(|n| n.starts_with("argentor"))
        .map(|n| *total_tokens_per_runner.get(n).unwrap_or(&0) as f64 / task_count);

    if let Some(ag_tok) = argentor_tokens {
        println!("\n## Argentor savings vs competitors\n");
        println!("| Competitor | tokens/task | Argentor tokens | Savings | Ratio |");
        println!("|------------|-------------|-----------------|---------|-------|");
        for runner_name in &runners_sorted {
            if runner_name.starts_with("argentor") {
                continue;
            }
            let comp_tok =
                *total_tokens_per_runner.get(runner_name).unwrap_or(&0) as f64 / task_count;
            let savings = comp_tok - ag_tok;
            let ratio = if ag_tok > 0.0 { comp_tok / ag_tok } else { 0.0 };
            println!(
                "| {} | {:.0} | {:.0} | {:.0} ({:.1}%) | {:.2}× |",
                runner_name,
                comp_tok,
                ag_tok,
                savings,
                if comp_tok > 0.0 {
                    savings / comp_tok * 100.0
                } else {
                    0.0
                },
                ratio,
            );
        }
    }

    // Persist JSON results.
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let out = tasks_dir
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("results")
        .join(format!("cost_{ts}.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // Flatten tuple keys for JSON.
    let flat: serde_json::Map<String, serde_json::Value> = results_by_combo
        .iter()
        .map(|((task_id, runner_name), results)| {
            (
                format!("{task_id} :: {runner_name}"),
                serde_json::to_value(results).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    let payload = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "scale": scale.label(),
        "requests_per_day": rpd,
        "pricing_model": pricing_model,
        "results_by_combo": flat,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&payload)?)?;
    println!("\nResults written to {}", out.display());

    Ok(())
}

/// Long-horizon track runner. Discovers `kind: long_horizon` tasks, runs each
/// runner, then prints a comparison table of token accumulation, memory recall,
/// and goal drift — the key metrics for long-horizon agent quality.
async fn run_long_horizon(
    tasks_dir: &std::path::Path,
    runners: &[RunnerArg],
    samples: usize,
) -> anyhow::Result<()> {
    let all_tasks =
        Task::discover(tasks_dir).with_context(|| format!("discovering tasks in {tasks_dir:?}"))?;
    let lh_tasks: Vec<_> = all_tasks
        .into_iter()
        .filter(|(t, _)| t.kind == TaskKind::LongHorizon)
        .collect();

    if lh_tasks.is_empty() {
        anyhow::bail!(
            "no long-horizon tasks found in {:?} (looking for kind: long_horizon)",
            tasks_dir
        );
    }

    println!(
        "Running {} long-horizon tasks × {} runners × {} samples = {} total runs",
        lh_tasks.len(),
        runners.len(),
        samples,
        lh_tasks.len() * runners.len() * samples
    );

    // Collect per-(task, runner) results across all samples.
    let mut results_by_combo: HashMap<(String, String), Vec<TaskResult>> = HashMap::new();
    let mut runner_display: Vec<String> = Vec::new();

    for (task, dir) in &lh_tasks {
        for r_arg in runners {
            let runner_box = r_arg.build(r_arg.is_argentor());
            let runner_name = runner_box.name();
            if !runner_display.contains(&runner_name) {
                runner_display.push(runner_name.clone());
            }
            println!("▶ {}  [{}] × {}", task.id, runner_name, samples);
            for _ in 0..samples {
                let r = r_arg.build(r_arg.is_argentor());
                let result = r.run(task, dir).await?;
                results_by_combo
                    .entry((task.id.clone(), runner_name.clone()))
                    .or_default()
                    .push(result);
            }
        }
    }

    // Compute per-task long-horizon metrics (averaged across samples).
    // We use the first sample's result for the metric computation since the
    // simulation is deterministic; for multiple samples we average tokens.
    let mut metrics_by_runner: HashMap<String, Vec<lh_metric::LongHorizonMetrics>> = HashMap::new();

    println!("\n## Per-task long-horizon results\n");
    println!(
        "| Task | Runner | Turns | Tokens | Tok@T10 | Recall | Drift | Checkpoints | Success |"
    );
    println!(
        "|------|--------|-------|--------|---------|--------|-------|-------------|---------|"
    );

    let mut sorted_tasks: Vec<_> = lh_tasks.iter().collect();
    sorted_tasks.sort_by(|a, b| a.0.id.cmp(&b.0.id));

    for (task, _) in &sorted_tasks {
        for runner_name in &runner_display {
            let Some(results) = results_by_combo.get(&(task.id.clone(), runner_name.clone()))
            else {
                continue;
            };
            if results.is_empty() {
                continue;
            }
            // Use first sample result for qualitative metrics; average tokens.
            let mut m = lh_metric::compute(task, &results[0]);
            if results.len() > 1 {
                let mean_tokens = results.iter().map(|r| r.prompt_tokens_sent).sum::<u64>()
                    / results.len() as u64;
                m.tokens_accumulated = mean_tokens;
                m.tokens_at_turn_10 = if m.turns_used >= 10 {
                    mean_tokens
                } else if m.turns_used == 0 {
                    0
                } else {
                    mean_tokens / m.turns_used as u64 * 10
                };
            }

            let total_checkpoints = task
                .memory_checkpoints
                .as_deref()
                .map(|v| v.len())
                .unwrap_or(0);

            println!(
                "| `{}` | {} | {} | {} | {} | {:.0}% | {:.1} | {}/{} | {} |",
                task.id,
                runner_name,
                m.turns_used,
                m.tokens_accumulated,
                m.tokens_at_turn_10,
                m.memory_recall_rate * 100.0,
                m.goal_drift_score,
                m.checkpoints_hit,
                total_checkpoints,
                if m.success { "✓" } else { "✗" },
            );

            metrics_by_runner
                .entry(runner_name.clone())
                .or_default()
                .push(m);
        }
    }

    // Summary table: per-runner aggregate.
    println!("\n## Summary — tokens at turn 10 (canonical cross-framework comparison)\n");
    println!(
        "| Runner | Tasks | Succeeded | Mean turns | Tok@T10 (mean) | Recall (mean) | Drift (mean) | Compaction savings |"
    );
    println!(
        "|--------|-------|-----------|------------|---------------|---------------|--------------|-------------------|"
    );

    // Sort: Argentor first, then alphabetical.
    let mut runners_sorted = runner_display.clone();
    runners_sorted.sort_by(|a, b| {
        let a_ag = a.starts_with("argentor");
        let b_ag = b.starts_with("argentor");
        b_ag.cmp(&a_ag).then(a.cmp(b))
    });

    let mut summaries: Vec<LongHorizonSummary> = Vec::new();
    for runner_name in &runners_sorted {
        let empty = Vec::new();
        let ms = metrics_by_runner.get(runner_name).unwrap_or(&empty);
        let s = LongHorizonSummary::aggregate(runner_name, ms);
        println!(
            "| {} | {} | {} | {:.1} | {:.0} | {:.0}% | {:.1} | {:+.1}% |",
            runner_name,
            s.tasks_run,
            s.tasks_succeeded,
            s.mean_turns,
            s.mean_tokens_at_turn_10,
            s.mean_memory_recall_rate * 100.0,
            s.mean_goal_drift_score,
            s.mean_compaction_savings_pct,
        );
        summaries.push(s);
    }

    // Argentor vs competitors: token savings at turn 10.
    let argentor_tok: Option<f64> = summaries
        .iter()
        .find(|s| s.runner.starts_with("argentor"))
        .map(|s| s.mean_tokens_at_turn_10);

    if let Some(ag_tok) = argentor_tok {
        println!("\n## Argentor savings vs competitors (tokens at turn 10)\n");
        println!("| Competitor | Tok@T10 | Argentor Tok@T10 | Savings | Ratio |");
        println!("|------------|---------|-----------------|---------|-------|");
        for s in &summaries {
            if s.runner.starts_with("argentor") {
                continue;
            }
            let comp_tok = s.mean_tokens_at_turn_10;
            let savings = comp_tok - ag_tok;
            let ratio = if ag_tok > 0.0 { comp_tok / ag_tok } else { 0.0 };
            println!(
                "| {} | {:.0} | {:.0} | {:.0} ({:.1}%) | {:.2}× |",
                s.runner,
                comp_tok,
                ag_tok,
                savings,
                if comp_tok > 0.0 {
                    savings / comp_tok * 100.0
                } else {
                    0.0
                },
                ratio,
            );
        }
    }

    // Family breakdown: repair / research / state.
    println!("\n## By task family\n");
    println!("| Family | Runner | Tok@T10 (mean) | Recall (mean) |");
    println!("|--------|--------|---------------|---------------|");
    for family in ["lh_repair", "lh_research", "lh_state"] {
        for runner_name in &runners_sorted {
            let empty = Vec::new();
            let ms = metrics_by_runner.get(runner_name).unwrap_or(&empty);
            let family_ms: Vec<_> = ms
                .iter()
                .filter(|m| m.task_id.starts_with(family))
                .collect();
            if family_ms.is_empty() {
                continue;
            }
            let n = family_ms.len() as f64;
            let mean_tok = family_ms
                .iter()
                .map(|m| m.tokens_at_turn_10 as f64)
                .sum::<f64>()
                / n;
            let mean_recall = family_ms
                .iter()
                .map(|m| m.memory_recall_rate as f64)
                .sum::<f64>()
                / n;
            let family_label = match family {
                "lh_repair" => "code_repair",
                "lh_research" => "multi_step_research",
                "lh_state" => "stateful_conversation",
                _ => family,
            };
            println!(
                "| {} | {} | {:.0} | {:.0}% |",
                family_label,
                runner_name,
                mean_tok,
                mean_recall * 100.0,
            );
        }
    }

    // Persist JSON results.
    let ts_lh = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let out = tasks_dir
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("results")
        .join(format!("long_horizon_{ts_lh}.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let flat_lh: serde_json::Map<String, serde_json::Value> = results_by_combo
        .iter()
        .map(|((task_id, runner_name), results)| {
            (
                format!("{task_id} :: {runner_name}"),
                serde_json::to_value(results).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    let payload_lh = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "results_by_combo": flat_lh,
        "summaries": summaries,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&payload_lh)?)?;
    println!("\nResults written to {}", out.display());

    Ok(())
}

/// Multi-agent track runner. Discovers `kind: multi_agent` tasks, runs each
/// runner, then prints a comparison table of completion rate, total turns,
/// total tokens, and coordination overhead per pattern.
async fn run_multi_agent(
    tasks_dir: &std::path::Path,
    runners: &[RunnerArg],
    samples: usize,
) -> anyhow::Result<()> {
    let all_tasks =
        Task::discover(tasks_dir).with_context(|| format!("discovering tasks in {tasks_dir:?}"))?;
    let ma_tasks: Vec<_> = all_tasks
        .into_iter()
        .filter(|(t, _)| t.kind == TaskKind::MultiAgent)
        .collect();

    if ma_tasks.is_empty() {
        anyhow::bail!(
            "no multi-agent tasks found in {:?} (looking for kind: multi_agent)",
            tasks_dir
        );
    }

    println!(
        "Running {} multi-agent tasks × {} runners × {} samples = {} total runs",
        ma_tasks.len(),
        runners.len(),
        samples,
        ma_tasks.len() * runners.len() * samples
    );

    let mut results_by_combo: HashMap<(String, String), Vec<TaskResult>> = HashMap::new();
    let mut runner_display: Vec<String> = Vec::new();

    for (task, dir) in &ma_tasks {
        for r_arg in runners {
            let runner_box = r_arg.build(r_arg.is_argentor());
            let runner_name = runner_box.name();
            if !runner_display.contains(&runner_name) {
                runner_display.push(runner_name.clone());
            }
            println!(
                "▶ {}  [{}] (agents={}, pattern={}) × {}",
                task.id, runner_name, task.agent_count, task.pattern, samples
            );
            for _ in 0..samples {
                let r = r_arg.build(r_arg.is_argentor());
                let result = r.run(task, dir).await?;
                results_by_combo
                    .entry((task.id.clone(), runner_name.clone()))
                    .or_default()
                    .push(result);
            }
        }
    }

    // Per-task table
    println!("\n## Per-task multi-agent results\n");
    println!(
        "| Task | Pattern | Agents | Runner | Completion | Turns | Tokens | Coord. overhead | Success |"
    );
    println!(
        "|------|---------|--------|--------|------------|-------|--------|-----------------|---------|"
    );

    let mut sorted_tasks: Vec<_> = ma_tasks.iter().collect();
    sorted_tasks.sort_by(|a, b| a.0.id.cmp(&b.0.id));

    let mut metrics_by_runner: HashMap<String, Vec<ma_metric::MultiAgentMetrics>> = HashMap::new();

    for (task, _) in &sorted_tasks {
        for runner_name in &runner_display {
            let Some(results) = results_by_combo.get(&(task.id.clone(), runner_name.clone()))
            else {
                continue;
            };
            if results.is_empty() {
                continue;
            }
            let m = ma_metric::compute(task, &results[0]);
            println!(
                "| `{}` | {} | {} | {} | {:.0}% | {} | {} | {} | {} |",
                task.id,
                m.pattern,
                m.agent_count,
                runner_name,
                m.completion_rate * 100.0,
                m.total_turns,
                m.total_tokens,
                m.coordination_overhead,
                if m.success { "✓" } else { "✗" },
            );
            metrics_by_runner
                .entry(runner_name.clone())
                .or_default()
                .push(m);
        }
    }

    // Summary table
    println!("\n## Summary — multi-agent runner comparison\n");
    println!(
        "| Runner | Tasks | Succeeded | Completion | Total turns | Total tokens | Coord. overhead | Wall ms |"
    );
    println!(
        "|--------|-------|-----------|------------|-------------|--------------|-----------------|---------|"
    );

    let mut runners_sorted = runner_display.clone();
    runners_sorted.sort_by(|a, b| {
        let a_ag = a.starts_with("argentor");
        let b_ag = b.starts_with("argentor");
        b_ag.cmp(&a_ag).then(a.cmp(b))
    });

    let mut summaries: Vec<MultiAgentSummary> = Vec::new();
    for runner_name in &runners_sorted {
        let empty = Vec::new();
        let ms = metrics_by_runner.get(runner_name).unwrap_or(&empty);
        let s = MultiAgentSummary::aggregate(runner_name, ms);
        println!(
            "| {} | {} | {} | {:.0}% | {:.1} | {:.0} | {:.0} | {:.1} |",
            runner_name,
            s.tasks_run,
            s.tasks_succeeded,
            s.mean_completion_rate * 100.0,
            s.mean_total_turns,
            s.mean_total_tokens,
            s.mean_coordination_overhead,
            s.mean_wall_time_ms,
        );
        summaries.push(s);
    }

    // Persist JSON
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let out = tasks_dir
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("results")
        .join(format!("multi_agent_{ts}.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let flat: serde_json::Map<String, serde_json::Value> = results_by_combo
        .iter()
        .map(|((task_id, runner_name), results)| {
            (
                format!("{task_id} :: {runner_name}"),
                serde_json::to_value(results).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    let payload = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "results_by_combo": flat,
        "summaries": summaries,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&payload)?)?;
    println!("\nResults written to {}", out.display());

    Ok(())
}

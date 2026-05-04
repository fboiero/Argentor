// SPDX-License-Identifier: AGPL-3.0-only
//! Dashboard generator — reads `benchmarks/results/*.json` and produces a
//! single static HTML file with embedded Chart.js bar charts.
//!
//! Output: `benchmarks/dashboard/index.html`

use anyhow::Context;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Scan `results_dir` for all `.json` files and build the dashboard HTML.
pub fn generate(results_dir: &Path, output_path: &Path) -> anyhow::Result<()> {
    let entries = fs::read_dir(results_dir)
        .with_context(|| format!("reading results dir {:?}", results_dir))?;

    let mut runs: Vec<Value> = Vec::new();
    let mut security_runs: Vec<Value> = Vec::new();
    let mut cost_runs: Vec<Value> = Vec::new();
    let mut long_horizon_runs: Vec<Value> = Vec::new();
    let mut multi_agent_runs: Vec<Value> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("reading {:?}", path))?;
        let v: Value = serde_json::from_str(&raw).with_context(|| format!("parsing {:?}", path))?;

        let fname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if fname.starts_with("security_") {
            security_runs.push(v);
        } else if fname.starts_with("cost_") {
            cost_runs.push(v);
        } else if fname.starts_with("long_horizon_") {
            long_horizon_runs.push(v);
        } else if fname.starts_with("multi_agent_") {
            multi_agent_runs.push(v);
        } else {
            runs.push(v);
        }
    }

    let html = build_html(
        &runs,
        &security_runs,
        &cost_runs,
        &long_horizon_runs,
        &multi_agent_runs,
    );

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating directory {:?}", parent))?;
    }
    fs::write(output_path, &html)
        .with_context(|| format!("writing dashboard to {:?}", output_path))?;

    Ok(())
}

fn build_html(
    runs: &[Value],
    security_runs: &[Value],
    cost_runs: &[Value],
    long_horizon_runs: &[Value],
    multi_agent_runs: &[Value],
) -> String {
    let summary_rows = build_summary_table(
        runs,
        security_runs,
        cost_runs,
        long_horizon_runs,
        multi_agent_runs,
    );
    let latency_chart_data = extract_latency_data(runs);
    let cost_chart_data = extract_cost_data(cost_runs);
    let security_chart_data = extract_security_data(security_runs);
    let lh_chart_data = extract_lh_data(long_horizon_runs);
    let ma_chart_data = extract_ma_data(multi_agent_runs);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Argentor Benchmark Dashboard</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.0/dist/chart.umd.min.js"></script>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
          background: #0d1117; color: #c9d1d9; min-height: 100vh; }}
  header {{ background: #161b22; border-bottom: 1px solid #30363d;
            padding: 1.5rem 2rem; }}
  header h1 {{ font-size: 1.5rem; font-weight: 700; color: #58a6ff; }}
  header p {{ font-size: 0.875rem; color: #8b949e; margin-top: 0.25rem; }}
  .container {{ max-width: 1200px; margin: 0 auto; padding: 2rem; }}
  section {{ margin-bottom: 3rem; }}
  h2 {{ font-size: 1.125rem; font-weight: 600; color: #e6edf3;
        border-bottom: 1px solid #30363d; padding-bottom: 0.5rem;
        margin-bottom: 1rem; }}
  table {{ width: 100%; border-collapse: collapse; font-size: 0.875rem; }}
  th, td {{ padding: 0.5rem 0.75rem; text-align: left;
             border-bottom: 1px solid #21262d; }}
  th {{ background: #161b22; color: #8b949e; font-weight: 600; }}
  tr:hover td {{ background: #161b22; }}
  .chart-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(480px, 1fr));
                  gap: 1.5rem; }}
  .chart-card {{ background: #161b22; border: 1px solid #30363d;
                  border-radius: 8px; padding: 1.25rem; }}
  .chart-card h3 {{ font-size: 0.9rem; color: #8b949e; margin-bottom: 1rem; }}
  canvas {{ max-height: 260px; }}
  .badge {{ display: inline-block; padding: 0.15rem 0.5rem; border-radius: 4px;
             font-size: 0.75rem; font-weight: 600; }}
  .badge-green {{ background: #1f4a1f; color: #3fb950; }}
  .badge-grey {{ background: #21262d; color: #8b949e; }}
  footer {{ text-align: center; padding: 2rem; color: #484f58; font-size: 0.8rem;
             border-top: 1px solid #21262d; }}
</style>
</head>
<body>
<header>
  <h1>Argentor Benchmark Dashboard</h1>
  <p>Generated from benchmark results — latency, cost, security, long-horizon, multi-agent</p>
</header>
<div class="container">

<section>
  <h2>Summary</h2>
  <table>
    <thead><tr>
      <th>Track</th><th>Files</th><th>Status</th>
    </tr></thead>
    <tbody>
{summary_rows}
    </tbody>
  </table>
</section>

<section>
  <h2>Charts</h2>
  <div class="chart-grid">

    <div class="chart-card">
      <h3>Latency — mean wall time (ms) per runner</h3>
      <canvas id="latencyChart"></canvas>
    </div>

    <div class="chart-card">
      <h3>Cost — prompt tokens per runner (mean)</h3>
      <canvas id="costChart"></canvas>
    </div>

    <div class="chart-card">
      <h3>Security — block rate % per runner</h3>
      <canvas id="securityChart"></canvas>
    </div>

    <div class="chart-card">
      <h3>Long-Horizon — tokens at turn 10 (mean)</h3>
      <canvas id="lhChart"></canvas>
    </div>

    <div class="chart-card">
      <h3>Multi-Agent — total tokens (mean) per runner</h3>
      <canvas id="maChart"></canvas>
    </div>

  </div>
</section>

</div>
<footer>Argentor Benchmarks &mdash; AGPL-3.0-only &mdash; github.com/fboiero/Agentor</footer>

<script>
const COLORS = [
  '#58a6ff','#3fb950','#f78166','#d2a8ff','#ffa657',
  '#7ee787','#ff7b72','#79c0ff','#e3b341','#a5d6ff'
];

function makeChart(id, labels, datasets, yLabel) {{
  const ctx = document.getElementById(id);
  if (!ctx) return;
  new Chart(ctx, {{
    type: 'bar',
    data: {{
      labels: labels,
      datasets: datasets.map((d, i) => ({{
        label: d.label,
        data: d.data,
        backgroundColor: COLORS[i % COLORS.length] + 'cc',
        borderColor: COLORS[i % COLORS.length],
        borderWidth: 1,
        borderRadius: 3,
      }}))
    }},
    options: {{
      responsive: true,
      plugins: {{ legend: {{ labels: {{ color: '#8b949e' }} }} }},
      scales: {{
        x: {{ ticks: {{ color: '#8b949e' }}, grid: {{ color: '#21262d' }} }},
        y: {{ ticks: {{ color: '#8b949e' }}, grid: {{ color: '#21262d' }},
               title: {{ display: !!yLabel, text: yLabel, color: '#8b949e' }} }}
      }}
    }}
  }});
}}

{latency_chart_data}
{cost_chart_data}
{security_chart_data}
{lh_chart_data}
{ma_chart_data}
</script>
</body>
</html>"#,
        summary_rows = summary_rows,
        latency_chart_data = latency_chart_data,
        cost_chart_data = cost_chart_data,
        security_chart_data = security_chart_data,
        lh_chart_data = lh_chart_data,
        ma_chart_data = ma_chart_data,
    )
}

fn build_summary_table(
    runs: &[Value],
    security: &[Value],
    cost: &[Value],
    lh: &[Value],
    ma: &[Value],
) -> String {
    let mut rows = String::new();
    let tracks = [
        ("General runs", runs.len()),
        ("Security", security.len()),
        ("Cost", cost.len()),
        ("Long-Horizon", lh.len()),
        ("Multi-Agent", ma.len()),
    ];
    for (name, count) in &tracks {
        let badge = if *count > 0 {
            format!(r#"<span class="badge badge-green">{count} file(s)</span>"#)
        } else {
            r#"<span class="badge badge-grey">no data</span>"#.to_string()
        };
        rows.push_str(&format!(
            "      <tr><td>{name}</td><td>{count}</td><td>{badge}</td></tr>\n"
        ));
    }
    rows
}

// ── Chart data extractors ─────────────────────────────────────────────────────

fn extract_latency_data(runs: &[Value]) -> String {
    // Collect (runner, mean_wall_ms) from `latency_samples_ms` keys.
    let mut runner_totals: std::collections::HashMap<String, (f64, usize)> =
        std::collections::HashMap::new();

    for run in runs {
        if let Some(samples) = run.get("latency_samples_ms").and_then(|v| v.as_object()) {
            for (key, vals) in samples {
                // key format: "task_id :: runner_name"
                let runner = key.split(" :: ").nth(1).unwrap_or(key.as_str()).to_string();
                if let Some(arr) = vals.as_array() {
                    let sum: f64 = arr.iter().filter_map(|v| v.as_f64()).sum();
                    let count = arr.len();
                    let e = runner_totals.entry(runner).or_insert((0.0, 0));
                    e.0 += sum;
                    e.1 += count;
                }
            }
        }
    }

    let mut pairs: Vec<(String, f64)> = runner_totals
        .into_iter()
        .map(|(r, (sum, n))| (r, if n > 0 { sum / n as f64 } else { 0.0 }))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let labels: Vec<String> = pairs.iter().map(|(r, _)| format!("\"{r}\"")).collect();
    let data: Vec<String> = pairs.iter().map(|(_, v)| format!("{v:.1}")).collect();

    if pairs.is_empty() {
        return "makeChart('latencyChart', ['no data'], [{label:'latency',data:[0]}], 'ms');"
            .to_string();
    }

    format!(
        "makeChart('latencyChart', [{labels}], [{{label:'mean wall ms', data:[{data}]}}], 'ms');",
        labels = labels.join(","),
        data = data.join(","),
    )
}

fn extract_cost_data(cost_runs: &[Value]) -> String {
    // Collect mean prompt_tokens_sent per runner from cost result combos.
    let mut runner_totals: std::collections::HashMap<String, (u64, usize)> =
        std::collections::HashMap::new();

    for run in cost_runs {
        if let Some(combos) = run.get("results_by_combo").and_then(|v| v.as_object()) {
            for (key, results_val) in combos {
                let runner = key.split(" :: ").nth(1).unwrap_or(key.as_str()).to_string();
                if let Some(arr) = results_val.as_array() {
                    for r in arr {
                        let tokens = r
                            .get("prompt_tokens_sent")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let e = runner_totals.entry(runner.clone()).or_insert((0, 0));
                        e.0 += tokens;
                        e.1 += 1;
                    }
                }
            }
        }
    }

    let mut pairs: Vec<(String, f64)> = runner_totals
        .into_iter()
        .map(|(r, (sum, n))| (r, if n > 0 { sum as f64 / n as f64 } else { 0.0 }))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    if pairs.is_empty() {
        return "makeChart('costChart', ['no data'], [{label:'tokens',data:[0]}], 'tokens');"
            .to_string();
    }

    let labels: Vec<String> = pairs.iter().map(|(r, _)| format!("\"{r}\"")).collect();
    let data: Vec<String> = pairs.iter().map(|(_, v)| format!("{v:.0}")).collect();
    format!(
        "makeChart('costChart', [{labels}], [{{label:'mean prompt tokens', data:[{data}]}}], 'tokens');",
        labels = labels.join(","),
        data = data.join(","),
    )
}

fn extract_security_data(security_runs: &[Value]) -> String {
    // Extract block_rate per runner from per-result was_blocked / expected_blocked.
    let mut runner_stats: std::collections::HashMap<String, (u32, u32)> =
        std::collections::HashMap::new();

    for run in security_runs {
        if let Some(by_runner) = run.get("results_by_runner").and_then(|v| v.as_object()) {
            for (runner, results_val) in by_runner {
                if let Some(arr) = results_val.as_array() {
                    let e = runner_stats.entry(runner.clone()).or_insert((0, 0));
                    for r in arr {
                        let blocked = r
                            .get("was_blocked")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        e.1 += 1;
                        if blocked {
                            e.0 += 1;
                        }
                    }
                }
            }
        }
    }

    let mut pairs: Vec<(String, f64)> = runner_stats
        .into_iter()
        .map(|(r, (blocked, total))| {
            (
                r,
                if total > 0 {
                    blocked as f64 / total as f64 * 100.0
                } else {
                    0.0
                },
            )
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    if pairs.is_empty() {
        return "makeChart('securityChart', ['no data'], [{label:'block rate',data:[0]}], '%');"
            .to_string();
    }

    let labels: Vec<String> = pairs.iter().map(|(r, _)| format!("\"{r}\"")).collect();
    let data: Vec<String> = pairs.iter().map(|(_, v)| format!("{v:.1}")).collect();
    format!(
        "makeChart('securityChart', [{labels}], [{{label:'block rate %', data:[{data}]}}], '%');",
        labels = labels.join(","),
        data = data.join(","),
    )
}

fn extract_lh_data(lh_runs: &[Value]) -> String {
    // Collect mean tokens_at_turn_10 per runner from summaries.
    let mut runner_totals: std::collections::HashMap<String, (f64, usize)> =
        std::collections::HashMap::new();

    for run in lh_runs {
        if let Some(summaries) = run.get("summaries").and_then(|v| v.as_array()) {
            for s in summaries {
                let runner = s
                    .get("runner")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let tok = s
                    .get("mean_tokens_at_turn_10")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let e = runner_totals.entry(runner).or_insert((0.0, 0));
                e.0 += tok;
                e.1 += 1;
            }
        }
    }

    let mut pairs: Vec<(String, f64)> = runner_totals
        .into_iter()
        .map(|(r, (sum, n))| (r, if n > 0 { sum / n as f64 } else { 0.0 }))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    if pairs.is_empty() {
        return "makeChart('lhChart', ['no data'], [{label:'tok@T10',data:[0]}], 'tokens');"
            .to_string();
    }

    let labels: Vec<String> = pairs.iter().map(|(r, _)| format!("\"{r}\"")).collect();
    let data: Vec<String> = pairs.iter().map(|(_, v)| format!("{v:.0}")).collect();
    format!(
        "makeChart('lhChart', [{labels}], [{{label:'mean tok@T10', data:[{data}]}}], 'tokens');",
        labels = labels.join(","),
        data = data.join(","),
    )
}

fn extract_ma_data(ma_runs: &[Value]) -> String {
    // Collect mean total_tokens per runner from multi-agent summaries.
    let mut runner_totals: std::collections::HashMap<String, (f64, usize)> =
        std::collections::HashMap::new();

    for run in ma_runs {
        if let Some(summaries) = run.get("summaries").and_then(|v| v.as_array()) {
            for s in summaries {
                let runner = s
                    .get("runner")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let tok = s
                    .get("mean_total_tokens")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let e = runner_totals.entry(runner).or_insert((0.0, 0));
                e.0 += tok;
                e.1 += 1;
            }
        }
    }

    let mut pairs: Vec<(String, f64)> = runner_totals
        .into_iter()
        .map(|(r, (sum, n))| (r, if n > 0 { sum / n as f64 } else { 0.0 }))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    if pairs.is_empty() {
        return "makeChart('maChart', ['no data'], [{label:'total tokens',data:[0]}], 'tokens');"
            .to_string();
    }

    let labels: Vec<String> = pairs.iter().map(|(r, _)| format!("\"{r}\"")).collect();
    let data: Vec<String> = pairs.iter().map(|(_, v)| format!("{v:.0}")).collect();
    format!(
        "makeChart('maChart', [{labels}], [{{label:'mean total tokens', data:[{data}]}}], 'tokens');",
        labels = labels.join(","),
        data = data.join(","),
    )
}

/// Returns the default output path: `{tasks_dir}/../dashboard/index.html`.
pub fn default_output(results_dir: &Path) -> PathBuf {
    results_dir
        .parent()
        .unwrap_or(Path::new("."))
        .join("dashboard")
        .join("index.html")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join("argentor_bench_tests").join(name);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn generate_empty_results_produces_html() {
        let base = tmp_dir("dashboard_empty");
        let results_dir = base.join("results");
        fs::create_dir_all(&results_dir).unwrap();
        let out = base.join("dashboard").join("index.html");
        generate(&results_dir, &out).unwrap();
        let html = fs::read_to_string(&out).unwrap();
        assert!(html.contains("Argentor Benchmark Dashboard"));
        assert!(html.contains("chart.umd.min.js"));
    }

    #[test]
    fn generate_with_run_json_parses_ok() {
        let base = tmp_dir("dashboard_with_json");
        let results_dir = base.join("results");
        fs::create_dir_all(&results_dir).unwrap();

        // Write a minimal run JSON to simulate existing results.
        let run_json = serde_json::json!({
            "latency_samples_ms": {
                "t1 :: argentor v0.1": [120.0, 130.0],
                "t1 :: mock v0.1": [50.0, 55.0]
            }
        });
        fs::write(
            results_dir.join("run_20260101_000000.json"),
            serde_json::to_string(&run_json).unwrap(),
        )
        .unwrap();

        let out = base.join("dashboard").join("index.html");
        generate(&results_dir, &out).unwrap();
        let html = fs::read_to_string(&out).unwrap();
        assert!(html.contains("latencyChart"));
        assert!(html.contains("argentor"));
    }

    #[test]
    fn default_output_path() {
        let results = Path::new("/some/project/benchmarks/results");
        let out = default_output(results);
        assert!(out.ends_with("dashboard/index.html"));
    }
}

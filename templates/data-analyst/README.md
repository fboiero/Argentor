# Data Analyst Agent Template

Reference profile for a data-analysis agent that processes structured files, computes statistics, and summarizes findings in plain language.

## Use case

On-demand or scheduled analysis over CSV, JSON, or Parquet files without a data warehouse.

## Files

- `system_prompt.txt` — behaviour: data-quality validation, step-by-step calculations, no causal claims without evidence.
- `config.toml` — agent-profile sketch (model, max turns, moderate guardrails, intended skill set).

## How to apply

The CLI does not load these files directly. Use them as a reference when wiring your own integration:

1. Copy `system_prompt.txt` into your `AgentRunner` setup (system message slot).
2. Declare each referenced skill in your `argentor.toml` `[[skills]]` block — every skill marked "available" below is implemented in `argentor-builtins`.
3. Adopt the `guardrails.profile = "moderate"` hint via the runner's guardrail configuration.
4. Apply `agent.max_turns = 25` via your `ModelConfig`.

## Skills referenced

| Skill | Status | Crate | Purpose |
|-------|--------|-------|---------|
| `csv_processor` | available | `argentor-builtins` | Parse and query tabular data |
| `calculator` | available | `argentor-builtins` | Arithmetic and statistical computations |
| `file_read` | available | `argentor-builtins` | Read local data files |
| `memory_search` | available | `argentor-builtins` | Recall previous analyses for context |
| `web_fetch` | not yet registered | — | Fetch remote datasets by URL — supply your own or remove from enabled list |
| `chart_generation` | not yet registered | — | Render line, bar, scatter, pie charts — supply your own or remove from enabled list |

## Profile-specific knobs

| Key | Default | Description |
|-----|---------|-------------|
| `agent.max_turns` | `25` | Max reasoning steps |
| `analytics.max_file_size_mb` | `100` | File size cap |
| `analytics.chart_output_format` | `png` | Intended chart output format |
| `analytics.supported_formats` | `[csv, json, parquet]` | Intended input file formats |

These fields are advisory — they describe the profile, not runtime flags accepted by `argentor-cli`.

## License

AGPL-3.0-only

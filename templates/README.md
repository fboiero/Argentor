# Argentor Agent Templates

Reference configurations and system prompts for four common agent profiles. Treat these as **starting points** for your own `argentor.toml` and prompts — not as drop-in executables. The CLI does not currently load these files directly; you copy the relevant pieces into your own integration.

## Available templates

| Template | Profile | Guardrails | Skills referenced |
|----------|---------|------------|-------------------|
| [code-reviewer](./code-reviewer/) | Senior reviewer over PRs and diffs | `moderate` | `code_analysis`, `file_read`, `memory_search` |
| [customer-support](./customer-support/) | First-line support with HITL escalation | `strict` | `web_search`, `human_approval`, `memory_search`, `file_read` |
| [data-analyst](./data-analyst/) | CSV/JSON/Parquet analysis and charts | `moderate` | `csv_processor`, `calculator`, `file_read`, `memory_search` |
| [rag-agent](./rag-agent/) | Q&A over a private corpus via vector search | `strict` | `web_search`, `memory_search`, `file_read` |

## What each template gives you

Every template ships two files:

- **`config.toml`** — an agent-profile sketch: model choice, max turns, guardrail profile, intended skill set, memory tier, and profile-specific tunables. The schema differs from the runtime `argentor.toml` consumed by `argentor-cli serve` — see "How to use" below.
- **`system_prompt.txt`** — a behavioural prompt you can paste into your own runtime configuration or pass through your own agent harness.

## How to use them today

1. Read the template `README.md` to understand the intent.
2. Copy the system prompt into your runtime (e.g., into your application that wraps `AgentRunner`, or into a custom skill that prepends it to messages).
3. Match the listed skills against your `argentor.toml` `[[skills]]` declarations — every skill referenced in the templates is implemented in `argentor-builtins`, but the runtime config requires you to declare each skill explicitly with its `type`, `path`, and capabilities.
4. Adopt the guardrail profile and memory tier hints from the template into your own configuration.

## Future work

A first-class `argentor template <list|init>` subcommand and a parallel `[agent]` config schema that consumes these files directly are tracked as separate work — these templates are the source material for that feature.

## License

AGPL-3.0-only

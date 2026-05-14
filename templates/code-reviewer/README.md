# Code Reviewer Agent Template

Reference profile for a senior code-review agent that analyzes pull requests and diffs for correctness, security, and maintainability.

## Use case

Automated code review against a diff or file set, called from CI/CD or run on demand.

## Files

- `system_prompt.txt` — review behavior: severity categorization, language coverage, no auto-rewrite policy.
- `config.toml` — agent-profile sketch (model, max turns, guardrail profile, intended skill set).

## How to apply

The CLI does not load these files directly. Use them as a reference when wiring your own integration:

1. Copy `system_prompt.txt` into your `AgentRunner` setup (system message slot).
2. Declare each referenced skill in your `argentor.toml` `[[skills]]` block — every skill listed below is implemented in `argentor-builtins`.
3. Adopt the `guardrails.profile = "moderate"` hint via the runner's guardrail configuration.
4. Apply `agent.max_turns = 15` via your `ModelConfig`.

## Skills referenced

| Skill | Crate | Purpose |
|-------|-------|---------|
| `code_analysis` | `argentor-builtins` | Static analysis and pattern detection |
| `file_read` | `argentor-builtins` | Read source files |
| `memory_search` | `argentor-builtins` | Recall previous review decisions for consistency |

> Note: `git_tools` is listed in `config.toml` as an intended skill but is not currently registered in `argentor-builtins`. Either remove it from the enabled list or supply your own implementation when adapting this template.

## Profile-specific knobs

| Key | Default | Description |
|-----|---------|-------------|
| `agent.max_turns` | `15` | Max review iterations |
| `review.languages` | `[rust, go, typescript, python]` | Intended language coverage |
| `review.severity_levels` | `[critical, warning, suggestion]` | Finding categories |
| `review.auto_fix` | `false` | Whether the agent should rewrite code (off by default) |

These fields are advisory — they describe the profile, not runtime flags accepted by `argentor-cli`.

## License

AGPL-3.0-only

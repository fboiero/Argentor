# Customer Support Agent Template

Reference profile for a first-line support agent with human-in-the-loop escalation and knowledge-base lookup.

## Use case

Automated triage that resolves common issues and escalates complex ones to a human agent.

## Files

- `system_prompt.txt` — behaviour: concise, confirms understanding, escalates pricing/SLA/legal, requests human approval for data-affecting actions.
- `config.toml` — agent-profile sketch (model, max turns, strict guardrails, intended skill set).

## How to apply

The CLI does not load these files directly. Use them as a reference when wiring your own integration:

1. Copy `system_prompt.txt` into your `AgentRunner` setup (system message slot).
2. Declare each referenced skill in your `argentor.toml` `[[skills]]` block — every skill listed below is implemented in `argentor-builtins`.
3. Adopt the `guardrails.profile = "strict"` hint via the runner's guardrail configuration — important for a customer-facing surface.
4. Apply `agent.max_turns = 20` via your `ModelConfig`.

## Skills referenced

| Skill | Crate | Purpose |
|-------|-------|---------|
| `web_search` | `argentor-builtins` | Look up current product info or docs |
| `human_approval` | `argentor-builtins` | Pause and request human review for sensitive actions |
| `memory_search` | `argentor-builtins` | Search past tickets and knowledge base |
| `file_read` | `argentor-builtins` | Read product documentation |

## Profile-specific knobs

| Key | Default | Description |
|-----|---------|-------------|
| `agent.max_turns` | `20` | Maximum turns per conversation |
| `support.escalation_enabled` | `true` | Allow escalating to human |
| `support.human_approval_timeout_secs` | `300` | Seconds to wait for human approval |
| `support.max_open_tickets` | `50` | Concurrent ticket cap |

These fields are advisory — they describe the profile, not runtime flags accepted by `argentor-cli`.

## Customization

Replace `system_prompt.txt` with your brand voice, product-specific guidelines, and escalation rules.

## License

AGPL-3.0-only

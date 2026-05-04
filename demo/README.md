# Recording the Argentor Demo

## Prerequisites

```bash
# asciinema
brew install asciinema

# Build the workspace first (examples + binaries must exist)
cargo build --workspace --quiet
```

## Record

```bash
asciinema rec demo/demo.cast --command="bash demo/demo_script.sh"
```

The script is interactive by default (waits for Enter between sections).
For a fully automated recording, pipe input:

```bash
yes "" | asciinema rec demo/demo.cast --command="bash demo/demo_script.sh"
```

## Play locally

```bash
asciinema play demo/demo.cast
```

## Upload to asciinema.org

```bash
asciinema upload demo/demo.cast
```

Copy the returned URL and paste it into the README badge or docs page.

## Convert to GIF (for README embed)

```bash
# Install agg (asciinema GIF generator)
cargo install agg

agg demo/demo.cast demo/demo.gif --font-size 14 --cols 100 --rows 35
```

## Embed in README

```markdown
[![Argentor Demo](demo/demo.gif)](https://asciinema.org/a/<your-id>)
```

## Demo sections (runtime ~3–4 minutes)

| # | Section | What it shows |
|---|---------|---------------|
| 1 | Quick Start | hello_world example — agent responds without API key |
| 2 | Tool Calling | with_tools example — calculator skill invoked via mock LLM |
| 3 | Custom Skills | custom_skill example — ReverseSkill author flow |
| 4 | Multi-Agent | multi_agent example — Orchestrator-Workers pipeline |
| 5 | Security | e2e_guardrail_check — 6-layer guardrail engine live test |
| 6 | Python SDK | Import check — SDK ready without Rust installation |
| 7 | Test Suite | Full workspace test run summary |
| 8 | Benchmarks | Argentor vs LangChain vs CrewAI comparison table |
| 9 | Architecture | All 17 crates and their responsibilities |

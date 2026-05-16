# argentor-sdk v1.4.6

Python SDK client for the [Argentor](https://github.com/fboiero/Agentor) AI agent framework REST API.

## Installation

```bash
pip install argentor-sdk
```

For development:

```bash
pip install argentor-sdk[dev]
```

## Quick Start

```python
from argentor import ArgentorClient

client = ArgentorClient(
    base_url="http://localhost:8080",
    api_key="your-api-key",
)

# Run a task
result = client.run_task(
    role="code_reviewer",
    context="Review the following pull request...",
)
print(result)

# Stream results via SSE
for chunk in client.run_task_stream(
    role="assistant",
    context="Explain how Argentor works",
):
    print(chunk, end="", flush=True)

# Batch execution
batch = client.batch_tasks(
    tasks=[
        {"agent_role": "analyst", "context": "Analyze Q1 sales data"},
        {"agent_role": "analyst", "context": "Analyze Q2 sales data"},
    ],
    max_concurrent=5,
)
print(batch)

# Evaluate a response
evaluation = client.evaluate(
    text="The code looks clean and follows best practices.",
    context="Code review task",
    criteria=["accuracy", "completeness", "helpfulness"],
)
print(evaluation)

# List skills
skills = client.list_skills()
for skill in skills:
    print(skill["name"])

# Execute a skill
result = client.execute_skill("echo", {"text": "Hello, world!"})
print(result)

# Health check
health = client.health()
print(health)
```

## Async Usage

```python
import asyncio
from argentor import AsyncArgentorClient

async def main():
    async with AsyncArgentorClient(
        base_url="http://localhost:8080",
        api_key="your-api-key",
    ) as client:
        # Run a task
        result = await client.run_task(
            role="assistant",
            context="Hello, world!",
        )
        print(result)

        # Stream results
        async for chunk in client.run_task_stream(
            role="assistant",
            context="Explain Argentor",
        ):
            print(chunk)

        # List sessions
        sessions = await client.list_sessions()
        print(sessions)

asyncio.run(main())
```

## Agent SDK (subprocess wrapper)

The agent module wraps the `argentor` CLI binary as a subprocess,
similar to how Claude Agent SDK wraps Claude Code.  It communicates
via NDJSON over stdin/stdout and works with any LLM provider.

```python
import asyncio
from argentor import query, query_simple, AgentOptions

# Stream events from the agent
async def main():
    async for event in query(
        "What files are in this directory?",
        AgentOptions.claude("sk-your-api-key"),
    ):
        if event.type == "assistant":
            print(event.text)
        elif event.type == "tool_use":
            print(f"[tool] {event.data.get('name')}")
        elif event.is_done:
            print(f"\nDone: {event.text}")

asyncio.run(main())
```

### One-liner convenience functions

```python
from argentor import ask_claude, ask_openai, ask_gemini, ask_ollama

# Each returns the final text output
result = asyncio.run(ask_claude("Explain Argentor", "sk-..."))
result = asyncio.run(ask_openai("Explain Argentor", "sk-..."))
result = asyncio.run(ask_gemini("Explain Argentor", "AIza..."))
result = asyncio.run(ask_ollama("Explain Argentor", "llama3"))
```

### Provider presets

```python
AgentOptions.claude(api_key)   # Anthropic Claude
AgentOptions.openai(api_key)   # OpenAI GPT-4o
AgentOptions.gemini(api_key)   # Google Gemini
AgentOptions.ollama("llama3")  # Local Ollama (no key)
```

### AgentOptions

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `provider` | `str` | `"claude"` | LLM provider |
| `model` | `str` | `"claude-sonnet-4-20250514"` | Model name |
| `api_key` | `str` | `""` | API key |
| `system_prompt` | `str\|None` | `None` | System prompt override |
| `max_turns` | `int` | `10` | Max agentic turns |
| `temperature` | `float` | `0.7` | Sampling temperature |
| `tools` | `list[str]\|None` | `None` | Tool names (None = builtins) |
| `permission_mode` | `str` | `"default"` | `"default"`, `"strict"`, `"permissive"`, `"plan"` |
| `working_directory` | `str\|None` | `None` | Working directory |
| `mcp_servers` | `list[dict]\|None` | `None` | MCP server configs |
| `argentor_binary` | `str\|None` | `None` | Path to binary (auto-detected if omitted) |

## Error Handling

```python
from argentor import ArgentorClient
from argentor.exceptions import ArgentorAPIError, ArgentorConnectionError

client = ArgentorClient(base_url="http://localhost:8080")

try:
    result = client.run_task(role="assistant", context="Hello")
except ArgentorAPIError as e:
    print(f"API error {e.status_code}: {e.message}")
    print(f"Response body: {e.response_body}")
except ArgentorConnectionError as e:
    print(f"Connection failed: {e.message}")
```

## API Reference

### ArgentorClient / AsyncArgentorClient

Both clients expose the same methods. The async client returns awaitables and uses `async for` for streaming.

| Method | Description |
|--------|-------------|
| `run_task(role, context, **kwargs)` | Execute a single agent task |
| `run_task_stream(role, context, **kwargs)` | Stream task results via SSE |
| `batch_tasks(tasks, max_concurrent=5)` | Submit a batch of tasks |
| `evaluate(text, context, criteria)` | Evaluate text against criteria |
| `agent_chat(message, **kwargs)` | Send a message via agent chat |
| `agent_status()` | Get agent status |
| `create_session()` | Create a new session |
| `get_session(session_id)` | Retrieve a session |
| `list_sessions()` | List all sessions |
| `delete_session(session_id)` | Delete a session |
| `list_skills()` | List registered skills |
| `get_skill(name)` | Get skill details |
| `execute_skill(name, arguments)` | Execute a skill |
| `health()` | Check API health |
| `health_ready()` | Readiness probe |
| `metrics()` | Get Prometheus metrics |
| `list_connections()` | List WebSocket connections |
| `create_persona(tenant_id, agent_role, persona)` | Create persona |
| `list_personas(tenant_id)` | List tenant personas |
| `get_usage(tenant_id)` | Get usage breakdown |
| `webhook_proxy(event, data, **kwargs)` | Forward a webhook |
| `search_marketplace(query, category)` | Search skill marketplace |
| `install_skill(name)` | Install from marketplace |

## License

AGPL-3.0-only

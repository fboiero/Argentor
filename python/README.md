# Argentor Python SDK

Pure-Python client for the [Argentor](https://github.com/fboiero/Argentor) AI agent framework.

## Install

```bash
pip install -e python/
```

Requires Python 3.9+ and `httpx`.

## Quick start

```python
from argentor import Agent

agent = Agent(api_key="sk-ant-...", model="claude-3-5-sonnet-20241022")
response = agent.run("What is the capital of Argentina?")
print(response)
```

Set `ANTHROPIC_API_KEY` in your environment to omit `api_key`:

```python
import os
os.environ["ANTHROPIC_API_KEY"] = "sk-ant-..."
agent = Agent()
```

## Agent with skills (tool use)

```python
def get_weather(city: str) -> str:
    return f"Sunny in {city}, 22C"

agent = Agent(api_key="sk-ant-...")
agent.add_skill(
    name="get_weather",
    description="Get the current weather for a city",
    fn=get_weather,
    parameters={
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    },
)

response = agent.run("What is the weather in Buenos Aires?")
print(response)
```

## Async API

```python
import asyncio
from argentor import Agent

async def main():
    agent = Agent(api_key="sk-ant-...")
    response = await agent.run_async("Explain quantum entanglement in one sentence.")
    print(response)

asyncio.run(main())
```

## Session management

```python
agent = Agent(api_key="sk-ant-...")
agent.run("My name is Federico.")
response = agent.run("What is my name?")  # maintains context
print(response)  # "Your name is Federico."

# Reset session
agent.reset_session()
```

## Classes

| Class | Description |
|-------|-------------|
| `Agent` | Main agent class with `run()` / `run_async()` |
| `Session` | Conversation history tracking |
| `Message` | Single message (role + content) |
| `Skill` | A callable tool the agent can invoke |
| `SkillRegistry` | Registry for discovering and invoking skills |

## Rust extension (optional)

For the compiled Rust extension (PyO3/maturin, faster execution):

```bash
pip install maturin
cd crates/argentor-python
maturin develop
```

The `crates/argentor-python/` extension exposes `Session`, `SkillRegistry`,
`GuardrailEngine`, `Calculator`, `HashTool`, and `JsonQuery` as compiled
Rust classes.

## License

AGPL-3.0-only

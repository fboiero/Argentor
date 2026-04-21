# SPDX-License-Identifier: AGPL-3.0-only
"""Agent with custom skills (tool use) example.

Usage:
    pip install -e python/
    export ANTHROPIC_API_KEY=sk-ant-...
    python python/examples/with_tools.py
"""

import os
import json
from argentor import Agent

api_key = os.environ.get("ANTHROPIC_API_KEY", "")
if not api_key:
    print("Set ANTHROPIC_API_KEY to run this example.")
    raise SystemExit(1)


# --- Define skills ---

def get_weather(city: str) -> str:
    """Simulated weather lookup."""
    data = {
        "Buenos Aires": "Sunny, 22°C",
        "London": "Cloudy, 14°C",
        "Tokyo": "Rainy, 18°C",
    }
    return data.get(city, f"Weather data unavailable for {city}")


def calculate(expression: str) -> str:
    """Safely evaluate a simple math expression."""
    try:
        # Only allow safe characters
        allowed = set("0123456789+-*/()., ")
        if not all(c in allowed for c in expression):
            return "Error: unsafe expression"
        result = eval(expression, {"__builtins__": {}})  # noqa: S307
        return str(result)
    except Exception as e:
        return f"Error: {e}"


# --- Build agent ---

agent = Agent(
    api_key=api_key,
    model="claude-3-5-haiku-20241022",
    system_prompt="You are a helpful assistant with access to weather and calculator tools.",
)

agent.add_skill(
    name="get_weather",
    description="Get the current weather for a city",
    fn=get_weather,
    parameters={
        "type": "object",
        "properties": {
            "city": {"type": "string", "description": "City name (e.g., 'Buenos Aires')"},
        },
        "required": ["city"],
    },
)

agent.add_skill(
    name="calculate",
    description="Evaluate a mathematical expression",
    fn=calculate,
    parameters={
        "type": "object",
        "properties": {
            "expression": {
                "type": "string",
                "description": "Math expression, e.g., '2 + 3 * 4'",
            },
        },
        "required": ["expression"],
    },
)

print(f"Agent has {agent.registry.skill_count} skills: {agent.registry.list_skills()}")
print()

# --- Run agent ---

response = agent.run(
    "What is the weather in Buenos Aires? "
    "Also, what is 42 * 7? Give me both answers."
)
print(f"Agent: {response}")
print()
print(f"Session history: {agent.session.message_count} messages")

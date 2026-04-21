# SPDX-License-Identifier: AGPL-3.0-only
"""Simplest possible Argentor agent example.

Usage:
    pip install -e python/
    export ANTHROPIC_API_KEY=sk-ant-...
    python python/examples/hello_world.py
"""

import os
from argentor import Agent

api_key = os.environ.get("ANTHROPIC_API_KEY", "")
if not api_key:
    print("Set ANTHROPIC_API_KEY to run this example.")
    raise SystemExit(1)

agent = Agent(
    api_key=api_key,
    model="claude-3-5-haiku-20241022",
    system_prompt="You are a concise assistant. Answer in one sentence.",
)

response = agent.run("What is the capital of Argentina?")
print(f"Agent: {response}")

# The session tracks history
print(f"\nSession has {agent.session.message_count} messages")
print(f"Agent repr: {agent!r}")

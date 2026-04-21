# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2024 Argentor contributors
"""Core Agent class for the Argentor Python SDK."""

from __future__ import annotations

import json
import os
from typing import Any, Callable, Dict, List, Optional

import httpx

from .session import Session
from .skills import Skill, SkillRegistry

# Default Anthropic API endpoint
_DEFAULT_BASE_URL = "https://api.anthropic.com"
_DEFAULT_MODEL = "claude-3-5-sonnet-20241022"
_ANTHROPIC_VERSION = "2023-06-01"
_MAX_TURNS = 10


class Agent:
    """An AI agent powered by the Anthropic Claude API.

    The agent maintains a session of messages and supports tool/skill
    invocation via the Anthropic tool-use API.

    Example::

        agent = Agent(api_key="sk-ant-...", model="claude-3-5-sonnet-20241022")

        # Simple prompt
        response = agent.run("What is 2 + 2?")
        print(response)

        # With a custom skill
        def get_weather(city: str) -> str:
            return f"Sunny in {city}, 22°C"

        agent.add_skill(
            name="get_weather",
            description="Get the current weather for a city",
            fn=get_weather,
        )
        response = agent.run("What is the weather in Buenos Aires?")
        print(response)

    Async usage::

        import asyncio
        response = asyncio.run(agent.run_async("Hello!"))
    """

    def __init__(
        self,
        api_key: Optional[str] = None,
        model: str = _DEFAULT_MODEL,
        system_prompt: Optional[str] = None,
        base_url: str = _DEFAULT_BASE_URL,
        max_tokens: int = 4096,
        max_turns: int = _MAX_TURNS,
        temperature: float = 0.7,
        session: Optional[Session] = None,
    ) -> None:
        self.api_key: str = api_key or os.environ.get("ANTHROPIC_API_KEY", "")
        if not self.api_key:
            raise ValueError(
                "api_key is required. Pass it directly or set ANTHROPIC_API_KEY."
            )
        self.model = model
        self.system_prompt = system_prompt
        self.base_url = base_url.rstrip("/")
        self.max_tokens = max_tokens
        self.max_turns = max_turns
        self.temperature = temperature
        self.session: Session = session or Session()
        self.registry = SkillRegistry()

    # ------------------------------------------------------------------
    # Skill registration
    # ------------------------------------------------------------------

    def add_skill(
        self,
        name: str,
        description: str,
        fn: Callable,
        parameters: Optional[Dict] = None,
    ) -> "Agent":
        """Register a Python function as an agent skill.

        Returns `self` for chaining.

        Example::

            agent.add_skill("add", "Add two integers", lambda a, b: a + b)
        """
        self.registry.register(name=name, description=description, fn=fn, parameters=parameters)
        return self

    def add_skill_obj(self, skill: Skill) -> "Agent":
        """Register a pre-built Skill instance."""
        self.registry.add_skill(skill)
        return self

    # ------------------------------------------------------------------
    # Synchronous API
    # ------------------------------------------------------------------

    def run(self, prompt: str) -> str:
        """Run the agent with the given user prompt and return the final text response.

        Blocks until the agentic loop completes.  Internally runs the async
        implementation via a new event loop.
        """
        import asyncio

        return asyncio.run(self.run_async(prompt))

    # ------------------------------------------------------------------
    # Asynchronous API
    # ------------------------------------------------------------------

    async def run_async(self, prompt: str) -> str:
        """Run the agent asynchronously with the given user prompt.

        Executes the full agentic loop (tool-use included) and returns the
        final text response.
        """
        self.session.add_user_message(prompt)

        for _ in range(self.max_turns):
            response = await self._call_api()
            stop_reason = response.get("stop_reason", "end_turn")

            if stop_reason == "tool_use":
                # Extract text content (if any) before tool calls
                assistant_text = _extract_text(response)
                if assistant_text:
                    self.session.add_assistant_message(assistant_text)

                # Execute tool calls and collect results
                tool_results = await self._handle_tool_use(response)

                # Append tool results as a user turn (Anthropic convention)
                results_content = json.dumps(tool_results)
                self.session.add_message("tool", results_content)
                continue

            # Terminal states
            text = _extract_text(response)
            self.session.add_assistant_message(text)
            return text

        # Max turns reached — return whatever text we have
        return "Maximum turns reached without a final response."

    async def _call_api(self) -> Dict[str, Any]:
        """Make a single call to the Anthropic messages API."""
        headers = {
            "x-api-key": self.api_key,
            "anthropic-version": _ANTHROPIC_VERSION,
            "content-type": "application/json",
        }

        body: Dict[str, Any] = {
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": self.session.api_messages(),
        }

        if self.system_prompt:
            body["system"] = self.system_prompt

        tools = self.registry.to_tools()
        if tools:
            body["tools"] = tools

        async with httpx.AsyncClient(timeout=120.0) as client:
            resp = await client.post(
                f"{self.base_url}/v1/messages",
                headers=headers,
                json=body,
            )
            if resp.status_code != 200:
                raise RuntimeError(
                    f"Anthropic API error {resp.status_code}: {resp.text}"
                )
            return resp.json()

    async def _handle_tool_use(self, response: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Execute all tool calls in the response and return results."""
        results = []
        content_blocks = response.get("content", [])

        for block in content_blocks:
            if block.get("type") != "tool_use":
                continue

            tool_id = block.get("id", "")
            tool_name = block.get("name", "")
            tool_input = block.get("input", {})

            output = await self.registry.execute(tool_name, tool_input)
            results.append(
                {
                    "type": "tool_result",
                    "tool_use_id": tool_id,
                    "content": output,
                }
            )

        return results

    # ------------------------------------------------------------------
    # Session management
    # ------------------------------------------------------------------

    def reset_session(self) -> None:
        """Clear message history and start a fresh session."""
        self.session = Session()

    def get_history(self) -> List[Dict[str, str]]:
        """Return the conversation history as a list of role/content dicts."""
        return [m.to_dict() for m in self.session.messages]

    def __repr__(self) -> str:
        return (
            f"Agent(model={self.model!r}, "
            f"skills={self.registry.skill_count}, "
            f"messages={self.session.message_count})"
        )


def _extract_text(response: Dict[str, Any]) -> str:
    """Extract concatenated text from an Anthropic API response."""
    parts = []
    for block in response.get("content", []):
        if block.get("type") == "text":
            parts.append(block.get("text", ""))
    return "".join(parts)

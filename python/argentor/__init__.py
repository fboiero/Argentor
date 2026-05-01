# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2024 Argentor contributors
"""Argentor Python SDK — pure-Python client for the Argentor agent framework.

Quick start::

    from argentor import Agent

    agent = Agent(api_key="sk-ant-...", model="claude-3-5-sonnet-20241022")
    response = agent.run("What is 2 + 2?")
    print(response)

For async usage::

    import asyncio
    from argentor import Agent

    async def main():
        agent = Agent(api_key="sk-ant-...", model="claude-3-5-sonnet-20241022")
        response = await agent.run_async("What is 2 + 2?")
        print(response)

    asyncio.run(main())
"""

from .agent import Agent
from .session import Session, Message
from .skills import Skill, SkillRegistry

__version__ = "1.2.0"

__all__ = [
    "Agent",
    "Session",
    "Message",
    "Skill",
    "SkillRegistry",
    "__version__",
]

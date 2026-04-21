# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2024 Argentor contributors
"""Skill registration and management for Argentor agents."""

from __future__ import annotations

import inspect
import json
from typing import Any, Callable, Dict, List, Optional


class Skill:
    """A callable skill that the agent can invoke.

    Skills are registered with a name, description, and input schema. The agent
    passes tool-call arguments as keyword arguments to the underlying function.

    Example::

        def get_weather(city: str) -> str:
            return f"Sunny in {city}"

        skill = Skill(
            name="get_weather",
            description="Get the current weather for a city",
            fn=get_weather,
            parameters={
                "type": "object",
                "properties": {"city": {"type": "string", "description": "City name"}},
                "required": ["city"],
            },
        )
    """

    def __init__(
        self,
        name: str,
        description: str,
        fn: Callable,
        parameters: Optional[Dict] = None,
    ) -> None:
        self.name = name
        self.description = description
        self.fn = fn
        self.parameters: Dict = parameters or _infer_parameters(fn)

    def to_tool_dict(self) -> dict:
        """Return the Anthropic tool definition for this skill."""
        return {
            "name": self.name,
            "description": self.description,
            "input_schema": self.parameters,
        }

    async def invoke(self, **kwargs: Any) -> str:
        """Invoke the skill with keyword arguments, returning a string result."""
        if inspect.iscoroutinefunction(self.fn):
            result = await self.fn(**kwargs)
        else:
            result = self.fn(**kwargs)
        if isinstance(result, str):
            return result
        return json.dumps(result, ensure_ascii=False, default=str)

    def __repr__(self) -> str:
        return f"Skill(name={self.name!r})"


class SkillRegistry:
    """Central registry for discovering and invoking Argentor skills.

    Example::

        registry = SkillRegistry()

        def add(a: int, b: int) -> int:
            return a + b

        registry.register(
            name="add",
            description="Add two integers",
            fn=add,
            parameters={
                "type": "object",
                "properties": {
                    "a": {"type": "integer"},
                    "b": {"type": "integer"},
                },
                "required": ["a", "b"],
            },
        )

        result = await registry.execute("add", {"a": 1, "b": 2})
        print(result)  # "3"
    """

    def __init__(self) -> None:
        self._skills: Dict[str, Skill] = {}

    def register(
        self,
        name: str,
        description: str,
        fn: Callable,
        parameters: Optional[Dict] = None,
    ) -> Skill:
        """Register a callable as a named skill."""
        skill = Skill(name=name, description=description, fn=fn, parameters=parameters)
        self._skills[name] = skill
        return skill

    def add_skill(self, skill: Skill) -> None:
        """Add a pre-constructed Skill instance."""
        self._skills[skill.name] = skill

    def get(self, name: str) -> Optional[Skill]:
        """Retrieve a skill by name."""
        return self._skills.get(name)

    def list_skills(self) -> List[str]:
        """Return sorted list of registered skill names."""
        return sorted(self._skills)

    @property
    def skill_count(self) -> int:
        """Number of registered skills."""
        return len(self._skills)

    def to_tools(self) -> List[dict]:
        """Return all skills formatted as Anthropic tool definitions."""
        return [s.to_tool_dict() for s in self._skills.values()]

    async def execute(self, name: str, arguments: Dict[str, Any]) -> str:
        """Invoke a skill by name with the given arguments dict."""
        skill = self._skills.get(name)
        if skill is None:
            return json.dumps({"error": f"Unknown skill: {name!r}"})
        try:
            return await skill.invoke(**arguments)
        except Exception as exc:  # noqa: BLE001
            return json.dumps({"error": str(exc)})

    def __repr__(self) -> str:
        return f"SkillRegistry(skills={self.skill_count})"


def _infer_parameters(fn: Callable) -> Dict:
    """Infer a minimal JSON schema from a function's type annotations."""
    sig = inspect.signature(fn)
    properties: Dict[str, Any] = {}
    required: List[str] = []

    _type_map = {
        int: "integer",
        float: "number",
        bool: "boolean",
        str: "string",
        list: "array",
        dict: "object",
    }

    for param_name, param in sig.parameters.items():
        annotation = param.annotation
        json_type = "string"  # default
        if annotation in _type_map:
            json_type = _type_map[annotation]
        properties[param_name] = {"type": json_type}
        if param.default is inspect.Parameter.empty:
            required.append(param_name)

    return {
        "type": "object",
        "properties": properties,
        "required": required,
    }

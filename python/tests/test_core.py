# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2024 Argentor contributors

import asyncio
import json

import argentor
from argentor import Message, Session, SkillRegistry


def test_public_version_and_exports() -> None:
    assert argentor.__version__ == "1.3.0"
    assert Message(role="user", content="hello").to_dict() == {
        "role": "user",
        "content": "hello",
    }


def test_session_formats_api_messages_and_system_prompt() -> None:
    session = Session("test-session")

    system = session.add_system_message("Be concise.")
    user = session.add_user_message("Hello")
    assistant = session.add_assistant_message("Hi")

    assert system.session_id == "test-session"
    assert user.session_id == "test-session"
    assert assistant.session_id == "test-session"
    assert session.message_count == 3
    assert session.system_prompt() == "Be concise."
    assert session.api_messages() == [
        {"role": "user", "content": "Hello"},
        {"role": "assistant", "content": "Hi"},
    ]


def test_skill_registry_executes_and_reports_unknown_skills() -> None:
    registry = SkillRegistry()

    def add(a: int, b: int) -> int:
        return a + b

    registry.register("add", "Add two integers", add)

    assert registry.list_skills() == ["add"]
    assert registry.skill_count == 1
    assert asyncio.run(registry.execute("add", {"a": 2, "b": 3})) == "5"

    missing = asyncio.run(registry.execute("missing", {}))
    assert json.loads(missing) == {"error": "Unknown skill: 'missing'"}

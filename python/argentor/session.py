# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2024 Argentor contributors
"""Session and message management for Argentor agents."""

from __future__ import annotations

import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import List, Literal


RoleType = Literal["user", "assistant", "system", "tool"]


@dataclass
class Message:
    """A single message in a conversation session."""

    role: RoleType
    content: str
    timestamp: datetime = field(default_factory=lambda: datetime.now(timezone.utc))
    session_id: str = field(default_factory=lambda: str(uuid.uuid4()))

    def to_dict(self) -> dict:
        """Serialize to a dict compatible with Anthropic's messages API."""
        return {"role": self.role, "content": self.content}

    def __repr__(self) -> str:
        snippet = self.content[:40] + "..." if len(self.content) > 40 else self.content
        return f"Message(role={self.role!r}, content={snippet!r})"


class Session:
    """A conversation session that groups messages and tracks metadata.

    Example::

        session = Session()
        session.add_user_message("Hello!")
        session.add_assistant_message("Hi, how can I help?")
        print(session.message_count)
    """

    def __init__(self, session_id: str | None = None) -> None:
        self.id: str = session_id or str(uuid.uuid4())
        self.messages: List[Message] = []
        self.created_at: datetime = datetime.now(timezone.utc)
        self.updated_at: datetime = self.created_at

    def add_message(self, role: RoleType, content: str) -> Message:
        """Add a message with the given role and return it."""
        msg = Message(role=role, content=content, session_id=self.id)
        self.messages.append(msg)
        self.updated_at = datetime.now(timezone.utc)
        return msg

    def add_user_message(self, content: str) -> Message:
        """Append a user message."""
        return self.add_message("user", content)

    def add_assistant_message(self, content: str) -> Message:
        """Append an assistant message."""
        return self.add_message("assistant", content)

    def add_system_message(self, content: str) -> Message:
        """Append a system message."""
        return self.add_message("system", content)

    @property
    def message_count(self) -> int:
        """Number of messages in the session."""
        return len(self.messages)

    def api_messages(self) -> List[dict]:
        """Return messages formatted for the Anthropic messages API (no system role)."""
        return [m.to_dict() for m in self.messages if m.role != "system"]

    def system_prompt(self) -> str | None:
        """Return the first system message content, or None."""
        for m in self.messages:
            if m.role == "system":
                return m.content
        return None

    def clear(self) -> None:
        """Remove all messages from the session."""
        self.messages.clear()
        self.updated_at = datetime.now(timezone.utc)

    def __repr__(self) -> str:
        return f"Session(id={self.id!r}, messages={self.message_count})"

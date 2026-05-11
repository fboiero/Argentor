"""Synchronous and asynchronous HTTP clients for the Argentor REST API."""

from __future__ import annotations

import json
from typing import Any, AsyncIterator, Dict, Iterator, List, Optional

import httpx

from argentor.exceptions import (
    ArgentorAPIError,
    ArgentorConnectionError,
    ArgentorTimeoutError,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_DEFAULT_BASE_URL = "http://localhost:8080"
_DEFAULT_TIMEOUT = 60.0


def _build_headers(api_key: Optional[str], tenant_id: Optional[str]) -> Dict[str, str]:
    headers: Dict[str, str] = {"Content-Type": "application/json"}
    if api_key:
        headers["X-API-Key"] = api_key
    if tenant_id:
        headers["X-Tenant-ID"] = tenant_id
    return headers


def _handle_error(resp: httpx.Response) -> None:
    """Raise an ``ArgentorAPIError`` if *resp* is not 2xx."""
    if resp.is_success:
        return
    try:
        body = resp.json()
    except Exception:
        body = {"detail": resp.text}
    raise ArgentorAPIError(
        message=body.get("detail", body.get("error", resp.reason_phrase or "Unknown error")),
        status_code=resp.status_code,
        response_body=body,
    )


# ============================================================================
# Synchronous client
# ============================================================================


class ArgentorClient:
    """Synchronous client for the Argentor REST API.

    Usage::

        client = ArgentorClient(base_url="http://localhost:8080", api_key="sk-...")

        result = client.run_task(role="assistant", context="Hello!")
        print(result)

        client.close()

    The client can also be used as a context manager::

        with ArgentorClient(api_key="sk-...") as client:
            print(client.health())
    """

    def __init__(
        self,
        base_url: str = _DEFAULT_BASE_URL,
        api_key: Optional[str] = None,
        tenant_id: Optional[str] = None,
        timeout: float = _DEFAULT_TIMEOUT,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self._headers = _build_headers(api_key, tenant_id)
        try:
            self._http = httpx.Client(
                base_url=self.base_url,
                headers=self._headers,
                timeout=timeout,
            )
        except Exception as exc:
            raise ArgentorConnectionError(str(exc)) from exc

    # -- lifecycle -----------------------------------------------------------

    def close(self) -> None:
        """Close the underlying HTTP connection pool."""
        self._http.close()

    def __enter__(self) -> "ArgentorClient":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- agent / task endpoints ----------------------------------------------

    def run_task(
        self,
        role: str,
        context: str,
        *,
        model: Optional[str] = None,
        max_tokens: Optional[int] = None,
        tools: Optional[List[str]] = None,
    ) -> dict:
        """Execute a single agent task (``POST /v1/run``).

        Parameters
        ----------
        role:
            The agent role / persona to use (e.g. ``"code_reviewer"``).
        context:
            The user prompt or task description.
        model:
            Optional model override (e.g. ``"gpt-4"``).
        max_tokens:
            Maximum tokens for the response.
        tools:
            List of tool / skill names available to the agent.
        """
        payload: Dict[str, Any] = {"agent_role": role, "context": context}
        if model is not None:
            payload["model"] = model
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens
        if tools is not None:
            payload["tools"] = tools
        resp = self._http.post("/v1/run", json=payload)
        _handle_error(resp)
        return resp.json()

    def run_task_stream(
        self,
        role: str,
        context: str,
        *,
        model: Optional[str] = None,
        max_tokens: Optional[int] = None,
        tools: Optional[List[str]] = None,
    ) -> Iterator[dict]:
        """Stream task results via SSE (``POST /v1/run/stream``).

        Yields parsed JSON objects for each ``data:`` line. Stops when the
        server sends ``data: [DONE]``.
        """
        payload: Dict[str, Any] = {"agent_role": role, "context": context}
        if model is not None:
            payload["model"] = model
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens
        if tools is not None:
            payload["tools"] = tools
        with self._http.stream("POST", "/v1/run/stream", json=payload) as resp:
            _handle_error(resp)
            for line in resp.iter_lines():
                if line.startswith("data: "):
                    data = line[len("data: ") :].strip()
                    if data == "[DONE]":
                        return
                    yield json.loads(data)

    def batch_tasks(
        self,
        tasks: List[Dict[str, Any]],
        *,
        max_concurrent: int = 5,
    ) -> dict:
        """Submit a batch of tasks (``POST /v1/batch``).

        *tasks* is a list of dicts, each with at least ``agent_role`` and
        ``context`` keys.
        """
        payload = {"tasks": tasks, "max_concurrent": max_concurrent}
        resp = self._http.post("/v1/batch", json=payload)
        _handle_error(resp)
        return resp.json()

    def evaluate(
        self,
        text: str,
        context: str = "",
        criteria: Optional[List[str]] = None,
    ) -> dict:
        """Evaluate text against criteria (``POST /v1/evaluate``)."""
        payload: Dict[str, Any] = {"response": text, "context": context}
        if criteria is not None:
            payload["criteria"] = criteria
        resp = self._http.post("/v1/evaluate", json=payload)
        _handle_error(resp)
        return resp.json()

    # -- agent chat ----------------------------------------------------------

    def agent_chat(
        self,
        message: str,
        *,
        session_id: Optional[str] = None,
        model: Optional[str] = None,
        max_tokens: Optional[int] = None,
    ) -> dict:
        """Send a message through the agent chat endpoint (``POST /api/v1/agent/chat``)."""
        payload: Dict[str, Any] = {"message": message}
        if session_id is not None:
            payload["session_id"] = session_id
        if model is not None:
            payload["model"] = model
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens
        resp = self._http.post("/api/v1/agent/chat", json=payload)
        _handle_error(resp)
        return resp.json()

    def agent_status(self) -> dict:
        """Get agent status (``GET /api/v1/agent/status``)."""
        resp = self._http.get("/api/v1/agent/status")
        _handle_error(resp)
        return resp.json()

    # -- session endpoints ---------------------------------------------------

    def create_session(self) -> dict:
        """Create a new session (``POST /api/v1/sessions``).

        Note: the gateway may create sessions implicitly; this method
        provides an explicit way to obtain a session ID.
        """
        resp = self._http.post("/api/v1/sessions", json={})
        _handle_error(resp)
        return resp.json()

    def get_session(self, session_id: str) -> dict:
        """Retrieve a session by ID (``GET /api/v1/sessions/{id}``)."""
        resp = self._http.get(f"/api/v1/sessions/{session_id}")
        _handle_error(resp)
        return resp.json()

    def list_sessions(self) -> list:
        """List all sessions (``GET /api/v1/sessions``)."""
        resp = self._http.get("/api/v1/sessions")
        _handle_error(resp)
        return resp.json()

    def delete_session(self, session_id: str) -> None:
        """Delete a session (``DELETE /api/v1/sessions/{id}``)."""
        resp = self._http.delete(f"/api/v1/sessions/{session_id}")
        _handle_error(resp)

    # -- skill endpoints -----------------------------------------------------

    def list_skills(self) -> list:
        """List registered skills (``GET /api/v1/skills``)."""
        resp = self._http.get("/api/v1/skills")
        _handle_error(resp)
        return resp.json()

    def get_skill(self, name: str) -> dict:
        """Get details for a specific skill (``GET /api/v1/skills/{name}``)."""
        resp = self._http.get(f"/api/v1/skills/{name}")
        _handle_error(resp)
        return resp.json()

    def execute_skill(self, name: str, arguments: Optional[Dict[str, Any]] = None) -> dict:
        """Execute a skill by name (``POST /api/v1/skills/{name}/execute``).

        Parameters
        ----------
        name:
            Skill name (e.g. ``"echo"``, ``"memory_search"``).
        arguments:
            Key-value arguments passed to the skill.
        """
        payload = {"arguments": arguments or {}}
        resp = self._http.post(f"/api/v1/skills/{name}/execute", json=payload)
        _handle_error(resp)
        return resp.json()

    # -- health & metrics ----------------------------------------------------

    def health(self) -> dict:
        """Check API server health (``GET /health``)."""
        resp = self._http.get("/health")
        _handle_error(resp)
        return resp.json()

    def health_ready(self) -> dict:
        """Readiness probe (``GET /health/ready``)."""
        resp = self._http.get("/health/ready")
        _handle_error(resp)
        return resp.json()

    def metrics(self) -> str:
        """Retrieve Prometheus-format metrics (``GET /api/v1/metrics``)."""
        resp = self._http.get("/api/v1/metrics")
        _handle_error(resp)
        return resp.text

    def enterprise_readiness(self) -> dict:
        """Retrieve the enterprise readiness report (``GET /api/v1/enterprise/readiness``)."""
        resp = self._http.get("/api/v1/enterprise/readiness")
        _handle_error(resp)
        return resp.json()

    # -- connections ---------------------------------------------------------

    def list_connections(self) -> list:
        """List active WebSocket connections (``GET /api/v1/connections``)."""
        resp = self._http.get("/api/v1/connections")
        _handle_error(resp)
        return resp.json()

    # -- personas ------------------------------------------------------------

    def create_persona(
        self,
        tenant_id: str,
        agent_role: str,
        persona: Dict[str, Any],
    ) -> dict:
        """Create a new agent persona (``POST /v1/personas``)."""
        payload = {"tenant_id": tenant_id, "agent_role": agent_role, "persona": persona}
        resp = self._http.post("/v1/personas", json=payload)
        _handle_error(resp)
        return resp.json()

    def list_personas(self, tenant_id: str) -> dict:
        """List personas for a tenant (``GET /v1/personas``)."""
        resp = self._http.get("/v1/personas", params={"tenant_id": tenant_id})
        _handle_error(resp)
        return resp.json()

    # -- usage ---------------------------------------------------------------

    def get_usage(self, tenant_id: str) -> dict:
        """Get usage breakdown for a tenant (``GET /v1/usage/{tenant_id}``)."""
        resp = self._http.get(f"/v1/usage/{tenant_id}")
        _handle_error(resp)
        return resp.json()

    # -- webhooks ------------------------------------------------------------

    def webhook_proxy(
        self,
        event: str,
        data: Dict[str, Any],
        *,
        source: str = "",
        secret: str = "",
    ) -> dict:
        """Forward a webhook event (``POST /v1/webhooks/proxy``)."""
        payload: Dict[str, Any] = {"event": event, "data": data}
        if source:
            payload["source"] = source
        if secret:
            payload["secret"] = secret
        resp = self._http.post("/v1/webhooks/proxy", json=payload)
        _handle_error(resp)
        return resp.json()

    # -- marketplace ---------------------------------------------------------

    def search_marketplace(
        self,
        query: Optional[str] = None,
        category: Optional[str] = None,
    ) -> list:
        """Search the skill marketplace (``GET /v1/marketplace/search``)."""
        params: Dict[str, str] = {}
        if query:
            params["q"] = query
        if category:
            params["category"] = category
        resp = self._http.get("/v1/marketplace/search", params=params)
        _handle_error(resp)
        return resp.json()

    def install_skill(self, name: str) -> dict:
        """Install a skill from the marketplace (``POST /v1/marketplace/install``)."""
        resp = self._http.post("/v1/marketplace/install", json={"name": name})
        _handle_error(resp)
        return resp.json()


# ============================================================================
# Async client
# ============================================================================


class AsyncArgentorClient:
    """Asynchronous client for the Argentor REST API.

    Usage::

        async with AsyncArgentorClient(api_key="sk-...") as client:
            result = await client.run_task(role="assistant", context="Hello!")
            print(result)
    """

    def __init__(
        self,
        base_url: str = _DEFAULT_BASE_URL,
        api_key: Optional[str] = None,
        tenant_id: Optional[str] = None,
        timeout: float = _DEFAULT_TIMEOUT,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self._headers = _build_headers(api_key, tenant_id)
        self._http = httpx.AsyncClient(
            base_url=self.base_url,
            headers=self._headers,
            timeout=timeout,
        )

    # -- lifecycle -----------------------------------------------------------

    async def close(self) -> None:
        """Close the underlying async HTTP connection pool."""
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncArgentorClient":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.close()

    # -- agent / task endpoints ----------------------------------------------

    async def run_task(
        self,
        role: str,
        context: str,
        *,
        model: Optional[str] = None,
        max_tokens: Optional[int] = None,
        tools: Optional[List[str]] = None,
    ) -> dict:
        """Execute a single agent task (``POST /v1/run``)."""
        payload: Dict[str, Any] = {"agent_role": role, "context": context}
        if model is not None:
            payload["model"] = model
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens
        if tools is not None:
            payload["tools"] = tools
        resp = await self._http.post("/v1/run", json=payload)
        _handle_error(resp)
        return resp.json()

    async def run_task_stream(
        self,
        role: str,
        context: str,
        *,
        model: Optional[str] = None,
        max_tokens: Optional[int] = None,
        tools: Optional[List[str]] = None,
    ) -> AsyncIterator[dict]:
        """Stream task results via SSE (``POST /v1/run/stream``)."""
        payload: Dict[str, Any] = {"agent_role": role, "context": context}
        if model is not None:
            payload["model"] = model
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens
        if tools is not None:
            payload["tools"] = tools
        async with self._http.stream("POST", "/v1/run/stream", json=payload) as resp:
            _handle_error(resp)
            async for line in resp.aiter_lines():
                if line.startswith("data: "):
                    data = line[len("data: ") :].strip()
                    if data == "[DONE]":
                        return
                    yield json.loads(data)

    async def batch_tasks(
        self,
        tasks: List[Dict[str, Any]],
        *,
        max_concurrent: int = 5,
    ) -> dict:
        """Submit a batch of tasks (``POST /v1/batch``)."""
        payload = {"tasks": tasks, "max_concurrent": max_concurrent}
        resp = await self._http.post("/v1/batch", json=payload)
        _handle_error(resp)
        return resp.json()

    async def evaluate(
        self,
        text: str,
        context: str = "",
        criteria: Optional[List[str]] = None,
    ) -> dict:
        """Evaluate text against criteria (``POST /v1/evaluate``)."""
        payload: Dict[str, Any] = {"response": text, "context": context}
        if criteria is not None:
            payload["criteria"] = criteria
        resp = await self._http.post("/v1/evaluate", json=payload)
        _handle_error(resp)
        return resp.json()

    # -- agent chat ----------------------------------------------------------

    async def agent_chat(
        self,
        message: str,
        *,
        session_id: Optional[str] = None,
        model: Optional[str] = None,
        max_tokens: Optional[int] = None,
    ) -> dict:
        """Send a message through the agent chat endpoint (``POST /api/v1/agent/chat``)."""
        payload: Dict[str, Any] = {"message": message}
        if session_id is not None:
            payload["session_id"] = session_id
        if model is not None:
            payload["model"] = model
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens
        resp = await self._http.post("/api/v1/agent/chat", json=payload)
        _handle_error(resp)
        return resp.json()

    async def agent_status(self) -> dict:
        """Get agent status (``GET /api/v1/agent/status``)."""
        resp = await self._http.get("/api/v1/agent/status")
        _handle_error(resp)
        return resp.json()

    # -- session endpoints ---------------------------------------------------

    async def create_session(self) -> dict:
        """Create a new session (``POST /api/v1/sessions``)."""
        resp = await self._http.post("/api/v1/sessions", json={})
        _handle_error(resp)
        return resp.json()

    async def get_session(self, session_id: str) -> dict:
        """Retrieve a session by ID (``GET /api/v1/sessions/{id}``)."""
        resp = await self._http.get(f"/api/v1/sessions/{session_id}")
        _handle_error(resp)
        return resp.json()

    async def list_sessions(self) -> list:
        """List all sessions (``GET /api/v1/sessions``)."""
        resp = await self._http.get("/api/v1/sessions")
        _handle_error(resp)
        return resp.json()

    async def delete_session(self, session_id: str) -> None:
        """Delete a session (``DELETE /api/v1/sessions/{id}``)."""
        resp = await self._http.delete(f"/api/v1/sessions/{session_id}")
        _handle_error(resp)

    # -- skill endpoints -----------------------------------------------------

    async def list_skills(self) -> list:
        """List registered skills (``GET /api/v1/skills``)."""
        resp = await self._http.get("/api/v1/skills")
        _handle_error(resp)
        return resp.json()

    async def get_skill(self, name: str) -> dict:
        """Get details for a specific skill (``GET /api/v1/skills/{name}``)."""
        resp = await self._http.get(f"/api/v1/skills/{name}")
        _handle_error(resp)
        return resp.json()

    async def execute_skill(self, name: str, arguments: Optional[Dict[str, Any]] = None) -> dict:
        """Execute a skill by name (``POST /api/v1/skills/{name}/execute``)."""
        payload = {"arguments": arguments or {}}
        resp = await self._http.post(f"/api/v1/skills/{name}/execute", json=payload)
        _handle_error(resp)
        return resp.json()

    # -- health & metrics ----------------------------------------------------

    async def health(self) -> dict:
        """Check API server health (``GET /health``)."""
        resp = await self._http.get("/health")
        _handle_error(resp)
        return resp.json()

    async def health_ready(self) -> dict:
        """Readiness probe (``GET /health/ready``)."""
        resp = await self._http.get("/health/ready")
        _handle_error(resp)
        return resp.json()

    async def metrics(self) -> str:
        """Retrieve Prometheus-format metrics (``GET /api/v1/metrics``)."""
        resp = await self._http.get("/api/v1/metrics")
        _handle_error(resp)
        return resp.text

    async def enterprise_readiness(self) -> dict:
        """Retrieve the enterprise readiness report (``GET /api/v1/enterprise/readiness``)."""
        resp = await self._http.get("/api/v1/enterprise/readiness")
        _handle_error(resp)
        return resp.json()

    # -- connections ---------------------------------------------------------

    async def list_connections(self) -> list:
        """List active WebSocket connections (``GET /api/v1/connections``)."""
        resp = await self._http.get("/api/v1/connections")
        _handle_error(resp)
        return resp.json()

    # -- personas ------------------------------------------------------------

    async def create_persona(
        self,
        tenant_id: str,
        agent_role: str,
        persona: Dict[str, Any],
    ) -> dict:
        """Create a new agent persona (``POST /v1/personas``)."""
        payload = {"tenant_id": tenant_id, "agent_role": agent_role, "persona": persona}
        resp = await self._http.post("/v1/personas", json=payload)
        _handle_error(resp)
        return resp.json()

    async def list_personas(self, tenant_id: str) -> dict:
        """List personas for a tenant (``GET /v1/personas``)."""
        resp = await self._http.get("/v1/personas", params={"tenant_id": tenant_id})
        _handle_error(resp)
        return resp.json()

    # -- usage ---------------------------------------------------------------

    async def get_usage(self, tenant_id: str) -> dict:
        """Get usage breakdown for a tenant (``GET /v1/usage/{tenant_id}``)."""
        resp = await self._http.get(f"/v1/usage/{tenant_id}")
        _handle_error(resp)
        return resp.json()

    # -- webhooks ------------------------------------------------------------

    async def webhook_proxy(
        self,
        event: str,
        data: Dict[str, Any],
        *,
        source: str = "",
        secret: str = "",
    ) -> dict:
        """Forward a webhook event (``POST /v1/webhooks/proxy``)."""
        payload: Dict[str, Any] = {"event": event, "data": data}
        if source:
            payload["source"] = source
        if secret:
            payload["secret"] = secret
        resp = await self._http.post("/v1/webhooks/proxy", json=payload)
        _handle_error(resp)
        return resp.json()

    # -- marketplace ---------------------------------------------------------

    async def search_marketplace(
        self,
        query: Optional[str] = None,
        category: Optional[str] = None,
    ) -> list:
        """Search the skill marketplace (``GET /v1/marketplace/search``)."""
        params: Dict[str, str] = {}
        if query:
            params["q"] = query
        if category:
            params["category"] = category
        resp = await self._http.get("/v1/marketplace/search", params=params)
        _handle_error(resp)
        return resp.json()

    async def install_skill(self, name: str) -> dict:
        """Install a skill from the marketplace (``POST /v1/marketplace/install``)."""
        resp = await self._http.post("/v1/marketplace/install", json={"name": name})
        _handle_error(resp)
        return resp.json()

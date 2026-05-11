"""Pydantic models for Argentor API request and response types."""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field


# ---------------------------------------------------------------------------
# Run Task
# ---------------------------------------------------------------------------


class RunTaskRequest(BaseModel):
    """Request body for POST /v1/run."""

    agent_role: str
    context: str
    model: Optional[str] = None
    max_tokens: Optional[int] = None
    tools: Optional[List[str]] = None


class RunTaskResponse(BaseModel):
    """Response from POST /v1/run."""

    task_id: str
    status: str
    result: Optional[str] = None
    tokens_used: Optional[int] = None
    duration_ms: Optional[int] = None
    metadata: Optional[Dict[str, Any]] = None


class StreamEvent(BaseModel):
    """A single event from a POST /v1/run/stream SSE response."""

    event: Optional[str] = None
    data: Optional[Dict[str, Any]] = None
    content: Optional[str] = None
    done: bool = False


# ---------------------------------------------------------------------------
# Batch
# ---------------------------------------------------------------------------


class BatchTask(BaseModel):
    """A single task within a batch request."""

    agent_role: str
    context: str
    model: Optional[str] = None
    max_tokens: Optional[int] = None


class BatchRequest(BaseModel):
    """Request body for POST /v1/batch."""

    tasks: List[BatchTask]
    max_concurrent: int = Field(default=5, ge=1, le=50)


class BatchTaskResult(BaseModel):
    """Result for one task in a batch."""

    task_id: str
    status: str
    result: Optional[str] = None
    error: Optional[str] = None


class BatchResponse(BaseModel):
    """Response from POST /v1/batch."""

    batch_id: str
    results: List[BatchTaskResult]
    total: int
    succeeded: int
    failed: int


# ---------------------------------------------------------------------------
# Evaluate
# ---------------------------------------------------------------------------


class EvaluateRequest(BaseModel):
    """Request body for POST /v1/evaluate."""

    response: str
    context: str
    criteria: Optional[List[str]] = None


class CriterionScore(BaseModel):
    """Score for a single evaluation criterion."""

    criterion: str
    score: float = Field(ge=0.0, le=1.0)
    explanation: Optional[str] = None


class EvaluateResponse(BaseModel):
    """Response from POST /v1/evaluate."""

    overall_score: float = Field(ge=0.0, le=1.0)
    scores: List[CriterionScore]
    summary: Optional[str] = None


# ---------------------------------------------------------------------------
# Sessions
# ---------------------------------------------------------------------------


class SessionInfo(BaseModel):
    """Session metadata returned by session endpoints."""

    session_id: str
    created_at: Optional[str] = None
    updated_at: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None


# ---------------------------------------------------------------------------
# Skills
# ---------------------------------------------------------------------------


class SkillParameter(BaseModel):
    """A parameter accepted by a skill."""

    name: str
    description: Optional[str] = None
    required: bool = False
    param_type: Optional[str] = Field(default=None, alias="type")


class SkillDescriptor(BaseModel):
    """Description of a registered skill."""

    name: str
    description: Optional[str] = None
    parameters: Optional[List[SkillParameter]] = None
    version: Optional[str] = None


class ExecuteSkillRequest(BaseModel):
    """Request body for POST /api/v1/skills/{name}/execute."""

    arguments: Dict[str, Any] = Field(default_factory=dict)


class ToolResult(BaseModel):
    """Response from executing a skill."""

    success: bool
    output: Optional[str] = None
    error: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None


# ---------------------------------------------------------------------------
# Health & Metrics
# ---------------------------------------------------------------------------


class HealthResponse(BaseModel):
    """Response from GET /health."""

    status: str
    version: Optional[str] = None
    uptime_seconds: Optional[float] = None


class ReadinessResponse(BaseModel):
    """Response from GET /health/ready."""

    ready: bool
    checks: Optional[Dict[str, Any]] = None


class EnterpriseRuntimeSnapshot(BaseModel):
    """Runtime counters in the enterprise readiness report."""

    skills_registered: int
    active_connections: int
    active_sessions: int
    uptime_seconds: int


class EnterpriseReadinessCheck(BaseModel):
    """A single enterprise readiness check."""

    id: str
    category: str
    title: str
    status: str
    detail: str


class EnterpriseReadinessReport(BaseModel):
    """Response from GET /api/v1/enterprise/readiness."""

    version: str
    posture: str
    score: int
    runtime: EnterpriseRuntimeSnapshot
    checks: List[EnterpriseReadinessCheck]
    next_actions: List[str]


# ---------------------------------------------------------------------------
# Agent Chat
# ---------------------------------------------------------------------------


class AgentChatRequest(BaseModel):
    """Request body for POST /api/v1/agent/chat."""

    message: str
    session_id: Optional[str] = None
    model: Optional[str] = None
    max_tokens: Optional[int] = None


class AgentChatResponse(BaseModel):
    """Response from POST /api/v1/agent/chat."""

    response: Optional[str] = None
    session_id: Optional[str] = None
    tokens_used: Optional[int] = None
    metadata: Optional[Dict[str, Any]] = None


# ---------------------------------------------------------------------------
# Connections
# ---------------------------------------------------------------------------


class ConnectionInfo(BaseModel):
    """Information about an active WebSocket connection."""

    connection_id: str
    connected_at: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None


# ---------------------------------------------------------------------------
# Marketplace
# ---------------------------------------------------------------------------


class MarketplaceEntry(BaseModel):
    """A skill entry returned by the marketplace search."""

    name: str
    description: Optional[str] = None
    version: Optional[str] = None
    author: Optional[str] = None
    category: Optional[str] = None
    downloads: Optional[int] = None
    rating: Optional[float] = None


class InstallSkillResponse(BaseModel):
    """Response from POST /v1/marketplace/install."""

    success: bool
    name: str
    version: Optional[str] = None
    message: Optional[str] = None

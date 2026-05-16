"""Tests for Argentor SDK Pydantic models."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from argentor.models import (
    AgentChatRequest,
    AgentChatResponse,
    BatchRequest,
    BatchResponse,
    BatchTask,
    BatchTaskResult,
    ConnectionInfo,
    CriterionScore,
    EvaluateRequest,
    EvaluateResponse,
    ExecuteSkillRequest,
    HealthResponse,
    InstallSkillResponse,
    MarketplaceEntry,
    ReadinessResponse,
    EnterpriseReadinessCheck,
    EnterpriseReadinessReport,
    EnterpriseRuntimeSnapshot,
    RunTaskRequest,
    RunTaskResponse,
    SessionInfo,
    SkillDescriptor,
    SkillParameter,
    StreamEvent,
    ToolResult,
)


# ---------------------------------------------------------------------------
# RunTask models
# ---------------------------------------------------------------------------


class TestRunTaskRequest:
    def test_minimal(self):
        req = RunTaskRequest(agent_role="assistant", context="Hello")
        assert req.agent_role == "assistant"
        assert req.context == "Hello"
        assert req.model is None
        assert req.tools is None

    def test_full(self):
        req = RunTaskRequest(
            agent_role="reviewer",
            context="Review PR",
            model="gpt-4",
            max_tokens=1024,
            tools=["echo", "time"],
        )
        assert req.model == "gpt-4"
        assert req.max_tokens == 1024
        assert len(req.tools) == 2

    def test_serialization_roundtrip(self):
        req = RunTaskRequest(agent_role="a", context="c", model="m")
        data = req.model_dump()
        restored = RunTaskRequest(**data)
        assert restored == req


class TestRunTaskResponse:
    def test_minimal(self):
        resp = RunTaskResponse(task_id="t1", status="completed")
        assert resp.task_id == "t1"
        assert resp.result is None

    def test_full(self):
        resp = RunTaskResponse(
            task_id="t1",
            status="completed",
            result="done",
            tokens_used=100,
            duration_ms=500,
            metadata={"key": "value"},
        )
        assert resp.tokens_used == 100
        assert resp.metadata["key"] == "value"


# ---------------------------------------------------------------------------
# StreamEvent
# ---------------------------------------------------------------------------


class TestStreamEvent:
    def test_defaults(self):
        ev = StreamEvent()
        assert ev.event is None
        assert ev.done is False

    def test_done_event(self):
        ev = StreamEvent(event="complete", done=True, content="final text")
        assert ev.done is True
        assert ev.content == "final text"


# ---------------------------------------------------------------------------
# Batch models
# ---------------------------------------------------------------------------


class TestBatchModels:
    def test_batch_task(self):
        task = BatchTask(agent_role="analyst", context="Analyze data")
        assert task.agent_role == "analyst"

    def test_batch_request_defaults(self):
        req = BatchRequest(tasks=[BatchTask(agent_role="a", context="c")])
        assert req.max_concurrent == 5

    def test_batch_request_max_concurrent_validation(self):
        with pytest.raises(ValidationError):
            BatchRequest(
                tasks=[BatchTask(agent_role="a", context="c")],
                max_concurrent=0,
            )

    def test_batch_response(self):
        resp = BatchResponse(
            batch_id="b1",
            results=[BatchTaskResult(task_id="t1", status="ok")],
            total=1,
            succeeded=1,
            failed=0,
        )
        assert resp.batch_id == "b1"
        assert len(resp.results) == 1


# ---------------------------------------------------------------------------
# Evaluate models
# ---------------------------------------------------------------------------


class TestEvaluateModels:
    def test_evaluate_request(self):
        req = EvaluateRequest(response="good code", context="review")
        assert req.response == "good code"

    def test_criterion_score_bounds(self):
        score = CriterionScore(criterion="accuracy", score=0.85)
        assert score.score == 0.85

    def test_criterion_score_validation(self):
        with pytest.raises(ValidationError):
            CriterionScore(criterion="accuracy", score=1.5)

    def test_evaluate_response(self):
        resp = EvaluateResponse(
            overall_score=0.9,
            scores=[CriterionScore(criterion="accuracy", score=0.9)],
            summary="Good overall",
        )
        assert resp.overall_score == 0.9


# ---------------------------------------------------------------------------
# Session, Skill, Health, etc.
# ---------------------------------------------------------------------------


class TestSessionInfo:
    def test_minimal(self):
        s = SessionInfo(session_id="s-1")
        assert s.session_id == "s-1"
        assert s.created_at is None

    def test_json_roundtrip(self):
        s = SessionInfo(session_id="s-1", created_at="2025-01-01T00:00:00Z")
        data = s.model_dump_json()
        restored = SessionInfo.model_validate_json(data)
        assert restored.session_id == "s-1"


class TestSkillModels:
    def test_skill_parameter(self):
        p = SkillParameter(name="text", description="input text", required=True)
        assert p.required is True

    def test_skill_parameter_alias(self):
        p = SkillParameter.model_validate({"name": "x", "type": "string"})
        assert p.param_type == "string"

    def test_skill_descriptor(self):
        sd = SkillDescriptor(
            name="echo",
            description="Echo back",
            parameters=[SkillParameter(name="text")],
            version="1.0",
        )
        assert sd.name == "echo"
        assert len(sd.parameters) == 1


class TestHealthModels:
    def test_health_response(self):
        h = HealthResponse(status="ok", version="1.0.0", uptime_seconds=3600.0)
        assert h.status == "ok"

    def test_readiness_response(self):
        r = ReadinessResponse(ready=True, checks={"db": "ok"})
        assert r.ready is True

    def test_enterprise_readiness_report(self):
        report = EnterpriseReadinessReport(
            version="1.4.7",
            posture="ready",
            score=86,
            runtime=EnterpriseRuntimeSnapshot(
                skills_registered=42,
                active_connections=1,
                active_sessions=1,
                uptime_seconds=60,
            ),
            checks=[
                EnterpriseReadinessCheck(
                    id="rest_api",
                    category="runtime",
                    title="REST API mounted",
                    status="active",
                    detail="REST management endpoints are available.",
                )
            ],
            next_actions=["Run the enterprise golden path smoke test."],
        )
        assert report.runtime.skills_registered == 42
        assert report.checks[0].status == "active"


class TestMiscModels:
    def test_execute_skill_request_default(self):
        req = ExecuteSkillRequest()
        assert req.arguments == {}

    def test_tool_result(self):
        tr = ToolResult(success=True, output="hello")
        assert tr.success is True

    def test_agent_chat_request(self):
        req = AgentChatRequest(message="hi")
        assert req.message == "hi"
        assert req.session_id is None

    def test_agent_chat_response(self):
        resp = AgentChatResponse(response="hello", session_id="s1", tokens_used=10)
        assert resp.tokens_used == 10

    def test_connection_info(self):
        ci = ConnectionInfo(connection_id="c1", connected_at="2025-01-01")
        assert ci.connection_id == "c1"

    def test_marketplace_entry(self):
        me = MarketplaceEntry(name="cool-skill", version="0.1.0", rating=4.5)
        assert me.rating == 4.5

    def test_install_skill_response(self):
        resp = InstallSkillResponse(success=True, name="cool-skill", version="0.1.0")
        assert resp.success is True
        assert resp.name == "cool-skill"

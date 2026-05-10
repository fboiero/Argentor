"""argentor-sdk -- Python client for the Argentor REST API."""

from argentor.client import ArgentorClient, AsyncArgentorClient
from argentor.agent import (
    query,
    query_simple,
    AgentOptions,
    AgentEvent,
    ask_claude,
    ask_openai,
    ask_gemini,
    ask_ollama,
)
from argentor.models import (
    RunTaskRequest,
    RunTaskResponse,
    StreamEvent,
    BatchTask,
    BatchRequest,
    BatchTaskResult,
    BatchResponse,
    EvaluateRequest,
    CriterionScore,
    EvaluateResponse,
    SessionInfo,
    SkillDescriptor,
    SkillParameter,
    ExecuteSkillRequest,
    ToolResult,
    HealthResponse,
    ReadinessResponse,
    MarketplaceEntry,
    InstallSkillResponse,
    AgentChatRequest,
    AgentChatResponse,
    ConnectionInfo,
)
from argentor.streaming import SSEStream, AsyncSSEStream

__version__ = "1.3.0"

__all__ = [
    # Clients -- REST API
    "ArgentorClient",
    "AsyncArgentorClient",
    # Agent -- subprocess wrapper
    "query",
    "query_simple",
    "AgentOptions",
    "AgentEvent",
    "ask_claude",
    "ask_openai",
    "ask_gemini",
    "ask_ollama",
    # Streaming
    "SSEStream",
    "AsyncSSEStream",
    # Models -- requests
    "RunTaskRequest",
    "BatchTask",
    "BatchRequest",
    "EvaluateRequest",
    "ExecuteSkillRequest",
    "AgentChatRequest",
    # Models -- responses
    "RunTaskResponse",
    "StreamEvent",
    "BatchTaskResult",
    "BatchResponse",
    "CriterionScore",
    "EvaluateResponse",
    "SessionInfo",
    "SkillDescriptor",
    "SkillParameter",
    "ToolResult",
    "HealthResponse",
    "ReadinessResponse",
    "MarketplaceEntry",
    "InstallSkillResponse",
    "AgentChatResponse",
    "ConnectionInfo",
]

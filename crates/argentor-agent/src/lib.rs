//! Agent runner with LLM backend abstraction, failover, streaming, and context management.
//!
//! This crate implements the core agentic loop that drives Argentor agents,
//! including multi-provider LLM backends, automatic failover, token-aware
//! context windowing, and streaming event support.
//!
//! # Main types
//!
//! - [`AgentRunner`] — Executes the agentic loop: prompt, call tools, respond.
//! - [`ModelConfig`] — Configuration for model provider, name, and parameters.
//! - [`LlmProvider`] — Enum of supported LLM providers (OpenAI, Anthropic, etc.).
//! - [`ContextWindow`] — Token-aware sliding window over conversation history.
//! - [`StreamEvent`] — Events emitted during streamed agent responses.
//! - [`FailoverBackend`] — Multi-backend wrapper with automatic retry and failover.

/// Adaptive memory integration for automatic context recall across sessions.
pub mod adaptive_memory;
/// LLM backend implementations.
pub mod backends;
/// Batch processor for grouping and executing multiple LLM requests.
pub mod batch_processor;
/// Agent benchmark suite for measuring performance, quality, and cost.
pub mod benchmark;
/// State checkpointing for save/restore of complete agent state (LangGraph-style time-travel).
pub mod checkpoint;
/// Circuit breaker for LLM provider resilience.
pub mod circuit_breaker;
/// Lightweight code structure analysis: symbols, dependencies, call graph.
pub mod code_graph;
/// Implementation planning with dependency ordering and risk assessment.
pub mod code_planner;
/// Automatic context compaction — summarizes conversation when approaching token limits.
pub mod compaction;
/// Model and provider configuration.
pub mod config;
/// Token-aware context windowing.
pub mod context;
/// Automatic system prompt assembly from project context (git, config files, tools).
pub mod context_assembly;
/// Intelligent cost optimization for LLM routing.
pub mod cost_optimizer;
/// Self-critique loop — Reflexion pattern for reviewing and revising agent responses.
pub mod critique;
/// Debug recorder for step-by-step agent reasoning traces.
pub mod debug_recorder;
/// Precise diff generation, application, and validation.
pub mod diff_engine;
/// Standardized evaluation framework for benchmarking agent performance.
pub mod eval_framework;
/// Self-evaluation engine for agent response quality.
pub mod evaluator;
/// Failover and retry logic for LLM backends.
pub mod failover;
/// Production-grade guardrails for filtering, validating, and sanitizing LLM inputs/outputs.
pub mod guardrails;
/// Hook system for intercepting tool calls and agent events (pre/post tool use, LLM calls).
pub mod hooks;
/// Agent identity, personality, session commands, and context compaction.
pub mod identity;
/// Learning feedback loop — tool selector that improves over time with execution outcomes.
pub mod learning;
/// LLM client trait and HTTP transport.
pub mod llm;
/// Cost-aware model routing for multi-tier LLM selection.
pub mod model_router;
/// Multimodal message types (text + images) for vision-capable LLMs.
pub mod multimodal;
/// Permission modes for global agent tool authorization control.
pub mod permission_mode;
/// Versioned prompt template management with A/B testing and chains.
pub mod prompt_manager;
/// NDJSON protocol for headless agent communication (SDK wrapping via stdin/stdout).
pub mod protocol;
/// Universal high-level query API — model-agnostic, works with all 14 providers.
pub mod query;
/// ReAct (Reasoning + Acting) engine for structured agent reasoning.
pub mod react;
/// In-memory LRU response cache for LLM calls with TTL expiration.
pub mod response_cache;
/// Transparent caching layer that wraps any `LlmBackend` with SHA-256 keyed LRU caching.
pub mod response_cache_layer;
/// Multi-dimensional code review engine (security, performance, style, correctness).
pub mod review_engine;
/// Process reward scoring — scores each reasoning step, not just the final output.
pub mod reward;
/// Agent runner and agentic loop.
pub mod runner;
/// Streaming event types.
pub mod stream;
/// Structured output parsing and validation for LLM responses.
pub mod structured_output;
/// Test output parsing and TDD loop automation.
pub mod test_oracle;
/// Extended thinking mode — test-time compute scaling for deeper reasoning.
pub mod thinking;
/// Token counting and cost estimation for different LLM providers.
pub mod token_counter;
/// Dynamic tool discovery — semantic search for relevant tools instead of loading all.
pub mod tool_discovery;
/// Smart tool selection to reduce token waste and improve relevance.
pub mod tool_selector;
/// Vision-capable LLM backends (Claude, OpenAI, Gemini).
pub mod vision_backends;
/// Voice (STT/TTS) types and traits.
pub mod voice;
/// Voice backends (Whisper, Deepgram, OpenAI TTS, ElevenLabs).
pub mod voice_backends;
/// Webhook notification system for agent events.
pub mod webhooks;

pub use adaptive_memory::{
    AdaptiveMemory, AdaptiveMemoryConfig, MemoryEntry, MemoryKind, RecallResult,
};
pub use backends::LlmBackend;
pub use batch_processor::{
    BatchConfig, BatchProcessor, BatchProcessorStats, BatchRequest, BatchResult, RequestResult,
    RequestStatus,
};
pub use benchmark::{
    BenchmarkCase, BenchmarkCategory, BenchmarkComparisonReport, BenchmarkReport, BenchmarkResult,
    BenchmarkSuite, CategoryStats, MockBenchmarkBackend, Regression,
};
pub use checkpoint::{
    AgentState, Checkpoint, CheckpointConfig, CheckpointDiff, CheckpointManager, CheckpointMessage,
    ModelSnapshot, ToolCallSnapshot,
};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerRegistry, CircuitBreakerStatus, CircuitConfig, CircuitState,
};
pub use code_graph::{
    CodeContext, CodeGraph, CodeGraphSummary, CodeSnippet, CodeSymbol, Dependency, ImpactAnalysis,
    Language, SymbolKind, Visibility,
};
pub use code_planner::{
    AgentRole as PlannerRole, CodePlanner, Effort, FileOperation, ImplementationPlan, PlanStep,
    PlannerConfig, RiskAssessment, TaskType, TestStrategy,
};
pub use compaction::{
    CompactedMessage, CompactionConfig, CompactionResult, CompactionStrategy,
    ContextCompactorEngine,
};
pub use config::{LlmProvider, ModelConfig};
pub use context::ContextWindow;
pub use context_assembly::{AssembledContext, ContextAssembler, GitContext};
pub use cost_optimizer::{
    CostModelOption, CostModelTier, CostOptimizer, CostOptimizerConfig, ModelUsageStats,
    OptimizationStrategy, RoutingDecision as CostRoutingDecision, SpendingSummary,
    TaskComplexity as CostTaskComplexity,
};
pub use critique::{Critique, CritiqueConfig, CritiqueDimension, CritiqueEngine, CritiqueResult};
pub use debug_recorder::{
    DebugRecorder, DebugStep, DebugTrace, StepType, TokenUsage, TraceSummary,
};
pub use diff_engine::{
    ApplyResult, DiffConfig, DiffEngine, DiffHunk, DiffLine, DiffOperation, DiffPlan, FileDiff,
};
pub use eval_framework::{
    lead_qualification_suite, support_quality_suite, ticket_routing_suite, CaseResult,
    CategoryResult, ComparisonReport, CompositeEvaluator, ContainsEvaluator, EvalCase,
    EvalFramework, EvalReport, EvalSuite, Evaluator, ExactMatchEvaluator, HeuristicEvaluator,
    JsonSchemaEvaluator, SimilarityEvaluator,
};
pub use evaluator::{
    EvaluationAction, EvaluationResult, EvaluatorConfig, QualityScore, ResponseEvaluator,
};
pub use failover::{FailoverBackend, RetryPolicy};
pub use guardrails::{
    redact_pii, ContentPolicy, GuardrailEngine, GuardrailProfile, GuardrailResult, GuardrailRule,
    PiiMatch, RuleSeverity, RuleType, Violation,
};
pub use hooks::{hook_fn, Hook, HookChain, HookDecision, HookEvent};
pub use identity::{AgentPersonality, ContextCompactor, SessionCommand, ThinkingLevel};
pub use learning::{
    LearnedPattern, LearningConfig, LearningEngine, LearningFeedback, LearningReport,
    ToolLearningStats, ToolRecommendation, Trend,
};
pub use llm::LlmClient;
pub use model_router::{
    ModelCost, ModelOption, ModelRouter, ModelTier, RoutingDecision, RoutingStrategy,
    TaskComplexity,
};
pub use multimodal::{ImageInput, MultimodalMessage, VisionBackend, VisionCapability};
pub use permission_mode::{CapturedCall, PermissionDecision, PermissionEvaluator, PermissionMode};
pub use prompt_manager::{
    outreach_composer_v1, register_xcapit_templates, sales_qualifier_v1, support_responder_v1,
    ticket_router_v1, ChainStep, PromptChain, PromptError, PromptManager, PromptTemplate,
    TemplateSummary, TemplateVariable, VarType,
};
pub use protocol::{
    decode_message, decode_outbound, encode_inbound, encode_message, InboundMessage,
    McpServerConfig as ProtocolMcpServerConfig, OutboundMessage, ProtocolHandler, SystemEvent,
};
pub use query::{
    ask_claude, ask_deepseek, ask_gemini, ask_groq, ask_mistral, ask_ollama, ask_openai,
    ask_openrouter, query, query_simple, query_simple_with_backend, query_with_backend,
    query_with_callback, query_with_callback_and_backend, QueryEvent, QueryOptions, ToolConfig,
};
pub use react::{ReActAction, ReActConfig, ReActEngine, ReActOutcome, ReActStep, ReActTrace};
pub use response_cache::{CacheKey, CacheMessage, CacheStats, ResponseCache};
pub use response_cache_layer::{CacheConfig, CacheLayer, CacheLayerStats};
pub use review_engine::{
    DimensionScore, FindingSeverity, ReviewConfig, ReviewDimension, ReviewEngine, ReviewFinding,
    ReviewReport, ReviewVerdict,
};
pub use reward::{
    AggregateMethod, ProcessRewardModel, ProcessRewardResult, RewardConfig, RewardFlag,
    StepCategory, StepReward, TrajectoryQuality,
};
pub use runner::{AgentRunner, ScaffoldMode};
pub use stream::StreamEvent;
pub use structured_output::{
    ExtractedOutput, ExtractionPattern, FieldDefinition, FieldType, OutputSchema,
    StructuredOutputParser, ValidationError,
};
pub use test_oracle::{
    ErrorType, FailureAnalysis, FixStrategy, TddCycle, TddPhase, TestCase, TestFramework,
    TestOracle, TestRunSummary, TestStatus,
};
pub use thinking::{
    ThinkingConfig, ThinkingDepth, ThinkingEngine, ThinkingResult, ThinkingStep, ThinkingStepType,
};
pub use token_counter::{TokenCounter, TokenEstimate, UsageTracker};
pub use tool_discovery::{
    DiscoveredTool, DiscoveryConfig, DiscoveryResult, DiscoveryStrategy, ToolDiscoveryCache,
    ToolDiscoveryEngine,
};
pub use tool_selector::{SelectionStrategy, ToolSelection, ToolSelector, ToolStats};
pub use vision_backends::{ClaudeVisionBackend, GeminiVisionBackend, OpenAiVisionBackend};
pub use voice::{
    AudioFormat, AudioInput, SttBackend, TranscriptSegment, TranscriptionRequest,
    TranscriptionResult, TtsBackend, VoiceConfig,
};
pub use voice_backends::{
    DeepgramSttBackend, ElevenLabsTtsBackend, OpenAiTtsBackend, OpenAiWhisperBackend,
};
pub use webhooks::{WebhookConfig, WebhookEvent, WebhookManager, WebhookPayload};

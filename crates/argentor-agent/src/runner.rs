use crate::circuit_breaker::{CircuitBreakerRegistry, CircuitConfig};
use crate::config::ModelConfig;
use crate::context::ContextWindow;
use crate::debug_recorder::{DebugRecorder, StepType};
use crate::guardrails::{GuardrailEngine, GuardrailResult, RuleSeverity};
use crate::hooks::{HookChain, HookDecision, HookEvent};
use crate::identity::AgentPersonality;
use crate::llm::{LlmClient, LlmResponse};
use crate::permission_mode::{PermissionDecision, PermissionEvaluator};
use crate::response_cache::{CacheKey, CacheMessage, ResponseCache};
use crate::stream::StreamEvent;
use argentor_core::{ArgentorError, ArgentorResult, Message, Role};
use argentor_security::audit::AuditOutcome;
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::SkillRegistry;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Default system prompt used when none is provided.
const DEFAULT_SYSTEM_PROMPT: &str =
    "You are Argentor, a secure AI assistant. You have access to tools (skills) \
     that you can use to help the user. Each tool runs in a sandboxed environment \
     with specific permissions. Always explain what you're doing before using a tool.";

/// Optional MCP proxy for centralized tool call logging and metrics.
type OptionalProxy = Option<(Arc<argentor_mcp::McpProxy>, String)>;

/// The Agent Runner: orchestrates the agentic loop.
/// Prompt -> LLM -> ToolCall -> Execute Skill -> Backfill -> Repeat.
///
/// # Sharing across threads
///
/// `AgentRunner` is designed to be shared via `Arc<AgentRunner>`:
///
/// ```rust,no_run
/// use argentor_agent::AgentRunner;
/// use std::sync::Arc;
///
/// # async fn example(runner: AgentRunner) {
/// let runner = Arc::new(runner);
/// let r1 = runner.clone();
/// let r2 = runner.clone();
/// # }
/// ```
///
/// All mutation after construction goes through interior mutability
/// (`LearningEngine`, `ResponseCache`, `SkillRegistry` all use `Arc<RwLock<…>>`),
/// so you never need `&mut AgentRunner` once it is constructed.
///
/// # Examples
///
/// ```no_run
/// use argentor_agent::{AgentRunner, ModelConfig, LlmProvider};
/// use argentor_security::{AuditLog, PermissionSet};
/// use argentor_skills::SkillRegistry;
/// use argentor_builtins::register_builtins;
/// use std::sync::Arc;
/// use std::path::PathBuf;
///
/// let registry = SkillRegistry::new();
/// register_builtins(&registry);
/// let skills = Arc::new(registry);
/// let permissions = PermissionSet::new();
/// let audit = Arc::new(AuditLog::new(PathBuf::from("/tmp/audit")));
/// let config = ModelConfig {
///     provider: LlmProvider::Claude,
///     model_id: "claude-sonnet-4-20250514".into(),
///     api_key: "your-key".into(),
///     api_base_url: None,
///     temperature: 0.7,
///     max_tokens: 4096,
///     max_turns: 10,
///     max_context_tokens: 200_000,
///     fallback_models: vec![],
///     retry_policy: None,
/// };
///
/// let agent = AgentRunner::new(config, skills, permissions, audit)
///     .with_default_guardrails();
/// ```
pub struct AgentRunner {
    llm: LlmClient,
    skills: Arc<SkillRegistry>,
    permissions: PermissionSet,
    audit: Arc<AuditLog>,
    max_turns: u32,
    system_prompt: String,
    /// Optional (proxy, agent_id) — when set, tool calls route through MCP proxy.
    proxy: OptionalProxy,
    /// Optional LLM response cache for deduplication.
    cache: Option<ResponseCache>,
    /// Circuit breaker registry for provider resilience.
    circuit_breakers: CircuitBreakerRegistry,
    /// Debug recorder for step-by-step trace capture.
    debug_recorder: DebugRecorder,
    /// Optional guardrail engine for input/output validation and sanitization.
    guardrails: Option<GuardrailEngine>,
    /// Optional hook chain for intercepting tool calls and agent events.
    hooks: Option<HookChain>,
    /// Optional permission evaluator for global tool authorization modes.
    permission_evaluator: Option<PermissionEvaluator>,
    /// Optional extended thinking engine for deeper reasoning before acting.
    thinking: Option<crate::thinking::ThinkingEngine>,
    /// Optional self-critique engine for reviewing and revising responses.
    critique: Option<crate::critique::CritiqueEngine>,
    /// Optional context compaction engine for automatic conversation summarization.
    compaction: Option<crate::compaction::ContextCompactorEngine>,
    /// Optional dynamic tool discovery engine for semantic tool selection.
    tool_discovery: Option<crate::tool_discovery::ToolDiscoveryEngine>,
    /// Optional checkpoint manager for save/restore of agent state.
    checkpoint_manager: Option<crate::checkpoint::CheckpointManager>,
    /// Optional learning engine for improving tool selection over time.
    learning: Option<crate::learning::LearningEngine>,
}

impl AgentRunner {
    /// Create a new agent runner with the given model config, skills, permissions, and audit log.
    pub fn new(
        config: ModelConfig,
        skills: Arc<SkillRegistry>,
        permissions: PermissionSet,
        audit: Arc<AuditLog>,
    ) -> Self {
        let max_turns = config.max_turns;
        Self {
            llm: LlmClient::new(config),
            skills,
            permissions,
            audit,
            max_turns,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            proxy: None,
            cache: None,
            circuit_breakers: CircuitBreakerRegistry::new(CircuitConfig::default()),
            debug_recorder: DebugRecorder::disabled(),
            guardrails: None,
            hooks: None,
            permission_evaluator: None,
            thinking: None,
            critique: None,
            compaction: None,
            tool_discovery: None,
            checkpoint_manager: None,
            learning: None,
        }
    }

    /// Create from a custom LLM backend (for testing or custom providers).
    ///
    /// Ownership: `skills` and `audit` are passed as `Arc` so multiple runners
    /// (or concurrent tasks) can share the same registry and audit log without
    /// cloning the underlying data.  `permissions` is cloned per-runner; mutating
    /// a runner's permission set does not affect any other runner.
    pub fn from_backend(
        backend: Box<dyn crate::backends::LlmBackend>,
        skills: Arc<SkillRegistry>,
        permissions: PermissionSet,
        audit: Arc<AuditLog>,
        max_turns: u32,
    ) -> Self {
        Self {
            llm: LlmClient::from_backend(backend),
            skills,
            permissions,
            audit,
            max_turns,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            proxy: None,
            cache: None,
            circuit_breakers: CircuitBreakerRegistry::new(CircuitConfig::default()),
            debug_recorder: DebugRecorder::disabled(),
            guardrails: None,
            hooks: None,
            permission_evaluator: None,
            thinking: None,
            critique: None,
            compaction: None,
            tool_discovery: None,
            checkpoint_manager: None,
            learning: None,
        }
    }

    /// Create with a custom system prompt (used by orchestrator for specialized workers).
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Configure the agent with a personality (generates system prompt from it).
    pub fn with_personality(mut self, personality: &AgentPersonality) -> Self {
        self.system_prompt = personality.to_system_prompt();
        self
    }

    /// Route tool calls through the MCP proxy for centralized logging and metrics.
    pub fn with_proxy(
        mut self,
        proxy: Arc<argentor_mcp::McpProxy>,
        agent_id: impl Into<String>,
    ) -> Self {
        self.proxy = Some((proxy, agent_id.into()));
        self
    }

    /// Enable LLM response caching with the given capacity and TTL.
    pub fn with_cache(mut self, capacity: usize, ttl: Duration) -> Self {
        self.cache = Some(ResponseCache::new(capacity, ttl));
        self
    }

    /// Set a custom circuit breaker configuration for LLM providers.
    pub fn with_circuit_breaker(mut self, config: CircuitConfig) -> Self {
        self.circuit_breakers = CircuitBreakerRegistry::new(config);
        self
    }

    /// Enable debug recording for this agent run.
    pub fn with_debug_recorder(mut self, trace_id: impl Into<String>) -> Self {
        self.debug_recorder = DebugRecorder::new(trace_id);
        self
    }

    /// Get the debug recorder (for finalizing traces after the run).
    pub fn debug_recorder(&self) -> &DebugRecorder {
        &self.debug_recorder
    }

    /// Get cache statistics (if caching is enabled).
    pub fn cache_stats(&self) -> Option<crate::response_cache::CacheStats> {
        self.cache
            .as_ref()
            .map(super::response_cache::ResponseCache::stats)
    }

    /// Get the circuit breaker registry.
    pub fn circuit_breakers(&self) -> &CircuitBreakerRegistry {
        &self.circuit_breakers
    }

    /// Enable guardrails with the provided engine. Guardrails validate input before
    /// LLM calls, output after LLM responses, and tool call arguments/results.
    pub fn with_guardrails(mut self, engine: GuardrailEngine) -> Self {
        self.guardrails = Some(engine);
        self
    }

    /// Enable guardrails with default rules (PII, prompt injection, toxicity, max length).
    pub fn with_default_guardrails(mut self) -> Self {
        self.guardrails = Some(GuardrailEngine::new());
        self
    }

    /// Get a reference to the guardrail engine (if enabled).
    pub fn guardrails(&self) -> Option<&GuardrailEngine> {
        self.guardrails.as_ref()
    }

    /// Attach a hook chain for intercepting tool calls and agent events.
    ///
    /// Hooks are evaluated before and after each tool call (and optionally
    /// around LLM calls). A `Deny` decision prevents the tool from executing;
    /// a `Modify` decision replaces the arguments.
    pub fn with_hooks(mut self, hooks: HookChain) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Get a reference to the hook chain (if configured).
    pub fn hooks(&self) -> Option<&HookChain> {
        self.hooks.as_ref()
    }

    /// Set a permission evaluator for global tool authorization control.
    ///
    /// When set, every tool call is checked against the evaluator *before*
    /// being dispatched to the skill registry or MCP proxy.
    pub fn with_permission_mode(mut self, evaluator: PermissionEvaluator) -> Self {
        self.permission_evaluator = Some(evaluator);
        self
    }

    /// Get a reference to the permission evaluator (if configured).
    pub fn permission_evaluator(&self) -> Option<&PermissionEvaluator> {
        self.permission_evaluator.as_ref()
    }

    // ----- Intelligence modules (Phase E1-E3) -----

    /// Enable extended thinking for deeper reasoning before acting.
    pub fn with_thinking(mut self, config: crate::thinking::ThinkingConfig) -> Self {
        self.thinking = Some(crate::thinking::ThinkingEngine::new(config));
        self
    }

    /// Enable extended thinking with default settings (Standard depth).
    pub fn with_default_thinking(mut self) -> Self {
        self.thinking = Some(crate::thinking::ThinkingEngine::with_defaults());
        self
    }

    /// Get a reference to the thinking engine (if enabled).
    pub fn thinking(&self) -> Option<&crate::thinking::ThinkingEngine> {
        self.thinking.as_ref()
    }

    /// Enable self-critique loop for reviewing and revising responses.
    pub fn with_critique(mut self, config: crate::critique::CritiqueConfig) -> Self {
        self.critique = Some(crate::critique::CritiqueEngine::new(config));
        self
    }

    /// Enable self-critique with default settings.
    pub fn with_default_critique(mut self) -> Self {
        self.critique = Some(crate::critique::CritiqueEngine::with_defaults());
        self
    }

    /// Get a reference to the critique engine (if enabled).
    pub fn critique(&self) -> Option<&crate::critique::CritiqueEngine> {
        self.critique.as_ref()
    }

    /// Enable automatic context compaction when conversation approaches token limits.
    pub fn with_compaction(mut self, config: crate::compaction::CompactionConfig) -> Self {
        self.compaction = Some(crate::compaction::ContextCompactorEngine::new(config));
        self
    }

    /// Enable context compaction with default settings (30K token trigger, Hybrid strategy).
    pub fn with_default_compaction(mut self) -> Self {
        self.compaction = Some(crate::compaction::ContextCompactorEngine::with_defaults());
        self
    }

    /// Get a reference to the compaction engine (if enabled).
    pub fn compaction(&self) -> Option<&crate::compaction::ContextCompactorEngine> {
        self.compaction.as_ref()
    }

    /// Enable dynamic tool discovery for semantic tool selection.
    pub fn with_tool_discovery(mut self, config: crate::tool_discovery::DiscoveryConfig) -> Self {
        self.tool_discovery = Some(crate::tool_discovery::ToolDiscoveryEngine::new(config));
        self
    }

    /// Enable tool discovery with default settings (max 8 tools, Hybrid strategy).
    pub fn with_default_tool_discovery(mut self) -> Self {
        self.tool_discovery = Some(crate::tool_discovery::ToolDiscoveryEngine::with_defaults());
        self
    }

    /// Get a reference to the tool discovery engine (if enabled).
    pub fn tool_discovery(&self) -> Option<&crate::tool_discovery::ToolDiscoveryEngine> {
        self.tool_discovery.as_ref()
    }

    /// Enable state checkpointing for save/restore of agent state.
    pub fn with_checkpoint(mut self, config: crate::checkpoint::CheckpointConfig) -> Self {
        self.checkpoint_manager = Some(crate::checkpoint::CheckpointManager::new(config));
        self
    }

    /// Enable state checkpointing with default settings.
    pub fn with_default_checkpoint(mut self) -> Self {
        self.checkpoint_manager = Some(crate::checkpoint::CheckpointManager::with_defaults());
        self
    }

    /// Get a reference to the checkpoint manager (if enabled).
    pub fn checkpoint_manager(&self) -> Option<&crate::checkpoint::CheckpointManager> {
        self.checkpoint_manager.as_ref()
    }

    /// Get a mutable reference to the checkpoint manager (for creating/restoring checkpoints).
    pub fn checkpoint_manager_mut(&mut self) -> Option<&mut crate::checkpoint::CheckpointManager> {
        self.checkpoint_manager.as_mut()
    }

    /// Enable learning feedback loop for improving tool selection over time.
    pub fn with_learning(mut self, config: crate::learning::LearningConfig) -> Self {
        self.learning = Some(crate::learning::LearningEngine::new(config));
        self
    }

    /// Enable learning with default settings.
    pub fn with_default_learning(mut self) -> Self {
        self.learning = Some(crate::learning::LearningEngine::with_defaults());
        self
    }

    /// Get a reference to the learning engine (if enabled).
    ///
    /// `LearningEngine` uses interior mutability (`Arc<RwLock<…>>`), so all
    /// mutation methods (`record_feedback`, `learn_patterns`, `deserialize`) take
    /// `&self` — no `&mut AgentRunner` is required.
    pub fn learning(&self) -> Option<&crate::learning::LearningEngine> {
        self.learning.as_ref()
    }

    /// Enable all intelligence features with default configurations.
    /// Equivalent to chaining `with_default_thinking()`, `with_default_critique()`,
    /// `with_default_compaction()`, `with_default_tool_discovery()`,
    /// `with_default_checkpoint()`, and `with_default_learning()`.
    pub fn with_intelligence(self) -> Self {
        self.with_default_thinking()
            .with_default_critique()
            .with_default_compaction()
            .with_default_tool_discovery()
            .with_default_checkpoint()
            .with_default_learning()
    }

    /// Run the agentic loop for a session. Returns the final assistant response.
    #[tracing::instrument(
        skip(self, session, user_input),
        fields(session_id = %session.id, max_turns = self.max_turns)
    )]
    pub async fn run(&self, session: &mut Session, user_input: &str) -> ArgentorResult<String> {
        let session_id = session.id;

        self.debug_recorder
            .record(StepType::Input, user_input, None);

        // Add user message
        let user_msg = Message::user(user_input, session_id);
        session.add_message(user_msg);

        let mut context = ContextWindow::new(100);
        context.set_system_prompt(&self.system_prompt);

        for msg in &session.messages {
            context.push(msg.clone());
        }

        // --- Intelligence: Dynamic Tool Discovery ---
        // If tool discovery is enabled, select only relevant tools instead of all.
        let tool_descriptors: Vec<_> = if let Some(discovery) = &self.tool_discovery {
            let all_tools: Vec<_> = self
                .skills
                .list_descriptors()
                .into_iter()
                .map(|d| crate::tool_discovery::ToolEntry::new(&d.name, &d.description))
                .collect();
            if let Some(result) = discovery.discover(user_input, &all_tools) {
                self.debug_recorder.record(
                    StepType::Custom("tool_discovery".into()),
                    format!(
                        "Discovered {}/{} tools (~{} tokens saved)",
                        result.selected_tools.len(),
                        result.total_available,
                        result.token_savings,
                    ),
                    None,
                );
                let selected_names: std::collections::HashSet<_> = result
                    .selected_tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect();
                self.skills
                    .list_descriptors()
                    .into_iter()
                    .filter(|d| selected_names.contains(d.name.as_str()))
                    .collect()
            } else {
                self.skills.list_descriptors()
            }
        } else {
            self.skills.list_descriptors()
        };

        // --- Intelligence: Extended Thinking ---
        // If thinking is enabled, perform a pre-reasoning pass before entering the loop.
        if let Some(thinking) = &self.thinking {
            let tool_names: Vec<&str> = tool_descriptors.iter().map(|d| d.name.as_str()).collect();
            if let Some(think_result) = thinking.think(user_input, &tool_names) {
                self.debug_recorder.record(
                    StepType::Thinking,
                    format!(
                        "Extended thinking (confidence={:.2}, subtasks={}, recommended_tools=[{}])",
                        think_result.confidence,
                        think_result.decomposed_subtasks.len(),
                        think_result.recommended_tools.join(", "),
                    ),
                    None,
                );
                if let Some(plan) = &think_result.plan {
                    let plan_msg =
                        Message::new(Role::System, format!("[Agent Plan] {plan}"), session_id);
                    context.push(plan_msg);
                }
            }
        }

        info!(session_id = %session_id, "Starting agentic loop");

        for turn in 0..self.max_turns {
            info!(turn = turn, "Agentic loop turn");

            // --- Intelligence: Context Compaction ---
            // If compaction is enabled, check whether context needs summarization.
            if let Some(compactor) = &self.compaction {
                let messages: Vec<_> = context
                    .messages()
                    .iter()
                    .map(|m| {
                        crate::compaction::CompactableMessage::new(
                            &format!("{:?}", m.role),
                            &m.content,
                            m.content.contains("tool_result") || m.content.contains("tool_use"),
                        )
                    })
                    .collect();
                if compactor.should_compact(&messages) {
                    if let Some(result) = compactor.compact(&messages) {
                        self.debug_recorder.record(
                            StepType::Custom("compaction".into()),
                            format!(
                                "Compacted {} → {} messages ({:.0}% reduction)",
                                result.original_message_count,
                                result.compacted_message_count,
                                (1.0 - result.compression_ratio) * 100.0,
                            ),
                            None,
                        );
                        context = ContextWindow::new(100);
                        context.set_system_prompt(&self.system_prompt);
                        for cm in &result.preserved_messages {
                            let role = match cm.role.as_str() {
                                "System" => Role::System,
                                "Assistant" => Role::Assistant,
                                _ => Role::User,
                            };
                            context.push(Message::new(role, &cm.content, session_id));
                        }
                    }
                }
            }

            // Check circuit breaker before LLM call
            let provider_name = self.llm.provider_name();
            if !self.circuit_breakers.allow_request(provider_name) {
                self.debug_recorder.record(
                    StepType::Error,
                    format!("Circuit breaker open for provider: {provider_name}"),
                    None,
                );
                return Err(ArgentorError::Agent(format!(
                    "Circuit breaker open for provider: {provider_name}"
                )));
            }

            // Check response cache before making LLM call
            let cache_messages: Vec<CacheMessage> = context
                .messages()
                .iter()
                .map(|m| CacheMessage::new(format!("{:?}", m.role), &m.content))
                .collect();
            let cache_key = CacheKey::compute(provider_name, &cache_messages);

            if let Some(cached) = self.cache.as_ref().and_then(|c| c.get(&cache_key)) {
                self.debug_recorder.record(
                    StepType::CacheHit,
                    "LLM response served from cache",
                    None,
                );
                self.circuit_breakers.record_success(provider_name);

                let assistant_msg = Message::assistant(&cached, session_id);
                session.add_message(assistant_msg.clone());
                context.push(assistant_msg);

                info!(session_id = %session_id, turn = turn, "Cache hit — returning cached response");
                return Ok(cached);
            }

            // --- Pre-LLM Input Guardrails ---
            if let Some(engine) = &self.guardrails {
                let latest_input = context
                    .messages()
                    .last()
                    .map(|m| m.content.as_str())
                    .unwrap_or("");
                let gr = engine.check_input(latest_input);
                self.log_guardrail_result(session_id, "input", &gr);
                if !gr.passed {
                    return Err(ArgentorError::Agent(format!(
                        "Input blocked by guardrails: {}",
                        gr.violations
                            .iter()
                            .filter(|v| v.severity == RuleSeverity::Block)
                            .map(|v| v.message.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    )));
                }
            }

            // --- Pre-LLM Hook ---
            if let Some(hooks) = &self.hooks {
                let pre_llm = HookEvent::PreLlmCall {
                    provider: provider_name.to_string(),
                    message_count: context.messages().len(),
                    turn,
                };
                let _ = hooks.evaluate(&pre_llm);
            }

            self.debug_recorder.record(
                StepType::LlmCall,
                format!("Turn {turn}: calling {provider_name}"),
                None,
            );
            let llm_start = std::time::Instant::now();

            let llm_span = tracing::info_span!(
                "llm_call",
                provider = %provider_name,
                turn = turn,
                session_id = %session_id,
            );
            let response = {
                let _guard = llm_span.enter();
                self.llm
                    .chat(
                        context.system_prompt(),
                        context.messages(),
                        &tool_descriptors,
                    )
                    .await
            };

            let llm_duration = llm_start.elapsed().as_millis() as u64;

            // Handle LLM errors with circuit breaker
            let response = match response {
                Ok(r) => {
                    self.circuit_breakers.record_success(provider_name);
                    self.debug_recorder.record_with_metrics(
                        StepType::LlmResponse,
                        format!("Turn {turn}: response received"),
                        llm_duration,
                        0,
                        0,
                    );
                    r
                }
                Err(e) => {
                    self.circuit_breakers.record_failure(provider_name);
                    self.debug_recorder.record(
                        StepType::Error,
                        format!("LLM call failed: {e}"),
                        None,
                    );
                    return Err(e);
                }
            };

            // --- Post-LLM Hook ---
            if let Some(hooks) = &self.hooks {
                let response_type = match &response {
                    LlmResponse::Done(_) => "done",
                    LlmResponse::Text(_) => "text",
                    LlmResponse::ToolUse { .. } => "tool_use",
                };
                let post_llm = HookEvent::PostLlmCall {
                    provider: provider_name.to_string(),
                    response_type: response_type.to_string(),
                    duration_ms: llm_duration,
                    turn,
                };
                let _ = hooks.evaluate(&post_llm);
            }

            match response {
                LlmResponse::Done(text) => {
                    // --- Post-LLM Output Guardrails ---
                    let text = self.apply_output_guardrails(session_id, text)?;

                    // Cache the final response
                    if let Some(cache) = &self.cache {
                        let estimate = (text.len() / 4) as u64;
                        cache.put(cache_key, &text, provider_name, estimate);
                    }

                    self.debug_recorder.record(StepType::Output, &text, None);

                    let assistant_msg = Message::assistant(&text, session_id);
                    session.add_message(assistant_msg.clone());
                    context.push(assistant_msg);

                    self.audit.log_action(
                        session_id,
                        "agent_response",
                        None,
                        serde_json::json!({"turn": turn, "type": "final"}),
                        AuditOutcome::Success,
                    );

                    // --- Intelligence: Self-Critique ---
                    // If critique is enabled, review the response before returning.
                    if let Some(critique) = &self.critique {
                        let empty_tools: Vec<&str> = Vec::new();
                        if let Some(cr) = critique.critique(user_input, &text, &empty_tools) {
                            self.debug_recorder.record(
                                StepType::Custom("critique".into()),
                                format!(
                                    "Self-critique: score={:.2}, accepted={}, revisions={}",
                                    cr.final_score, cr.accepted, cr.revision_count,
                                ),
                                None,
                            );
                            if let Some(revised) = &cr.revised_response {
                                info!(
                                    session_id = %session_id,
                                    original_score = cr.final_score,
                                    "Using revised response from self-critique"
                                );
                                return Ok(revised.clone());
                            }
                        }
                    }

                    info!(session_id = %session_id, turns = turn + 1, "Agentic loop completed");
                    return Ok(text);
                }

                LlmResponse::Text(text) => {
                    self.debug_recorder.record(StepType::Thinking, &text, None);
                    let assistant_msg = Message::assistant(&text, session_id);
                    session.add_message(assistant_msg.clone());
                    context.push(assistant_msg);
                }

                LlmResponse::ToolUse {
                    content,
                    tool_calls,
                } => {
                    // Add any text content from the assistant
                    if let Some(text) = &content {
                        self.debug_recorder.record(StepType::Thinking, text, None);
                        let msg = Message::assistant(text, session_id);
                        session.add_message(msg.clone());
                        context.push(msg);
                    }

                    // Execute each tool call
                    for call in tool_calls {
                        self.debug_recorder.record(
                            StepType::ToolCall,
                            format!("Calling tool: {}", call.name),
                            Some(serde_json::json!({"call_id": &call.id, "arguments": &call.arguments})),
                        );

                        info!(
                            session_id = %session_id,
                            tool = %call.name,
                            call_id = %call.id,
                            "Executing tool call"
                        );

                        // --- Pre-Tool Hook Evaluation ---
                        let mut effective_call = call.clone();
                        if let Some(hooks) = &self.hooks {
                            let pre_event = HookEvent::PreToolUse {
                                tool_name: call.name.clone(),
                                arguments: call.arguments.clone(),
                                call_id: call.id.clone(),
                            };
                            match hooks.evaluate(&pre_event) {
                                HookDecision::Deny { reason } => {
                                    warn!(tool = %call.name, reason = %reason, "Hook denied tool call");
                                    self.debug_recorder.record(
                                        StepType::Custom("hook_deny".into()),
                                        format!("Hook denied tool '{}': {}", call.name, reason),
                                        None,
                                    );
                                    let error_msg = Message::new(
                                        Role::User,
                                        format!("Tool '{}' denied by hook: {}", call.name, reason),
                                        session_id,
                                    );
                                    session.add_message(error_msg.clone());
                                    context.push(error_msg);
                                    continue;
                                }
                                HookDecision::Modify { new_arguments } => {
                                    info!(tool = %call.name, "Hook modified tool arguments");
                                    self.debug_recorder.record(
                                        StepType::Custom("hook_modify".into()),
                                        format!("Hook modified arguments for tool '{}'", call.name),
                                        None,
                                    );
                                    effective_call.arguments = new_arguments;
                                }
                                HookDecision::Allow | HookDecision::Continue => {}
                            }
                        }

                        self.audit.log_action(
                            session_id,
                            "tool_call",
                            Some(effective_call.name.clone()),
                            serde_json::json!({
                                "call_id": effective_call.id,
                                "arguments": effective_call.arguments,
                            }),
                            AuditOutcome::Success,
                        );

                        let tool_start = std::time::Instant::now();
                        let result = self.execute_tool(effective_call.clone()).await;
                        let tool_duration = tool_start.elapsed().as_millis() as u64;

                        match result {
                            Ok(tool_result) => {
                                self.debug_recorder.record(
                                    StepType::ToolResult,
                                    format!(
                                        "Tool {} result (error={})",
                                        call.name, tool_result.is_error
                                    ),
                                    None,
                                );

                                // --- Post-Tool Hook Evaluation (informational) ---
                                if let Some(hooks) = &self.hooks {
                                    let post_event = HookEvent::PostToolUse {
                                        tool_name: call.name.clone(),
                                        result: tool_result.content.clone(),
                                        is_error: tool_result.is_error,
                                        call_id: tool_result.call_id.clone(),
                                        duration_ms: tool_duration,
                                    };
                                    let _ = hooks.evaluate(&post_event);
                                }

                                let outcome = if tool_result.is_error {
                                    AuditOutcome::Error
                                } else {
                                    AuditOutcome::Success
                                };

                                self.audit.log_action(
                                    session_id,
                                    "tool_result",
                                    Some(call.name.clone()),
                                    serde_json::json!({
                                        "call_id": tool_result.call_id,
                                        "is_error": tool_result.is_error,
                                    }),
                                    outcome,
                                );

                                // --- Post-Tool Result Guardrails ---
                                // Sanitize tool output to prevent data leakage (PII, secrets)
                                let sanitized_content =
                                    self.sanitize_tool_result(session_id, &tool_result.content);

                                // Backfill tool result as a user message (tool role)
                                let result_content = serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_result.call_id,
                                    "content": sanitized_content,
                                    "is_error": tool_result.is_error,
                                });
                                let tool_msg = Message::new(
                                    Role::User,
                                    result_content.to_string(),
                                    session_id,
                                );
                                session.add_message(tool_msg.clone());
                                context.push(tool_msg);

                                // --- Intelligence: Learning Feedback ---
                                // Note: feedback is logged for post-run batch application
                                // via `learning().record_feedback()` since LearningEngine uses interior mutability.
                                if self.learning.is_some() {
                                    self.debug_recorder.record(
                                        StepType::Custom("learning".into()),
                                        format!(
                                            "Tool '{}': success={}, time={}ms",
                                            call.name, !tool_result.is_error, tool_duration,
                                        ),
                                        None,
                                    );
                                }
                            }
                            Err(e) => {
                                error!(error = %e, tool = %call.name, "Tool execution failed");
                                self.audit.log_action(
                                    session_id,
                                    "tool_error",
                                    Some(call.name.clone()),
                                    serde_json::json!({"error": e.to_string()}),
                                    AuditOutcome::Error,
                                );

                                let error_msg = Message::new(
                                    Role::User,
                                    format!("Tool error: {e}"),
                                    session_id,
                                );
                                session.add_message(error_msg.clone());
                                context.push(error_msg);
                            }
                        }
                    }
                }
            }
        }

        warn!(
            session_id = %session_id,
            max_turns = self.max_turns,
            "Agentic loop reached max turns"
        );

        Err(ArgentorError::Agent(format!(
            "Agentic loop exceeded maximum of {} turns",
            self.max_turns
        )))
    }

    /// Execute a tool call — checks permission mode first, then routes through
    /// MCP proxy if configured, otherwise falls back to the skill registry.
    #[tracing::instrument(
        skip(self, call),
        fields(tool_name = %call.name, call_id = %call.id)
    )]
    async fn execute_tool(
        &self,
        call: argentor_core::ToolCall,
    ) -> ArgentorResult<argentor_core::ToolResult> {
        // --- Permission mode check ---
        if let Some(evaluator) = &self.permission_evaluator {
            match evaluator.check(&call.name, &call.arguments) {
                PermissionDecision::Allow => {
                    // Fall through to normal execution
                }
                PermissionDecision::Deny { reason } => {
                    warn!(tool = %call.name, reason = %reason, "Tool denied by permission mode");
                    return Ok(argentor_core::ToolResult::error(
                        &call.id,
                        format!("Permission denied: {reason}"),
                    ));
                }
                PermissionDecision::Captured {
                    tool_name,
                    arguments,
                } => {
                    info!(tool = %tool_name, "Tool call captured (plan-only mode)");
                    return Ok(argentor_core::ToolResult::success(
                        &call.id,
                        format!(
                            "Captured (plan mode): tool={tool_name}, args={}",
                            serde_json::to_string(&arguments).unwrap_or_default()
                        ),
                    ));
                }
                PermissionDecision::RequiresApproval {
                    tool_name,
                    description,
                } => {
                    warn!(tool = %tool_name, description = %description, "Tool requires approval");
                    return Ok(argentor_core::ToolResult::error(
                        &call.id,
                        format!("Requires approval: {description}"),
                    ));
                }
            }
        }

        if let Some((proxy, agent_id)) = &self.proxy {
            proxy.execute(call, agent_id).await
        } else {
            self.skills.execute(call, &self.permissions).await
        }
    }

    /// Run the agentic loop with streaming.
    ///
    /// Works like `run()` but uses `chat_stream()` to send partial LLM output to
    /// the caller in real time via the provided `event_tx` channel.  Text responses
    /// are streamed token-by-token; tool calls are accumulated and then executed
    /// (non-streaming) before the next turn.
    #[tracing::instrument(
        skip(self, session, user_input, event_tx),
        fields(session_id = %session.id, max_turns = self.max_turns, streaming = true)
    )]
    pub async fn run_streaming(
        &self,
        session: &mut Session,
        user_input: &str,
        event_tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> ArgentorResult<String> {
        let session_id = session.id;

        // Add user message
        let user_msg = Message::user(user_input, session_id);
        session.add_message(user_msg);

        let mut context = ContextWindow::new(100);
        context.set_system_prompt(&self.system_prompt);

        for msg in &session.messages {
            context.push(msg.clone());
        }

        let tool_descriptors = self.skills.list_descriptors();

        info!(session_id = %session_id, "Starting streaming agentic loop");

        for turn in 0..self.max_turns {
            info!(turn = turn, "Streaming agentic loop turn");

            // Start streaming from the LLM
            let (mut stream_rx, join_handle) = self
                .llm
                .chat_stream(
                    context.system_prompt(),
                    context.messages(),
                    &tool_descriptors,
                )
                .await?;

            // Forward all stream events to the caller
            while let Some(event) = stream_rx.recv().await {
                // Forward the event; if the receiver is gone, we keep going
                // so we can still collect the aggregated response.
                let _ = event_tx.send(event);
            }

            // Wait for the aggregated response
            let response = join_handle
                .await
                .map_err(|e| ArgentorError::Agent(format!("Stream task panicked: {e}")))??;

            match response {
                LlmResponse::Done(text) => {
                    // --- Post-LLM Output Guardrails (streaming) ---
                    let text = self.apply_output_guardrails(session_id, text)?;

                    let assistant_msg = Message::assistant(&text, session_id);
                    session.add_message(assistant_msg.clone());
                    context.push(assistant_msg);

                    self.audit.log_action(
                        session_id,
                        "agent_response",
                        None,
                        serde_json::json!({"turn": turn, "type": "final"}),
                        AuditOutcome::Success,
                    );

                    info!(
                        session_id = %session_id,
                        turns = turn + 1,
                        "Streaming agentic loop completed"
                    );
                    return Ok(text);
                }

                LlmResponse::Text(text) => {
                    let assistant_msg = Message::assistant(&text, session_id);
                    session.add_message(assistant_msg.clone());
                    context.push(assistant_msg);
                }

                LlmResponse::ToolUse {
                    content,
                    tool_calls,
                } => {
                    // Add any text content from the assistant
                    if let Some(text) = &content {
                        let msg = Message::assistant(text, session_id);
                        session.add_message(msg.clone());
                        context.push(msg);
                    }

                    // Execute each tool call (non-streaming)
                    for call in tool_calls {
                        info!(
                            session_id = %session_id,
                            tool = %call.name,
                            call_id = %call.id,
                            "Executing tool call (streaming mode)"
                        );

                        self.audit.log_action(
                            session_id,
                            "tool_call",
                            Some(call.name.clone()),
                            serde_json::json!({
                                "call_id": call.id,
                                "arguments": call.arguments,
                            }),
                            AuditOutcome::Success,
                        );

                        let result = self.execute_tool(call.clone()).await;

                        match result {
                            Ok(tool_result) => {
                                let outcome = if tool_result.is_error {
                                    AuditOutcome::Error
                                } else {
                                    AuditOutcome::Success
                                };

                                self.audit.log_action(
                                    session_id,
                                    "tool_result",
                                    Some(call.name.clone()),
                                    serde_json::json!({
                                        "call_id": tool_result.call_id,
                                        "is_error": tool_result.is_error,
                                    }),
                                    outcome,
                                );

                                // --- Post-Tool Result Guardrails (streaming) ---
                                let sanitized_content =
                                    self.sanitize_tool_result(session_id, &tool_result.content);

                                let result_content = serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_result.call_id,
                                    "content": sanitized_content,
                                    "is_error": tool_result.is_error,
                                });
                                let tool_msg = Message::new(
                                    Role::User,
                                    result_content.to_string(),
                                    session_id,
                                );
                                session.add_message(tool_msg.clone());
                                context.push(tool_msg);
                            }
                            Err(e) => {
                                error!(error = %e, tool = %call.name, "Tool execution failed (streaming)");
                                self.audit.log_action(
                                    session_id,
                                    "tool_error",
                                    Some(call.name.clone()),
                                    serde_json::json!({"error": e.to_string()}),
                                    AuditOutcome::Error,
                                );

                                let error_msg = Message::new(
                                    Role::User,
                                    format!("Tool error: {e}"),
                                    session_id,
                                );
                                session.add_message(error_msg.clone());
                                context.push(error_msg);
                            }
                        }
                    }
                }
            }
        }

        warn!(
            session_id = %session_id,
            max_turns = self.max_turns,
            "Streaming agentic loop reached max turns"
        );

        let _ = event_tx.send(StreamEvent::Error {
            message: format!("Agentic loop exceeded maximum of {} turns", self.max_turns),
        });

        Err(ArgentorError::Agent(format!(
            "Agentic loop exceeded maximum of {} turns",
            self.max_turns
        )))
    }

    // -- Guardrail helpers ---------------------------------------------------

    /// Apply output guardrails to LLM response text. Returns sanitized text or error.
    fn apply_output_guardrails(
        &self,
        session_id: uuid::Uuid,
        text: String,
    ) -> ArgentorResult<String> {
        let Some(engine) = &self.guardrails else {
            return Ok(text);
        };

        let gr = engine.check_output(&text, None);
        self.log_guardrail_result(session_id, "output", &gr);

        if !gr.passed {
            return Err(ArgentorError::Agent(format!(
                "Output blocked by guardrails: {}",
                gr.violations
                    .iter()
                    .filter(|v| v.severity == RuleSeverity::Block)
                    .map(|v| v.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }

        // Use sanitized text if available (e.g. PII redacted)
        Ok(gr.sanitized_text.unwrap_or(text))
    }

    /// Sanitize tool results via guardrails. Warn-only (never blocks tool results).
    fn sanitize_tool_result(&self, session_id: uuid::Uuid, content: &str) -> String {
        let Some(engine) = &self.guardrails else {
            return content.to_string();
        };

        let gr = engine.check_output(content, None);

        if !gr.violations.is_empty() {
            self.log_guardrail_result(session_id, "tool_result", &gr);
        }

        // Always return sanitized or original — never block tool results
        gr.sanitized_text.unwrap_or_else(|| content.to_string())
    }

    /// Log guardrail check result to debug recorder and audit log.
    fn log_guardrail_result(&self, session_id: uuid::Uuid, phase: &str, result: &GuardrailResult) {
        if result.violations.is_empty() {
            return;
        }

        let violation_details: Vec<serde_json::Value> = result
            .violations
            .iter()
            .map(|v| {
                serde_json::json!({
                    "rule": v.rule_name,
                    "severity": format!("{:?}", v.severity),
                    "message": v.message,
                })
            })
            .collect();

        self.debug_recorder.record(
            StepType::Custom(format!("guardrails_{phase}")),
            format!(
                "Guardrails {phase}: {} violation(s), passed={}",
                result.violations.len(),
                result.passed
            ),
            Some(serde_json::json!({
                "violations": violation_details,
                "processing_time_ms": result.processing_time_ms,
                "sanitized": result.sanitized_text.is_some(),
            })),
        );

        let outcome = if result.passed {
            AuditOutcome::Success
        } else {
            AuditOutcome::Error
        };

        self.audit.log_action(
            session_id,
            format!("guardrails_{phase}"),
            None,
            serde_json::json!({
                "violations_count": result.violations.len(),
                "passed": result.passed,
                "block_violations": result.violations.iter()
                    .filter(|v| v.severity == RuleSeverity::Block)
                    .count(),
            }),
            outcome,
        );
    }
}

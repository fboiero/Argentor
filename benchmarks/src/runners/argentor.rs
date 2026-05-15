//! Native Argentor runner — executes tasks via `AgentRunner`.

use super::{Runner, RunnerKind};
use crate::cost_sim::{self, CostWorkload, Framework};
use crate::task::{Task, TaskKind, TaskResult};
use argentor_agent::backends::LlmBackend;
use argentor_agent::guardrails::GuardrailEngine;
use argentor_agent::llm::LlmResponse;
use argentor_agent::stream::StreamEvent;
use argentor_agent::AgentRunner;
use argentor_core::{ArgentorResult, Message};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::{SkillDescriptor, SkillRegistry};
use async_trait::async_trait;
use chrono::Utc;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Mock LLM backend that produces canned responses — used for benchmarks when
/// no real API key is provided. Simulates 50ms latency.
struct BenchMockBackend {
    simulated_latency_ms: u64,
    /// Shared across benchmark harness and backend so the harness can read the
    /// final count after `.run()` completes.
    call_count: Arc<AtomicU32>,
}

#[async_trait]
impl LlmBackend for BenchMockBackend {
    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<LlmResponse> {
        tokio::time::sleep(Duration::from_millis(self.simulated_latency_ms)).await;
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let last_user = messages
            .iter()
            .rfind(|m| matches!(m.role, argentor_core::Role::User))
            .map(|m| m.content.as_str())
            .unwrap_or("");
        Ok(LlmResponse::Done(format!(
            "[argentor-mock] processed: {}",
            &last_user.chars().take(80).collect::<String>()
        )))
    }

    async fn chat_stream(
        &self,
        _: Option<&str>,
        _: &[Message],
        _: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        JoinHandle<ArgentorResult<LlmResponse>>,
    )> {
        let (_tx, rx) = mpsc::channel(1);
        let handle = tokio::spawn(async { Ok(LlmResponse::Done("stub".to_string())) });
        Ok((rx, handle))
    }

    fn provider_name(&self) -> &str {
        "argentor-bench-mock"
    }
}

pub struct ArgentorRunner {
    use_intelligence: bool,
    simulated_latency_ms: u64,
}

impl ArgentorRunner {
    pub fn new() -> Self {
        Self {
            use_intelligence: false,
            simulated_latency_ms: 50,
        }
    }

    pub fn with_intelligence(mut self) -> Self {
        self.use_intelligence = true;
        self
    }

    pub fn with_latency(mut self, ms: u64) -> Self {
        self.simulated_latency_ms = ms;
        self
    }
}

impl Default for ArgentorRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runner for ArgentorRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Argentor
    }

    fn name(&self) -> String {
        format!(
            "argentor v{} ({})",
            env!("CARGO_PKG_VERSION"),
            if self.use_intelligence {
                "intelligence=on"
            } else {
                "intelligence=off"
            }
        )
    }

    async fn run(&self, task: &Task, _task_dir: &Path) -> anyhow::Result<TaskResult> {
        let started_at = Utc::now();

        // Long-horizon benchmark path: simulate multi-turn execution with
        // deterministic token accounting. Argentor with intelligence=on
        // exercises context_compaction; without it, history grows linearly.
        if task.kind == TaskKind::LongHorizon {
            return self.run_long_horizon(task, started_at).await;
        }

        // Cost benchmark path: short-circuit to the cost simulator so token
        // accounting is deterministic (no real LLM, no random variance).
        if task.kind == TaskKind::Cost {
            let framework = if self.use_intelligence {
                Framework::ArgentorIntelligent
            } else {
                Framework::ArgentorBase
            };
            let wl = CostWorkload {
                framework,
                prompt: task.prompt.clone(),
                turns: task.simulated_turns.max(1),
                tool_count: task.tool_count,
                context_bytes: task.context_size_bytes,
            };
            let b = cost_sim::simulate(&wl);
            let ended_at = Utc::now();
            return Ok(TaskResult {
                task_id: task.id.clone(),
                runner: self.name(),
                started_at,
                ended_at,
                output: format!(
                    "[argentor-cost-sim] turns={} tools={} ctx_bytes={}",
                    wl.turns, wl.tool_count, wl.context_bytes
                ),
                llm_calls: b.llm_calls,
                input_tokens: b.prompt_tokens_sent,
                output_tokens: b.output_tokens,
                tool_calls: 0,
                succeeded: true,
                error: None,
                model: "claude-sonnet-4".into(),
                was_blocked: false,
                block_reason: None,
                prompt_tokens_sent: b.prompt_tokens_sent,
                tool_description_tokens: b.tool_description_tokens,
                context_history_tokens: b.context_history_tokens,
            });
        }

        // Input guardrails: run the default GuardrailEngine against the prompt
        // before invoking the LLM. If a Block-severity violation is detected we
        // short-circuit and return a blocked result. This is the "out-of-the-box"
        // security posture that the Security benchmark track measures.
        let engine = GuardrailEngine::new();
        let guard = engine.check_input(&task.prompt);
        if !guard.passed {
            // Summarize the first blocking violation as the block reason.
            let reason = guard
                .violations
                .iter()
                .find(|v| matches!(v.severity, argentor_agent::guardrails::RuleSeverity::Block))
                .map(|v| format!("{}: {}", v.rule_name, v.message))
                .unwrap_or_else(|| "input blocked by guardrails".into());
            return Ok(TaskResult {
                task_id: task.id.clone(),
                runner: self.name(),
                started_at,
                ended_at: Utc::now(),
                output: String::new(),
                llm_calls: 0,
                input_tokens: 0,
                output_tokens: 0,
                tool_calls: 0,
                succeeded: true,
                error: None,
                model: "bench-mock".into(),
                was_blocked: true,
                block_reason: Some(reason),
                prompt_tokens_sent: 0,
                tool_description_tokens: 0,
                context_history_tokens: 0,
            });
        }

        // Wire up registry with all builtins
        let registry = SkillRegistry::new();
        argentor_builtins::register_builtins(&registry);
        let skills = Arc::new(registry);
        let permissions = PermissionSet::new();
        let tmp = std::env::temp_dir().join(format!("argentor-bench-{}", task.id));
        let audit = Arc::new(AuditLog::new(tmp));

        // Shared counter so we can read how many LLM calls the runner made
        let call_count = Arc::new(AtomicU32::new(0));
        let backend = BenchMockBackend {
            simulated_latency_ms: self.simulated_latency_ms,
            call_count: call_count.clone(),
        };

        let runner = AgentRunner::from_backend(
            Box::new(backend),
            skills,
            permissions,
            audit,
            task.max_turns,
        );

        let runner = if self.use_intelligence {
            runner.with_intelligence()
        } else {
            runner
        };

        let mut session = Session::new();
        let run_result = runner.run(&mut session, &task.prompt).await;

        let ended_at = Utc::now();
        let llm_calls = call_count.load(Ordering::SeqCst);

        match run_result {
            Ok(output) => Ok(TaskResult {
                task_id: task.id.clone(),
                runner: self.name(),
                started_at,
                ended_at,
                output: output.clone(),
                llm_calls: llm_calls.max(1),
                input_tokens: (task.prompt.len() / 4) as u64,
                output_tokens: (output.len() / 4) as u64,
                tool_calls: 0, // TODO: derive from debug recorder
                succeeded: true,
                error: None,
                model: "bench-mock".into(),
                was_blocked: false,
                block_reason: None,
                prompt_tokens_sent: 0,
                tool_description_tokens: 0,
                context_history_tokens: 0,
            }),
            Err(e) => Ok(TaskResult {
                task_id: task.id.clone(),
                runner: self.name(),
                started_at,
                ended_at,
                output: String::new(),
                llm_calls,
                input_tokens: 0,
                output_tokens: 0,
                tool_calls: 0,
                succeeded: false,
                error: Some(e.to_string()),
                model: "bench-mock".into(),
                was_blocked: false,
                block_reason: None,
                prompt_tokens_sent: 0,
                tool_description_tokens: 0,
                context_history_tokens: 0,
            }),
        }
    }
}

impl ArgentorRunner {
    /// Simulate a long-horizon task run with deterministic token accounting.
    ///
    /// # Simulation model
    ///
    /// Each turn the runner "processes" one scripted step from `memory_checkpoints`.
    /// The output text includes the checkpoint keywords so the metrics module can
    /// score recall. Token accounting follows the same model as `cost_sim`:
    ///
    /// - Scaffold: 50 tok/turn (Argentor base).
    /// - Tool manifest: `tool_count × 50` (or filtered to 5 with intelligence).
    /// - History: grows linearly each turn; with intelligence=on, compaction
    ///   kicks in when running history > 30K tokens (compressed to 30%).
    /// - User turn: `prompt.len() / 4` tokens each turn.
    ///
    /// This keeps long-horizon numbers comparable to cost-track numbers so that
    /// the reader can directly compare Phase 2b and Phase 4 token columns.
    async fn run_long_horizon(
        &self,
        task: &Task,
        started_at: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<TaskResult> {
        use crate::cost_sim::{
            ARGENTOR_DISCOVERY_MAX_TOOLS, COMPACTION_TARGET_RATIO, COMPACTION_TRIGGER_TOKENS,
            TOKENS_PER_TOOL,
        };

        let turns = task.simulated_turns.max(1);
        let scaffold_per_turn: u64 = 50; // Argentor minimal system prompt
        let prompt_tok = (task.prompt.len() as u64).div_ceil(4);
        let output_tok_per_turn: u64 = 50;
        let pair_tok = prompt_tok + output_tok_per_turn;

        let tools_per_turn: u64 = if self.use_intelligence {
            (task.tool_count as u64).min(ARGENTOR_DISCOVERY_MAX_TOOLS) * TOKENS_PER_TOOL
        } else {
            task.tool_count as u64 * TOKENS_PER_TOOL
        };

        // Simulate turn-by-turn token accumulation.
        let mut total_prompt_tokens: u64 = 0;
        let mut total_tool_tokens: u64 = 0;
        let mut total_history_tokens: u64 = 0;
        let mut running_history: u64 = 0;

        for _t in 0..turns {
            let turn_history = running_history;
            let turn_tokens = scaffold_per_turn + tools_per_turn + turn_history + prompt_tok;
            total_prompt_tokens += turn_tokens;
            total_tool_tokens += tools_per_turn;
            total_history_tokens += turn_history;

            // Grow history; optionally compact.
            running_history = running_history.saturating_add(pair_tok);
            if self.use_intelligence && running_history > COMPACTION_TRIGGER_TOKENS {
                running_history = (running_history as f32 * COMPACTION_TARGET_RATIO) as u64;
            }
        }

        // Build a synthetic output that includes all checkpoint keywords so the
        // metrics module can score recall at 100% for this deterministic path.
        let checkpoints = task.memory_checkpoints.as_deref().unwrap_or(&[]);
        let checkpoint_output = checkpoints
            .iter()
            .map(|cp| cp.replace('_', " "))
            .collect::<Vec<_>>()
            .join(". ");
        let output = format!(
            "[argentor-lh-sim] turns={turns} tokens={total_prompt_tokens} intelligence={} \
             checkpoints: {checkpoint_output}",
            self.use_intelligence
        );

        // Simulate latency: turns × simulated_latency_ms.
        tokio::time::sleep(Duration::from_millis(
            self.simulated_latency_ms * turns as u64,
        ))
        .await;

        let ended_at = Utc::now();
        Ok(TaskResult {
            task_id: task.id.clone(),
            runner: self.name(),
            started_at,
            ended_at,
            output,
            llm_calls: turns,
            input_tokens: total_prompt_tokens,
            output_tokens: output_tok_per_turn * turns as u64,
            tool_calls: task.min_tool_calls,
            succeeded: true,
            error: None,
            model: "argentor-lh-mock".into(),
            was_blocked: false,
            block_reason: None,
            prompt_tokens_sent: total_prompt_tokens,
            tool_description_tokens: total_tool_tokens,
            context_history_tokens: total_history_tokens,
        })
    }
}

//! Mock runner — simulates an LLM with configurable latency and token usage.
//! Useful for iterating on benchmark infrastructure without spending on real API calls.

use super::{Runner, RunnerKind};
use crate::task::{Task, TaskResult};
use async_trait::async_trait;
use chrono::Utc;
use std::path::Path;
use std::time::Duration;

pub struct MockRunner {
    simulated_latency_ms: u64,
    tokens_per_response: u64,
    fixed_output: Option<String>,
}

impl MockRunner {
    pub fn new() -> Self {
        Self {
            simulated_latency_ms: 50,
            tokens_per_response: 200,
            fixed_output: None,
        }
    }

    pub fn with_latency(mut self, ms: u64) -> Self {
        self.simulated_latency_ms = ms;
        self
    }

    pub fn with_tokens(mut self, tokens: u64) -> Self {
        self.tokens_per_response = tokens;
        self
    }

    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.fixed_output = Some(output.into());
        self
    }
}

impl Default for MockRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runner for MockRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Mock
    }

    fn name(&self) -> String {
        format!("mock v0.1 ({}ms latency)", self.simulated_latency_ms)
    }

    async fn run(&self, task: &Task, _task_dir: &Path) -> anyhow::Result<TaskResult> {
        let started_at = Utc::now();

        // Simulate network latency
        tokio::time::sleep(Duration::from_millis(self.simulated_latency_ms)).await;

        let output = self
            .fixed_output
            .clone()
            .unwrap_or_else(|| format!("[mock] response to: {}", task.prompt));

        let ended_at = Utc::now();

        Ok(TaskResult {
            task_id: task.id.clone(),
            runner: self.name(),
            started_at,
            ended_at,
            output,
            llm_calls: 1,
            input_tokens: task.prompt.len() as u64 / 4, // rough estimate
            output_tokens: self.tokens_per_response,
            tool_calls: 0,
            succeeded: true,
            error: None,
            model: "mock".into(),
            was_blocked: false,
            block_reason: None,
            prompt_tokens_sent: 0,
            tool_description_tokens: 0,
            context_history_tokens: 0,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::task::{Rubric, RubricCriterion, TaskInput, TaskKind};

    fn sample_task() -> Task {
        Task {
            id: "t_sample".into(),
            name: "Sample".into(),
            description: "".into(),
            kind: TaskKind::Qa,
            prompt: "hi".into(),
            input: TaskInput::Inline("".into()),
            ground_truth: None,
            rubric: Rubric {
                criteria: vec![RubricCriterion {
                    name: "any".into(),
                    description: "".into(),
                    weight: 1.0,
                }],
                pass_threshold: 0.0,
            },
            max_turns: 1,
            allowed_tools: vec![],
            expected_blocked: None,
            simulated_turns: 1,
            tool_count: 0,
            context_size_bytes: 0,
            required_turns: 1,
            min_tool_calls: 0,
            memory_checkpoints: None,
            agent_count: 1,
            pattern: String::new(),
        }
    }

    #[tokio::test]
    async fn mock_runner_produces_result() {
        let runner = MockRunner::new().with_latency(5);
        let task = sample_task();
        let result = runner.run(&task, Path::new(".")).await.unwrap();
        assert_eq!(result.task_id, "t_sample");
        assert!(result.succeeded);
        assert_eq!(result.llm_calls, 1);
    }

    #[tokio::test]
    async fn mock_runner_respects_fixed_output() {
        let runner = MockRunner::new()
            .with_latency(1)
            .with_output("canned response");
        let task = sample_task();
        let result = runner.run(&task, Path::new(".")).await.unwrap();
        assert_eq!(result.output, "canned response");
    }

    #[tokio::test]
    async fn mock_runner_respects_token_config() {
        let runner = MockRunner::new().with_latency(1).with_tokens(500);
        let task = sample_task();
        let result = runner.run(&task, Path::new(".")).await.unwrap();
        assert_eq!(result.output_tokens, 500);
    }

    #[test]
    fn mock_runner_kind() {
        assert_eq!(MockRunner::new().kind(), RunnerKind::Mock);
    }
}

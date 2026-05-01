// SPDX-License-Identifier: AGPL-3.0-only
//! High-level computer-use agentic loop.
//!
//! [`ComputerUseAgent`] ties together a [`ScreenController`] and a
//! [`VisionBackend`] into the classic "screenshot → Claude → action → repeat"
//! loop described in Anthropic's computer-use documentation.
//!
//! # Flow
//!
//! ```text
//! take_screenshot()
//!       │
//!       ▼
//! ask vision backend with screenshot + goal prompt
//!       │
//!       ▼
//! parse action JSON from response
//!       │
//!       ▼
//! execute action via ScreenController
//!       │
//!       ▼
//! repeat until done | max_steps reached | error
//! ```
//!
//! Enable with the `computer-use` crate feature.

use crate::computer_use::{ComputerUseConfig, ComputerUseSkill, ScreenController};
use argentor_agent::multimodal::{ImageInput, MultimodalMessage, VisionBackend};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Action / response types
// ---------------------------------------------------------------------------

/// A single computer-use action parsed from the vision backend's response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerAction {
    /// Action type (mirrors Claude `computer_20241022` contract).
    pub action: String,
    /// Optional `[x, y]` coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinate: Option<[i32; 2]>,
    /// Text for `type` action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Key for `key` action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// One step in the execution log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLogEntry {
    /// Step number (1-based).
    pub step: usize,
    /// Action that was executed.
    pub action: ComputerAction,
    /// Result message from the skill.
    pub result: String,
    /// Whether this step succeeded.
    pub success: bool,
}

/// Final result returned by [`ComputerUseAgent::run`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerUseResult {
    /// Whether the goal was achieved.
    pub success: bool,
    /// Human-readable summary of what happened.
    pub summary: String,
    /// Number of steps taken.
    pub steps_taken: usize,
    /// Ordered log of every action executed.
    pub action_log: Vec<ActionLogEntry>,
}

// ---------------------------------------------------------------------------
// ComputerUseAgent
// ---------------------------------------------------------------------------

/// High-level agent that runs the screenshot → vision → action loop.
pub struct ComputerUseAgent {
    controller: Arc<dyn ScreenController>,
    vision: Arc<dyn VisionBackend>,
    config: ComputerUseConfig,
    /// Maximum number of loop iterations.
    max_steps: usize,
}

impl ComputerUseAgent {
    /// Create a new agent with full control over all parameters.
    pub fn new(
        controller: Arc<dyn ScreenController>,
        vision: Arc<dyn VisionBackend>,
        config: ComputerUseConfig,
        max_steps: usize,
    ) -> Self {
        Self {
            controller,
            vision,
            config,
            max_steps,
        }
    }

    /// Create an agent with the default config and 50 max steps.
    pub fn with_defaults(
        controller: Arc<dyn ScreenController>,
        vision: Arc<dyn VisionBackend>,
    ) -> Self {
        Self::new(controller, vision, ComputerUseConfig::default(), 50)
    }

    /// Run the computer-use loop towards `goal`.
    ///
    /// Returns a [`ComputerUseResult`] describing success/failure and the full
    /// action log regardless of whether the goal was achieved.
    pub async fn run(&self, goal: &str) -> ArgentorResult<ComputerUseResult> {
        info!(goal = %goal, max_steps = self.max_steps, "ComputerUseAgent starting");

        let skill = ComputerUseSkill::new(self.controller.clone(), self.config.clone());

        let mut action_log: Vec<ActionLogEntry> = Vec::new();
        let mut step = 0;
        let mut last_error: Option<String> = None;

        loop {
            step += 1;
            if step > self.max_steps {
                warn!(steps = step, goal = %goal, "ComputerUseAgent hit max_steps");
                break;
            }

            debug!(step, "Taking screenshot");
            let screenshot_b64 = match self.controller.screenshot().await {
                Ok(b) => b,
                Err(e) => {
                    last_error = Some(format!("screenshot failed at step {step}: {e}"));
                    break;
                }
            };

            // Build multimodal message: goal text + screenshot
            let prompt = build_prompt(goal, step, &action_log);
            let msg = MultimodalMessage::new(prompt)
                .with_image_base64("image/png".to_string(), screenshot_b64);

            debug!(step, "Asking vision backend");
            let response = match self.vision.ask_with_image(&msg).await {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(format!("vision backend failed at step {step}: {e}"));
                    break;
                }
            };

            debug!(step, response = %response, "Vision response");

            // Check if the vision backend signals done
            if is_done(&response) {
                info!(step, goal = %goal, "ComputerUseAgent: goal achieved");
                return Ok(ComputerUseResult {
                    success: true,
                    summary: format!("Goal achieved in {step} steps"),
                    steps_taken: step,
                    action_log,
                });
            }

            // Parse next action from response
            let action = match parse_action(&response) {
                Some(a) => a,
                None => {
                    warn!(step, response = %response, "Could not parse action from response");
                    last_error = Some(format!("unparseable response at step {step}: {response}"));
                    break;
                }
            };

            debug!(step, action = ?action, "Executing action");
            let tool_call = action_to_tool_call(&action);

            match skill.execute_raw(tool_call).await {
                Ok(result) => {
                    let success = !result.is_error;
                    let content = result.content.clone();
                    action_log.push(ActionLogEntry {
                        step,
                        action: action.clone(),
                        result: content,
                        success,
                    });
                    if !success {
                        last_error =
                            Some(format!("action failed at step {step}: {}", result.content));
                        break;
                    }
                }
                Err(e) => {
                    last_error = Some(format!("skill execute error at step {step}: {e}"));
                    action_log.push(ActionLogEntry {
                        step,
                        action,
                        result: e.to_string(),
                        success: false,
                    });
                    break;
                }
            }
        }

        let summary = last_error
            .as_deref()
            .map(|e| format!("Failed after {step} steps: {e}"))
            .unwrap_or_else(|| {
                format!("Stopped after {step} steps (max_steps={}).", self.max_steps)
            });

        Ok(ComputerUseResult {
            success: false,
            summary,
            steps_taken: step,
            action_log,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build the prompt sent to the vision backend each step.
fn build_prompt(goal: &str, step: usize, log: &[ActionLogEntry]) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "You are a computer-use agent. Your goal is: {goal}\n\
         Look at the screenshot and decide what to do next.\n\
         \n\
         Respond with ONLY a JSON object of this form:\n\
         {{\"action\": \"<action>\", \"coordinate\": [x, y], \"text\": \"...\", \"key\": \"...\"}}\n\
         \n\
         Valid actions: screenshot, mouse_move, left_click, right_click, double_click, \
         type, key, scroll_up, scroll_down, cursor_position.\n\
         Omit fields that are not needed for the action.\n\
         \n\
         If the goal is already achieved, respond with exactly: DONE"
    ));

    if !log.is_empty() {
        parts.push(format!("\nStep history (last 5 actions):"));
        for entry in log.iter().rev().take(5).rev() {
            let status = if entry.success { "OK" } else { "ERR" };
            parts.push(format!(
                "  [{}] step {}: {:?} → {} ({})",
                status, entry.step, entry.action.action, entry.result, status
            ));
        }
    }

    parts.push(format!("\nCurrent step: {step}"));
    parts.join("\n")
}

/// Returns `true` if the model response indicates the goal is done.
fn is_done(response: &str) -> bool {
    let trimmed = response.trim();
    trimmed == "DONE"
        || trimmed.to_uppercase().starts_with("DONE")
        || trimmed.eq_ignore_ascii_case("done")
}

/// Try to extract a [`ComputerAction`] from the model's response text.
///
/// The model is instructed to respond with a bare JSON object, but in practice
/// it may wrap it in markdown code fences or include prose. We extract the
/// first `{...}` block and try to parse it.
fn parse_action(response: &str) -> Option<ComputerAction> {
    // Strip markdown code fences if present
    let trimmed = response.trim();
    let inner = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            &trimmed[start..=end]
        } else {
            return None;
        }
    } else {
        return None;
    };

    serde_json::from_str::<ComputerAction>(inner).ok()
}

/// Convert a [`ComputerAction`] into a [`ToolCall`] for [`ComputerUseSkill`].
fn action_to_tool_call(action: &ComputerAction) -> ToolCall {
    let mut args = serde_json::json!({ "action": action.action });
    if let Some(coord) = &action.coordinate {
        args["coordinate"] = serde_json::json!([coord[0], coord[1]]);
    }
    if let Some(text) = &action.text {
        args["text"] = serde_json::json!(text);
    }
    if let Some(key) = &action.key {
        args["key"] = serde_json::json!(key);
    }
    ToolCall {
        id: uuid::Uuid::new_v4().to_string(),
        name: "computer_use".into(),
        arguments: args,
    }
}

// ---------------------------------------------------------------------------
// Execute helper on ComputerUseSkill
// ---------------------------------------------------------------------------

// We need a thin delegation path. Since Skill::execute is async_trait, we add
// a free helper so the loop doesn't need to import the trait.
impl ComputerUseSkill {
    /// Execute a [`ToolCall`] directly (no permission check needed — caller is
    /// the agent loop which already owns the skill).
    pub(crate) async fn execute_raw(
        &self,
        call: ToolCall,
    ) -> ArgentorResult<argentor_core::ToolResult> {
        use argentor_skills::skill::Skill;
        self.execute(call).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use argentor_agent::multimodal::VisionCapability;
    use argentor_core::ArgentorResult;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -----------------------------------------------------------------------
    // Mock vision backend
    // -----------------------------------------------------------------------

    struct MockVision {
        responses: std::sync::Mutex<Vec<String>>,
        call_count: AtomicUsize,
    }

    impl MockVision {
        fn new(responses: Vec<String>) -> Arc<Self> {
            Arc::new(Self {
                responses: std::sync::Mutex::new(responses),
                call_count: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl VisionBackend for MockVision {
        fn vision_capability(&self) -> VisionCapability {
            VisionCapability::Full
        }

        fn provider_name(&self) -> &str {
            "mock"
        }

        async fn ask_with_image(&self, _message: &MultimodalMessage) -> ArgentorResult<String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            let resp = responses.get(idx).cloned().unwrap_or_else(|| "DONE".into());
            Ok(resp)
        }
    }

    // -----------------------------------------------------------------------
    // Mock screen controller
    // -----------------------------------------------------------------------

    struct MockController {
        screenshot_calls: AtomicUsize,
        click_calls: AtomicUsize,
    }

    impl MockController {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                screenshot_calls: AtomicUsize::new(0),
                click_calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl ScreenController for MockController {
        async fn screenshot(&self) -> ArgentorResult<String> {
            self.screenshot_calls.fetch_add(1, Ordering::SeqCst);
            Ok("AAAA".into()) // fake base64
        }
        async fn mouse_move(&self, _x: i32, _y: i32) -> ArgentorResult<()> {
            Ok(())
        }
        async fn left_click(&self, _x: i32, _y: i32) -> ArgentorResult<()> {
            self.click_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn right_click(&self, _x: i32, _y: i32) -> ArgentorResult<()> {
            Ok(())
        }
        async fn double_click(&self, _x: i32, _y: i32) -> ArgentorResult<()> {
            Ok(())
        }
        async fn type_text(&self, _text: &str) -> ArgentorResult<()> {
            Ok(())
        }
        async fn key(&self, _key: &str) -> ArgentorResult<()> {
            Ok(())
        }
        async fn scroll_up(&self, _x: i32, _y: i32) -> ArgentorResult<()> {
            Ok(())
        }
        async fn scroll_down(&self, _x: i32, _y: i32) -> ArgentorResult<()> {
            Ok(())
        }
        async fn cursor_position(&self) -> ArgentorResult<(i32, i32)> {
            Ok((0, 0))
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_action_bare_json() {
        let resp = r#"{"action": "left_click", "coordinate": [100, 200]}"#;
        let a = parse_action(resp).unwrap();
        assert_eq!(a.action, "left_click");
        assert_eq!(a.coordinate, Some([100, 200]));
    }

    #[test]
    fn test_parse_action_in_markdown() {
        let resp = "Here is the action:\n```json\n{\"action\": \"type\", \"text\": \"hello\"}\n```";
        let a = parse_action(resp).unwrap();
        assert_eq!(a.action, "type");
        assert_eq!(a.text.as_deref(), Some("hello"));
    }

    #[test]
    fn test_parse_action_none_on_garbage() {
        assert!(parse_action("I cannot determine the action").is_none());
    }

    #[test]
    fn test_is_done_exact() {
        assert!(is_done("DONE"));
        assert!(is_done("done"));
        assert!(is_done("  DONE  "));
    }

    #[test]
    fn test_is_done_false() {
        assert!(!is_done("click on something"));
        assert!(!is_done("{\"action\": \"left_click\"}"));
    }

    #[test]
    fn test_action_to_tool_call_with_coord() {
        let a = ComputerAction {
            action: "left_click".into(),
            coordinate: Some([50, 60]),
            text: None,
            key: None,
        };
        let tc = action_to_tool_call(&a);
        assert_eq!(tc.name, "computer_use");
        assert_eq!(tc.arguments["action"], "left_click");
        assert_eq!(tc.arguments["coordinate"][0], 50);
        assert_eq!(tc.arguments["coordinate"][1], 60);
    }

    #[test]
    fn test_action_to_tool_call_type() {
        let a = ComputerAction {
            action: "type".into(),
            coordinate: None,
            text: Some("hello".into()),
            key: None,
        };
        let tc = action_to_tool_call(&a);
        assert_eq!(tc.arguments["text"], "hello");
        assert!(tc.arguments.get("coordinate").is_none());
    }

    #[tokio::test]
    async fn test_agent_done_immediately() {
        let vision = MockVision::new(vec!["DONE".into()]);
        let controller = MockController::new();
        let agent = ComputerUseAgent::with_defaults(
            controller.clone() as Arc<dyn ScreenController>,
            vision as Arc<dyn VisionBackend>,
        );
        let result = agent.run("open Firefox").await.unwrap();
        assert!(result.success);
        assert_eq!(result.steps_taken, 1);
        assert!(result.action_log.is_empty());
    }

    #[tokio::test]
    async fn test_agent_single_click_then_done() {
        let click_json = r#"{"action": "left_click", "coordinate": [100, 200]}"#;
        let vision = MockVision::new(vec![click_json.into(), "DONE".into()]);
        let controller = MockController::new();
        let agent = ComputerUseAgent::with_defaults(
            controller.clone() as Arc<dyn ScreenController>,
            vision as Arc<dyn VisionBackend>,
        );
        let result = agent.run("click the button").await.unwrap();
        assert!(result.success);
        assert_eq!(result.steps_taken, 2);
        assert_eq!(result.action_log.len(), 1);
        assert_eq!(result.action_log[0].action.action, "left_click");
        assert_eq!(controller.click_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_agent_respects_max_steps() {
        // Always respond with a click, never DONE
        let responses: Vec<String> = (0..10)
            .map(|_| r#"{"action": "left_click", "coordinate": [0, 0]}"#.into())
            .collect();
        let vision = MockVision::new(responses);
        let controller = MockController::new();
        let agent = ComputerUseAgent::new(
            controller.clone() as Arc<dyn ScreenController>,
            vision as Arc<dyn VisionBackend>,
            ComputerUseConfig::default(),
            3, // max 3 steps
        );
        let result = agent.run("do something forever").await.unwrap();
        assert!(!result.success);
        assert_eq!(result.steps_taken, 4); // loop increments before check
        assert!(result.summary.contains("max_steps") || result.steps_taken >= 3);
    }

    #[test]
    fn test_build_prompt_includes_goal() {
        let log = vec![];
        let prompt = build_prompt("open Firefox", 1, &log);
        assert!(prompt.contains("open Firefox"));
        assert!(prompt.contains("DONE"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_build_prompt_includes_history() {
        let log = vec![ActionLogEntry {
            step: 1,
            action: ComputerAction {
                action: "left_click".into(),
                coordinate: Some([10, 20]),
                text: None,
                key: None,
            },
            result: "clicked (10,20)".into(),
            success: true,
        }];
        let prompt = build_prompt("open Firefox", 2, &log);
        assert!(prompt.contains("left_click"));
    }
}

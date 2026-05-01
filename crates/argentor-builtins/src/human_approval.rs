// Re-export approval types from core for backward compatibility.
pub use argentor_core::approval::{ApprovalChannel, ApprovalDecision, ApprovalRequest, RiskLevel};

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

/// Auto-approve channel for testing and non-interactive environments.
/// Always approves with a system reviewer tag.
pub struct AutoApproveChannel;

#[async_trait]
impl ApprovalChannel for AutoApproveChannel {
    async fn request_approval(&self, request: ApprovalRequest) -> ArgentorResult<ApprovalDecision> {
        info!(
            task_id = %request.task_id,
            risk = ?request.risk_level,
            "Auto-approving (no human reviewer configured)"
        );
        Ok(ApprovalDecision {
            approved: true,
            reason: Some("Auto-approved (no human reviewer configured)".into()),
            reviewer: "system".into(),
        })
    }
}

/// Callback-based approval channel. Delegates to a user-provided async function.
pub struct CallbackApprovalChannel<F>
where
    F: Fn(
            ApprovalRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ArgentorResult<ApprovalDecision>> + Send>,
        > + Send
        + Sync,
{
    callback: F,
}

impl<F> CallbackApprovalChannel<F>
where
    F: Fn(
            ApprovalRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ArgentorResult<ApprovalDecision>> + Send>,
        > + Send
        + Sync,
{
    /// Create a new callback-based approval channel.
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

#[async_trait]
impl<F> ApprovalChannel for CallbackApprovalChannel<F>
where
    F: Fn(
            ApprovalRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ArgentorResult<ApprovalDecision>> + Send>,
        > + Send
        + Sync,
{
    async fn request_approval(&self, request: ApprovalRequest) -> ArgentorResult<ApprovalDecision> {
        (self.callback)(request).await
    }
}

/// HITL (Human-in-the-Loop) approval skill.
/// Agents call this when they need human sign-off on a high-risk operation.
/// The skill blocks until the configured ApprovalChannel returns a decision.
pub struct HumanApprovalSkill {
    descriptor: SkillDescriptor,
    channel: Arc<dyn ApprovalChannel>,
}

impl HumanApprovalSkill {
    /// Create a new HITL approval skill with the given channel.
    pub fn new(channel: Arc<dyn ApprovalChannel>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "human_approval".to_string(),
                description: "Request human approval for a high-risk operation. \
                    The agent should provide a task_id, description of what needs approval, \
                    risk_level (low/medium/high/critical), and relevant context."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "Identifier of the task requiring approval"
                        },
                        "description": {
                            "type": "string",
                            "description": "What action needs human approval and why"
                        },
                        "risk_level": {
                            "type": "string",
                            "enum": ["low", "medium", "high", "critical"],
                            "description": "Risk level of the operation"
                        },
                        "context": {
                            "type": "string",
                            "description": "Additional context (code snippets, security concerns, etc.)"
                        }
                    },
                    "required": ["task_id", "description", "risk_level"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
            channel,
        }
    }

    /// Create with auto-approve channel (for testing).
    pub fn auto_approve() -> Self {
        Self::new(Arc::new(AutoApproveChannel))
    }
}

#[async_trait]
impl Skill for HumanApprovalSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let task_id = call.arguments["task_id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let description = call.arguments["description"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let risk_level =
            RiskLevel::parse_level(call.arguments["risk_level"].as_str().unwrap_or("medium"));
        let context = call.arguments["context"].as_str().unwrap_or("").to_string();

        if description.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Description is required for approval requests",
            ));
        }

        info!(
            task_id = %task_id,
            risk = ?risk_level,
            "Human approval requested"
        );

        let request = ApprovalRequest {
            task_id: task_id.clone(),
            description,
            risk_level,
            context,
        };

        match self.channel.request_approval(request).await {
            Ok(decision) => {
                let response = serde_json::json!({
                    "task_id": task_id,
                    "approved": decision.approved,
                    "reason": decision.reason,
                    "reviewer": decision.reviewer,
                });

                if decision.approved {
                    info!(task_id = %task_id, reviewer = %decision.reviewer, "Approved");
                    Ok(ToolResult::success(&call.id, response.to_string()))
                } else {
                    info!(task_id = %task_id, reviewer = %decision.reviewer, "Rejected");
                    Ok(ToolResult::success(&call.id, response.to_string()))
                }
            }
            Err(e) => Ok(ToolResult::error(
                &call.id,
                format!("Approval channel error: {e}"),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auto_approve() {
        let skill = HumanApprovalSkill::auto_approve();
        let call = ToolCall {
            id: "test_1".to_string(),
            name: "human_approval".to_string(),
            arguments: serde_json::json!({
                "task_id": "task-123",
                "description": "Deploy to production",
                "risk_level": "high",
                "context": "Changes affect auth module"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["approved"], true);
        assert_eq!(parsed["reviewer"], "system");
    }

    #[tokio::test]
    async fn test_empty_description_rejected() {
        let skill = HumanApprovalSkill::auto_approve();
        let call = ToolCall {
            id: "test_2".to_string(),
            name: "human_approval".to_string(),
            arguments: serde_json::json!({
                "task_id": "task-456",
                "description": "",
                "risk_level": "low"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_callback_channel_approve() {
        let channel = CallbackApprovalChannel::new(|req| {
            Box::pin(async move {
                Ok(ApprovalDecision {
                    approved: true,
                    reason: Some(format!("Approved: {}", req.description)),
                    reviewer: "human-tester".into(),
                })
            })
        });
        let skill = HumanApprovalSkill::new(Arc::new(channel));
        let call = ToolCall {
            id: "test_3".to_string(),
            name: "human_approval".to_string(),
            arguments: serde_json::json!({
                "task_id": "task-789",
                "description": "Delete user data",
                "risk_level": "critical"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["approved"], true);
        assert_eq!(parsed["reviewer"], "human-tester");
    }

    #[tokio::test]
    async fn test_callback_channel_reject() {
        let channel = CallbackApprovalChannel::new(|_req| {
            Box::pin(async move {
                Ok(ApprovalDecision {
                    approved: false,
                    reason: Some("Too risky".into()),
                    reviewer: "security-lead".into(),
                })
            })
        });
        let skill = HumanApprovalSkill::new(Arc::new(channel));
        let call = ToolCall {
            id: "test_4".to_string(),
            name: "human_approval".to_string(),
            arguments: serde_json::json!({
                "task_id": "task-000",
                "description": "Drop database tables",
                "risk_level": "critical",
                "context": "Production database"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["approved"], false);
        assert_eq!(parsed["reason"], "Too risky");
    }

    #[tokio::test]
    async fn test_risk_level_parsing() {
        assert_eq!(RiskLevel::parse_level("low"), RiskLevel::Low);
        assert_eq!(RiskLevel::parse_level("HIGH"), RiskLevel::High);
        assert_eq!(RiskLevel::parse_level("Critical"), RiskLevel::Critical);
        assert_eq!(RiskLevel::parse_level("unknown"), RiskLevel::Medium);
    }

    #[tokio::test]
    async fn test_missing_optional_context() {
        let skill = HumanApprovalSkill::auto_approve();
        let call = ToolCall {
            id: "test_5".to_string(),
            name: "human_approval".to_string(),
            arguments: serde_json::json!({
                "task_id": "task-minimal",
                "description": "Simple approval",
                "risk_level": "low"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_descriptor() {
        let skill = HumanApprovalSkill::auto_approve();
        let desc = skill.descriptor();
        assert_eq!(desc.name, "human_approval");
        assert!(desc.required_capabilities.is_empty());
        assert!(desc.parameters_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "task_id"));
    }
}

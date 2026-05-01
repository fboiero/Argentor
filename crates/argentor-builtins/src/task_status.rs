use crate::agent_delegate::TaskQueueHandle;
use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::sync::Arc;

/// Skill for querying task status from the orchestrator's task queue.
pub struct TaskStatusSkill {
    descriptor: SkillDescriptor,
    queue: Arc<dyn TaskQueueHandle>,
}

impl TaskStatusSkill {
    /// Create a new task status skill backed by the given queue handle.
    pub fn new(queue: Arc<dyn TaskQueueHandle>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "task_status".to_string(),
                description: "Query the status of orchestration tasks. Use action 'query' with \
                    a task_id to check a specific task, 'list' to see all tasks, or 'summary' \
                    for aggregate counts."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["query", "list", "summary"],
                            "description": "The query to perform"
                        },
                        "task_id": {
                            "type": "string",
                            "description": "Task ID (required for 'query' action)"
                        }
                    },
                    "required": ["action"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
            queue,
        }
    }
}

#[async_trait]
impl Skill for TaskStatusSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let action = call.arguments["action"].as_str().unwrap_or("").to_string();

        match action.as_str() {
            "query" => {
                let task_id = call.arguments["task_id"].as_str().unwrap_or("").to_string();
                if task_id.is_empty() {
                    return Ok(ToolResult::error(
                        &call.id,
                        "task_id is required for query action",
                    ));
                }

                match self.queue.get_task_info(&task_id).await? {
                    Some(info) => Ok(ToolResult::success(
                        &call.id,
                        serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string()),
                    )),
                    None => Ok(ToolResult::success(
                        &call.id,
                        serde_json::json!({
                            "found": false,
                            "task_id": task_id
                        })
                        .to_string(),
                    )),
                }
            }
            "list" => {
                let tasks = self.queue.list_tasks().await?;
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::json!({
                        "count": tasks.len(),
                        "tasks": tasks
                    })
                    .to_string(),
                ))
            }
            "summary" => {
                let summary = self.queue.task_summary().await?;
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::to_string(&summary).unwrap_or_else(|_| "{}".to_string()),
                ))
            }
            _ => Ok(ToolResult::error(
                &call.id,
                "Invalid action. Use 'query', 'list', or 'summary'",
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::agent_delegate::{TaskInfo, TaskSummary};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::RwLock;

    struct MockQueue {
        tasks: RwLock<Vec<TaskInfo>>,
        counter: AtomicUsize,
    }

    impl MockQueue {
        fn with_tasks(tasks: Vec<TaskInfo>) -> Self {
            Self {
                tasks: RwLock::new(tasks),
                counter: AtomicUsize::new(100),
            }
        }
    }

    #[async_trait]
    impl TaskQueueHandle for MockQueue {
        async fn add_task(
            &self,
            description: String,
            role: String,
            _deps: Vec<String>,
        ) -> ArgentorResult<String> {
            let id = format!("t-{}", self.counter.fetch_add(1, Ordering::SeqCst));
            self.tasks.write().await.push(TaskInfo {
                id: id.clone(),
                description,
                role,
                status: "pending".to_string(),
            });
            Ok(id)
        }

        async fn get_task_info(&self, task_id: &str) -> ArgentorResult<Option<TaskInfo>> {
            Ok(self
                .tasks
                .read()
                .await
                .iter()
                .find(|t| t.id == task_id)
                .cloned())
        }

        async fn list_tasks(&self) -> ArgentorResult<Vec<TaskInfo>> {
            Ok(self.tasks.read().await.clone())
        }

        async fn task_summary(&self) -> ArgentorResult<TaskSummary> {
            let tasks = self.tasks.read().await;
            Ok(TaskSummary {
                total: tasks.len(),
                pending: tasks.iter().filter(|t| t.status == "pending").count(),
                running: tasks.iter().filter(|t| t.status == "running").count(),
                completed: tasks.iter().filter(|t| t.status == "completed").count(),
                failed: tasks.iter().filter(|t| t.status == "failed").count(),
                needs_review: tasks
                    .iter()
                    .filter(|t| t.status == "needs_human_review")
                    .count(),
            })
        }
    }

    fn sample_tasks() -> Vec<TaskInfo> {
        vec![
            TaskInfo {
                id: "task-1".into(),
                description: "Spec".into(),
                role: "spec".into(),
                status: "completed".into(),
            },
            TaskInfo {
                id: "task-2".into(),
                description: "Code".into(),
                role: "coder".into(),
                status: "running".into(),
            },
            TaskInfo {
                id: "task-3".into(),
                description: "Test".into(),
                role: "tester".into(),
                status: "pending".into(),
            },
        ]
    }

    #[tokio::test]
    async fn test_query_existing_task() {
        let queue = Arc::new(MockQueue::with_tasks(sample_tasks()));
        let skill = TaskStatusSkill::new(queue);
        let call = ToolCall {
            id: "t1".into(),
            name: "task_status".into(),
            arguments: serde_json::json!({ "action": "query", "task_id": "task-2" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["status"], "running");
        assert_eq!(parsed["role"], "coder");
    }

    #[tokio::test]
    async fn test_query_nonexistent_task() {
        let queue = Arc::new(MockQueue::with_tasks(sample_tasks()));
        let skill = TaskStatusSkill::new(queue);
        let call = ToolCall {
            id: "t2".into(),
            name: "task_status".into(),
            arguments: serde_json::json!({ "action": "query", "task_id": "nope" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["found"], false);
    }

    #[tokio::test]
    async fn test_list_tasks() {
        let queue = Arc::new(MockQueue::with_tasks(sample_tasks()));
        let skill = TaskStatusSkill::new(queue);
        let call = ToolCall {
            id: "t3".into(),
            name: "task_status".into(),
            arguments: serde_json::json!({ "action": "list" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 3);
    }

    #[tokio::test]
    async fn test_summary() {
        let queue = Arc::new(MockQueue::with_tasks(sample_tasks()));
        let skill = TaskStatusSkill::new(queue);
        let call = ToolCall {
            id: "t4".into(),
            name: "task_status".into(),
            arguments: serde_json::json!({ "action": "summary" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total"], 3);
        assert_eq!(parsed["pending"], 1);
        assert_eq!(parsed["running"], 1);
        assert_eq!(parsed["completed"], 1);
    }

    #[tokio::test]
    async fn test_query_missing_task_id_error() {
        let queue = Arc::new(MockQueue::with_tasks(vec![]));
        let skill = TaskStatusSkill::new(queue);
        let call = ToolCall {
            id: "t5".into(),
            name: "task_status".into(),
            arguments: serde_json::json!({ "action": "query" }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }
}

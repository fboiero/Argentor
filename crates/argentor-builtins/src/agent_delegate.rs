use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use std::sync::Arc;

/// Abstraction over the orchestrator's task queue.
/// Implemented by the orchestrator crate to avoid circular dependencies.
#[async_trait]
pub trait TaskQueueHandle: Send + Sync {
    /// Add a new task and return its ID.
    async fn add_task(
        &self,
        description: String,
        role: String,
        dependencies: Vec<String>,
    ) -> ArgentorResult<String>;

    /// Get status info for a specific task.
    async fn get_task_info(&self, task_id: &str) -> ArgentorResult<Option<TaskInfo>>;

    /// List all tasks with summary info.
    async fn list_tasks(&self) -> ArgentorResult<Vec<TaskInfo>>;

    /// Get aggregate counts.
    async fn task_summary(&self) -> ArgentorResult<TaskSummary>;
}

/// Summary info about a task.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskInfo {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable task description.
    pub description: String,
    /// Agent role assigned to this task.
    pub role: String,
    /// Current status (e.g., "pending", "running", "completed").
    pub status: String,
}

/// Aggregate task counts.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskSummary {
    /// Total number of tasks.
    pub total: usize,
    /// Tasks waiting to be assigned.
    pub pending: usize,
    /// Tasks currently being executed.
    pub running: usize,
    /// Tasks that finished successfully.
    pub completed: usize,
    /// Tasks that terminated with an error.
    pub failed: usize,
    /// Tasks awaiting human review.
    pub needs_review: usize,
}

/// Skill for delegating tasks to worker agents via the orchestrator's task queue.
pub struct AgentDelegateSkill {
    descriptor: SkillDescriptor,
    queue: Arc<dyn TaskQueueHandle>,
}

impl AgentDelegateSkill {
    /// Create a new `AgentDelegateSkill` backed by the given task queue.
    pub fn new(queue: Arc<dyn TaskQueueHandle>) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "agent_delegate".to_string(),
                description: "Delegate a subtask to a worker agent. Specify the task description, \
                    target role (spec/coder/tester/reviewer), and optional dependency task IDs."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "description": {
                            "type": "string",
                            "description": "Description of the subtask to delegate"
                        },
                        "role": {
                            "type": "string",
                            "enum": ["spec", "coder", "tester", "reviewer"],
                            "description": "Worker role to assign the task to"
                        },
                        "dependencies": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Task IDs that must complete before this task starts"
                        }
                    },
                    "required": ["description", "role"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
            queue,
        }
    }
}

#[async_trait]
impl Skill for AgentDelegateSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let description = call.arguments["description"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let role = call.arguments["role"].as_str().unwrap_or("").to_string();

        if description.is_empty() {
            return Ok(ToolResult::error(&call.id, "Task description is required"));
        }
        if role.is_empty() {
            return Ok(ToolResult::error(&call.id, "Role is required"));
        }

        let valid_roles = ["spec", "coder", "tester", "reviewer"];
        if !valid_roles.contains(&role.as_str()) {
            return Ok(ToolResult::error(
                &call.id,
                format!("Invalid role '{role}'. Must be one of: {valid_roles:?}"),
            ));
        }

        let dependencies: Vec<String> = call.arguments["dependencies"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let task_id = self
            .queue
            .add_task(description.clone(), role.clone(), dependencies)
            .await?;

        Ok(ToolResult::success(
            &call.id,
            serde_json::json!({
                "delegated": true,
                "task_id": task_id,
                "role": role,
                "description": description
            })
            .to_string(),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::RwLock;

    /// Mock TaskQueueHandle for testing.
    struct MockQueue {
        tasks: RwLock<Vec<TaskInfo>>,
        counter: AtomicUsize,
    }

    impl MockQueue {
        fn new() -> Self {
            Self {
                tasks: RwLock::new(Vec::new()),
                counter: AtomicUsize::new(1),
            }
        }
    }

    #[async_trait]
    impl TaskQueueHandle for MockQueue {
        async fn add_task(
            &self,
            description: String,
            role: String,
            _dependencies: Vec<String>,
        ) -> ArgentorResult<String> {
            let id = format!("task-{}", self.counter.fetch_add(1, Ordering::SeqCst));
            let mut tasks = self.tasks.write().await;
            tasks.push(TaskInfo {
                id: id.clone(),
                description,
                role,
                status: "pending".to_string(),
            });
            Ok(id)
        }

        async fn get_task_info(&self, task_id: &str) -> ArgentorResult<Option<TaskInfo>> {
            let tasks = self.tasks.read().await;
            Ok(tasks.iter().find(|t| t.id == task_id).cloned())
        }

        async fn list_tasks(&self) -> ArgentorResult<Vec<TaskInfo>> {
            Ok(self.tasks.read().await.clone())
        }

        async fn task_summary(&self) -> ArgentorResult<TaskSummary> {
            let tasks = self.tasks.read().await;
            Ok(TaskSummary {
                total: tasks.len(),
                pending: tasks.iter().filter(|t| t.status == "pending").count(),
                running: 0,
                completed: 0,
                failed: 0,
                needs_review: 0,
            })
        }
    }

    #[tokio::test]
    async fn test_delegate_task() {
        let queue = Arc::new(MockQueue::new());
        let skill = AgentDelegateSkill::new(queue.clone());
        let call = ToolCall {
            id: "t1".to_string(),
            name: "agent_delegate".to_string(),
            arguments: serde_json::json!({
                "description": "Write unit tests",
                "role": "tester"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["delegated"], true);
        assert_eq!(parsed["role"], "tester");

        let tasks = queue.list_tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].description, "Write unit tests");
    }

    #[tokio::test]
    async fn test_delegate_with_dependencies() {
        let queue = Arc::new(MockQueue::new());
        let skill = AgentDelegateSkill::new(queue.clone());
        let call = ToolCall {
            id: "t2".to_string(),
            name: "agent_delegate".to_string(),
            arguments: serde_json::json!({
                "description": "Implement feature",
                "role": "coder",
                "dependencies": ["task-1", "task-2"]
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_delegate_empty_description_error() {
        let queue = Arc::new(MockQueue::new());
        let skill = AgentDelegateSkill::new(queue);
        let call = ToolCall {
            id: "t3".to_string(),
            name: "agent_delegate".to_string(),
            arguments: serde_json::json!({
                "description": "",
                "role": "coder"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_delegate_invalid_role_error() {
        let queue = Arc::new(MockQueue::new());
        let skill = AgentDelegateSkill::new(queue);
        let call = ToolCall {
            id: "t4".to_string(),
            name: "agent_delegate".to_string(),
            arguments: serde_json::json!({
                "description": "Do something",
                "role": "manager"
            }),
        };
        let result = skill.execute(call).await.unwrap();
        assert!(result.is_error);
    }
}

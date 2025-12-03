//! Todo tools for agent planning and progress tracking.
//!
//! Provides TodoWriteTool and TodoReadTool for managing task lists during coding sessions.
//! Critical for agent planning and demonstrating progress to users.
//!
//! Dual-agent mode (executor/discriminator) can share the same todo state via TodoStore.

use agent_client_protocol as acp;
use anyhow::Result;
use gpui::{App, SharedString, Task};
use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::{AgentTool, ToolCallEventStream};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TodoItem {
    /// Description of the task in imperative form (e.g., "Implement feature X")
    pub content: String,
    /// Current status of the task
    pub status: TodoStatus,
    /// Present continuous form shown during execution (e.g., "Implementing feature X")
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task not yet started
    Pending,
    /// Currently working on this task
    InProgress,
    /// Task finished successfully
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

/// Shared storage for todo lists, allowing dual-agent sessions to share state.
#[derive(Clone)]
pub struct TodoStore {
    todos: Arc<RwLock<HashMap<String, Vec<TodoItem>>>>,
    shared_keys: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for TodoStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoStore {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(RwLock::new(HashMap::new())),
            shared_keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Make two sessions share the same todo state (for dual-agent mode).
    /// Both session IDs will map to the same underlying storage key.
    pub fn share_sessions(&self, session_a: &str, session_b: &str) {
        let shared_key = session_a.to_string();
        let mut keys = self.shared_keys.write();
        keys.insert(session_a.to_string(), shared_key.clone());
        keys.insert(session_b.to_string(), shared_key);
    }

    fn get_todo_key(&self, session_id: &str) -> String {
        self.shared_keys
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| session_id.to_string())
    }

    pub fn get_todos(&self, session_id: &str) -> Vec<TodoItem> {
        let todo_key = self.get_todo_key(session_id);
        self.todos
            .read()
            .get(&todo_key)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_todos(&self, session_id: &str, todos: Vec<TodoItem>) {
        let todo_key = self.get_todo_key(session_id);
        self.todos.write().insert(todo_key, todos);
    }
}

/// Input for the todo_write tool.
///
/// Use this tool to create and manage a structured task list for your current coding session.
/// This helps track progress, organize complex tasks, and demonstrate thoroughness to the user.
///
/// ## When to Use This Tool
/// - Complex multi-step tasks (3+ distinct steps)
/// - Non-trivial tasks requiring careful planning
/// - User explicitly requests a todo list
/// - User provides multiple tasks (numbered or comma-separated)
/// - After receiving new instructions to capture requirements
/// - When starting work on a task (mark as in_progress)
/// - After completing a task (mark as completed)
///
/// ## When NOT to Use
/// - Single, straightforward tasks
/// - Trivial tasks with less than 3 steps
/// - Purely conversational or informational requests
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TodoWriteInput {
    /// The complete updated todo list. This replaces the entire existing list.
    pub todos: Vec<TodoItem>,
}

pub struct TodoWriteTool {
    store: TodoStore,
    session_id: String,
}

impl TodoWriteTool {
    pub fn new(store: TodoStore, session_id: String) -> Self {
        Self { store, session_id }
    }
}

impl AgentTool for TodoWriteTool {
    type Input = TodoWriteInput;
    type Output = String;

    fn name() -> &'static str {
        "todo_write"
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(input) => {
                let in_progress = input
                    .todos
                    .iter()
                    .find(|t| matches!(t.status, TodoStatus::InProgress));
                if let Some(task) = in_progress {
                    task.active_form.clone().into()
                } else {
                    format!("Updating {} todos", input.todos.len()).into()
                }
            }
            Err(_) => "Updating todos".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: Self::Input,
        event_stream: ToolCallEventStream,
        _cx: &mut App,
    ) -> Task<Result<String>> {
        let count = input.todos.len();
        self.store.set_todos(&self.session_id, input.todos.clone());

        let markdown = format_todos_markdown(&input.todos);
        event_stream.update_fields(acp::ToolCallUpdateFields {
            content: Some(vec![markdown.into()]),
            ..Default::default()
        });

        Task::ready(Ok(format!("Updated {} todos.", count)))
    }
}

/// Input for the todo_read tool.
///
/// Retrieves the current task list state to track pending or completed items.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TodoReadInput {}

pub struct TodoReadTool {
    store: TodoStore,
    session_id: String,
}

impl TodoReadTool {
    pub fn new(store: TodoStore, session_id: String) -> Self {
        Self { store, session_id }
    }
}

impl AgentTool for TodoReadTool {
    type Input = TodoReadInput;
    type Output = String;

    fn name() -> &'static str {
        "todo_read"
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Reading todos".into()
    }

    fn run(
        self: Arc<Self>,
        _input: Self::Input,
        event_stream: ToolCallEventStream,
        _cx: &mut App,
    ) -> Task<Result<String>> {
        let todos = self.store.get_todos(&self.session_id);
        let count = todos.len();

        let markdown = format_todos_markdown(&todos);
        event_stream.update_fields(acp::ToolCallUpdateFields {
            content: Some(vec![markdown.into()]),
            ..Default::default()
        });

        let output = serde_json::json!({
            "count": count,
            "todos": todos,
        });

        Task::ready(Ok(serde_json::to_string(&output).unwrap_or_default()))
    }
}

fn format_todos_markdown(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "No todos.".to_string();
    }

    let mut markdown = String::new();
    for (i, todo) in todos.iter().enumerate() {
        let checkbox = match todo.status {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Completed => "[x]",
        };
        markdown.push_str(&format!("{}. {} {}\n", i + 1, checkbox, todo.content));
    }
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todo_store_basic() {
        let store = TodoStore::new();
        let session = "test-session";

        assert!(store.get_todos(session).is_empty());

        let todos = vec![TodoItem {
            content: "Task 1".to_string(),
            status: TodoStatus::Pending,
            active_form: "Working on task 1".to_string(),
        }];
        store.set_todos(session, todos.clone());

        let retrieved = store.get_todos(session);
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].content, "Task 1");
    }

    #[test]
    fn test_todo_store_session_isolation() {
        let store = TodoStore::new();

        store.set_todos(
            "session-a",
            vec![TodoItem {
                content: "Task A".to_string(),
                status: TodoStatus::Pending,
                active_form: "A".to_string(),
            }],
        );

        store.set_todos(
            "session-b",
            vec![TodoItem {
                content: "Task B".to_string(),
                status: TodoStatus::Completed,
                active_form: "B".to_string(),
            }],
        );

        let todos_a = store.get_todos("session-a");
        let todos_b = store.get_todos("session-b");

        assert_eq!(todos_a.len(), 1);
        assert_eq!(todos_a[0].content, "Task A");

        assert_eq!(todos_b.len(), 1);
        assert_eq!(todos_b[0].content, "Task B");
    }

    #[test]
    fn test_todo_store_shared_sessions() {
        let store = TodoStore::new();

        store.share_sessions("executor", "discriminator");

        store.set_todos(
            "executor",
            vec![TodoItem {
                content: "Shared task".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Working".to_string(),
            }],
        );

        let todos_executor = store.get_todos("executor");
        let todos_discriminator = store.get_todos("discriminator");

        assert_eq!(todos_executor.len(), 1);
        assert_eq!(todos_discriminator.len(), 1);
        assert_eq!(todos_executor[0].content, todos_discriminator[0].content);
    }

    #[test]
    fn test_format_todos_markdown() {
        let todos = vec![
            TodoItem {
                content: "First task".to_string(),
                status: TodoStatus::Completed,
                active_form: "First".to_string(),
            },
            TodoItem {
                content: "Second task".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Second".to_string(),
            },
            TodoItem {
                content: "Third task".to_string(),
                status: TodoStatus::Pending,
                active_form: "Third".to_string(),
            },
        ];

        let markdown = format_todos_markdown(&todos);
        assert!(markdown.contains("1. [x] First task"));
        assert!(markdown.contains("2. [~] Second task"));
        assert!(markdown.contains("3. [ ] Third task"));
    }

    #[test]
    fn test_format_todos_empty() {
        let markdown = format_todos_markdown(&[]);
        assert_eq!(markdown, "No todos.");
    }
}

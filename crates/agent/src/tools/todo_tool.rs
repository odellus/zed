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
    /// Brief description of the task
    pub content: String,
    /// Current status of the task: pending, in_progress, completed, cancelled
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
    /// Task no longer needed
    Cancelled,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
            TodoStatus::Cancelled => write!(f, "cancelled"),
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
        let todos = self.todos
            .read()
            .get(&todo_key)
            .cloned()
            .unwrap_or_default();
        log::debug!(
            "TodoStore::get_todos session_id={} todo_key={} count={} shared_keys={:?}",
            session_id,
            todo_key,
            todos.len(),
            self.shared_keys.read().clone()
        );
        todos
    }

    pub fn set_todos(&self, session_id: &str, todos: Vec<TodoItem>) {
        let todo_key = self.get_todo_key(session_id);
        log::debug!(
            "TodoStore::set_todos session_id={} todo_key={} count={} shared_keys={:?}",
            session_id,
            todo_key,
            todos.len(),
            self.shared_keys.read().clone()
        );
        self.todos.write().insert(todo_key, todos);
    }
}

/// Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
/// It also helps the user understand the progress of the task and overall progress of their requests.
///
/// ## When to Use This Tool
/// Use this tool proactively in these scenarios:
///
/// 1. Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
/// 2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
/// 3. User explicitly requests todo list - When the user directly asks you to use the todo list
/// 4. User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
/// 5. After receiving new instructions - Immediately capture user requirements as todos. Feel free to edit the todo list based on new information.
/// 6. After completing a task - Mark it complete and add any new follow-up tasks
/// 7. When you start working on a new task, mark the todo as in_progress. Ideally you should only have one todo as in_progress at a time. Complete existing tasks before starting new ones.
///
/// ## When NOT to Use This Tool
///
/// Skip using this tool when:
/// 1. There is only a single, straightforward task
/// 2. The task is trivial and tracking it provides no organizational benefit
/// 3. The task can be completed in less than 3 trivial steps
/// 4. The task is purely conversational or informational
///
/// NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.
///
/// ## Examples of When to Use the Todo List
///
/// <example>
/// User: I want to add a dark mode toggle to the application settings. Make sure you run the tests and build when you're done!
/// Assistant: I'll help add a dark mode toggle to your application settings. Let me create a todo list to track this implementation.
/// *Creates todo list with the following items:*
/// 1. Create dark mode toggle component in Settings page
/// 2. Add dark mode state management (context/store)
/// 3. Implement CSS-in-JS styles for dark theme
/// 4. Update existing components to support theme switching
/// 5. Run tests and build process, addressing any failures or errors that occur
/// *Begins working on the first task*
///
/// <reasoning>
/// The assistant used the todo list because:
/// 1. Adding dark mode is a multi-step feature requiring UI, state management, and styling changes
/// 2. The user explicitly requested tests and build be run afterward
/// 3. The assistant inferred that tests and build need to pass by adding "Ensure tests and build succeed" as the final task
/// </reasoning>
/// </example>
///
/// <example>
/// User: Help me rename the function getCwd to getCurrentWorkingDirectory across my project
/// Assistant: Let me first search through your codebase to find all occurrences of 'getCwd'.
/// *Uses grep or search tools to locate all instances of getCwd in the codebase*
/// Assistant: I've found 15 instances of 'getCwd' across 8 different files. Let me create a todo list to track these changes.
/// *Creates todo list with specific items for each file that needs updating*
///
/// <reasoning>
/// The assistant used the todo list because:
/// 1. First, the assistant searched to understand the scope of the task
/// 2. Upon finding multiple occurrences across different files, it determined this was a complex task with multiple steps
/// 3. The todo list helps ensure every instance is tracked and updated systematically
/// 4. This approach prevents missing any occurrences and maintains code consistency
/// </reasoning>
/// </example>
///
/// <example>
/// User: I need to implement these features for my e-commerce site: user registration, product catalog, shopping cart, and checkout flow.
/// Assistant: I'll help implement these features. First, let's add all the features to the todo list.
/// *Creates a todo list breaking down each feature into specific tasks based on the project architecture*
/// Assistant: Let's start with user registration. This will involve creating database models, API endpoints, and frontend forms that integrate with your existing authentication system.
///
/// <reasoning>
/// The assistant used the todo list because:
/// 1. The user provided multiple complex features to implement in a comma separated list
/// 2. The todo list helps organize these large features into manageable tasks
/// 3. This approach allows for tracking progress across the entire implementation
/// </reasoning>
/// </example>
///
/// <example>
/// User: Can you help optimize my React application? It's rendering slowly and has performance issues.
/// Assistant: I'll help optimize your React application. First, let me examine your codebase to identify potential performance bottlenecks.
/// *Reviews component structure, render patterns, state management, and data fetching*
/// Assistant: After analyzing your codebase, I've identified several performance issues. Let me create a todo list to track our optimization efforts.
/// *Creates todo list with items like: 1) Implement memoization for expensive calculations in ProductList, 2) Add virtualization for long lists in Dashboard, 3) Optimize image loading in Gallery component, 4) Fix state update loops in ShoppingCart, 5) Review bundle size and implement code splitting*
/// Let's start by implementing memoization for the expensive calculations in your ProductList component.
///
/// <reasoning>
/// The assistant used the todo list because:
/// 1. First, the assistant examined the codebase to identify specific performance issues
/// 2. Based on this analysis, it identified multiple optimization opportunities
/// 3. Performance optimization is a non-trivial task requiring multiple steps
/// 4. The todo list helps methodically track improvements across different components
/// 5. This systematic approach ensures all performance bottlenecks are addressed
/// </reasoning>
/// </example>
///
/// ## Examples of When NOT to Use the Todo List
///
/// <example>
/// User: How do I print 'Hello World' in Python?
/// Assistant: In Python, you can print "Hello World" with this simple code:
///
/// python
/// print("Hello World")
///
/// This will output the text "Hello World" to the console when executed.
///
/// <reasoning>
/// The assistant did not use the todo list because this is a single, trivial task that can be completed in one step. There's no need to track multiple tasks or steps for such a straightforward request.
/// </reasoning>
/// </example>
///
/// <example>
/// User: What does the git status command do?
/// Assistant: The git status command shows the current state of your working directory and staging area. It displays which changes have been staged, which haven't, and which files aren't being tracked by Git. This is a fundamental command for understanding the current state of your repository before making commits.
///
/// <reasoning>
/// The assistant did not use the todo list because this is an informational request with no actual coding task to complete. The user is simply asking for an explanation, not for the assistant to perform multiple steps or tasks.
/// </reasoning>
/// </example>
///
/// <example>
/// User: Can you add a comment to the calculateTotal function to explain what it does?
/// Assistant: Sure, let me add a comment to the calculateTotal function to explain what it does.
/// * Uses the Edit tool to add a comment to the calculateTotal function *
///
/// <reasoning>
/// The assistant did not use the todo list because this is a single, straightforward task confined to one location in the code. Adding a comment doesn't require tracking multiple steps or systematic organization.
/// </reasoning>
/// </example>
///
/// <example>
/// User: Run npm install for me and tell me what happens.
/// Assistant: I'll run the npm install command for you.
///
/// *Executes: npm install*
///
/// The command completed successfully. Here's the output:
/// [Output of npm install command]
///
/// All dependencies have been installed according to your package.json file.
///
/// <reasoning>
/// The assistant did not use the todo list because this is a single command execution with immediate results. There are no multiple steps to track or organize, making the todo list unnecessary for this straightforward task.
/// </reasoning>
/// </example>
///
/// ## Task States and Management
///
/// 1. **Task States**: Use these states to track progress:
///    - pending: Task not yet started
///    - in_progress: Currently working on (limit to ONE task at a time)
///    - completed: Task finished successfully
///    - cancelled: Task no longer needed
///
/// 2. **Task Management**:
///    - Update task status in real-time as you work
///    - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)
///    - Only have ONE task in_progress at any time
///    - Complete current tasks before starting new ones
///    - Cancel tasks that become irrelevant
///
/// 3. **Task Breakdown**:
///    - Create specific, actionable items
///    - Break complex tasks into smaller, manageable steps
///    - Use clear, descriptive task names
///
/// When in doubt, use this tool. Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully.
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
                    let remaining = input
                        .todos
                        .iter()
                        .filter(|t| !matches!(t.status, TodoStatus::Completed | TodoStatus::Cancelled))
                        .count();
                    format!("{} todos", remaining).into()
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

        // Emit ACP Plan update for native visualization
        let plan_entries: Vec<acp::PlanEntry> = input
            .todos
            .iter()
            .map(|todo| acp::PlanEntry {
                content: todo.content.clone(),
                priority: acp::PlanEntryPriority::Medium,
                status: match todo.status {
                    TodoStatus::Pending => acp::PlanEntryStatus::Pending,
                    TodoStatus::InProgress => acp::PlanEntryStatus::InProgress,
                    TodoStatus::Completed => acp::PlanEntryStatus::Completed,
                    // ACP doesn't have Cancelled, map to Completed like OpenCode does
                    TodoStatus::Cancelled => acp::PlanEntryStatus::Completed,
                },
                meta: None,
            })
            .collect();
        event_stream.update_plan(acp::Plan {
            entries: plan_entries,
            meta: None,
        });

        Task::ready(Ok(format!("Updated {} todos.", count)))
    }
}

/// Action for the todo_read tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoReadAction {
    /// Show the current todo list
    Show,
}

impl Default for TodoReadAction {
    fn default() -> Self {
        Self::Show
    }
}

/// Input for the todo_read tool.
///
/// Retrieves the current task list state to track pending or completed items.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TodoReadInput {
    /// Action to perform (currently only "show" is supported)
    #[serde(default)]
    pub action: TodoReadAction,
}

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
            TodoStatus::Cancelled => "[-]",
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
                active_form: "Working on A".to_string(),
            }],
        );

        store.set_todos(
            "session-b",
            vec![TodoItem {
                content: "Task B".to_string(),
                status: TodoStatus::Completed,
                active_form: "Working on B".to_string(),
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
                active_form: "Working on shared task".to_string(),
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
                active_form: "Completing first task".to_string(),
            },
            TodoItem {
                content: "Second task".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Working on second task".to_string(),
            },
            TodoItem {
                content: "Third task".to_string(),
                status: TodoStatus::Pending,
                active_form: "Starting third task".to_string(),
            },
            TodoItem {
                content: "Fourth task".to_string(),
                status: TodoStatus::Cancelled,
                active_form: "Cancelling fourth task".to_string(),
            },
        ];

        let markdown = format_todos_markdown(&todos);
        assert!(markdown.contains("1. [x] First task"));
        assert!(markdown.contains("2. [~] Second task"));
        assert!(markdown.contains("3. [ ] Third task"));
        assert!(markdown.contains("4. [-] Fourth task"));
    }

    #[test]
    fn test_format_todos_empty() {
        let markdown = format_todos_markdown(&[]);
        assert_eq!(markdown, "No todos.");
    }
}

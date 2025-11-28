use std::sync::Arc;

use agent_client_protocol as acp;
use anyhow::Result;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream};

/// Signal that the executor's work is complete and correct.
/// Call this tool when you have reviewed the executor's work and determined it satisfies the user's request.
/// The summary will be shown to the user as the final response.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskCompleteToolInput {
    /// A summary of what was accomplished for the user.
    pub summary: String,
}

pub struct TaskCompleteTool;

impl AgentTool for TaskCompleteTool {
    type Input = TaskCompleteToolInput;
    type Output = String;

    fn name() -> &'static str {
        "task_complete"
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Task complete".into()
    }

    fn run(
        self: Arc<Self>,
        input: Self::Input,
        _event_stream: ToolCallEventStream,
        _cx: &mut App,
    ) -> Task<Result<String>> {
        // The orchestrator will intercept this tool call and break the loop.
        // The summary is returned as the tool result.
        Task::ready(Ok(input.summary))
    }
}

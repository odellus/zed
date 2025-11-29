//! Dual-agent orchestration: executor ↔ discriminator pattern.
//!
//! Two Threads/Sessions work together:
//! - Executor: does the actual work (edits, tool calls, etc.)
//! - Discriminator: reviews executor's work, calls task_complete when satisfied
//!
//! Both stream their events to the same AcpThread for unified UI display.

use crate::{AgentMessageContent, NativeAgentConnection, Thread, ThreadEvent, UserMessageContent};

/// Strip markdown role headers like "## User\n\n" or "## Assistant\n\n" from content.
/// Used when role-flipping messages for the discriminator to avoid confusion.
pub fn strip_role_header(markdown: &str) -> String {
    if let Some(rest) = markdown.strip_prefix("## User\n\n") {
        rest.to_string()
    } else if let Some(rest) = markdown.strip_prefix("## Assistant\n\n") {
        rest.to_string()
    } else {
        markdown.to_string()
    }
}
use acp_thread::{AcpThread, UserMessageId};
use agent_client_protocol as acp;
use anyhow::{Result, anyhow};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{App, AsyncApp, Entity, Task, WeakEntity};

/// The name of the task_complete tool that signals the discriminator is satisfied.
pub const TASK_COMPLETE_TOOL: &str = "task_complete";

/// Maximum number of executor↔discriminator iterations before giving up.
/// This prevents infinite loops if the discriminator never calls task_complete.
pub const MAX_DUAL_AGENT_ITERATIONS: u32 = 10;

/// Orchestrates dual-agent execution.
///
/// When the user sends a message in dual-agent mode:
/// 1. Executor runs its turn
/// 2. Executor's output is exported to markdown and sent to discriminator (role-flipped)
/// 3. Discriminator reviews and either:
///    a. Calls task_complete → loop ends, return to user
///    b. Provides feedback → sent back to executor, loop continues
pub struct DualAgentOrchestrator {
    /// The executor session (does the actual work)
    executor_session_id: acp::SessionId,
    /// The discriminator session (reviews executor's work)
    discriminator_session_id: acp::SessionId,
    /// Reference to the native agent that owns both sessions
    connection: NativeAgentConnection,
    /// Shared AcpThread for both agents to stream to
    acp_thread: WeakEntity<AcpThread>,
}

impl DualAgentOrchestrator {
    pub fn new(
        executor_session_id: acp::SessionId,
        discriminator_session_id: acp::SessionId,
        connection: NativeAgentConnection,
        acp_thread: WeakEntity<AcpThread>,
    ) -> Self {
        Self {
            executor_session_id,
            discriminator_session_id,
            connection,
            acp_thread,
        }
    }

    /// Run the dual-agent loop.
    ///
    /// Returns when discriminator calls task_complete or an error occurs.
    pub fn run(
        self,
        initial_message: Vec<UserMessageContent>,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        let executor_thread = self
            .connection
            .thread(&self.executor_session_id, cx)
            .ok_or_else(|| anyhow!("Executor session not found"));
        let discriminator_thread = self
            .connection
            .thread(&self.discriminator_session_id, cx)
            .ok_or_else(|| anyhow!("Discriminator session not found"));

        let (executor_thread, discriminator_thread) = match (executor_thread, discriminator_thread)
        {
            (Ok(e), Ok(d)) => (e, d),
            (Err(e), _) | (_, Err(e)) => return Task::ready(Err(e)),
        };

        let acp_thread = self.acp_thread.clone();

        cx.spawn(async move |cx| {
            Self::run_loop(
                executor_thread,
                discriminator_thread,
                acp_thread,
                initial_message,
                cx,
            )
            .await
        })
    }

    async fn run_loop(
        executor_thread: Entity<Thread>,
        discriminator_thread: Entity<Thread>,
        acp_thread: WeakEntity<AcpThread>,
        initial_message: Vec<UserMessageContent>,
        cx: &mut AsyncApp,
    ) -> Result<acp::PromptResponse> {
        // On first call, add the initial request as Agent message in discriminator.
        // This represents "what the user asked for" before we show executor's output.
        // Discriminator sees: User("How can I help?") -> Agent(<initial request>) -> User(<executor output>)
        cx.update(|cx| {
            discriminator_thread.update(cx, |thread, _cx| {
                let request_text: String = initial_message
                    .iter()
                    .filter_map(|c| match c {
                        UserMessageContent::Text(t) => Some(t.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                thread.push_agent_message(vec![AgentMessageContent::Text(request_text)]);
            })
        })?;

        let mut current_input = initial_message;
        let mut iteration = 0;

        loop {
            iteration += 1;
            if iteration > MAX_DUAL_AGENT_ITERATIONS {
                log::warn!(
                    "Dual-agent loop exceeded max iterations ({}), stopping",
                    MAX_DUAL_AGENT_ITERATIONS
                );
                return Ok(acp::PromptResponse {
                    stop_reason: acp::StopReason::EndTurn,
                    meta: None,
                });
            }

            log::debug!(
                "Dual-agent iteration {}/{}",
                iteration,
                MAX_DUAL_AGENT_ITERATIONS
            );
            // 1. Run executor turn with current input
            log::debug!("Dual-agent: Running executor turn");
            let executor_events = cx.update(|cx| {
                executor_thread.update(cx, |thread, cx| {
                    let id = UserMessageId::new();
                    thread.send(id, current_input.clone(), cx)
                })
            })??;

            // Stream executor events to the shared AcpThread
            Self::forward_events_to_acp_thread(executor_events, acp_thread.clone(), cx).await?;

            // 2. Export executor's last turn to markdown
            let executor_output = cx.update(|cx| executor_thread.read(cx).export_last_turn())?;

            log::debug!(
                "Dual-agent: Executor turn complete, output length: {}",
                executor_output.len()
            );

            // 3. Send executor output to discriminator as USER message (role flip)
            log::debug!("Dual-agent: Running discriminator turn");
            let discriminator_events = cx.update(|cx| {
                discriminator_thread.update(cx, |thread, cx| {
                    let id = UserMessageId::new();
                    let content = vec![UserMessageContent::Text(executor_output)];
                    thread.send(id, content, cx)
                })
            })??;

            // 4. Stream discriminator events, watching for task_complete
            let result = Self::forward_events_watching_for_complete(
                discriminator_events,
                acp_thread.clone(),
                cx,
            )
            .await?;

            match result {
                LoopResult::TaskComplete => {
                    log::debug!("Dual-agent: task_complete called, ending loop");
                    return Ok(acp::PromptResponse {
                        stop_reason: acp::StopReason::EndTurn,
                        meta: None,
                    });
                }
                LoopResult::Continue => {
                    // 5. Export discriminator feedback and loop back to executor
                    let feedback =
                        cx.update(|cx| discriminator_thread.read(cx).export_last_turn())?;

                    log::debug!(
                        "Dual-agent: Discriminator gave feedback, length: {}",
                        feedback.len()
                    );

                    current_input = vec![UserMessageContent::Text(feedback)];
                }
            }
        }
    }

    /// Forward events from a Thread to the AcpThread.
    async fn forward_events_to_acp_thread(
        mut events: mpsc::UnboundedReceiver<Result<ThreadEvent>>,
        acp_thread: WeakEntity<AcpThread>,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        while let Some(result) = events.next().await {
            match result {
                Ok(event) => {
                    Self::handle_event(event, &acp_thread, cx)?;
                }
                Err(e) => {
                    log::error!("Error in thread event stream: {:?}", e);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Forward events, but watch for task_complete tool call.
    /// Returns immediately when task_complete is detected.
    async fn forward_events_watching_for_complete(
        mut events: mpsc::UnboundedReceiver<Result<ThreadEvent>>,
        acp_thread: WeakEntity<AcpThread>,
        cx: &mut AsyncApp,
    ) -> Result<LoopResult> {
        while let Some(result) = events.next().await {
            match result {
                Ok(event) => {
                    // Check for task_complete tool call BEFORE forwarding
                    if let ThreadEvent::ToolCall(ref tool_call) = event {
                        // Tool name is stored in meta.tool_name
                        let is_task_complete = tool_call
                            .meta
                            .as_ref()
                            .and_then(|m| m.get("tool_name"))
                            .and_then(|v| v.as_str())
                            .map(|name| name == TASK_COMPLETE_TOOL)
                            .unwrap_or(false);

                        if is_task_complete {
                            // Forward this last event so UI shows the tool call
                            Self::handle_event(event, &acp_thread, cx)?;
                            // Immediately break - don't wait for the tool to "run"
                            return Ok(LoopResult::TaskComplete);
                        }
                    }

                    // Check for Stop event
                    if matches!(event, ThreadEvent::Stop(_)) {
                        Self::handle_event(event, &acp_thread, cx)?;
                        return Ok(LoopResult::Continue);
                    }

                    Self::handle_event(event, &acp_thread, cx)?;
                }
                Err(e) => {
                    log::error!("Error in discriminator event stream: {:?}", e);
                    return Err(e);
                }
            }
        }
        Ok(LoopResult::Continue)
    }

    /// Handle a single ThreadEvent by forwarding it to AcpThread.
    fn handle_event(
        event: ThreadEvent,
        acp_thread: &WeakEntity<AcpThread>,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        match event {
            ThreadEvent::UserMessage(message) => {
                acp_thread.update(cx, |thread, cx| {
                    for content in message.content {
                        thread.push_user_content_block(
                            Some(message.id.clone()),
                            content.into(),
                            cx,
                        );
                    }
                })?;
            }
            ThreadEvent::AgentText(text) => {
                acp_thread.update(cx, |thread, cx| {
                    thread.push_assistant_content_block(
                        acp::ContentBlock::Text(acp::TextContent {
                            text,
                            annotations: None,
                            meta: None,
                        }),
                        false,
                        cx,
                    )
                })?;
            }
            ThreadEvent::AgentThinking(text) => {
                acp_thread.update(cx, |thread, cx| {
                    thread.push_assistant_content_block(
                        acp::ContentBlock::Text(acp::TextContent {
                            text,
                            annotations: None,
                            meta: None,
                        }),
                        true,
                        cx,
                    )
                })?;
            }
            ThreadEvent::ToolCallAuthorization(_) => {
                // TODO: Handle authorization in dual-agent mode
                // For now, tools should be auto-approved in dual mode
            }
            ThreadEvent::ToolCall(tool_call) => {
                acp_thread.update(cx, |thread, cx| thread.upsert_tool_call(tool_call, cx))??;
            }
            ThreadEvent::ToolCallUpdate(update) => {
                acp_thread.update(cx, |thread, cx| thread.update_tool_call(update, cx))??;
            }
            ThreadEvent::Retry(status) => {
                acp_thread.update(cx, |thread, cx| thread.update_retry_status(status, cx))?;
            }
            ThreadEvent::Stop(_) => {
                // Don't forward stop - we control when the loop ends
            }
        }
        Ok(())
    }
}

enum LoopResult {
    TaskComplete,
    Continue,
}

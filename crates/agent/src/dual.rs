//! Dual-agent orchestration: executor ↔ discriminator pattern.
//!
//! Two Threads/Sessions work together:
//! - Executor: does the actual work (edits, tool calls, etc.)
//! - Discriminator: reviews executor's work, calls task_complete when satisfied
//!
//! Both stream their events to the same AcpThread for unified UI display.
//!
//! ## Summarization Checkpoints
//!
//! Instead of passing entire conversation histories between agents (which blows up
//! context limits), we use summarization checkpoints:
//!
//! 1. Executor finishes react loop
//! 2. We inject a "summarize your work" prompt to executor
//! 3. Executor's summary is sent to discriminator
//! 4. Discriminator reviews and either calls task_complete or provides feedback
//! 5. If feedback, we inject "summarize your feedback" prompt to discriminator
//! 6. Discriminator's feedback summary is sent back to executor
//! 7. Repeat until task_complete or max iterations

use crate::{NativeAgentConnection, Thread, ThreadEvent, UserMessageContent};
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

/// Prompt injected into executor session after react loop completes.
/// Asks executor to summarize its work concisely for discriminator review.
pub const EXECUTOR_SUMMARY_PROMPT: &str = r#"Please provide a concise summary of your work since my last message. Include:

1. **Goal**: What you were trying to accomplish
2. **Approach**: How you approached the problem
3. **Changes Made**: Which files you created, edited, or deleted
4. **Testing**: Location and how to run any tests you created or modified
5. **Running**: Location and how to run anything you built that can be executed

Do NOT call any tools. Reply directly with the summary in markdown format."#;

/// Prompt injected into discriminator session after it provides feedback.
/// Asks discriminator to summarize its feedback concisely for executor.
pub const DISCRIMINATOR_FEEDBACK_PROMPT: &str = r#"Please provide a concise summary of your feedback on the executor's work. Include:

1. **Assessment**: Is the work complete, incomplete, or incorrect?
2. **Issues Found**: Specific problems that need to be addressed
3. **Suggestions**: Concrete steps the executor should take to fix the issues
4. **Priority**: Which issues are most critical to address first

Do NOT call any tools. Reply directly with the feedback summary in markdown format."#;

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
        // Setup discriminator's initial context:
        // The discriminator sees the user's task as ITS OWN task (from assistant perspective)
        // So: User("How can I help?") -> Assistant(<the actual user request>)
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
                thread.push_agent_message(vec![crate::AgentMessageContent::Text(request_text)]);
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

            // === STEP 1: Run executor with current input (user request or discriminator feedback) ===
            log::debug!("Dual-agent: Running executor turn");
            let executor_events = cx.update(|cx| {
                executor_thread.update(cx, |thread, cx| {
                    let id = UserMessageId::new();
                    thread.send(id, current_input.clone(), cx)
                })
            })??;

            // Stream executor's react loop to UI
            Self::forward_events_to_acp_thread(executor_events, acp_thread.clone(), cx).await?;
            log::debug!("Dual-agent: Executor react loop complete");

            // === STEP 2: Ask executor to summarize its work ===
            log::debug!("Dual-agent: Requesting executor summary");
            let executor_summary =
                Self::request_summary(&executor_thread, EXECUTOR_SUMMARY_PROMPT, &acp_thread, cx)
                    .await?;

            log::debug!(
                "Dual-agent: Executor summary length: {}",
                executor_summary.len()
            );

            // === STEP 3: Send summary to discriminator as USER message ===
            // Discriminator sees executor's summary as if a user is reporting work done
            log::debug!("Dual-agent: Running discriminator review");
            let discriminator_events = cx.update(|cx| {
                discriminator_thread.update(cx, |thread, cx| {
                    let id = UserMessageId::new();
                    let content = vec![UserMessageContent::Text(executor_summary)];
                    thread.send(id, content, cx)
                })
            })??;

            // === STEP 4: Stream discriminator events, watch for task_complete ===
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
                    // === STEP 5: Ask discriminator to summarize its feedback ===
                    log::debug!("Dual-agent: Requesting discriminator feedback summary");
                    let feedback_summary = Self::request_summary(
                        &discriminator_thread,
                        DISCRIMINATOR_FEEDBACK_PROMPT,
                        &acp_thread,
                        cx,
                    )
                    .await?;

                    log::debug!(
                        "Dual-agent: Discriminator feedback length: {}",
                        feedback_summary.len()
                    );

                    // === STEP 6: Send feedback back to executor for next iteration ===
                    current_input = vec![UserMessageContent::Text(feedback_summary)];
                }
            }
        }
    }

    /// Send a summary request to a thread and collect the text response.
    ///
    /// This injects a user message asking for a summary, runs the agent's response,
    /// and extracts the text content (ignoring any tool calls).
    async fn request_summary(
        thread: &Entity<Thread>,
        prompt: &str,
        acp_thread: &WeakEntity<AcpThread>,
        cx: &mut AsyncApp,
    ) -> Result<String> {
        // Send the summary request
        let mut events = cx.update(|cx| {
            thread.update(cx, |thread, cx| {
                let id = UserMessageId::new();
                let content = vec![UserMessageContent::Text(prompt.to_string())];
                thread.send(id, content, cx)
            })
        })??;

        // Collect text from the response
        let mut summary_text = String::new();

        // We don't forward summary request events to the UI - these are internal
        // Just collect the text response
        while let Some(result) = events.next().await {
            match result {
                Ok(ThreadEvent::AgentText(text)) => {
                    summary_text.push_str(&text);
                }
                Ok(ThreadEvent::Stop(_)) => {
                    break;
                }
                Ok(ThreadEvent::ToolCall(_)) => {
                    // Agent tried to call a tool despite being asked not to
                    // Log warning but continue collecting text
                    log::warn!("Dual-agent: Agent called tool during summary request (ignoring)");
                }
                Ok(_) => {
                    // Ignore other events (thinking, tool updates, etc.)
                }
                Err(e) => {
                    log::error!("Error during summary request: {:?}", e);
                    return Err(e);
                }
            }
        }

        // Forward a marker to UI so user knows summarization happened
        acp_thread.update(cx, |thread, cx| {
            thread.push_assistant_content_block(
                acp::ContentBlock::Text(acp::TextContent {
                    text: format!("\n📋 *Summary checkpoint*\n"),
                    annotations: None,
                    meta: None,
                }),
                false, // not thinking
                cx,
            )
        })?;

        Ok(summary_text)
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

use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use gpui::AsyncApp;

use crate::init;
use crate::render::{OutputMode, TerminalRenderer};

/// Run a single chat message and stream the response
pub async fn run_chat_command(
    message: String,
    _new_session: bool,
    _session_id: Option<String>,
    output_mode: OutputMode,
    cx: &mut AsyncApp,
) -> Result<()> {
    log::info!("Starting chat command with message: {}", message);

    // Initialize the agent
    let crow = init::initialize(cx).await?;

    // Create a new thread via the NativeAgentConnection
    let acp_thread = cx
        .update(|cx| crow.new_thread(cx))?
        .await
        .context("Failed to create thread")?;

    log::info!("Thread created, sending message...");

    // Send the prompt and wait for completion
    let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent {
        text: message,
        annotations: None,
        meta: None,
    })];

    // Send returns a future that completes when the agent is done
    let send_future = acp_thread.update(cx, |thread, cx| thread.send(prompt_blocks, cx))?;

    // Wait for the agent to finish processing
    send_future.await?;

    log::info!("Agent finished processing");

    // Create the terminal renderer
    let mut renderer = TerminalRenderer::new(output_mode);

    // Read the final response from the thread
    let response = acp_thread.read_with(cx, |thread, cx| {
        // Get all entries and format them
        let mut output = String::new();
        for entry in thread.entries() {
            match entry {
                acp_thread::AgentThreadEntry::AssistantMessage(msg) => {
                    output.push_str(&msg.to_markdown(cx));
                    output.push('\n');
                }
                acp_thread::AgentThreadEntry::ToolCall(_tool_call) => {
                    // Skip tool calls in output for now - they're internal
                }
                acp_thread::AgentThreadEntry::UserMessage(_) => {
                    // Skip user messages in output
                }
            }
        }
        output
    })?;

    renderer.render_text(&response);
    renderer.finish();

    Ok(())
}

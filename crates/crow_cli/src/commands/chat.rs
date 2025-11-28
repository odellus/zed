use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use colored::Colorize;
use gpui::AsyncApp;
use std::io::{self, Write};
use std::time::Instant;

use crate::init;
use crate::render::OutputMode;

// Brand colors matching original crow-cli
fn purple_bold(s: &str) -> colored::ColoredString {
    s.truecolor(138, 43, 226).bold()
}

fn light_purple(s: &str) -> colored::ColoredString {
    s.truecolor(180, 130, 255)
}

fn mint_green(s: &str) -> colored::ColoredString {
    s.truecolor(0, 255, 170)
}

fn soft_green(s: &str) -> colored::ColoredString {
    s.truecolor(130, 220, 130)
}

fn lime_green(s: &str) -> colored::ColoredString {
    s.truecolor(180, 255, 100)
}

/// Run a single chat message and stream the response
pub async fn run_chat_command(
    message: String,
    _new_session: bool,
    _session_id: Option<String>,
    output_mode: OutputMode,
    cx: &mut AsyncApp,
) -> Result<()> {
    let start_time = Instant::now();

    log::info!("Starting chat command with message: {}", message);

    // Initialize the agent
    let crow = init::initialize(cx).await?;

    // Create a new thread via the NativeAgentConnection
    let acp_thread = cx
        .update(|cx| crow.new_thread(cx))?
        .await
        .context("Failed to create thread")?;

    log::info!("Thread created, sending message...");

    // Show header and user message (unless quiet/json mode)
    if output_mode == OutputMode::Verbose {
        eprintln!();
        eprintln!(
            "{}",
            "═══════════════════════════════════════════════════════════════".dimmed()
        );
        eprintln!("{}", purple_bold("CROW-CLI"));
        eprintln!(
            "{}",
            "═══════════════════════════════════════════════════════════════".dimmed()
        );
        eprintln!();

        // Show user message
        eprintln!("{}", "▶ USER".white().bold());
        eprintln!("{}", message.white());
        eprintln!();
    }

    // Send the prompt and wait for completion
    let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent {
        text: message,
        annotations: None,
        meta: None,
    })];

    let send_future = acp_thread.update(cx, |thread, cx| thread.send(prompt_blocks, cx))?;

    // Wait for completion
    send_future.await?;

    let elapsed = start_time.elapsed();
    log::info!("Agent finished processing in {:.1}s", elapsed.as_secs_f64());

    // Render all entries
    let mut tool_count = 0;
    cx.update(|cx| {
        let thread = acp_thread.read(cx);
        for entry in thread.entries() {
            render_entry(entry, output_mode, &mut tool_count, cx);
        }
    })?;

    // Show footer with stats
    if output_mode == OutputMode::Verbose {
        eprintln!();
        eprintln!(
            "{}",
            "═══════════════════════════════════════════════════════════════".dimmed()
        );
        eprintln!(
            "{} {} tool calls | {:.1}s",
            mint_green("✓").bold(),
            mint_green(&tool_count.to_string()),
            elapsed.as_secs_f64()
        );
        eprintln!(
            "{}",
            "═══════════════════════════════════════════════════════════════".dimmed()
        );
    }

    Ok(())
}

/// Render a single thread entry to stdout/stderr
fn render_entry(
    entry: &acp_thread::AgentThreadEntry,
    output_mode: OutputMode,
    tool_count: &mut usize,
    cx: &gpui::App,
) {
    match entry {
        acp_thread::AgentThreadEntry::UserMessage(_) => {
            // Already shown in header
        }
        acp_thread::AgentThreadEntry::AssistantMessage(msg) => {
            let content = msg.to_markdown(cx);

            if output_mode == OutputMode::Verbose {
                eprintln!("{}", lime_green("◀ ASSISTANT").bold());
                // Print the response content
                for line in content.lines() {
                    println!("{}", lime_green(line));
                }
                io::stdout().flush().ok();
            } else if output_mode == OutputMode::Quiet {
                print!("{}", content);
                io::stdout().flush().ok();
            }
        }
        acp_thread::AgentThreadEntry::ToolCall(tool_call) => {
            *tool_count += 1;
            if output_mode == OutputMode::Verbose {
                render_tool_call(tool_call, cx);
            }
        }
    }
}

/// Render a tool call with full details
fn render_tool_call(tool_call: &acp_thread::ToolCall, cx: &gpui::App) {
    let status = &tool_call.status;

    eprintln!();

    // Tool header - get label text from the markdown entity
    let label_text = tool_call.label.read(cx).source().to_string();

    let status_icon = match status {
        acp_thread::ToolCallStatus::Pending => "⏳",
        acp_thread::ToolCallStatus::WaitingForConfirmation { .. } => "❓",
        acp_thread::ToolCallStatus::InProgress => "🔄",
        acp_thread::ToolCallStatus::Completed => "✅",
        acp_thread::ToolCallStatus::Failed => "❌",
        acp_thread::ToolCallStatus::Rejected => "🚫",
        acp_thread::ToolCallStatus::Canceled => "⚪",
    };

    eprintln!(
        "{} {} {}",
        status_icon,
        purple_bold(&label_text),
        format!("({})", tool_call.id.0).dimmed()
    );

    // Show raw input if available - full JSON
    if let Some(input) = &tool_call.raw_input {
        eprintln!("   {}", "→ Input:".dimmed());
        if let Ok(pretty) = serde_json::to_string_pretty(input) {
            for line in pretty.lines() {
                eprintln!("   {}", line.cyan());
            }
        }
    }

    // Show raw output if available - full JSON
    if let Some(output) = &tool_call.raw_output {
        eprintln!("   {}", "← Output:".dimmed());
        if let Ok(pretty) = serde_json::to_string_pretty(output) {
            for line in pretty.lines() {
                eprintln!("   {}", soft_green(line));
            }
        } else if let Some(s) = output.as_str() {
            for line in s.lines() {
                eprintln!("   {}", soft_green(line));
            }
        }
    }

    // Show error for failed status
    if matches!(status, acp_thread::ToolCallStatus::Failed) {
        eprintln!("   {}", "Tool call failed".red());
    }
}

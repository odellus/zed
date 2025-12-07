use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use colored::Colorize;
use futures::channel::oneshot;
use futures::future::FutureExt;
use gpui::AsyncApp;
use std::io::{self, Write};
use std::time::Instant;

use crate::init::{self, CrowContext};
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
    new_session: bool,
    session_id: Option<String>,
    output_mode: OutputMode,
    auto_mode: bool,
    cx: &mut AsyncApp,
) -> Result<()> {
    let start_time = Instant::now();

    log::info!("Starting chat command with message: {}", message);

    // Initialize the agent
    let crow = init::initialize(cx).await?;

    // Either resume an existing session or create a new one
    let (acp_thread, is_resumed) = if let Some(session_id_str) = session_id {
        if new_session {
            anyhow::bail!("Cannot use --new and --session together");
        }
        // Resume existing session
        let session_id = agent_client_protocol::SessionId(session_id_str.into());
        log::info!("Resuming session: {}", session_id.0);

        let thread = cx
            .update(|cx| crow.connection.open_thread(session_id.clone(), cx))?
            .await
            .context(format!("Failed to open session: {}", session_id.0))?;

        (thread, true)
    } else {
        // Create a new thread
        let thread = cx
            .update(|cx| crow.new_thread(cx))?
            .await
            .context("Failed to create thread")?;
        (thread, false)
    };

    log::info!(
        "Thread {} - sending message...",
        if is_resumed { "resumed" } else { "created" }
    );

    // Get session info for display
    let session_id = cx.update(|cx| acp_thread.read(cx).session_id().clone())?;

    // Show header and user message (unless quiet/json mode)
    if output_mode == OutputMode::Verbose {
        eprintln!();
        eprintln!(
            "{}",
            "═══════════════════════════════════════════════════════════════".dimmed()
        );
        let mode_str = if auto_mode { " (AUTO)" } else { "" };
        let resumed_str = if is_resumed { " [resumed]" } else { "" };
        eprintln!(
            "{}{}",
            purple_bold(&format!("CROW-CLI{}", mode_str)),
            resumed_str.yellow()
        );
        eprintln!("{}", format!("Session: {}", session_id.0).dimmed());
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

    // If auto mode, enable dual-agent BEFORE sending the message
    // This creates the discriminator, backfills history, and ensures
    // the message goes through the DualAgentOrchestrator
    if auto_mode {
        if output_mode == OutputMode::Verbose {
            eprintln!(
                "{}",
                "🔄 Enabling auto mode (dual-agent)...".yellow().bold()
            );
        }

        cx.update(|cx| {
            crow.agent.update(cx, |agent, cx| {
                agent.enable_dual_agent_mode(session_id.clone(), cx)
            })
        })??;

        if output_mode == OutputMode::Verbose {
            eprintln!(
                "{}",
                "✓ Auto mode enabled. Message will go through executor↔discriminator loop.".green()
            );
            eprintln!();
        }
    }

    // Send the prompt and wait for completion
    // If auto mode is enabled, this goes through the DualAgentOrchestrator
    let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent {
        text: message,
        annotations: None,
        meta: None,
    })];

    let send_future = acp_thread.update(cx, |thread, cx| thread.send(prompt_blocks, cx))?;

    // Set up Ctrl+C handler for graceful cancellation
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let cancel_tx = std::sync::Mutex::new(Some(cancel_tx));

    ctrlc::set_handler(move || {
        if let Some(tx) = cancel_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    })
    .ok();

    // Race between completion and Ctrl+C
    let mut send_future = send_future.fuse();
    let mut cancel_rx = cancel_rx.fuse();

    let result = futures::select! {
        result = send_future => result,
        _ = cancel_rx => {
            // User pressed Ctrl+C - cancel the thread
            if output_mode == OutputMode::Verbose {
                eprintln!("\n{}", "Cancelling...".yellow().bold());
            }
            cx.update(|cx| acp_thread.update(cx, |thread, cx| thread.cancel(cx)))?.await;
            if output_mode == OutputMode::Verbose {
                eprintln!("{}", "Cancelled by user.".yellow());
            }
            Ok(())
        }
    };

    // Clear the Ctrl+C handler
    let _ = ctrlc::set_handler(|| {});

    result?;

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
    let total_elapsed = start_time.elapsed();
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
            total_elapsed.as_secs_f64()
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

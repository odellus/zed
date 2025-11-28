use agent_client_protocol as acp;
use colored::Colorize;
use std::io::{self, Write};

use super::OutputMode;

/// Terminal renderer for agent output
pub struct TerminalRenderer {
    mode: OutputMode,
    current_tool: Option<String>,
    tool_depth: usize,
}

impl TerminalRenderer {
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            current_tool: None,
            tool_depth: 0,
        }
    }

    /// Render streaming text from the agent
    pub fn render_text(&mut self, text: &str) {
        match self.mode {
            OutputMode::Verbose => {
                print!("{}", text);
                io::stdout().flush().ok();
            }
            OutputMode::Quiet => {
                // Buffer until complete
                print!("{}", text);
                io::stdout().flush().ok();
            }
            OutputMode::Json => {
                // JSON mode buffers everything
            }
        }
    }

    /// Render thinking text (extended thinking / chain of thought)
    pub fn render_thinking(&mut self, text: &str) {
        if self.mode == OutputMode::Verbose {
            print!("{}", text.dimmed());
            io::stdout().flush().ok();
        }
    }

    /// Render a tool call starting
    pub fn render_tool_call(&mut self, tool_call: &acp::ToolCall) {
        if self.mode != OutputMode::Verbose {
            return;
        }

        self.tool_depth += 1;
        self.current_tool = Some(tool_call.title.clone());

        let prefix = "  ".repeat(self.tool_depth.saturating_sub(1));
        eprintln!(
            "{}{}",
            prefix,
            format!("▶ {}", tool_call.title).cyan().bold()
        );
    }

    /// Render a tool call update
    pub fn render_tool_update(&mut self, update: &acp::ToolCallUpdate) {
        if self.mode != OutputMode::Verbose {
            return;
        }

        let prefix = "  ".repeat(self.tool_depth.saturating_sub(1));

        // Render status changes
        if let Some(status) = &update.fields.status {
            match status {
                acp::ToolCallStatus::Completed => {
                    if let Some(tool_name) = &self.current_tool {
                        eprintln!("{}{}", prefix, format!("✓ {}", tool_name).green());
                    }
                    self.tool_depth = self.tool_depth.saturating_sub(1);
                    self.current_tool = None;
                }
                acp::ToolCallStatus::Failed => {
                    if let Some(tool_name) = &self.current_tool {
                        eprintln!("{}{}", prefix, format!("✗ {}", tool_name).red());
                    }
                    self.tool_depth = self.tool_depth.saturating_sub(1);
                    self.current_tool = None;
                }
                acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress => {
                    // Tool is still running
                }
            }
        }
    }

    /// Render a retry status
    pub fn render_retry(&mut self, status: &acp_thread::RetryStatus) {
        if self.mode == OutputMode::Verbose {
            eprintln!(
                "{}",
                format!(
                    "⟳ Retrying in {:?} (attempt {}/{})",
                    status.duration, status.attempt, status.max_attempts
                )
                .yellow()
            );
        }
    }

    /// Render the final stop reason
    pub fn render_stop(&mut self, reason: &acp::StopReason) {
        if self.mode == OutputMode::Verbose {
            match reason {
                acp::StopReason::EndTurn => {
                    // Normal completion, no message needed
                    println!(); // Ensure newline at end
                }
                acp::StopReason::MaxTokens => {
                    eprintln!("{}", "⚠ Response truncated (max tokens reached)".yellow());
                }
                acp::StopReason::Refusal => {
                    eprintln!("{}", "✗ Request refused".red());
                }
                acp::StopReason::MaxTurnRequests => {
                    eprintln!(
                        "{}",
                        "⚠ Maximum turn requests reached".yellow()
                    );
                }
                acp::StopReason::Cancelled => {
                    eprintln!("{}", "✗ Request cancelled".yellow());
                }
            }
        }
    }

    /// Render an error
    pub fn render_error(&mut self, error: &anyhow::Error) {
        eprintln!("{}", format!("Error: {:#}", error).red());
    }

    /// Finish rendering and return any buffered output (for JSON mode)
    pub fn finish(&mut self) {
        if self.mode != OutputMode::Json {
            println!(); // Ensure final newline
        }
    }
}

/// Format a tool call as JSON for JSON output mode
pub fn tool_call_to_json(tool_call: &acp::ToolCall) -> serde_json::Value {
    serde_json::json!({
        "type": "tool_call",
        "id": tool_call.id.0,
        "title": tool_call.title,
        "status": format!("{:?}", tool_call.status),
        "kind": format!("{:?}", tool_call.kind),
    })
}

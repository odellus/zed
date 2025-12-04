//! ACP (Agent Client Protocol) command for testing external agents like Claude Code
//!
//! This is a simplified version that directly interacts with Claude Code's JSON-RPC
//! protocol for debugging telemetry capture.

use anyhow::{Context as _, Result};
use colored::Colorize;
use crow_telemetry::{AgentRole, CrowTelemetryDb, TraceBuilder};
use futures::io::BufReader;
use futures::{AsyncBufReadExt as _, AsyncWriteExt as _};
use gpui::AsyncApp;
use serde_json::json;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

/// Run an interactive ACP session with an external agent (Claude Code, Gemini, etc.)
pub async fn run_acp_command(
    agent: String,
    message: Option<String>,
    cx: &mut AsyncApp,
) -> Result<()> {
    eprintln!("{}", "ACP External Agent Mode".purple().bold());
    eprintln!("{}", "════════════════════════════════════════".dimmed());

    // Determine which agent to connect to
    let (command_path, args, telemetry_id, agent_role) = match agent.as_str() {
        "claude" | "claude-code" => {
            // Try to find claude-code-acp or fall back to claude
            let (path, args) = find_claude_code_command()?;
            (path, args, "claude-code", AgentRole::ExternalClaudeCode)
        }
        "gemini" => {
            anyhow::bail!("Gemini ACP not yet implemented in crow-cli");
        }
        _ => {
            // Treat as a custom command path
            (PathBuf::from(&agent), vec![], "custom", AgentRole::ExternalCustom)
        }
    };

    eprintln!("Connecting to {}...", telemetry_id.cyan());

    // Connect to telemetry database
    let trace_db = match cx.update(|cx| CrowTelemetryDb::connect(cx)) {
        Ok(task) => task.await.ok().map(Arc::new),
        Err(_) => None,
    };

    if trace_db.is_some() {
        eprintln!("{}", "Telemetry: Connected to trace database".green());
    } else {
        eprintln!("{}", "Telemetry: No trace database available".yellow());
    }

    // Spawn the ACP agent process
    let mut child = util::command::new_smol_command(&command_path)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn ACP agent")?;

    let mut stdin = child.stdin.take().context("Failed to take stdin")?;
    let stdout = child.stdout.take().context("Failed to take stdout")?;
    let stderr = child.stderr.take().context("Failed to take stderr")?;

    // Spawn stderr reader in background
    cx.background_executor().spawn(async move {
        let mut stderr = BufReader::new(stderr);
        let mut line = String::new();
        while let Ok(n) = stderr.read_line(&mut line).await {
            if n == 0 {
                break;
            }
            log::warn!("ACP stderr: {}", line.trim());
            line.clear();
        }
    }).detach();

    eprintln!("{}", "Connected. Initializing session...".dimmed());

    // Send initialize request
    // Protocol version is a simple integer (V1 = 1)
    let init_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {
                    "readTextFile": true,
                    "writeTextFile": true
                },
                "terminal": false
            },
            "clientInfo": {
                "name": "crow-cli",
                "version": "0.1.0"
            }
        }
    });

    send_json(&mut stdin, &init_request).await?;

    // Read initialize response
    let mut stdout_reader = BufReader::new(stdout);
    let init_response = read_json_line(&mut stdout_reader).await?;

    // Verify initialization succeeded
    if init_response.get("error").is_some() {
        anyhow::bail!("Initialize failed: {}", init_response);
    }

    // Create a new session
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let session_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "mcpServers": [],
            "cwd": cwd
        }
    });

    send_json(&mut stdin, &session_request).await?;
    let session_response = read_json_line(&mut stdout_reader).await?;

    // Verify session creation succeeded
    if session_response.get("error").is_some() {
        anyhow::bail!("Session creation failed: {}", session_response);
    }

    let session_id = session_response
        .get("result")
        .and_then(|r| r.get("sessionId"))
        .and_then(|s| s.as_str())
        .context("No session ID in response")?
        .to_string();

    eprintln!("Session: {}", session_id.cyan());
    eprintln!("{}", "════════════════════════════════════════".dimmed());
    eprintln!();

    // If a message was provided, send it
    if let Some(msg) = message {
        send_prompt(
            &mut stdin,
            &mut stdout_reader,
            &session_id,
            &msg,
            trace_db.as_ref(),
            agent_role,
            telemetry_id,
        )
        .await?;
    } else {
        // Interactive mode
        eprintln!("Enter messages (Ctrl+D to exit):");
        eprintln!();

        let stdin_io = io::stdin();
        loop {
            eprint!("{} ", "▶".white().bold());
            io::stderr().flush().ok();

            let mut input = String::new();
            match stdin_io.read_line(&mut input) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let input = input.trim();
                    if input.is_empty() {
                        continue;
                    }
                    send_prompt(
                        &mut stdin,
                        &mut stdout_reader,
                        &session_id,
                        input,
                        trace_db.as_ref(),
                        agent_role,
                        telemetry_id,
                    )
                    .await?;
                }
                Err(e) => {
                    eprintln!("Error reading input: {}", e);
                    break;
                }
            }
        }
    }

    eprintln!();
    eprintln!("{}", "Session ended.".dimmed());

    // Clean up
    drop(stdin);
    child.kill().ok();

    Ok(())
}

/// Find the claude-code-acp command
fn find_claude_code_command() -> Result<(PathBuf, Vec<String>)> {
    let home = dirs::home_dir().context("No home directory")?;

    // First, try the zed external_agents directory (where crow/zed installs it)
    let external_agents_dir = home.join(".local/share/zed/external_agents/claude-code-acp");
    if external_agents_dir.exists() {
        // Find the latest version directory
        if let Ok(entries) = std::fs::read_dir(&external_agents_dir) {
            let mut versions: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect();
            versions.sort_by(|a, b| b.path().cmp(&a.path())); // Sort descending

            if let Some(latest) = versions.first() {
                let index_js = latest.path()
                    .join("node_modules/@zed-industries/claude-code-acp/dist/index.js");
                if index_js.exists() {
                    return Ok((PathBuf::from("node"), vec![index_js.to_string_lossy().to_string()]));
                }
            }
        }
    }

    // Try ~/.zed/node_modules (old location)
    let npm_path = home
        .join(".zed")
        .join("node_modules")
        .join("@zed-industries")
        .join("claude-code-acp")
        .join("dist")
        .join("index.js");

    if npm_path.exists() {
        return Ok((PathBuf::from("node"), vec![npm_path.to_string_lossy().to_string()]));
    }

    // Fall back to global claude command with --acp flag
    Ok((PathBuf::from("claude"), vec!["--acp".to_string()]))
}

/// Send a JSON-RPC message
async fn send_json<W: futures::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &serde_json::Value,
) -> Result<()> {
    let json_str = serde_json::to_string(value)?;
    writer
        .write_all(format!("{}\n", json_str).as_bytes())
        .await?;
    writer.flush().await?;
    Ok(())
}

/// Read a JSON-RPC message (single line)
async fn read_json_line<R: futures::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<serde_json::Value> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let value: serde_json::Value = serde_json::from_str(&line)?;
    Ok(value)
}

/// Send a prompt and display the response
async fn send_prompt<W, R>(
    stdin: &mut W,
    stdout: &mut R,
    session_id: &str,
    message: &str,
    trace_db: Option<&Arc<CrowTelemetryDb>>,
    agent_role: AgentRole,
    telemetry_id: &str,
) -> Result<()>
where
    W: futures::io::AsyncWrite + Unpin,
    R: futures::io::AsyncBufRead + Unpin,
{
    static mut REQUEST_ID: i64 = 100;
    let request_id = unsafe {
        REQUEST_ID += 1;
        REQUEST_ID
    };

    let start = Instant::now();

    // Build trace before the call
    let trace_builder = trace_db.map(|_| {
        TraceBuilder::new(
            session_id.to_string(),
            agent_role,
            telemetry_id.to_string(),
            "unknown".to_string(),
            message.to_string(),
        )
    });

    // Send the prompt request
    let prompt_request = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{
                "type": "text",
                "text": message
            }]
        }
    });

    send_json(stdin, &prompt_request).await?;

    // Read responses until we get the final result
    let mut response_content = String::new();
    let mut tool_calls = Vec::new();

    loop {
        let response = read_json_line(stdout).await?;

        // Check if this is a notification (no id) or a response
        if response.get("id").is_some() {
            // This is the final response
            let elapsed = start.elapsed();

            if let Some(error) = response.get("error") {
                eprintln!();
                eprintln!("{} {}", "ERROR".red().bold(), error);

                // Save error trace
                if let (Some(db), Some(builder)) = (trace_db, trace_builder) {
                    let trace = builder.fail(error.to_string());
                    db.save_trace(trace).await.ok();
                }
            } else {
                let stop_reason = response
                    .get("result")
                    .and_then(|r| r.get("stopReason"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");

                eprintln!();
                eprintln!(
                    "{} {} {}",
                    "◀".green().bold(),
                    "DONE".green().bold(),
                    format!("({:.1}s, stop: {})", elapsed.as_secs_f64(), stop_reason).dimmed()
                );

                // Save trace
                if let (Some(db), Some(builder)) = (trace_db, trace_builder) {
                    let tool_calls_str = if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls.join(", "))
                    };

                    let trace = builder.complete(
                        Some(response_content.clone()),
                        tool_calls_str,
                        None,
                        None,
                        None,
                    );
                    db.save_trace(trace).await.ok();
                    eprintln!("{}", "Trace saved".dimmed());
                }
            }

            eprintln!();
            break;
        } else {
            // This is a notification - process it
            if let Some(method) = response.get("method").and_then(|m| m.as_str()) {
                match method {
                    "session/update" => {
                        if let Some(params) = response.get("params") {
                            if let Some(update) = params.get("update") {
                                handle_session_update(update, &mut response_content, &mut tool_calls);
                            }
                        }
                    }
                    _ => {
                        log::debug!("Unknown notification: {}", method);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle a session update notification
fn handle_session_update(
    update: &serde_json::Value,
    response_content: &mut String,
    tool_calls: &mut Vec<String>,
) {
    let update_type = update.get("sessionUpdate").and_then(|s| s.as_str());

    match update_type {
        Some("agent_message_chunk") => {
            // Streaming text content
            if let Some(content) = update.get("content") {
                if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        print!("{}", text.green());
                        io::stdout().flush().ok();
                        response_content.push_str(text);
                    }
                }
            }
        }
        Some("tool_call") => {
            if let Some(tool_call) = update.get("toolCall") {
                if let Some(title) = tool_call.get("title").and_then(|t| t.as_str()) {
                    eprintln!();
                    eprintln!(
                        "{} {}",
                        "TOOL".magenta().bold(),
                        title
                    );
                    tool_calls.push(title.to_string());
                }
            }
        }
        Some("tool_call_update") => {
            if let Some(fields) = update.get("fields") {
                if let Some(status) = fields.get("status").and_then(|s| s.as_str()) {
                    if status == "completed" {
                        eprintln!("{}", "  ✓ completed".green());
                    } else if status == "failed" {
                        eprintln!("{}", "  ✗ failed".red());
                    }
                }
            }
        }
        _ => {
            // Unknown update type - ignore
        }
    }
}

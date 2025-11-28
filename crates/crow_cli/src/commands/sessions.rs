use agent::{AgentMessageContent, Message, ThreadsDatabase, UserMessageContent};
use agent_client_protocol as acp;
use anyhow::{Context, Result};
use colored::Colorize;
use gpui::AsyncApp;
use std::io::{self, Write};

use crate::init;

/// List all saved sessions
pub async fn run_list_sessions_command(
    limit: usize,
    json_output: bool,
    cx: &mut AsyncApp,
) -> Result<()> {
    log::info!("Listing sessions (limit: {})", limit);

    // Initialize (needed to set up the environment)
    let _crow = init::initialize(cx).await?;

    // Load threads directly from the database
    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let threads = database.list_threads().await?;

    // Already sorted by updated_at DESC from the database query

    if json_output {
        let sessions: Vec<_> = threads
            .iter()
            .take(limit)
            .map(|t| {
                serde_json::json!({
                    "id": t.id.0,
                    "title": t.title,
                    "updated_at": t.updated_at.to_rfc3339(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }

    if threads.is_empty() {
        println!("{}", "No sessions found.".yellow());
        return Ok(());
    }

    println!("{}", "Sessions:".green().bold());
    println!();

    for thread in threads.iter().take(limit) {
        let title = if thread.title.is_empty() {
            "Untitled".dimmed().to_string()
        } else {
            thread.title.to_string()
        };
        let updated = thread.updated_at.format("%Y-%m-%d %H:%M");
        println!(
            "  {} {} {}",
            thread.id.0.bright_cyan(),
            title,
            format!("({})", updated).dimmed()
        );
    }

    if threads.len() > limit {
        println!();
        println!(
            "{}",
            format!("Showing {} of {} sessions. Use -n to show more.", limit, threads.len())
                .dimmed()
        );
    }

    println!();
    println!(
        "{}",
        "Commands: crow-cli session show <id> | crow-cli chat -s <id> \"message\"".dimmed()
    );

    Ok(())
}

/// Show session details and messages
pub async fn run_show_session_command(
    session_id: String,
    json_output: bool,
    last_n: Option<usize>,
    cx: &mut AsyncApp,
) -> Result<()> {
    log::info!("Showing session: {}", session_id);

    // Initialize
    let _crow = init::initialize(cx).await?;

    // Load the thread from database
    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let session_id_parsed = acp::SessionId(session_id.clone().into());
    let thread_data = database
        .load_thread(session_id_parsed.clone())
        .await?
        .context(format!("Session not found: {}", session_id))?;

    if json_output {
        // Output the raw thread data as JSON
        println!("{}", serde_json::to_string_pretty(&thread_data)?);
        return Ok(());
    }

    // Display thread info
    println!("{}", "═".repeat(65).dimmed());
    println!(
        "{} {}",
        "Session:".green().bold(),
        session_id.bright_cyan()
    );

    // Show title and updated time from thread_data
    let title = if thread_data.title.is_empty() {
        "Untitled"
    } else {
        &thread_data.title
    };
    println!("{} {}", "Title:".green().bold(), title);

    // Show profile if present
    if let Some(ref profile) = thread_data.profile {
        let profile_display = match profile.as_str() {
            "discriminator" => "discriminator (dual-agent reviewer)".yellow(),
            "write" => "write (executor)".normal(),
            other => other.normal(),
        };
        println!("{} {}", "Profile:".green().bold(), profile_display);
    }

    // Check for session pairing (auto/dual-agent mode)
    if let Ok(Some(pair)) = database.get_session_pair(session_id_parsed.clone()).await {
        let is_executor = pair.executor_session_id.0 == session_id.as_str().into();
        if is_executor {
            println!(
                "{} {} (paired with discriminator: {})",
                "Mode:".green().bold(),
                "auto (executor)".cyan(),
                pair.discriminator_session_id.0.bright_cyan()
            );
        } else {
            println!(
                "{} {} (paired with executor: {})",
                "Mode:".green().bold(),
                "auto (discriminator)".yellow(),
                pair.executor_session_id.0.bright_cyan()
            );
        }
    } else {
        println!("{} {}", "Mode:".green().bold(), "single-agent".normal());
    }

    println!(
        "{} {}",
        "Updated:".green().bold(),
        thread_data.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("{}", "═".repeat(65).dimmed());
    println!();

    // Display messages from thread data
    let messages = &thread_data.messages;
    let messages_to_show: Vec<_> = if let Some(n) = last_n {
        messages.iter().rev().take(n).rev().collect()
    } else {
        messages.iter().collect()
    };

    if messages_to_show.is_empty() {
        println!("{}", "No messages in this session.".yellow());
        return Ok(());
    }

    for (idx, message) in messages_to_show.iter().enumerate() {
        match message {
            Message::User(user_msg) => {
                println!("{}", format!("▶ USER [{}]", idx + 1).white().bold());
                for content in &user_msg.content {
                    match content {
                        UserMessageContent::Text(text) => {
                            println!("{}", text.white());
                        }
                        UserMessageContent::Image(_) => {
                            println!("{}", "[Image]".dimmed());
                        }
                        UserMessageContent::Mention { uri, content } => {
                            println!("{} {}", format!("@{:?}", uri).bright_blue(), content.dimmed());
                        }
                    }
                }
                println!();
            }
            Message::Agent(agent_msg) => {
                println!(
                    "{}",
                    format!("◀ ASSISTANT [{}]", idx + 1)
                        .truecolor(180, 255, 100)
                        .bold()
                );

                // Show text content
                for block in &agent_msg.content {
                    match block {
                        AgentMessageContent::Text(text) => {
                            for line in text.lines() {
                                println!("{}", line.truecolor(180, 255, 100));
                            }
                        }
                        AgentMessageContent::Thinking { text, .. } => {
                            println!("{}", "💭 Thinking:".dimmed());
                            for line in text.lines() {
                                println!("  {}", line.dimmed());
                            }
                        }
                        AgentMessageContent::RedactedThinking(_) => {
                            println!("{}", "💭 [Redacted thinking]".dimmed());
                        }
                        AgentMessageContent::ToolUse(tool_use) => {
                            println!(
                                "  {} {} {}",
                                "🔧".dimmed(),
                                tool_use.name.bright_purple(),
                                format!("({})", tool_use.id).dimmed()
                            );
                            if let Ok(pretty) = serde_json::to_string_pretty(&tool_use.input) {
                                for line in pretty.lines().take(5) {
                                    println!("     {}", line.cyan());
                                }
                                if pretty.lines().count() > 5 {
                                    println!("     {}", "...".dimmed());
                                }
                            }
                        }
                    }
                }

                // Show tool results
                for (tool_id, result) in &agent_msg.tool_results {
                    let status = if result.is_error {
                        "❌".to_string()
                    } else {
                        "✅".to_string()
                    };
                    println!(
                        "  {} {} {}",
                        status,
                        format!("{:?}", tool_id).dimmed(),
                        if result.is_error { "(failed)" } else { "" }.red()
                    );
                }
                println!();
            }
            Message::Resume => {
                println!("{}", "↻ [Resume marker]".dimmed());
                println!();
            }
        }
    }

    println!("{}", "═".repeat(65).dimmed());
    println!(
        "{}",
        format!("Total messages: {}", messages.len()).dimmed()
    );

    Ok(())
}

/// Delete a session
pub async fn run_delete_session_command(
    session_id: String,
    force: bool,
    cx: &mut AsyncApp,
) -> Result<()> {
    log::info!("Deleting session: {}", session_id);

    if !force {
        print!(
            "{}",
            format!("Delete session {}? [y/N] ", session_id).yellow()
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("{}", "Cancelled.".dimmed());
            return Ok(());
        }
    }

    // Initialize
    let crow = init::initialize(cx).await?;

    // Delete via history store
    let session_id_parsed = acp::SessionId(session_id.clone().into());

    cx.update(|cx| {
        let history = crow.connection.history(cx);
        history.update(cx, |h, cx| h.delete_thread(session_id_parsed, cx))
    })?
    .await?;

    println!("{} {}", "Deleted:".green(), session_id);
    Ok(())
}

/// Inspect raw session data (for debugging)
pub async fn run_inspect_session_command(session_id: String, cx: &mut AsyncApp) -> Result<()> {
    log::info!("Inspecting session: {}", session_id);

    // Initialize
    let _crow = init::initialize(cx).await?;

    // Load the thread from database
    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let session_id_parsed = acp::SessionId(session_id.clone().into());
    let thread_data = database
        .load_thread(session_id_parsed.clone())
        .await?
        .context(format!("Session not found: {}", session_id))?;

    // Output full raw JSON
    println!("{}", "═".repeat(65).dimmed());
    println!(
        "{} {}",
        "Raw session data for:".green().bold(),
        session_id.bright_cyan()
    );
    println!("{}", "═".repeat(65).dimmed());
    println!();
    println!("{}", serde_json::to_string_pretty(&thread_data)?);

    Ok(())
}

/// Create a new session
pub async fn run_new_session_command(title: Option<String>, cx: &mut AsyncApp) -> Result<()> {
    log::info!("Creating new session");

    // Initialize
    let crow = init::initialize(cx).await?;

    // Create a new thread
    let acp_thread = cx.update(|cx| crow.new_thread(cx))?.await?;

    // Set title if provided
    if let Some(title) = title {
        acp_thread
            .update(cx, |thread, cx| thread.set_title(title.into(), cx))?
            .await?;
    }

    let session_id = acp_thread.read_with(cx, |thread, _| thread.session_id().clone())?;

    println!("{} {}", "Created session:".green(), session_id.0);

    Ok(())
}

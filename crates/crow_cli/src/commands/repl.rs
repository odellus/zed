use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use gpui::{AsyncApp, Entity};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::init;
use crate::render::{OutputMode, TerminalRenderer};

/// Run an interactive REPL session
pub async fn run_repl_command(session_id: Option<String>, cx: &mut AsyncApp) -> Result<()> {
    log::info!("Starting REPL session");

    // Initialize the agent
    let crow = init::initialize(cx).await?;

    // Create or load a thread
    let acp_thread = if let Some(_id) = session_id {
        // TODO: Load existing session
        // For now, just create a new one
        cx.update(|cx| crow.new_thread(cx))?
            .await
            .context("Failed to create thread")?
    } else {
        cx.update(|cx| crow.new_thread(cx))?
            .await
            .context("Failed to create thread")?
    };

    println!("Crow REPL - Type your message and press Enter. Use Ctrl+D to exit.");
    println!();

    // Create readline editor
    let mut rl = DefaultEditor::new()?;

    loop {
        let readline = rl.readline("you> ");

        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                // Add to history
                rl.add_history_entry(line)?;

                // Handle special commands
                if line.starts_with('/') {
                    match line {
                        "/quit" | "/exit" => {
                            println!("Goodbye!");
                            break;
                        }
                        "/clear" => {
                            // Clear screen
                            print!("\x1B[2J\x1B[1;1H");
                            continue;
                        }
                        "/help" => {
                            println!("Commands:");
                            println!("  /quit, /exit - Exit the REPL");
                            println!("  /clear       - Clear the screen");
                            println!("  /help        - Show this help");
                            println!();
                            continue;
                        }
                        _ => {
                            println!("Unknown command: {}", line);
                            continue;
                        }
                    }
                }

                // Send the message and wait for response
                if let Err(e) = send_and_wait_response(&acp_thread, line.to_string(), cx).await {
                    eprintln!("Error: {:#}", e);
                }

                println!();
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    Ok(())
}

async fn send_and_wait_response(
    acp_thread: &Entity<acp_thread::AcpThread>,
    message: String,
    cx: &mut AsyncApp,
) -> Result<()> {
    // Send the prompt
    let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent {
        text: message,
        annotations: None,
        meta: None,
    })];

    // Send and wait for completion
    let send_future = acp_thread.update(cx, |thread, cx| thread.send(prompt_blocks, cx))?;
    send_future.await?;

    // Create the terminal renderer
    let mut renderer = TerminalRenderer::new(OutputMode::Verbose);

    // Get the last assistant response
    let response = acp_thread.read_with(cx, |thread, cx| {
        // Find the last assistant message
        thread
            .entries()
            .iter()
            .rev()
            .find_map(|entry| {
                if let acp_thread::AgentThreadEntry::AssistantMessage(msg) = entry {
                    Some(msg.to_markdown(cx))
                } else {
                    None
                }
            })
    })?;

    if let Some(response) = response {
        renderer.render_text(&response);
    }

    renderer.finish();
    Ok(())
}

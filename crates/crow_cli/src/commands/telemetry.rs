use agent::ThreadsDatabase;
use anyhow::Result;
use colored::Colorize;
use gpui::AsyncApp;

use crate::init;

/// List all registered prompts
pub async fn list_prompts(json: bool, cx: &mut AsyncApp) -> Result<()> {
    let _crow = init::initialize(cx).await?;

    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let prompts = database.list_prompts().await?;

    if json {
        let json_output: Vec<_> = prompts
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "template_hash": p.template_hash,
                    "created_at": p.created_at.to_rfc3339(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_output)?);
    } else {
        println!("{}", "Registered Prompts".green().bold());
        println!("{}", "─".repeat(80));

        for prompt in &prompts {
            println!(
                "{} {} {}",
                prompt.name.bright_cyan(),
                format!("({})", &prompt.template_hash[..8]).dimmed(),
                prompt.created_at.format("%Y-%m-%d %H:%M").to_string().dimmed(),
            );
            println!("  ID: {}", prompt.id.dimmed());
        }

        println!();
        println!("{} prompts registered", prompts.len());
    }

    Ok(())
}

/// Show a specific prompt's content
pub async fn show_prompt(prompt_id: String, cx: &mut AsyncApp) -> Result<()> {
    let _crow = init::initialize(cx).await?;

    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let prompt = database.get_prompt(prompt_id.clone()).await?;

    match prompt {
        Some(p) => {
            println!("{}: {}", "Name".green().bold(), p.name);
            println!("{}: {}", "ID".green().bold(), p.id);
            println!("{}: {}", "Hash".green().bold(), p.template_hash);
            println!("{}: {}", "Created".green().bold(), p.created_at);
            println!();
            println!("{}", "Template Content".green().bold());
            println!("{}", "─".repeat(80));
            println!("{}", p.template_content);
        }
        None => {
            eprintln!("Prompt not found: {}", prompt_id);
        }
    }

    Ok(())
}

/// List recent traces
pub async fn list_traces(limit: usize, session_id: Option<String>, json: bool, cx: &mut AsyncApp) -> Result<()> {
    let _crow = init::initialize(cx).await?;

    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let traces = if let Some(sid) = session_id {
        database.list_traces_for_session(sid).await?
    } else {
        database.list_recent_traces(limit).await?
    };

    if json {
        let json_output: Vec<_> = traces
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "session_id": t.session_id,
                    "agent_role": format!("{:?}", t.agent_role),
                    "model_provider": t.model_provider,
                    "model_id": t.model_id,
                    "latency_ms": t.latency_ms,
                    "input_tokens": t.input_tokens,
                    "output_tokens": t.output_tokens,
                    "started_at": t.started_at.to_rfc3339(),
                    "error": t.error,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_output)?);
    } else {
        println!("{}", "Recent Traces".green().bold());
        println!("{}", "─".repeat(100));

        for trace in &traces {
            let role_color = match trace.agent_role {
                agent::AgentRole::Executor => "executor".cyan(),
                agent::AgentRole::Discriminator => "discriminator".yellow(),
                agent::AgentRole::EditAgent => "edit_agent".magenta(),
                agent::AgentRole::DiffJudge => "diff_judge".blue(),
            };

            let latency = trace.latency_ms
                .map(|ms| format!("{}ms", ms))
                .unwrap_or_else(|| "?".to_string());

            let tokens = match (trace.input_tokens, trace.output_tokens) {
                (Some(i), Some(o)) => format!("{}/{}", i, o),
                _ => "-/-".to_string(),
            };

            let status = if trace.error.is_some() {
                "ERROR".red()
            } else {
                "OK".green()
            };

            println!(
                "{} {} {} {} {} {} {}",
                trace.started_at.format("%H:%M:%S").to_string().dimmed(),
                role_color,
                format!("{}/{}", trace.model_provider, trace.model_id).bright_blue(),
                latency.yellow(),
                tokens.dimmed(),
                status,
                format!("[{}]", &trace.session_id[..8]).dimmed(),
            );
        }

        println!();
        println!("{} traces", traces.len());
    }

    Ok(())
}

/// Show a specific trace's full details
pub async fn show_trace(trace_id: String, cx: &mut AsyncApp) -> Result<()> {
    let _crow = init::initialize(cx).await?;

    let database = cx
        .update(|cx| ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let trace = database.get_trace(trace_id.clone()).await?;

    match trace {
        Some(t) => {
            println!("{}", "Trace Details".green().bold());
            println!("{}", "─".repeat(80));
            println!("{}: {}", "ID".bright_cyan(), t.id);
            println!("{}: {}", "Session ID".bright_cyan(), t.session_id);
            println!("{}: {:?}", "Agent Role".bright_cyan(), t.agent_role);
            println!("{}: {}/{}", "Model".bright_cyan(), t.model_provider, t.model_id);
            println!("{}: {}", "Started".bright_cyan(), t.started_at);
            if let Some(completed) = t.completed_at {
                println!("{}: {}", "Completed".bright_cyan(), completed);
            }
            if let Some(latency) = t.latency_ms {
                println!("{}: {}ms", "Latency".bright_cyan(), latency);
            }

            println!();
            println!("{}", "Token Usage".green().bold());
            println!("{}", "─".repeat(40));
            println!("  Input:  {}", t.input_tokens.map(|v| v.to_string()).unwrap_or("-".into()));
            println!("  Output: {}", t.output_tokens.map(|v| v.to_string()).unwrap_or("-".into()));
            println!("  Total:  {}", t.total_tokens.map(|v| v.to_string()).unwrap_or("-".into()));

            if let Some(prompt_id) = &t.prompt_id {
                println!();
                println!("{}: {}", "Prompt ID".green().bold(), prompt_id);
            }

            if let Some(error) = &t.error {
                println!();
                println!("{}", "Error".red().bold());
                println!("{}", "─".repeat(40));
                println!("{}", error);
            }

            if let Some(response) = &t.response_content {
                println!();
                println!("{}", "Response".green().bold());
                println!("{}", "─".repeat(40));
                // Truncate long responses
                if response.len() > 500 {
                    println!("{}...", &response[..500]);
                    println!("({} chars total)", response.len());
                } else {
                    println!("{}", response);
                }
            }

            if let Some(tool_calls) = &t.response_tool_calls {
                println!();
                println!("{}", "Tool Calls".green().bold());
                println!("{}", "─".repeat(40));
                // Pretty print JSON
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(tool_calls) {
                    println!("{}", serde_json::to_string_pretty(&parsed)?);
                } else {
                    println!("{}", tool_calls);
                }
            }
        }
        None => {
            eprintln!("Trace not found: {}", trace_id);
        }
    }

    Ok(())
}

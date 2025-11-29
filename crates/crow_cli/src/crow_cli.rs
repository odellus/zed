mod commands;
mod init;
mod render;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gpui::Application;

#[derive(Parser)]
#[command(name = "crow-cli")]
#[command(about = "Crow Agent CLI - Run the Zed agent from the command line")]
struct Cli {
    /// Print environment variables as JSON (used by shell environment detection)
    #[arg(long)]
    printenv: bool,

    #[command(subcommand)]
    command: Option<Commands>,

    /// Message to send (shorthand for `crow-cli chat "message"`)
    #[arg(trailing_var_arg = true)]
    message: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Send a message to the agent (default command)
    Chat {
        /// The message to send
        #[arg(trailing_var_arg = true)]
        message: Vec<String>,

        /// Force a new session instead of resuming
        #[arg(long, short = 'n')]
        new: bool,

        /// Use a specific session ID
        #[arg(long, short = 's')]
        session: Option<String>,

        /// Output only the final response (no streaming decorations)
        #[arg(long, short = 'q')]
        quiet: bool,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,

        /// Enable auto mode (executor + discriminator dual-agent pattern)
        #[arg(long, short = 'a')]
        auto: bool,
    },

    /// Start an interactive REPL session
    Repl {
        /// Session ID to resume (optional)
        session: Option<String>,
    },

    /// Session management commands
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// List all sessions (alias for `session list`)
    Sessions,

    /// Create a new session
    New {
        /// Title for the new session
        title: Option<String>,
    },

    /// Telemetry commands (prompts, traces)
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommands,
    },

    /// List recent traces (alias for `telemetry traces`)
    Traces {
        /// Maximum number of traces to show
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Filter by session ID
        #[arg(long, short = 's')]
        session: Option<String>,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// List prompts (alias for `telemetry prompts`)
    Prompts {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    List {
        /// Maximum number of sessions to show
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Show session details and messages
    Show {
        /// Session ID to show
        session_id: String,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,

        /// Show only the last N messages
        #[arg(long, short = 'n')]
        last: Option<usize>,
    },

    /// Delete a session
    Delete {
        /// Session ID to delete
        session_id: String,

        /// Skip confirmation
        #[arg(long, short = 'f')]
        force: bool,
    },

    /// Show raw session data (for debugging)
    Inspect {
        /// Session ID to inspect
        session_id: String,
    },
}

#[derive(Subcommand)]
enum TelemetryCommands {
    /// List registered prompt templates
    Prompts {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Show a specific prompt's content
    Prompt {
        /// Prompt ID
        prompt_id: String,
    },

    /// List recent LLM call traces
    Traces {
        /// Maximum number of traces to show
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Filter by session ID
        #[arg(long, short = 's')]
        session: Option<String>,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Show a specific trace's full details
    Trace {
        /// Trace ID
        trace_id: String,
    },
}

fn main() {
    env_logger::init();

    let cli = Cli::parse();

    // Handle --printenv early (used by shell environment detection)
    if cli.printenv {
        util::shell_env::print_env();
        return;
    }

    // Determine what to run
    let result = match cli.command {
        Some(Commands::Chat {
            message,
            new,
            session,
            quiet,
            json,
            auto,
        }) => run_chat(message.join(" "), new, session, quiet, json, auto),

        Some(Commands::Repl { session }) => run_repl(session),

        Some(Commands::Session { command }) => run_session_command(command),

        Some(Commands::Sessions) => run_session_command(SessionCommands::List {
            limit: 20,
            json: false,
        }),

        Some(Commands::New { title }) => run_new_session(title),

        Some(Commands::Telemetry { command }) => run_telemetry_command(command),

        Some(Commands::Traces { limit, session, json }) => {
            run_telemetry_command(TelemetryCommands::Traces { limit, session, json })
        }

        Some(Commands::Prompts { json }) => {
            run_telemetry_command(TelemetryCommands::Prompts { json })
        }

        None => {
            // No subcommand - treat trailing args as chat message
            if cli.message.is_empty() {
                // No message either - show help or start REPL
                run_repl(None)
            } else {
                run_chat(cli.message.join(" "), false, None, false, false, false)
            }
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

fn run_chat(
    message: String,
    new_session: bool,
    session_id: Option<String>,
    quiet: bool,
    json: bool,
    auto: bool,
) -> Result<()> {
    if message.is_empty() {
        anyhow::bail!("No message provided. Usage: crow-cli chat \"your message\"");
    }

    let output_mode = if json {
        render::OutputMode::Json
    } else if quiet {
        render::OutputMode::Quiet
    } else {
        render::OutputMode::Verbose
    };

    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::chat::run_chat_command(
                message,
                new_session,
                session_id,
                output_mode,
                auto,
                &mut cx,
            )
            .await;

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

fn run_repl(session_id: Option<String>) -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::repl::run_repl_command(session_id, &mut cx).await;

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

fn run_session_command(command: SessionCommands) -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = match command {
                SessionCommands::List { limit, json } => {
                    commands::sessions::run_list_sessions_command(limit, json, &mut cx).await
                }
                SessionCommands::Show {
                    session_id,
                    json,
                    last,
                } => commands::sessions::run_show_session_command(session_id, json, last, &mut cx).await,
                SessionCommands::Delete { session_id, force } => {
                    commands::sessions::run_delete_session_command(session_id, force, &mut cx).await
                }
                SessionCommands::Inspect { session_id } => {
                    commands::sessions::run_inspect_session_command(session_id, &mut cx).await
                }
            };

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

fn run_new_session(title: Option<String>) -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::sessions::run_new_session_command(title, &mut cx).await;

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

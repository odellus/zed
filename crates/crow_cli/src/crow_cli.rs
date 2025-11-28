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
    },

    /// Start an interactive REPL session
    Repl {
        /// Session ID to resume (optional)
        session: Option<String>,
    },

    /// List all sessions
    Sessions,

    /// Create a new session
    New {
        /// Title for the new session
        title: Option<String>,
    },
}

fn main() {
    env_logger::init();

    let cli = Cli::parse();

    // Determine what to run
    let result = match cli.command {
        Some(Commands::Chat {
            message,
            new,
            session,
            quiet,
            json,
        }) => run_chat(message.join(" "), new, session, quiet, json),

        Some(Commands::Repl { session }) => run_repl(session),

        Some(Commands::Sessions) => run_list_sessions(),

        Some(Commands::New { title }) => run_new_session(title),

        None => {
            // No subcommand - treat trailing args as chat message
            if cli.message.is_empty() {
                // No message either - show help or start REPL
                run_repl(None)
            } else {
                run_chat(cli.message.join(" "), false, None, false, false)
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

fn run_list_sessions() -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::sessions::run_list_sessions_command(&mut cx).await;

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

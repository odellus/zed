#![doc = include_str!("../README.md")]

mod commands;
mod init;
mod render;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gpui::Application;

const ABOUT: &str = "\
Crow Agent CLI - Run and observe the Zed agent from the command line.

Crow is a revolutionary tool for agent-driven development of Zed itself. It provides:
  - Direct CLI access to Zed's native agent (same brain, no UI)
  - Auto mode: dual-agent executor/discriminator loop for autonomous coding
  - Full telemetry: every LLM call traced with prompts, responses, and latency
  - Session management: resume, inspect, and debug agent conversations

Built for Claude Code and other AI agents to help develop and test Zed's agent system.";

const AFTER_HELP: &str = "\
EXAMPLES:
    # Quick one-shot question
    crow-cli \"What files handle the agent's tool execution?\"

    # Start autonomous coding with dual-agent mode
    crow-cli chat --auto \"Implement a new grep tool that supports multiline\"

    # Interactive REPL for exploration
    crow-cli repl

    # Resume a previous session
    crow-cli chat -s abc123 \"Continue where we left off\"

    # List recent sessions
    crow-cli sessions

    # Inspect telemetry for debugging
    crow-cli traces                    # Recent LLM calls
    crow-cli traces -s <session_id>    # Traces for specific session
    crow-cli telemetry trace <id>      # Full trace details with prompt/response

    # See registered prompt templates
    crow-cli prompts

AGENT WORKFLOW:
    1. Use 'crow-cli sessions' to see existing work
    2. Resume with 'crow-cli chat -s <id>' or start fresh with 'crow-cli chat -n'
    3. For autonomous tasks, use '--auto' to engage the discriminator loop
    4. Debug issues with 'crow-cli traces' to see what the model received/returned
    5. Cross-reference prompts with 'crow-cli telemetry prompt <id>'

For more information, see: crates/crow_cli/AGENTS.md";

#[derive(Parser)]
#[command(name = "crow-cli")]
#[command(about = "Crow Agent CLI - Run the Zed agent from the command line")]
#[command(long_about = ABOUT)]
#[command(after_help = AFTER_HELP)]
#[command(arg_required_else_help = false)]
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
    /// Send a message to the agent and get a response
    #[command(
        long_about = "Send a message to the Zed agent and receive a response.\n\n\
            By default, resumes the most recent session. Use -n for a fresh start.\n\
            Use --auto to engage the dual-agent loop where a discriminator reviews\n\
            the executor's work and requests corrections until satisfied.",
        after_help = "EXAMPLES:\n    \
            crow-cli chat \"Explain the Thread struct\"\n    \
            crow-cli chat -n \"Start fresh: implement a new tool\"\n    \
            crow-cli chat -s abc123 \"Continue this session\"\n    \
            crow-cli chat --auto \"Autonomously fix all type errors in agent.rs\""
    )]
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

    /// Start an interactive REPL session for ongoing conversation
    #[command(
        long_about = "Start an interactive read-eval-print loop with the agent.\n\n\
            Useful for exploratory conversations, debugging, or when you want\n\
            to iterate on a problem with back-and-forth dialogue.",
        after_help = "EXAMPLES:\n    \
            crow-cli repl                  # New REPL session\n    \
            crow-cli repl abc123           # Resume session in REPL mode"
    )]
    Repl {
        /// Session ID to resume (optional)
        session: Option<String>,
    },

    /// Session management commands
    #[command(
        long_about = "Manage agent conversation sessions.\n\n\
            Sessions persist the full conversation history between you and the agent.\n\
            Use these commands to list, inspect, resume, or delete sessions.",
        after_help = "EXAMPLES:\n    \
            crow-cli session list              # List recent sessions\n    \
            crow-cli session show abc123       # View session messages\n    \
            crow-cli session show abc123 -n 5  # Last 5 messages only\n    \
            crow-cli session inspect abc123    # Raw data for debugging\n    \
            crow-cli session delete abc123     # Delete a session"
    )]
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// List all sessions (alias for `session list`)
    #[command(
        long_about = "List recent agent sessions with their IDs, titles, and timestamps.\n\n\
            This is a convenient alias for 'crow-cli session list'.",
        after_help = "EXAMPLES:\n    \
            crow-cli sessions      # Show last 20 sessions"
    )]
    Sessions,

    /// Create a new session
    #[command(
        long_about = "Create a new empty session, optionally with a title.\n\n\
            Useful when you want to explicitly start fresh rather than resuming.",
        after_help = "EXAMPLES:\n    \
            crow-cli new                          # New untitled session\n    \
            crow-cli new \"Refactoring thread.rs\"  # New session with title"
    )]
    New {
        /// Title for the new session
        title: Option<String>,
    },

    /// Telemetry commands (prompts, traces)
    #[command(
        long_about = "Access telemetry data: prompt templates and LLM call traces.\n\n\
            Every LLM call made by crow-cli is traced with full request/response data,\n\
            token usage, latency, and which prompt template was used. This is invaluable\n\
            for debugging agent behavior and understanding model interactions.",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry prompts          # List registered templates\n    \
            crow-cli telemetry prompt <id>      # Show template content\n    \
            crow-cli telemetry traces           # List recent LLM calls\n    \
            crow-cli telemetry traces -s <sid>  # Traces for a session\n    \
            crow-cli telemetry trace <id>       # Full trace details"
    )]
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommands,
    },

    /// List recent traces (alias for `telemetry traces`)
    #[command(
        long_about = "List recent LLM call traces across all sessions.\n\n\
            Each trace captures: session, agent role, model, latency, and token counts.\n\
            Use 'crow-cli telemetry trace <id>' to see full request/response details.",
        after_help = "EXAMPLES:\n    \
            crow-cli traces                 # Last 20 traces\n    \
            crow-cli traces -n 50           # Last 50 traces\n    \
            crow-cli traces -s abc123       # Traces for specific session\n    \
            crow-cli traces -j              # JSON output for scripting"
    )]
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
    #[command(
        long_about = "List all registered prompt templates with their content hashes.\n\n\
            Prompts are versioned by content hash - if a template changes, a new\n\
            version is registered. This lets you correlate traces with exact prompts.",
        after_help = "EXAMPLES:\n    \
            crow-cli prompts        # List all prompt templates\n    \
            crow-cli prompts -j     # JSON output"
    )]
    Prompts {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Import API keys from system keyring
    #[command(
        long_about = "Import API keys from the system keyring into crow-cli's credential store.\n\n\
            This allows crow-cli to use API keys stored by Zed without requiring\n\
            keyring access on every command. You may be prompted for your system password.",
        after_help = "EXAMPLES:\n    \
            crow-cli login      # Import keys from keyring\n    \
            crow-cli status     # Check which credentials are available\n    \
            crow-cli logout     # Clear stored credentials"
    )]
    Login,

    /// Clear stored credentials
    #[command(
        long_about = "Remove all API keys from crow-cli's local credential store.\n\n\
            This does not affect credentials stored in the system keyring or Zed."
    )]
    Logout,

    /// Show credential status
    #[command(
        long_about = "Display which API credentials are available to crow-cli.\n\n\
            Shows both file-stored credentials (from 'crow-cli login') and\n\
            environment variables."
    )]
    Status,

    /// Connect to an external ACP agent (Claude Code, Gemini, etc.)
    #[command(
        long_about = "Connect directly to an ACP-compatible agent like Claude Code or Gemini.\n\n\
            This bypasses the native crow agent and connects to an external agent server.\n\
            Useful for debugging ACP telemetry capture and testing external agents.",
        after_help = "EXAMPLES:\n    \
            crow-cli acp claude                    # Interactive Claude Code session\n    \
            crow-cli acp claude \"What is 2+2?\"    # One-shot message\n    \
            crow-cli acp /path/to/agent            # Custom agent binary"
    )]
    Acp {
        /// Agent to connect to: claude, gemini, or path to custom agent
        #[arg(default_value = "claude")]
        agent: String,

        /// Message to send (if omitted, starts interactive mode)
        message: Option<String>,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    #[command(
        long_about = "List recent sessions with ID, title, message count, and timestamps.\n\n\
            Sessions are sorted by most recent activity. Use the session ID to\n\
            resume a conversation with 'crow-cli chat -s <id>'.",
        after_help = "EXAMPLES:\n    \
            crow-cli session list           # Last 20 sessions\n    \
            crow-cli session list -n 50     # Last 50 sessions\n    \
            crow-cli session list -j        # JSON for parsing"
    )]
    List {
        /// Maximum number of sessions to show
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Show session details and messages
    #[command(
        long_about = "Display the full conversation history for a session.\n\n\
            Shows all messages in chronological order with role labels (user/assistant).\n\
            Use -n to limit to recent messages for long conversations.",
        after_help = "EXAMPLES:\n    \
            crow-cli session show abc123        # Full conversation\n    \
            crow-cli session show abc123 -n 10  # Last 10 messages\n    \
            crow-cli session show abc123 -j     # JSON format"
    )]
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
    #[command(
        long_about = "Permanently delete a session and all its messages.\n\n\
            This action cannot be undone. Use -f to skip the confirmation prompt.",
        after_help = "EXAMPLES:\n    \
            crow-cli session delete abc123      # With confirmation\n    \
            crow-cli session delete abc123 -f   # Force delete"
    )]
    Delete {
        /// Session ID to delete
        session_id: String,

        /// Skip confirmation
        #[arg(long, short = 'f')]
        force: bool,
    },

    /// Show raw session data (for debugging)
    #[command(
        long_about = "Dump the raw session data structure for debugging.\n\n\
            Shows the internal representation including metadata, tool calls,\n\
            and any agent-specific state. Useful for debugging issues.",
        after_help = "EXAMPLES:\n    \
            crow-cli session inspect abc123"
    )]
    Inspect {
        /// Session ID to inspect
        session_id: String,
    },
}

#[derive(Subcommand)]
enum TelemetryCommands {
    /// List registered prompt templates
    #[command(
        long_about = "List all prompt templates registered in the database.\n\n\
            Each prompt shows: ID, name (template file), content hash, and creation time.\n\
            Templates are versioned - changing a .hbs file creates a new version.",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry prompts      # List all prompts\n    \
            crow-cli telemetry prompts -j   # JSON output"
    )]
    Prompts {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Show a specific prompt's content
    #[command(
        long_about = "Display the full content of a prompt template.\n\n\
            Shows the complete Handlebars template including variables and logic.\n\
            Use 'crow-cli telemetry prompts' to find prompt IDs.",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry prompt abc123    # Show template content"
    )]
    Prompt {
        /// Prompt ID
        prompt_id: String,
    },

    /// List recent LLM call traces
    #[command(
        long_about = "List recent LLM API calls with summary information.\n\n\
            Each trace shows: ID, session, agent role, model, latency, and token counts.\n\
            Filter by session to see all calls within a specific conversation.",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry traces              # Last 20 traces\n    \
            crow-cli telemetry traces -n 100       # Last 100 traces\n    \
            crow-cli telemetry traces -s abc123    # Traces for session\n    \
            crow-cli telemetry traces -j           # JSON for scripting"
    )]
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
    #[command(
        long_about = "Display complete details for a single LLM call.\n\n\
            Shows the full request (messages sent to the model) and response\n\
            (content and tool calls), along with timing and token usage.\n\
            Essential for debugging why the agent behaved a certain way.",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry trace abc123           # Summary with truncated content\n    \
            crow-cli telemetry trace abc123 --full    # Full untruncated output\n    \
            crow-cli telemetry trace abc123 --json    # Complete JSON for export\n\n\
            The output includes:\n    \
            - System prompt (rendered)\n    \
            - Request messages (full conversation sent to LLM)\n    \
            - Request tools (tool definitions)\n    \
            - Response content and tool calls\n    \
            - Model info, latency, token counts"
    )]
    Trace {
        /// Trace ID
        trace_id: String,

        /// Output as JSON (complete payload for scripting/export)
        #[arg(long, short = 'j')]
        json: bool,

        /// Show full content without truncation
        #[arg(long, short = 'f')]
        full: bool,
    },

    /// List traces from external agents (Claude Code, Gemini via ACP)
    #[command(
        long_about = "List traces captured from external agents like Claude Code or Gemini.\n\n\
            These are captured when you use Claude Code or Gemini through Zed's Agent Panel.\n\
            Stored separately from native crow-cli traces in ~/.local/share/crow/telemetry.db",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry external                      # Last 20 external traces\n    \
            crow-cli telemetry external -n 50                # Last 50 traces\n    \
            crow-cli telemetry external -s <session-id>      # Filter by session\n    \
            crow-cli telemetry external -j                   # JSON output"
    )]
    External {
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

    /// Show a specific external trace's full details
    #[command(
        long_about = "Display complete details for an external agent LLM call.\n\n\
            Shows the full request content sent to Claude Code/Gemini and the response.",
        after_help = "EXAMPLES:\n    \
            crow-cli telemetry external-trace abc123        # Summary\n    \
            crow-cli telemetry external-trace abc123 --full # Full output"
    )]
    ExternalTrace {
        /// Trace ID
        trace_id: String,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,

        /// Show full content without truncation
        #[arg(long, short = 'f')]
        full: bool,
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

        Some(Commands::Traces {
            limit,
            session,
            json,
        }) => run_telemetry_command(TelemetryCommands::Traces {
            limit,
            session,
            json,
        }),

        Some(Commands::Prompts { json }) => {
            run_telemetry_command(TelemetryCommands::Prompts { json })
        }

        Some(Commands::Login) => run_login(),
        Some(Commands::Logout) => run_logout(),
        Some(Commands::Status) => run_status(),
        Some(Commands::Acp { agent, message }) => run_acp(agent, message),

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

fn run_acp(agent: String, message: Option<String>) -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::acp::run_acp_command(agent, message, &mut cx).await;

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
                } => {
                    commands::sessions::run_show_session_command(session_id, json, last, &mut cx)
                        .await
                }
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

fn run_login() -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::login::run_login_command(&mut cx).await;

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

fn run_logout() -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::login::run_logout_command(&mut cx).await;

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

fn run_status() -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = commands::login::run_status_command(&mut cx).await;

            if let Err(e) = result {
                eprintln!("Error: {:#}", e);
            }

            std::process::exit(0);
        })
        .detach();
    });

    Ok(())
}

fn run_telemetry_command(command: TelemetryCommands) -> Result<()> {
    Application::headless().run(move |cx| {
        cx.spawn(async move |mut cx| {
            let result = match command {
                TelemetryCommands::Prompts { json } => {
                    commands::telemetry::list_prompts(json, &mut cx).await
                }
                TelemetryCommands::Prompt { prompt_id } => {
                    commands::telemetry::show_prompt(prompt_id, &mut cx).await
                }
                TelemetryCommands::Traces {
                    limit,
                    session,
                    json,
                } => commands::telemetry::list_traces(limit, session, json, &mut cx).await,
                TelemetryCommands::Trace {
                    trace_id,
                    json,
                    full,
                } => commands::telemetry::show_trace(trace_id, json, full, &mut cx).await,
                TelemetryCommands::External {
                    limit,
                    session,
                    json,
                } => commands::telemetry::list_external_traces(limit, session, json, &mut cx).await,
                TelemetryCommands::ExternalTrace {
                    trace_id,
                    json,
                    full,
                } => commands::telemetry::show_external_trace(trace_id, json, full, &mut cx).await,
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

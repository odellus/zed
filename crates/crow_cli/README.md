# crow-cli

CLI interface to Zed's native agent for headless operation and testing.

## Building

```bash
# Debug build
cargo build -p crow_cli
./target/debug/crow-cli --help

# Release build
cargo build -p crow_cli --release
./target/release/crow-cli --help
```

The binary is named `crow-cli`. Add to your PATH or create an alias:

```bash
alias crow-cli="/path/to/zed/target/debug/crow-cli"
```

## Usage

```bash
# One-shot question (resumes most recent session)
crow-cli "What does the Thread struct do?"

# Start fresh session
crow-cli chat -n "Implement feature X"

# Resume specific session
crow-cli chat -s abc123 "Continue"

# Auto mode (executor + discriminator dual-agent loop)
crow-cli chat --auto "Fix all type errors in agent.rs"

# Interactive REPL
crow-cli repl

# List sessions
crow-cli sessions

# View session history
crow-cli session show <session_id>

# View LLM call traces
crow-cli traces
crow-cli telemetry trace <trace_id>
```

Run `crow-cli --help` for full documentation with examples.

## Crate Structure

```
src/
├── crow_cli.rs      # Entry point and CLI definition (clap)
├── init.rs          # Initialization (database, templates, language models)
├── commands/        # Command implementations
│   ├── chat.rs      # Chat command and auto mode
│   ├── repl.rs      # Interactive REPL
│   ├── sessions.rs  # Session management
│   └── telemetry.rs # Prompt and trace inspection
└── render/          # Output formatting
    ├── mod.rs
    └── terminal.rs  # Terminal rendering helpers
```

## Key Dependencies

- `agent` - Core agent logic (Thread, Templates, ThreadsDatabase)
- `acp_thread` - Agent client protocol thread management
- `clap` - CLI argument parsing
- `rustyline` - REPL line editing

## Testing

Currently tested manually via the CLI. See `TEST_AGENTS.md` for test scenarios.

## See Also

- `AGENTS.md` - Guide for AI agents working with crow-cli
- `TEST_AGENTS.md` - Test scenarios and expected behaviors

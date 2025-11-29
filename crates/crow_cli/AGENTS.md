# Crow CLI - Agent Guide

This document provides guidance for AI agents (Claude Code, etc.) working with crow-cli.

## What is Crow?

Crow is a CLI interface to Zed's native agent. It allows you to:

1. **Run Zed's agent without the GUI** - Same model, same tools, pure command line
2. **Use auto mode** - Dual-agent loop where a discriminator reviews the executor's work
3. **Inspect telemetry** - Every LLM call is traced with full request/response data
4. **Manage sessions** - Resume, inspect, and debug agent conversations

## Quick Reference

```bash
# One-shot question (resumes most recent session)
crow-cli "What does the Thread struct do?"

# Start fresh session
crow-cli chat -n "Implement feature X"

# Resume specific session
crow-cli chat -s abc123 "Continue"

# Auto mode (executor + discriminator loop)
crow-cli chat --auto "Fix all type errors in agent.rs"

# Interactive REPL
crow-cli repl

# List sessions
crow-cli sessions

# View session messages
crow-cli session show abc123

# List recent LLM traces
crow-cli traces

# View full trace details
crow-cli telemetry trace <trace_id>

# List prompt templates
crow-cli prompts
```

## When to Use Crow

### Use crow-cli when:
- Testing changes to Zed's agent system
- Debugging agent behavior (check traces for what the model received)
- Running autonomous coding tasks with `--auto` mode
- Exploring how Zed's agent handles specific scenarios

### Don't use crow-cli when:
- You need GUI features (file preview, inline diffs, etc.)
- You're just editing code normally (use Claude Code directly)

## Architecture Overview

### Key Components

- **Thread** (`agent/src/thread.rs`) - Core conversation loop, manages message history and tool execution
- **Templates** (`agent/src/templates.rs`) - Handlebars prompt templates with version tracking
- **ThreadsDatabase** (`agent/src/db.rs`) - SQLite persistence for sessions, prompts, and traces

### Auto Mode

Auto mode uses a dual-agent pattern:
1. **Executor** - Main agent that performs the task
2. **Discriminator** - Reviewer that evaluates executor's work

The discriminator can request retries until satisfied. See `agent/src/discriminator.rs`.

### Telemetry

Every LLM call is recorded in the `traces` table with:
- Session and thread IDs
- Agent role (Executor, Discriminator, etc.)
- Full request messages
- Full response content and tool calls
- Token usage and latency

Prompts are versioned by content hash in the `prompts` table.

## Debugging Workflow

1. **Run your test case**
   ```bash
   crow-cli chat "Your test message"
   ```

2. **Check traces for the session**
   ```bash
   crow-cli traces -s <session_id>
   ```

3. **Inspect a specific LLM call**
   ```bash
   crow-cli telemetry trace <trace_id>
   ```
   This shows the exact messages sent to the model and what it returned.

4. **Check prompt template used**
   ```bash
   crow-cli telemetry prompt <prompt_id>
   ```

## File Locations

```
crates/crow_cli/
├── src/
│   ├── crow_cli.rs      # CLI definition (clap)
│   ├── init.rs          # Initialization (DB, templates, etc.)
│   ├── render/          # Output formatting
│   └── commands/        # Command implementations
│       ├── chat.rs      # Chat and auto mode
│       ├── repl.rs      # Interactive REPL
│       ├── sessions.rs  # Session management
│       └── telemetry.rs # Prompt and trace inspection
└── AGENTS.md            # This file

crates/agent/
├── src/
│   ├── thread.rs        # Core conversation loop
│   ├── templates.rs     # Prompt templates
│   ├── db.rs            # Database (sessions, prompts, traces)
│   └── discriminator.rs # Auto mode discriminator
└── prompts/             # Handlebars templates (.hbs files)
```

## Common Tasks

### Testing a prompt change

1. Edit the `.hbs` file in `crates/agent/prompts/`
2. Run crow-cli - templates are re-registered on startup
3. Check `crow-cli prompts` to see new version registered
4. Run your test, then inspect the trace to verify

### Debugging unexpected behavior

1. Get session ID from `crow-cli sessions`
2. List traces: `crow-cli traces -s <session_id>`
3. Find the problematic turn and inspect: `crow-cli telemetry trace <id>`
4. Check what messages were sent and what tools were called

### Testing auto mode

```bash
crow-cli chat --auto "Task description"
```

Watch the executor/discriminator loop. The discriminator will request retries
if unsatisfied. Check traces to see both agent roles' reasoning.

## Tips

- Use `-j` (JSON output) when parsing programmatically
- Use `-q` (quiet) in chat to skip streaming decorations
- Session IDs are prefixed substrings of UUIDs - you can use short prefixes
- Traces include latency - useful for performance debugging
- The `--new` flag ensures a fresh session without prior context

# Crow CLI in Zed - Complete Specification

## Executive Summary

This document specifies the implementation of `crow-cli`, a command-line interface for the Zed editor's agent system. The CLI uses GPUI's headless mode to run the full agent stack without a window, enabling rapid agent development and testing independent of the editor UI.

**Key Finding:** GPUI headless mode provides everything we need. The existing agent code runs unchanged—we just need initialization glue and terminal rendering.

**Naming:** This is the first step in rebranding the Zed agent codebase as Crow. The CLI binary will be `crow-cli`.

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         crow-cli                                │
├─────────────────────────────────────────────────────────────────┤
│  Terminal Renderer     │  Argument Parser   │  REPL Loop        │
│  (streaming output)    │  (commands/flags)  │  (readline)       │
├─────────────────────────────────────────────────────────────────┤
│                    GPUI Headless Runtime                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │ Entity<T>    │  │ Spawn/Tasks  │  │ Globals      │           │
│  │ Context<T>   │  │ Background   │  │ Settings     │           │
│  │ Observers    │  │ Foreground   │  │ Registry     │           │
│  └──────────────┘  └──────────────┘  └──────────────┘           │
├─────────────────────────────────────────────────────────────────┤
│                    Existing Zed Agent Stack                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │ NativeAgent  │  │ Thread       │  │ Tools        │           │
│  │ Session mgmt │  │ Messages     │  │ 12/13 work   │           │
│  │ Dual-agent   │  │ Model calls  │  │ headless     │           │
│  └──────────────┘  └──────────────┘  └──────────────┘           │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │ Project      │  │ LLM Registry │  │ ThreadsDB    │           │
│  │ Worktree     │  │ Providers    │  │ SQLite       │           │
│  │ Buffers      │  │ Auth (env)   │  │ Shared w/UI  │           │
│  └──────────────┘  └──────────────┘  └──────────────┘           │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. GPUI Headless Mode

### What's Available (Everything We Need)

| Capability | Status | Notes |
|------------|--------|-------|
| `Entity<T>` state management | ✅ | Full entity system |
| `cx.spawn()` async tasks | ✅ | Foreground + background |
| `cx.subscribe()` events | ✅ | Full observer pattern |
| `cx.set_global()` globals | ✅ | Settings, registry |
| `cx.emit()` events | ✅ | EventEmitter works |
| Background executor | ✅ | I/O, network, etc. |
| Timers/delays | ✅ | `executor.timer()` |

### What's NOT Available (Don't Need)

| Capability | Status | Impact |
|------------|--------|--------|
| Windows | ❌ | Not needed |
| Rendering | ❌ | We render to terminal |
| Clipboard | ❌ | N/A for CLI |
| Mouse/keyboard | ❌ | Use stdin/readline |

### Initialization

```rust
use gpui::Application;

fn main() {
    Application::headless().run(|cx| {
        // Full GPUI context available here
        // All entities, spawns, globals work
    });
}
```

**Platform Notes:**
- Linux: Uses `calloop` event loop (no X11/Wayland needed)
- macOS: Works without display server
- Set `ZED_HEADLESS=1` to force headless even with display

---

## 3. Project Initialization

### Minimal Dependencies

```rust
// Required for Project::local()
let fs = Arc::new(RealFs::new(None, cx.background_executor()));
let languages = Arc::new(LanguageRegistry::new(cx.background_executor()));
let http = Arc::new(HttpClientWithUrl::new(...));
let client = Client::new(clock, http, cx);
let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
```

### Project Creation

```rust
let project = Project::local(
    client,
    NodeRuntime::unavailable(),  // No node needed
    user_store,
    languages,
    fs,
    None,  // env vars
    cx,
);

// Mount current directory
let cwd = std::env::current_dir()?;
let (worktree, _) = project
    .update(cx, |project, cx| {
        project.find_or_create_worktree(&cwd, true, cx)
    })
    .await?;

// Wait for filesystem scan
worktree.read_with(cx, |tree, _| {
    tree.as_local().unwrap().scan_complete()
}).await;
```

### What Gets Initialized Automatically

All 13 service entities are created by `Project::local()`:
- WorktreeStore, BufferStore (needed for file ops)
- LspStore (can be inactive)
- GitStore (optional features)
- Others (minimal stubs OK)

---

## 4. NativeAgent Initialization

### Required Parameters

```rust
NativeAgent::new(
    project: Entity<Project>,           // From above
    history: Entity<HistoryStore>,      // Thread history
    templates: Arc<Templates>,          // System prompts
    prompt_store: Option<Entity<PromptStore>>,  // User rules (optional)
    fs: Arc<dyn Fs>,                    // Filesystem
    cx: &mut AsyncApp,                  // GPUI async context
) -> Result<Entity<NativeAgent>>
```

### Initialization Sequence

```rust
// 1. Templates (embedded, synchronous)
let templates = Templates::new();

// 2. HistoryStore (requires TextThreadStore)
let text_thread_store = cx.new(|cx| {
    TextThreadStore::new(project.clone(), cx)
});
let history = cx.new(|cx| {
    HistoryStore::new(text_thread_store, cx)
});

// 3. PromptStore (optional, for user rules)
let prompt_store = None;  // Or initialize from paths::prompts_dir()

// 4. Create agent (async)
let agent = NativeAgent::new(
    project,
    history,
    templates,
    prompt_store,
    fs,
    cx,
).await?;
```

### What Happens Inside

1. Builds `ProjectContext` from worktrees + rules files
2. Creates `ContextServerRegistry` for MCP tools
3. Initializes `LanguageModels` from registry
4. Spawns `maintain_project_context` background task
5. Subscribes to project/model/prompt events

---

## 5. Tool Compatibility

### Fully Compatible (No Changes)

| Tool | Dependencies | Notes |
|------|--------------|-------|
| `read_file` | Project, ActionLog | Works as-is |
| `grep` | Project | Full regex search |
| `find_path` | Project | Glob patterns |
| `list_directory` | Project | Directory listing |
| `diagnostics` | Project | LSP diagnostics (if LSP active) |
| `fetch` | HttpClient | Web fetching |
| `web_search` | WebSearchRegistry | Requires Zed Cloud |
| `create_directory` | Project | Trivial adaptation |
| `copy_path` | Project | Works as-is |
| `move_path` | Project | Works as-is |
| `delete_path` | Project, ActionLog | Works as-is |
| `now` | None | Timestamp utility |
| `thinking` | None | Extended thinking |

### Needs Work: `edit_file`

**Current:** Requires `Thread` entity for model access (EditAgent)

**Solution:** Pass model directly or initialize Thread first. The tool itself works headless.

### Needs Work: `terminal`

**Current:** Creates `Entity<acp_thread::Terminal>` via `ThreadEnvironment`

**Solution:** Implement `HeadlessTerminalHandle`:

```rust
struct HeadlessTerminalHandle {
    child: std::process::Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl TerminalHandle for HeadlessTerminalHandle {
    fn id(&self, _cx: &AsyncApp) -> Result<TerminalId> { ... }
    fn current_output(&self, _cx: &AsyncApp) -> Result<TerminalOutputResponse> { ... }
    fn wait_for_exit(&self, _cx: &AsyncApp) -> Result<Shared<Task<TerminalExitStatus>>> { ... }
}
```

Use `std::process::Command` to execute real shell commands.

---

## 6. Session Persistence

### Storage

- **Format:** SQLite with zstd-compressed JSON
- **Location:** `{data_dir}/db/0-{scope}/db.sqlite`
- **Table:** `threads` (id, summary, updated_at, data_type, data)

### Shared with Editor

**Yes!** CLI and editor share the same database:
- Same file path
- Same schema (version 0.3.0)
- SQLite WAL mode handles concurrent access
- Sessions created in CLI appear in editor and vice versa

### API

```rust
// Connect to database
let db = ThreadsDatabase::connect(cx).await?;

// List sessions
let threads = db.list_threads().await?;

// Load specific session
let thread_data = db.load_thread(session_id).await?;

// Save session
db.save_thread(session_id, thread_data).await?;
```

---

## 7. Language Model Authentication

### Environment Variable Support

The system checks environment variables first, synchronously:

| Provider | Environment Variable |
|----------|---------------------|
| Anthropic | `ANTHROPIC_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| Google | `GEMINI_API_KEY` or `GOOGLE_AI_API_KEY` |
| XAI | `XAI_API_KEY` |
| Moonshot | Provider-specific |
| Local (llama.cpp, Ollama, LM Studio) | Endpoint configuration |

### No UI Required

Set the appropriate environment variable for your provider and the CLI authenticates automatically. No prompts, no keychain access needed.

### Initialization

```rust
// Register all providers
language_models::init(user_store, client, cx);

// Get registry
let registry = LanguageModelRegistry::global(cx);

// Providers auto-authenticate from env vars or local endpoints
let available_models = registry.read(cx).available_models(cx);
```

---

## 8. Event Streaming

### ThreadEvent Types

```rust
pub enum ThreadEvent {
    UserMessage(UserMessage),           // User input captured
    AgentText(String),                  // Response text chunk
    AgentThinking(String),              // Thinking/reasoning chunk
    ToolCall(acp::ToolCall),            // Tool invocation start
    ToolCallUpdate(ToolCallUpdate),     // Tool progress/result
    ToolCallAuthorization(Auth),        // Permission request
    Retry(RetryStatus),                 // Auto-retry happening
    Stop(StopReason),                   // Turn complete
}
```

### Event Order

```
1. UserMessage (input captured)
2. AgentThinking* (reasoning, streamed)
3. AgentText* (response, streamed)
4. ToolCall (tool announced, Pending)
5. ToolCallUpdate (InProgress -> Completed)
6. [Model gets tool result, repeats 2-5]
7. Stop (EndTurn | MaxTokens | Refusal | Cancelled)
```

### Consumption Pattern

```rust
let mut events = thread.update(cx, |t, cx| {
    t.send(user_id, content, cx)
})?.await;

while let Some(event) = events.next().await {
    match event? {
        ThreadEvent::AgentText(text) => print!("{}", text),
        ThreadEvent::ToolCall(call) => println!("Tool: {}", call.title),
        ThreadEvent::Stop(reason) => break,
        // ...
    }
}
```

---

## 9. CLI Command Structure

### Commands

```
crow-cli chat "message"              # One-shot, verbose streaming
crow-cli chat --quiet "message"      # Just the response
crow-cli chat --json "message"       # Structured output
crow-cli chat --new "message"        # Force new session
crow-cli chat --session ID "msg"     # Use specific session

crow-cli repl [session-id]           # Interactive mode
crow-cli sessions                    # List all sessions
crow-cli session info <id>           # Session details
crow-cli new [title]                 # Create named session
```

### Session Auto-Resume

```rust
// Find most recent session in current directory
fn find_session_for_cwd(cwd: &Path, db: &ThreadsDatabase) -> Option<SessionId> {
    db.list_threads()
        .filter(|t| t.directory == cwd)
        .filter(|t| t.parent_id.is_none())  // Top-level only
        .max_by_key(|t| t.updated_at)
        .map(|t| t.id)
}
```

---

## 10. Terminal Rendering

### Color Scheme

```rust
const PURPLE: (u8, u8, u8) = (138, 43, 226);      // Headers, tools
const THINKING_PURPLE: (u8, u8, u8) = (147, 112, 219);  // Reasoning
const MINT_GREEN: (u8, u8, u8) = (0, 255, 170);   // Success
const LIME_GREEN: (u8, u8, u8) = (180, 255, 100); // Response text
```

### Output Structure

```
═══════════════════════════════════════════════════════════════
Session: sess-abc123 | Model: claude-sonnet-4
Working dir: /home/user/project
═══════════════════════════════════════════════════════════════

🔮 THINKING
[streamed reasoning in purple]

🟪 TOOL CALL: read_file
   Input: {"path": "src/main.rs"}

📖 read_file (45ms)
   -> src/main.rs
   <- 42 lines
   ✓

🟢 RESPONSE
[streamed response in green]

═══════════════════════════════════════════════════════════════
✓ ~150 thinking, ~200 response | 1 tool call | 2.3s
Cost: $0.02 | Context: 12k/128k (9.4%)
```

### Streaming Implementation

```rust
struct TerminalRenderer {
    mode: OutputMode,
    in_thinking: bool,
    in_response: bool,
    current_tool: Option<String>,
}

impl TerminalRenderer {
    fn handle_event(&mut self, event: ThreadEvent) {
        match event {
            ThreadEvent::AgentThinking(text) => {
                if !self.in_thinking {
                    eprintln!("\n🔮 THINKING");
                    self.in_thinking = true;
                }
                eprint!("{}", text.purple());
                std::io::stderr().flush().ok();
            }
            ThreadEvent::AgentText(text) => {
                if !self.in_response {
                    if self.in_thinking { eprintln!(); }
                    eprintln!("\n🟢 RESPONSE");
                    self.in_response = true;
                }
                print!("{}", text.green());
                std::io::stdout().flush().ok();
            }
            // ... tool calls, updates, stop
        }
    }
}
```

---

## 11. REPL Implementation

### Features

- **Readline:** Use `rustyline` with history persistence
- **History file:** `~/.local/state/crow/repl_history.txt`
- **Commands:** `/exit`, `/new`, `/session`, `/help`
- **Interrupt:** Type during execution to redirect agent

### Basic Loop

```rust
fn run_repl(session_id: Option<SessionId>, cx: &mut App) {
    let mut rl = Editor::new()?;
    rl.load_history(&history_path()).ok();
    
    loop {
        match rl.readline("crow> ") {
            Ok(line) => {
                if line.starts_with('/') {
                    handle_command(&line);
                } else {
                    send_message(&line, cx);
                }
                rl.add_history_entry(&line);
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => { eprintln!("Error: {}", e); break; }
        }
    }
    
    rl.save_history(&history_path()).ok();
}
```

---

## 12. Crate Structure

```
zed/crates/crow_cli/
├── Cargo.toml
├── src/
│   ├── main.rs              # Entry point, argument parsing
│   ├── commands/
│   │   ├── chat.rs          # One-shot chat command
│   │   ├── repl.rs          # Interactive REPL
│   │   ├── sessions.rs      # Session management
│   │   └── mod.rs
│   ├── render/
│   │   ├── terminal.rs      # Terminal renderer
│   │   ├── colors.rs        # Color definitions
│   │   ├── tools.rs         # Tool-specific formatters
│   │   └── mod.rs
│   ├── init.rs              # GPUI/Project/Agent setup
│   └── headless_terminal.rs # HeadlessTerminalHandle
```

### Cargo.toml

```toml
[package]
name = "crow_cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "crow-cli"
path = "src/main.rs"

[dependencies]
agent.workspace = true
gpui.workspace = true
project.workspace = true
language_model.workspace = true
language_models.workspace = true
fs.workspace = true
client.workspace = true
settings.workspace = true
db.workspace = true
paths.workspace = true

# CLI-specific
clap = { version = "4", features = ["derive"] }
rustyline = "14"
colored = "2"
```

---

## 13. THINGS YOU ARE NOT ALLOWED TO REWARD HACK

This section exists because the entire point of this CLI is to test the real agent against real backends. A CLI full of mocks is useless, harmful, and a waste of time.

| Topic | What You Must NOT Do |
|-------|---------------------|
| **Terminal Tool** | Do NOT create a fake/mock terminal. Do NOT skip terminal functionality. The `HeadlessTerminalHandle` MUST execute real shell commands via `std::process::Command`, capture real stdout/stderr, and return real exit codes. If a user says "run `cargo build`", cargo must actually build. |
| **LSP/Diagnostics** | Do NOT mock LSP responses. If LSP isn't running, `diagnostics_tool` returns empty results—that's fine and honest. Do NOT fabricate diagnostic data to make tests pass. |
| **Session Persistence** | Do NOT use in-memory databases for "convenience". Use the real `ThreadsDatabase` with real SQLite at the real path. Sessions MUST persist across CLI invocations and MUST be visible in the editor. |
| **LLM Providers** | Do NOT use `FakeLanguageModel` in production code. Do NOT mock API responses. The CLI MUST call real LLM APIs (Anthropic, OpenAI, Moonshot, local llama.cpp, whatever is configured). Use the real provider registry. |
| **Project/Filesystem** | Do NOT use `FakeFs`. Use `RealFs`. The agent MUST read and write real files on the real filesystem. If `edit_file` is called, the file MUST actually change on disk. |
| **GPUI Context** | Do NOT create a fake App context. Use real `Application::headless()`. The entity system, spawns, and event loop MUST be the real GPUI runtime. |
| **Tool Execution** | Do NOT stub tool results. When `grep` runs, it MUST actually search files. When `read_file` runs, it MUST actually read the file. No "return canned response to make the test green" bullshit. |

### The Test Smell Check

Before any test or implementation is considered "done", ask:

1. **If I run a command that takes 5 seconds, does it take 5 seconds?** — If no, you mocked the terminal.
2. **If I create a session and restart the CLI, is it still there?** — If no, you mocked persistence.
3. **If I `edit_file` and then `cat` the file in my shell, did it change?** — If no, you mocked the filesystem.
4. **If the API returns an error, do I see that error?** — If no, you swallowed it.
5. **If I configure a local model endpoint, does it use that endpoint?** — If no, you hardcoded providers.

### Why This Matters

The entire point of this CLI is to **test the real agent against real backends**. A CLI full of mocks is:
- Useless for development (can't trust the results)
- Useless for debugging (not reproducing real behavior)
- Actively harmful (gives false confidence)
- A waste of everyone's time

If something doesn't work headless, **fix it or document the limitation**. Do not paper over it with mocks.

---

## 14. Implementation Phases

### Phase 1: Minimal Chat (1-2 days)

1. Create `crow_cli` crate
2. Headless GPUI initialization
3. Project setup for cwd
4. NativeAgent initialization
5. Simple `chat "message"` command
6. Basic terminal streaming (no colors)
7. Real LLM provider auth

**Deliverable:** `crow-cli chat "Hello"` works with real model

### Phase 2: Full CLI (2-3 days)

1. Color rendering with tool formatters
2. Session management (list, resume, new)
3. Auto-resume by directory
4. Output modes (verbose/quiet/json)
5. `--session` and `--new` flags

**Deliverable:** Feature parity with crow's one-shot mode

### Phase 3: REPL (1-2 days)

1. Readline integration with history
2. `/commands` for session management
3. Interrupt handling (type to redirect)
4. Multi-turn conversation

**Deliverable:** Interactive REPL

### Phase 4: Polish (1-2 days)

1. HeadlessTerminalHandle for terminal tool (real `std::process::Command`)
2. Session inspection commands
3. Cost/token tracking display
4. Error handling and edge cases
5. Help text and documentation

**Deliverable:** Production-ready CLI

---

## 15. Resolved Questions

| Question | Resolution |
|----------|------------|
| Does GPUI headless work? | ✅ Yes, full entity system available |
| Can we share sessions with editor? | ✅ Yes, same SQLite database |
| Do tools work headless? | ✅ 12/13 work, terminal needs real process wrapper |
| How does auth work without UI? | ✅ Environment variables + provider config |
| Can we stream to terminal? | ✅ Same event system, custom renderer |

---

## Conclusion

The architecture is sound. GPUI headless mode provides everything we need, and the existing agent code requires zero modifications. The work is:

1. **Initialization glue** — Project, NativeAgent setup
2. **Terminal renderer** — event → colored output  
3. **CLI commands** — argument parsing, REPL
4. **One tool adapter** — HeadlessTerminalHandle with real `std::process::Command`

Total estimated effort: **5-8 days** for full feature parity with crow CLI.

No mocks. No fakes. Real agent, real tools, real files, real models.

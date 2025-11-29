# Prompt Management & Telemetry

This document describes how Zed's agent manages prompt templates and records telemetry for LLM calls. This system enables debugging, versioning, and observability of agent behavior.

## Overview

The agent uses two interconnected systems:

1. **Prompt Management**: Version-controlled prompt templates (`.hbs` files) tracked by content hash
2. **Telemetry/Tracing**: Full recording of every LLM call with request, response, and timing data

Both systems are accessible via the `crow-cli` command-line tool.

## Prompt Management

### How It Works

Prompt templates are Handlebars (`.hbs`) files in `crates/agent/prompts/`. At startup, each template is:

1. Loaded from the embedded assets
2. Hashed (content-based hash)
3. Registered in the `prompts` table

If the template content hasn't changed, the existing prompt ID is reused. If the content has changed, a new version is created with a new ID.

### Database Schema

```sql
CREATE TABLE prompts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,           -- Template filename (e.g., "agent.hbs")
    template_hash TEXT NOT NULL,  -- SHA hash of content
    template_content TEXT NOT NULL,
    input_schema TEXT,            -- Optional JSON schema for template variables
    created_at TEXT NOT NULL,
    UNIQUE(name, template_hash)   -- Same content = same version
);
```

### Key Insight

Templates are versioned by **content hash**, not by time or manual version numbers. This means:

- Changing a `.hbs` file creates a new version automatically
- Reverting to old content reuses the original version
- Traces reference specific prompt versions, so you always know exactly what prompt was used

### Code Structure

**`crates/agent/src/templates.rs`**:
```rust
pub struct Templates {
    handlebars: Handlebars<'static>,
    prompt_registry: RwLock<HashMap<String, PromptInfo>>,
}

pub struct PromptInfo {
    pub template_content: String,
    pub prompt_id: Option<String>,  // Set after DB registration
}

impl Templates {
    /// Register all templates with the database at startup
    pub async fn register_with_database(&self, db: &ThreadsDatabase) -> Result<()>;
    
    /// Get the prompt_id for a template (used when recording traces)
    pub fn get_prompt_id(&self, template_name: &str) -> Option<String>;
}
```

**`crates/agent/src/db.rs`**:
```rust
impl ThreadsDatabase {
    /// Register a prompt, returning existing ID if content matches
    pub fn register_prompt(
        &self,
        name: String,
        template_content: String,
        input_schema: Option<String>,
    ) -> Task<Result<String>>;
    
    pub fn get_prompt(&self, id: String) -> Task<Result<Option<Prompt>>>;
    pub fn list_prompts(&self) -> Task<Result<Vec<Prompt>>>;
}
```

### CLI Commands

```bash
# List all registered prompt templates
crow-cli prompts

# Output:
# ID              NAME                    HASH              CREATED
# abc123...       agent.hbs               a1b2c3d4e5f6...   2024-01-15T10:30:00Z
# def456...       discriminator.hbs       f6e5d4c3b2a1...   2024-01-15T10:30:00Z

# Show full content of a specific prompt
crow-cli telemetry prompt <prompt_id>
```

## Telemetry / Tracing

### How It Works

Every LLM call made by the agent is recorded as a **trace**. This happens in `thread.rs` around the `model.stream_completion()` call:

1. **Before the call**: Create a `TraceBuilder` with request info
2. **After the call**: Complete the trace with response data
3. **Async save**: Fire-and-forget save to database (doesn't block agent)

### What's Recorded

Each trace captures:

| Field | Description |
|-------|-------------|
| `session_id` | The conversation session |
| `thread_id` | Specific thread within session |
| `agent_role` | Executor, Discriminator, EditAgent, DiffJudge |
| `prompt_id` | Which prompt template version was used |
| `model_provider` | anthropic, openai, etc. |
| `model_id` | claude-3-5-sonnet, gpt-4, etc. |
| `request_messages` | Full messages sent to model (JSON) |
| `response_content` | Model's text response |
| `tool_calls` | Any tool calls made (JSON) |
| `input_tokens` | Tokens in request |
| `output_tokens` | Tokens in response |
| `latency_ms` | Time from request to complete response |
| `started_at` | Timestamp |

### Database Schema

```sql
CREATE TABLE traces (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_role TEXT NOT NULL,
    started_at TEXT NOT NULL,
    data TEXT NOT NULL  -- JSON blob with all trace fields
);

CREATE INDEX idx_traces_session ON traces(session_id);
CREATE INDEX idx_traces_started ON traces(started_at);
```

### Code Structure

**`crates/agent/src/db.rs`**:
```rust
pub enum AgentRole {
    Executor,
    Discriminator,
    EditAgent,
    DiffJudge,
}

pub struct TraceBuilder {
    session_id: String,
    agent_role: AgentRole,
    model_provider: String,
    model_id: String,
    request_messages: Vec<serde_json::Value>,
    started_at: DateTime<Utc>,
    // ... other fields
}

impl TraceBuilder {
    pub fn new(...) -> Self;
    
    pub fn with_prompt(self, prompt_id: Option<String>) -> Self;
    pub fn with_thread_id(self, thread_id: Option<String>) -> Self;
    pub fn with_tools(self, tools: Vec<String>) -> Self;
    
    pub fn complete(
        self,
        response_content: String,
        tool_calls: Vec<serde_json::Value>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    ) -> Trace;
}
```

**Recording in `thread.rs`**:
```rust
// Before LLM call
let trace_builder = TraceBuilder::new(
    session_id,
    AgentRole::Executor,
    model_provider,
    model_id,
    request_messages,
)
.with_thread_id(thread_id)
.with_prompt(prompt_id);

// Make the LLM call
let events = model.stream_completion(request, cx).await?;
// ... collect response ...

// After LLM call
let trace = trace_builder.complete(
    response_content,
    tool_calls,
    input_tokens,
    output_tokens,
);

// Fire-and-forget save
cx.background_executor().spawn(async move {
    db.save_trace(trace).await.log_err();
}).detach();
```

### CLI Commands

```bash
# List recent traces
crow-cli traces

# Output:
# ID          SESSION     ROLE        MODEL                   LATENCY   TOKENS
# abc123...   def456...   Executor    anthropic/claude-3-5    1234ms    1500/500

# Filter by session
crow-cli traces -s <session_id>

# Show full trace details (request + response)
crow-cli telemetry trace <trace_id>

# Output includes:
# - Full request messages sent to model
# - Complete response text
# - Tool calls made
# - Token counts
# - Latency
# - Which prompt template was used
```

## Debugging Workflow

### 1. Find the session

```bash
crow-cli sessions
```

### 2. List traces for that session

```bash
crow-cli traces -s <session_id>
```

### 3. Inspect a specific LLM call

```bash
crow-cli telemetry trace <trace_id>
```

This shows you exactly:
- What messages were sent to the model
- What the model responded
- Which prompt template version was used
- How long it took

### 4. Check the prompt template

```bash
crow-cli telemetry prompt <prompt_id>
```

This shows the full Handlebars template that was used.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        Agent Startup                             │
├─────────────────────────────────────────────────────────────────┤
│  1. Load .hbs templates from crates/agent/prompts/              │
│  2. Hash each template's content                                 │
│  3. Register with database (reuse existing if hash matches)     │
│  4. Store prompt_id in Templates.prompt_registry                │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        During LLM Call                          │
├─────────────────────────────────────────────────────────────────┤
│  1. Create TraceBuilder with request info + prompt_id          │
│  2. Call model.stream_completion()                              │
│  3. Collect response events                                      │
│  4. Complete trace with response data                           │
│  5. Async save to traces table                                  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        Database Tables                          │
├─────────────────────────────────────────────────────────────────┤
│  prompts:  id, name, template_hash, template_content, ...       │
│  traces:   id, session_id, agent_role, data (JSON blob), ...   │
│                                                                  │
│  Relationship: traces.data.prompt_id → prompts.id               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        CLI Access                               │
├─────────────────────────────────────────────────────────────────┤
│  crow-cli prompts              # List all prompt versions       │
│  crow-cli traces               # List recent LLM calls          │
│  crow-cli telemetry trace <id> # Full trace details             │
│  crow-cli telemetry prompt <id># Full prompt content            │
└─────────────────────────────────────────────────────────────────┘
```

## Files

| File | Purpose |
|------|---------|
| `crates/agent/prompts/*.hbs` | Handlebars prompt templates |
| `crates/agent/src/templates.rs` | Template loading and registry |
| `crates/agent/src/db.rs` | Database schema and operations |
| `crates/agent/src/thread.rs` | Trace recording around LLM calls |
| `crates/crow_cli/src/commands/telemetry.rs` | CLI commands |

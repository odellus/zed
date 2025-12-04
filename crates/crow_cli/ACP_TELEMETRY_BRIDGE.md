# ACP-to-Crow Telemetry Bridge

## Overview

This document describes the architecture for capturing Claude Code (and other ACP-based agents) sessions into crow's telemetry database, enabling the discriminator to watch and learn from Claude Code sessions.

## The Goal

**The discriminator watching Claude Code IS the force multiplier.**

When Claude Code runs via ACP in Zed, we want to:
1. Capture every prompt/response into crow's trace database
2. Feed traces to a local discriminator model
3. Enable the discriminator to inject guidance back into sessions

## Current State

### What Crow Has
- `ThreadsDatabase` with full trace schema (see `crates/agent/src/db.rs`)
- `Trace` struct capturing: session_id, thread_id, agent_role, model info, request/response content, tool calls, tokens, latency
- `TraceBuilder` for constructing traces around LLM calls
- `crow-cli traces` command for viewing traces

### What ACP Has  
- `AcpConnection` in `crates/agent_servers/src/acp.rs` - handles Claude Code communication
- `AcpThread` in `crates/acp_thread/src/acp_thread.rs` - manages conversation state
- `AgentTelemetry` trait in `crates/acp_thread/src/connection.rs` - telemetry interface
- Events for tool calls, messages, completions

### The Gap
ACP events flow through the Zed UI but don't get captured into crow's trace database. We need to bridge them.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Claude Code (subprocess)                 │
│                         via ACP                             │
└─────────────────────────────────────────────────────────────┘
                              │
                    acp::PromptRequest/Response
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      AcpConnection                          │
│              (crates/agent_servers/src/acp.rs)              │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │            NEW: AcpTraceCapture                     │   │
│  │                                                     │   │
│  │  on_prompt_request() → start TraceBuilder           │   │
│  │  on_prompt_response() → complete Trace              │   │
│  │  on_tool_call() → log to trace                      │   │
│  │  on_error() → fail Trace                            │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                              │
                        Trace objects
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    ThreadsDatabase                          │
│              (crates/agent/src/db.rs)                       │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  traces table (SQLite)                              │   │
│  │  - id, session_id, thread_id                        │   │
│  │  - agent_role (NEW: "external_claude_code")         │   │
│  │  - request_messages, response_content               │   │
│  │  - response_tool_calls                              │   │
│  │  - tokens, latency, timestamps                      │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                              │
                       Query traces
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    Discriminator                            │
│                                                             │
│  Input: Recent traces from external agent sessions          │
│  Process: Evaluate quality, detect issues, suggest fixes    │
│  Output: Feedback/guidance (future: inject into session)    │
│                                                             │
│  Model: Local (moonshot/llama) or API (anthropic)           │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Plan

### Phase 1: Capture ACP Events into Traces

**Files to modify:**
- `crates/agent_servers/src/acp.rs`
- `crates/agent/src/db.rs`

**New AgentRole variant:**
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentRole {
    Executor,
    Discriminator,
    EditAgent,
    DiffJudge,
    ExternalClaudeCode,  // NEW
    ExternalGemini,      // NEW (future)
}
```

**AcpConnection modifications:**

```rust
impl AcpConnection {
    // Add database handle
    database: Option<Arc<ThreadsDatabase>>,
    
    // Wrap prompt calls to capture traces
    fn prompt_with_trace(
        &self,
        params: acp::PromptRequest,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        let trace_builder = TraceBuilder::new(
            self.current_session_id(),
            AgentRole::ExternalClaudeCode,
            "claude-code".into(),
            params.model.clone().unwrap_or_default(),
            serde_json::to_string(&params.messages).unwrap_or_default(),
        );
        
        // Capture tools if present
        let trace_builder = if let Some(tools) = &params.tools {
            trace_builder.with_tools(serde_json::to_string(tools).unwrap_or_default())
        } else {
            trace_builder
        };
        
        let database = self.database.clone();
        let prompt_task = self.connection.prompt(params);
        
        cx.spawn(async move {
            match prompt_task.await {
                Ok(response) => {
                    // Build trace from response
                    let trace = trace_builder.complete(
                        extract_content(&response),
                        extract_tool_calls(&response),
                        response.usage.map(|u| u.input_tokens as i64),
                        response.usage.map(|u| u.output_tokens as i64),
                        None, // total
                    );
                    
                    // Save to database
                    if let Some(db) = database {
                        db.save_trace(trace).await.ok();
                    }
                    
                    Ok(response)
                }
                Err(e) => {
                    let trace = trace_builder.fail(e.to_string());
                    if let Some(db) = database {
                        db.save_trace(trace).await.ok();
                    }
                    Err(e)
                }
            }
        })
    }
}
```

### Phase 2: Discriminator Ingestion

**New module:** `crates/agent/src/discriminator_watcher.rs`

```rust
pub struct DiscriminatorWatcher {
    database: Arc<ThreadsDatabase>,
    discriminator_model: LanguageModel,
    poll_interval: Duration,
    last_checked_trace_id: Option<String>,
}

impl DiscriminatorWatcher {
    /// Start watching for new external agent traces
    pub fn start(&self, cx: &mut App) -> Task<()> {
        cx.spawn(async move {
            loop {
                // Get recent traces from external agents
                let traces = self.database
                    .list_traces_by_role(AgentRole::ExternalClaudeCode, 10)
                    .await?;
                
                // Skip already-reviewed traces
                let new_traces: Vec<_> = traces
                    .into_iter()
                    .filter(|t| self.last_checked_trace_id.as_ref()
                        .map(|id| &t.id > id)
                        .unwrap_or(true))
                    .collect();
                
                for trace in new_traces {
                    self.evaluate_trace(&trace).await?;
                    self.last_checked_trace_id = Some(trace.id.clone());
                }
                
                smol::Timer::after(self.poll_interval).await;
            }
        })
    }
    
    async fn evaluate_trace(&self, trace: &Trace) -> Result<DiscriminatorFeedback> {
        // Build prompt for discriminator
        let prompt = format!(
            "You are a code review discriminator. Evaluate this agent action:\n\n\
             Session: {}\n\
             Request: {}\n\
             Response: {}\n\
             Tool Calls: {:?}\n\n\
             Evaluate:\n\
             1. Did the agent accomplish the task correctly?\n\
             2. Were there any mistakes or inefficiencies?\n\
             3. What would you have done differently?\n\
             4. Score (1-10) for quality\n",
            trace.session_id,
            trace.request_messages,
            trace.response_content.as_deref().unwrap_or("(none)"),
            trace.response_tool_calls,
        );
        
        // Run discriminator
        let response = self.discriminator_model
            .complete(prompt)
            .await?;
        
        // Parse response into structured feedback
        DiscriminatorFeedback::parse(&response)
    }
}
```

### Phase 3: Feedback Injection (Future)

Once we have discriminator feedback, we can:

1. **Passive mode**: Log feedback for human review
2. **Advisory mode**: Show feedback in UI (like code review comments)
3. **Active mode**: Inject guidance into next prompt

For active mode, we'd extend ACP with a feedback mechanism:

```rust
// In AcpConnection
fn inject_discriminator_feedback(
    &self,
    session_id: &acp::SessionId,
    feedback: DiscriminatorFeedback,
    cx: &mut App,
) -> Task<Result<()>> {
    // Option 1: System message injection
    // Add feedback as a system message before next prompt
    
    // Option 2: Tool-based injection
    // Provide a "discriminator_feedback" tool the agent can call
    
    // Option 3: Direct modification
    // Modify the agent's context with feedback
}
```

## Data Flow Example

1. User runs Claude Code in Zed via ACP
2. Claude Code sends `acp::PromptRequest`
3. `AcpConnection::prompt_with_trace()` wraps the call
4. Creates `TraceBuilder` with request details
5. Forwards to actual Claude Code subprocess
6. Claude Code returns `acp::PromptResponse`
7. `TraceBuilder.complete()` creates `Trace`
8. `ThreadsDatabase::save_trace()` persists it
9. `DiscriminatorWatcher` picks up new trace
10. Local model evaluates the trace
11. Feedback stored/displayed/injected

## CLI Access

```bash
# List traces from Claude Code sessions
crow-cli traces --role external_claude_code

# Show specific trace
crow-cli trace show <trace_id>

# Start discriminator watcher
crow-cli watch --discriminator moonshot/kimi-k2

# View discriminator feedback
crow-cli feedback list
```

## Benefits

1. **Observability**: Full visibility into what Claude Code is doing
2. **Learning**: Discriminator learns your preferences from traces
3. **Quality Gate**: Catch mistakes before they compound
4. **Training Data**: Traces become training data for local models
5. **Debugging**: When something goes wrong, you have the full trace

## Open Questions

1. **Token cost**: Discriminator reviewing every trace could be expensive
   - Solution: Only review traces with tool calls, or sample
   
2. **Latency**: Don't want discriminator to slow down the agent
   - Solution: Async review, no blocking

3. **Feedback format**: How to structure discriminator output
   - Start simple: score + comments
   - Evolve: structured JSON with specific improvement suggestions

4. **Session boundaries**: When does a "session" start/end for Claude Code?
   - Use ACP session ID
   - Track across multiple prompts

## Timeline

- **Week 1**: Phase 1 - Capture traces (modify AcpConnection)
- **Week 2**: Phase 2 - Discriminator watcher (new module)
- **Week 3**: Phase 3 - UI integration and feedback display
- **Future**: Active feedback injection

## Success Criteria

1. All Claude Code prompts/responses captured in crow traces
2. `crow-cli traces` shows external agent activity
3. Discriminator can evaluate traces and produce feedback
4. Feedback visible in UI (at minimum, logs)

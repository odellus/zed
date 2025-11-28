# Zed Agent Tools System

This document explains how tools work in Zed's agent system, from definition through execution and result handling.

## Table of Contents

1. [Tool Definition](#tool-definition)
2. [Tool Registration](#tool-registration)
3. [Tool Filtering via Profiles](#tool-filtering-via-profiles)
4. [Sending Tools to Language Model](#sending-tools-to-language-model)
5. [Tool Call Detection and Handling](#tool-call-detection-and-handling)
6. [Tool Execution](#tool-execution)
7. [Result Handling](#result-handling)
8. [Complete Tool Call Lifecycle](#complete-tool-call-lifecycle)
9. [Code References](#code-references)

## Tool Definition

### The AgentTool Trait

All tools in Zed implement the `AgentTool` trait, defined in `/crates/agent/src/thread.rs` at line 2228. This trait defines the interface that every tool must provide:

```rust
pub trait AgentTool
where
    Self: 'static + Sized,
{
    type Input: for<'de> Deserialize<'de> + Serialize + JsonSchema;
    type Output: for<'de> Deserialize<'de> + Serialize + Into<LanguageModelToolResultContent>;
    
    fn name() -> &'static str;
    fn description() -> SharedString;
    fn kind() -> acp::ToolKind;
    fn initial_title(&self, input: Result<Self::Input, serde_json::Value>, cx: &mut App) -> SharedString;
    fn input_schema(format: LanguageModelToolSchemaFormat) -> Schema;
    fn supports_provider(_provider: &LanguageModelProviderId) -> bool;
    fn run(
        self: Arc<Self>,
        input: Self::Input,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output>>;
    fn replay(&self, _input: Self::Input, _output: Self::Output, _event_stream: ToolCallEventStream, _cx: &mut App) -> Result<()>;
    fn erase(self) -> Arc<dyn AnyAgentTool>;
}
```

**Key trait requirements:**

- **Input**: Must implement `Deserialize`, `Serialize`, and `JsonSchema` for tool input validation
- **Output**: Must be convertible into `LanguageModelToolResultContent` for returning to the model
- **name()**: Static method returning the tool's unique identifier (e.g., "read_file")
- **description()**: Returns the tool's documentation, typically from the JSON schema
- **kind()**: Returns the tool category (Read, Write, Execute, Other)
- **initial_title()**: Generates a user-friendly title for the tool call UI
- **input_schema()**: Generates JSON Schema describing the tool's input parameters
- **supports_provider()**: Allows tools to opt-out for certain language model providers
- **run()**: Main execution logic, async task returning the tool's output
- **replay()**: Optional logic for replaying past tool executions
- **erase()**: Type-erases the tool into `Arc<dyn AnyAgentTool>` for storage

### Example Tool Implementation

The `ReadFileTool` in `/crates/agent/src/tools/read_file_tool.rs` demonstrates a complete tool implementation:

**Tool Definition (line 50-65):**
```rust
pub struct ReadFileToolInput {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
}

pub struct ReadFileTool {
    project: Entity<Project>,
    action_log: Entity<ActionLog>,
}

impl ReadFileTool {
    pub fn new(project: Entity<Project>, action_log: Entity<ActionLog>) -> Self {
        Self { project, action_log }
    }
}
```

**AgentTool Implementation (line 72-93):**
```rust
impl AgentTool for ReadFileTool {
    type Input = ReadFileToolInput;
    type Output = LanguageModelToolResultContent;

    fn name() -> &'static str {
        "read_file"
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(&self, input: Result<Self::Input, serde_json::Value>, cx: &mut App) -> SharedString {
        // Generate a user-friendly title like "Read file `src/main.rs` (lines 1-50)"
        if let Ok(input) = input { /* format title */ } else { "Read file".into() }
    }

    fn run(self: Arc<Self>, input: Self::Input, event_stream: ToolCallEventStream, cx: &mut App) -> Task<Result<Self::Output>> {
        // Main execution: read file content, send updates via event_stream
        cx.spawn(async move |cx| {
            // Validate path, check permissions
            // Read file content
            // Update UI with progress
            // Return file content as LanguageModelToolResultContent::Text
        })
    }
}
```

### Type Erasure: AnyAgentTool

The `AnyAgentTool` trait at line 2298 provides a type-erased interface for storing and managing tools of different types:

```rust
pub trait AnyAgentTool {
    fn name(&self) -> SharedString;
    fn description(&self) -> SharedString;
    fn kind(&self) -> acp::ToolKind;
    fn initial_title(&self, input: serde_json::Value, _cx: &mut App) -> SharedString;
    fn input_schema(&self, format: LanguageModelToolSchemaFormat) -> Result<serde_json::Value>;
    fn supports_provider(&self, _provider: &LanguageModelProviderId) -> bool;
    fn run(
        self: Arc<Self>,
        input: serde_json::Value,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<AgentToolOutput>>;
    fn replay(
        &self,
        input: serde_json::Value,
        output: serde_json::Value,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Result<()>;
}
```

The `Erased<T>` type at line 2303 wraps generic tools and implements `AnyAgentTool` for them, allowing different tool types to be stored together in a `BTreeMap`.

## Tool Registration

### Adding Tools to Thread

Tools are added to a Thread through the `add_tool()` method at line 1034 in `/crates/agent/src/thread.rs`:

```rust
pub fn add_tool<T: AgentTool>(&mut self, tool: T) {
    self.tools.insert(T::name().into(), tool.erase());
}
```

This method:
1. Takes a concrete tool instance
2. Calls `erase()` to wrap it as `Arc<dyn AnyAgentTool>`
3. Stores it in `self.tools: BTreeMap<SharedString, Arc<dyn AnyAgentTool>>`

### Default Tools Setup

Default tools are registered when a thread is created for a session. In `/crates/agent/src/agent.rs` at line 309, when registering a session:

```rust
thread_handle.update(cx, |thread, cx| {
    thread.set_summarization_model(summarization_model, cx);
    thread.add_default_tools(
        Rc::new(AcpThreadEnvironment { acp_thread: acp_thread.downgrade() }) as _,
        cx,
    )
});
```

The `add_default_tools()` method is defined in `/crates/agent/src/thread.rs` at line 999. It creates and registers all built-in tools:

```rust
pub fn add_default_tools(&mut self, environment: Rc<dyn ThreadEnvironment>, cx: &mut Context<Self>) {
    let language_registry = self.project.read(cx).languages().clone();
    self.add_tool(CopyPathTool::new(self.project.clone()));
    self.add_tool(CreateDirectoryTool::new(self.project.clone()));
    self.add_tool(DeletePathTool::new(self.project.clone(), self.action_log.clone()));
    self.add_tool(DiagnosticsTool::new(self.project.clone()));
    self.add_tool(EditFileTool::new(
        self.project.clone(),
        cx.weak_entity(),
        language_registry,
        Templates::new(),
    ));
    self.add_tool(FetchTool::new(self.project.read(cx).client().http_client()));
    self.add_tool(FindPathTool::new(self.project.clone()));
    self.add_tool(GrepTool::new(self.project.clone()));
    self.add_tool(ListDirectoryTool::new(self.project.clone()));
    self.add_tool(MovePathTool::new(self.project.clone()));
    self.add_tool(NowTool);
    self.add_tool(OpenTool::new(self.project.clone()));
    self.add_tool(ReadFileTool::new(self.project.clone(), self.action_log.clone()));
    self.add_tool(TerminalTool::new(self.project.clone(), environment));
    self.add_tool(ThinkingTool);
    self.add_tool(WebSearchTool);
}
```

**Built-in tools include:**
- **File I/O**: ReadFileTool, EditFileTool, CreateDirectoryTool, DeletePathTool, CopyPathTool, MovePathTool
- **Search**: GrepTool, FindPathTool, WebSearchTool
- **Navigation**: OpenTool, ListDirectoryTool
- **System**: TerminalTool, DiagnosticsTool, FetchTool, NowTool
- **Thinking**: ThinkingTool

### Context Server Tools

In addition to built-in tools, the system supports dynamic tools from context servers. These are registered through `ContextServerRegistry`, which is checked in `enabled_tools()` at line 1925.

## Tool Filtering via Profiles

### Profile Structure

Profiles are defined in `/crates/agent_settings/src/agent_profile.rs`. The `AgentProfileSettings` struct at line 113 controls which tools are enabled:

```rust
pub struct AgentProfileSettings {
    pub name: SharedString,
    pub tools: IndexMap<Arc<str>, bool>,  // tool_name -> enabled
    pub enable_all_context_servers: bool,
    pub context_servers: IndexMap<Arc<str>, ContextServerPreset>,
    pub default_model: Option<LanguageModelSelection>,
    pub system_prompt: Option<Arc<str>>,
}
```

Each profile maintains:
- **tools**: A map of built-in tool names to their enabled status (true/false)
- **enable_all_context_servers**: Whether all context server tools are enabled
- **context_servers**: Per-context-server tool configurations

### Tool Enable/Disable Methods

Two methods determine if a tool should be enabled:

**For built-in tools (line 125):**
```rust
pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
    self.tools.get(tool_name) == Some(&true)
}
```

**For context server tools (line 129):**
```rust
pub fn is_context_server_tool_enabled(&self, server_id: &str, tool_name: &str) -> bool {
    self.enable_all_context_servers
        || self
            .context_servers
            .get(server_id)
            .is_some_and(|preset| preset.tools.get(tool_name) == Some(&true))
}
```

### Tool Filtering in Thread

The `enabled_tools()` method in `/crates/agent/src/thread.rs` at line 1925 combines all filtering logic:

```rust
fn enabled_tools(
    &self,
    profile: &AgentProfileSettings,
    model: &Arc<dyn LanguageModel>,
    cx: &App,
) -> BTreeMap<SharedString, Arc<dyn AnyAgentTool>> {
    // Filter built-in tools
    let mut tools = self
        .tools
        .iter()
        .filter_map(|(tool_name, tool)| {
            if tool.supports_provider(&model.provider_id())
                && profile.is_tool_enabled(tool_name)
            {
                Some((truncate(tool_name), tool.clone()))
            } else {
                None
            }
        })
        .collect::<BTreeMap<_, _>>();

    // Filter context server tools
    let mut context_server_tools = Vec::new();
    let mut seen_tools = tools.keys().cloned().collect::<HashSet<_>>();
    let mut duplicate_tool_names = HashSet::default();
    for (server_id, server_tools) in self.context_server_registry.read(cx).servers() {
        for (tool_name, tool) in server_tools {
            if profile.is_context_server_tool_enabled(&server_id.0, &tool_name) {
                let tool_name = truncate(tool_name);
                if !seen_tools.insert(tool_name.clone()) {
                    duplicate_tool_names.insert(tool_name.clone());
                }
                context_server_tools.push((server_id.clone(), tool_name, tool.clone()));
            }
        }
    }

    // Handle duplicate tool names by prefixing with server ID
    for (server_id, tool_name, tool) in context_server_tools {
        if duplicate_tool_names.contains(&tool_name) {
            let available = MAX_TOOL_NAME_LENGTH.saturating_sub(tool_name.len());
            if available >= 2 {
                let mut disambiguated = server_id.0.to_string();
                disambiguated.truncate(available - 1);
                disambiguated.push('_');
                disambiguated.push_str(&tool_name);
                tools.insert(disambiguated.into(), tool.clone());
            } else {
                tools.insert(tool_name, tool.clone());
            }
        } else {
            tools.insert(tool_name, tool.clone());
        }
    }

    tools
}
```

**Filtering logic:**
1. Check if tool supports the language model provider
2. Check if tool is enabled in the profile
3. Truncate tool names to MAX_TOOL_NAME_LENGTH (64 characters)
4. Add context server tools that are enabled
5. Handle naming conflicts between built-in and context server tools with prefixes

## Sending Tools to Language Model

### Building the Completion Request

When starting a turn, `run_turn()` at line 1194 calls `build_completion_request()` (implicit in the internal method) which includes the enabled tools.

The tools are converted to a format the language model API expects. In `/crates/agent/src/tools.rs` at line 47, a macro generates helper functions:

```rust
macro_rules! tools {
    ($($tool:ty),* $(,)?) => {
        pub fn built_in_tools() -> impl Iterator<Item = LanguageModelRequestTool> {
            fn language_model_tool<T: AgentTool>() -> LanguageModelRequestTool {
                LanguageModelRequestTool {
                    name: T::name().to_string(),
                    description: T::description().to_string(),
                    input_schema: T::input_schema(LanguageModelToolSchemaFormat::JsonSchema).to_value(),
                }
            }
            [ /* ... */ ].into_iter()
        }
    };
}
```

**LanguageModelRequestTool structure:**
- **name**: Tool identifier (e.g., "read_file")
- **description**: Tool documentation for the model
- **input_schema**: JSON Schema describing required and optional input parameters

### System Prompt Integration

Tools are included in the system prompt via the `SystemPromptTemplate` at line 1998 in `/crates/agent/src/thread.rs`:

```rust
let system_prompt = if let Some(custom) = custom_prompt {
    custom.to_string()
} else {
    SystemPromptTemplate {
        project: self.project_context.read(cx),
        available_tools,  // List of enabled tool names
        model_name: self.model.as_ref().map(|m| m.name().0.to_string()),
    }
    .render(&self.templates)
    .expect("Invalid template")
};
```

This provides the model with context about which tools are available and how to use them.

## Tool Call Detection and Handling

### Event Stream Processing

The `run_turn_internal()` async function at line 1247 processes the completion event stream:

```rust
let (mut events, mut error) = match model.stream_completion(request, cx).await {
    Ok(events) => (events, None),
    Err(err) => (stream::empty().boxed(), Some(err)),
};
let mut tool_results = FuturesUnordered::new();
while let Some(event) = events.next().await {
    log::trace!("Received completion event: {:?}", event);
    match event {
        Ok(event) => {
            tool_results.extend(this.update(cx, |this, cx| {
                this.handle_completion_event(event, event_stream, cx)
            })??);
        }
        Err(err) => {
            error = Some(err);
            break;
        }
    }
}
```

The stream contains `LanguageModelCompletionEvent` variants including `ToolUse` events.

### Tool Use Event Handling

The `handle_completion_event()` method at line 1406 delegates to `handle_tool_use_event()` for `ToolUse` variants:

```rust
fn handle_completion_event(
    &mut self,
    event: LanguageModelCompletionEvent,
    event_stream: &ThreadEventStream,
    cx: &mut Context<Self>,
) -> Result<Option<Task<LanguageModelToolResult>>> {
    match event {
        LanguageModelCompletionEvent::ToolUse(tool_use) => {
            return Ok(self.handle_tool_use_event(tool_use, event_stream, cx));
        }
        // ... handle other events (Text, Thinking, etc.)
    }
}
```

### Tool Use Validation

In `handle_tool_use_event()` at line 1529, the tool use event is validated:

```rust
fn handle_tool_use_event(
    &mut self,
    tool_use: LanguageModelToolUse,
    event_stream: &ThreadEventStream,
    cx: &mut Context<Self>,
) -> Option<Task<LanguageModelToolResult>> {
    // Get tool instance by name
    let tool = self.tool(tool_use.name.as_ref());
    
    // Generate UI title and kind
    let mut title = SharedString::from(&tool_use.name);
    let mut kind = acp::ToolKind::Other;
    if let Some(tool) = tool.as_ref() {
        title = tool.initial_title(tool_use.input.clone(), cx);
        kind = tool.kind();
    }

    // Track pending tool use in pending message
    let last_message = self.pending_message();
    let push_new_tool_use = last_message.content.last_mut().is_none_or(|content| {
        if let AgentMessageContent::ToolUse(last_tool_use) = content {
            if last_tool_use.id == tool_use.id {
                *last_tool_use = tool_use.clone();
                false
            } else {
                true
            }
        } else {
            true
        }
    });

    if push_new_tool_use {
        event_stream.send_tool_call(
            &tool_use.id,
            &tool_use.name,
            title,
            kind,
            tool_use.input.clone(),
        );
        last_message.content.push(AgentMessageContent::ToolUse(tool_use.clone()));
    }

    // If input is still streaming, don't execute yet
    if !tool_use.is_input_complete {
        return None;
    }

    // Return error if tool doesn't exist
    let Some(tool) = tool else {
        let content = format!("No tool named {} exists", tool_use.name);
        return Some(Task::ready(LanguageModelToolResult {
            content: LanguageModelToolResultContent::Text(Arc::from(content)),
            tool_use_id: tool_use.id,
            tool_name: tool_use.name,
            is_error: true,
            output: None,
        }));
    };

    // Schedule tool execution
    let fs = self.project.read(cx).fs().clone();
    let tool_event_stream = ToolCallEventStream::new(tool_use.id.clone(), event_stream.clone(), Some(fs));
    // ... execute tool
}
```

**Key validation steps:**
1. Look up tool by name
2. Generate UI-friendly title and category
3. Track tool use in pending agent message
4. Wait for input to be complete (not streaming)
5. Verify tool exists (error if not)
6. Create a tool event stream for communication

## Tool Execution

### Spawning Tool Task

Continuing from `handle_tool_use_event()` at line 1529, the tool is executed:

```rust
tool_event_stream.update_fields(acp::ToolCallUpdateFields {
    status: Some(acp::ToolCallStatus::InProgress),
    ..Default::default()
});

let supports_images = self.model().is_some_and(|model| model.supports_images());
let tool_result = tool.run(tool_use.input, tool_event_stream, cx);
log::debug!("Running tool {}", tool_use.name);

Some(cx.foreground_executor().spawn(async move {
    let tool_result = tool_result.await.and_then(|output| {
        if let LanguageModelToolResultContent::Image(_) = &output.llm_output
            && !supports_images
        {
            return Err(anyhow!(
                "Attempted to read an image, but this model doesn't support it.",
            ));
        }
        Ok(output)
    });

    match tool_result {
        Ok(output) => LanguageModelToolResult {
            tool_use_id: tool_use.id,
            tool_name: tool_use.name,
            is_error: false,
            content: output.llm_output,
            output: Some(output.raw_output),
        },
        Err(error) => LanguageModelToolResult {
            tool_use_id: tool_use.id,
            tool_name: tool_use.name,
            is_error: true,
            content: LanguageModelToolResultContent::Text(Arc::from(error.to_string())),
            output: None,
        },
    }
}))
```

**Execution flow:**
1. Mark tool as InProgress
2. Call tool's `run()` method (returns a Task)
3. Wrap in async block to handle completion
4. Check if image output is supported (some models don't support images)
5. Convert tool output into `LanguageModelToolResult`
6. Return task that yields the result

### Tool Input Deserialization

Inside the tool's `run()` implementation (in the `Erased<T>` wrapper at line 2365):

```rust
fn run(
    self: Arc<Self>,
    input: serde_json::Value,
    event_stream: ToolCallEventStream,
    cx: &mut App,
) -> Task<Result<AgentToolOutput>> {
    cx.spawn(async move |cx| {
        // Deserialize JSON input to concrete type T::Input
        let input = serde_json::from_value(input)?;
        
        // Call the actual tool's run method
        let output = cx
            .update(|cx| self.0.clone().run(input, event_stream, cx))?
            .await?;
        
        // Serialize output back to JSON for storage
        let raw_output = serde_json::to_value(&output)?;
        
        Ok(AgentToolOutput {
            llm_output: output.into(),
            raw_output,
        })
    })
}
```

### Tool Event Communication

During execution, tools communicate progress via `ToolCallEventStream` (defined at line 2485):

```rust
pub struct ToolCallEventStream {
    tool_use_id: LanguageModelToolUseId,
    stream: ThreadEventStream,
    fs: Option<Arc<dyn Fs>>,
}

impl ToolCallEventStream {
    pub fn update_fields(&self, fields: acp::ToolCallUpdateFields) {
        self.stream.update_tool_call_fields(&self.tool_use_id, fields);
    }

    pub fn update_diff(&self, diff: Entity<acp_thread::Diff>) {
        // Send diff updates (for incremental edits)
    }

    pub fn authorize(&self, title: impl Into<String>, cx: &mut App) -> Task<Result<()>> {
        // Request user authorization for destructive actions
    }
}
```

The event stream allows tools to:
- Update UI fields (status, title, content, locations)
- Send diffs for large file operations
- Request user authorization for destructive operations

## Result Handling

### Collecting Tool Results

Back in `run_turn_internal()` at line 1247, tool execution tasks are collected and awaited:

```rust
let mut tool_results = FuturesUnordered::new();
// ... spawn tool execution tasks into tool_results

let end_turn = tool_results.is_empty();
while let Some(tool_result) = tool_results.next().await {
    log::debug!("Tool finished {:?}", tool_result);

    // Update tool call with final status and output
    event_stream.update_tool_call_fields(
        &tool_result.tool_use_id,
        acp::ToolCallUpdateFields {
            status: Some(if tool_result.is_error {
                acp::ToolCallStatus::Failed
            } else {
                acp::ToolCallStatus::Completed
            }),
            raw_output: tool_result.output.clone(),
            ..Default::default()
        },
    );
    
    // Add result to pending message
    this.update(cx, |this, _cx| {
        this.pending_message()
            .tool_results
            .insert(tool_result.tool_use_id.clone(), tool_result);
    })?;
}
```

### Pending Message Structure

Tool results are stored in the pending agent message. The `AgentMessage` struct (line 568) contains:

```rust
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub content: Vec<AgentMessageContent>,
    pub tool_results: IndexMap<LanguageModelToolUseId, LanguageModelToolResult>,
}
```

The `tool_results` map stores results keyed by their tool use ID.

### Converting Results to Model Request

When building the next request, results are converted to model format via `AgentMessage::to_request()` (line 414):

```rust
pub fn to_request(&self) -> Vec<LanguageModelRequestMessage> {
    let mut assistant_message = LanguageModelRequestMessage {
        role: Role::Assistant,
        content: Vec::with_capacity(self.content.len()),
        cache: false,
    };
    
    // Add assistant's tool use calls
    for chunk in &self.content {
        match chunk {
            AgentMessageContent::ToolUse(tool_use) => {
                if self.tool_results.contains_key(&tool_use.id) {
                    assistant_message
                        .content
                        .push(language_model::MessageContent::ToolUse(tool_use.clone()));
                }
            }
            // ... handle other content types
        }
    }

    let mut user_message = LanguageModelRequestMessage {
        role: Role::User,
        content: Vec::new(),
        cache: false,
    };

    // Add tool results in a user message following the assistant's tool uses
    for tool_result in self.tool_results.values() {
        let mut tool_result = tool_result.clone();
        if tool_result.content.is_empty() {
            tool_result.content = "<Tool returned an empty string>".into();
        }
        user_message
            .content
            .push(language_model::MessageContent::ToolResult(tool_result));
    }

    let mut messages = Vec::new();
    if !assistant_message.content.is_empty() {
        messages.push(assistant_message);
    }
    if !user_message.content.is_empty() {
        messages.push(user_message);
    }
    messages
}
```

**Result message structure:**
1. Assistant message containing the tool use calls
2. User message containing all tool results (from the system)
3. This follows the standard LLM API pattern for tool use

### Turn Loop

The `run_turn_internal()` function implements a loop at line 1247:

```rust
loop {
    // 1. Build request with current messages and available tools
    let request = this.update(cx, |this, cx| this.build_completion_request(intent, cx))??;

    // 2. Stream completion from model
    let (mut events, mut error) = match model.stream_completion(request, cx).await {
        Ok(events) => (events, None),
        Err(err) => (stream::empty().boxed(), Some(err)),
    };

    // 3. Process completion events and collect tool execution tasks
    let mut tool_results = FuturesUnordered::new();
    while let Some(event) = events.next().await {
        match event {
            Ok(event) => {
                tool_results.extend(this.update(cx, |this, cx| {
                    this.handle_completion_event(event, event_stream, cx)
                })??);
            }
            Err(err) => {
                error = Some(err);
                break;
            }
        }
    }

    // 4. Wait for all tools to complete
    let end_turn = tool_results.is_empty();
    while let Some(tool_result) = tool_results.next().await {
        // ... collect results
    }

    // 5. Flush pending message and update UI
    this.update(cx, |this, cx| {
        this.flush_pending_message(cx);
        if this.title.is_none() && this.pending_title_generation.is_none() {
            this.generate_title(cx);
        }
    })?;

    // 6. Handle errors with retry logic
    if let Some(error) = error {
        attempt += 1;
        let retry = this.update(cx, |this, cx| {
            let user_store = this.user_store.read(cx);
            this.handle_completion_error(error, attempt, user_store.plan())
        })??;
        let timer = cx.background_executor().timer(retry.duration);
        event_stream.send_retry(retry);
        timer.await;
        // ... prepare for retry
    } else if end_turn {
        // No more tool calls, turn is complete
        return Ok(());
    } else {
        // More tool calls to make, continue loop with ToolResults intent
        intent = CompletionIntent::ToolResults;
        attempt = 0;
    }
}
```

**Loop termination conditions:**
- **end_turn = true**: No tools were called, exit with success
- **error**: Connection/API error, retry with exponential backoff
- **neither**: Tools were called, continue loop to get model's next response with tool results

## Complete Tool Call Lifecycle

Here's the complete flow from user prompt to tool execution to model response:

### 1. User Submits Message

```
User Message → Thread.push_user_message()
                ↓
              Run Turn Started
```

### 2. Request Building

```
Thread.run_turn() 
  ↓
Thread.build_completion_request()
  ↓
enabled_tools(profile, model)  // Filter tools by profile and provider
  ↓
built_in_tools() + context_server_tools()  // Get tool definitions
  ↓
LanguageModelRequest { tools, messages, system_prompt }
```

### 3. Model Stream Processing

```
model.stream_completion(request)
  ↓
Stream<LanguageModelCompletionEvent>
  ├─ Text("Beginning to...") → add to pending message
  ├─ Text(" analyze...") → append to pending message
  ├─ ToolUse { name: "read_file", input: {...}, id: "abc123" }
  │   ↓
  │   handle_tool_use_event()
  │   ├─ Validate tool exists
  │   ├─ Create ToolCallEventStream
  │   ├─ Schedule execution task
  │   └─ Return Task<LanguageModelToolResult>
  ├─ ToolUse { name: "grep_tool", ... } → schedule more tools
  └─ Stop(EndTurn) → end of response
```

### 4. Tool Execution (Concurrent)

```
Tool 1 (read_file)           Tool 2 (grep_tool)
  ├─ Deserialize input        ├─ Deserialize input
  ├─ Validate path            ├─ Validate pattern
  ├─ Update UI (InProgress)   ├─ Update UI (InProgress)
  ├─ Read file content        ├─ Search files
  ├─ Send diffs/updates       ├─ Report matches
  └─ Return ToolResult        └─ Return ToolResult
```

### 5. Result Collection

```
All tools complete
  ↓
Collect all LanguageModelToolResult items
  ↓
Add to AgentMessage.tool_results map
  ↓
Flush pending message to message history
```

### 6. Next Request (with Tool Results)

```
AgentMessage.to_request()
  ├─ Assistant message: [Text("..."), ToolUse("read_file", ...), ToolUse("grep_tool", ...)]
  └─ User message: [ToolResult("read_file", "...content..."), ToolResult("grep_tool", "...matches...")]
  ↓
model.stream_completion(request_with_results)
  ├─ Model analyzes results
  ├─ May call more tools or generate final response
  └─ Loop continues or ends
```

### 7. Turn Completion

```
Model response with no tool calls
  ↓
run_turn_internal() exits loop
  ↓
Thread updates UI with final response
  ↓
Messages persisted to history
```

## Code References

### Core Tool System Files

| File | Purpose | Key Lines |
|------|---------|-----------|
| `/crates/agent/src/thread.rs` | Tool trait and Thread implementation | 2228 (AgentTool), 1034 (add_tool), 1194 (run_turn), 1247 (run_turn_internal), 1406 (handle_completion_event), 1529 (handle_tool_use_event), 1925 (enabled_tools), 2298 (AnyAgentTool) |
| `/crates/agent/src/tools.rs` | Tool macro and built-in tool registry | 47 (tools macro), 68 (built_in_tools), 47 (supported_built_in_tool_names) |
| `/crates/agent_settings/src/agent_profile.rs` | Profile configuration and tool filtering | 113 (AgentProfileSettings), 125 (is_tool_enabled), 129 (is_context_server_tool_enabled) |

### Tool Implementation Examples

| File | Tool | Purpose |
|------|------|---------|
| `/crates/agent/src/tools/read_file_tool.rs` | ReadFileTool | Read file with line ranges, handles images |
| `/crates/agent/src/tools/edit_file_tool.rs` | EditFileTool | Edit file with diffs, multiple edit modes |
| `/crates/agent/src/tools/terminal_tool.rs` | TerminalTool | Run terminal commands, stream output |
| `/crates/agent/src/tools/grep_tool.rs` | GrepTool | Search files with regex patterns |
| `/crates/agent/src/tools/fetch_tool.rs` | FetchTool | HTTP requests with browser headers |
| `/crates/agent/src/tools/thinking_tool.rs` | ThinkingTool | Extended reasoning without generating output |

### Supporting Structures

| Structure | File | Purpose |
|-----------|------|---------|
| `LanguageModelCompletionEvent` | language_model crate | Events from model stream (Text, ToolUse, etc.) |
| `LanguageModelRequestTool` | language_model crate | Tool definition sent to model API |
| `ToolCallEventStream` | `/crates/agent/src/thread.rs` (2485) | Bidirectional communication during tool execution |
| `AgentMessage` | `/crates/agent/src/thread.rs` (568) | Agent's response with tool uses and results |
| `LanguageModelToolResult` | language_model crate | Result from tool execution |

## Profile System Example

Here's how profiles control tool availability:

```json
{
  "agent": {
    "profiles": {
      "write": {
        "name": "Write",
        "tools": {
          "read_file": true,
          "edit_file": true,
          "create_directory": true,
          "delete_path": true,
          "copy_path": true,
          "move_path": true,
          "terminal": true,
          "grep_tool": true,
          "list_directory": true,
          "find_path": true,
          "open_tool": true,
          "diagnostics_tool": true,
          "fetch_tool": true,
          "now_tool": true,
          "thinking_tool": true,
          "web_search_tool": true
        },
        "enable_all_context_servers": true
      },
      "ask": {
        "name": "Ask",
        "tools": {
          "read_file": true,
          "grep_tool": true,
          "list_directory": true,
          "find_path": true,
          "open_tool": true,
          "diagnostics_tool": true,
          "fetch_tool": true,
          "now_tool": true,
          "thinking_tool": true,
          "web_search_tool": true
        },
        "enable_all_context_servers": false
      },
      "minimal": {
        "name": "Minimal",
        "tools": {
          "read_file": true,
          "thinking_tool": true
        },
        "enable_all_context_servers": false
      }
    }
  }
}
```

The "write" profile enables all tools, "ask" restricts to read-only, and "minimal" only allows reading files and thinking.

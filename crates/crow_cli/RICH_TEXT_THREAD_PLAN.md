# Rich Text Thread with Tool Calling

## Vision

Transform Text Thread into a **Notion-style rich text editor for agent conversations**. Full tool calling support, beautifully rendered, but still just markdown underneath. Portable, editable, git-diffable.

The agent conversation becomes a living document you can edit, share, and replay.

## Current State

### What Text Thread Has
- WYSIWYG markdown editing (ligatures, unicode substitutions like `<-` → `←`)
- Message headers as custom blocks (User/Assistant labels)
- Crease system for foldable sections (slash command output)
- Full buffer editability
- Lamport clocks for versioning

### What Text Thread Lacks
- Tool call handling (explicitly no-op'd in `assist()`)
- Tool call rendering
- Tool result tracking
- Agentic loop integration

### What crow-cli Already Does
```
◀ ASSISTANT [3]
Now let me create that todo list for you:
  🔧 todo_write (todo_write:0)
     {
       "todos": [
         {
           "content": "Learn Rust",
           "status": "pending",
     ...
  ✅ LanguageModelToolUseId("todo_write:0")
```

This serialization format is the bridge. Same data, different rendering.

---

## Architecture

### The Key Insight

**Tool calls are just fenced code blocks with special rendering.**

Underlying markdown:
````markdown
## Assistant

I'll look at that file.

```tool:read_file:toolu_abc123
{"path": "src/main.rs"}
```

```tool-result:toolu_abc123
fn main() {
    println!("Hello, world!");
}
```

Found the issue on line 42...
````

Rendered view:
```
┌─────────────────────────────────────────┐
│ 🔧 read_file                    ✓ Done  │
├─────────────────────────────────────────┤
│ path: src/main.rs                       │
├─────────────────────────────────────────┤
│ ▶ Output (click to expand)              │
└─────────────────────────────────────────┘
```

### Data Flow

```
User sends message
       ↓
TextThread.assist() streams response
       ↓
LanguageModelCompletionEvent::ToolUse received
       ↓
[NEW] Insert tool call fenced block into buffer
       ↓
[NEW] Track pending tool call in HashMap
       ↓
[NEW] Execute tool asynchronously
       ↓
[NEW] Insert tool result fenced block
       ↓
[NEW] Resume model with tool results
       ↓
Editor renders fenced blocks as widgets
```

---

## Implementation Plan

### Phase 1: Tool Call Serialization Format

**Goal**: Define the markdown format for tool calls

**Files to modify**:
- `crates/assistant_text_thread/src/text_thread.rs`

**Format**:
````markdown
```tool:<tool_name>:<tool_use_id>
<json_input>
```

```tool-result:<tool_use_id>:<status>
<output_content>
```
````

**Example**:
````markdown
```tool:edit_file:toolu_xyz789
{
  "path": "src/main.rs",
  "operations": [
    {"type": "insert", "line": 10, "content": "// Fixed!"}
  ]
}
```

```tool-result:toolu_xyz789:success
File edited successfully. 1 insertion made.
```
````

**Parsing**: Add regex/parser to detect these blocks in the buffer.

---

### Phase 2: Tool Call Data Structures

**Goal**: Track tool calls within TextThread

**Add to `text_thread.rs`**:
```rust
#[derive(Clone, Debug)]
pub struct TextThreadToolCall {
    pub id: LanguageModelToolUseId,
    pub name: Arc<str>,
    pub input: serde_json::Value,
    pub status: ToolCallStatus,
    pub output: Option<ToolCallOutput>,
    pub block_range: Range<language::Anchor>,
    pub result_range: Option<Range<language::Anchor>>,
}

#[derive(Clone, Debug)]
pub enum ToolCallStatus {
    Pending,
    Running,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct ToolCallOutput {
    pub content: String,
    pub is_error: bool,
}

// Add to TextThread struct:
pub struct TextThread {
    // ... existing fields ...
    tool_calls: HashMap<LanguageModelToolUseId, TextThreadToolCall>,
    pending_tool_executions: FuturesUnordered<BoxFuture<'static, ToolExecutionResult>>,
}
```

---

### Phase 3: Event Handling in assist()

**Goal**: Process tool use events instead of ignoring them

**Modify `assist()` in `text_thread.rs`**:

```rust
LanguageModelCompletionEvent::ToolUse(tool_use) => {
    // 1. Serialize tool call to markdown
    let tool_block = format!(
        "```tool:{}:{}\n{}\n```\n",
        tool_use.name,
        tool_use.id.0,
        serde_json::to_string_pretty(&tool_use.input).unwrap_or_default()
    );
    
    // 2. Insert into buffer at current position
    this.buffer.update(cx, |buffer, cx| {
        let offset = buffer.len();
        buffer.edit([(offset..offset, tool_block)], None, cx);
    });
    
    // 3. Track the tool call
    let anchor_start = /* get anchor at block start */;
    let anchor_end = /* get anchor at block end */;
    
    this.tool_calls.insert(tool_use.id.clone(), TextThreadToolCall {
        id: tool_use.id.clone(),
        name: tool_use.name.clone(),
        input: tool_use.input.clone(),
        status: ToolCallStatus::Pending,
        output: None,
        block_range: anchor_start..anchor_end,
        result_range: None,
    });
    
    // 4. Emit event for UI
    cx.emit(TextThreadEvent::ToolCallInserted {
        tool_call_id: tool_use.id.clone(),
    });
    
    // 5. Spawn tool execution
    let execution = this.execute_tool(tool_use, cx);
    this.pending_tool_executions.push(execution);
}
```

---

### Phase 4: Tool Execution

**Goal**: Run tools and capture results

**Add to `text_thread.rs`**:

```rust
impl TextThread {
    fn execute_tool(
        &mut self,
        tool_use: LanguageModelToolUse,
        cx: &mut Context<Self>,
    ) -> BoxFuture<'static, ToolExecutionResult> {
        // Get the tool from registry
        let tool = self.tool_registry.get(&tool_use.name);
        
        // Execute asynchronously
        async move {
            match tool {
                Some(tool) => {
                    let result = tool.run(tool_use.input, cx).await;
                    ToolExecutionResult {
                        tool_use_id: tool_use.id,
                        output: result,
                    }
                }
                None => {
                    ToolExecutionResult {
                        tool_use_id: tool_use.id,
                        output: Err(anyhow!("Unknown tool: {}", tool_use.name)),
                    }
                }
            }
        }.boxed()
    }
    
    fn handle_tool_result(
        &mut self,
        result: ToolExecutionResult,
        cx: &mut Context<Self>,
    ) {
        // 1. Update tool call status
        if let Some(tool_call) = self.tool_calls.get_mut(&result.tool_use_id) {
            tool_call.status = match &result.output {
                Ok(_) => ToolCallStatus::Success,
                Err(_) => ToolCallStatus::Error,
            };
            tool_call.output = Some(ToolCallOutput {
                content: result.output.unwrap_or_else(|e| e.to_string()),
                is_error: result.output.is_err(),
            });
        }
        
        // 2. Insert result block into buffer
        let result_block = format!(
            "```tool-result:{}:{}\n{}\n```\n",
            result.tool_use_id.0,
            if result.output.is_ok() { "success" } else { "error" },
            result.output.unwrap_or_else(|e| e.to_string())
        );
        
        self.buffer.update(cx, |buffer, cx| {
            let offset = buffer.len();
            buffer.edit([(offset..offset, result_block)], None, cx);
        });
        
        // 3. Continue the model with tool results
        self.continue_with_tool_results(cx);
    }
}
```

---

### Phase 5: Rich Rendering with Creases

**Goal**: Render tool blocks as beautiful widgets

**Modify `text_thread_editor.rs`**:

```rust
impl TextThreadEditor {
    fn insert_tool_call_creases(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tool_calls = self.text_thread.read(cx).tool_calls.clone();
        
        self.editor.update(cx, |editor, cx| {
            for (id, tool_call) in tool_calls {
                // Create crease for tool call block
                let crease = Crease::inline(
                    tool_call.block_range.clone(),
                    FoldPlaceholder {
                        render: Arc::new(move |_, _, _| {
                            self.render_tool_call_placeholder(&tool_call)
                        }),
                        merge_adjacent: false,
                    },
                    self.render_tool_call_toggle(&tool_call),
                    self.render_tool_call_expanded(&tool_call),
                );
                
                editor.insert_creases(vec![crease], cx);
            }
        });
    }
    
    fn render_tool_call_placeholder(
        &self,
        tool_call: &TextThreadToolCall,
    ) -> AnyElement {
        let status_icon = match tool_call.status {
            ToolCallStatus::Pending => "⏳",
            ToolCallStatus::Running => "🔄",
            ToolCallStatus::Success => "✅",
            ToolCallStatus::Error => "❌",
        };
        
        h_flex()
            .gap_2()
            .child(div().child(format!("🔧 {}", tool_call.name)))
            .child(div().child(status_icon))
            .into_any()
    }
    
    fn render_tool_call_expanded(
        &self,
        tool_call: &TextThreadToolCall,
    ) -> AnyElement {
        v_flex()
            .p_2()
            .bg(cx.theme().colors().surface_background)
            .border_1()
            .border_color(cx.theme().colors().border)
            .rounded_md()
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new(format!("🔧 {}", tool_call.name)))
                    .child(self.status_badge(&tool_call.status))
            )
            .child(
                div()
                    .mt_2()
                    .p_2()
                    .bg(cx.theme().colors().editor_background)
                    .rounded_sm()
                    .child(self.render_json_input(&tool_call.input))
            )
            .children(tool_call.output.as_ref().map(|output| {
                div()
                    .mt_2()
                    .p_2()
                    .bg(if output.is_error {
                        cx.theme().colors().error_background
                    } else {
                        cx.theme().colors().success_background
                    })
                    .rounded_sm()
                    .child(Label::new(&output.content))
            }))
            .into_any()
    }
}
```

---

### Phase 6: Message Reconstruction for Model

**Goal**: Include tool results when sending next message

**Modify `to_completion_request()` in `text_thread.rs`**:

```rust
fn to_completion_request(&self, cx: &App) -> LanguageModelRequest {
    let mut messages = Vec::new();
    
    // Parse buffer into messages
    for message in self.messages() {
        match message.role {
            Role::User => {
                messages.push(/* user message */);
            }
            Role::Assistant => {
                let mut content = Vec::new();
                
                // Add text content
                content.push(MessageContent::Text(message.text.clone()));
                
                // Add tool uses from this message
                for tool_call in self.tool_calls_in_range(&message.range) {
                    content.push(MessageContent::ToolUse(LanguageModelToolUse {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        input: tool_call.input.clone(),
                    }));
                }
                
                messages.push(LanguageModelRequestMessage {
                    role: Role::Assistant,
                    content,
                    cache: false,
                });
                
                // Add tool results as separate user message (per Anthropic format)
                let tool_results: Vec<_> = self.tool_calls_in_range(&message.range)
                    .filter_map(|tc| tc.output.as_ref().map(|o| (tc, o)))
                    .collect();
                
                if !tool_results.is_empty() {
                    messages.push(LanguageModelRequestMessage {
                        role: Role::User,
                        content: vec![MessageContent::ToolResults(
                            tool_results.into_iter().map(|(tc, output)| {
                                LanguageModelToolResult {
                                    tool_use_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    is_error: output.is_error,
                                    content: output.content.clone(),
                                }
                            }).collect()
                        )],
                        cache: true,
                    });
                }
            }
            _ => {}
        }
    }
    
    LanguageModelRequest { messages, ..default() }
}
```

---

### Phase 7: Persistence & Sync

**Goal**: Save/load text threads with tool calls intact

The markdown format handles this automatically:
- Tool calls are fenced code blocks → saved as text
- Load file → parse fenced blocks → reconstruct tool_calls HashMap
- Git-friendly, diff-friendly, copy-paste friendly

**Add parsing on load**:
```rust
fn parse_tool_blocks(&mut self, cx: &mut Context<Self>) {
    let content = self.buffer.read(cx).text();
    
    // Regex to find tool blocks
    let tool_re = Regex::new(r"```tool:(\w+):(\S+)\n([\s\S]*?)```").unwrap();
    let result_re = Regex::new(r"```tool-result:(\S+):(\w+)\n([\s\S]*?)```").unwrap();
    
    for cap in tool_re.captures_iter(&content) {
        let name = &cap[1];
        let id = &cap[2];
        let input: serde_json::Value = serde_json::from_str(&cap[3]).unwrap_or_default();
        
        // Reconstruct tool call
        self.tool_calls.insert(
            LanguageModelToolUseId(id.into()),
            TextThreadToolCall { /* ... */ }
        );
    }
    
    // Match results to tool calls
    for cap in result_re.captures_iter(&content) {
        let id = &cap[1];
        let status = &cap[2];
        let output = &cap[3];
        
        if let Some(tool_call) = self.tool_calls.get_mut(&LanguageModelToolUseId(id.into())) {
            tool_call.status = match status {
                "success" => ToolCallStatus::Success,
                "error" => ToolCallStatus::Error,
                _ => ToolCallStatus::Pending,
            };
            tool_call.output = Some(ToolCallOutput {
                content: output.to_string(),
                is_error: status == "error",
            });
        }
    }
}
```

---

## File Changes Summary

| File | Changes |
|------|---------|
| `assistant_text_thread/src/text_thread.rs` | Add tool call structs, event handling, execution, serialization |
| `agent_ui/src/text_thread_editor.rs` | Add crease rendering for tool calls |
| `assistant_text_thread/src/lib.rs` | Export new types |

---

## Testing Strategy

1. **Unit tests**: Tool call serialization/parsing roundtrip
2. **Integration tests**: Full flow from user message → tool execution → result rendering
3. **Manual testing**: 
   - Create text thread, trigger tool call
   - Verify rendering looks correct
   - Edit document around tool blocks
   - Save/reload → verify tool calls persist
   - Copy-paste tool blocks between documents

---

## Future Enhancements

1. **Tool approval UI**: Button to approve/reject before execution
2. **Streaming tool output**: Show output as it streams
3. **Tool call editing**: Edit tool input JSON inline, re-run
4. **Diff view for edit_file**: Show before/after in tool result
5. **Terminal embedding**: Inline terminal for command tools
6. **Image rendering**: Show images from tool results inline

---

## Timeline

- **Phase 1-2**: 1 day (data structures and format)
- **Phase 3-4**: 2 days (event handling and execution)
- **Phase 5**: 2 days (rich rendering)
- **Phase 6**: 1 day (message reconstruction)
- **Phase 7**: 1 day (persistence)

**Total: ~1 week for MVP**

---

## The Payoff

Once this works:
- **Notion-style agent chat** with beautiful tool rendering
- **Fully editable history** - change context, re-run tools
- **Portable markdown** - copy whole conversations, git version them
- **Bridge to crow-cli** - same format, different UI
- **Foundation for collaborative agent sessions** - multiple users editing same agent thread

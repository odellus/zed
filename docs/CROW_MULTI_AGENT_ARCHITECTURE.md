# Crow Multi-Agent Architecture

This document explains how agents are currently defined in the Zed codebase (soon to be Crow), identifies limitations for multi-agent patterns, and proposes architectural changes to support GEPA-style prompt evolution and flexible agent orchestration.

## Current Architecture: How Agents Are Defined

### The Profile → Template → Thread Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           AGENT DEFINITION FLOW                             │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  AgentProfileId ──────► AgentProfileSettings ──────► Thread                 │
│  ("write", "ask",       - name                       - tools: BTreeMap      │
│   "discriminator")      - tools: IndexMap<bool>      - model: Arc<dyn LM>   │
│                         - default_model              - messages: Vec        │
│                         - prompt_template            - pending_message      │
│                                                                             │
│                              │                                              │
│                              ▼                                              │
│                                                                             │
│                      PromptTemplate enum                                    │
│                      - Default → system_prompt.hbs                          │
│                      - Discriminator → discriminator_prompt.hbs             │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Files

| Component | File | Description |
|-----------|------|-------------|
| Profile IDs | `crates/agent_settings/src/agent_profile.rs:16` | `builtin_profiles` module defines WRITE, ASK, MINIMAL, DISCRIMINATOR |
| Profile Settings | `crates/settings/src/settings_content/agent.rs:166` | `AgentProfileContent` struct with tools, model, prompt_template |
| Prompt Templates | `crates/settings/src/settings_content/agent.rs:188` | `PromptTemplate` enum (Default, Discriminator) |
| Template Files | `crates/agent/src/templates/*.hbs` | Handlebars templates for system prompts |
| Thread | `crates/agent/src/thread.rs` | The actual runtime agent with tools, messages, model |

### How Discriminator Gets Defined

1. **Profile ID**: `builtin_profiles::DISCRIMINATOR` = `"discriminator"`

2. **Profile maps to template** (in `PromptTemplate::template_name()`):
   ```rust
   PromptTemplate::Discriminator => "discriminator_prompt.hbs"
   ```

3. **Thread created with profile** (in `agent.rs:432-438`):
   ```rust
   let discriminator_profile_id = AgentProfileId(builtin_profiles::DISCRIMINATOR.into());
   thread.set_profile(discriminator_profile_id, cx);
   
   // Manually add exclusive tool
   thread.add_tool(crate::tools::TaskCompleteTool);
   ```

4. **Template rendered** (`discriminator_prompt.hbs`):
   - Describes discriminator's review role
   - Documents when to call `task_complete` vs provide feedback
   - Inherits standard code block formatting rules

### Tool Registration Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           TOOL REGISTRATION                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Thread::add_default_tools()         Thread::add_tool<T: AgentTool>()       │
│  ├── CopyPathTool                    └── Inserts into self.tools BTreeMap   │
│  ├── CreateDirectoryTool                                                    │
│  ├── DeletePathTool                  AgentTool trait:                       │
│  ├── DiagnosticsTool                 ├── name() -> &'static str             │
│  ├── EditFileTool                    ├── description() -> SharedString      │
│  ├── FetchTool                       ├── kind() -> ToolKind                 │
│  ├── FindPathTool                    ├── input_schema() -> Schema           │
│  ├── GrepTool                        ├── run() -> Task<Result<Output>>      │
│  ├── ListDirectoryTool               └── erase() -> Arc<dyn AnyAgentTool>   │
│  ├── MovePathTool                                                           │
│  ├── NowTool                         enabled_tools() filters by:            │
│  ├── OpenTool                        ├── profile.is_tool_enabled(name)      │
│  ├── ReadFileTool                    └── tool.supports_provider(provider)   │
│  ├── TerminalTool                                                           │
│  ├── ThinkingTool                                                           │
│  └── WebSearchTool                                                          │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Limitations for Multi-Agent Patterns

### 1. Prompt Templates Are an Enum

```rust
pub enum PromptTemplate {
    #[default]
    Default,
    Discriminator,
}
```

**Problem**: Adding a new agent type requires:
- Adding a variant to `PromptTemplate`
- Creating a `.hbs` file
- Recompiling

**Impact**: Can't create agents at runtime, can't A/B test prompts, can't evolve prompts with GEPA.

### 2. No First-Class Agent Definition

Agents are an emergent property of:
- A profile ID (string)
- Settings lookup (profile → tools, model, template)
- Manual tool additions (TaskCompleteTool added in code)

**Problem**: No single entity represents "what is this agent?"

### 3. Tools Hardcoded Per-Profile

```rust
// In enable_dual_agent_mode:
thread.add_tool(crate::tools::TaskCompleteTool);  // Manual addition
```

**Problem**: Exclusive tools (only discriminator has `task_complete`) are scattered in orchestration code, not declarative.

### 4. Sessions Don't Track Agent Identity

```rust
pub struct Session {
    thread: Entity<Thread>,
    acp_thread: WeakEntity<AcpThread>,
    _subscriptions: Vec<Subscription>,
    pending_save: Task<()>,
}
```

**Problem**: Session doesn't know:
- Which agent definition it's running
- Which prompt version/candidate is active
- Its role in multi-agent orchestration

### 5. Traces Lack Agent Metadata

Current trace schema:
```sql
CREATE TABLE traces (
    id TEXT PRIMARY KEY,
    session_id TEXT,
    agent_role TEXT,  -- "Executor" or "Discriminator" (string, not FK)
    prompt_id TEXT,   -- Links to prompts table
    ...
);
```

**Problem**: `agent_role` is a string, not linked to a proper agent definition. Can't query "all traces for discriminator-v3 prompt".

## Proposed Architecture: Agents as Data

### New Core Types

```rust
/// A first-class agent definition - the "what" of an agent
pub struct AgentDefinition {
    pub id: AgentId,
    pub name: String,
    
    // Prompt management (content-addressed, evolvable)
    pub prompt_family: PromptFamilyId,  // "executor", "discriminator", "planner"
    pub active_prompt: ContentHash,      // Current prompt version (or A/B selected)
    
    // Tool configuration (declarative, not scattered)
    pub tool_policy: ToolPolicy,
    pub exclusive_tools: Vec<ToolId>,    // Tools ONLY this agent has
    
    // Model preferences
    pub model_selector: ModelSelector,
    
    // Multi-agent relationships
    pub role: AgentRole,
    pub reports_to: Option<AgentId>,     // For hierarchical orchestration
}

pub enum AgentRole {
    Executor,       // Does work, has write tools
    Discriminator,  // Reviews work, has task_complete
    Planner,        // Decomposes tasks, read-only
    Critic,         // Provides feedback, no tools
    Specialist(String),  // Domain-specific
}

pub enum ToolPolicy {
    AllDefault,                          // All default tools enabled
    AllowList(HashSet<ToolId>),          // Only these tools
    DenyList(HashSet<ToolId>),           // All except these
    Custom(IndexMap<ToolId, bool>),      // Explicit per-tool
}
```

### Updated Session

```rust
pub struct Session {
    pub id: SessionId,
    
    // Agent identity (NEW)
    pub agent_id: AgentId,
    pub prompt_candidate_id: CandidateId,  // For A/B tracking
    
    // Existing
    pub thread: Entity<Thread>,
    pub acp_thread: WeakEntity<AcpThread>,
    _subscriptions: Vec<Subscription>,
    pending_save: Task<()>,
}
```

### Updated Trace Schema

```sql
-- Agent definitions (NEW)
CREATE TABLE agent_definitions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    prompt_family_id TEXT NOT NULL,
    role TEXT NOT NULL,
    tool_policy JSONB,
    exclusive_tools JSONB,
    reports_to TEXT REFERENCES agent_definitions(id),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Prompt candidates for GEPA (NEW)
CREATE TABLE prompt_candidates (
    id TEXT PRIMARY KEY,
    family_id TEXT NOT NULL,
    content_hash TEXT NOT NULL REFERENCES prompts(content_hash),
    parent_hash TEXT REFERENCES prompts(content_hash),
    generation INTEGER DEFAULT 0,
    mutation_rationale TEXT,
    pareto_rank INTEGER,
    is_active BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Updated traces (MODIFIED)
ALTER TABLE traces ADD COLUMN agent_id TEXT REFERENCES agent_definitions(id);
ALTER TABLE traces ADD COLUMN prompt_candidate_id TEXT REFERENCES prompt_candidates(id);
```

## Migration Path

### Phase 1: Agent Definitions (Non-Breaking)

1. Create `AgentDefinition` struct alongside existing profiles
2. Builtin profiles (`write`, `ask`, `discriminator`) get corresponding `AgentDefinition` entries
3. Session gains optional `agent_id` field
4. Traces gain optional `agent_id` field

### Phase 2: Content-Addressed Prompts (Non-Breaking)

1. Templates remain as `.hbs` files but get hashed on load
2. `PromptTemplate` enum becomes `ContentHash` reference
3. New templates can be added via database without recompile
4. A/B assignment logic in `Thread::select_prompt()`

### Phase 3: GEPA Evolution (Additive)

1. `prompt_candidates` table tracks Pareto front
2. `GEPAEvolver` runs periodically on collected traces
3. New candidates promoted to A/B rotation automatically
4. Dashboard shows prompt lineage and performance

### Phase 4: Declarative Multi-Agent (Breaking)

1. `ToolPolicy` replaces scattered `add_tool()` calls
2. `reports_to` enables hierarchical orchestration
3. `DualAgentOrchestrator` reads from `AgentDefinition` instead of hardcoding
4. New agent types defined in database, not code

## Dual-Agent Mode: Before and After

### Current (Hardcoded)

```rust
// In enable_dual_agent_mode:
let discriminator_profile_id = AgentProfileId(builtin_profiles::DISCRIMINATOR.into());
thread.set_profile(discriminator_profile_id, cx);
thread.add_tool(crate::tools::TaskCompleteTool);  // Manual!
```

### Proposed (Declarative)

```rust
// AgentDefinition for discriminator (in database or config)
AgentDefinition {
    id: "discriminator-v1",
    name: "Discriminator",
    prompt_family: "discriminator",
    active_prompt: ContentHash("abc123..."),
    tool_policy: ToolPolicy::AllowList(hashset!["task_complete", "read_file"]),
    exclusive_tools: vec!["task_complete"],
    role: AgentRole::Discriminator,
    reports_to: None,
}

// In enable_dual_agent_mode:
let discriminator_def = self.agent_registry.get("discriminator-v1")?;
let session = self.create_session_from_definition(discriminator_def, cx)?;
```

## GEPA Integration Points

### Metrics Collection

From existing telemetry:
- `task_complete` call rate (discriminator approval)
- Feedback loop count before approval
- User thumbs up/down on final output
- Time to completion

### Evolution Triggers

```rust
impl GEPAEvolver {
    /// Called periodically or on-demand
    pub async fn evolve(&mut self, cx: &mut AsyncApp) -> Result<()> {
        // 1. Gather recent traces for each prompt family
        let traces = self.storage.recent_traces_by_family("discriminator", 100)?;
        
        // 2. Compute metrics per candidate
        let metrics = self.compute_metrics(&traces);
        
        // 3. Update Pareto front
        self.front.update(metrics);
        
        // 4. Select ancestor and mutate
        let ancestor = self.front.select_ancestor();
        let failures = traces.iter().filter(|t| !t.succeeded()).collect();
        let new_candidate = self.reflect_and_mutate(ancestor, &failures).await?;
        
        // 5. Add to A/B rotation
        self.storage.add_candidate(new_candidate, is_active: true)?;
        
        Ok(())
    }
}
```

### Reflection Prompt

```handlebars
You are optimizing a {{agent_role}} prompt for the Crow agent system.

## Current Prompt
```
{{{current_prompt}}}
```

## Failure Traces ({{failure_count}} samples)
{{#each failures}}
### Trace {{@index}}
- Task: {{this.task_description}}
- Outcome: {{this.outcome}}
- Feedback: {{this.discriminator_feedback}}
{{#if this.vision_analysis}}
- Vision: {{this.vision_analysis.observations}}
{{/if}}
{{/each}}

## Your Task
1. Identify the pattern causing these failures
2. Propose a revised prompt that addresses this pattern
3. Explain your reasoning

Respond with:
- RATIONALE: Why these failures occurred
- REVISED_PROMPT: The complete updated prompt
```

## Vision Integration (Qwen3-VL-30B)

Vision analysis becomes another trace field:

```rust
pub struct Trace {
    // ... existing fields
    pub vision_analysis: Option<VisionAnalysis>,
}

pub struct VisionAnalysis {
    pub screenshot_hash: ContentHash,
    pub observations: Vec<String>,      // "Login button visible", "Error modal present"
    pub ui_state: serde_json::Value,    // Structured extraction
    pub confidence: f32,
}
```

GEPA uses vision observations as richer failure context - the prompt evolution happens on text prompts, but informed by visual feedback.

## Summary: What Changes

| Component | Current | Proposed |
|-----------|---------|----------|
| Agent identity | Emergent (profile + scattered code) | First-class `AgentDefinition` |
| Prompt templates | Enum + `.hbs` files | Content-addressed, database-stored |
| Tool assignment | Manual `add_tool()` calls | Declarative `ToolPolicy` |
| Session metadata | Just thread reference | Includes `agent_id`, `prompt_candidate_id` |
| Traces | String `agent_role` | FK to `agent_definitions` |
| Prompt evolution | Manual editing | GEPA-automated with Pareto front |
| Multi-agent patterns | Hardcoded dual-agent | Declarative `reports_to` relationships |

The key insight: **Agents should be data, not code**. This enables runtime creation, A/B testing, automated evolution, and flexible orchestration patterns.

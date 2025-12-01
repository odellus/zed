# ZED Agent System Overview

This document provides a comprehensive overview of the ZED agent system architecture and implementation. The agent system enables the ZED editor to perform complex code editing tasks through an intelligent, multi-component architecture that combines language models, tool execution, and project context management.

## Architecture Overview

The ZED agent system is built around several key components:

1. **NativeAgent**: The main agent implementation that coordinates all agent operations
2. **Thread System**: Manages individual conversation threads with language models
3. **Tool System**: Provides access to editor and system operations (file system, terminals, etc.)
4. **Language Model Integration**: Connects to various LLM providers
5. **Project Context System**: Maintains awareness of the current project state

## Core Components

### 1. NativeAgent

The `NativeAgent` is the central coordinator for all agent operations. It manages:
- Session management (individual threads)
- Dual-agent mode (executor-discriminator architecture)
- Project context
- Language model configuration
- Tool registration

Key features:
- Handles both single-agent and dual-agent modes
- Maintains project context that informs language model responses
- Provides access to file system tools through the tool system
- Manages session persistence and state

### 2. Thread System

The thread system manages individual conversation sessions with language models. Each thread represents a distinct conversation context and has:
- A unique session ID
- Conversation history
- Model configuration
- Tool registration
- State persistence

The system supports multiple thread types:
- Regular editing threads
- Dual-agent threads (executor and discriminator)
- Specialized threads for tasks like code summarization

### 3. Tool System

The ZED agent system exposes a rich set of tools for interacting with the editor and system:

**File System Tools**
- `edit_file`: Edit or create files with intelligent code editing
- `create_directory`: Create new directories
- `delete_path`: Delete files or directories
- `copy_path`: Copy files or directories
- `move_path`: Move files or directories

**Search and Navigation Tools**
- `find_path`: Search for files by pattern
- `grep`: Search file contents
- `list_directory`: List directory contents

**System Tools**
- `terminal`: Execute shell commands
- `open`: Open files or URLs
- `read_file`: Read file contents
- `fetch`: Fetch data from URLs
- `web_search`: Perform web searches
- `now`: Get current timestamp

**Agent Control Tools**
- `task_complete`: Signal completion of a task in dual-agent mode
- `thinking_tool`: Enable or disable agent thinking

### 4. Language Model Integration

The agent system integrates with multiple language model providers through a unified interface:

**Provider Architecture**
- `LanguageModelRegistry`: Central registry for all LLM providers
- `LanguageModelProvider`: Abstract interface for provider-specific functionality
- `LanguageModel`: Core interface for language model operations

**Supported Providers**
- ZED Cloud (in-house)
- OpenAI
- Anthropic
- Google
- OpenRouter
- X AI

**Model Configuration**
- Supports multiple models with different capabilities
- Configurable default models for various tasks (editing, summarization, etc.)
- Model selection persisted across sessions
- Authentication management for all providers

### 5. Project Context System

The agent maintains awareness of the current project state through a comprehensive context system:

**Context Components**
- Project tree structure
- File system rules (`.clinerules`, `.gitignore`, etc.)
- User-defined rules and configurations
- Language-specific settings

**Context Updates**
- Automatic updates when files change
- Context refresh triggered by project events
- Background context loading for improved performance

## Dual-Agent Mode

The dual-agent mode enables a sophisticated two-stage process for complex tasks:

**Architecture**
- **Executor**: Performs the actual work (e.g., code generation)
- **Discriminator**: Reviews and validates the executor's output

**Workflow**
1. Executor completes a task
2. Discriminator reviews the output in a role-flipped context
3. Discriminator can request revisions or signal completion
4. Task is completed when discriminator calls `task_complete`

**Benefits**
- Higher quality output through validation
- Reduced hallucinations
- Improved reliability for complex tasks

## Key Design Principles

1. **Safety First**: All file operations require user confirmation by default
2. **Context Awareness**: Agents have full awareness of the project state
3. **Error Handling**: Comprehensive error handling with meaningful feedback
4. **Performance**: Efficient background loading and caching
5. **Extensibility**: Modular design that allows easy addition of new tools and providers

## Usage Patterns

The agent system supports several key usage patterns:

**Code Editing**
- Complete refactorings
- Code generation
- Bug fixes
- Documentation updates

**Project Management**
- File system navigation
- Directory organization
- Bulk file operations

**Information Retrieval**
- Web searches
- File content analysis
- Documentation lookups

**Code Review**
- Dual-agent validation
- Style and convention checks
- Security analysis

## Implementation Details

The agent system leverages GPUI's concurrency model, with all entity operations occurring on a single foreground thread. Background tasks are used for I/O operations and long-running processes.

The system uses a robust subscription model for state changes, ensuring that all relevant components are notified of updates. The use of weak references prevents memory leaks in complex scenarios.

## How Agents Are Defined

Understanding how agents are currently defined is crucial for extending the system:

### Profile → Template → Thread Pipeline

1. **AgentProfileId**: A string identifier (`"write"`, `"ask"`, `"discriminator"`)
2. **AgentProfileSettings**: Configuration loaded from settings
   - `tools`: IndexMap of tool names → enabled/disabled
   - `default_model`: Optional model preference
   - `prompt_template`: Which `.hbs` template to use
3. **Thread**: The runtime agent instance with tools, messages, and model

### Builtin Profiles

Defined in `crates/agent_settings/src/agent_profile.rs`:

```rust
pub mod builtin_profiles {
    pub const WRITE: &str = "write";      // Full tool access for editing
    pub const ASK: &str = "ask";          // Read-only tools for questions
    pub const MINIMAL: &str = "minimal";  // Just read_file and thinking
    pub const DISCRIMINATOR: &str = "discriminator";  // Reviews executor work
}
```

### Prompt Templates

The `PromptTemplate` enum in `crates/settings/src/settings_content/agent.rs` maps to `.hbs` files:

```rust
pub enum PromptTemplate {
    Default,        // → system_prompt.hbs
    Discriminator,  // → discriminator_prompt.hbs
}
```

Templates live in `crates/agent/src/templates/` and use Handlebars with context like `{{model_name}}`, `{{available_tools}}`, `{{worktrees}}`.

### Adding a New Agent Type (Current Process)

1. Add variant to `PromptTemplate` enum
2. Create `.hbs` template file
3. Add profile ID to `builtin_profiles` (optional)
4. Configure tools in settings or code
5. Recompile

### Discriminator Example

The discriminator agent is created in `enable_dual_agent_mode()`:

```rust
// Set profile (loads discriminator_prompt.hbs)
let discriminator_profile_id = AgentProfileId(builtin_profiles::DISCRIMINATOR.into());
thread.set_profile(discriminator_profile_id, cx);

// Add exclusive tool (only discriminator has this)
thread.add_tool(crate::tools::TaskCompleteTool);
```

## Current Limitations

The current architecture has limitations for advanced multi-agent patterns:

1. **Prompt templates are compile-time**: Adding new agent types requires code changes
2. **No first-class agent definition**: Agents are emergent from profiles + scattered code
3. **Tools hardcoded per-profile**: Exclusive tools added manually in orchestration code
4. **Sessions don't track agent identity**: Can't query "all sessions for discriminator-v2"
5. **No prompt versioning**: Can't A/B test or evolve prompts automatically

See [CROW_MULTI_AGENT_ARCHITECTURE.md](./CROW_MULTI_AGENT_ARCHITECTURE.md) for the proposed evolution to address these limitations.

## Future Enhancements (Crow Roadmap)

The Crow fork is evolving the agent system toward:

- **Agents as data**: First-class `AgentDefinition` entities stored in database
- **Content-addressed prompts**: Prompts identified by hash, enabling versioning and A/B testing
- **GEPA integration**: Automated prompt evolution based on discriminator approval rates
- **Declarative tool policies**: `ToolPolicy` enum replaces scattered `add_tool()` calls
- **Hierarchical orchestration**: `reports_to` relationships for multi-agent collaboration
- **Vision integration**: Qwen3-VL-30B for visual feedback in agent traces

This comprehensive agent system enables Crow to function as an intelligent coding assistant, capable of performing complex tasks while maintaining safety and awareness of the project context.
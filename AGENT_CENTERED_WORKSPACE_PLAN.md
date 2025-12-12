# Agent-Centered Workspace Implementation Plan

## Overview
This plan outlines how to create a new workspace layout where the agent occupies the center position with tool call side windows, and the editor/file explorer are positioned on the right side in a dock.

## Current Architecture Understanding

### Current Structure
- **Panels**: Dockable components (Left, Bottom, Right) that live in the `Dock` system
- **AgentPanel**: A Panel that contains thread views, but currently restricted from bottom docking
- **Workspace**: Main container that manages panes, docks, and layout
- **Pane**: Individual editor tabs and items
- **Panel Trait**: Defines how panels work (position, size, icon, etc.)

### Key Findings
- AgentPanel implements `Panel` trait but has restriction `position != DockPosition::Bottom`
- Thread system lives *inside* AgentPanel as `ActiveView` variants
- Panels can be dragged between dock positions, but AgentPanel is artificially restricted
- Current layout: Side panels (left/right/bottom) + main editor area

## New Layout Design

### Target Layout
```
+---------------------+---------------------+
|                     |  Editor Pane Area   |
|   Agent Center      |  +-----------------+ |
|   Area (Main)       |  |  File Explorer  | |
|   +-------------+   |  |  (Docked)      | |
|   |  Thread     |   |  +-----------------+ |
|   |  Chat       |   |                     |
|   |  Interface  |   |  Editor Tabs      |
|   +-------------+   |  (Multiple files)  |
|   | Tool Call   |   |                     |
|   | Side Win 1  |   +---------------------+
|   +-------------+           ^
|   | Tool Call   |           | Dock Position
|   | Side Win 2  |           |
|   +-------------+           v
+---------------------+
```

### Layout Components
1. **Center Agent Area**: Main focus with thread/chat interface
2. **Tool Call Side Windows**: Small windows showing file modifications
3. **Right Side Area**: Editor pane area with docked file explorer
4. **Bottom Dock**: Available for other panels (terminal, etc.)

## Implementation Plan

### Phase 1: Remove AgentPanel Restrictions

**File**: `crates/agent_ui/src/agent_panel.rs`

**Changes**:
```rust
// Line ~1527: Remove bottom position restriction
fn position_is_valid(&self, position: DockPosition) -> bool {
    // Remove: position != DockPosition::Bottom
    true  // Allow all positions
}
```

**Impact**: AgentPanel can now be dragged to bottom dock position

### Phase 2: Create AgentCenteredWorkspace Component

**New File**: `crates/agent_centered_layout/src/lib.rs`

```rust
use gpui::{Entity, EventEmitter, Render, Context, App};
use workspace::{Workspace, DockPosition};
use agent_ui::AgentPanel;

pub struct AgentCenteredWorkspace {
    /// Main agent interface (center focus)
    agent_area: Entity<AgentArea>,
    
    /// Side windows showing tool call results/file modifications
    tool_windows: Vec<Entity<ToolCallSideWindow>>,
    
    /// Right side dock for editor and file explorer
    right_dock: Dock,
    
    /// Whether this layout is active
    is_active: bool,
}

pub struct AgentArea {
    agent_panel: Entity<AgentPanel>,
    thread_view: Entity<ThreadView>,
}

pub struct ToolCallSideWindow {
    tool_call_id: String,
    file_path: PathBuf,
    diff_content: String,
    is_visible: bool,
    size: Pixels,
}
```

**Key Features**:
- Not a `Panel` - first-class workspace component
- Manages tool call windows dynamically
- Integrates with existing AgentPanel system

### Phase 3: Implement ToolCallSideWindow System

**New File**: `crates/tool_call_window/src/lib.rs`

```rust
use gpui::{Entity, Render, Context, Window, App};
use editor::{Editor, EditorElement};
use project::Project;

pub struct ToolCallSideWindow {
    id: String,
    file_path: PathBuf,
    original_content: String,
    modified_content: String,
    is_visible: bool,
    size: Size<Pixels>,
}

impl ToolCallSideWindow {
    pub fn new(tool_call_id: &str, file_path: PathBuf) -> Self {
        Self {
            id: format!("tool-{}", tool_call_id),
            file_path,
            original_content: String::new(),
            modified_content: String::new(),
            is_visible: true,
            size: Size::new(px(400.0), px(300.0)),
        }
    }
    
    pub fn update_diff(&mut self, new_content: String) {
        self.modified_content = new_content;
    }
    
    pub fn toggle_visibility(&mut self) {
        self.is_visible = !self.is_visible;
    }
}

impl Render for ToolCallSideWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div();
        }
        
        v_flex()
            .w(self.size.width)
            .h(self.size.height)
            .bg(cx.theme().colors().panel_background)
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .p_2()
                    .bg(cx.theme().colors().title_bar_background)
                    .child(Label::new(format!("File: {}", self.file_path.file_name().unwrap().to_string_lossy())))
                    .child(
                        IconButton::new(format!("close-{}", self.id), IconName::Close)
                            .on_click(|_, _, _| {
                                // Handle close
                            })
                    )
            )
            .child(
                // Show diff or editor with file content
                div().p_2().child(
                    // TODO: Implement diff view or mini editor
                    Label::new(format!("Changes to: {}", self.file_path.display()))
                )
            )
    }
}
```

### Phase 4: Modify Workspace Layout

**File**: `crates/workspace/src/workspace.rs`

**Add to Workspace struct**:
```rust
// Add new field
pub struct Workspace {
    // ... existing fields
    agent_centered_mode: bool,
    agent_centered_layout: Option<Entity<AgentCenteredWorkspace>>,
    tool_windows: HashMap<String, Entity<ToolCallSideWindow>>,
    // ... existing fields
}
```

**Add to workspace init**:
```rust
impl Workspace {
    pub fn new_centered_mode(
        window: &mut Window, 
        cx: &mut Context<Self>
    ) -> Self {
        Self {
            // ... existing initialization
            agent_centered_mode: true,
            agent_centered_layout: Some(AgentCenteredWorkspace::new(window, cx)),
            tool_windows: HashMap::new(),
            // ... existing initialization
        }
    }
}
```

**Modify main render method**:
```rust
impl Workspace {
    fn render_agent_centered_layout(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h_full()
            .child(
                // Center agent area (60% width)
                self.render_agent_area(window, cx)
                    .w(px(window.bounds().width * 0.6))
            )
            .child(
                // Right side area (40% width)
                self.render_right_side_area(window, cx)
                    .w(px(window.bounds().width * 0.4))
                    .border_l_1()
                    .border_color(cx.theme().colors().border)
            )
    }
    
    fn render_agent_area(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .h_full()
            .child(
                // Main agent panel (80% height)
                self.render_agent_panel(window, cx)
                    .h(px(window.bounds().height * 0.8))
            )
            .child(
                // Tool call windows area (20% height)
                self.render_tool_call_windows(window, cx)
                    .h(px(window.bounds().height * 0.2))
                    .border_t_1()
                    .border_color(cx.theme().colors().border)
            )
    }
    
    fn render_tool_call_windows(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h_full()
            .gap_2()
            .children(
                self.tool_windows
                    .values()
                    .filter_map(|window_entity| {
                        let window = window_entity.read(cx);
                        if window.is_visible {
                            Some(window.render(window, cx))
                        } else {
                            None
                        }
                    })
            )
    }
}
```

### Phase 5: Update Panel Positioning System

**File**: `crates/workspace/src/dock.rs`

**Add new dock position** (if needed):
```rust
pub enum DockPosition {
    Left,
    Bottom,
    Right,
    Active,      // Center area
    RightSide,   // Specific right side dock
}
```

**Update panel positioning logic** to handle new layout:
- Panels can dock to right side area
- Agent area gets priority in center
- Tool windows manage their own sizing

### Phase 6: Integrate with Existing AgentPanel System

**File**: `crates/agent_ui/src/agent_panel.rs`

**Add integration method**:
```rust
impl AgentPanel {
    /// Convert to work in agent-centered layout
    pub fn for_centered_layout(&self, cx: &mut Context<Self>) -> AgentArea {
        AgentArea {
            agent_panel: self.entity(),
            thread_view: self.get_active_thread_view(cx),
        }
    }
    
    fn get_active_thread_view(&self, cx: &Context<Self>) -> Entity<ThreadView> {
        match &self.active_view {
            ActiveView::ExternalAgentThread { thread_view } => thread_view.clone(),
            ActiveView::TextThread { text_thread_editor, .. } => {
                text_thread_editor.read(cx).thread_view().clone()
            }
            _ => // Create default thread view
        }
    }
}
```

### Phase 7: Update Workspace Persistence

**File**: `crates/workspace/src/persistence/model.rs`

**Add new layout serialization**:
```rust
#[derive(Serialize, Deserialize, Debug)]
pub enum WorkspaceLayout {
    Traditional,
    AgentCentered {
        agent_area_size: Size<Pixels>,
        right_area_size: Size<Pixels>,
        tool_windows: Vec<ToolWindowState>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ToolWindowState {
    tool_call_id: String,
    file_path: String,
    is_visible: bool,
    size: Size<Pixels>,
}
```

**Update persistence logic** to save/load new layout state.

### Phase 8: Add Settings and Migration

**File**: `crates/settings/src/settings.rs`

**Add new settings**:
```rust
#[derive(Serialize, Deserialize, Debug, Settings)]
pub struct AgentCenteredLayoutSettings {
    pub enabled: bool,
    pub agent_area_width_ratio: f32,    // 0.6 = 60% width
    pub right_area_width_ratio: f32,    // 0.4 = 40% width
    pub tool_window_height_ratio: f32,  // 0.2 = 20% height
    pub default_tool_window_size: Size<Pixels>,
}
```

**Migration path**:
- Add setting to enable new layout
- Migrate existing workspace layouts
- Provide toggle in settings UI

## Implementation Steps

1. **Remove AgentPanel bottom restriction** (Phase 1)
2. **Create ToolCallSideWindow component** (Phase 3)
3. **Create AgentCenteredWorkspace component** (Phase 2)
4. **Modify Workspace to support new layout** (Phase 4)
5. **Update persistence system** (Phase 7)
6. **Add settings and migration** (Phase 8)
7. **Update panel positioning** (Phase 5)
8. **Integrate with existing AgentPanel** (Phase 6)

## Key Files to Modify

### New Files:
- `crates/agent_centered_layout/src/lib.rs`
- `crates/tool_call_window/src/lib.rs`
- `crates/tool_call_window/Cargo.toml`

### Existing Files:
- `crates/agent_ui/src/agent_panel.rs`
- `crates/workspace/src/workspace.rs`
- `crates/workspace/src/dock.rs`
- `crates/workspace/src/persistence/model.rs`
- `crates/settings/src/settings.rs`

## Testing Considerations

1. **Test panel dragging** in new layout
2. **Test tool window creation/destruction**
3. **Test persistence** of new layout state
4. **Test migration** from old layout
5. **Test performance** with multiple tool windows
6. **Test integration** with existing agent features

## Alternative Approaches

### Option 1: Modify Existing Workspace
- Keep current Workspace structure
- Add conditional rendering for agent-centered mode
- Less invasive but more complex conditional logic

### Option 2: Create Separate Workspace Mode
- Create entirely new workspace variant
- Cleaner separation but more work
- Better for experimentation

### Option 3: Hybrid Approach
- Use existing panel system where possible
- Add new components only where needed
- Balance between familiarity and new functionality

**Recommended**: Option 1 (Modify Existing) for minimal disruption, with Option 2 if complexity becomes too high.

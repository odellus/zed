use anyhow::{Context as _, Result};
use db::kvp::KeyValueStore;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Task, WeakEntity, Window, prelude::*,
};
use project::Project;
use ui::prelude::*;
use workspace::{
    ItemId, Workspace, WorkspaceId,
    item::{Item, ItemEvent, SerializableItem},
    register_serializable_item,
};

use crate::agent_panel::AgentPanel;
use crate::conversation_view::{AcpServerViewEvent, ConversationView};
use crate::thread_metadata_store::ThreadId;

const ITEM_KIND: &str = "AgentThread";

fn kvp_key(item_id: ItemId) -> String {
    format!("agent_thread_item:{item_id}")
}

/// A center-pane presentation of a single agent thread. Wraps a
/// `ConversationView` (owned by `WorkspaceAgentThreads`) and gives it
/// an `Item` identity so Zed's pane infrastructure handles tabs,
/// splits, serialization, and focus.
pub struct AgentThreadItem {
    thread_id: ThreadId,
    conversation_view: Entity<ConversationView>,
    _workspace: WeakEntity<Workspace>,
    _workspace_id: Option<WorkspaceId>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<gpui::Subscription>,
}

impl AgentThreadItem {
    pub fn new(
        thread_id: ThreadId,
        conversation_view: Entity<ConversationView>,
        workspace: WeakEntity<Workspace>,
        workspace_id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let subscriptions = vec![cx.subscribe_in(
            &conversation_view,
            window,
            |_this, _view, _event: &AcpServerViewEvent, _window, cx| {
                cx.emit(ItemEvent::UpdateTab);
                cx.notify();
            },
        )];

        Self {
            thread_id,
            conversation_view,
            _workspace: workspace,
            _workspace_id: workspace_id,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub fn conversation_view(&self) -> &Entity<ConversationView> {
        &self.conversation_view
    }

    fn title(&self, cx: &App) -> SharedString {
        self.conversation_view
            .read(cx)
            .active_thread()
            .and_then(|tv| tv.read(cx).thread.read(cx).title())
            .unwrap_or_else(|| "Agent Thread".into())
    }
}

impl EventEmitter<ItemEvent> for AgentThreadItem {}

impl Focusable for AgentThreadItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AgentThreadItem {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(self.conversation_view.clone())
    }
}

impl Item for AgentThreadItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.title(cx)
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<ui::Icon> {
        Some(ui::Icon::new(ui::IconName::ZedAgent))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl SerializableItem for AgentThreadItem {
    fn serialized_item_kind() -> &'static str {
        ITEM_KIND
    }

    fn cleanup(
        _workspace_id: WorkspaceId,
        _alive_items: Vec<ItemId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<()>> {
        // Orphaned kvp rows are a few bytes each; not worth a prefix scan.
        Task::ready(Ok(()))
    }

    fn deserialize(
        _project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        workspace_id: WorkspaceId,
        item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let thread_id_str = match KeyValueStore::global(cx).read_kvp(&kvp_key(item_id)) {
            Ok(Some(s)) => s,
            _ => {
                return Task::ready(Err(anyhow::anyhow!(
                    "no thread id for item {item_id}"
                )))
            }
        };

        let thread_id: ThreadId = match serde_json::from_str(&thread_id_str) {
            Ok(id) => id,
            Err(e) => {
                return Task::ready(Err(anyhow::anyhow!("bad thread id: {e}")))
            }
        };

        window.spawn(cx, async move |cx| {
            let threads_entity = workspace
                .read_with(cx, |workspace, cx| {
                    workspace
                        .panel::<AgentPanel>(cx)
                        .map(|panel| panel.read(cx).threads().clone())
                })
                .ok()
                .flatten()
                .context("no agent panel / threads store")?;

            let conversation_view = cx.update(|window, cx| {
                threads_entity.update(cx, |threads, cx| {
                    if let Some(cv) = threads.conversation_view_for_id(&thread_id, cx) {
                        return cv;
                    }
                    threads.create_thread_with_resume(
                        crate::Agent::default(),
                        None,
                        Some(thread_id),
                        None,
                        None,
                        None,
                        crate::AgentThreadSource::AgentPanel,
                        window,
                        cx,
                    )
                })
            })?;

            cx.update(|window, cx| {
                cx.new(|cx| {
                    AgentThreadItem::new(
                        thread_id,
                        conversation_view,
                        workspace,
                        Some(workspace_id),
                        window,
                        cx,
                    )
                })
            })
        })
    }

    fn serialize(
        &mut self,
        _workspace: &mut Workspace,
        item_id: ItemId,
        _closing: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        let kvp = KeyValueStore::global(cx);
        let thread_id = self.thread_id;
        Some(cx.background_spawn(async move {
            let value = serde_json::to_string(&thread_id)?;
            kvp.write_kvp(kvp_key(item_id), value).await?;
            Ok(())
        }))
    }

    fn should_serialize(&self, _event: &Self::Event) -> bool {
        false
    }
}

/// Register `AgentThreadItem` as a serializable center-pane item.
pub fn init(cx: &mut App) {
    register_serializable_item::<AgentThreadItem>(cx);
}

use std::rc::Rc;
use std::sync::Arc;

use agent::ThreadStore;
use agent_client_protocol::schema::v1 as acp;
use agent_servers::AgentServer;
use collections::HashMap;
use fs::Fs;
use gpui::{App, Context, Entity, EventEmitter, SharedString, Subscription, WeakEntity, Window, prelude::*};
use project::Project;
use workspace::{PathList, Workspace};

use crate::agent_connection_store::AgentConnectionStore;
use crate::thread_metadata_store::{ThreadId, ThreadMetadataStore, ThreadMetadataStoreEvent};
use crate::{Agent, AgentInitialContent, AgentThreadSource, ConversationView, MaxIdleRetainedThreads};

/// Events emitted when the thread collection changes. Presentations
/// (AgentPanel, AgentThreadItem) subscribe to update their UI.
#[derive(Clone, Debug)]
pub enum WorkspaceAgentThreadsEvent {
    ThreadAdded(ThreadId),
    ThreadRemoved(ThreadId),
    DraftChanged,
}

/// Owns the live set of agent threads for a workspace. Neither the panel
/// nor center-pane items own threads — they hold `Entity<ConversationView>`
/// handles obtained from this store. One live ConversationView per thread;
/// no duplication.
pub struct WorkspaceAgentThreads {
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    fs: Arc<dyn Fs>,
    thread_store: Entity<ThreadStore>,
    connection_store: Entity<AgentConnectionStore>,
    draft: Option<Entity<ConversationView>>,
    live_threads: HashMap<ThreadId, Entity<ConversationView>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WorkspaceAgentThreadsEvent> for WorkspaceAgentThreads {}

impl WorkspaceAgentThreads {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        fs: Arc<dyn Fs>,
        thread_store: Entity<ThreadStore>,
        connection_store: Entity<AgentConnectionStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _subscriptions = vec![cx.subscribe(
            &ThreadMetadataStore::global(cx),
            |this, _store, event, cx| {
                let ThreadMetadataStoreEvent::ThreadArchived(thread_id) = event;
                if this.live_threads.remove(thread_id).is_some() {
                    cx.emit(WorkspaceAgentThreadsEvent::ThreadRemoved(*thread_id));
                    cx.notify();
                }
            },
        )];

        Self {
            workspace,
            project,
            fs,
            thread_store,
            connection_store,
            draft: None,
            live_threads: HashMap::default(),
            _subscriptions,
        }
    }

    // ── Accessors ──────────────────────────────────────────────────────

    pub fn connection_store(&self) -> &Entity<AgentConnectionStore> {
        &self.connection_store
    }

    pub fn thread_store(&self) -> &Entity<ThreadStore> {
        &self.thread_store
    }

    pub fn draft(&self) -> Option<&Entity<ConversationView>> {
        self.draft.as_ref()
    }

    pub fn live_threads(&self) -> &HashMap<ThreadId, Entity<ConversationView>> {
        &self.live_threads
    }

    pub fn conversation_view_for_id(
        &self,
        id: &ThreadId,
        cx: &App,
    ) -> Option<Entity<ConversationView>> {
        if self.draft.as_ref().is_some_and(|d| d.read(cx).thread_id == *id) {
            return self.draft.clone();
        }
        self.live_threads.get(id).cloned()
    }

    pub fn all_conversation_views(&self) -> Vec<Entity<ConversationView>> {
        self.draft
            .iter()
            .cloned()
            .chain(self.live_threads.values().cloned())
            .collect()
    }

    pub fn is_live(&self, id: &ThreadId) -> bool {
        self.live_threads.contains_key(id)
    }

    // ── Thread creation ────────────────────────────────────────────────

    /// Core creation path. Builds a `ConversationView` for the given agent,
    /// optionally resuming an existing thread. Returns the new entity.
    ///
    /// Does NOT insert into `live_threads` — the caller decides whether the
    /// thread is retained, shown in a panel, or opened as a center-pane item.
    pub fn create_thread(
        &mut self,
        agent: Agent,
        server_override: Option<Rc<dyn AgentServer>>,
        resume_thread_id: Option<ThreadId>,
        resume_session_id: Option<acp::SessionId>,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        initial_content: Option<AgentInitialContent>,
        source: AgentThreadSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ConversationView> {
        let thread_id = resume_thread_id.unwrap_or_else(ThreadId::new);

        let server = server_override
            .unwrap_or_else(|| agent.server(self.fs.clone(), self.thread_store.clone()));
        let thread_store = server
            .clone()
            .downcast::<agent::NativeAgentServer>()
            .is_some()
            .then(|| self.thread_store.clone());

        let conversation_view = cx.new(|cx| {
            ConversationView::new(
                server,
                self.connection_store.clone(),
                agent,
                resume_session_id,
                Some(thread_id),
                work_dirs,
                title,
                initial_content,
                self.workspace.clone(),
                self.project.clone(),
                thread_store,
                source,
                window,
                cx,
            )
        });

        cx.emit(WorkspaceAgentThreadsEvent::ThreadAdded(thread_id));
        conversation_view
    }

    /// Convenience: resolves `resume_session_id` from the metadata store.
    pub fn create_thread_with_resume(
        &mut self,
        agent: Agent,
        server_override: Option<Rc<dyn AgentServer>>,
        resume_thread_id: Option<ThreadId>,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        initial_content: Option<AgentInitialContent>,
        source: AgentThreadSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ConversationView> {
        let resume_session_id = resume_thread_id.and_then(|tid| {
            ThreadMetadataStore::try_global(cx)
                .and_then(|store| store.read(cx).entry(tid).and_then(|m| m.session_id.clone()))
        });
        self.create_thread(
            agent,
            server_override,
            resume_thread_id,
            resume_session_id,
            work_dirs,
            title,
            initial_content,
            source,
            window,
            cx,
        )
    }

    // ── Retention ──────────────────────────────────────────────────────

    /// Parks a thread in the live set. Called when a thread is no longer
    /// shown in any presentation but should stay alive.
    pub fn retain_thread(
        &mut self,
        conversation_view: Entity<ConversationView>,
        cx: &mut Context<Self>,
    ) {
        let thread_id = conversation_view.read(cx).thread_id;

        if self.live_threads.contains_key(&thread_id) {
            return;
        }

        // If this was the draft, promote it only if it has content.
        if self
            .draft
            .as_ref()
            .is_some_and(|d| d.entity_id() == conversation_view.entity_id())
        {
            if !self.draft_has_content(&conversation_view, cx) {
                return;
            }
            self.draft = None;
            cx.emit(WorkspaceAgentThreadsEvent::DraftChanged);
        }

        self.live_threads.insert(thread_id, conversation_view);
        self.cleanup_idle_threads(cx);
    }

    fn cleanup_idle_threads(&mut self, cx: &mut Context<Self>) {
        let mut idle: Vec<_> = self
            .live_threads
            .iter()
            .filter(|(_id, view)| {
                let Some(thread_view) = view.read(cx).root_thread_view() else {
                    return true;
                };
                let thread = thread_view.read(cx).thread.read(cx);
                thread.connection().supports_load_session()
                    && thread.status() == acp_thread::ThreadStatus::Idle
            })
            .collect();

        let max_idle = MaxIdleRetainedThreads::global(cx);
        idle.sort_unstable_by_key(|(_, view)| view.read(cx).updated_at(cx));
        let n = idle.len().saturating_sub(max_idle);
        let to_remove: Vec<ThreadId> = idle.into_iter().map(|(id, _)| *id).take(n).collect();
        for id in to_remove {
            self.live_threads.remove(&id);
            cx.emit(WorkspaceAgentThreadsEvent::ThreadRemoved(id));
        }
    }

    // ── Removal ────────────────────────────────────────────────────────

    /// Deletes a thread from the live set and its metadata row.
    pub fn remove_thread(&mut self, id: ThreadId, cx: &mut Context<Self>) {
        self.live_threads.remove(&id);
        ThreadMetadataStore::global(cx).update(cx, |store, cx| {
            store.delete(id, cx);
        });

        if self.draft.as_ref().is_some_and(|d| d.read(cx).thread_id == id) {
            self.draft = None;
            cx.emit(WorkspaceAgentThreadsEvent::DraftChanged);
        }

        cx.emit(WorkspaceAgentThreadsEvent::ThreadRemoved(id));
    }

    /// Takes a thread out of the live set without deleting it.
    /// Used when a thread moves from "parked" to "shown in a presentation."
    pub fn take_thread(&mut self, id: &ThreadId) -> Option<Entity<ConversationView>> {
        self.live_threads.remove(id)
    }

    // ── Draft management ───────────────────────────────────────────────

    pub fn draft_has_content(&self, draft: &Entity<ConversationView>, cx: &App) -> bool {
        let cv = draft.read(cx);
        if let Some(thread_view) = cv.active_thread() {
            let text = thread_view.read(cx).message_editor.read(cx).text(cx);
            if !text.trim().is_empty() {
                return true;
            }
        }
        if let Some(acp_thread) = cv.root_thread(cx) {
            let thread = acp_thread.read(cx);
            if !thread.is_draft_thread() {
                return true;
            }
        }
        false
    }

    pub fn set_draft(
        &mut self,
        conversation_view: Entity<ConversationView>,
        cx: &mut Context<Self>,
    ) {
        if let Some(old_draft) = self.draft.take() {
            let old_id = old_draft.read(cx).thread_id;
            let new_id = conversation_view.read(cx).thread_id;
            if old_id != new_id {
                ThreadMetadataStore::global(cx).update(cx, |store, cx| {
                    store.delete(old_id, cx);
                });
            }
        }
        self.draft = Some(conversation_view);
        cx.emit(WorkspaceAgentThreadsEvent::DraftChanged);
    }

    pub fn take_draft(&mut self) -> Option<Entity<ConversationView>> {
        self.draft.take()
    }

    /// If the given view is an empty draft in the live set, reclaim it as
    /// the ephemeral draft. Returns true if reclaimed.
    pub fn try_reclaim_empty_draft(
        &mut self,
        conversation_view: Entity<ConversationView>,
        cx: &mut Context<Self>,
    ) -> bool {
        let (thread_id, is_draft, is_empty) = {
            let conversation = conversation_view.read(cx);
            let thread_id = conversation.thread_id;
            let is_draft = conversation
                .root_thread(cx)
                .is_some_and(|thread| thread.read(cx).is_draft_thread());
            let is_empty = if let Some(thread_view) = conversation.active_thread() {
                thread_view
                    .read(cx)
                    .message_editor
                    .read(cx)
                    .text(cx)
                    .trim()
                    .is_empty()
            } else {
                !self.draft_has_content(&conversation_view, cx)
            };
            (thread_id, is_draft, is_empty)
        };

        if !is_draft || !is_empty {
            return false;
        }

        self.live_threads.remove(&thread_id);
        self.set_draft(conversation_view, cx);
        true
    }
}

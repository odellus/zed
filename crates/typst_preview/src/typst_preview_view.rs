use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use editor::Editor;
use file_icons::FileIcons;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    RenderImage, Styled, Subscription, Task, WeakEntity, Window, div, img,
};
use language::{Buffer, BufferEvent};
use multi_buffer::MultiBuffer;
use ui::prelude::*;
use workspace::item::Item;
use workspace::{Pane, Workspace};

use typst::LibraryExt as _;

use crate::{OpenFollowingPreview, OpenPreview, OpenPreviewToTheSide};

fn is_typst_language(buffer: &Entity<MultiBuffer>, cx: &App) -> bool {
    buffer.read_with(cx, |buffer, cx| {
        buffer
            .as_singleton()
            .and_then(|b| {
                b.read_with(cx, |b, _| {
                    b.language().map(|l| l.name().as_ref() == "Typst")
                })
            })
            .unwrap_or(false)
    })
}

pub struct TypstPreviewView {
    focus_handle: FocusHandle,
    buffer: Option<Entity<Buffer>>,
    pages: Vec<Arc<RenderImage>>,
    error: Option<SharedString>,
    _refresh: Task<()>,
    _buffer_subscription: Option<Subscription>,
    _workspace_subscription: Option<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypstPreviewMode {
    Default,
    Follow,
}

impl TypstPreviewView {
    fn create(
        mode: TypstPreviewMode,
        active_buffer: Entity<MultiBuffer>,
        workspace_handle: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let workspace_subscription = if mode == TypstPreviewMode::Follow {
                workspace_handle.upgrade().map(|workspace| {
                    Self::subscribe_to_workspace(workspace, window, cx)
                })
            } else {
                None
            };

            let buffer = active_buffer.read_with(cx, |buffer, _cx| buffer.as_singleton());

            let subscription = buffer
                .as_ref()
                .map(|buffer| Self::create_buffer_subscription(buffer, window, cx));

            let mut this = Self {
                focus_handle: cx.focus_handle(),
                buffer,
                pages: Vec::new(),
                error: None,
                _refresh: Task::ready(()),
                _buffer_subscription: subscription,
                _workspace_subscription: workspace_subscription,
            };

            this.refresh(window, cx);
            this
        })
    }

    fn create_buffer_subscription(
        buffer: &Entity<Buffer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(buffer, window, |this, _, e: &BufferEvent, window, cx| {
            if matches!(e, BufferEvent::Edited { .. } | BufferEvent::Reloaded) {
                this.refresh(window, cx);
            }
        })
    }

    fn subscribe_to_workspace(
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            &workspace,
            window,
            move |this: &mut Self, workspace, event: &workspace::Event, window, cx| {
                if let workspace::Event::ActiveItemChanged = event {
                    if let Some(multi) = Self::resolve_active_typst_buffer(workspace.read(cx), cx) {
                        let singleton = multi.read_with(cx, |mb, _| mb.as_singleton());
                        if let Some(buffer) = singleton {
                            this._buffer_subscription =
                                Some(Self::create_buffer_subscription(&buffer, window, cx));
                            this.buffer = Some(buffer);
                            this.refresh(window, cx);
                            cx.notify();
                        }
                    }
                }
            },
        )
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffer.clone() else {
            return;
        };

        let (text, path) = buffer.read_with(cx, |buffer, cx| {
            let text = buffer.text();
            let path = buffer
                .file()
                .and_then(|f| f.as_local())
                .map(|f| f.abs_path(cx));
            (text, path)
        });

        self._refresh = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { compile_typst(&text, path.as_deref()) })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(pages) => {
                        this.pages = pages;
                        this.error = None;
                    }
                    Err(e) => {
                        this.error = Some(e.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn resolve_active_typst_buffer(
        workspace: &Workspace,
        cx: &App,
    ) -> Option<Entity<MultiBuffer>> {
        let editor = workspace.active_item(cx)?.act_as::<Editor>(cx)?;
        let buffer = editor.read(cx).buffer().clone();
        is_typst_language(&buffer, cx).then_some(buffer)
    }

    fn activate_or_add_preview(
        _workspace: &mut Workspace,
        buffer: Entity<MultiBuffer>,
        pane: Entity<Pane>,
        focus: bool,
        mode: TypstPreviewMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let workspace_handle = cx.entity().downgrade();
        let view = Self::create(mode, buffer, workspace_handle, window, cx);
        pane.update(cx, |pane, cx| {
            pane.add_item(Box::new(view), focus, focus, None, window, cx)
        });
        cx.notify();
    }

    pub fn register(workspace: &mut Workspace, _window: &mut Window, _cx: &mut Context<Workspace>) {
        workspace.register_action(move |workspace, _: &OpenPreview, window, cx| {
            if let Some(buffer) = Self::resolve_active_typst_buffer(workspace, cx) {
                let pane = workspace.active_pane().clone();
                Self::activate_or_add_preview(
                    workspace, buffer, pane, true, TypstPreviewMode::Default, window, cx,
                );
            }
        });

        workspace.register_action(move |workspace, _: &OpenPreviewToTheSide, window, cx| {
            if let Some(buffer) = Self::resolve_active_typst_buffer(workspace, cx) {
                let origin_pane = workspace.active_pane().clone();
                let target_pane = workspace.adjacent_pane_of(&origin_pane, window, cx);
                Self::activate_or_add_preview(
                    workspace, buffer, target_pane, false, TypstPreviewMode::Default, window, cx,
                );
            }
        });

        workspace.register_action(move |workspace, _: &OpenFollowingPreview, window, cx| {
            if let Some(buffer) = Self::resolve_active_typst_buffer(workspace, cx) {
                let workspace_handle = cx.entity().downgrade();
                let view =
                    Self::create(TypstPreviewMode::Follow, buffer, workspace_handle, window, cx);
                workspace.active_pane().update(cx, |pane, cx| {
                    pane.add_item(Box::new(view), true, true, None, window, cx)
                });
                cx.notify();
            }
        });
    }
}

/// Compile typst source to page images.
fn compile_typst(source: &str, path: Option<&Path>) -> Result<Vec<Arc<RenderImage>>> {
    let world = EditorWorld::new(source, path)?;

    let result: typst::diag::SourceResult<typst_layout::PagedDocument> =
        typst::compile(&world).output;
    let document = result.map_err(|diags| {
        let msgs: Vec<String> = diags
            .iter()
            .map(|d| format!("{:?}: {}", d.severity, d.message))
            .collect();
        anyhow::anyhow!("{}", msgs.join("\n"))
    })?;

    let mut pages = Vec::new();
    let opts = typst_render::RenderOptions::default();
    for page in document.pages() {
        let pixmap = typst_render::render(page, &opts);
        let (w, h) = (pixmap.width(), pixmap.height());
        // tiny-skia Pixmap is premultiplied RGBA; un-premultiply for image crate
        let mut rgba = pixmap.data().to_vec();
        for chunk in rgba.chunks_exact_mut(4) {
            let a = chunk[3] as f32;
            if a > 0.0 && a < 255.0 {
                let inv = 255.0 / a;
                chunk[0] = ((chunk[0] as f32) * inv).min(255.0) as u8;
                chunk[1] = ((chunk[1] as f32) * inv).min(255.0) as u8;
                chunk[2] = ((chunk[2] as f32) * inv).min(255.0) as u8;
            }
        }
        let image = image::RgbaImage::from_raw(w, h, rgba)
            .ok_or_else(|| anyhow::anyhow!("failed to create image from pixmap"))?;
        let frame = image::Frame::new(image);
        pages.push(Arc::new(RenderImage::new(smallvec::smallvec![frame])));
    }

    Ok(pages)
}

/// A minimal typst World for the editor preview.
struct EditorWorld {
    library: typst::utils::LazyHash<typst::Library>,
    fonts: typst_kit::fonts::FontStore,
    main_id: typst::syntax::FileId,
    source: typst::syntax::Source,
    root: PathBuf,
}

impl EditorWorld {
    fn new(source_text: &str, path: Option<&Path>) -> Result<Self> {
        let root = path
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let vpath = typst::syntax::VirtualPath::new("main.typ")
            .map_err(|e| anyhow::anyhow!("bad vpath: {e}"))?;
        let rooted = typst::syntax::RootedPath::new(typst::syntax::VirtualRoot::Project, vpath);
        let main_id = typst::syntax::FileId::new(rooted);
        let source = typst::syntax::Source::new(main_id, source_text.to_string());

        let mut fonts = typst_kit::fonts::FontStore::new();
        for (font, info) in typst_kit::fonts::embedded() {
            fonts.push((font, info));
        }
        for (path, info) in typst_kit::fonts::system() {
            fonts.push((path, info));
        }

        Ok(Self {
            library: typst::utils::LazyHash::new(typst::Library::builder().build()),
            fonts,
            main_id,
            source,
            root,
        })
    }
}

impl typst::World for EditorWorld {
    fn library(&self) -> &typst::utils::LazyHash<typst::Library> {
        &self.library
    }

    fn book(&self) -> &typst::utils::LazyHash<typst::text::FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> typst::syntax::FileId {
        self.main_id
    }

    fn source(&self, id: typst::syntax::FileId) -> typst::diag::FileResult<typst::syntax::Source> {
        if id == self.main_id {
            return Ok(self.source.clone());
        }
        let path = id
            .vpath()
            .realize(&self.root)
            .map_err(|_| typst::diag::FileError::NotFound(self.root.clone()))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|_| typst::diag::FileError::NotFound(path.clone()))?;
        Ok(typst::syntax::Source::new(id, text))
    }

    fn file(&self, id: typst::syntax::FileId) -> typst::diag::FileResult<typst::foundations::Bytes> {
        let path = id
            .vpath()
            .realize(&self.root)
            .map_err(|_| typst::diag::FileError::NotFound(self.root.clone()))?;
        let data = std::fs::read(&path)
            .map_err(|_| typst::diag::FileError::NotFound(path.clone()))?;
        Ok(typst::foundations::Bytes::new(data))
    }

    fn font(&self, index: usize) -> Option<typst::text::Font> {
        self.fonts.font(index)
    }

    fn today(
        &self,
        offset: Option<typst::foundations::Duration>,
    ) -> Option<typst::foundations::Datetime> {
        typst_kit::datetime::Time::system().today(offset)
    }
}

impl EventEmitter<()> for TypstPreviewView {}

impl Focusable for TypstPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TypstPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("TypstPreview")
            .key_context("TypstPreview")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .map(|this| {
                if let Some(error) = &self.error {
                    this.child(
                        div()
                            .p_4()
                            .child(
                                Label::new(format!("Typst error:\n{error}")).color(Color::Error),
                            )
                            .into_any_element(),
                    )
                } else if self.pages.is_empty() {
                    this.child(
                        div()
                            .p_4()
                            .child(Label::new("No Typst content.").color(Color::Muted))
                            .into_any_element(),
                    )
                } else {
                    this.children(self.pages.iter().map(|page| {
                        div()
                            .flex()
                            .justify_center()
                            .p_2()
                            .child(img(page.clone()).max_w_full())
                            .into_any_element()
                    }))
                }
            })
    }
}

impl Item for TypstPreviewView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Typst Preview".into()
    }

    fn tab_icon(&self, _window: &Window, cx: &App) -> Option<Icon> {
        FileIcons::get_icon("typst".as_ref(), cx)
            .map(Icon::from_path)
            .or_else(|| Some(Icon::new(IconName::Eye)))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("typst preview")
    }
}

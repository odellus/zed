use std::mem;
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

use crate::{OpenFollowingPreview, OpenPreview, OpenPreviewToTheSide};

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
    pub fn new(
        mode: TypstPreviewMode,
        active_buffer: Entity<MultiBuffer>,
        workspace_handle: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let workspace_subscription = if mode == TypstPreviewMode::Follow
                && let Some(workspace) = workspace_handle.upgrade()
            {
                Some(Self::subscribe_to_workspace(workspace, window, cx))
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
                _workspace_subscription,
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
                    let active = workspace
                        .read(cx)
                        .active_item(cx)
                        .and_then(|item| item.act_as::<Editor>(cx));
                    if let Some(editor) = active {
                        let buffer = editor.read(cx).buffer().read(cx).as_singleton();
                        if let Some(buffer) = buffer {
                            let is_typst = buffer.read_with(cx, |b, _| {
                                b.language()
                                    .map(|l| l.name() == "Typst".into())
                                    .unwrap_or(false)
                            });
                            if is_typst {
                                this._buffer_subscription =
                                    Some(Self::create_buffer_subscription(&buffer, window, cx));
                                this.buffer = Some(buffer);
                                this.refresh(window, cx);
                                cx.notify();
                            }
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

    pub fn register(workspace: &mut Workspace, _window: &mut Window, _cx: &mut Context<Workspace>) {
        workspace.register_action(|workspace, _: &OpenPreview, window, cx| {
            Self::open(workspace, TypstPreviewMode::Default, window, cx);
        });
        workspace.register_action(|workspace, _: &OpenPreviewToTheSide, window, cx| {
            Self::open_to_side(workspace, TypstPreviewMode::Default, window, cx);
        });
        workspace.register_action(|workspace, _: &OpenFollowingPreview, window, cx| {
            Self::open(workspace, TypstPreviewMode::Follow, window, cx);
        });
    }

    fn open(
        workspace: &mut Workspace,
        mode: TypstPreviewMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let Some(buffer) = Self::active_typst_buffer(workspace, cx) else {
            return;
        };
        let workspace_handle = cx.entity().downgrade();
        let preview = Self::new(mode, buffer, workspace_handle, window, cx);
        workspace.add_item_to_active_pane(Box::new(preview), None, true, cx);
    }

    fn open_to_side(
        workspace: &mut Workspace,
        mode: TypstPreviewMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let Some(buffer) = Self::active_typst_buffer(workspace, cx) else {
            return;
        };
        let workspace_handle = cx.entity().downgrade();
        let preview = Self::new(mode, buffer, workspace_handle, window, cx);
        workspace.split_item(
            workspace::SplitDirection::Right,
            Box::new(preview),
            window,
            cx,
        );
    }

    fn active_typst_buffer(workspace: &Workspace, cx: &App) -> Option<Entity<MultiBuffer>> {
        let editor = workspace.active_item(cx)?.act_as::<Editor>(cx)?;
        let buffer = editor.read(cx).buffer().clone();
        let is_typst = buffer.read_with(cx, |buffer, cx| {
            buffer
                .as_singleton()
                .and_then(|b| {
                    b.read_with(cx, |b, _| {
                        b.language().map(|l| l.name() == "Typst".into())
                    })
                })
                .unwrap_or(false)
        });
        is_typst.then_some(buffer)
    }
}

/// Compile typst source to page images.
fn compile_typst(source: &str, path: Option<&Path>) -> Result<Vec<Arc<RenderImage>>> {
    let world = EditorWorld::new(source, path)?;

    let document = typst::compile(&world).output.map_err(|diags| {
        let msgs: Vec<String> = diags
            .iter()
            .map(|d| format!("{:?}: {}", d.severity, d.message))
            .collect();
        anyhow::anyhow!("{}", msgs.join("\n"))
    })?;

    let mut pages = Vec::new();
    let opts = typst_render::RenderOptions::default();
    for page in &document.pages {
        let pixmap = typst_render::render(page, &opts);
        // tiny-skia Pixmap is RGBA premultiplied; un-premultiply for image crate
        let (w, h) = (pixmap.width(), pixmap.height());
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
        pages.push(Arc::new(RenderImage::new(frame)));
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

        let main_id = typst::syntax::FileId::new(None, "main.typ".into());
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
            .resolve(&self.root)
            .ok_or_else(|| typst::diag::eco_format!("failed to resolve path for {id:?}"))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| typst::diag::eco_format!("failed to read {}: {e}", path.display()))?;
        Ok(typst::syntax::Source::new(id, text))
    }

    fn file(&self, id: typst::syntax::FileId) -> typst::diag::FileResult<typst::foundations::Bytes> {
        let path = id
            .vpath()
            .resolve(&self.root)
            .ok_or_else(|| typst::diag::eco_format!("failed to resolve path for {id:?}"))?;
        let data = std::fs::read(&path)
            .map_err(|e| typst::diag::eco_format!("failed to read {}: {e}", path.display()))?;
        Ok(typst::foundations::Bytes::new(data))
    }

    fn font(&self, index: usize) -> Option<typst::text::Font> {
        self.fonts.font(index)
    }

    fn today(&self, offset: Option<i64>) -> Option<typst::foundations::Datetime> {
        typst_kit::datetime::Time::system().datetime(offset)
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
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scrollbar()
            .bg(cx.theme().colors().editor_background)
            .children(if let Some(error) = &self.error {
                vec![div()
                    .p_4()
                    .child(
                        Label::new(format!("Typst compilation error:\n{error}"))
                            .color(Color::Error),
                    )
                    .into_any_element()]
            } else if self.pages.is_empty() {
                vec![div()
                    .p_4()
                    .child(Label::new("No Typst content to preview.").color(Color::Muted))
                    .into_any_element()]
            } else {
                self.pages
                    .iter()
                    .map(|page| {
                        div()
                            .flex()
                            .justify_center()
                            .p_2()
                            .child(img(page.clone()))
                            .into_any_element()
                    })
                    .collect()
            })
    }
}

impl Item for TypstPreviewView {
    type Event = ();

    fn tab_content_text(&self, _cx: &App) -> Option<SharedString> {
        Some("Typst Preview".into())
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

use gpui::{App, actions};
use workspace::Workspace;

pub mod typst_preview_view;

pub use zed_actions::preview::typst::{OpenPreview, OpenPreviewToTheSide};

actions!(
    typst,
    [
        /// Opens a following Typst preview that syncs with the editor.
        OpenFollowingPreview
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        crate::typst_preview_view::TypstPreviewView::register(workspace, window, cx);
    })
    .detach();
}

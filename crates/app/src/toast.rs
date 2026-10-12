//! Quick-access thumbnail: after a capture, a small preview in the bottom-right corner of the
//! main display for a few seconds. Clicking it opens the capture in the library. It stays
//! while the pointer is over it, and a new capture closes it first so it is never captured.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Global, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, div, hsla, img, point, px, size,
};

use crate::library_view;

const WIDTH: f32 = 240.;
const HEIGHT: f32 = 176.;
const MARGIN: f32 = 16.;
/// How long the preview stays after the pointer leaves it.
const LINGER: Duration = Duration::from_secs(6);
/// Time for the window server to take the preview off screen before a new grab.
pub const SETTLE: Duration = Duration::from_millis(120);

#[derive(Default)]
struct Open(Option<AnyWindowHandle>);
impl Global for Open {}

struct Toast {
    id: i64,
    thumb: PathBuf,
    hovered: bool,
}

impl Render for Toast {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id;
        div()
            .id("toast")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(8.))
            .rounded(px(12.))
            .bg(hsla(0., 0., 0.1, 0.94))
            .border_1()
            .border_color(hsla(0., 0., 1., 0.15))
            .cursor_pointer()
            .on_hover(cx.listener(|this, hovered: &bool, _, _| this.hovered = *hovered))
            .on_click(cx.listener(move |_, _, window, cx| {
                window.remove_window();
                cx.set_global(Open(None));
                library_view::open_at(Some(id), cx);
            }))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(hsla(0., 0., 0., 0.4))
                    .child(
                        img(self.thumb.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(hsla(0., 0., 1., 0.85))
                    .child(div().flex_1().child("Copied · click to open"))
                    .child(
                        div()
                            .id("toast-edit")
                            .px(px(6.))
                            .rounded(px(4.))
                            .bg(hsla(0., 0., 1., 0.12))
                            .hover(|d| d.bg(hsla(0., 0., 1., 0.24)))
                            .child("Edit")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                                cx.set_global(Open(None));
                                crate::editor_view::open(id, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("toast-close")
                            .px(px(6.))
                            .text_color(hsla(0., 0., 1., 0.6))
                            .hover(|d| d.text_color(hsla(0., 0., 1., 1.)))
                            .child("×")
                            .on_click(cx.listener(|_, _, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                                cx.set_global(Open(None));
                            })),
                    ),
            )
    }
}

/// Shows the preview for capture `id`, replacing any preview already showing.
pub fn show(id: i64, thumb: PathBuf, cx: &mut App) {
    dismiss(cx);
    let Some(screen) = cx.primary_display() else {
        return;
    };
    let b = screen.bounds();
    let origin = point(
        b.origin.x + b.size.width - px(WIDTH + MARGIN),
        b.origin.y + b.size.height - px(HEIGHT + MARGIN),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(crate::session::popup_bounds(
            Bounds::new(origin, size(px(WIDTH), px(HEIGHT))),
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("snapwright".into()),
        ..Default::default()
    };
    let window = match cx.open_window(options, |_, cx| {
        cx.new(|_| Toast {
            id,
            thumb,
            hovered: false,
        })
    }) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "quick-access preview unavailable");
            return;
        }
    };
    let handle: AnyWindowHandle = window.into();
    cx.set_global(Open(Some(handle)));
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(LINGER).await;
            let hovered = window.update(cx, |toast, _, _| toast.hovered);
            match hovered {
                Ok(true) => continue,
                Ok(false) => break,
                Err(_) => return, // already closed
            }
        }
        let _ = window.update(cx, |_, window, _| window.remove_window());
        cx.update(|cx| {
            if cx.default_global::<Open>().0 == Some(handle) {
                cx.set_global(Open(None));
            }
        });
    })
    .detach();
}

/// Closes the preview if one is showing. Returns whether one was.
pub fn dismiss(cx: &mut App) -> bool {
    let Some(handle) = cx.default_global::<Open>().0.take() else {
        return false;
    };
    handle
        .update(cx, |_, window, _| window.remove_window())
        .is_ok()
}

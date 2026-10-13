//! Quick-access thumbnail: after a capture, a small preview in the bottom-right corner of the
//! main display for a few seconds. Clicking it opens the capture in the library. It stays
//! while the pointer is over it. Where the OS can keep it out of captures (macOS, Windows) a
//! new capture does not wait for it; elsewhere it is closed first so it is never captured.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Bounds, Context,
    FontWeight, Global, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, hsla, img, point, px, relative,
    size,
};

use crate::library_view;
use crate::theme::hud;

const WIDTH: f32 = 280.;
const THUMB_H: f32 = 150.;
/// Padding, thumbnail, gap, the two-line caption row and bottom padding.
const HEIGHT: f32 = 8. + THUMB_H + 10. + 36. + 12.;
const MARGIN: f32 = 16.;
/// How long the preview stays after the pointer leaves it.
const LINGER: Duration = Duration::from_secs(6);
/// Time for the window server to take the preview off screen before a new grab.
pub const SETTLE: Duration = Duration::from_millis(120);

/// The preview on screen, and whether the OS keeps it out of captures.
#[derive(Default)]
struct Open(Option<(AnyWindowHandle, bool)>);
impl Global for Open {}

struct Toast {
    id: i64,
    thumb: PathBuf,
    size: (u32, u32),
    /// For a recording: the video file and its length in milliseconds.
    video: Option<(PathBuf, u64)>,
    hovered: bool,
    /// Bumped when the pointer leaves, so the time-left bar starts over.
    linger: usize,
}

impl Render for Toast {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id;
        let icon_button = |icon: IconName| Icon::new(icon).size(px(13.)).text_color(hud::fg2());
        let close = div()
            .id("toast-close")
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .bg(hsla(240. / 360., 0.09, 0.1, 0.8))
            .hover(|d| d.bg(hud::bg()))
            .child(icon_button(IconName::X))
            .on_click(cx.listener(|_, _, window, cx| {
                cx.stop_propagation();
                window.remove_window();
                cx.set_global(Open(None));
            }));
        let video = self.video.clone();
        let (action_icon, action_label) = if video.is_some() {
            (IconName::Play, "Play")
        } else {
            (IconName::Pencil, "Edit")
        };
        let edit = div()
            .id("toast-edit")
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .rounded(px(7.))
            .bg(hud::key())
            .hover(|d| d.bg(hsla(0., 0., 1., 0.18)))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(hud::fg())
            .child(Icon::new(action_icon).size(px(13.)))
            .child(action_label)
            .on_click(cx.listener(move |_, _, window, cx| {
                cx.stop_propagation();
                window.remove_window();
                cx.set_global(Open(None));
                match &video {
                    Some((path, _)) => cx.open_with_system(path),
                    None => crate::editor_view::open(id, cx),
                }
            }));
        let check = div()
            .size(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(hud::success_soft())
            .child(
                Icon::new(IconName::Check)
                    .size(px(13.))
                    .text_color(hud::success()),
            );
        let (w, h) = self.size;
        let (title, detail) = match &self.video {
            Some((_, ms)) => (
                "Recording saved",
                format!("{} · {w} × {h} · click to open", clock(*ms)),
            ),
            None => ("Copied to clipboard", format!("{w} × {h} · click to open")),
        };
        // The time left before the preview goes; full while the pointer holds it open.
        let bar = div()
            .absolute()
            .left_0()
            .bottom_0()
            .h(px(2.))
            .bg(hud::accent());
        let bar = if self.hovered {
            bar.w_full().into_any_element()
        } else {
            bar.with_animation(
                ("toast-linger", self.linger),
                Animation::new(LINGER),
                |bar, t| bar.w(relative(1. - t)),
            )
            .into_any_element()
        };
        div()
            .id("toast")
            .size_full()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(8.))
            .px(px(8.))
            .pb(px(12.))
            .rounded(px(14.))
            .bg(hsla(240. / 360., 0.09, 0.1, 0.96))
            .border_1()
            .border_color(hsla(0., 0., 1., 0.1))
            .cursor_pointer()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.hovered && !*hovered {
                    this.linger += 1;
                }
                this.hovered = *hovered;
                cx.notify();
            }))
            .on_click(cx.listener(move |_, _, window, cx| {
                window.remove_window();
                cx.set_global(Open(None));
                library_view::open_at(Some(id), cx);
            }))
            .child(
                div()
                    .h(px(THUMB_H))
                    .w_full()
                    .flex_none()
                    .relative()
                    .rounded(px(8.))
                    .overflow_hidden()
                    .bg(hsla(240. / 360., 0.11, 0.06, 1.))
                    .child(
                        img(self.thumb.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .child(close),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(4.))
                    .child(check)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(18.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(hud::fg())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .line_height(px(15.))
                                    .text_color(hud::fg3())
                                    .truncate()
                                    .child(detail),
                            ),
                    )
                    .child(edit),
            )
            .child(bar)
    }
}

/// `m:ss` (or `h:mm:ss`) for a length in milliseconds.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Shows the preview for capture `id` of `dims` pixels, replacing any preview already showing.
/// `video` is a recording's file and length in milliseconds.
pub fn show(
    id: i64,
    thumb: PathBuf,
    dims: (u32, u32),
    video: Option<(PathBuf, u64)>,
    cx: &mut App,
) {
    dismiss(cx);
    let mut hidden = false;
    // Wayland has no primary display; the layer-shell surface finds its corner there.
    let origin = match cx.primary_display() {
        Some(screen) => {
            let b = screen.bounds();
            point(
                b.origin.x + b.size.width - px(WIDTH + MARGIN),
                b.origin.y + b.size.height - px(HEIGHT + MARGIN),
            )
        }
        None if snapwright_capture::wayland_session() => point(px(0.), px(0.)),
        None => return,
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(crate::session::popup_bounds(
            Bounds::new(origin, size(px(WIDTH), px(HEIGHT))),
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: crate::session::popup_kind(crate::session::Placement::BottomRight(MARGIN), false),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("snapwright".into()),
        ..Default::default()
    };
    let window = match cx.open_window(options, |window, cx| {
        hidden = hide_from_capture(window);
        cx.new(|_| Toast {
            id,
            thumb,
            size: dims,
            video,
            hovered: false,
            linger: 0,
        })
    }) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "quick-access preview unavailable");
            return;
        }
    };
    let handle: AnyWindowHandle = window.into();
    cx.set_global(Open(Some((handle, hidden))));
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
            if cx
                .default_global::<Open>()
                .0
                .is_some_and(|(h, _)| h == handle)
            {
                cx.set_global(Open(None));
            }
        });
    })
    .detach();
}

/// Closes the preview if one is showing. Returns whether a capture must wait [`SETTLE`] for
/// it to leave the screen: only when one was showing and the OS does not keep it out.
pub fn dismiss(cx: &mut App) -> bool {
    let Some((handle, hidden)) = cx.default_global::<Open>().0.take() else {
        return false;
    };
    let closed = handle
        .update(cx, |_, window, _| window.remove_window())
        .is_ok();
    closed && !hidden
}

/// Asks the OS to leave `window` out of screen captures. Returns whether it will.
pub fn hide_from_capture(window: &Window) -> bool {
    if snapwright_capture::EXCLUDES_OWN_WINDOWS {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        // `Window` has its own `window_handle()`; this is raw-window-handle's.
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            return snapwright_capture::exclude_window(h.hwnd.get());
        }
    }
    let _ = window;
    false
}

#[cfg(test)]
mod tests {
    use super::clock;

    #[test]
    fn clock_formats_lengths() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(65_400), "1:05");
        assert_eq!(clock(3_725_000), "1:02:05");
    }
}

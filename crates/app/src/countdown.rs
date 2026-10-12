//! Delayed capture: a small countdown in the middle of the main display, then a normal
//! capture session. The countdown window closes before the screen is grabbed, so it never
//! appears in the capture.

use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, Bounds, Context, FontWeight, Global, IntoElement, ParentElement as _,
    Render, Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, div, hsla, px, size,
};

use crate::session;

const SIDE: f32 = 120.;
/// Time for the window server to take the countdown off screen before the grab.
const SETTLE: Duration = Duration::from_millis(150);

/// Set while a countdown runs, so a second request is ignored.
#[derive(Default)]
struct Running(bool);
impl Global for Running {}

struct Countdown {
    remaining: u32,
}

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(24.))
            .bg(hsla(0., 0., 0.08, 0.85))
            .text_color(hsla(0., 0., 1., 1.))
            .text_size(px(64.))
            .font_weight(FontWeight::BOLD)
            .child(self.remaining.to_string())
    }
}

/// Starts a capture session after `seconds`, showing a countdown meanwhile.
pub fn start(seconds: u32, cx: &mut App) {
    if cx.default_global::<Running>().0 {
        return;
    }
    cx.set_global(Running(true));
    let bounds = Bounds::centered(None, size(px(SIDE), px(SIDE)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(session::popup_bounds(bounds))),
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
    let window = cx
        .open_window(options, |_, cx| {
            cx.new(|_| Countdown {
                remaining: seconds.max(1),
            })
        })
        .map_err(|e| tracing::warn!(error = %e, "countdown window unavailable"))
        .ok();
    cx.spawn(async move |cx| {
        for left in (1..=seconds).rev() {
            if let Some(w) = window {
                let _ = w.update(cx, |view, _, cx| {
                    view.remaining = left;
                    cx.notify();
                });
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }
        if let Some(w) = window {
            let _ = w.update(cx, |_, window, _| window.remove_window());
            cx.background_executor().timer(SETTLE).await;
        }
        cx.update(|cx| {
            cx.set_global(Running(false));
            // Timing starts when the capture does, not when the countdown began.
            session::start(Instant::now(), cx);
        });
    })
    .detach();
}

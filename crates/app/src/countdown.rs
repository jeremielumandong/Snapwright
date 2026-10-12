//! Delayed capture: a small countdown in the middle of the main display, then a normal
//! capture session. The countdown window closes before the screen is grabbed, so it never
//! appears in the capture.

use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, Bounds, Context, FontWeight, Global, IntoElement, ParentElement as _,
    Render, Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, div,
    hsla, px, size,
};

use crate::session;
use crate::theme::hud;

const SIDE: f32 = 148.;
/// Most dots shown under the number, one per second.
const MAX_DOTS: u32 = 10;
/// Time for the window server to take the countdown off screen before the grab.
const SETTLE: Duration = Duration::from_millis(150);

/// Set while a countdown runs, so a second request is ignored.
#[derive(Default)]
struct Running(bool);
impl Global for Running {}

struct Countdown {
    remaining: u32,
    total: u32,
}

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dots = (1..=self.total.min(MAX_DOTS)).map(|i| {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(if i <= self.remaining {
                    hud::fg()
                } else {
                    hsla(0., 0., 1., 0.18)
                })
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .rounded(px(32.))
            .bg(hsla(240. / 360., 0.09, 0.1, 0.9))
            .border_1()
            .border_color(hsla(0., 0., 1., 0.1))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(hud::fg3())
                    .child("Capturing in"),
            )
            .child(
                div()
                    .text_size(px(60.))
                    .line_height(px(60.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hud::fg())
                    .child(self.remaining.to_string()),
            )
            .child(div().mt(px(8.)).flex().gap(px(5.)).children(dots))
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
        kind: session::popup_kind(session::Placement::Centered, false),
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
                total: seconds.max(1),
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

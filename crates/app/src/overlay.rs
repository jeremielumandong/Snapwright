//! The selection overlay: one borderless window per display showing that display's frozen
//! frame. Drag to select; Enter or releasing the mouse copies, Esc cancels. A click without a
//! drag takes the whole display.

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{
    Bounds, Context, CursorStyle, FocusHandle, Hsla, ImageSource, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, RenderImage, SharedString, Size, Styled as _,
    Window, div, hsla, img, point, px, size,
};
use snapwright_capture::{Frame, Rect};

use crate::session;

/// Selections smaller than this (logical pixels) count as a click.
const MIN_DRAG: f32 = 4.0;

pub struct Overlay {
    display_id: u64,
    /// The frozen frame. Its pixels live in the image (GPUI takes BGRA, so no conversion and
    /// no copy); crops read them back from there.
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    focus: FocusHandle,
    drag_from: Option<Point<Pixels>>,
    selection: Option<Bounds<Pixels>>,
    viewport: Size<Pixels>,
    pressed_at: Instant,
    shown: bool,
}

impl Overlay {
    pub fn new(
        frame: Frame,
        pressed_at: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Frame {
            display_id,
            width,
            height,
            data,
        } = frame;
        let buffer = image::RgbaImage::from_raw(width, height, data)
            .unwrap_or_else(|| image::RgbaImage::new(width, height));
        let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            display_id,
            image,
            width,
            height,
            focus,
            drag_from: None,
            selection: None,
            viewport: size(px(1.), px(1.)),
            pressed_at,
            shown: false,
        }
    }

    /// The selection in frame pixels, or the whole display when nothing is selected.
    fn selected_rect(&self) -> Rect {
        let Some(sel) = self.selection else {
            return Rect::new(0, 0, self.width, self.height);
        };
        let sx = self.width as f32 / f32::from(self.viewport.width).max(1.0);
        let sy = self.height as f32 / f32::from(self.viewport.height).max(1.0);
        let x0 = (f32::from(sel.origin.x) * sx).round();
        let y0 = (f32::from(sel.origin.y) * sy).round();
        let x1 = (f32::from(sel.origin.x + sel.size.width) * sx).round();
        let y1 = (f32::from(sel.origin.y + sel.size.height) * sy).round();
        Rect::new(
            x0 as i32,
            y0 as i32,
            (x1 - x0).max(1.0) as u32,
            (y1 - y0).max(1.0) as u32,
        )
    }

    fn deliver(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let r = self.selected_rect();
        let Some(bytes) = self.image.as_bytes(0) else {
            return;
        };
        // Crop straight from the image's pixels: one copy of the selected rows only.
        let Some(r) = r.intersect(&Rect::new(0, 0, self.width, self.height)) else {
            return;
        };
        let stride = self.width as usize * 4;
        let start = r.y as usize * stride + r.x as usize * 4;
        let Some(rows) = bytes.get(start..) else {
            return;
        };
        let frame = Frame::from_bgrx_rows(self.display_id, r.width, r.height, stride, rows);
        session::deliver(frame, self.pressed_at, window, cx);
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag_from = Some(e.position);
        self.selection = None;
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(from) = self.drag_from
            && e.pressed_button == Some(MouseButton::Left)
        {
            let to = e.position;
            let origin = point(from.x.min(to.x), from.y.min(to.y));
            let size = size((from.x - to.x).abs(), (from.y - to.y).abs());
            self.selection = Some(Bounds::new(origin, size));
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_from = None;
        if self.selection.is_some_and(|s| {
            f32::from(s.size.width) < MIN_DRAG || f32::from(s.size.height) < MIN_DRAG
        }) {
            self.selection = None;
        }
        self.deliver(window, cx);
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => session::finish(window, cx),
            "enter" => self.deliver(window, cx),
            _ => {}
        }
    }
}

fn shade() -> Hsla {
    hsla(0., 0., 0., 0.45)
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = window.viewport_size();
        if !self.shown {
            self.shown = true;
            let pressed_at = self.pressed_at;
            let display_id = self.display_id;
            window.on_next_frame(move |_, _| {
                tracing::info!(
                    display_id,
                    ms = pressed_at.elapsed().as_secs_f64() * 1000.0,
                    "overlay visible (from hotkey)"
                );
            });
        }

        let vw = self.viewport.width;
        let vh = self.viewport.height;
        let mut root = div()
            .id("overlay")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .cursor(CursorStyle::Crosshair)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                img(ImageSource::Render(self.image.clone()))
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            );

        match self.selection {
            None => {
                root = root.child(div().absolute().size_full().bg(shade()));
            }
            Some(sel) => {
                let x0 = sel.origin.x;
                let y0 = sel.origin.y;
                let x1 = sel.origin.x + sel.size.width;
                let y1 = sel.origin.y + sel.size.height;
                let band = |x: Pixels, y: Pixels, w: Pixels, h: Pixels| {
                    div().absolute().left(x).top(y).w(w).h(h).bg(shade())
                };
                let r = self.selected_rect();
                let label: SharedString = format!("{} × {}", r.width, r.height).into();
                let label_y = if y0 > px(28.) {
                    y0 - px(26.)
                } else {
                    y1 + px(6.)
                };
                root = root
                    .child(band(px(0.), px(0.), vw, y0))
                    .child(band(px(0.), y1, vw, vh - y1))
                    .child(band(px(0.), y0, x0, sel.size.height))
                    .child(band(x1, y0, vw - x1, sel.size.height))
                    .child(
                        div()
                            .absolute()
                            .left(x0)
                            .top(y0)
                            .w(sel.size.width)
                            .h(sel.size.height)
                            .border_1()
                            .border_color(hsla(0., 0., 1., 0.9)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(x0)
                            .top(label_y)
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(hsla(0., 0., 0., 0.75))
                            .text_color(hsla(0., 0., 1., 1.))
                            .text_size(px(12.))
                            .child(label),
                    );
            }
        }
        root
    }
}

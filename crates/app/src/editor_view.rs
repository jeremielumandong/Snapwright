//! The editor window: annotate a capture with arrows, shapes, text, numbered steps,
//! highlights, blur and pixelation, and crop it. Edits are a [`Scene`] over the untouched
//! original, saved with the capture in the library so they stay editable.
//!
//! The preview is drawn by the same renderer as the export (on a background thread, newest
//! request wins), so what is on screen is exactly what gets copied.

use std::sync::{Arc, Mutex};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, CursorStyle, FocusHandle, Global,
    ImageSource, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, hsla, img, point, px, size,
};
use snapwright_editor::{
    Color, History, Image, Kind, Point, Rect, Renderer, Scene, Shape, render as render_scene,
};
use snapwright_library::{EditedImage, Library};

use crate::session::{self, LibraryService};
use crate::{library_view, output};

const TOOLBAR_H: f32 = 52.;
const STATUS_H: f32 = 28.;
const PAD: f32 = 16.;
/// Stroke widths offered, in image pixels.
const WIDTHS: [(f32, &str); 3] = [(2., "Thin"), (4., "Medium"), (8., "Thick")];
/// How far from a shape a click still picks it, in screen pixels.
const PICK: f32 = 5.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Arrow,
    Line,
    Rectangle,
    Ellipse,
    Highlight,
    Text,
    Step,
    Blur,
    Pixelate,
    Crop,
}

impl Tool {
    const ALL: [Tool; 11] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Line,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Highlight,
        Tool::Text,
        Tool::Step,
        Tool::Blur,
        Tool::Pixelate,
        Tool::Crop,
    ];

    fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Highlight => "Highlight",
            Tool::Text => "Text",
            Tool::Step => "Step",
            Tool::Blur => "Blur",
            Tool::Pixelate => "Pixelate",
            Tool::Crop => "Crop",
        }
    }

    /// Single-key shortcut.
    fn key(self) -> &'static str {
        match self {
            Tool::Select => "v",
            Tool::Arrow => "a",
            Tool::Line => "l",
            Tool::Rectangle => "r",
            Tool::Ellipse => "e",
            Tool::Highlight => "h",
            Tool::Text => "t",
            Tool::Step => "n",
            Tool::Blur => "b",
            Tool::Pixelate => "p",
            Tool::Crop => "c",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Tool::Select => IconName::MousePointer2,
            Tool::Arrow => IconName::MoveUpRight,
            Tool::Line => IconName::Slash,
            Tool::Rectangle => IconName::Square,
            Tool::Ellipse => IconName::Circle,
            Tool::Highlight => IconName::Highlighter,
            Tool::Text => IconName::Type,
            Tool::Step => IconName::CircleDot,
            Tool::Blur => IconName::Droplets,
            Tool::Pixelate => IconName::Grid3x3,
            Tool::Crop => IconName::Crop,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click a shape to select it, drag to move it, Delete removes it",
            Tool::Arrow | Tool::Line => "Drag to draw; hold Shift for 45° angles",
            Tool::Rectangle | Tool::Ellipse => "Drag to draw; hold Shift for a square or circle",
            Tool::Highlight => "Drag over text to highlight it",
            Tool::Text => "Click to type; Enter finishes, Shift+Enter starts a new line",
            Tool::Step => "Click to place the next numbered step",
            Tool::Blur | Tool::Pixelate => "Drag over anything private to hide it",
            Tool::Crop => "Drag the area to keep; pick another tool to see the result",
        }
    }

    /// The shape this tool draws when dragged from `a` to `b`.
    fn shape(self, a: Point, b: Point, shift: bool) -> Option<Kind> {
        let rect = || Rect::from_corners(a, if shift { square(a, b) } else { b });
        let seg = || if shift { snap_45(a, b) } else { b };
        Some(match self {
            Tool::Arrow => Kind::Arrow { from: a, to: seg() },
            Tool::Line => Kind::Line { from: a, to: seg() },
            Tool::Rectangle => Kind::Rectangle { rect: rect() },
            Tool::Ellipse => Kind::Ellipse { rect: rect() },
            Tool::Highlight => Kind::Highlight { rect: rect() },
            Tool::Blur => Kind::Blur { rect: rect() },
            Tool::Pixelate => Kind::Pixelate { rect: rect() },
            Tool::Select | Tool::Text | Tool::Step | Tool::Crop => return None,
        })
    }
}

/// `b` moved so the box from `a` is square.
fn square(a: Point, b: Point) -> Point {
    let side = (b.x - a.x).abs().max((b.y - a.y).abs());
    Point::new(
        a.x + side.copysign(b.x - a.x),
        a.y + side.copysign(b.y - a.y),
    )
}

/// `b` moved onto the nearest 45° direction from `a`.
fn snap_45(a: Point, b: Point) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    Point::new(a.x + len * angle.cos(), a.y + len * angle.sin())
}

/// Open editor windows, by capture id.
#[derive(Default)]
struct EditorWindows(Vec<(i64, AnyWindowHandle)>);
impl Global for EditorWindows {}

/// Opens capture `id` in the editor, or brings its editor to the front.
pub fn open(id: i64, cx: &mut App) {
    let existing = cx
        .default_global::<EditorWindows>()
        .0
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, h)| *h);
    if let Some(h) = existing
        && h.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let Some(library) = cx.global::<LibraryService>().0.clone() else {
        tracing::error!("the capture library is unavailable");
        return;
    };
    let lib = library.clone();
    let load = cx.background_executor().spawn(async move {
        let (capture, path) = {
            let lib = lib
                .lock()
                .map_err(|_| anyhow::anyhow!("library poisoned by an earlier panic"))?;
            let capture = lib.get(id)?;
            let path = lib.original_path_of(&capture);
            (capture, path)
        };
        let png = std::fs::read(&path)?;
        let (w, h, rgba) = output::decode_png(&png)?;
        let scene = match &capture.edit {
            Some(edit) => Scene::from_json(&edit.scene).unwrap_or_else(|e| {
                tracing::warn!(id, error = %e, "the saved edit could not be read; starting over");
                Scene::default()
            }),
            None => Scene::default(),
        };
        anyhow::Ok((Image::new(w, h, rgba)?, scene))
    });
    cx.spawn(async move |cx| match load.await {
        Ok((image, scene)) => cx.update(|cx| open_window(id, library, image, scene, cx)),
        Err(e) => tracing::error!(id, error = %e, "could not open the capture for editing"),
    })
    .detach();
}

fn open_window(id: i64, library: Arc<Mutex<Library>>, image: Image, scene: Scene, cx: &mut App) {
    let (iw, ih) = (image.width as f32, image.height as f32);
    let (mut w, mut h) = (iw + 2. * PAD, ih + TOOLBAR_H + STATUS_H + 2. * PAD);
    if let Some(d) = cx.primary_display() {
        let b = d.bounds().size;
        w = w.min(f32::from(b.width) * 0.9);
        h = h.min(f32::from(b.height) * 0.9);
    }
    let bounds = Bounds::centered(None, size(px(w.max(900.)), px(h.max(560.))), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(760.), px(420.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Snapwright Editor".into()),
            ..Default::default()
        }),
        app_id: Some("snapwright".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| EditorView::new(id, library, image, scene, window, cx))
    }) {
        Ok((handle, _)) => cx.default_global::<EditorWindows>().0.push((id, handle)),
        Err(e) => tracing::error!(error = %e, "could not open the editor window"),
    }
}

/// A pointer drag in progress.
enum Drag {
    /// Drawing the last shape in the scene.
    Create {
        start: Point,
        before: Scene,
    },
    /// Moving the selected shape.
    Move {
        last: Point,
        before: Scene,
    },
    Crop {
        start: Point,
        before: Scene,
    },
}

/// Text being typed into the shape at `index`.
struct Typing {
    index: usize,
    before: Scene,
}

pub struct EditorView {
    id: i64,
    library: Arc<Mutex<Library>>,
    renderer: Arc<Mutex<Renderer>>,
    width: u32,
    height: u32,
    scene: Scene,
    /// The scene as last saved, to tell whether there is anything to save.
    saved: Scene,
    history: History,
    tool: Tool,
    color: Color,
    stroke: f32,
    selected: Option<usize>,
    drag: Option<Drag>,
    typing: Option<Typing>,
    /// The latest rendered preview and the part of the image it shows.
    preview: Option<(Arc<RenderImage>, Rect)>,
    rendering: bool,
    render_again: bool,
    status: SharedString,
    focus: FocusHandle,
    _release: Subscription,
}

impl EditorView {
    fn new(
        id: i64,
        library: Arc<Mutex<Library>>,
        image: Image,
        scene: Scene,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            // Closing keeps the work: unsaved edits are saved on the way out.
            let _ = this.update(cx, |this, cx| this.save(false, cx));
            true
        });
        let release = cx.on_release(move |_, cx| {
            cx.default_global::<EditorWindows>()
                .0
                .retain(|(i, _)| *i != id);
        });
        let mut view = Self {
            id,
            library,
            width: image.width,
            height: image.height,
            renderer: Arc::new(Mutex::new(Renderer::new(image))),
            saved: scene.clone(),
            scene,
            history: History::default(),
            tool: Tool::Arrow,
            color: Color::RED,
            stroke: WIDTHS[1].0,
            selected: None,
            drag: None,
            typing: None,
            preview: None,
            rendering: false,
            render_again: false,
            status: SharedString::default(),
            focus,
            _release: release,
        };
        view.refresh(window, cx);
        view
    }

    // ------------------------------------------------------------------ preview

    /// The full image, or the part the crop keeps, in image pixels.
    fn crop_region(&self, crop: bool) -> Rect {
        match self
            .scene
            .crop
            .filter(|_| crop)
            .and_then(|c| c.pixels_within(self.width, self.height))
        {
            Some((x, y, w, h)) => Rect::new(x as f32, y as f32, w as f32, h as f32),
            None => Rect::new(0., 0., self.width as f32, self.height as f32),
        }
    }

    /// Renders the preview again, after any change to the scene or tool.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        if self.rendering {
            self.render_again = true;
            return;
        }
        self.rendering = true;
        let crop = self.tool != Tool::Crop;
        let shown = self.crop_region(crop);
        let scene = self.scene.clone();
        let renderer = self.renderer.clone();
        let task = cx.background_executor().spawn(async move {
            let out = renderer
                .lock()
                .map_err(|_| anyhow::anyhow!("renderer poisoned by an earlier panic"))?
                .render(&scene, crop);
            // GPUI takes BGRA.
            let mut px = out.rgba;
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            let buffer = image::RgbaImage::from_raw(out.width, out.height, px)
                .ok_or_else(|| anyhow::anyhow!("rendered image has the wrong size"))?;
            anyhow::Ok(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.rendering = false;
                match result {
                    Ok(image) => {
                        if let Some((old, _)) = this.preview.replace((image, shown)) {
                            // Release the old frame's texture now rather than at window close.
                            let _ = window.drop_image(old);
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "rendering the preview failed"),
                }
                if std::mem::take(&mut this.render_again) {
                    this.refresh(window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Where the preview sits in the window: its top-left corner and window pixels per image
    /// pixel. Never upscaled past 1:1 on the display.
    fn layout(&self, window: &Window) -> (gpui_kit::Point<Pixels>, f32, Rect) {
        let shown = self
            .preview
            .as_ref()
            .map_or_else(|| self.crop_region(self.tool != Tool::Crop), |(_, r)| *r);
        let vp = window.viewport_size();
        let aw = (f32::from(vp.width) - 2. * PAD).max(1.);
        let ah = (f32::from(vp.height) - TOOLBAR_H - STATUS_H - 2. * PAD).max(1.);
        let sf = window.scale_factor();
        let mut scale = (aw / shown.w).min(ah / shown.h).min(1. / sf);
        if scale * sf > 0.98 {
            // Close enough to 1:1 (displays report scales like 1.0002): show it exactly so.
            scale = 1. / sf;
        }
        // On whole device pixels: a half-pixel offset would blur a 1:1 preview.
        let snap = |v: f32| (v * sf).round() / sf;
        let x = snap(PAD + (aw - shown.w * scale) / 2.);
        let y = snap(TOOLBAR_H + PAD + (ah - shown.h * scale) / 2.);
        (point(px(x), px(y)), scale, shown)
    }

    fn to_image(&self, p: gpui_kit::Point<Pixels>, window: &Window) -> Point {
        let (origin, scale, shown) = self.layout(window);
        Point::new(
            f32::from(p.x - origin.x) / scale + shown.x,
            f32::from(p.y - origin.y) / scale + shown.y,
        )
    }

    fn to_window(&self, r: Rect, window: &Window) -> (Pixels, Pixels, Pixels, Pixels) {
        let (origin, scale, shown) = self.layout(window);
        (
            origin.x + px((r.x - shown.x) * scale),
            origin.y + px((r.y - shown.y) * scale),
            px(r.w * scale),
            px(r.h * scale),
        )
    }

    // ------------------------------------------------------------------ editing

    fn set_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_typing();
        let crop_changed = (self.tool == Tool::Crop) != (tool == Tool::Crop);
        self.tool = tool;
        if tool != Tool::Select {
            self.selected = None;
        }
        if crop_changed {
            self.refresh(window, cx);
        }
        cx.notify();
    }

    /// Applies `change` to the scene as one undoable step.
    fn change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Scene),
    ) {
        let before = self.scene.clone();
        change(&mut self.scene);
        if self.scene != before {
            self.history.record(before);
            self.refresh(window, cx);
        }
    }

    fn set_color(&mut self, color: Color, window: &mut Window, cx: &mut Context<Self>) {
        self.color = color;
        if let Some(i) = self.selected {
            self.change(window, cx, |s| {
                if let Some(shape) = s.shapes.get_mut(i) {
                    shape.color = color;
                }
            });
        }
        cx.notify();
    }

    fn set_stroke(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.stroke = width;
        if let Some(i) = self.selected {
            self.change(window, cx, |s| {
                if let Some(shape) = s.shapes.get_mut(i) {
                    match &mut shape.kind {
                        Kind::Text { size, .. } => *size = text_size(width),
                        _ => shape.width = width,
                    }
                }
            });
        }
        cx.notify();
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.selected.take() {
            self.change(window, cx, |s| {
                if i < s.shapes.len() {
                    s.shapes.remove(i);
                }
            });
        }
    }

    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_typing();
        if self.history.undo(&mut self.scene) {
            self.selected = None;
            self.refresh(window, cx);
        }
    }

    fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_typing();
        if self.history.redo(&mut self.scene) {
            self.selected = None;
            self.refresh(window, cx);
        }
    }

    fn shape(&self, kind: Kind) -> Shape {
        Shape {
            kind,
            color: self.color,
            width: self.stroke,
        }
    }

    /// Ends text entry: empty text is dropped, anything else becomes one undoable step.
    fn finish_typing(&mut self) {
        let Some(Typing { index, before }) = self.typing.take() else {
            return;
        };
        if self
            .scene
            .shapes
            .get(index)
            .is_some_and(Shape::is_degenerate)
        {
            self.scene.shapes.remove(index);
            self.selected = None;
        }
        if self.scene != before {
            self.history.record(before);
        }
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.finish_typing();
        let p = self.to_image(e.position, window);
        let (_, scale, _) = self.layout(window);
        let tolerance = PICK / scale;
        let before = self.scene.clone();
        match self.tool {
            Tool::Select => {
                self.selected = self.scene.hit(p, tolerance);
                if self.selected.is_some() {
                    self.drag = Some(Drag::Move { last: p, before });
                }
            }
            Tool::Text => {
                let hit = self
                    .scene
                    .hit(p, tolerance)
                    .filter(|&i| matches!(self.scene.shapes[i].kind, Kind::Text { .. }));
                let index = match hit {
                    Some(i) => i,
                    None => {
                        self.scene.shapes.push(self.shape(Kind::Text {
                            at: p,
                            text: String::new(),
                            size: text_size(self.stroke),
                        }));
                        self.scene.shapes.len() - 1
                    }
                };
                self.selected = Some(index);
                self.typing = Some(Typing { index, before });
            }
            Tool::Step => {
                let number = self.scene.next_step();
                let step = self.shape(Kind::Step { center: p, number });
                self.change(window, cx, |s| s.shapes.push(step));
                self.selected = Some(self.scene.shapes.len() - 1);
            }
            Tool::Crop => {
                self.drag = Some(Drag::Crop { start: p, before });
            }
            tool => {
                if let Some(kind) = tool.shape(p, p, false) {
                    self.scene.shapes.push(self.shape(kind));
                    self.selected = Some(self.scene.shapes.len() - 1);
                    self.drag = Some(Drag::Create { start: p, before });
                }
            }
        }
        self.refresh(window, cx);
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        let p = self.to_image(e.position, window);
        let Some(drag) = &mut self.drag else {
            return;
        };
        match drag {
            Drag::Create { start, .. } => {
                let kind = self.tool.shape(*start, p, e.modifiers.shift);
                if let (Some(kind), Some(last)) = (kind, self.scene.shapes.last_mut()) {
                    last.kind = kind;
                }
            }
            Drag::Move { last, .. } => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                if let Some(shape) = self.selected.and_then(|i| self.scene.shapes.get_mut(i)) {
                    shape.translate(dx, dy);
                }
            }
            Drag::Crop { start, .. } => {
                let (w, h) = (self.width as f32, self.height as f32);
                // Crops land on whole pixels.
                let clamp =
                    |q: Point| Point::new(q.x.round().clamp(0., w), q.y.round().clamp(0., h));
                self.scene.crop = Some(Rect::from_corners(clamp(*start), clamp(p)));
            }
        }
        self.refresh(window, cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        match drag {
            Drag::Create { before, .. } => {
                if self.scene.shapes.last().is_some_and(Shape::is_degenerate) {
                    // A click, not a drag: nothing drawn.
                    self.scene = before;
                    self.selected = None;
                } else {
                    self.history.record(before);
                }
            }
            Drag::Move { before, .. } => self.history.record(before),
            Drag::Crop { before, .. } => {
                if self.scene.crop.is_some_and(|c| c.w < 4. || c.h < 4.) {
                    self.scene = before;
                } else {
                    self.history.record(before);
                }
            }
        }
        self.refresh(window, cx);
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &e.keystroke;
        let mods = ks.modifiers;
        let key = ks.key.as_str();
        if mods.secondary() {
            match key {
                "z" if mods.shift => self.redo(window, cx),
                "z" => self.undo(window, cx),
                "y" => self.redo(window, cx),
                "c" => self.copy(cx),
                "s" => self.save(false, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if let Some(Typing { index, .. }) = self.typing {
            let Some(Kind::Text { text, .. }) =
                self.scene.shapes.get_mut(index).map(|s| &mut s.kind)
            else {
                self.typing = None;
                return;
            };
            match key {
                "escape" => self.finish_typing(),
                "enter" if !mods.shift => self.finish_typing(),
                "enter" => text.push('\n'),
                "backspace" => {
                    text.pop();
                }
                _ => match &ks.key_char {
                    Some(c) if !c.chars().any(char::is_control) => text.push_str(c),
                    _ => return,
                },
            }
            cx.stop_propagation();
            self.refresh(window, cx);
            return;
        }
        let nudge = if mods.shift { 10. } else { 1. };
        match key {
            "delete" | "backspace" => self.delete_selected(window, cx),
            "escape" => {
                if self.selected.take().is_none() {
                    self.set_tool(Tool::Select, window, cx);
                }
                cx.notify();
            }
            "left" | "right" | "up" | "down" => {
                let Some(i) = self.selected else {
                    return;
                };
                let (dx, dy) = match key {
                    "left" => (-nudge, 0.),
                    "right" => (nudge, 0.),
                    "up" => (0., -nudge),
                    _ => (0., nudge),
                };
                self.change(window, cx, |s| {
                    if let Some(shape) = s.shapes.get_mut(i) {
                        shape.translate(dx, dy);
                    }
                });
            }
            "[" | "]" => {
                let at = WIDTHS
                    .iter()
                    .position(|(w, _)| *w == self.stroke)
                    .unwrap_or(1);
                let next = if key == "[" {
                    at.saturating_sub(1)
                } else {
                    (at + 1).min(WIDTHS.len() - 1)
                };
                self.set_stroke(WIDTHS[next].0, window, cx);
            }
            _ => {
                if let Some(n) = key.parse::<usize>().ok().filter(|n| (1..=7).contains(n)) {
                    self.set_color(Color::PRESETS[n - 1], window, cx);
                } else if let Some(tool) = Tool::ALL.into_iter().find(|t| t.key() == key) {
                    self.set_tool(tool, window, cx);
                } else {
                    return;
                }
            }
        }
        cx.stop_propagation();
    }

    // ------------------------------------------------------------------ output

    /// Renders the edited image at full size and encodes it, off the UI thread.
    fn export(&self, cx: &mut Context<Self>) -> gpui_kit::Task<anyhow::Result<(Image, Vec<u8>)>> {
        let scene = self.scene.clone();
        let renderer = self.renderer.clone();
        cx.background_executor().spawn(async move {
            let base = renderer
                .lock()
                .map_err(|_| anyhow::anyhow!("renderer poisoned by an earlier panic"))?
                .base()
                .clone();
            let out = render_scene(&base, &scene);
            let png = output::encode_png(out.width, out.height, &out.rgba)?;
            Ok((out, png))
        })
    }

    /// Puts the edited image on the clipboard.
    fn copy(&mut self, cx: &mut Context<Self>) {
        self.finish_typing();
        let export = self.export(cx);
        cx.spawn(async move |this, cx| match export.await {
            Ok((out, png)) => {
                let _ = this.update(cx, |this, cx| {
                    session::copy_image(out.width, out.height, out.rgba, png, cx);
                    tracing::info!("edited image copied");
                    this.status = "Copied".into();
                    cx.notify();
                });
            }
            Err(e) => tracing::error!(error = %e, "copying the edited image failed"),
        })
        .detach();
    }

    /// Saves the scene and its rendered image into the library (an empty scene reverts to the
    /// original), and with `copy` also puts the image on the clipboard.
    fn save(&mut self, copy: bool, cx: &mut Context<Self>) {
        self.finish_typing();
        if self.scene == self.saved && !copy {
            return;
        }
        let scene = self.scene.clone();
        let export = self.export(cx);
        let library = self.library.clone();
        let id = self.id;
        let changed = self.scene != self.saved;
        cx.spawn(async move |this, cx| {
            let saved = async {
                let (out, png) = export.await?;
                if changed {
                    let mut lib = library
                        .lock()
                        .map_err(|_| anyhow::anyhow!("library poisoned by an earlier panic"))?;
                    if scene.is_empty() {
                        lib.revert_edit(id)?;
                    } else {
                        lib.save_edit(
                            id,
                            &scene.to_json(),
                            &EditedImage {
                                png: &png,
                                rgba: &out.rgba,
                                width: out.width,
                                height: out.height,
                            },
                        )?;
                    }
                }
                anyhow::Ok((out, png))
            }
            .await;
            match saved {
                Ok((out, png)) => {
                    tracing::info!(id, "edit saved");
                    cx.update(|cx| {
                        library_view::notify_changed(cx);
                        if copy {
                            session::copy_image(out.width, out.height, out.rgba, png, cx);
                        }
                    });
                    let _ = this.update(cx, |this, cx| {
                        this.saved = scene;
                        this.status = if copy { "Saved and copied" } else { "Saved" }.into();
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!(id, error = %e, "saving the edit failed");
                    let _ = this.update(cx, |this, cx| {
                        this.status = "Saving failed; see the log".into();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------ view

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut tools = div().flex().items_center().gap(px(2.));
        for tool in Tool::ALL {
            tools = tools.child(
                Button::new(tool.name())
                    .ghost()
                    .small()
                    .icon(Icon::new(tool.icon()))
                    .selected(self.tool == tool)
                    .tooltip(format!("{} ({})", tool.name(), tool.key().to_uppercase()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_tool(tool, window, cx);
                    })),
            );
        }
        let mut colors = div().flex().items_center().gap(px(6.));
        for (i, c) in Color::PRESETS.into_iter().enumerate() {
            let selected = c == self.color;
            colors = colors.child(
                div()
                    .id(("color", i))
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(gpui_kit::rgb(
                        (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b),
                    ))
                    .border_2()
                    .border_color(if selected {
                        theme.foreground
                    } else {
                        theme.border
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_color(c, window, cx);
                    })),
            );
        }
        let mut widths = div().flex().items_center().gap(px(2.));
        for (i, (w, name)) in WIDTHS.into_iter().enumerate() {
            widths = widths.child(
                Button::new(("width", i))
                    .ghost()
                    .small()
                    .selected(self.stroke == w)
                    .tooltip(format!("{name} ([ and ] to change)"))
                    .child(
                        div()
                            .w(px(16.))
                            .h(px(w.min(8.)))
                            .rounded_full()
                            .bg(theme.foreground),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_stroke(w, window, cx);
                    })),
            );
        }
        let sep = || div().w(px(1.)).h(px(24.)).bg(theme.border);
        div()
            .h(px(TOOLBAR_H))
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(tools)
            .child(sep())
            .child(colors)
            .child(sep())
            .child(widths)
            .child(sep())
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(
                        Button::new("undo")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::Undo2))
                            .tooltip("Undo")
                            .disabled(!self.history.can_undo())
                            .on_click(cx.listener(|this, _, window, cx| this.undo(window, cx))),
                    )
                    .child(
                        Button::new("redo")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::Redo2))
                            .tooltip("Redo")
                            .disabled(!self.history.can_redo())
                            .on_click(cx.listener(|this, _, window, cx| this.redo(window, cx))),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("copy")
                    .small()
                    .label("Copy")
                    .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
            )
            .child(
                Button::new("done")
                    .primary()
                    .small()
                    .label("Done")
                    .tooltip("Save, copy and close")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.save(true, cx);
                        window.remove_window();
                    })),
            )
    }
}

/// Text size for stroke width `width`: thin, medium and thick give 20, 28 and 44 px.
fn text_size(width: f32) -> f32 {
    12. + width * 4.
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (origin, scale, shown) = self.layout(window);
        let toolbar = self.render_toolbar(cx).into_any_element();
        let theme = cx.theme();
        let canvas = div()
            .absolute()
            .left_0()
            .top(px(TOOLBAR_H))
            .w_full()
            .h(window.viewport_size().height - px(TOOLBAR_H + STATUS_H))
            .bg(theme.muted)
            .cursor(match self.tool {
                Tool::Select => CursorStyle::Arrow,
                Tool::Text => CursorStyle::IBeam,
                _ => CursorStyle::Crosshair,
            });
        let mut layer = div().absolute().size_full().top_0().left_0();
        if let Some((image, _)) = &self.preview {
            layer = layer.child(
                img(ImageSource::Render(image.clone()))
                    .absolute()
                    .left(origin.x)
                    .top(origin.y)
                    .w(px(shown.w * scale))
                    .h(px(shown.h * scale)),
            );
        }
        let accent = hsla(0.58, 0.9, 0.55, 1.);
        if let Some(shape) = self.selected.and_then(|i| self.scene.shapes.get(i)) {
            let (x, y, w, h) = self.to_window(shape.bounds().inflate(2. / scale), window);
            layer = layer.child(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(w)
                    .h(h)
                    .border_1()
                    .border_color(accent)
                    .when(self.typing.is_some(), |d| d.border_2()),
            );
        }
        if self.tool == Tool::Crop
            && let Some(c) = self.scene.crop
        {
            let (x, y, w, h) = self.to_window(c, window);
            let (ix, iy, iw, ih) = self.to_window(shown, window);
            let shade = hsla(0., 0., 0., 0.55);
            let band = |l, t, w, h| div().absolute().left(l).top(t).w(w).h(h).bg(shade);
            layer = layer
                .child(band(ix, iy, iw, y - iy))
                .child(band(ix, y + h, iw, iy + ih - (y + h)))
                .child(band(ix, y, x - ix, h))
                .child(band(x + w, y, ix + iw - (x + w), h))
                .child(
                    div()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(w)
                        .h(h)
                        .border_1()
                        .border_color(hsla(0., 0., 1., 0.95)),
                );
        }
        let (out_w, out_h) = self.scene.output_size(self.width, self.height);
        let unsaved = self.scene != self.saved;
        div()
            .id("editor")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(canvas)
            // Positions in the layer are window positions, as `layout` computes them.
            .child(layer)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    // Clicks on the toolbar must not draw on the canvas below.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(toolbar),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(STATUS_H))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().truncate().child(self.tool.hint()))
                    .child(format!("{out_w} × {out_h} px"))
                    .child(if unsaved {
                        SharedString::from("Unsaved changes")
                    } else {
                        self.status.clone()
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_constrains_to_squares_and_45_degrees() {
        let a = Point::new(10., 10.);
        assert_eq!(square(a, Point::new(40., 20.)), Point::new(40., 40.));
        assert_eq!(square(a, Point::new(0., 50.)), Point::new(-30., 50.));
        let p = snap_45(a, Point::new(110., 14.));
        assert!(
            (p.y - 10.).abs() < 1e-3 && (p.x - 110.08).abs() < 0.01,
            "{p:?}"
        );
        let d = snap_45(a, Point::new(50., 55.));
        assert!(((d.x - a.x) - (d.y - a.y)).abs() < 1e-3, "{d:?}");
    }

    #[test]
    fn every_tool_has_a_distinct_shortcut() {
        let mut keys: Vec<_> = Tool::ALL.iter().map(|t| t.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Tool::ALL.len());
        assert_eq!(text_size(WIDTHS[1].0), 28.);
    }
}

//! Draws a [`Scene`] over its image with tiny-skia, the rasteriser Emulsion uses. The same
//! code produces the editor's preview and the exported image, so what is seen is what is
//! copied.

use tiny_skia::{
    BlendMode, FillRule, IntSize, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

use crate::geom::{Point, Rect};
use crate::scene::{Color, Kind, Scene, Shape, arrow_head};
use crate::{Error, redact, text};

/// An opaque RGBA image, rows tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, Error> {
        if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
            return Err(Error::Size {
                len: rgba.len(),
                width,
                height,
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// Renders scenes over one image, remembering the redacted image between calls: while a
/// shape is being dragged only the shapes are redrawn, not the blurs under them.
pub struct Renderer {
    base: Image,
    redacted: Option<(Vec<Shape>, Vec<u8>)>,
}

impl Renderer {
    pub fn new(base: Image) -> Self {
        Self {
            base,
            redacted: None,
        }
    }

    pub fn base(&self) -> &Image {
        &self.base
    }

    /// The image with `scene` drawn over it, cropped when `crop` is set.
    pub fn render(&mut self, scene: &Scene, crop: bool) -> Image {
        let (w, h) = (self.base.width, self.base.height);
        let redactions: Vec<Shape> = scene
            .shapes
            .iter()
            .filter(|s| s.kind.is_redaction())
            .cloned()
            .collect();
        let pixels = match &self.redacted {
            Some((done, px)) if *done == redactions => px.clone(),
            _ => {
                let mut px = self.base.rgba.clone();
                for shape in &redactions {
                    redact_into(&mut px, w, h, shape);
                }
                self.redacted = Some((redactions, px.clone()));
                px
            }
        };
        // The base is opaque, so its straight RGBA is already premultiplied, and drawing over
        // it keeps it opaque: the output needs no conversion either.
        let Some(mut pixmap) = IntSize::from_wh(w, h).and_then(|s| Pixmap::from_vec(pixels, s))
        else {
            return self.base.clone();
        };
        for shape in scene.shapes.iter().filter(|s| !s.kind.is_redaction()) {
            draw(&mut pixmap, shape);
        }
        let out = Image {
            width: w,
            height: h,
            rgba: pixmap.take(),
        };
        match scene.crop.filter(|_| crop) {
            Some(c) => crop_image(&out, &c),
            None => out,
        }
    }
}

/// One-off render of `scene` over `base`, cropped: the image to export.
pub fn render(base: &Image, scene: &Scene) -> Image {
    Renderer::new(base.clone()).render(scene, true)
}

fn redact_into(px: &mut [u8], w: u32, h: u32, shape: &Shape) {
    match &shape.kind {
        Kind::Blur { rect } => {
            if let Some(region) = rect.pixels_within(w, h) {
                redact::blur(px, w, h, region, Shape::blur_radius(shape.width));
            }
        }
        Kind::Pixelate { rect } => {
            if let Some(region) = rect.pixels_within(w, h) {
                redact::pixelate(px, w, region, Shape::pixel_cell(shape.width));
            }
        }
        _ => {}
    }
}

fn crop_image(image: &Image, c: &Rect) -> Image {
    let Some((x, y, w, h)) = c.pixels_within(image.width, image.height) else {
        return image.clone();
    };
    let stride = image.width as usize * 4;
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let start = row as usize * stride + x as usize * 4;
        rgba.extend_from_slice(&image.rgba[start..start + w as usize * 4]);
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color.r, color.g, color.b, color.a);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width: width.max(0.5),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// White or black, whichever stands out against `color`.
fn contrast(color: Color) -> Color {
    if color.is_light() {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

fn draw(pixmap: &mut Pixmap, shape: &Shape) {
    let id = Transform::identity();
    let fill = paint(shape.color);
    match &shape.kind {
        Kind::Line { from, to } => {
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            pb.line_to(to.x, to.y);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &fill, &stroke(shape.width), id, None);
            }
        }
        Kind::Arrow { from, to } => draw_arrow(pixmap, *from, *to, shape.width, &fill),
        Kind::Rectangle { rect } => {
            if let Some(r) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.w, rect.h) {
                let path = PathBuilder::from_rect(r);
                pixmap.stroke_path(&path, &fill, &stroke(shape.width), id, None);
            }
        }
        Kind::Ellipse { rect } => {
            if let Some(path) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.w, rect.h)
                .and_then(PathBuilder::from_oval)
            {
                pixmap.stroke_path(&path, &fill, &stroke(shape.width), id, None);
            }
        }
        Kind::Highlight { rect } => {
            if let Some(r) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.w, rect.h) {
                // Multiply keeps dark text under the marker readable, like a real highlighter.
                let mut p = paint(shape.color);
                p.blend_mode = BlendMode::Multiply;
                pixmap.fill_rect(r, &p, id, None);
            }
        }
        Kind::Text { at, text: t, size } => {
            if let Some(path) = text::path(t, *size, *at) {
                let halo = paint(contrast(shape.color));
                pixmap.stroke_path(&path, &halo, &stroke(text::halo(*size)), id, None);
                pixmap.fill_path(&path, &fill, FillRule::Winding, id, None);
            }
        }
        Kind::Step { center, number } => {
            let r = Shape::step_radius(shape.width);
            if let Some(circle) = PathBuilder::from_circle(center.x, center.y, r) {
                let ink = contrast(shape.color);
                pixmap.fill_path(&circle, &fill, FillRule::Winding, id, None);
                pixmap.stroke_path(&circle, &paint(ink), &stroke(2.), id, None);
                let label = number.to_string();
                let size = r * 1.1;
                let (tw, th) = text::measure(&label, size);
                let at = Point::new(center.x - tw / 2., center.y - th / 2.);
                if let Some(path) = text::path(&label, size, at) {
                    pixmap.fill_path(&path, &paint(ink), FillRule::Winding, id, None);
                }
            }
        }
        Kind::Blur { .. } | Kind::Pixelate { .. } => {}
    }
}

fn draw_arrow(pixmap: &mut Pixmap, from: Point, to: Point, width: f32, fill: &Paint<'_>) {
    let len = from.distance(to);
    if len < 0.5 {
        return;
    }
    let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let (head, half) = arrow_head(width);
    // Short arrows shrink their head rather than poke out behind the start.
    let scale = (len / head).min(1.);
    let (head, half) = (head * scale, half * scale);
    let base = Point::new(to.x - ux * head, to.y - uy * head);
    // The shaft ends inside the head so the round cap never shows past it.
    let shaft_end = Point::new(to.x - ux * head * 0.6, to.y - uy * head * 0.6);
    let id = Transform::identity();
    let mut pb = PathBuilder::new();
    pb.move_to(from.x, from.y);
    pb.line_to(shaft_end.x, shaft_end.y);
    if let Some(path) = pb.finish() {
        let s = Stroke {
            line_cap: LineCap::Butt,
            ..stroke(width)
        };
        pixmap.stroke_path(&path, fill, &s, id, None);
        // A round cap only at the tail.
        if let Some(dot) = PathBuilder::from_circle(from.x, from.y, width / 2.) {
            pixmap.fill_path(&dot, fill, FillRule::Winding, id, None);
        }
    }
    let mut pb = PathBuilder::new();
    pb.move_to(to.x, to.y);
    pb.line_to(base.x - uy * half, base.y + ux * half);
    pb.line_to(base.x + uy * half, base.y - ux * half);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, fill, FillRule::Winding, id, None);
    }
}

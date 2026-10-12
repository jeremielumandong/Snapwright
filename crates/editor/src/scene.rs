//! The editable description of an annotated capture: the original image is never changed;
//! a [`Scene`] lists what is drawn over it, in order, and how it is cropped.

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rect, distance_to_segment};
use crate::{Error, text};

/// Bumped when the saved form changes in a way older builds cannot read.
pub const SCENE_VERSION: u32 = 1;

/// Shapes and crop over one image, in that image's pixel coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub version: u32,
    /// The part of the image to keep, if cropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Rect>,
    /// Drawn in order: later shapes are on top. Redactions (blur, pixelate) always apply to
    /// the image itself, under every other shape.
    #[serde(default)]
    pub shapes: Vec<Shape>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            version: SCENE_VERSION,
            crop: None,
            shapes: Vec::new(),
        }
    }
}

impl Scene {
    /// Nothing drawn and nothing cropped: the image as captured.
    pub fn is_empty(&self) -> bool {
        self.crop.is_none() && self.shapes.is_empty()
    }

    pub fn to_json(&self) -> String {
        // A scene is plain data; serialising it cannot fail.
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        let scene: Scene = serde_json::from_str(json).map_err(|e| Error::Scene(e.to_string()))?;
        if scene.version > SCENE_VERSION {
            return Err(Error::Scene(format!(
                "scene version {} is newer than this build understands",
                scene.version
            )));
        }
        Ok(scene)
    }

    /// The topmost shape under `p`, allowing `tolerance` pixels of slack.
    pub fn hit(&self, p: Point, tolerance: f32) -> Option<usize> {
        self.shapes.iter().rposition(|s| s.hit(p, tolerance))
    }

    /// The number the next step marker should show.
    pub fn next_step(&self) -> u32 {
        self.shapes
            .iter()
            .filter_map(|s| match s.kind {
                Kind::Step { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Size of the image this scene produces from a `width` x `height` original.
    pub fn output_size(&self, width: u32, height: u32) -> (u32, u32) {
        match self.crop.and_then(|c| c.pixels_within(width, height)) {
            Some((_, _, w, h)) => (w, h),
            None => (width, height),
        }
    }
}

/// An sRGB colour with straight alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const RED: Color = Color::rgb(0xE5, 0x48, 0x4D);
    pub const ORANGE: Color = Color::rgb(0xF7, 0x6B, 0x15);
    pub const YELLOW: Color = Color::rgb(0xFF, 0xD6, 0x0A);
    pub const GREEN: Color = Color::rgb(0x30, 0xA4, 0x6C);
    pub const BLUE: Color = Color::rgb(0x00, 0x90, 0xFF);
    pub const BLACK: Color = Color::rgb(0x11, 0x11, 0x11);
    pub const WHITE: Color = Color::rgb(0xFF, 0xFF, 0xFF);

    /// The palette the editor offers.
    pub const PRESETS: [Color; 7] = [
        Self::RED,
        Self::ORANGE,
        Self::YELLOW,
        Self::GREEN,
        Self::BLUE,
        Self::BLACK,
        Self::WHITE,
    ];

    /// Whether dark text reads better than light text on this colour.
    pub fn is_light(self) -> bool {
        let l =
            0.2126 * f32::from(self.r) + 0.7152 * f32::from(self.g) + 0.0722 * f32::from(self.b);
        l > 150.
    }
}

/// One annotation: what it is, its colour, and its stroke width (or strength, for redactions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    #[serde(flatten)]
    pub kind: Kind,
    pub color: Color,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    Arrow {
        from: Point,
        to: Point,
    },
    Line {
        from: Point,
        to: Point,
    },
    Rectangle {
        rect: Rect,
    },
    Ellipse {
        rect: Rect,
    },
    /// A translucent marker over the image.
    Highlight {
        rect: Rect,
    },
    Blur {
        rect: Rect,
    },
    Pixelate {
        rect: Rect,
    },
    /// Text whose first line's top-left corner is at `at`.
    Text {
        at: Point,
        text: String,
        size: f32,
    },
    /// A numbered circle centred on `center`.
    Step {
        center: Point,
        number: u32,
    },
}

impl Kind {
    /// Blur and pixelate change the image's own pixels.
    pub fn is_redaction(&self) -> bool {
        matches!(self, Kind::Blur { .. } | Kind::Pixelate { .. })
    }
}

impl Shape {
    /// Radius of a step marker drawn with stroke width `width`.
    pub fn step_radius(width: f32) -> f32 {
        11. + width * 2.5
    }

    /// Gaussian blur radius for a blur drawn with strength `width`.
    pub fn blur_radius(width: f32) -> f32 {
        8. + width * 4.
    }

    /// Block size for a pixelation drawn with strength `width`.
    pub fn pixel_cell(width: f32) -> u32 {
        (6. + width * 2.).round().max(2.) as u32
    }

    /// The area this shape paints, including its stroke and arrowhead.
    pub fn bounds(&self) -> Rect {
        let half = self.width / 2.;
        match &self.kind {
            Kind::Arrow { from, to } => {
                Rect::from_corners(*from, *to).inflate(arrow_head(self.width).0)
            }
            Kind::Line { from, to } => Rect::from_corners(*from, *to).inflate(half),
            Kind::Rectangle { rect } | Kind::Ellipse { rect } => rect.inflate(half),
            Kind::Highlight { rect } | Kind::Blur { rect } | Kind::Pixelate { rect } => *rect,
            Kind::Text { at, text, size } => {
                let (w, h) = text::measure(text, *size);
                Rect::new(at.x, at.y, w, h).inflate(text::halo(*size))
            }
            Kind::Step { center, .. } => {
                let r = Self::step_radius(self.width) + 2.;
                Rect::new(center.x - r, center.y - r, 2. * r, 2. * r)
            }
        }
    }

    /// Whether `p` is on this shape. Outlines are picked by their stroke, filled shapes
    /// anywhere inside.
    pub fn hit(&self, p: Point, tolerance: f32) -> bool {
        let reach = self.width / 2. + tolerance;
        match &self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => {
                distance_to_segment(p, *from, *to) <= reach
            }
            Kind::Rectangle { rect } => {
                rect.inflate(reach).contains(p) && !rect.inflate(-reach).contains(p)
            }
            Kind::Ellipse { rect } => {
                let (a, b) = (rect.w / 2., rect.h / 2.);
                if a <= 0. || b <= 0. {
                    return rect.inflate(reach).contains(p);
                }
                let c = rect.center();
                let r = ((p.x - c.x) / a).hypot((p.y - c.y) / b);
                (r - 1.).abs() * a.min(b) <= reach
            }
            _ => self.bounds().inflate(tolerance).contains(p),
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        match &mut self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => {
                *from = from.offset(dx, dy);
                *to = to.offset(dx, dy);
            }
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Highlight { rect }
            | Kind::Blur { rect }
            | Kind::Pixelate { rect } => *rect = rect.offset(dx, dy),
            Kind::Text { at, .. } => *at = at.offset(dx, dy),
            Kind::Step { center, .. } => *center = center.offset(dx, dy),
        }
    }

    /// Too small to be meant: a click rather than a drag with a drawing tool.
    pub fn is_degenerate(&self) -> bool {
        const MIN: f32 = 3.;
        match &self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => from.distance(*to) < MIN,
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Highlight { rect }
            | Kind::Blur { rect }
            | Kind::Pixelate { rect } => rect.w < MIN || rect.h < MIN,
            Kind::Text { text, .. } => text.trim().is_empty(),
            Kind::Step { .. } => false,
        }
    }
}

/// Arrowhead `(length, half width)` for stroke width `width`.
pub(crate) fn arrow_head(width: f32) -> (f32, f32) {
    let len = (width * 4.5).max(14.);
    (len, len * 0.55)
}

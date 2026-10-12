//! Annotation text: laid out with the bundled Geist font (SIL OFL 1.1, see
//! `assets/Geist-OFL.txt`) and turned into vector paths, so text renders exactly the same on
//! every platform and in every export.

use std::sync::OnceLock;

use ab_glyph::{Font as _, FontRef, OutlineCurve, ScaleFont as _, VariableFont as _};
use tiny_skia::{Path, PathBuilder};

use crate::geom::Point;

static GEIST: &[u8] = include_bytes!("../assets/Geist.ttf");
/// Annotations read better a little heavier than regular text.
const WEIGHT: f32 = 600.;

fn font() -> Option<&'static FontRef<'static>> {
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| {
        let mut font = FontRef::try_from_slice(GEIST).ok()?;
        font.set_variation(b"wght", WEIGHT);
        Some(font)
    })
    .as_ref()
}

/// Line height for text of `size` pixels.
fn line_height(size: f32) -> f32 {
    match font() {
        Some(f) => {
            let s = f.as_scaled(size);
            s.height() + s.line_gap()
        }
        None => size * 1.2,
    }
}

/// Width of the contrasting outline drawn around text of `size` pixels.
pub fn halo(size: f32) -> f32 {
    (size * 0.14).max(2.)
}

/// `(width, height)` of `text` set at `size` pixels. Empty text still has room for a caret.
pub fn measure(text: &str, size: f32) -> (f32, f32) {
    let lines = text.split('\n').count().max(1) as f32;
    let width = text
        .split('\n')
        .map(|line| advance(line, size))
        .fold(size * 0.3, f32::max);
    (width, lines * line_height(size))
}

fn advance(line: &str, size: f32) -> f32 {
    let Some(f) = font() else {
        return line.chars().count() as f32 * size * 0.55;
    };
    let s = f.as_scaled(size);
    let mut x = 0.;
    let mut prev = None;
    for c in line.chars() {
        let id = s.glyph_id(c);
        if let Some(p) = prev {
            x += s.kern(p, id);
        }
        x += s.h_advance(id);
        prev = Some(id);
    }
    x
}

/// The outlines of `text` set at `size` pixels with its top-left corner at `at`, or `None`
/// when there is nothing to draw.
pub fn path(text: &str, size: f32, at: Point) -> Option<Path> {
    let f = font()?;
    let s = f.as_scaled(size);
    let (hs, vs) = (s.h_scale_factor(), s.v_scale_factor());
    let mut pb = PathBuilder::new();
    for (i, line) in text.split('\n').enumerate() {
        let baseline = at.y + s.ascent() + i as f32 * line_height(size);
        let mut pen = at.x;
        let mut prev = None;
        for c in line.chars() {
            let id = s.glyph_id(c);
            if let Some(p) = prev {
                pen += s.kern(p, id);
            }
            if let Some(outline) = f.outline(id) {
                let map = |p: ab_glyph::Point| (pen + p.x * hs, baseline - p.y * vs);
                let mut end: Option<ab_glyph::Point> = None;
                for curve in &outline.curves {
                    let (start, last) = match curve {
                        OutlineCurve::Line(a, b) => (*a, *b),
                        OutlineCurve::Quad(a, _, c) => (*a, *c),
                        OutlineCurve::Cubic(a, _, _, d) => (*a, *d),
                    };
                    if end != Some(start) {
                        if end.is_some() {
                            pb.close();
                        }
                        let (x, y) = map(start);
                        pb.move_to(x, y);
                    }
                    match curve {
                        OutlineCurve::Line(_, b) => {
                            let (x, y) = map(*b);
                            pb.line_to(x, y);
                        }
                        OutlineCurve::Quad(_, b, c) => {
                            let ((x1, y1), (x, y)) = (map(*b), map(*c));
                            pb.quad_to(x1, y1, x, y);
                        }
                        OutlineCurve::Cubic(_, b, c, d) => {
                            let ((x1, y1), (x2, y2), (x, y)) = (map(*b), map(*c), map(*d));
                            pb.cubic_to(x1, y1, x2, y2, x, y);
                        }
                    }
                    end = Some(last);
                }
                if end.is_some() {
                    pb.close();
                }
            }
            pen += s.h_advance(id);
            prev = Some(id);
        }
    }
    pb.finish()
}

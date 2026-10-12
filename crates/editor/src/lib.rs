//! `snapwright-editor` — annotations over a capture, kept apart from it.
//!
//! A [`Scene`] lists shapes (arrows, lines, rectangles, ellipses, highlights, text, numbered
//! steps, blurs and pixelations) and a crop, in the original image's pixels. The original is
//! never changed; [`render`] draws the scene over it to produce the image to export, and the
//! scene is saved beside the capture so it stays editable. [`History`] gives undo and redo.
//!
//! The renderer is tiny-skia, as in Emulsion; the Gaussian blur is ported from Emulsion's
//! filters (MIT). Text uses the bundled Geist font (SIL OFL 1.1).

mod geom;
mod history;
mod redact;
mod render;
mod scene;
mod text;

pub use geom::{Point, Rect};
pub use history::History;
pub use render::{Image, Renderer, render};
pub use scene::{Color, Kind, SCENE_VERSION, Scene, Shape};
pub use text::measure as measure_text;

/// Errors from the editor.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("scene: {0}")]
    Scene(String),
    #[error("{len} bytes is not a {width}x{height} RGBA image")]
    Size { len: usize, width: u32, height: u32 },
}

#[cfg(test)]
mod tests;

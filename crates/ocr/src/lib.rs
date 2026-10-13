//! `snapwright-ocr` — read the text in a capture, on the device.
//!
//! One API over the text recognition each OS already has:
//!
//! * macOS: Apple Vision (`VNRecognizeTextRequest`, accurate level, language correction on).
//! * Windows 10+: `Windows.Media.Ocr` in the user's profile languages.
//! * Linux: [ocrs](https://github.com/robertknight/ocrs), a pure Rust engine. Its two models
//!   (about 12 MB) are fetched once into [`Config::models_dir`] and checked against pinned
//!   hashes; `SNAPWRIGHT_OCR_MODELS` points at a folder that already holds them instead.
//!
//! Nothing leaves the machine except that one model download on Linux. A [`Recognizer`] holds
//! the engine warm; it is meant to live on one background thread, since recognising a
//! full-screen capture takes from tens of milliseconds (Vision) to a second or two (ocrs).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::path::PathBuf;

#[cfg(target_os = "linux")]
use linux::Backend;
#[cfg(target_os = "macos")]
use macos::Backend;
#[cfg(target_os = "windows")]
use windows::Backend;

/// Name of this platform's engine, for logs and the UI.
pub const ENGINE: &str = if cfg!(target_os = "macos") {
    "Apple Vision"
} else if cfg!(target_os = "windows") {
    "Windows OCR"
} else {
    "ocrs"
};

/// Errors from text recognition.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The engine cannot run here (no OCR language installed, unsupported OS).
    #[error("text recognition unavailable: {0}")]
    Unavailable(String),
    /// The OCR models could not be fetched or loaded.
    #[error("OCR model {name}: {message}")]
    Model { name: &'static str, message: String },
    /// The pixels do not make an image of the stated size.
    #[error("{len} bytes is not a {width}x{height} image")]
    Size { len: usize, width: u32, height: u32 },
    /// The platform engine failed.
    #[error("{api}: {message}")]
    Platform { api: &'static str, message: String },
}

/// Shorthand for OCR results.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Byte order of a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// R G B A, as the library stores and decodes images.
    Rgba,
    /// B G R A, as capture frames arrive.
    Bgra,
}

/// Pixels to read: tightly packed rows, four bytes a pixel, alpha ignored.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub order: Order,
}

impl Image<'_> {
    fn check(&self) -> Result<()> {
        let want = self.width as usize * self.height as usize * 4;
        if self.width == 0 || self.height == 0 || self.pixels.len() != want {
            return Err(Error::Size {
                len: self.pixels.len(),
                width: self.width,
                height: self.height,
            });
        }
        Ok(())
    }
}

/// An axis-aligned box in image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// One line of recognised text, in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub bounds: Bounds,
}

/// Joins recognised lines into text, one line per line, dropping empty ones.
pub fn text_of(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        let t = line.text.trim();
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(t);
    }
    out
}

/// Where the engine keeps what it needs.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Folder for downloaded models (Linux only; the other engines ship with the OS).
    pub models_dir: PathBuf,
}

/// A text recognition engine, set up once and reused.
pub struct Recognizer {
    backend: Backend,
}

impl Recognizer {
    /// Sets up the engine. On Linux this fetches the models on first use.
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            backend: Backend::new(config)?,
        })
    }

    /// The lines of text in `image`, top to bottom.
    pub fn recognize(&mut self, image: &Image<'_>) -> Result<Vec<Line>> {
        image.check()?;
        let started = std::time::Instant::now();
        let lines = self.backend.recognize(image)?;
        tracing::debug!(
            engine = ENGINE,
            width = image.width,
            height = image.height,
            lines = lines.len(),
            ms = started.elapsed().as_secs_f64() * 1000.0,
            "recognised"
        );
        Ok(lines)
    }
}

/// `image` resized by `scale` (bilinear), as RGBA. Used to bring small UI text up to the size
/// an engine reads best, or a huge image down under an engine's limit.
fn resized_rgba(image: &Image<'_>, scale: f32) -> (Vec<u8>, u32, u32) {
    let w = ((image.width as f32 * scale).round() as u32).max(1);
    let h = ((image.height as f32 * scale).round() as u32).max(1);
    let (sw, sh) = (image.width as usize, image.height as usize);
    let (r, b) = match image.order {
        Order::Rgba => (0, 2),
        Order::Bgra => (2, 0),
    };
    let px = |x: usize, y: usize, c: usize| f32::from(image.pixels[(y * sw + x) * 4 + c]);
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let fy = ((y as f32 + 0.5) / scale - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w as usize {
            let fx = ((x as f32 + 0.5) / scale - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            for c in [r, 1, b] {
                let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
                let bottom = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
                out.push((top * (1.0 - ty) + bottom * ty).round() as u8);
            }
            out.push(255);
        }
    }
    (out, w, h)
}

/// One tile along an axis: the pixels `start..end` it reads, and the share `own` it answers
/// for (up to halfway into each overlap), so something cut by one tile's edge is counted once,
/// from the tile where it is whole.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
    own: std::ops::Range<usize>,
}

/// Overlapping tiles of at most `tile` pixels covering `0..len`.
fn spans(len: usize, tile: usize, overlap: usize) -> Vec<Span> {
    let step = tile.saturating_sub(overlap).max(1);
    let mut out: Vec<Span> = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + tile).min(len);
        let own_start = if start == 0 { 0 } else { start + overlap / 2 };
        let own_end = if end == len { len } else { end - overlap / 2 };
        out.push(Span {
            start,
            end,
            own: own_start..own_end,
        });
        if end == len {
            return out;
        }
        start += step;
    }
}

/// Sorts lines into reading order: top to bottom, and left to right within a row.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn reading_order(lines: &mut [Line]) {
    lines.sort_by(|a, b| {
        let (ay, by) = (a.bounds.y, b.bounds.y);
        let same_row = ay.abs_diff(by) < a.bounds.height.min(b.bounds.height) / 2;
        if same_row {
            a.bounds.x.cmp(&b.bounds.x)
        } else {
            ay.cmp(&by)
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: u32, y: u32) -> Line {
        Line {
            text: text.into(),
            bounds: Bounds {
                x,
                y,
                width: 50,
                height: 20,
            },
        }
    }

    #[test]
    fn text_joins_non_empty_lines() {
        let lines = [
            line(" Hello ", 0, 0),
            line("  ", 0, 30),
            line("world", 0, 60),
        ];
        assert_eq!(text_of(&lines), "Hello\nworld");
        assert_eq!(text_of(&[]), "");
    }

    #[test]
    fn reading_order_is_rows_then_columns() {
        let mut lines = vec![
            line("c", 0, 100),
            line("b", 300, 4),
            line("a", 10, 0),
            line("d", 200, 98),
        ];
        reading_order(&mut lines);
        let order: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(order, ["a", "b", "c", "d"]);
    }

    #[test]
    fn spans_cover_once_with_overlap() {
        assert_eq!(
            spans(500, 800, 96),
            [Span {
                start: 0,
                end: 500,
                own: 0..500
            }]
        );
        let s = spans(2000, 800, 96);
        assert_eq!((s[0].start, s[s.len() - 1].end), (0, 2000));
        for pair in s.windows(2) {
            assert_eq!(pair[0].end - pair[1].start, 96);
            // The owned shares tile the line with no gap and no overlap.
            assert_eq!(pair[0].own.end, pair[1].own.start);
        }
        assert_eq!(s[s.len() - 1].own.end, 2000);
    }

    #[test]
    fn size_is_checked() {
        let image = Image {
            pixels: &[0; 12],
            width: 2,
            height: 2,
            order: Order::Rgba,
        };
        assert!(matches!(image.check(), Err(Error::Size { len: 12, .. })));
    }

    #[test]
    fn resize_doubles_and_swaps_to_rgba() {
        // One blue pixel, BGRA.
        let image = Image {
            pixels: &[255, 0, 0, 255],
            width: 1,
            height: 1,
            order: Order::Bgra,
        };
        let (rgba, w, h) = resized_rgba(&image, 2.0);
        assert_eq!((w, h), (2, 2));
        assert_eq!(&rgba[..4], &[0, 0, 255, 255]);
    }
}

//! Windows engine: `Windows.Media.Ocr`, in the first of the user's profile languages that
//! has an OCR pack installed (English when none do and the English pack is there).
//!
//! The engine refuses images larger than `OcrEngine::MaxImageDimension` on either side, so a
//! bigger capture is read in overlapping tiles and each line is kept from the tile that owns
//! its centre. Small images are read at 2x, which helps with 1x UI text.

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::core::HSTRING;

use crate::{
    Bounds, Config, Error, Image, Line, Order, Result, reading_order, resized_rgba, spans,
};

/// How far tiles overlap; a line of text is shorter than this in height.
const OVERLAP: usize = 128;

pub struct Backend {
    engine: OcrEngine,
    max: usize,
}

impl Backend {
    pub fn new(_config: &Config) -> Result<Self> {
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .or_else(|_| {
                OcrEngine::TryCreateFromLanguage(&Language::CreateLanguage(&HSTRING::from(
                    "en-US",
                ))?)
            })
            .map_err(|e| {
                Error::Unavailable(format!(
                    "no OCR language pack installed (Settings > Time & language > Language): {e}"
                ))
            })?;
        let max = OcrEngine::MaxImageDimension().unwrap_or(2600).max(256) as usize;
        Ok(Self { engine, max })
    }

    pub fn recognize(&mut self, image: &Image<'_>) -> Result<Vec<Line>> {
        let scale = if image.width.max(image.height) as usize * 2 <= self.max {
            2.0
        } else {
            1.0
        };
        let owned;
        let (bgra, width, height) = if scale != 1.0 || image.order == Order::Rgba {
            // `resized_rgba` gives RGBA; swap to the BGRA the bitmap takes.
            let (mut rgba, w, h) = resized_rgba(image, scale);
            for px in rgba.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
            owned = rgba;
            (owned.as_slice(), w as usize, h as usize)
        } else {
            (image.pixels, image.width as usize, image.height as usize)
        };
        let mut lines = Vec::new();
        for xs in spans(width, self.max, OVERLAP) {
            for ys in spans(height, self.max, OVERLAP) {
                let (tw, th) = (xs.end - xs.start, ys.end - ys.start);
                let mut tile = Vec::with_capacity(tw * th * 4);
                for y in ys.start..ys.end {
                    let row = (y * width + xs.start) * 4;
                    tile.extend_from_slice(&bgra[row..row + tw * 4]);
                }
                for (text, r) in self.read(&tile, tw, th)? {
                    let x = xs.start as f32 + r.X;
                    let y = ys.start as f32 + r.Y;
                    let (cx, cy) = ((x + r.Width / 2.0) as usize, (y + r.Height / 2.0) as usize);
                    if !(xs.own.contains(&cx) && ys.own.contains(&cy)) {
                        continue;
                    }
                    let unscale = |v: f32| (v.max(0.0) / scale).round() as u32;
                    lines.push(Line {
                        text,
                        bounds: Bounds {
                            x: unscale(x),
                            y: unscale(y),
                            width: unscale(r.Width),
                            height: unscale(r.Height),
                        },
                    });
                }
            }
        }
        reading_order(&mut lines);
        Ok(lines)
    }

    /// The lines in one BGRA tile, with their boxes in tile pixels.
    fn read(
        &self,
        bgra: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<(String, windows::Foundation::Rect)>> {
        let platform = |api: &'static str| {
            move |e: windows::core::Error| Error::Platform {
                api,
                message: e.message(),
            }
        };
        let buffer = CryptographicBuffer::CreateFromByteArray(bgra)
            .map_err(platform("CryptographicBuffer::CreateFromByteArray"))?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
            BitmapAlphaMode::Ignore,
        )
        .map_err(platform("SoftwareBitmap::CreateCopyWithAlphaFromBuffer"))?;
        let result = self
            .engine
            .RecognizeAsync(&bitmap)
            .and_then(|op| op.join())
            .map_err(platform("OcrEngine::RecognizeAsync"))?;
        let mut out = Vec::new();
        for line in result.Lines().map_err(platform("OcrResult::Lines"))? {
            let text = line.Text().map_err(platform("OcrLine::Text"))?.to_string();
            let mut bounds: Option<windows::Foundation::Rect> = None;
            for word in line.Words().map_err(platform("OcrLine::Words"))? {
                let r = word
                    .BoundingRect()
                    .map_err(platform("OcrWord::BoundingRect"))?;
                bounds = Some(match bounds {
                    None => r,
                    Some(b) => {
                        let x = b.X.min(r.X);
                        let y = b.Y.min(r.Y);
                        windows::Foundation::Rect {
                            X: x,
                            Y: y,
                            Width: (b.X + b.Width).max(r.X + r.Width) - x,
                            Height: (b.Y + b.Height).max(r.Y + r.Height) - y,
                        }
                    }
                });
            }
            if let Some(bounds) = bounds {
                out.push((text, bounds));
            }
        }
        Ok(out)
    }
}

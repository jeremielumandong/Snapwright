//! Linux engine: ocrs with its detection and recognition models.
//!
//! The detection model reads a fixed-size input (800x600 for the published model) and ocrs
//! squeezes larger images into it, which loses the small text of a screenshot. So the image
//! is cut into overlapping tiles of that size, words are found per tile, and the words whose
//! centre lies in a tile's own share are kept; lines are then grouped and read on the whole
//! image. Small images (a selection to copy text from) are read at 2x.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem as _};
use rten_imageproc::{Point, RotatedRect};

use crate::{Bounds, Config, Error, Image, Line, Order, Result, resized_rgba, spans};

/// A model file, where to get it and what it must hash to (BLAKE3).
struct ModelFile {
    name: &'static str,
    url: &'static str,
    blake3: &'static str,
}

const DETECTION: ModelFile = ModelFile {
    name: "text-detection.rten",
    url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten",
    blake3: "d13ee4d9e68ffca3dd2b2cdb5d5943a2901b96f8e2b59e35bf47143c71004311",
};

const RECOGNITION: ModelFile = ModelFile {
    name: "text-recognition.rten",
    url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten",
    blake3: "7cf8a167801a8322eebf4987f74c95dce6868e0fe459ef7340096490d8b43816",
};

/// Models are a few MB; anything much bigger is not what we asked for.
const MAX_MODEL_BYTES: u64 = 64 << 20;

/// Detection tile size, when the model does not say.
const TILE: (usize, usize) = (800, 600);
/// How far tiles overlap, so a word cut by one tile's edge is whole in the next.
const OVERLAP: usize = 96;
/// Images up to this many pixels (a selection rather than a screen) are read at 2x, which
/// helps with 1x UI text and still fits in a tile or two. Bigger ones are read as they are:
/// at 2x a full screen takes three times as long for about the same text.
const UPSCALE_MAX_PIXELS: u64 = 240_000;

pub struct Backend {
    engine: OcrEngine,
}

impl Backend {
    pub fn new(config: &Config) -> Result<Self> {
        let dir = std::env::var_os("SNAPWRIGHT_OCR_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|| config.models_dir.clone());
        let detection = load(&dir, &DETECTION)?;
        let recognition = load(&dir, &RECOGNITION)?;
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            ..Default::default()
        })
        .map_err(|e| Error::Model {
            name: DETECTION.name,
            message: e.to_string(),
        })?;
        Ok(Self { engine })
    }

    pub fn recognize(&mut self, image: &Image<'_>) -> Result<Vec<Line>> {
        let scale = if u64::from(image.width) * u64::from(image.height) <= UPSCALE_MAX_PIXELS {
            2.0
        } else {
            1.0
        };
        // ocrs reads RGB(A) and turns it grey with BT.601 weights, so BGRA must be swapped.
        let owned;
        let (rgba, width, height) = if scale != 1.0 || image.order == Order::Bgra {
            owned = resized_rgba(image, scale);
            (owned.0.as_slice(), owned.1, owned.2)
        } else {
            (image.pixels, image.width, image.height)
        };
        let platform = |e: &dyn std::fmt::Display| Error::Platform {
            api: "ocrs",
            message: e.to_string(),
        };
        let source = ImageSource::from_bytes(rgba, (width, height)).map_err(|e| platform(&e))?;
        let input = self
            .engine
            .prepare_input(source)
            .map_err(|e| platform(&e))?;
        let words = self.words(rgba, width as usize, height as usize)?;
        let rows = self.engine.find_text_lines(&input, &words);
        let lines = self
            .engine
            .recognize_text(&input, &rows)
            .map_err(|e| platform(&e))?;
        Ok(lines
            .into_iter()
            .flatten()
            .map(|line| {
                let r = line.bounding_rect();
                let unscale = |v: i32| (v.max(0) as f32 / scale).round() as u32;
                Line {
                    text: line.to_string(),
                    bounds: Bounds {
                        x: unscale(r.left()),
                        y: unscale(r.top()),
                        width: unscale(r.width()),
                        height: unscale(r.height()),
                    },
                }
            })
            .collect())
    }

    /// Word boxes over the whole image, found tile by tile.
    fn words(&self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<RotatedRect>> {
        let (tw, th) = TILE;
        let mut words = Vec::new();
        for xs in spans(width, tw, OVERLAP) {
            for ys in spans(height, th, OVERLAP) {
                let (x0, x1, y0, y1) = (xs.start, xs.end, ys.start, ys.end);
                let mut tile = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
                for y in y0..y1 {
                    let row = (y * width + x0) * 4;
                    tile.extend_from_slice(&rgba[row..row + (x1 - x0) * 4]);
                }
                let source = ImageSource::from_bytes(&tile, ((x1 - x0) as u32, (y1 - y0) as u32))
                    .map_err(|e| Error::Platform {
                    api: "ocrs",
                    message: e.to_string(),
                })?;
                let input = self
                    .engine
                    .prepare_input(source)
                    .map_err(|e| Error::Platform {
                        api: "ocrs detect",
                        message: e.to_string(),
                    })?;
                let found = self
                    .engine
                    .detect_words(&input)
                    .map_err(|e| Error::Platform {
                        api: "ocrs detect",
                        message: e.to_string(),
                    })?;
                for word in found {
                    let c = word.center();
                    let center = Point::from_yx(c.y + y0 as f32, c.x + x0 as f32);
                    let (cx, cy) = (center.x as usize, center.y as usize);
                    if xs.own.contains(&cx) && ys.own.contains(&cy) {
                        words.push(RotatedRect::new(
                            center,
                            word.up_axis(),
                            word.width(),
                            word.height(),
                        ));
                    }
                }
            }
        }
        Ok(words)
    }
}

/// Loads a model from `dir`, fetching it first if it is missing or not the pinned file.
fn load(dir: &Path, model: &ModelFile) -> Result<rten::Model> {
    let path = dir.join(model.name);
    let err = |message: String| Error::Model {
        name: model.name,
        message,
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) if hash(&bytes) == model.blake3 => bytes,
        _ => fetch(dir, &path, model)?,
    };
    rten::Model::load(bytes).map_err(|e| err(e.to_string()))
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Downloads a model, checks its hash and writes it into place atomically.
fn fetch(dir: &Path, path: &Path, model: &ModelFile) -> Result<Vec<u8>> {
    let err = |message: String| Error::Model {
        name: model.name,
        message,
    };
    tracing::info!(url = model.url, "fetching OCR model");
    let mut body = Vec::new();
    ureq::get(model.url)
        .call()
        .map_err(|e| err(format!("download: {e}")))?
        .into_body()
        .into_reader()
        .take(MAX_MODEL_BYTES)
        .read_to_end(&mut body)
        .map_err(|e| err(format!("download: {e}")))?;
    if hash(&body) != model.blake3 {
        return Err(err("downloaded file does not match the pinned hash".into()));
    }
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    // A partial name of its own, so two recognizers fetching at once (parallel tests, two
    // windows) never rename each other's file away.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let partial = dir.join(format!(
        "{}.{}-{}.part",
        model.name,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&partial, &body)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            err(format!("{}: {e}", path.display()))
        })?;
    Ok(body)
}

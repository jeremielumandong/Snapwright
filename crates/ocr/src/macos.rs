//! macOS engine: Vision's `VNRecognizeTextRequest` at the accurate level, with language
//! correction, over a `CGImage` that wraps the pixels as they are (BGRA or RGBA, no copy into
//! another layout). Big images are read in overlapping tiles (see [`TILE`]), each line kept
//! from the tile that owns its centre.

use objc2::AllocAnyThread as _;
use objc2::rc::autoreleasepool;
use objc2_core_foundation::CFData;
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo,
};
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use crate::{
    Bounds, Config, Error, Image, Line, Order, Result, reading_order, resized_rgba, spans,
};

/// Smallest text, in pixels, Vision is asked to read.
const MIN_TEXT_PX: f32 = 8.0;
/// Tile size and overlap. Vision reads a whole screen at a reduced resolution and garbles
/// small UI text there, but reads the same text cleanly in a crop of about this size.
const TILE: usize = 1024;
const OVERLAP: usize = 128;
/// Images up to this long side are read at 2x: Vision misreads some 1x UI text ("renort").
const UPSCALE_MAX_SIDE: u32 = 4096;

pub struct Backend;

impl Backend {
    pub fn new(_config: &Config) -> Result<Self> {
        Ok(Self)
    }

    pub fn recognize(&mut self, image: &Image<'_>) -> Result<Vec<Line>> {
        let scale = if image.width.max(image.height) <= UPSCALE_MAX_SIDE {
            2.0
        } else {
            1.0
        };
        let owned;
        let image = if scale != 1.0 {
            owned = resized_rgba(image, scale);
            &Image {
                pixels: &owned.0,
                width: owned.1,
                height: owned.2,
                order: Order::Rgba,
            }
        } else {
            image
        };
        let (width, height) = (image.width as usize, image.height as usize);
        let mut lines = Vec::new();
        for xs in spans(width, TILE, OVERLAP) {
            for ys in spans(height, TILE, OVERLAP) {
                let (tw, th) = (xs.end - xs.start, ys.end - ys.start);
                let mut pixels = Vec::with_capacity(tw * th * 4);
                for y in ys.start..ys.end {
                    let row = (y * width + xs.start) * 4;
                    pixels.extend_from_slice(&image.pixels[row..row + tw * 4]);
                }
                let tile = Image {
                    pixels: &pixels,
                    width: tw as u32,
                    height: th as u32,
                    order: image.order,
                };
                for mut line in autoreleasepool(|_| read(&tile))? {
                    let b = &mut line.bounds;
                    let cx = xs.start + (b.x + b.width / 2) as usize;
                    let cy = ys.start + (b.y + b.height / 2) as usize;
                    if xs.own.contains(&cx) && ys.own.contains(&cy) {
                        let unscale = |v: u32| (v as f32 / scale).round() as u32;
                        b.x = unscale(b.x + xs.start as u32);
                        b.y = unscale(b.y + ys.start as u32);
                        b.width = unscale(b.width);
                        b.height = unscale(b.height);
                        lines.push(line);
                    }
                }
            }
        }
        reading_order(&mut lines);
        Ok(lines)
    }
}

/// The lines in one image, with boxes in its pixels.
fn read(image: &Image<'_>) -> Result<Vec<Line>> {
    let platform = |api: &'static str, message: String| Error::Platform { api, message };
    let (w, h) = (image.width as usize, image.height as usize);
    let data = CFData::from_bytes(image.pixels);
    let provider = CGDataProvider::with_cf_data(Some(&data))
        .ok_or_else(|| platform("CGDataProviderCreateWithCFData", "failed".into()))?;
    let space = CGColorSpace::new_device_rgb()
        .ok_or_else(|| platform("CGColorSpaceCreateDeviceRGB", "failed".into()))?;
    let info = match image.order {
        Order::Bgra => CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::NoneSkipFirst.0,
        Order::Rgba => CGImageByteOrderInfo::OrderDefault.0 | CGImageAlphaInfo::NoneSkipLast.0,
    };
    // SAFETY: the provider holds `w * h * 4` bytes (checked by `Image::check`), which is what
    // 8-bit, 32-bit-per-pixel rows of `w * 4` bytes need; a null decode array is allowed.
    let cg_image = unsafe {
        CGImage::new(
            w,
            h,
            8,
            32,
            w * 4,
            Some(&space),
            CGBitmapInfo(info),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| platform("CGImageCreate", "failed".into()))?;

    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    // Vision ignores text shorter than 1/32 of the image by default (34 px on a 1080p screen),
    // which drops ordinary UI text in a full-screen capture. Ask for anything from 8 px up.
    request.setMinimumTextHeight((MIN_TEXT_PX / h as f32).min(1.0 / 32.0));
    // SAFETY: an empty options dictionary is valid for `initWithCGImage:options:`.
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &cg_image,
            &NSDictionary::new(),
        )
    };
    let as_request: &VNRequest = &request;
    handler
        .performRequests_error(&NSArray::from_slice(&[as_request]))
        .map_err(|e| platform("VNImageRequestHandler.performRequests", e.to_string()))?;

    let mut lines = Vec::new();
    let Some(results) = request.results() else {
        return Ok(lines);
    };
    let (wf, hf) = (w as f64, h as f64);
    for observation in results.iter() {
        let Some(best) = observation.topCandidates(1).firstObject() else {
            continue;
        };
        // SAFETY: a plain property read on a finished observation.
        let bb = unsafe { observation.boundingBox() };
        // Normalised, origin bottom-left; flip to top-left pixels.
        let x = (bb.origin.x * wf).max(0.0);
        let y = ((1.0 - bb.origin.y - bb.size.height) * hf).max(0.0);
        lines.push(Line {
            text: best.string().to_string(),
            bounds: Bounds {
                x: x.round() as u32,
                y: y.round() as u32,
                width: (bb.size.width * wf).round().max(0.0) as u32,
                height: (bb.size.height * hf).round().max(0.0) as u32,
            },
        });
    }
    Ok(lines)
}

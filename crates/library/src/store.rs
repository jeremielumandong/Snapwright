//! Files on disk: write-once content-addressed images and thumbnails.

use std::io::Write as _;
use std::path::Path;

use crate::{Error, Result};

/// Writes `bytes` to `path` unless it already exists. Writes go to a temporary file first
/// and are renamed into place, so a crash never leaves a truncated image under a real name.
pub fn write_once(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let io = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("image"),
        std::process::id()
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// Removes a file, logging rather than failing: a stray file costs disk, not correctness.
pub fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "could not remove library file");
    }
}

/// A PNG thumbnail no larger than `max` on its long side; smaller images keep their size.
pub fn thumbnail(rgba: &[u8], width: u32, height: u32, max: u32) -> Result<Vec<u8>> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
        Error::Image(format!(
            "{} bytes is not a {width}x{height} RGBA image",
            rgba.len()
        ))
    })?;
    let (tw, th) = fit(width, height, max);
    let small = if (tw, th) == (width, height) {
        image
    } else {
        image::imageops::thumbnail(&image, tw, th)
    };
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, tw, th);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| Error::Image(e.to_string()))?;
    writer
        .write_image_data(small.as_raw())
        .map_err(|e| Error::Image(e.to_string()))?;
    writer.finish().map_err(|e| Error::Image(e.to_string()))?;
    Ok(out)
}

/// Scales `(w, h)` down to fit in `max` x `max`, keeping the aspect ratio and at least 1 px.
pub fn fit(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max {
        return (w.max(1), h.max(1));
    }
    let scale = |v: u32| {
        ((u64::from(v) * u64::from(max) + u64::from(long) / 2) / u64::from(long)).max(1) as u32
    };
    (scale(w), scale(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_never_grows() {
        assert_eq!(fit(1920, 1080, 320), (320, 180));
        assert_eq!(fit(1080, 1920, 320), (180, 320));
        assert_eq!(fit(100, 50, 320), (100, 50));
        assert_eq!(fit(5000, 1, 320), (320, 1));
    }
}

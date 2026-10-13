//! The encoder behind each format and platform.

use std::path::Path;
use std::time::Duration;

use snapwright_capture::Frame;

use crate::{Format, Options, Result};

/// Turns timestamped frames into a file. Lives on the encoder thread for its whole life, so
/// implementations need not be `Send` (COM and AVFoundation objects stay on one thread).
pub(crate) trait Encoder {
    /// Adds a frame shown from `at` until the next one. Frames have the size passed to
    /// [`open`] and arrive in time order.
    fn push(&mut self, frame: Frame, at: Duration) -> Result<()>;

    /// Size of the encoded picture.
    fn size(&self) -> (u32, u32);

    /// Writes everything out; the last frame lasts until `end`.
    fn finish(self: Box<Self>, end: Duration) -> Result<()>;
}

/// Opens the encoder for `options.format` writing to `path`, replacing any file there.
pub(crate) fn open(
    options: Options,
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Box<dyn Encoder>> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|source| crate::RecordError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    match options.format {
        Format::Gif => Ok(Box::new(crate::gif::GifEncoder::create(
            path,
            width,
            height,
            options.gif_max_width,
        )?)),
        Format::Mp4 => mp4(path, width, height, fps),
    }
}

/// Average bit rate for H.264 screen video: about 0.1 bit per pixel per frame (6 Mbit/s for
/// 1080p at 30 fps), which keeps text crisp; never below 1 Mbit/s.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let bits = u64::from(width) * u64::from(height) * u64::from(fps) / 10;
    bits.clamp(1_000_000, 50_000_000) as u32
}

#[cfg(target_os = "macos")]
fn mp4(path: &Path, width: u32, height: u32, fps: u32) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(crate::macos::AvEncoder::create(
        path, width, height, fps,
    )?))
}

#[cfg(target_os = "windows")]
fn mp4(path: &Path, width: u32, height: u32, fps: u32) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(crate::windows::MfEncoder::create(
        path, width, height, fps,
    )?))
}

#[cfg(target_os = "linux")]
fn mp4(path: &Path, width: u32, height: u32, fps: u32) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(crate::ffmpeg::FfmpegEncoder::create(
        path, width, height, fps,
    )?))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn mp4(_: &Path, _: u32, _: u32, _: u32) -> Result<Box<dyn Encoder>> {
    Err(crate::RecordError::Unsupported(
        "MP4 recording is not available on this platform".into(),
    ))
}

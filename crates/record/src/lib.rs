//! `snapwright-record` — screen recording.
//!
//! A [`Recorder`] pulls frames from a [`FrameSource`] (normally [`DisplaySource`]: a display,
//! or a region of one, through the app's warm [`Capturer`]) at a steady rate and hands them to
//! an encoder on its own thread, so a slow encode drops a frame instead of stretching time.
//!
//! * MP4 (H.264): AVFoundation on macOS and Media Foundation on Windows, which use the GPU's
//!   encoder when there is one; `ffmpeg` (libx264) on Linux.
//! * GIF: a pure Rust encoder on every platform. Only the part of the screen that changed is
//!   written per frame, so a mostly still screen gives a small file.
//!
//! Timestamps are wall-clock time since the start, minus time spent paused.

mod encoder;
#[cfg(target_os = "linux")]
mod ffmpeg;
mod gif;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;
#[cfg(any(target_os = "windows", test))]
mod yuv;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use snapwright_capture::{CaptureError, Capturer, Display, Frame, Rect};

/// Errors from recording.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The OS encoder (or ffmpeg) failed.
    #[error("{api} failed: {message}")]
    Encoder { api: &'static str, message: String },
    /// No encoder for this format here, such as MP4 on Linux without ffmpeg.
    #[error("{0}")]
    Unsupported(String),
    /// The region does not overlap the display, or is too small to encode.
    #[error("the region {0:?} has nothing to record")]
    EmptyRegion(Rect),
    #[error("the recording thread stopped unexpectedly")]
    Panicked,
}

/// Shorthand for recording results.
pub type Result<T, E = RecordError> = std::result::Result<T, E>;

/// The file a recording becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// H.264 in an MP4 container, laid out for streaming (the index comes first).
    #[default]
    Mp4,
    /// An animated GIF that loops.
    Gif,
}

impl Format {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
        }
    }

    /// The format a path's extension names, if any.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" => Some(Self::Mp4),
            "gif" => Some(Self::Gif),
            _ => None,
        }
    }

    /// A sensible frame rate: smooth video, and a GIF rate that keeps files small.
    pub fn default_fps(self) -> u32 {
        match self {
            Self::Mp4 => 30,
            Self::Gif => 15,
        }
    }
}

/// How to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub format: Format,
    /// Frames per second to capture (clamped to 1..=60).
    pub fps: u32,
    /// GIFs wider than this are scaled down by a whole factor (0 keeps the full size).
    pub gif_max_width: u32,
}

impl Options {
    /// Defaults for a format.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            fps: format.default_fps(),
            gif_max_width: 1280,
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new(Format::default())
    }
}

/// Where frames come from.
pub trait FrameSource: Send + 'static {
    /// The next frame. Every frame should have the size of the first; smaller ones are
    /// skipped and larger ones cropped.
    fn next_frame(&mut self) -> Result<Frame>;
}

/// A display, or a region of it, with the pointer drawn in.
pub struct DisplaySource {
    capturer: Arc<Mutex<Capturer>>,
    display: Display,
    region: Option<Rect>,
}

impl DisplaySource {
    /// `region` is in the display's pixels (the frame's coordinates); `None` records all of
    /// it. The capturer is shared with screenshots and only locked for each grab.
    pub fn new(capturer: Arc<Mutex<Capturer>>, display: Display, region: Option<Rect>) -> Self {
        Self {
            capturer,
            display,
            region,
        }
    }
}

impl FrameSource for DisplaySource {
    fn next_frame(&mut self) -> Result<Frame> {
        let frame = {
            let mut capturer = self
                .capturer
                .lock()
                .map_err(|_| CaptureError::Unsupported("capture backend poisoned".into()))?;
            let was = capturer.cursor();
            capturer.set_cursor(true);
            let frame = capturer.capture_display(&self.display);
            capturer.set_cursor(was);
            frame?
        };
        match self.region {
            Some(r) if r != Rect::new(0, 0, frame.width, frame.height) => {
                frame.crop(r).ok_or(RecordError::EmptyRegion(r))
            }
            _ => Ok(frame),
        }
    }
}

/// What a finished recording produced.
#[derive(Debug)]
pub struct Summary {
    pub path: PathBuf,
    pub format: Format,
    /// Size of the video, which can be a pixel smaller than the region (H.264 wants even
    /// sizes) or scaled down (GIF).
    pub width: u32,
    pub height: u32,
    /// Length of the recording, pauses excluded.
    pub duration: Duration,
    /// Frames handed to the encoder.
    pub frames: u64,
    /// Frames skipped because the encoder was behind.
    pub dropped: u64,
    /// The first frame, at the source's size, for a poster image and thumbnail.
    pub poster: Frame,
    /// Why the recording ended early, if the source failed partway (the file still holds
    /// everything before that).
    pub interrupted: Option<String>,
}

/// State the controlling thread and the capture thread share.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    /// Recorded time so far, in microseconds, pauses excluded.
    elapsed_us: AtomicU64,
}

/// A recording in progress. Dropping it without [`Recorder::stop`] still finishes the file.
pub struct Recorder {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Result<Summary>>>,
    path: PathBuf,
}

impl Recorder {
    /// Starts recording into `path` (replaced if it exists). Returns once the first frame is
    /// captured and the encoder is open, so a failure there is reported here; blocks for
    /// that long, so call it off the UI thread.
    pub fn start(
        source: impl FrameSource,
        options: Options,
        path: impl Into<PathBuf>,
    ) -> Result<Self> {
        let path = path.into();
        let shared = Arc::new(Shared::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let shared = shared.clone();
            let out = path.clone();
            std::thread::Builder::new()
                .name("snapwright-record".into())
                .spawn(move || record(source, options, out, shared, ready_tx))
                .map_err(|source| RecordError::Io {
                    path: path.clone(),
                    source,
                })?
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                thread: Some(thread),
                path,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(thread.join().map_or(RecordError::Panicked, |r| match r {
                Err(e) => e,
                Ok(_) => RecordError::Panicked,
            })),
        }
    }

    /// The file being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recorded time so far, pauses excluded.
    pub fn elapsed(&self) -> Duration {
        Duration::from_micros(self.shared.elapsed_us.load(Ordering::Relaxed))
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// Stops, finishes the file and reports what was recorded. Blocks until the encoder has
    /// written everything; call it off the UI thread.
    pub fn stop(mut self) -> Result<Summary> {
        self.finish()
    }

    fn finish(&mut self) -> Result<Summary> {
        self.shared.stop.store(true, Ordering::Relaxed);
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| RecordError::Panicked)?,
            None => Err(RecordError::Panicked),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.thread.is_some()
            && let Err(e) = self.finish()
        {
            tracing::warn!(error = %e, "recording dropped without stop");
        }
    }
}

/// H.264 in 4:2:0 needs even sizes; a GIF takes any size.
fn encoded_size(format: Format, width: u32, height: u32) -> (u32, u32) {
    match format {
        Format::Mp4 => (width & !1, height & !1),
        Format::Gif => (width, height),
    }
}

/// The capture thread: paces grabs, timestamps them, and feeds the encoder thread.
fn record(
    mut source: impl FrameSource,
    options: Options,
    path: PathBuf,
    shared: Arc<Shared>,
    ready: SyncSender<Result<()>>,
) -> Result<Summary> {
    let fps = options.fps.clamp(1, 60);
    let first = match source.next_frame() {
        Ok(f) => f,
        Err(e) => {
            // `start` reports the error it receives here; this return value is not read.
            let _ = ready.send(Err(e));
            return Err(RecordError::Panicked);
        }
    };
    let (width, height) = encoded_size(options.format, first.width, first.height);
    if width < 2 || height < 2 {
        let r = Rect::new(0, 0, first.width, first.height);
        let _ = ready.send(Err(RecordError::EmptyRegion(r)));
        return Err(RecordError::Panicked);
    }
    let start = Instant::now();
    let (frames_tx, frames_rx) = mpsc::sync_channel::<(Frame, Duration)>(3);
    let (open_tx, open_rx) = mpsc::sync_channel(1);
    let encoder = {
        let out = path.clone();
        std::thread::Builder::new()
            .name("snapwright-encode".into())
            .spawn(move || -> Result<(u32, u32)> {
                let mut enc = match encoder::open(options, &out, width, height, fps) {
                    Ok(enc) => {
                        let _ = open_tx.send(Ok(()));
                        enc
                    }
                    Err(e) => {
                        // Reported through `open_tx`; this return value is not read.
                        let _ = open_tx.send(Err(e));
                        return Err(RecordError::Panicked);
                    }
                };
                let mut end = Duration::ZERO;
                let mut failed = None;
                for (frame, at) in frames_rx {
                    end = at;
                    if failed.is_none()
                        && let Err(e) = enc.push(frame, at)
                    {
                        // Keep draining so the capture thread never blocks on a dead encoder.
                        failed = Some(e);
                    }
                }
                let size = enc.size();
                let finished = enc.finish(end + Duration::from_secs(1) / fps);
                match failed {
                    Some(e) => Err(e),
                    None => finished.map(|()| size),
                }
            })
            .map_err(|source| RecordError::Io {
                path: path.clone(),
                source,
            })?
    };
    match open_rx.recv() {
        Ok(Ok(())) => {
            let _ = ready.send(Ok(()));
        }
        Ok(Err(e)) => {
            drop(frames_tx);
            let _ = encoder.join();
            let _ = ready.send(Err(e));
            return Err(RecordError::Panicked);
        }
        Err(_) => {
            let _ = ready.send(Err(RecordError::Panicked));
            return Err(RecordError::Panicked);
        }
    }

    let fit = |frame: Frame| -> Option<Frame> {
        if frame.width == width && frame.height == height {
            Some(frame)
        } else if frame.width >= width && frame.height >= height {
            frame.crop(Rect::new(0, 0, width, height))
        } else {
            None
        }
    };
    let poster = first.clone();
    let interval = Duration::from_secs(1) / fps;
    let mut frames = 0u64;
    let mut dropped = 0u64;
    let mut interrupted = None;
    let mut paused_total = Duration::ZERO;
    let mut paused_since: Option<Instant> = None;
    let mut pending = fit(first);
    let mut next_tick = start;
    let mut at = Duration::ZERO;
    loop {
        if let Some(frame) = pending.take() {
            match frames_tx.try_send((frame, at)) {
                Ok(()) => frames += 1,
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        // Sleep to the next tick; if a grab overran, start the next one now instead of
        // trying to catch up.
        next_tick += interval;
        let now = Instant::now();
        if next_tick > now {
            std::thread::sleep(next_tick - now);
        } else {
            next_tick = now;
        }
        let now = Instant::now();
        if shared.paused.load(Ordering::Relaxed) {
            paused_since.get_or_insert(now);
            continue;
        }
        if let Some(since) = paused_since.take() {
            paused_total += now - since;
        }
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        at = now - start - paused_total;
        shared
            .elapsed_us
            .store(at.as_micros() as u64, Ordering::Relaxed);
        match source.next_frame() {
            Ok(frame) => pending = fit(frame),
            Err(e) => {
                tracing::warn!(error = %e, "recording source failed; finishing the file");
                interrupted = Some(e.to_string());
                break;
            }
        }
    }
    let duration = at + interval;
    drop(frames_tx);
    let (width, height) = encoder.join().map_err(|_| RecordError::Panicked)??;
    tracing::info!(
        path = %path.display(),
        frames,
        dropped,
        seconds = duration.as_secs_f64(),
        "recording finished"
    );
    Ok(Summary {
        path,
        format: options.format,
        width,
        height,
        duration,
        frames,
        dropped,
        poster,
        interrupted,
    })
}

#[cfg(test)]
mod tests;

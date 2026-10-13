//! MP4 on Linux through an `ffmpeg` process (libx264).
//!
//! Linux has no system H.264 encoder to call, and bundling one brings patent licensing with
//! it, so recordings use the `ffmpeg` the user has installed (`SNAPWRIGHT_FFMPEG` names
//! another binary). Raw BGRA frames go down a pipe at a constant rate; a frame that arrives
//! late is repeated to fill the gap, so the video keeps wall-clock time.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use snapwright_capture::Frame;

use crate::encoder::Encoder;
use crate::{RecordError, Result};

pub(crate) struct FfmpegEncoder {
    child: Child,
    stdin: Option<ChildStdin>,
    path: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    /// Frames written so far.
    written: u64,
    last: Option<Frame>,
}

/// The ffmpeg binary to run.
fn binary() -> PathBuf {
    std::env::var_os("SNAPWRIGHT_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

impl FfmpegEncoder {
    pub(crate) fn create(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        let mut child = Command::new(binary())
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "bgra"])
            .args(["-video_size", &format!("{width}x{height}")])
            .args(["-framerate", &fps.to_string(), "-i", "pipe:0"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"])
            // BT.709 in limited range, tagged, so players show the colours the screen had.
            .args([
                "-vf",
                "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
            ])
            .args(["-colorspace", "bt709", "-color_primaries", "bt709"])
            .args(["-color_trc", "bt709", "-color_range", "tv"])
            .args(["-movflags", "+faststart"])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => RecordError::Unsupported(
                    "MP4 recording on Linux needs ffmpeg; install it, or record a GIF".into(),
                ),
                _ => RecordError::Encoder {
                    api: "ffmpeg",
                    message: e.to_string(),
                },
            })?;
        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            path: path.to_path_buf(),
            width,
            height,
            fps,
            written: 0,
            last: None,
        })
    }

    /// Writes `frame` until `count` frames are in the pipe.
    fn fill(&mut self, frame: &Frame, count: u64) -> Result<()> {
        while self.written < count {
            let Some(stdin) = self.stdin.as_mut() else {
                break;
            };
            if let Err(e) = stdin.write_all(&frame.data) {
                // ffmpeg exited; its own message says why.
                self.stdin = None;
                return Err(self.failure(e.to_string()));
            }
            self.written += 1;
        }
        Ok(())
    }

    /// Reaps ffmpeg and builds an error from what it printed.
    fn failure(&mut self, fallback: String) -> RecordError {
        let _ = self.child.wait();
        let mut stderr = String::new();
        if let Some(mut err) = self.child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        let message = stderr.trim();
        RecordError::Encoder {
            api: "ffmpeg",
            message: if message.is_empty() {
                fallback
            } else {
                message.to_owned()
            },
        }
    }

    fn slot(&self, at: Duration) -> u64 {
        (at.as_secs_f64() * f64::from(self.fps)).round() as u64
    }
}

impl Encoder for FfmpegEncoder {
    fn push(&mut self, frame: Frame, at: Duration) -> Result<()> {
        // Repeat the previous frame up to this one's slot, then write this one.
        let slot = self.slot(at);
        if let Some(last) = self.last.take() {
            self.fill(&last, slot)?;
        }
        if self.written <= slot {
            self.fill(&frame, slot + 1)?;
        }
        self.last = Some(frame);
        Ok(())
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn finish(mut self: Box<Self>, end: Duration) -> Result<()> {
        let end = self.slot(end);
        if let Some(last) = self.last.take() {
            self.fill(&last, end)?;
        }
        // Closing stdin tells ffmpeg the input ended; it then writes the index and exits.
        drop(self.stdin.take());
        let status = self.child.wait().map_err(|e| RecordError::Encoder {
            api: "ffmpeg",
            message: e.to_string(),
        })?;
        if status.success() {
            Ok(())
        } else {
            let path = self.path.display().to_string();
            Err(self.failure(format!("{path}: ffmpeg exited with {status}")))
        }
    }
}

impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        // Only reached with ffmpeg still running when `finish` was skipped (an error).
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

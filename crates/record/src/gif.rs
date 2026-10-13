//! Animated GIF, in pure Rust.
//!
//! Screen recordings mostly stand still, so each frame writes only the rectangle that changed
//! since the last one, and pixels inside it that did not change are transparent: the
//! quantiser spends its 256 colours on what moved and the LZW stream compresses the rest to
//! almost nothing. A frame with no change is not written at all; the previous one is shown
//! longer instead.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Duration;

use snapwright_capture::{Frame, Rect};

use crate::encoder::Encoder;
use crate::{RecordError, Result};

/// NeuQuant speed (1 best .. 30 fastest); 10 is the crate's balance of quality and time.
const QUANT_SPEED: i32 = 10;
/// Browsers show delays under 2 centiseconds as 10, so never write less than 2.
const MIN_DELAY_CS: u16 = 2;

pub(crate) struct GifEncoder {
    out: Option<gif::Encoder<BufWriter<File>>>,
    path: PathBuf,
    /// Source pixels per GIF pixel along each axis.
    factor: u32,
    width: u32,
    height: u32,
    /// The picture as a viewer shows it after the frames written so far (RGBA).
    shown: Vec<u8>,
    /// The last frame, waiting for the next one to know how long it lasts.
    pending: Option<(gif::Frame<'static>, Duration)>,
}

fn gif_err(path: &Path, e: impl std::fmt::Display) -> RecordError {
    RecordError::Encoder {
        api: "GIF",
        message: format!("{}: {e}", path.display()),
    }
}

impl GifEncoder {
    /// `width` and `height` are the source frames'; frames wider than `max_width` are scaled
    /// down by the smallest whole factor that fits.
    pub(crate) fn create(path: &Path, width: u32, height: u32, max_width: u32) -> Result<Self> {
        let factor = if max_width == 0 {
            1
        } else {
            width.div_ceil(max_width).max(1)
        };
        let (w, h) = (width / factor, height / factor);
        if w == 0 || h == 0 || w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
            return Err(RecordError::EmptyRegion(Rect::new(0, 0, width, height)));
        }
        let file = File::create(path).map_err(|source| RecordError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut out = gif::Encoder::new(BufWriter::new(file), w as u16, h as u16, &[])
            .map_err(|e| gif_err(path, e))?;
        out.set_repeat(gif::Repeat::Infinite)
            .map_err(|e| gif_err(path, e))?;
        Ok(Self {
            out: Some(out),
            path: path.to_path_buf(),
            factor,
            width: w,
            height: h,
            shown: Vec::new(),
            pending: None,
        })
    }

    fn write_pending(&mut self, until: Duration) -> Result<()> {
        let Some((mut frame, from)) = self.pending.take() else {
            return Ok(());
        };
        // Rounded per timestamp, not per gap, so rounding never accumulates.
        let delay = centis(until).saturating_sub(centis(from));
        frame.delay = delay.clamp(u64::from(MIN_DELAY_CS), u64::from(u16::MAX)) as u16;
        let out = self
            .out
            .as_mut()
            .ok_or_else(|| gif_err(&self.path, "closed"))?;
        out.write_frame(&frame).map_err(|e| gif_err(&self.path, e))
    }
}

fn centis(d: Duration) -> u64 {
    (d.as_millis() as u64 + 5) / 10
}

impl Encoder for GifEncoder {
    fn push(&mut self, frame: Frame, at: Duration) -> Result<()> {
        let rgba = downscale(&frame, self.factor, self.width, self.height);
        let first = self.shown.is_empty();
        let rect = if first {
            Rect::new(0, 0, self.width, self.height)
        } else {
            match changed(&self.shown, &rgba, self.width, self.height) {
                Some(r) => r,
                None => return Ok(()),
            }
        };
        let stride = self.width as usize * 4;
        let mut sub = Vec::with_capacity(rect.width as usize * rect.height as usize * 4);
        for y in rect.y as usize..rect.y as usize + rect.height as usize {
            let row = y * stride + rect.x as usize * 4
                ..y * stride + (rect.x as usize + rect.width as usize) * 4;
            let new = &rgba[row.clone()];
            if first {
                sub.extend_from_slice(new);
            } else {
                let old = &self.shown[row];
                for (n, o) in new.as_chunks::<4>().0.iter().zip(old.as_chunks::<4>().0) {
                    // Unchanged pixels show through from the frame below.
                    sub.extend_from_slice(if n == o { &[0, 0, 0, 0] } else { n });
                }
            }
        }
        if first {
            self.shown = rgba;
        } else {
            for y in rect.y as usize..rect.y as usize + rect.height as usize {
                let row = y * stride + rect.x as usize * 4
                    ..y * stride + (rect.x as usize + rect.width as usize) * 4;
                self.shown[row.clone()].copy_from_slice(&rgba[row]);
            }
        }
        let mut gif_frame = gif::Frame::from_rgba_speed(
            rect.width as u16,
            rect.height as u16,
            &mut sub,
            QUANT_SPEED,
        );
        gif_frame.left = rect.x as u16;
        gif_frame.top = rect.y as u16;
        gif_frame.dispose = gif::DisposalMethod::Keep;
        self.write_pending(at)?;
        self.pending = Some((gif_frame, at));
        Ok(())
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn finish(mut self: Box<Self>, end: Duration) -> Result<()> {
        self.write_pending(end)?;
        let out = self
            .out
            .take()
            .ok_or_else(|| gif_err(&self.path, "closed"))?;
        let mut file = out.into_inner().map_err(|e| gif_err(&self.path, e))?;
        std::io::Write::flush(&mut file).map_err(|source| RecordError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

/// The frame as RGBA at `1 / factor` of its size, each pixel the average of a
/// `factor` x `factor` block.
fn downscale(frame: &Frame, factor: u32, width: u32, height: u32) -> Vec<u8> {
    let src_stride = frame.width as usize * 4;
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    if factor == 1 {
        for y in 0..height as usize {
            let row = &frame.data[y * src_stride..y * src_stride + width as usize * 4];
            for &[b, g, r, _] in row.as_chunks::<4>().0 {
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
        return out;
    }
    let f = factor as usize;
    let n = (f * f) as u32;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let mut sum = [0u32; 3];
            for dy in 0..f {
                let start = (y * f + dy) * src_stride + x * f * 4;
                for px in frame.data[start..start + f * 4].as_chunks::<4>().0 {
                    sum[0] += u32::from(px[2]);
                    sum[1] += u32::from(px[1]);
                    sum[2] += u32::from(px[0]);
                }
            }
            out.extend_from_slice(&[
                ((sum[0] + n / 2) / n) as u8,
                ((sum[1] + n / 2) / n) as u8,
                ((sum[2] + n / 2) / n) as u8,
                255,
            ]);
        }
    }
    out
}

/// The smallest rectangle holding every pixel that differs between two pictures.
fn changed(old: &[u8], new: &[u8], width: u32, height: u32) -> Option<Rect> {
    let stride = width as usize * 4;
    let row_differs =
        |y: usize| old[y * stride..(y + 1) * stride] != new[y * stride..(y + 1) * stride];
    let top = (0..height as usize).find(|&y| row_differs(y))?;
    let bottom = (top..height as usize).rev().find(|&y| row_differs(y))?;
    let (mut left, mut right) = (width as usize, 0);
    for y in top..=bottom {
        let o = old[y * stride..(y + 1) * stride].as_chunks::<4>().0;
        let n = new[y * stride..(y + 1) * stride].as_chunks::<4>().0;
        if let Some(l) = (0..left).find(|&x| o[x] != n[x]) {
            left = l;
        }
        if let Some(r) = (right..width as usize).rev().find(|&x| o[x] != n[x]) {
            right = right.max(r);
        }
    }
    (left <= right).then(|| {
        Rect::new(
            left as i32,
            top as i32,
            (right - left + 1) as u32,
            (bottom - top + 1) as u32,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, bgra: [u8; 4]) -> Frame {
        Frame {
            display_id: 0,
            width,
            height,
            data: bgra.repeat((width * height) as usize),
        }
    }

    #[test]
    fn downscale_averages_blocks_and_swaps_channels() {
        let mut f = solid(4, 2, [0, 0, 0, 255]);
        // Top-left 2x2 block: two pixels of blue 200 → average 100.
        f.data[0] = 200;
        f.data[4] = 200;
        f.data[8 + 2] = 90; // red in the second block
        let out = downscale(&f, 2, 2, 1);
        assert_eq!(out, vec![0, 0, 100, 255, 23, 0, 0, 255]);
        assert_eq!(downscale(&f, 1, 4, 2)[..4], [0, 0, 200, 255]);
    }

    #[test]
    fn changed_finds_the_bounding_box() {
        let a = vec![0u8; 5 * 4 * 4];
        let mut b = a.clone();
        assert_eq!(changed(&a, &b, 5, 4), None);
        b[(5 + 3) * 4] = 9;
        b[(2 * 5 + 1) * 4 + 2] = 9;
        assert_eq!(changed(&a, &b, 5, 4), Some(Rect::new(1, 1, 3, 2)));
    }

    #[test]
    fn delays_round_per_timestamp() {
        assert_eq!(centis(Duration::from_millis(66)), 7);
        assert_eq!(centis(Duration::from_millis(133)), 13);
        assert_eq!(centis(Duration::from_millis(4)), 0);
    }
}

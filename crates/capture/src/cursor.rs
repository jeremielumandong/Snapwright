//! The mouse pointer, drawn into frames for screen recordings.
//!
//! Screenshots leave the pointer out. Recordings want it, and only macOS and Wayland can ask
//! the compositor to include it; X11 and Windows hand over the pointer image separately, so
//! those backends composite it with [`Frame::draw_cursor`].

use crate::Frame;

/// How a pointer image's pixels combine with the screen under them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Each backend builds only the kinds its OS hands over.
#[allow(dead_code)]
pub(crate) enum CursorKind {
    /// 32-bit BGRA with premultiplied alpha (X11 XFixes).
    Premultiplied,
    /// 32-bit BGRA with straight alpha (DXGI colour pointers).
    Straight,
    /// 1-bit AND mask rows followed by as many 1-bit XOR mask rows (DXGI monochrome
    /// pointers); `height` counts both halves.
    Monochrome,
    /// 32-bit BGR whose alpha byte is a mask: 0 replaces the screen pixel, 0xFF XORs it
    /// (DXGI masked-colour pointers).
    MaskedColor,
}

/// A pointer image and where its top-left corner falls in a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
pub(crate) struct Cursor {
    pub kind: CursorKind,
    /// Top-left corner in frame pixels (the hotspot already subtracted).
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Bytes per row of `data`.
    pub pitch: usize,
    pub data: Vec<u8>,
}

#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
impl Frame {
    /// Draws the pointer into the frame, clipped to its edges.
    pub(crate) fn draw_cursor(&mut self, c: &Cursor) {
        let rows = match c.kind {
            CursorKind::Monochrome => c.height / 2,
            _ => c.height,
        };
        for cy in 0..rows {
            let fy = c.y + cy as i32;
            if fy < 0 || fy >= self.height as i32 {
                continue;
            }
            for cx in 0..c.width {
                let fx = c.x + cx as i32;
                if fx < 0 || fx >= self.width as i32 {
                    continue;
                }
                let at = (fy as usize * self.width as usize + fx as usize) * 4;
                let Some(dst) = self.data.get_mut(at..at + 3) else {
                    continue;
                };
                match c.kind {
                    CursorKind::Monochrome => {
                        let byte = cx as usize / 8;
                        let bit = 0x80u8 >> (cx % 8);
                        let and_row = cy as usize * c.pitch;
                        let xor_row = (cy + rows) as usize * c.pitch;
                        let (Some(&and), Some(&xor)) =
                            (c.data.get(and_row + byte), c.data.get(xor_row + byte))
                        else {
                            continue;
                        };
                        let (and, xor) = (and & bit != 0, xor & bit != 0);
                        for d in dst.iter_mut() {
                            *d = match (and, xor) {
                                (false, false) => 0,
                                (false, true) => 255,
                                (true, false) => *d,
                                (true, true) => !*d,
                            };
                        }
                    }
                    kind => {
                        let si = cy as usize * c.pitch + cx as usize * 4;
                        let Some(src) = c.data.get(si..si + 4) else {
                            continue;
                        };
                        let a = u32::from(src[3]);
                        for (d, &s) in dst.iter_mut().zip(&src[..3]) {
                            let (dv, sv) = (u32::from(*d), u32::from(s));
                            *d = match kind {
                                CursorKind::Premultiplied => {
                                    (sv + (dv * (255 - a) + 127) / 255).min(255) as u8
                                }
                                CursorKind::Straight => {
                                    ((sv * a + dv * (255 - a) + 127) / 255) as u8
                                }
                                _ if a == 0 => s,
                                _ => *d ^ s,
                            };
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(width: u32, height: u32, v: u8) -> Frame {
        Frame {
            display_id: 0,
            width,
            height,
            data: [v, v, v, 255].repeat((width * height) as usize),
        }
    }

    #[test]
    fn premultiplied_blends_and_clips() {
        let mut f = grey(4, 4, 100);
        // 2x2: opaque red, transparent, half-covered white (premultiplied 128), opaque blue.
        let data = vec![
            0, 0, 255, 255, 0, 0, 0, 0, //
            128, 128, 128, 128, 255, 0, 0, 255,
        ];
        let c = Cursor {
            kind: CursorKind::Premultiplied,
            x: 3,
            y: 2,
            width: 2,
            height: 2,
            pitch: 8,
            data,
        };
        f.draw_cursor(&c);
        assert_eq!(f.pixel(3, 2), Some([0, 0, 255, 255]));
        assert_eq!(f.pixel(3, 3), Some([178, 178, 178, 255]));
        assert_eq!(
            f.pixel(2, 2),
            Some([100, 100, 100, 255]),
            "left of the cursor"
        );
        // Off the frame on the right and below: nothing panics, nothing else changes.
        let c = Cursor { x: -1, y: -1, ..c };
        f.draw_cursor(&c);
        assert_eq!(f.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn straight_alpha_blends() {
        let mut f = grey(1, 1, 0);
        let c = Cursor {
            kind: CursorKind::Straight,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            pitch: 4,
            data: vec![255, 255, 255, 51],
        };
        f.draw_cursor(&c);
        assert_eq!(f.pixel(0, 0), Some([51, 51, 51, 255]));
    }

    #[test]
    fn monochrome_masks() {
        let mut f = grey(4, 1, 100);
        // AND row 0b0011, XOR row 0b0101 over four pixels: black, white, screen, inverted.
        let c = Cursor {
            kind: CursorKind::Monochrome,
            x: 0,
            y: 0,
            width: 4,
            height: 2,
            pitch: 1,
            data: vec![0b0011_0000, 0b0101_0000],
        };
        f.draw_cursor(&c);
        assert_eq!(f.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(f.pixel(1, 0), Some([255, 255, 255, 255]));
        assert_eq!(f.pixel(2, 0), Some([100, 100, 100, 255]));
        assert_eq!(f.pixel(3, 0), Some([155, 155, 155, 255]));
    }

    #[test]
    fn masked_color_replaces_or_xors() {
        let mut f = grey(2, 1, 0x0f);
        let c = Cursor {
            kind: CursorKind::MaskedColor,
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            pitch: 8,
            data: vec![1, 2, 3, 0, 0xff, 0xff, 0xff, 0xff],
        };
        f.draw_cursor(&c);
        assert_eq!(f.pixel(0, 0), Some([1, 2, 3, 255]));
        assert_eq!(f.pixel(1, 0), Some([0xf0, 0xf0, 0xf0, 255]));
    }
}

//! Captured pixels.

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl Rect {
    /// Creates a rectangle.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The overlap of two rectangles, or `None` when they do not overlap.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x as i64 + self.width as i64).min(other.x as i64 + other.width as i64);
        let y1 = (self.y as i64 + self.height as i64).min(other.y as i64 + other.height as i64);
        (x1 > x0 as i64 && y1 > y0 as i64)
            .then(|| Rect::new(x0, y0, (x1 - x0 as i64) as u32, (y1 - y0 as i64) as u32))
    }
}

/// One captured image: tightly packed BGRA rows, alpha opaque.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    /// Display the frame came from.
    pub display_id: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, B G R A per pixel, rows top to bottom.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("display_id", &self.display_id)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Frame {
    /// Builds a frame from rows that may carry padding (`stride` bytes per row), forcing alpha
    /// to opaque. Backends hand the OS buffer straight to this, so it is the one copy a
    /// capture makes on the CPU.
    pub fn from_bgrx_rows(
        display_id: u64,
        width: u32,
        height: u32,
        stride: usize,
        src: &[u8],
    ) -> Self {
        let row = width as usize * 4;
        let mut data = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = y * stride;
            data.extend_from_slice(&src[start..start + row]);
        }
        set_opaque(&mut data);
        Self {
            display_id,
            width,
            height,
            data,
        }
    }

    /// The pixel at `(x, y)` as `[b, g, r, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ])
    }

    /// A copy of the part of the frame inside `rect` (frame coordinates), clipped to the frame.
    pub fn crop(&self, rect: Rect) -> Option<Frame> {
        let r = rect.intersect(&Rect::new(0, 0, self.width, self.height))?;
        let stride = self.width as usize * 4;
        let start = r.y as usize * stride + r.x as usize * 4;
        let src = self.data.get(start..)?;
        Some(Frame::from_bgrx_rows(
            self.display_id,
            r.width,
            r.height,
            stride,
            src,
        ))
    }

    /// The pixels as RGBA, for encoders that want that order.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = self.data.clone();
        bgra_to_rgba_in_place(&mut out);
        out
    }
}

/// Sets every pixel's alpha byte to 255. Capture APIs leave it undefined (X11) or zero.
fn set_opaque(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
}

/// Swaps the B and R channels of each pixel. A plain loop the compiler vectorises; a
/// hand-written SIMD kernel replaces it only if `snapwright bench` shows it matters.
pub(crate) fn bgra_to_rgba_in_place(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> Frame {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[x as u8, y as u8, 7, 0]);
            }
        }
        Frame::from_bgrx_rows(1, width, height, width as usize * 4, &data)
    }

    #[test]
    fn from_rows_drops_padding_and_sets_alpha() {
        // 2x2 pixels, 12-byte stride (4 bytes of padding per row).
        let src = [
            1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, //
            7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9,
        ];
        let f = Frame::from_bgrx_rows(3, 2, 2, 12, &src);
        assert_eq!(f.data.len(), 16);
        assert_eq!(f.pixel(0, 0), Some([1, 2, 3, 255]));
        assert_eq!(f.pixel(1, 1), Some([10, 11, 12, 255]));
        assert_eq!(f.pixel(2, 0), None);
    }

    #[test]
    fn crop_takes_the_right_pixels() {
        let f = gradient(10, 8);
        let c = f.crop(Rect::new(3, 2, 4, 5)).expect("overlaps");
        assert_eq!((c.width, c.height), (4, 5));
        assert_eq!(c.pixel(0, 0), Some([3, 2, 7, 255]));
        assert_eq!(c.pixel(3, 4), Some([6, 6, 7, 255]));
    }

    #[test]
    fn crop_clips_to_the_frame() {
        let f = gradient(10, 8);
        let c = f.crop(Rect::new(-5, 6, 8, 10)).expect("overlaps");
        assert_eq!((c.width, c.height), (3, 2));
        assert_eq!(c.pixel(0, 0), Some([0, 6, 7, 255]));
        assert!(f.crop(Rect::new(20, 20, 4, 4)).is_none());
    }

    #[test]
    fn rgba_swaps_red_and_blue() {
        let f = Frame::from_bgrx_rows(1, 1, 1, 4, &[10, 20, 30, 0]);
        assert_eq!(f.to_rgba(), vec![30, 20, 10, 255]);
    }

    #[test]
    fn intersect_handles_edges() {
        let a = Rect::new(0, 0, 10, 10);
        assert_eq!(a.intersect(&Rect::new(10, 0, 5, 5)), None);
        assert_eq!(
            a.intersect(&Rect::new(5, 5, 10, 10)),
            Some(Rect::new(5, 5, 5, 5))
        );
    }
}

//! BGRA to NV12 (BT.709, limited range), the input hardware H.264 encoders take.

/// Converts tightly packed BGRA (`width` and `height` even) into NV12 in `out`: a full-size
/// Y plane followed by interleaved U and V at half size, each from a 2x2 block's average.
pub(crate) fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(width * height * 3 / 2, 0);
    let (luma, chroma) = out.split_at_mut(width * height);
    let stride = width * 4;
    for (y, row) in bgra.chunks_exact(stride).take(height).enumerate() {
        let dst = &mut luma[y * width..(y + 1) * width];
        for (d, &[b, g, r, _]) in dst.iter_mut().zip(row.as_chunks::<4>().0) {
            let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
            *d = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
        }
    }
    for cy in 0..height / 2 {
        let top = &bgra[cy * 2 * stride..];
        let bottom = &bgra[(cy * 2 + 1) * stride..];
        let dst = &mut chroma[cy * width..(cy + 1) * width];
        for cx in 0..width / 2 {
            let mut sum = [0i32; 3];
            for row in [top, bottom] {
                for px in row[cx * 8..cx * 8 + 8].as_chunks::<4>().0 {
                    sum[0] += i32::from(px[2]);
                    sum[1] += i32::from(px[1]);
                    sum[2] += i32::from(px[0]);
                }
            }
            let [r, g, b] = sum.map(|v| (v + 2) / 4);
            dst[cx * 2] = (((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            dst[cx * 2 + 1] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(bgra: [u8; 4]) -> (u8, u8, u8) {
        let mut out = Vec::new();
        bgra_to_nv12(&bgra.repeat(4), 2, 2, &mut out);
        assert_eq!(out.len(), 6);
        assert!(out[..4].iter().all(|&v| v == out[0]));
        (out[0], out[4], out[5])
    }

    #[test]
    fn reference_colours() {
        // BT.709 limited range: black 16, white 235, neutral chroma 128.
        assert_eq!(convert([0, 0, 0, 255]), (16, 128, 128));
        assert_eq!(convert([255, 255, 255, 255]), (235, 128, 128));
        // Pure red is about (63, 102, 240), blue about (32, 240, 118).
        let (y, u, v) = convert([0, 0, 255, 255]);
        assert!((62..=64).contains(&y) && (101..=103).contains(&u) && (239..=240).contains(&v));
        let (y, u, v) = convert([255, 0, 0, 255]);
        assert!((31..=33).contains(&y) && (239..=240).contains(&u) && (117..=119).contains(&v));
    }

    #[test]
    fn chroma_averages_two_by_two_blocks() {
        // 4x2: left block black, right block white.
        let mut bgra = Vec::new();
        for _ in 0..2 {
            bgra.extend([[0, 0, 0, 255]; 2].concat());
            bgra.extend([[255, 255, 255, 255]; 2].concat());
        }
        let mut out = Vec::new();
        bgra_to_nv12(&bgra, 4, 2, &mut out);
        assert_eq!(&out[..4], &[16, 16, 235, 235]);
        assert_eq!(&out[8..], &[128, 128, 128, 128]);
    }
}

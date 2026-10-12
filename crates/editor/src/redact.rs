//! Blur and pixelate, applied to the image's own pixels so the original content does not
//! survive in an export. Pixels are opaque RGBA, 4 bytes each, rows tightly packed.

/// Replaces each `cell` x `cell` block of the region with its average colour. Blocks are
/// aligned to the region's corner, so the same region always pixelates the same way.
pub fn pixelate(px: &mut [u8], width: u32, region: (u32, u32, u32, u32), cell: u32) {
    let (x0, y0, w, h) = region;
    let cell = cell.max(1);
    let stride = width as usize * 4;
    for by in (y0..y0 + h).step_by(cell as usize) {
        let bh = cell.min(y0 + h - by);
        for bx in (x0..x0 + w).step_by(cell as usize) {
            let bw = cell.min(x0 + w - bx);
            let mut sum = [0u64; 3];
            for y in by..by + bh {
                let row = y as usize * stride;
                for x in bx..bx + bw {
                    let i = row + x as usize * 4;
                    sum[0] += u64::from(px[i]);
                    sum[1] += u64::from(px[i + 1]);
                    sum[2] += u64::from(px[i + 2]);
                }
            }
            let n = u64::from(bw) * u64::from(bh);
            let avg = sum.map(|s| ((s + n / 2) / n) as u8);
            for y in by..by + bh {
                let row = y as usize * stride;
                for x in bx..bx + bw {
                    let i = row + x as usize * 4;
                    px[i..i + 3].copy_from_slice(&avg);
                }
            }
        }
    }
}

/// Gaussian kernel for blur radius `radius` (sigma = radius / 2), normalised to sum to 1.
/// Ported from Emulsion's `emulsion-filters` (MIT).
fn gaussian_kernel(radius: f32) -> Vec<f32> {
    let sigma = (radius / 2.0).max(0.3);
    let r = (sigma * 3.0).ceil() as i32;
    let mut k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let s: f32 = k.iter().sum();
    for v in &mut k {
        *v /= s;
    }
    k
}

/// Gaussian-blurs the region. Pixels just outside it feed the blur (so its edges blend in)
/// but are not changed. Weights are renormalised at the image border so edges do not darken.
pub fn blur(px: &mut [u8], width: u32, height: u32, region: (u32, u32, u32, u32), radius: f32) {
    let (x0, y0, w, h) = region;
    let k = gaussian_kernel(radius);
    let r = (k.len() / 2) as u32;
    // Work on the region plus a margin of the kernel radius, clamped to the image.
    let mx0 = x0.saturating_sub(r);
    let my0 = y0.saturating_sub(r);
    let mx1 = (x0 + w + r).min(width);
    let my1 = (y0 + h + r).min(height);
    let (mw, mh) = ((mx1 - mx0) as usize, (my1 - my0) as usize);
    let stride = width as usize * 4;
    let mut src = vec![[0f32; 3]; mw * mh];
    for y in 0..mh {
        let row = (my0 as usize + y) * stride + mx0 as usize * 4;
        for x in 0..mw {
            let i = row + x * 4;
            src[y * mw + x] = [f32::from(px[i]), f32::from(px[i + 1]), f32::from(px[i + 2])];
        }
    }
    let r = r as usize;
    // Horizontal pass, only on rows that the vertical pass will read.
    let mut tmp = vec![[0f32; 3]; mw * mh];
    for y in 0..mh {
        for x in 0..mw {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(mw - 1);
            let (mut acc, mut wsum) = ([0f32; 3], 0f32);
            for xx in lo..=hi {
                let wgt = k[xx + r - x];
                let p = src[y * mw + xx];
                acc[0] += p[0] * wgt;
                acc[1] += p[1] * wgt;
                acc[2] += p[2] * wgt;
                wsum += wgt;
            }
            tmp[y * mw + x] = acc.map(|a| a / wsum);
        }
    }
    // Vertical pass, written back only inside the region.
    let (ox, oy) = ((x0 - mx0) as usize, (y0 - my0) as usize);
    for y in oy..oy + h as usize {
        let lo = y.saturating_sub(r);
        let hi = (y + r).min(mh - 1);
        let row = (my0 as usize + y) * stride;
        for x in ox..ox + w as usize {
            let (mut acc, mut wsum) = ([0f32; 3], 0f32);
            for yy in lo..=hi {
                let wgt = k[yy + r - y];
                let p = tmp[yy * mw + x];
                acc[0] += p[0] * wgt;
                acc[1] += p[1] * wgt;
                acc[2] += p[2] * wgt;
                wsum += wgt;
            }
            let i = row + (mx0 as usize + x) * 4;
            for c in 0..3 {
                px[i + c] = (acc[c] / wsum).round().clamp(0., 255.) as u8;
            }
        }
    }
}

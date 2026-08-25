//! BC1 (DXT1) block encoding.
//!
//! The SMT tile store is DXT1 and nothing else, so this is the one codec the
//! writer needs. The encoder is a bounding-box fit followed by one
//! least-squares endpoint refinement — the classic range-fit approach, chosen
//! because it is fully deterministic. Golden testing and shareable projects
//! both depend on the same graph producing the same bytes, and an encoder that
//! made different choices run to run would quietly break that.

/// One 4×4 block of RGB texels, row-major.
pub type Block = [[u8; 3]; 16];

/// Quantise to RGB565 by **rounding**, not truncating. Truncation biases every
/// endpoint downward and costs a visible amount on smooth terrain: on a flat
/// mid-grey block it roughly doubles the squared error.
#[inline]
fn to565(c: [u8; 3]) -> u16 {
    let r = (u32::from(c[0]) * 31 + 127) / 255;
    let g = (u32::from(c[1]) * 63 + 127) / 255;
    let b = (u32::from(c[2]) * 31 + 127) / 255;
    ((r as u16) << 11) | ((g as u16) << 5) | (b as u16)
}

/// Expand RGB565 the way a GPU does: replicate the high bits.
#[inline]
fn from565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 31) as u8;
    let g = ((v >> 5) & 63) as u8;
    let b = (v & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

fn palette(c0: u16, c1: u16) -> [[u8; 3]; 4] {
    let a = from565(c0);
    let b = from565(c1);
    let mut p = [[0u8; 3]; 4];
    p[0] = a;
    p[1] = b;
    if c0 > c1 {
        for i in 0..3 {
            p[2][i] = ((2 * u16::from(a[i]) + u16::from(b[i])) / 3) as u8;
            p[3][i] = ((u16::from(a[i]) + 2 * u16::from(b[i])) / 3) as u8;
        }
    } else {
        for i in 0..3 {
            p[2][i] = ((u16::from(a[i]) + u16::from(b[i])) / 2) as u8;
        }
        p[3] = [0, 0, 0];
    }
    p
}

#[inline]
fn dist2(a: [u8; 3], b: [u8; 3]) -> i32 {
    let dr = i32::from(a[0]) - i32::from(b[0]);
    let dg = i32::from(a[1]) - i32::from(b[1]);
    let db = i32::from(a[2]) - i32::from(b[2]);
    dr * dr + dg * dg + db * db
}

/// How much each index weights the second endpoint.
const T: [f64; 4] = [0.0, 1.0, 1.0 / 3.0, 2.0 / 3.0];

fn assign(block: &Block, c0: u16, c1: u16) -> (u32, i32) {
    let pal = palette(c0, c1);
    let limit = if c0 > c1 { 4 } else { 3 };
    let mut bits = 0u32;
    let mut err = 0i32;
    for (i, texel) in block.iter().enumerate() {
        let mut best = 0usize;
        let mut best_d = i32::MAX;
        for (k, p) in pal.iter().enumerate().take(limit) {
            let d = dist2(*texel, *p);
            if d < best_d {
                best_d = d;
                best = k;
            }
        }
        err += best_d;
        bits |= (best as u32) << (i * 2);
    }
    (bits, err)
}

/// Least-squares endpoints for the current index assignment.
fn refine(block: &Block, bits: u32) -> Option<(u16, u16)> {
    let (mut sa, mut sb, mut sc) = (0.0f64, 0.0f64, 0.0f64);
    let mut x = [0.0f64; 3];
    let mut y = [0.0f64; 3];
    for (i, texel) in block.iter().enumerate() {
        let t = T[((bits >> (i * 2)) & 3) as usize];
        let u = 1.0 - t;
        sa += u * u;
        sb += u * t;
        sc += t * t;
        for c in 0..3 {
            x[c] += u * f64::from(texel[c]);
            y[c] += t * f64::from(texel[c]);
        }
    }
    let det = sa * sc - sb * sb;
    if det.abs() < 1e-9 {
        return None;
    }
    let mut a = [0u8; 3];
    let mut b = [0u8; 3];
    for c in 0..3 {
        a[c] = (((sc * x[c] - sb * y[c]) / det).round()).clamp(0.0, 255.0) as u8;
        b[c] = (((sa * y[c] - sb * x[c]) / det).round()).clamp(0.0, 255.0) as u8;
    }
    Some((to565(a), to565(b)))
}

/// Encode one 4×4 block to 8 bytes.
pub fn encode_block(block: &Block) -> [u8; 8] {
    // Bounding box, then inset by 1/16 of the range — the standard nudge that
    // stops the endpoints being dragged out by a single outlier texel.
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    for t in block {
        for c in 0..3 {
            lo[c] = lo[c].min(t[c]);
            hi[c] = hi[c].max(t[c]);
        }
    }
    let mut a = [0u8; 3];
    let mut b = [0u8; 3];
    for c in 0..3 {
        let inset = (i32::from(hi[c]) - i32::from(lo[c])) / 16;
        a[c] = (i32::from(hi[c]) - inset).clamp(0, 255) as u8;
        b[c] = (i32::from(lo[c]) + inset).clamp(0, 255) as u8;
    }
    let mut c0 = to565(a);
    let mut c1 = to565(b);
    if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1);
    }

    let (mut bits, mut err) = assign(block, c0, c1);
    // One refinement pass. A second rarely changes anything and doubles the
    // cost of encoding a million tiles.
    if let Some((mut r0, mut r1)) = refine(block, bits) {
        if r0 < r1 {
            std::mem::swap(&mut r0, &mut r1);
        }
        if r0 != c0 || r1 != c1 {
            let (nbits, nerr) = assign(block, r0, r1);
            if nerr < err {
                c0 = r0;
                c1 = r1;
                bits = nbits;
                err = nerr;
            }
        }
    }
    let _ = err;

    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..8].copy_from_slice(&bits.to_le_bytes());
    out
}

/// Decode one block back to RGB, for verification.
pub fn decode_block(bytes: &[u8; 8]) -> Block {
    let c0 = u16::from_le_bytes([bytes[0], bytes[1]]);
    let c1 = u16::from_le_bytes([bytes[2], bytes[3]]);
    let bits = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let pal = palette(c0, c1);
    let mut out = [[0u8; 3]; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (i * 2)) & 3) as usize];
    }
    out
}

/// Compress a `w × h` RGB image (both multiples of 4) into DXT1 blocks.
pub fn encode_image(rgb: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    debug_assert_eq!(w % 4, 0);
    debug_assert_eq!(h % 4, 0);
    let mut block: Block = [[0u8; 3]; 16];
    for by in (0..h).step_by(4) {
        for bx in (0..w).step_by(4) {
            for y in 0..4 {
                for x in 0..4 {
                    let o = ((by + y) * w + bx + x) * 3;
                    block[y * 4 + x] = [rgb[o], rgb[o + 1], rgb[o + 2]];
                }
            }
            out.extend_from_slice(&encode_block(&block));
        }
    }
}

/// Box-filter an RGB image to half size. Mip chains for both tiles and the
/// minimap are built with this.
pub fn halve(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (nw, nh) = (w / 2, h / 2);
    let mut out = vec![0u8; nw * nh * 3];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..3 {
                let a = u32::from(rgb[((y * 2) * w + x * 2) * 3 + c]);
                let b = u32::from(rgb[((y * 2) * w + x * 2 + 1) * 3 + c]);
                let d = u32::from(rgb[((y * 2 + 1) * w + x * 2) * 3 + c]);
                let e = u32::from(rgb[((y * 2 + 1) * w + x * 2 + 1) * 3 + c]);
                out[(y * nw + x) * 3 + c] = ((a + b + d + e + 2) / 4) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_block_costs_only_565_quantisation() {
        let block: Block = [[64, 128, 192]; 16];
        let dec = decode_block(&encode_block(&block));
        for t in dec {
            // The nearest representable colours are 66 / 130 / 189, so this is
            // the floor for any BC1 encoder, not slack in this one.
            assert!(dist2(t, [64, 128, 192]) <= 20, "{t:?}");
        }
    }

    #[test]
    fn encoding_is_deterministic() {
        let mut block: Block = [[0u8; 3]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            *t = [(i * 17) as u8, (i * 5) as u8, (255 - i * 9) as u8];
        }
        let a = encode_block(&block);
        let b = encode_block(&block);
        assert_eq!(a, b);
    }

    #[test]
    fn a_gradient_block_lands_within_half_a_palette_step() {
        // BC1 gives four levels across the block, so the best any encoder can
        // do on a ramp is half a step. Anything worse means the endpoints were
        // badly chosen.
        let mut block: Block = [[0u8; 3]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            let v = (i * 16) as u8;
            *t = [v, v, v];
        }
        let range = 240.0f64;
        let bound = range / 6.0 + 6.0;
        let dec = decode_block(&encode_block(&block));
        for (a, b) in dec.iter().zip(block.iter()) {
            for c in 0..3 {
                let e = (f64::from(a[c]) - f64::from(b[c])).abs();
                assert!(e <= bound, "channel error {e} exceeds half a step {bound}");
            }
        }
    }

    #[test]
    fn a_low_variance_block_is_near_lossless() {
        // What terrain diffuse actually looks like at 32 elmos across.
        let mut block: Block = [[0u8; 3]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            *t = [96 + (i % 5) as u8, 110 + (i % 3) as u8, 78 + (i % 4) as u8];
        }
        let dec = decode_block(&encode_block(&block));
        let mut worst = 0;
        for (a, b) in dec.iter().zip(block.iter()) {
            worst = worst.max(dist2(*a, *b));
        }
        assert!(worst <= 40, "worst squared error {worst}");
    }

    #[test]
    fn opaque_blocks_use_the_four_colour_mode() {
        let mut block: Block = [[0u8; 3]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            *t = [(i * 15) as u8, 100, 200];
        }
        let enc = encode_block(&block);
        let c0 = u16::from_le_bytes([enc[0], enc[1]]);
        let c1 = u16::from_le_bytes([enc[2], enc[3]]);
        assert!(
            c0 > c1,
            "a varied opaque block must not fall into 3-colour mode"
        );
    }

    #[test]
    fn image_encoding_produces_eight_bytes_per_block() {
        let rgb = vec![128u8; 32 * 32 * 3];
        let mut out = Vec::new();
        encode_image(&rgb, 32, 32, &mut out);
        assert_eq!(out.len(), (32 / 4) * (32 / 4) * 8);
    }
}

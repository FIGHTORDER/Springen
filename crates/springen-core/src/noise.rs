//! Gradient noise. `f64` throughout — the port plan is explicit that using
//! `f32` here would break bit-identity with the prototype's golden files.

use crate::fdlibm;
use crate::rng::{hash2i, to_i32};

/// The prototype's literal, which is bit-identical to `f64::consts::TAU`.
const TAU: f64 = std::f64::consts::TAU;

fn grad_dot(ix: f64, iy: f64, seed: f64, dx: f64, dy: f64) -> f64 {
    let h = hash2i(to_i32(ix), to_i32(iy), to_i32(seed));
    let a = f64::from(h & 1023) / 1024.0 * TAU;
    fdlibm::cos(a) * dx + fdlibm::sin(a) * dy
}

/// Perlin-style gradient noise, output roughly -1..1.
pub fn perlin2(x: f64, y: f64, seed: f64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let n00 = grad_dot(x0, y0, seed, fx, fy);
    let n10 = grad_dot(x0 + 1.0, y0, seed, fx - 1.0, fy);
    let n01 = grad_dot(x0, y0 + 1.0, seed, fx, fy - 1.0);
    let n11 = grad_dot(x0 + 1.0, y0 + 1.0, seed, fx - 1.0, fy - 1.0);
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    // 1.4142, not SQRT_2: the prototype used a truncated constant to bring
    // gradient noise roughly into -1..1, and the golden fields carry it.
    #[allow(clippy::approx_constant)]
    let scale = 1.4142;
    (a + (b - a) * v) * scale
}

/// The lattice coordinate a tiling noise should hash, folded into `0..period`.
///
/// `v` is always a `floor` result, so the remainder is exact.
fn wrap(v: f64, period: f64) -> f64 {
    let p = if period >= 1.0 { period } else { 1.0 };
    let m = v % p;
    if m < 0.0 {
        m + p
    } else {
        m
    }
}

/// [`perlin2`], but repeating exactly every `period` units on both axes.
///
/// Detail textures are small power-of-two tiles the GPU repeats forever, so a
/// seam is not a cosmetic problem — it is a grid of seams across the whole map.
/// Folding the gradient lattice makes the two edges share their gradients, so
/// the tile meets itself exactly.
pub fn perlin2_tiled(x: f64, y: f64, seed: f64, period: f64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let (wx0, wy0) = (wrap(x0, period), wrap(y0, period));
    let (wx1, wy1) = (wrap(x0 + 1.0, period), wrap(y0 + 1.0, period));
    let n00 = grad_dot(wx0, wy0, seed, fx, fy);
    let n10 = grad_dot(wx1, wy0, seed, fx - 1.0, fy);
    let n01 = grad_dot(wx0, wy1, seed, fx, fy - 1.0);
    let n11 = grad_dot(wx1, wy1, seed, fx - 1.0, fy - 1.0);
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    #[allow(clippy::approx_constant)]
    let scale = 1.4142;
    (a + (b - a) * v) * scale
}

/// [`fbm`] over [`perlin2_tiled`]. `period` is the tile size in the same units
/// as `x` and `y` at the first octave, and follows `lacunarity` up with them.
///
/// Eight positional arguments, matching [`fbm`] plus the period, because the
/// two are read side by side and an options struct on one of them only would
/// make the pair harder to compare, not easier.
#[allow(clippy::too_many_arguments)]
pub fn fbm_tiled(
    x: f64,
    y: f64,
    seed: f64,
    octaves: i64,
    gain: f64,
    lacunarity: f64,
    ridged: bool,
    period: f64,
) -> f64 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut tot = 0.0;
    let mut fx = x;
    let mut fy = y;
    let mut p = period;
    let mut prev = 1.0;
    for o in 0..octaves {
        let mut n = perlin2_tiled(fx, fy, seed + (o as f64) * 8191.0, p);
        if ridged {
            n = 1.0 - n.abs();
            n *= n;
            n *= prev;
            prev = n;
        }
        sum += n * amp;
        tot += amp;
        amp *= gain;
        fx *= lacunarity;
        fy *= lacunarity;
        p *= lacunarity;
    }
    if tot > 0.0 {
        sum / tot
    } else {
        0.0
    }
}

/// Fractal sum. The accumulation order is load-bearing: reordering it changes
/// the low bits and the golden fields stop matching.
pub fn fbm(
    x: f64,
    y: f64,
    seed: f64,
    octaves: i64,
    gain: f64,
    lacunarity: f64,
    ridged: bool,
) -> f64 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut tot = 0.0;
    let mut fx = x;
    let mut fy = y;
    let mut prev = 1.0;
    for o in 0..octaves {
        let mut n = perlin2(fx, fy, seed + (o as f64) * 8191.0);
        if ridged {
            n = 1.0 - n.abs();
            n *= n;
            n *= prev;
            prev = n;
        }
        sum += n * amp;
        tot += amp;
        amp *= gain;
        fx *= lacunarity;
        fy *= lacunarity;
    }
    if tot > 0.0 {
        sum / tot
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiled_noise_repeats_and_plain_noise_does_not() {
        let period = 8.0;
        let mut plain_differs = false;
        // Eighths, so `x` and `x + period` have identical fractional bits and
        // the comparison is genuinely exact rather than exact-to-a-tolerance.
        for i in 0..37 {
            let (x, y) = (i as f64 / 8.0, i as f64 / 4.0);
            // Exact, because the folded lattice makes the two corners the
            // same corner rather than merely a similar one.
            assert_eq!(
                perlin2_tiled(x, y, 3.0, period).to_bits(),
                perlin2_tiled(x + period, y, 3.0, period).to_bits(),
                "x wrap at {x}"
            );
            assert_eq!(
                perlin2_tiled(x, y, 3.0, period).to_bits(),
                perlin2_tiled(x, y + period, 3.0, period).to_bits(),
                "y wrap at {y}"
            );
            assert_eq!(
                fbm_tiled(x, y, 3.0, 5, 0.5, 2.0, false, period).to_bits(),
                fbm_tiled(x + period, y, 3.0, 5, 0.5, 2.0, false, period).to_bits(),
                "fbm x wrap at {x}"
            );
            if perlin2(x, y, 3.0) != perlin2(x + period, y, 3.0) {
                plain_differs = true;
            }
        }
        assert!(
            plain_differs,
            "the untiled noise must not repeat, or the test proves nothing"
        );
    }

    #[test]
    fn tiling_does_not_flatten_the_noise() {
        // Folding the lattice must not collapse the range: a tiled field that
        // is nearly constant would tile beautifully and look like nothing.
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for y in 0..64 {
            for x in 0..64 {
                let v = fbm_tiled(
                    x as f64 / 64.0 * 8.0,
                    y as f64 / 64.0 * 8.0,
                    5.0,
                    4,
                    0.5,
                    2.0,
                    false,
                    8.0,
                );
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        assert!(hi - lo > 0.5, "tiled fbm spans only {:.3}", hi - lo);
    }
}

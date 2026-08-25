//! Square, resolution-independent fields.
//!
//! Storage is `f32` (the prototype's `Float32Array`); every intermediate
//! computation happens in `f64` and is narrowed only on store, which is what
//! JavaScript does implicitly. Keeping that split is what makes the golden
//! `.f32` dumps match byte for byte.

use std::sync::Arc;

/// Written out rather than `v.clamp(0.0, 1.0)` so it reads as the prototype's
/// helper, which it must match exactly for NaN and signed zero alike.
#[allow(clippy::manual_clamp)]
#[inline]
pub fn clamp01(v: f64) -> f64 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

/// A square field of `res × res` samples with 1 (grayscale) or 3 (colour)
/// interleaved channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub res: usize,
    pub ch: usize,
    pub data: Vec<f32>,
}

impl Field {
    pub fn new(res: usize, ch: usize) -> Field {
        Field {
            res,
            ch,
            data: vec![0.0; res * res * ch],
        }
    }

    pub fn gray(res: usize) -> Field {
        Field::new(res, 1)
    }

    #[inline]
    pub fn get(&self, i: usize) -> f64 {
        self.data[i] as f64
    }

    #[inline]
    pub fn set(&mut self, i: usize, v: f64) {
        self.data[i] = v as f32;
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.res + x] as f64
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn fill(&mut self, v: f64) {
        for s in self.data.iter_mut() {
            *s = v as f32;
        }
    }

    /// Fill the field one row at a time, across every core.
    ///
    /// The rule that makes this safe is the one every node kernel already
    /// follows: read the inputs, write each output sample exactly once, never
    /// read the output. Under that rule the answer does not depend on the
    /// order rows are computed in, or on how many threads there are — which
    /// matters more here than speed, because the golden suite compares the
    /// `f32` dumps byte for byte.
    ///
    /// The callback gets the row index and that row's `res * ch` samples.
    /// Erosion is deliberately not expressed this way: it accumulates into the
    /// field it is reading, and the order of those accumulations is part of
    /// the result.
    pub fn par_rows(&mut self, f: impl Fn(usize, &mut [f32]) + Sync + Send) {
        use rayon::prelude::*;
        let stride = self.res * self.ch;
        self.data
            .par_chunks_mut(stride)
            .enumerate()
            .for_each(|(y, row)| f(y, row));
    }

    /// Little-endian raw dump, the format the golden `.f32` files use.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for v in &self.data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    pub fn stats(&self) -> Stats {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for v in &self.data {
            let v = *v as f64;
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
        }
        if !min.is_finite() {
            min = 0.0;
            max = 0.0;
        }
        Stats { min, max }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
}

/// A field as passed between nodes. Evaluation results are shared, never
/// mutated, so `Arc` keeps the graph cache cheap.
pub type SharedField = Arc<Field>;

/// Collapse a multi-channel field to luminance. Grayscale passes through
/// untouched; a 4-channel splat field is read on its RGB channels.
pub fn as_gray(f: &SharedField) -> SharedField {
    if f.ch == 1 {
        return Arc::clone(f);
    }
    let n = f.res * f.res;
    let ch = f.ch;
    let mut out = Field::gray(f.res);
    for i in 0..n {
        let v = 0.299 * f.get(i * ch) + 0.587 * f.get(i * ch + 1) + 0.114 * f.get(i * ch + 2);
        out.set(i, v);
    }
    Arc::new(out)
}

/// Promote a grayscale field to colour by replicating the channel, or drop a
/// splat field's alpha.
pub fn as_color(f: &SharedField) -> SharedField {
    if f.ch == 3 {
        return Arc::clone(f);
    }
    let n = f.res * f.res;
    let mut out = Field::new(f.res, 3);
    if f.ch == 1 {
        for i in 0..n {
            let v = f.get(i);
            out.set(i * 3, v);
            out.set(i * 3 + 1, v);
            out.set(i * 3 + 2, v);
        }
    } else {
        let ch = f.ch;
        for i in 0..n {
            for c in 0..3 {
                out.set(i * 3 + c, f.get(i * ch + c));
            }
        }
    }
    Arc::new(out)
}

/// Bilinear sample of a grayscale field in pixel coordinates, edge-clamped.
pub fn sample_bilinear(f: &Field, x: f64, y: f64) -> f64 {
    let r = f.res;
    let rf = (r - 1) as f64;
    let x = if x < 0.0 {
        0.0
    } else if x > rf {
        rf
    } else {
        x
    };
    let y = if y < 0.0 {
        0.0
    } else if y > rf {
        rf
    } else {
        y
    };
    let x0 = x as usize;
    let y0 = y as usize;
    let x1 = if x0 + 1 < r { x0 + 1 } else { x0 };
    let y1 = if y0 + 1 < r { y0 + 1 } else { y0 };
    let tx = x - x0 as f64;
    let ty = y - y0 as f64;
    let a = f.data[y0 * r + x0] as f64;
    let b = f.data[y0 * r + x1] as f64;
    let c = f.data[y1 * r + x0] as f64;
    let e = f.data[y1 * r + x1] as f64;
    let top = a + (b - a) * tx;
    (a + (b - a) * tx) + ((c + (e - c) * tx) - top) * ty
}

/// Bilinear sample of an n-channel field.
pub fn sample_color(f: &Field, x: f64, y: f64, out: &mut [f64]) {
    let r = f.res;
    let ch = f.ch;
    let rf = (r - 1) as f64;
    let x = if x < 0.0 {
        0.0
    } else if x > rf {
        rf
    } else {
        x
    };
    let y = if y < 0.0 {
        0.0
    } else if y > rf {
        rf
    } else {
        y
    };
    let x0 = x as usize;
    let y0 = y as usize;
    let x1 = if x0 + 1 < r { x0 + 1 } else { x0 };
    let y1 = if y0 + 1 < r { y0 + 1 } else { y0 };
    let tx = x - x0 as f64;
    let ty = y - y0 as f64;
    for c in 0..ch {
        let a = f.data[(y0 * r + x0) * ch + c] as f64;
        let b = f.data[(y0 * r + x1) * ch + c] as f64;
        let d = f.data[(y1 * r + x0) * ch + c] as f64;
        let e = f.data[(y1 * r + x1) * ch + c] as f64;
        let top = a + (b - a) * tx;
        let bot = d + (e - d) * tx;
        out[c] = top + (bot - top) * ty;
    }
}

/// Resample a grayscale field to another square resolution.
pub fn resample(f: &Field, res: usize) -> Field {
    if f.res == res {
        return f.clone();
    }
    let mut out = Field::gray(res);
    let s = (f.res - 1) as f64 / (res - 1) as f64;
    for y in 0..res {
        for x in 0..res {
            let v = sample_bilinear(f, x as f64 * s, y as f64 * s);
            out.set(y * res + x, v);
        }
    }
    out
}

/// Three box passes approximate a gaussian. The radius arrives in pixels,
/// already converted from the elmo-authored parameter by the caller.
pub fn box_blur(src: &[f32], res: usize, radius: usize) -> Vec<f32> {
    box_blur_xy(src, res, radius, radius)
}

/// As [`box_blur`], with a separate radius per axis.
///
/// A radius is authored in elmos, and on a non-square map one elmo is a
/// different number of lattice samples along each axis — so a blur that is
/// round in the world is elliptical here. The two radii are equal on a square
/// map, where this is the same three-pass separable blur it has always been,
/// arithmetic included.
pub fn box_blur_xy(src: &[f32], res: usize, radius_x: usize, radius_y: usize) -> Vec<f32> {
    let wx = (radius_x * 2 + 1) as f64;
    let wy = (radius_y * 2 + 1) as f64;
    let mut tmp = vec![0.0f32; res * res];
    let mut out: Vec<f32> = src.to_vec();
    let last = res - 1;
    let irx = radius_x as isize;
    let iry = radius_y as isize;
    for _pass in 0..3 {
        for y in 0..res {
            let row = y * res;
            let mut sum = 0.0f64;
            for k in -irx..=irx {
                let idx = k.clamp(0, last as isize) as usize;
                sum += out[row + idx] as f64;
            }
            for x in 0..res {
                tmp[row + x] = (sum / wx) as f32;
                let add = (x + radius_x + 1).min(last);
                let sub = x.saturating_sub(radius_x);
                sum += out[row + add] as f64 - out[row + sub] as f64;
            }
        }
        let mut nx = vec![0.0f32; res * res];
        for x in 0..res {
            let mut s2 = 0.0f64;
            for k in -iry..=iry {
                let idx = k.clamp(0, last as isize) as usize;
                s2 += tmp[idx * res + x] as f64;
            }
            for y in 0..res {
                nx[y * res + x] = (s2 / wy) as f32;
                let a2 = (y + radius_y + 1).min(last);
                let b2 = y.saturating_sub(radius_y);
                s2 += tmp[a2 * res + x] as f64 - tmp[b2 * res + x] as f64;
            }
        }
        out = nx;
    }
    out
}

pub fn hex_to_rgb(hex: &str) -> [f64; 3] {
    let mut h = hex.trim().replace('#', "");
    if h.len() == 3 {
        let b: Vec<char> = h.chars().collect();
        h = format!("{0}{0}{1}{1}{2}{2}", b[0], b[1], b[2]);
    }
    let n = u32::from_str_radix(&h, 16).unwrap_or(0x808080);
    [
        f64::from((n >> 16) & 255) / 255.0,
        f64::from((n >> 8) & 255) / 255.0,
        f64::from(n & 255) / 255.0,
    ]
}

pub fn rgb_to_hex(r: f64, g: f64, b: f64) -> String {
    fn h(v: f64) -> String {
        let x = (v * 255.0).round().clamp(0.0, 255.0) as u32;
        format!("{x:02x}")
    }
    format!("#{}{}{}", h(r), h(g), h(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_and_colour_round_trip() {
        let mut f = Field::gray(4);
        f.set(5, 0.25);
        let shared: SharedField = Arc::new(f);
        let c = as_color(&shared);
        assert_eq!(c.ch, 3);
        assert_eq!(c.get(15), 0.25);
        let g = as_gray(&c);
        assert!((g.get(5) - 0.25).abs() < 1e-7);
    }

    #[test]
    fn hex_parses_short_and_long_forms() {
        assert_eq!(hex_to_rgb("#fff"), [1.0, 1.0, 1.0]);
        assert_eq!(hex_to_rgb("#000000"), [0.0, 0.0, 0.0]);
        assert_eq!(rgb_to_hex(1.0, 0.0, 0.5), "#ff0080");
    }
}

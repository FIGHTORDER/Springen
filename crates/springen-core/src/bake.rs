//! Resampling fields onto the true SMF layer grids.
//!
//! The one rule that matters here: **category layers are never interpolated.**
//! Bilinear resampling of a typemap blends index 0 with index 1 into indices
//! that do not exist in `mapinfo.terrainTypes` — the prototype shipped 200+
//! distinct values where there should have been 2 before this was found.

use std::sync::Arc;

use rayon::prelude::*;

use crate::field::{as_color, as_gray, clamp01, sample_bilinear, sample_color, Field, SharedField};
use crate::project::Context;
use crate::ramps::hypso;

/// How a layer is sampled when it is resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resample {
    Bilinear,
    /// Mandatory for typemap and grass.
    Nearest,
}

/// Resample a normalised field onto a `w × h` grid of integer samples.
pub fn bake_gray(
    field: &SharedField,
    w: usize,
    h: usize,
    bit_depth: u8,
    resample: Resample,
    scale: Option<f64>,
) -> Vec<u16> {
    let field = as_gray(field);
    let maxv = scale.unwrap_or(if bit_depth == 16 { 65535.0 } else { 255.0 });
    let mut out = vec![0u16; w * h];
    let sx = (field.res - 1) as f64 / (w.saturating_sub(1).max(1)) as f64;
    let sy = (field.res - 1) as f64 / (h.saturating_sub(1).max(1)) as f64;
    // Every output sample reads the source and writes exactly one slot, so
    // splitting by row is bit-for-bit the same answer on any thread count.
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let v = match resample {
                Resample::Nearest => {
                    let nx = ((x as f64 * sx).round() as isize).clamp(0, field.res as isize - 1);
                    let ny = ((y as f64 * sy).round() as isize).clamp(0, field.res as isize - 1);
                    field.data[ny as usize * field.res + nx as usize] as f64
                }
                Resample::Bilinear => sample_bilinear(&field, x as f64 * sx, y as f64 * sy),
            };
            *o = (clamp01(v) * maxv).round() as u16;
        }
    });
    out
}

/// Typemap bytes are **indices** into `mapinfo.terrainTypes`, not scaled greys.
/// Terrain type 1 must be byte 1, not 255.
pub fn bake_index(field: &SharedField, w: usize, h: usize, levels: u32) -> Vec<u16> {
    let levels = levels.max(1);
    bake_gray(
        field,
        w,
        h,
        8,
        Resample::Nearest,
        Some(f64::from(levels - 1)),
    )
}

/// Bake a colour field to 8-bit RGB samples at an arbitrary resolution.
pub fn bake_color(field: &SharedField, w: usize, h: usize) -> Vec<u16> {
    let field = as_color(field);
    let mut out = vec![0u16; w * h * 3];
    let sx = (field.res - 1) as f64 / (w.saturating_sub(1).max(1)) as f64;
    let sy = (field.res - 1) as f64 / (h.saturating_sub(1).max(1)) as f64;
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let mut tmp = [0.0f64; 4];
        for x in 0..w {
            sample_color(&field, x as f64 * sx, y as f64 * sy, &mut tmp);
            for c in 0..3 {
                row[x * 3 + c] = (clamp01(tmp[c]) * 255.0).round() as u16;
            }
        }
    });
    out
}

/// Bake a splat distribution to 8-bit RGBA. `splats.texScales` and `texMults`
/// are per RGBA channel, so all four are carried.
pub fn bake_rgba(field: &SharedField, w: usize, h: usize) -> Vec<u16> {
    let src = if field.ch == 4 {
        Arc::clone(field)
    } else {
        let c = as_color(field);
        let mut f = Field::new(c.res, 4);
        for i in 0..c.res * c.res {
            for k in 0..3 {
                f.set(i * 4 + k, c.get(i * 3 + k));
            }
        }
        Arc::new(f)
    };
    let mut out = vec![0u16; w * h * 4];
    let sx = (src.res - 1) as f64 / (w.saturating_sub(1).max(1)) as f64;
    let sy = (src.res - 1) as f64 / (h.saturating_sub(1).max(1)) as f64;
    out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let mut tmp = [0.0f64; 4];
        for x in 0..w {
            sample_color(&src, x as f64 * sx, y as f64 * sy, &mut tmp);
            for c in 0..4 {
                row[x * 4 + c] = (clamp01(tmp[c]) * 255.0).round() as u16;
            }
        }
    });
    out
}

/// Shaded-relief RGB preview with sea-level flooding and optional contour
/// banding — the same painting the evaluator produces.
pub fn bake_shaded(
    field: &SharedField,
    w: usize,
    h: usize,
    ctx: &Context,
    sea_t: f64,
    contour: f64,
) -> Vec<u8> {
    let field = as_gray(field);
    let mut out = vec![0u8; w * h * 3];
    let sx = (field.res - 1) as f64 / (w.saturating_sub(1).max(1)) as f64;
    let sy = (field.res - 1) as f64 / (h.saturating_sub(1).max(1)) as f64;
    let vscale = ctx.height_range;
    // The minimap is drawn over the world, not the domain: `w` samples span
    // `elmos_x` and `h` samples span `elmos_y`, and on a 16x8 map those give
    // different ground distances per step. Equal when the world is square.
    let hstep_x = ctx.elmos_x / (w.saturating_sub(1).max(1)) as f64;
    let hstep_y = ctx.elmos_y / (h.saturating_sub(1).max(1)) as f64;
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let fx = x as f64;
            let fy = y as f64;
            let v = sample_bilinear(&field, fx * sx, fy * sy);
            let hl = sample_bilinear(&field, (fx - 1.0) * sx, fy * sy);
            let hr = sample_bilinear(&field, (fx + 1.0) * sx, fy * sy);
            let hu = sample_bilinear(&field, fx * sx, (fy - 1.0) * sy);
            let hd = sample_bilinear(&field, fx * sx, (fy + 1.0) * sy);
            let gx = (hr - hl) * vscale / (2.0 * hstep_x);
            let gy = (hd - hu) * vscale / (2.0 * hstep_y);
            let nl = (gx * gx + gy * gy + 1.0).sqrt();
            let lam = (-gx * 0.55 - gy * 0.55 + 1.0) / (nl * 1.42);
            let mut shade = 0.42 + 0.75 * clamp01(lam);
            let c;
            if v < sea_t {
                let depth = clamp01((sea_t - v) / sea_t.max(0.001));
                c = [
                    26.0 - 12.0 * depth,
                    74.0 - 34.0 * depth,
                    96.0 - 40.0 * depth,
                ];
                shade = 0.85 + 0.15 * (1.0 - depth);
            } else {
                c = hypso(if sea_t >= 1.0 {
                    v
                } else {
                    (v - sea_t) / (1.0 - sea_t)
                });
            }
            let mut r = c[0] * shade;
            let mut g = c[1] * shade;
            let mut b = c[2] * shade;
            if contour > 0.0 && v >= sea_t {
                let band = v * contour;
                let frac = (band - band.round()).abs();
                // Contour bands are a screen-space decoration: this turns the
                // gradient back into field-value change per minimap pixel so
                // the band keeps one width. Left in its original form, using
                // the X step, so the golden minimap hashes are untouched; on a
                // non-square map the bands are a hair wider along one axis.
                let grad = ((gx * gx + gy * gy).sqrt() / vscale * hstep_x * contour).max(1e-6);
                if frac < grad * 0.5 {
                    r = r * 0.55 + 30.0;
                    g = g * 0.55 + 22.0;
                    b = b * 0.55 + 14.0;
                }
            }
            row[x * 3] = (r as i64).clamp(0, 255) as u8;
            row[x * 3 + 1] = (g as i64).clamp(0, 255) as u8;
            row[x * 3 + 2] = (b as i64).clamp(0, 255) as u8;
        }
    });
    out
}

/// Normalise a field to 0..1 over its own extent.
///
/// Real maps do **not** do this — the reference map declares -60..440 but its
/// terrain only reaches 294 elmos. Auto-fitting rescales the terrain on every
/// regeneration and makes the declared range meaningless, which is why
/// [`HeightMode::Absolute`] exists.
pub fn normalised(field: &SharedField) -> SharedField {
    let st = field.stats();
    let d = if st.max - st.min == 0.0 {
        1.0
    } else {
        st.max - st.min
    };
    let mut out = Field::gray(field.res);
    for i in 0..field.len() {
        out.set(i, (field.get(i) - st.min) / d);
    }
    Arc::new(out)
}

/// How the 0..1 field maps onto the declared `minHeight`/`maxHeight` range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HeightMode {
    /// Stretch the terrain to fill 0..65535. Simple, but the declared range
    /// stops meaning anything.
    #[default]
    Fit,
    /// Treat the field as already spanning the declared range, so terrain sits
    /// inside a fixed vertical scale the way hand-made maps do.
    Absolute,
}

/// Prepare the height field for the 16-bit vertex-lattice bake, **and** the
/// height range it has to be declared with.
///
/// These two are one decision and were previously two. A raw field sample `v`
/// means the elmo height `minHeight + v * (maxHeight - minHeight)` — that is
/// the whole reason the waterline can be derived instead of set, and it is
/// what every sea test, the water tint, the slope and the preview assume.
/// `HeightMode::Fit` stretched the field to fill 0..1 for the sake of 16-bit
/// precision and left the declared range alone, which silently broke that
/// correspondence: on the default starter the terrain the graph put at the
/// waterline came out 57 elmos below it, so the engine's water sat 57 elmos
/// above the shoreline the map was painted with.
///
/// Stretching the field and shrinking the declared range by the same factor
/// keeps both properties: the heightmap uses its full range, and
/// `declared_min + sample * declared_range` is still the elmo height the
/// graph meant. The declared range is rounded outward to whole elmos and the
/// remap is done against the rounded numbers, so what `mapinfo.lua` says is
/// exactly what the samples mean.
pub fn height_and_range(
    field: &SharedField,
    mode: HeightMode,
    min_height: f64,
    max_height: f64,
) -> (SharedField, f64, f64) {
    let range = max_height - min_height;
    if mode == HeightMode::Absolute || range <= 0.0 {
        return (Arc::clone(field), min_height, max_height);
    }
    let st = field.stats();
    // A flat field, or one carrying NaN, has no extent to fit to.
    if !matches!(st.min.partial_cmp(&st.max), Some(std::cmp::Ordering::Less)) {
        return (Arc::clone(field), min_height, max_height);
    }
    let decl_min = (min_height + st.min * range).floor();
    let decl_max = (min_height + st.max * range).ceil();
    // A range the engine would reject or divide by is worse than wasted
    // precision, so a field with almost no relief keeps the declared range.
    if decl_max - decl_min < 1.0 {
        return (Arc::clone(field), min_height, max_height);
    }
    let decl_range = decl_max - decl_min;
    let mut out = Field::gray(field.res);
    out.par_rows(|y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let v = field.get(y * field.res + x);
            *o = (((min_height + v * range) - decl_min) / decl_range) as f32;
        }
    });
    (Arc::new(out), decl_min, decl_max)
}

/// Prepare the height field for the 16-bit vertex-lattice bake.
///
/// Superseded by [`height_and_range`]: the field and the range it is read back
/// with cannot be decided separately without moving every elmo height.
pub fn height_for_bake(field: &SharedField, mode: HeightMode) -> SharedField {
    match mode {
        HeightMode::Fit => normalised(field),
        HeightMode::Absolute => Arc::clone(field),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_layers_keep_their_index_set() {
        // Two terrain types in, exactly two byte values out.
        let mut f = Field::gray(8);
        for i in 0..f.len() {
            f.set(i, if i % 3 == 0 { 1.0 } else { 0.0 });
        }
        let shared: SharedField = Arc::new(f);
        let out = bake_index(&shared, 17, 17, 2);
        let mut seen: Vec<u16> = out.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, vec![0, 1], "typemap must stay a set of indices");

        // Bilinear on the same data invents values that are not terrain types.
        let bil = bake_gray(&shared, 17, 17, 8, Resample::Bilinear, Some(1.0));
        let mut bseen: Vec<u16> = bil;
        bseen.sort_unstable();
        bseen.dedup();
        assert_eq!(bseen, vec![0, 1], "scale 1 still rounds to two values");
    }

    #[test]
    fn fitting_the_range_keeps_the_waterline_at_zero_elmos() {
        use crate::project::water_level_t;
        let (min_h, max_h) = (-80.0, 420.0);
        let range = max_h - min_h;
        let sea_t = water_level_t(min_h, max_h);

        // A field that spans neither 0 nor 1, which is what a real graph
        // produces and what makes Fit move things.
        let mut f = Field::gray(64);
        for i in 0..f.len() {
            f.set(i, 0.1448 + (i as f64 / (f.len() - 1) as f64) * 0.7533);
        }
        let raw: SharedField = Arc::new(f);

        // What the graph meant: the sample at the waterline is at 0 elmos.
        // Read the baked field back through the declared range and it must
        // still be at 0 elmos, or every sea test, the water tint and the
        // engine's own water plane disagree about where the shore is.
        for mode in [HeightMode::Fit, HeightMode::Absolute] {
            let (baked, dmin, dmax) = height_and_range(&raw, mode, min_h, max_h);
            let drange = dmax - dmin;
            for i in 0..raw.len() {
                let meant = min_h + raw.get(i) * range;
                let read_back = dmin + baked.get(i) * drange;
                assert!(
                    (meant - read_back).abs() < 0.02,
                    "{mode:?}: sample {i} meant {meant:.3} elmos, reads back {read_back:.3}"
                );
            }
            // And specifically the shoreline.
            let shore = dmin + ((min_h + sea_t * range) - dmin);
            assert!(
                shore.abs() < 1e-9,
                "{mode:?}: the waterline moved to {shore}"
            );
        }

        // Fit must still be worth doing: the samples span the full 16-bit range.
        let (fitted, _, _) = height_and_range(&raw, HeightMode::Fit, min_h, max_h);
        let st = fitted.stats();
        assert!(
            st.min < 0.01 && st.max > 0.99,
            "Fit wasted the range: {st:?}"
        );
    }

    #[test]
    fn sixteen_bit_bake_uses_the_full_range() {
        let mut f = Field::gray(4);
        for i in 0..f.len() {
            f.set(i, i as f64 / (f.len() - 1) as f64);
        }
        let shared: SharedField = Arc::new(f);
        let out = bake_gray(&shared, 4, 4, 16, Resample::Bilinear, None);
        assert_eq!(out[0], 0);
        assert_eq!(out[15], 65535);
    }
}

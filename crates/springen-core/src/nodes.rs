//! The base node inventory: Generate, Combine, Filter, Select, Erode, Layout
//! and the SMF Output carve-outs.
//!
//! Each `eval` is a direct port of the prototype's. Where a line looks odd it
//! is usually load-bearing.

use std::sync::Arc;

use crate::fdlibm;
use crate::field::{box_blur_xy, clamp01, sample_bilinear, Field, SharedField};
use crate::graph::{
    p_bool, p_elmos, p_enum, p_float, p_int, p_points, p_strokes, p_text, Chan, Inputs, NodeSpec,
    Params, RegistryBuilder, Stroke, StrokeMode,
};
use crate::noise::{fbm, perlin2};
use crate::project::Context;
use crate::rng::{hash2i, to_i32, Mulberry32};

pub(crate) fn inp<'a>(ins: &'a Inputs, port: &str) -> Option<&'a SharedField> {
    ins.get(port)
}

pub(crate) fn spec(
    type_name: &'static str,
    label: &'static str,
    cat: &'static str,
    inputs: &'static [&'static str],
    params: Vec<crate::graph::ParamSpec>,
    eval: crate::graph::EvalFn,
) -> NodeSpec {
    NodeSpec {
        type_name,
        label,
        cat,
        inputs,
        in_types: &[],
        produces: Chan::Gray,
        output: None,
        params,
        eval,
    }
}

pub fn register(b: &mut RegistryBuilder) {
    /* -- generators ------------------------------------------------------ */
    b.def(spec(
        "noise",
        "Noise",
        "Generate",
        &[],
        vec![
            p_elmos("feature", "Feature size", 3072.0, 32.0, 40000.0),
            p_int("octaves", "Octaves", 6.0, 1.0, 12.0),
            p_float("gain", "Persistence", 0.5, 0.1, 0.9),
            p_float("lacunarity", "Lacunarity", 2.0, 1.2, 4.0),
            p_int("seed", "Seed offset", 0.0, 0.0, 9999.0),
        ],
        |_ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let freq = ctx.elmos / p.f("feature").max(1.0);
            // A feature size is a world length. The lattice is square and the
            // world need not be, so Y fits more of them in: exactly `freq` on
            // a square map, where the factor is 1.0 and this is a no-op.
            let freq_y = freq * ctx.aspect_y();
            let s = f64::from(ctx.seed) + p.f("seed") * 7919.0;
            let oct = p.i("octaves");
            let (gain, lac) = (p.f("gain"), p.f("lacunarity"));
            let rm1 = (r - 1) as f64;
            f.par_rows(|y, row| {
                let ny = (y as f64 / rm1) * freq_y;
                for (x, o) in row.iter_mut().enumerate() {
                    let v = fbm((x as f64 / rm1) * freq, ny, s, oct, gain, lac, false);
                    *o = clamp01(v * 0.5 + 0.5) as f32;
                }
            });
            Arc::new(f)
        },
    ));

    b.def(spec(
        "ridged",
        "Ridged noise",
        "Generate",
        &[],
        vec![
            p_elmos("feature", "Feature size", 4096.0, 32.0, 40000.0),
            p_int("octaves", "Octaves", 6.0, 1.0, 12.0),
            p_float("gain", "Persistence", 0.55, 0.1, 0.9),
            p_float("lacunarity", "Lacunarity", 2.1, 1.2, 4.0),
            p_int("seed", "Seed offset", 3.0, 0.0, 9999.0),
        ],
        |_ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let freq = ctx.elmos / p.f("feature").max(1.0);
            // See `noise`: a world length, so Y repeats by the world's aspect.
            let freq_y = freq * ctx.aspect_y();
            let s = f64::from(ctx.seed) + p.f("seed") * 7919.0;
            let oct = p.i("octaves");
            let (gain, lac) = (p.f("gain"), p.f("lacunarity"));
            let rm1 = (r - 1) as f64;
            // min/max track the unrounded values, before the f32 store, so
            // the extent cannot be recovered from the field afterwards -- it
            // is reduced across the rows instead.
            let (mn, mx) = {
                use rayon::prelude::*;
                f.data
                    .par_chunks_mut(r)
                    .enumerate()
                    .map(|(y, row)| {
                        let ny = (y as f64 / rm1) * freq_y;
                        let mut lo = f64::INFINITY;
                        let mut hi = f64::NEG_INFINITY;
                        for (x, o) in row.iter_mut().enumerate() {
                            let v = fbm((x as f64 / rm1) * freq, ny, s, oct, gain, lac, true);
                            *o = v as f32;
                            if v < lo {
                                lo = v;
                            }
                            if v > hi {
                                hi = v;
                            }
                        }
                        (lo, hi)
                    })
                    .reduce(
                        || (f64::INFINITY, f64::NEG_INFINITY),
                        |a, b| (a.0.min(b.0), a.1.max(b.1)),
                    )
            };
            let d = if mx - mn == 0.0 { 1.0 } else { mx - mn };
            for i in 0..f.len() {
                let v = (f.get(i) - mn) / d;
                f.set(i, v);
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "voronoi",
        "Voronoi",
        "Generate",
        &[],
        vec![
            p_elmos("cell", "Cell size", 2048.0, 64.0, 20000.0),
            p_float("jitter", "Jitter", 0.85, 0.0, 1.0),
            p_enum("mode", "Mode", "f1", &["f1", "f2-f1", "cell"]),
            p_int("seed", "Seed offset", 11.0, 0.0, 9999.0),
        ],
        |_ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let cells = (ctx.elmos / p.f("cell").max(1.0)).max(1.0);
            // A cell size is a world length: Y fits `aspect_y` times as many.
            let cells_y = cells * ctx.aspect_y();
            let s = f64::from(ctx.seed) + p.f("seed") * 7919.0;
            let seed_i = to_i32(s);
            let jitter = p.f("jitter");
            let mode = p.s("mode").to_string();
            let rm1 = (r - 1) as f64;
            f.par_rows(|y, row| {
                for x in 0..r {
                    let px = (x as f64 / rm1) * cells;
                    let py = (y as f64 / rm1) * cells_y;
                    let cx = px.floor();
                    let cy = py.floor();
                    let mut d1 = 1e9;
                    let mut d2 = 1e9;
                    let mut id: u32 = 0;
                    for oy in -1..=1 {
                        for ox in -1..=1 {
                            let gx = cx + f64::from(ox);
                            let gy = cy + f64::from(oy);
                            let h = hash2i(to_i32(gx), to_i32(gy), seed_i);
                            let jx = gx + 0.5 + (f64::from(h & 255) / 255.0 - 0.5) * jitter;
                            let jy = gy + 0.5 + (f64::from((h >> 8) & 255) / 255.0 - 0.5) * jitter;
                            let dd = ((jx - px) * (jx - px) + (jy - py) * (jy - py)).sqrt();
                            if dd < d1 {
                                d2 = d1;
                                d1 = dd;
                                id = h;
                            } else if dd < d2 {
                                d2 = dd;
                            }
                        }
                    }
                    let v = match mode.as_str() {
                        "f1" => clamp01(d1 / 1.2),
                        "f2-f1" => clamp01((d2 - d1) / 1.2),
                        _ => f64::from((id >> 16) & 255) / 255.0,
                    };
                    row[x] = v as f32;
                }
            });
            Arc::new(f)
        },
    ));

    // The doorway an imported map comes through.
    //
    // It is a generator like any other, which is the entire design: from the
    // moment an imported `.sd7` is behind this node it is terrain, and `grade`,
    // `ramp`, the erosion nodes, the masks, `symmetry`, the metal placer and
    // the bake all work on it without knowing where it came from.
    //
    // No file is opened here. Loading happens where the `Context` is built, so
    // `eval` stays a pure function of its inputs and the signature cache and
    // golden parity keep meaning what they mean.
    b.def(spec(
        "import",
        "Imported terrain",
        "Generate",
        &[],
        vec![p_text("name", "Raster", crate::raster::Rasters::TERRAIN)],
        |_ins, p, ctx| {
            let r = ctx.res;
            let Some(src) = ctx.rasters.get(p.s("name")) else {
                // Flat, and the bake refuses it by name rather than shipping
                // a level plane with nothing anywhere saying why.
                return Arc::new(Field::gray(r));
            };
            if src.res == r {
                return src.clone();
            }
            // Bilinear: an imported height field is a continuous surface, and
            // this is the one resample in the tool that is *not* category data.
            let mut f = Field::gray(r);
            let last = (r - 1).max(1) as f64;
            let sl = (src.res - 1).max(1) as f64;
            f.par_rows(|y, row| {
                let sy = (y as f64 / last) * sl;
                for (x, o) in row.iter_mut().enumerate() {
                    let sx = (x as f64 / last) * sl;
                    *o = sample_bilinear(src, sx, sy) as f32;
                }
            });
            Arc::new(f)
        },
    ));

    b.def(spec(
        "constant",
        "Constant",
        "Generate",
        &[],
        vec![p_float("value", "Value", 0.5, 0.0, 1.0)],
        |_ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            f.fill(p.f("value"));
            Arc::new(f)
        },
    ));

    b.def(spec(
        "radial",
        "Radial shape",
        "Generate",
        &[],
        vec![
            p_float("cx", "Center X", 0.5, -0.5, 1.5),
            p_float("cy", "Center Y", 0.5, -0.5, 1.5),
            p_elmos("radius", "Radius", 3072.0, 64.0, 60000.0),
            p_float("falloff", "Falloff", 1.6, 0.1, 6.0),
            p_bool("invert", "Invert", false),
        ],
        |_ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let rad = p.f("radius").max(1.0) / ctx.elmos;
            // The lattice is square and the world need not be, so a shape that
            // is round here is an ellipse on the ground. Stretching Y by the
            // world's aspect makes it round where it matters; the factor is
            // exactly 1.0 on a square map.
            let ky = ctx.aspect_y();
            let (pcx, pcy, falloff, invert) = (p.f("cx"), p.f("cy"), p.f("falloff"), p.b("invert"));
            let rm1 = (r - 1) as f64;
            f.par_rows(|y, row| {
                let ny = (y as f64 / rm1 - pcy) * ky;
                for (x, o) in row.iter_mut().enumerate() {
                    let nx = x as f64 / rm1 - pcx;
                    let d = (nx * nx + ny * ny).sqrt() / rad;
                    let mut v = clamp01(1.0 - d);
                    v = fdlibm::pow(v, falloff);
                    *o = if invert { 1.0 - v } else { v } as f32;
                }
            });
            Arc::new(f)
        },
    ));

    b.def(spec(
        "gradient",
        "Linear gradient",
        "Generate",
        &[],
        vec![
            p_float("angle", "Angle", 0.0, 0.0, 360.0),
            p_float("offset", "Offset", 0.0, -1.0, 1.0),
            p_float("scale", "Scale", 1.0, 0.05, 4.0),
        ],
        |_ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let a = p.f("angle") * std::f64::consts::PI / 180.0;
            let (ca, sa) = (fdlibm::cos(a), fdlibm::sin(a));
            let (scale, offset) = (p.f("scale"), p.f("offset"));
            let rm1 = (r - 1) as f64;
            f.par_rows(|y, row| {
                let ny = y as f64 / rm1 - 0.5;
                for (x, o) in row.iter_mut().enumerate() {
                    let nx = x as f64 / rm1 - 0.5;
                    *o = clamp01((nx * ca + ny * sa) * scale + 0.5 + offset) as f32;
                }
            });
            Arc::new(f)
        },
    ));

    /* -- combine --------------------------------------------------------- */
    b.def(spec(
        "mix",
        "Combine",
        "Combine",
        &["A", "B", "Mask"],
        vec![
            p_enum(
                "mode",
                "Mode",
                "blend",
                &[
                    "blend",
                    "add",
                    "subtract",
                    "multiply",
                    "min",
                    "max",
                    "screen",
                    "difference",
                ],
            ),
            p_float("amount", "Amount", 0.5, 0.0, 1.0),
        ],
        |ins, p, ctx| {
            let a_in = inp(ins, "A");
            let b_in = inp(ins, "B");
            let m_in = inp(ins, "Mask");
            let mut f = Field::gray(ctx.res);
            let mode = p.s("mode").to_string();
            let amount = p.f("amount");
            for i in 0..f.len() {
                let a = a_in.map(|f| f.get(i)).unwrap_or(0.0);
                let b = b_in.map(|f| f.get(i)).unwrap_or(0.0);
                let v = match mode.as_str() {
                    "add" => a + b,
                    "subtract" => a - b,
                    "multiply" => a * b,
                    "min" => a.min(b),
                    "max" => a.max(b),
                    "screen" => 1.0 - (1.0 - a) * (1.0 - b),
                    "difference" => (a - b).abs(),
                    // `blend` is a + (b - a) * 1 in the prototype; the
                    // rounding of that form is not always exactly b.
                    _ => a + (b - a) * 1.0,
                };
                let t = amount * m_in.map(|f| f.get(i)).unwrap_or(1.0);
                f.set(i, a + (v - a) * t);
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "invert",
        "Invert",
        "Filter",
        &["In"],
        vec![],
        |ins, _p, ctx| {
            let mut f = Field::gray(ctx.res);
            if let Some(i_in) = inp(ins, "In") {
                for i in 0..f.len() {
                    f.set(i, 1.0 - i_in.get(i));
                }
            }
            Arc::new(f)
        },
    ));

    /* -- filters --------------------------------------------------------- */
    b.def(spec(
        "curve",
        "Curve",
        "Filter",
        &["In"],
        vec![
            p_enum(
                "mode",
                "Shape",
                "gain",
                &["gain", "bias", "power", "smoothstep"],
            ),
            p_float("amount", "Amount", 0.5, 0.01, 0.99),
        ],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let mode = p.s("mode").to_string();
            let amount = p.f("amount");
            for i in 0..f.len() {
                let v = clamp01(i_in.get(i));
                let o = match mode.as_str() {
                    "bias" => {
                        fdlibm::pow(v, fdlibm::log(clamp01(amount) + 1e-6) / fdlibm::log(0.5))
                    }
                    "power" => fdlibm::pow(v, amount * 4.0),
                    "smoothstep" => {
                        let t = clamp01((v - (0.5 - amount / 2.0)) / amount.max(1e-6));
                        t * t * (3.0 - 2.0 * t)
                    }
                    _ => {
                        let k = fdlibm::log(1.0 - clamp01(amount) + 1e-6) / fdlibm::log(0.5);
                        if v < 0.5 {
                            fdlibm::pow(2.0 * v, k) / 2.0
                        } else {
                            1.0 - fdlibm::pow(2.0 - 2.0 * v, k) / 2.0
                        }
                    }
                };
                f.set(i, clamp01(o));
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "terrace",
        "Terrace",
        "Filter",
        &["In", "Mask"],
        vec![
            p_int("steps", "Steps", 8.0, 2.0, 64.0),
            p_float("sharpness", "Sharpness", 0.7, 0.0, 1.0),
        ],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let m_in = inp(ins, "Mask");
            let steps = p.f("steps");
            let sharpness = p.f("sharpness");
            for i in 0..f.len() {
                let v = clamp01(i_in.get(i));
                let s = v * steps;
                let fl = s.floor();
                let fr = s - fl;
                let e = if sharpness >= 1.0 {
                    0.0
                } else {
                    fdlibm::pow(fr, 1.0 / (1.0001 - sharpness))
                };
                let t = (fl + e.min(1.0)) / steps;
                let amt = m_in.map(|f| f.get(i)).unwrap_or(1.0);
                f.set(i, v + (t - v) * amt);
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "remap",
        "Remap",
        "Filter",
        &["In"],
        vec![
            p_float("inMin", "In min", 0.0, -1.0, 2.0),
            p_float("inMax", "In max", 1.0, -1.0, 2.0),
            p_float("outMin", "Out min", 0.0, -1.0, 2.0),
            p_float("outMax", "Out max", 1.0, -1.0, 2.0),
            p_bool("clamp", "Clamp", true),
        ],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let (in_min, in_max) = (p.f("inMin"), p.f("inMax"));
            let (out_min, out_max) = (p.f("outMin"), p.f("outMax"));
            let do_clamp = p.b("clamp");
            let d = if in_max - in_min == 0.0 {
                1e-6
            } else {
                in_max - in_min
            };
            for i in 0..f.len() {
                let mut v = (i_in.get(i) - in_min) / d;
                v = out_min + v * (out_max - out_min);
                f.set(i, if do_clamp { clamp01(v) } else { v });
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "normalize",
        "Normalize",
        "Filter",
        &["In"],
        vec![],
        |ins, _p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let st = i_in.stats();
            let d = if st.max - st.min == 0.0 {
                1.0
            } else {
                st.max - st.min
            };
            for i in 0..f.len() {
                f.set(i, (i_in.get(i) - st.min) / d);
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "blur",
        "Blur",
        "Filter",
        &["In", "Mask"],
        vec![p_elmos("radius", "Radius", 256.0, 8.0, 8000.0)],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let px = |per: f64| {
                (p.f("radius") * per)
                    .round()
                    .min((ctx.res / 3) as f64)
                    .max(1.0) as usize
            };
            // A radius is in elmos, and one elmo is a different number of
            // samples per axis unless the world is square.
            let blurred = box_blur_xy(
                &i_in.data,
                ctx.res,
                px(ctx.px_per_elmo_x()),
                px(ctx.px_per_elmo_y()),
            );
            let m_in = inp(ins, "Mask");
            for i in 0..f.len() {
                let t = m_in.map(|f| f.get(i)).unwrap_or(1.0);
                let base = i_in.get(i);
                f.set(i, base + (blurred[i] as f64 - base) * t);
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "warp",
        "Warp",
        "Filter",
        &["In", "Amount"],
        vec![
            p_elmos("strength", "Strength", 512.0, 0.0, 10000.0),
            p_elmos("feature", "Feature size", 2048.0, 32.0, 40000.0),
            p_int("seed", "Seed offset", 17.0, 0.0, 9999.0),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let freq = ctx.elmos / p.f("feature").max(1.0);
            let freq_y = freq * ctx.aspect_y();
            let s = f64::from(ctx.seed) + p.f("seed") * 7919.0;
            // Displacement is authored in elmos, so it is a different number
            // of samples along each axis.
            let spx = p.f("strength") * ctx.px_per_elmo_x();
            let spy = p.f("strength") * ctx.px_per_elmo_y();
            let a_in = inp(ins, "Amount");
            let rm1 = (r - 1) as f64;
            f.par_rows(|y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let u = (x as f64 / rm1) * freq;
                    let v = (y as f64 / rm1) * freq_y;
                    let dx = perlin2(u, v, s);
                    let dy = perlin2(u + 5.2, v + 1.3, s + 991.0);
                    let a = a_in.map(|f| f.get(y * r + x)).unwrap_or(1.0);
                    let (kx, kz) = (spx * a, spy * a);
                    *o = sample_bilinear(i_in, x as f64 + dx * kx, y as f64 + dy * kz) as f32;
                }
            });
            Arc::new(f)
        },
    ));

    /* -- selectors ------------------------------------------------------- */
    b.def(spec(
        "slopemask",
        "Slope mask",
        "Select",
        &["In"],
        vec![
            p_float("minDeg", "Min angle", 20.0, 0.0, 89.0),
            p_float("maxDeg", "Max angle", 90.0, 1.0, 90.0),
            p_float("falloff", "Falloff", 8.0, 0.1, 45.0),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let (min_deg, max_deg, falloff) = (p.f("minDeg"), p.f("maxDeg"), p.f("falloff"));
            let vscale = ctx.height_range;
            let (hx, hy) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
            f.par_rows(|y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let deg = slope_degrees_aniso(i_in, x, y, r, vscale, hx, hy);
                    let lo = clamp01((deg - min_deg) / falloff);
                    let hi = clamp01((max_deg - deg) / falloff);
                    *o = lo.min(hi) as f32;
                }
            });
            Arc::new(f)
        },
    ));

    b.def(spec(
        "heightmask",
        "Height mask",
        "Select",
        &["In"],
        vec![
            p_float("min", "Min", 0.3, 0.0, 1.0),
            p_float("max", "Max", 1.0, 0.0, 1.0),
            p_float("falloff", "Falloff", 0.08, 0.001, 0.5),
        ],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let (lo_v, hi_v, falloff) = (p.f("min"), p.f("max"), p.f("falloff"));
            for i in 0..f.len() {
                let v = i_in.get(i);
                f.set(
                    i,
                    clamp01((v - lo_v) / falloff).min(clamp01((hi_v - v) / falloff)),
                );
            }
            Arc::new(f)
        },
    ));

    /* -- simulation ------------------------------------------------------ */
    b.def(spec(
        "thermal",
        "Thermal erosion",
        "Erode",
        &["In", "Mask"],
        vec![
            p_int("iterations", "Iterations", 30.0, 1.0, 400.0),
            p_float("talus", "Talus angle", 35.0, 1.0, 80.0),
            p_float("rate", "Rate", 0.5, 0.01, 1.0),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(Field::gray(r));
            };
            let mut h: Vec<f32> = i_in.data.clone();
            // Repose is an angle, so the height difference that clears it
            // depends on how far apart the two samples are on the ground —
            // which is not the same along both axes unless the world is
            // square. Neighbours 0/1 step in X, 2/3 in Z.
            let tan_repose = fdlibm::tan(p.f("talus") * std::f64::consts::PI / 180.0);
            let talus_x = tan_repose * ctx.elmo_per_px_x() / ctx.height_range.max(1e-6);
            let talus_y = tan_repose * ctx.elmo_per_px_y() / ctx.height_range.max(1e-6);
            let talus_n = [talus_x, talus_x, talus_y, talus_y];
            let m_in = inp(ins, "Mask");
            let rate = p.f("rate");
            let iterations = p.i("iterations");
            // delta is a Float32Array in the prototype; the narrowing on every
            // accumulation is part of the result.
            let mut delta = vec![0.0f32; r * r];
            for _ in 0..iterations {
                delta.iter_mut().for_each(|d| *d = 0.0);
                for y in 1..r - 1 {
                    for x in 1..r - 1 {
                        let i = y * r + x;
                        let hc = h[i] as f64;
                        let mut total = 0.0;
                        // The largest *excess* over repose rather than the
                        // largest drop: with a different threshold per axis
                        // the steepest neighbour need not be the one furthest
                        // past its own limit. Identical on a square map, where
                        // the thresholds are equal.
                        let mut emax = 0.0;
                        let mut d = [0.0f64; 4];
                        let nb = [i - 1, i + 1, i - r, i + r];
                        for n in 0..4 {
                            let diff = hc - h[nb[n]] as f64;
                            if diff > talus_n[n] {
                                d[n] = diff;
                                total += diff;
                                let excess = diff - talus_n[n];
                                if excess > emax {
                                    emax = excess;
                                }
                            }
                        }
                        if total > 0.0 {
                            let mv = emax * 0.5 * rate * m_in.map(|f| f.get(i)).unwrap_or(1.0);
                            delta[i] = (delta[i] as f64 - mv) as f32;
                            for n2 in 0..4 {
                                if d[n2] > 0.0 {
                                    delta[nb[n2]] =
                                        (delta[nb[n2]] as f64 + mv * (d[n2] / total)) as f32;
                                }
                            }
                        }
                    }
                }
                for k in 0..h.len() {
                    h[k] = (h[k] as f64 + delta[k] as f64) as f32;
                }
            }
            Arc::new(Field {
                res: r,
                ch: 1,
                data: h,
            })
        },
    ));

    b.def(spec(
        "hydraulic",
        "Hydraulic erosion",
        "Erode",
        &["In", "Mask"],
        vec![
            p_float("density", "Droplet density", 0.6, 0.05, 4.0),
            p_float("inertia", "Inertia", 0.05, 0.0, 0.95),
            p_float("capacity", "Capacity", 4.0, 0.5, 20.0),
            p_float("erode", "Erode rate", 0.3, 0.01, 1.0),
            p_float("deposit", "Deposit rate", 0.3, 0.01, 1.0),
            p_float("evaporate", "Evaporation", 0.02, 0.001, 0.2),
            p_elmos("radius", "Brush radius", 96.0, 8.0, 2000.0),
            p_int("lifetime", "Max steps", 48.0, 8.0, 200.0),
        ],
        hydraulic_eval,
    ));

    /* -- layout (RTS) ---------------------------------------------------- */
    b.def(spec(
        "symmetry",
        "Symmetry",
        "Layout",
        &["In"],
        vec![
            p_enum(
                "mode",
                "Mode",
                "mirrorX",
                &[
                    "mirrorX", "mirrorY", "quad", "rot180", "rot90", "rot72", "diagonal",
                ],
            ),
            p_float("blend", "Blend", 1.0, 0.0, 1.0),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let mode = p.s("mode").to_string();
            let blend = p.f("blend");
            let rf = r as f64;
            let half = rf / 2.0;
            for y in 0..r {
                for x in 0..r {
                    let (mut sx, mut sy) = (x, y);
                    let (xf, yf) = (x as f64, y as f64);
                    match mode.as_str() {
                        "mirrorX" => {
                            if xf >= half {
                                sx = r - 1 - x;
                            }
                        }
                        "mirrorY" => {
                            if yf >= half {
                                sy = r - 1 - y;
                            }
                        }
                        "quad" => {
                            if xf >= half {
                                sx = r - 1 - x;
                            }
                            if yf >= half {
                                sy = r - 1 - y;
                            }
                        }
                        "rot180" => {
                            // Half-open over the raster index. At odd
                            // resolutions this is exactly the prototype's
                            // `y > r/2 || (y == floor(r/2) && x > r/2)`; at
                            // even ones that predicate kept 33 cells too many
                            // and the result was not actually symmetric.
                            if (y * r + x) * 2 > r * r - 1 {
                                sx = r - 1 - x;
                                sy = r - 1 - y;
                            }
                        }
                        "rot90" => {
                            // The C4 fundamental domain must be half-open, or
                            // the centre row and column get two representatives
                            // and the "symmetric" output is not symmetric.
                            let (mut cx, mut cy) = (x, y);
                            let mut guard = 0;
                            while !(cx * 2 + 1 < r && cy * 2 < r) && guard < 4 {
                                let t = cx;
                                cx = cy;
                                cy = r - 1 - t;
                                guard += 1;
                            }
                            sx = cx;
                            sy = cy;
                        }
                        "diagonal" if x + y > r - 1 => {
                            sx = r - 1 - y;
                            sy = r - 1 - x;
                        }
                        // Handled below: five-fold has no lattice image.
                        "rot72" => {}
                        _ => {}
                    }
                    let src = if mode == "rot72" {
                        rot72_source(i_in, r, x, y)
                    } else {
                        i_in.get(sy * r + sx)
                    };
                    let orig = i_in.get(y * r + x);
                    f.set(y * r + x, orig + (src - orig) * blend);
                }
            }
            Arc::new(f)
        },
    ));

    b.def(spec(
        "flatten",
        "Flatten",
        "Layout",
        &["In", "Mask"],
        vec![
            p_float("level", "Level", 0.4, 0.0, 1.0),
            p_float("amount", "Amount", 1.0, 0.0, 1.0),
            p_bool("useLocal", "Use local average", false),
        ],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let m_in = inp(ins, "Mask");
            let mut target = p.f("level");
            if p.b("useLocal") {
                if let Some(m) = m_in {
                    let mut sum = 0.0;
                    let mut wsum = 0.0;
                    for i in 0..i_in.len() {
                        let w = m.get(i);
                        sum += i_in.get(i) * w;
                        wsum += w;
                    }
                    if wsum > 1e-6 {
                        target = sum / wsum;
                    }
                }
            }
            let amount = p.f("amount");
            for k in 0..f.len() {
                let t = m_in.map(|f| f.get(k)).unwrap_or(1.0) * amount;
                let base = i_in.get(k);
                f.set(k, base + (target - base) * t);
            }
            Arc::new(f)
        },
    ));

    // Grading is the plains map's main instrument. `flatten` pulls everything
    // toward one level and takes the map's character with it; `blur` softens
    // without promising anything; this bounds the slope and leaves the shape.
    //
    // What it guarantees is exact: no two neighbouring lattice samples differ
    // by more than the grade. What it does not guarantee is the gradient
    // *magnitude* at a crease, where two cones meet and the two axis readings
    // add — expect a measured slope up to about a quarter over the number you
    // asked for at those samples, which is why `both` is the default: it
    // creases half as hard as either one-sided mode.
    b.def(spec(
        "grade",
        "Grade limit",
        "Layout",
        &["In", "Mask"],
        vec![
            p_float("grade", "Max grade", 12.0, 1.0, 60.0),
            p_enum("mode", "Mode", "both", &["both", "cut", "fill"]),
            p_float("amount", "Amount", 1.0, 0.0, 1.0),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(Field::gray(r));
            };
            let m_in = inp(ins, "Mask");
            // The cone's slope, in field units per lattice step. The world's
            // two extents are used rather than the domain's one, so a grade
            // on a 16x8 map is the grade the engine will measure.
            let t = fdlibm::tan(p.f("grade") * std::f64::consts::PI / 180.0)
                / ctx.height_range.max(1e-6);
            let (ex, ey) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());

            let h: Vec<f64> = (0..i_in.len()).map(|i| i_in.get(i)).collect();
            let mode = p.s("mode");
            let cap = || cone_cap(&h, r, ex, ey, t);
            // Filling is capping upside down, which is the cheapest way to be
            // certain the two directions treat the terrain identically.
            let fill = || {
                let neg: Vec<f64> = h.iter().map(|v| -v).collect();
                cone_cap(&neg, r, ex, ey, t)
                    .into_iter()
                    .map(|v| -v)
                    .collect::<Vec<f64>>()
            };
            let graded: Vec<f64> = match mode {
                "cut" => cap(),
                "fill" => fill(),
                // Halfway between the two: peaks come down and hollows come
                // up by the same rule, and the average of two grade-limited
                // surfaces is still grade-limited.
                _ => {
                    let (a, b) = (cap(), fill());
                    a.iter().zip(b.iter()).map(|(x, y)| (x + y) * 0.5).collect()
                }
            };

            let amount = p.f("amount");
            let mut f = Field::gray(r);
            for k in 0..f.len() {
                let w = m_in.map(|m| m.get(k)).unwrap_or(1.0) * amount;
                f.set(k, h[k] + (graded[k] - h[k]) * w);
            }
            Arc::new(f)
        },
    ));

    /* -- hand editing ----------------------------------------------------- */
    b.def(spec(
        "sculpt",
        "Sculpt",
        "Layout",
        &["In", "Mask"],
        vec![
            p_strokes("strokes", "Strokes"),
            p_enum(
                "symmetry",
                "Mirror strokes",
                "none",
                &[
                    "none", "mirrorX", "mirrorY", "quad", "rot180", "rot90", "rot72", "diagonal",
                ],
            ),
            p_float("amount", "Strength", 1.0, 0.0, 1.0),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(Field::gray(r));
            };
            let strokes = p.strokes("strokes");
            if strokes.is_empty() {
                return Arc::clone(i_in);
            }
            let amount = p.f("amount");
            let sym = p.s("symmetry").to_string();
            let m_in = inp(ins, "Mask");
            let mut h: Vec<f32> = i_in.data.clone();
            // Replayed in order, each stroke reading what the one before it
            // wrote. That is the same rule erosion follows and for the same
            // reason: the order is part of the result, so this cannot be
            // parallelised across strokes.
            for k in strokes {
                apply_stroke(&mut h, r, ctx, k, amount, m_in);
                for (ix, iz) in crate::zk::symmetry_images(k.x, k.z, &sym, ctx.elmos_x, ctx.elmos_y)
                {
                    // A hand edit moves the whole symmetry group, or the map
                    // goes quietly unfair — the same rule `zk::move_group`
                    // exists for on the metal side.
                    let mut image = *k;
                    image.x = ix;
                    image.z = iz;
                    apply_stroke(&mut h, r, ctx, &image, amount, m_in);
                }
            }
            Arc::new(Field {
                res: r,
                ch: 1,
                data: h,
            })
        },
    ));

    /* -- routes ---------------------------------------------------------- */
    b.def(spec(
        "ramp",
        "Ramp",
        "Layout",
        &["In", "Mask"],
        vec![
            p_points("points", "Waypoints", vec![]),
            p_elmos("width", "Width", 320.0, 32.0, 4000.0),
            p_elmos("falloff", "Shoulder", 240.0, 0.0, 4000.0),
            p_float("amount", "Strength", 1.0, 0.0, 1.0),
            p_enum("mode", "Mode", "both", &["both", "cut", "fill"]),
            p_float("ends", "Hold ends", 0.0, 0.0, 0.45),
        ],
        |ins, p, ctx| {
            let r = ctx.res;
            let mut f = Field::gray(r);
            let Some(i_in) = inp(ins, "In") else {
                return Arc::new(f);
            };
            let pts = p.points("points");
            if pts.len() < 2 {
                return Arc::clone(i_in);
            }

            // Waypoints are world coordinates; the field is a square lattice.
            // Everything below happens in lattice space, converted once.
            let (sx, sz) = (ctx.px_per_elmo_x(), ctx.px_per_elmo_y());
            let node: Vec<[f64; 2]> = pts.iter().map(|q| [q[0] * sx, q[1] * sz]).collect();

            // Arc length along the path, so the grade is constant rather than
            // per-segment: two short hops and one long one should climb at the
            // same rate, not at three different ones.
            let mut seg_len = Vec::with_capacity(node.len() - 1);
            let mut total = 0.0;
            for w in node.windows(2) {
                let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
                seg_len.push(d);
                total += d;
            }
            if total <= 1e-9 {
                return Arc::clone(i_in);
            }
            // Height at each waypoint, read from the terrain: a ramp connects
            // what is there, so its ends have to sit on it.
            let at = |q: &[f64; 2]| {
                sample_bilinear(
                    i_in,
                    q[0].clamp(0.0, (r - 1) as f64),
                    q[1].clamp(0.0, (r - 1) as f64),
                )
            };
            let node_h: Vec<f64> = node.iter().map(at).collect();
            let mut node_t = Vec::with_capacity(node.len());
            let mut run = 0.0;
            node_t.push(0.0);
            for d in &seg_len {
                run += d;
                node_t.push(run / total);
            }

            let half = (p.f("width") * 0.5 * sx).max(0.5);
            let shoulder = (p.f("falloff") * sx).max(0.0);
            let amount = clamp01(p.f("amount"));
            let mode = p.s("mode").to_string();
            // Holding the ends keeps the first and last stretch level, so a
            // ramp meets a plateau flat instead of arriving at an angle.
            let hold = p.f("ends").clamp(0.0, 0.45);
            let m_in = inp(ins, "Mask");

            f.par_rows(|y, row| {
                let py = y as f64;
                for (x, o) in row.iter_mut().enumerate() {
                    let px = x as f64;
                    // Nearest point on the polyline, and how far along it is.
                    let mut best_d2 = f64::MAX;
                    let mut best_t = 0.0;
                    for (i, w) in node.windows(2).enumerate() {
                        let (ax, ay) = (w[0][0], w[0][1]);
                        let (bx, by) = (w[1][0], w[1][1]);
                        let (dx, dy) = (bx - ax, by - ay);
                        let len2 = dx * dx + dy * dy;
                        let t = if len2 <= 1e-12 {
                            0.0
                        } else {
                            (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
                        };
                        let (cx, cy) = (ax + dx * t, ay + dy * t);
                        let d2 = (px - cx).powi(2) + (py - cy).powi(2);
                        if d2 < best_d2 {
                            best_d2 = d2;
                            best_t = node_t[i] + (node_t[i + 1] - node_t[i]) * t;
                        }
                    }

                    let d = best_d2.sqrt();
                    if d > half + shoulder {
                        *o = i_in.get(y * r + x) as f32;
                        continue;
                    }
                    // Flat across the corridor, easing out across the shoulder.
                    let w = if d <= half {
                        1.0
                    } else if shoulder <= 0.0 {
                        0.0
                    } else {
                        let u = 1.0 - (d - half) / shoulder;
                        u * u * (3.0 - 2.0 * u)
                    };

                    // Target height: interpolate the waypoint heights along
                    // the path, with the ends held level if asked.
                    let t = if hold > 0.0 {
                        ((best_t - hold) / (1.0 - 2.0 * hold)).clamp(0.0, 1.0)
                    } else {
                        best_t
                    };
                    let mut target = node_h[node_h.len() - 1];
                    for i in 0..node_t.len() - 1 {
                        if t <= node_t[i + 1] || i + 2 == node_t.len() {
                            let span = node_t[i + 1] - node_t[i];
                            let k = if span <= 1e-12 {
                                0.0
                            } else {
                                ((t - node_t[i]) / span).clamp(0.0, 1.0)
                            };
                            target = node_h[i] + (node_h[i + 1] - node_h[i]) * k;
                            break;
                        }
                    }

                    let base = i_in.get(y * r + x);
                    // Cut only lowers, fill only raises. Both is a road bed.
                    let allowed = match mode.as_str() {
                        "cut" => target < base,
                        "fill" => target > base,
                        _ => true,
                    };
                    let mut k = if allowed { w * amount } else { 0.0 };
                    if let Some(m) = m_in {
                        k *= clamp01(m.get(y * r + x));
                    }
                    *o = (base + (target - base) * k) as f32;
                }
            });
            Arc::new(f)
        },
    ));

    /* -- outputs (Spring carve-outs) ------------------------------------- */
    let mut out_height = spec(
        "out_height",
        "Heightmap out",
        "Output",
        &["In"],
        vec![],
        |ins, _p, ctx| {
            inp(ins, "In")
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::new(Field::gray(ctx.res)))
        },
    );
    out_height.output = Some("height");
    b.def(out_height);

    let mut out_metal = spec(
        "out_metal",
        "Metal map out",
        "Output",
        &["In"],
        vec![p_float("gain", "Gain", 1.0, 0.0, 4.0)],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            if let Some(i_in) = inp(ins, "In") {
                let gain = p.f("gain");
                for i in 0..f.len() {
                    f.set(i, clamp01(i_in.get(i) * gain));
                }
            }
            Arc::new(f)
        },
    );
    out_metal.output = Some("metal");
    b.def(out_metal);

    let mut out_type = spec(
        "out_type",
        "Type map out",
        "Output",
        &["In"],
        vec![p_int("levels", "Terrain types", 4.0, 1.0, 255.0)],
        |ins, p, ctx| {
            let mut f = Field::gray(ctx.res);
            if let Some(i_in) = inp(ins, "In") {
                let levels = p.f("levels");
                let denom = (levels - 1.0).max(1.0);
                for i in 0..f.len() {
                    let v = (clamp01(i_in.get(i)) * (levels - 1.0)).round() / denom;
                    f.set(i, v);
                }
            }
            Arc::new(f)
        },
    );
    out_type.output = Some("type");
    b.def(out_type);
}

/// Central-difference slope in degrees, edge-clamped, with the vertical scale
/// taken from the project height range so the angle is a real world angle.
pub fn slope_degrees(f: &Field, x: usize, y: usize, r: usize, vscale: f64, hstep: f64) -> f64 {
    slope_degrees_aniso(f, x, y, r, vscale, hstep, hstep)
}

/// As [`slope_degrees`], but with a separate elmos-per-sample on each axis.
///
/// One lattice sample is not the same distance in both directions on a
/// non-square map: at 16×8 a step in Z covers half the ground a step in X
/// does, so a single `hstep` reports the Z slope twice as steep as it is.
pub fn slope_degrees_aniso(
    f: &Field,
    x: usize,
    y: usize,
    r: usize,
    vscale: f64,
    hstep_x: f64,
    hstep_y: f64,
) -> f64 {
    let xl = if x > 0 { x - 1 } else { x };
    let xr = if x < r - 1 { x + 1 } else { x };
    let yu = if y > 0 { y - 1 } else { y };
    let yd = if y < r - 1 { y + 1 } else { y };
    let gx = (f.at(xr, y) - f.at(xl, y)) * vscale / ((xr - xl) as f64 * hstep_x);
    let gy = (f.at(x, yd) - f.at(x, yu)) * vscale / ((yd - yu) as f64 * hstep_y);
    fdlibm::atan((gx * gx + gy * gy).sqrt()) * 180.0 / std::f64::consts::PI
}

struct Grad {
    gx: f64,
    gy: f64,
    h: f64,
    x0: usize,
    y0: usize,
    u: f64,
    v: f64,
}

fn grad_at(h: &[f32], r: usize, x: f64, y: f64) -> Grad {
    let x0 = (x as isize).clamp(0, r as isize - 2) as usize;
    let y0 = (y as isize).clamp(0, r as isize - 2) as usize;
    let u = x - x0 as f64;
    let v = y - y0 as f64;
    let i = y0 * r + x0;
    let nw = h[i] as f64;
    let ne = h[i + 1] as f64;
    let sw = h[i + r] as f64;
    let se = h[i + r + 1] as f64;
    Grad {
        gx: (ne - nw) * (1.0 - v) + (se - sw) * v,
        gy: (sw - nw) * (1.0 - u) + (se - ne) * u,
        h: nw * (1.0 - u) * (1.0 - v) + ne * u * (1.0 - v) + sw * (1.0 - u) * v + se * u * v,
        x0,
        y0,
        u,
        v,
    }
}

fn hydraulic_eval(ins: &Inputs, p: &Params, ctx: &Context) -> SharedField {
    let r = ctx.res;
    let Some(i_in) = inp(ins, "In") else {
        return Arc::new(Field::gray(r));
    };
    let mut h: Vec<f32> = i_in.data.clone();
    let m_in = inp(ins, "Mask");
    let mut rng = Mulberry32::from_f64(f64::from(ctx.seed) * 7757.0 + 13.0);
    let count = (p.f("density") * (r * r) as f64 / 16.0)
        .round()
        .min(400000.0) as usize;
    let brush = (p.f("radius") * ctx.px_per_elmo).round().clamp(1.0, 8.0) as isize;
    // Everything below works in an isotropic space: X in lattice columns, Z in
    // lattice rows scaled by the world's aspect, so that one unit is the same
    // distance on the ground either way. Water runs downhill on the map rather
    // than downhill on the lattice, which are different directions as soon as
    // the world is not square. `ky` is exactly 1.0 when it is.
    let ky = ctx.aspect_y();
    let brush_y = ((brush as f64 * ky).round() as isize).max(1);

    let (inertia, capacity, erode, deposit, evaporate) = (
        p.f("inertia"),
        p.f("capacity"),
        p.f("erode"),
        p.f("deposit"),
        p.f("evaporate"),
    );
    let lifetime = p.i("lifetime");

    let mut bo: Vec<(isize, isize)> = Vec::new();
    let mut bw: Vec<f64> = Vec::new();
    let mut tw = 0.0;
    for by in -brush_y..=brush_y {
        for bx in -brush..=brush {
            let fy = by as f64 / ky;
            let dd = ((bx * bx) as f64 + fy * fy).sqrt();
            if dd <= brush as f64 {
                let w = 1.0 - dd / (brush + 1) as f64;
                bo.push((bx, by));
                bw.push(w);
                tw += w;
            }
        }
    }
    for w in bw.iter_mut() {
        *w /= tw;
    }

    let rf = (r - 1) as f64;
    for _ in 0..count {
        let mut px = rng.next() * rf;
        let mut py = rng.next() * rf;
        let (mut dx, mut dy) = (0.0f64, 0.0f64);
        let mut water = 1.0f64;
        let mut sed = 0.0f64;
        let mut speed = 0.0f64;
        for _life in 0..lifetime {
            let g = grad_at(&h, r, px, py);
            dx = dx * inertia - g.gx * (1.0 - inertia);
            dy = dy * inertia - (g.gy / ky) * (1.0 - inertia);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-8 {
                break;
            }
            dx /= len;
            dy /= len;
            let nx = px + dx;
            // One step of ground along Z is fewer lattice rows than along X
            // when the world is longer in X.
            let ny = py + dy / ky;
            if nx < 1.0 || nx > (r - 2) as f64 || ny < 1.0 || ny > (r - 2) as f64 {
                break;
            }
            let nh = grad_at(&h, r, nx, ny).h;
            let dh = nh - g.h;
            let maskv = m_in
                .map(|f| f.get((py as usize) * r + px as usize))
                .unwrap_or(1.0);
            let cap = (-dh).max(0.0001) * speed * water * capacity;
            if dh > 0.0 || sed > cap {
                let drop = if dh > 0.0 {
                    dh.min(sed)
                } else {
                    (sed - cap) * deposit
                };
                sed -= drop;
                let i0 = g.y0 * r + g.x0;
                h[i0] = (h[i0] as f64 + drop * (1.0 - g.u) * (1.0 - g.v)) as f32;
                h[i0 + 1] = (h[i0 + 1] as f64 + drop * g.u * (1.0 - g.v)) as f32;
                h[i0 + r] = (h[i0 + r] as f64 + drop * (1.0 - g.u) * g.v) as f32;
                h[i0 + r + 1] = (h[i0 + r + 1] as f64 + drop * g.u * g.v) as f32;
            } else {
                let amt = ((cap - sed) * erode).min(-dh) * maskv;
                let (bx0, by0) = (px as isize, py as isize);
                for bi in 0..bo.len() {
                    let ox = bx0 + bo[bi].0;
                    let oy = by0 + bo[bi].1;
                    if ox < 0 || oy < 0 || ox >= r as isize || oy >= r as isize {
                        continue;
                    }
                    let idx = oy as usize * r + ox as usize;
                    let take = amt * bw[bi];
                    h[idx] = (h[idx] as f64 - take) as f32;
                    sed += take;
                }
            }
            speed = (speed * speed - dh * 40.0).max(0.0).sqrt();
            water *= 1.0 - evaporate;
            px = nx;
            py = ny;
            if water < 0.01 {
                break;
            }
        }
    }
    for v in h.iter_mut() {
        if !v.is_finite() {
            *v = 0.0;
        }
    }
    Arc::new(Field {
        res: r,
        ch: 1,
        data: h,
    })
}

/// Lay one brush stroke into a height field in place.
///
/// Everything about a stroke is in elmos and converted here, like every other
/// node parameter — a brush recorded in pixels would paint a different shape
/// at preview resolution than at bake resolution, which is the whole reason
/// this is a node parameter and not a painted raster.
fn apply_stroke(
    h: &mut [f32],
    r: usize,
    ctx: &Context,
    k: &Stroke,
    amount: f64,
    mask: Option<&SharedField>,
) {
    if k.radius <= 0.0 || amount <= 0.0 {
        return;
    }
    let (px, py) = (ctx.px_per_elmo_x(), ctx.px_per_elmo_y());
    let (cx, cy) = (k.x * px, k.z * py);
    // Elliptical on the lattice so it is round on the ground.
    let (rx, ry) = (k.radius * px, k.radius * py);
    let x0 = (cx - rx).floor().max(0.0) as usize;
    let x1 = ((cx + rx).ceil() as isize).min(r as isize - 1).max(0) as usize;
    let y0 = (cy - ry).floor().max(0.0) as usize;
    let y1 = ((cy + ry).ceil() as isize).min(r as isize - 1).max(0) as usize;
    if x1 < x0 || y1 < y0 {
        return;
    }
    let range = ctx.height_range.max(1e-6);
    // `Smooth` needs the ground as it was before this stroke touched it, or
    // the averaging chases its own output across the disc.
    let before: Option<Vec<f32>> = (k.mode == StrokeMode::Smooth).then(|| h.to_vec());

    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (
                (x as f64 - cx) / rx.max(1e-9),
                (y as f64 - cy) / ry.max(1e-9),
            );
            let d = (dx * dx + dy * dy).sqrt();
            if d >= 1.0 {
                continue;
            }
            // Smoothstep, so a stroke meets the ground tangentially instead of
            // leaving a rim the grade probe then reports as a cliff.
            let t = 1.0 - d;
            let fall = t * t * (3.0 - 2.0 * t);
            let i = y * r + x;
            let w = fall * amount * mask.map(|m| m.get(i)).unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }
            let cur = h[i] as f64;
            let next = match k.mode {
                // Strength is elmos of height; the field is normalised.
                StrokeMode::Raise => cur + k.strength / range * w,
                StrokeMode::Smooth => {
                    let src = before.as_ref().unwrap();
                    let mut sum = 0.0;
                    let mut n = 0.0;
                    for oy in -1isize..=1 {
                        for ox in -1isize..=1 {
                            let sx = (x as isize + ox).clamp(0, r as isize - 1) as usize;
                            let sy = (y as isize + oy).clamp(0, r as isize - 1) as usize;
                            sum += src[sy * r + sx] as f64;
                            n += 1.0;
                        }
                    }
                    cur + (sum / n - cur) * clamp01(k.strength) * w
                }
                StrokeMode::Level => {
                    let target = k.seat / range;
                    cur + (target - cur) * clamp01(k.strength) * w
                }
            };
            h[i] = clamp01(next) as f32;
        }
    }
}

/// How far an absolute stroke's seat has drifted from the ground it now sits
/// on, in elmos, worst case over the history.
///
/// A stroke replays at a world position, not at a piece of terrain. Change a
/// noise seed upstream and every `Level` stroke goes on levelling to a height
/// the ground no longer has anywhere near it — flattening a hilltop to the
/// altitude of a valley that used to be there. Relative strokes do not care,
/// which is why they are the default. This is what lets the tool say so rather
/// than quietly doing it.
pub fn stroke_drift(strokes: &[Stroke], field: &Field, ctx: &Context) -> f64 {
    let range = ctx.height_range.max(1e-6);
    let (px, py) = (ctx.px_per_elmo_x(), ctx.px_per_elmo_y());
    let mut worst = 0.0f64;
    for k in strokes.iter().filter(|k| k.mode.is_absolute()) {
        let x = (k.x * px).round().clamp(0.0, (field.res - 1) as f64) as usize;
        let y = (k.z * py).round().clamp(0.0, (field.res - 1) as f64) as usize;
        let here = field.get(y * field.res + x) * range;
        worst = worst.max((here - k.seat).abs());
    }
    worst
}

/// The half of a 5×5 chamfer stencil that a raster sweep has already written.
///
/// Three-by-three would be cheaper and is the usual choice, but its metric
/// overstates the knight-move directions by 8%, and an 8% error on a grade
/// limit is the difference between a map that builds and one that nearly
/// does. The 5×5 offsets bring the worst direction inside 2%, which is under
/// the lattice's own quantisation.
const CONE_STENCIL: [(isize, isize); 8] = [
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-2, -1),
    (2, -1),
    (-1, -2),
    (1, -2),
];

/// The lower envelope of cones of a fixed slope, one placed on every sample.
///
/// `out(p) = min over q of h(q) + t·d(p, q)`, which is the highest surface
/// under `h` whose slope nowhere exceeds `t`. Two chamfer sweeps compute it in
/// one pass each rather than the O(n²) the definition suggests: the forward
/// sweep propagates every cone downhill through the samples it has already
/// written, the backward sweep closes the other half.
///
/// `ex`/`ey` are elmos per lattice step on each axis, so the cone is round in
/// the world rather than in the domain — on a 16×8 map those differ by two.
///
/// Sequential on purpose. Each sweep reads what it has just written, exactly
/// like erosion, so `par_rows` does not apply.
fn cone_cap(h: &[f64], r: usize, ex: f64, ey: f64, t: f64) -> Vec<f64> {
    let w: Vec<f64> = CONE_STENCIL
        .iter()
        .map(|(dx, dy)| {
            let (a, b) = (*dx as f64 * ex, *dy as f64 * ey);
            t * (a * a + b * b).sqrt()
        })
        .collect();
    let last = r as isize - 1;
    let mut o = h.to_vec();
    let sweep = |o: &mut Vec<f64>, back: bool| {
        for k in 0..r * r {
            let k = if back { r * r - 1 - k } else { k };
            let (x, y) = ((k % r) as isize, (k / r) as isize);
            let mut v = o[k];
            for (n, (dx, dy)) in CONE_STENCIL.iter().enumerate() {
                let (nx, ny) = if back {
                    (x - dx, y - dy)
                } else {
                    (x + dx, y + dy)
                };
                if nx < 0 || ny < 0 || nx > last || ny > last {
                    continue;
                }
                v = v.min(o[ny as usize * r + nx as usize] + w[n]);
            }
            o[k] = v;
        }
    };
    sweep(&mut o, false);
    sweep(&mut o, true);
    o
}

/// One fifth of a turn, as a rotation the lattice cannot express.
///
/// Every other symmetry here maps a sample onto another *sample* — mirrors and
/// quarter turns are permutations of the index, exact and free. A fifth of a
/// turn is not: 72° carries lattice points to places where no lattice point
/// is, so the source has to be interpolated. That is the whole difference, and
/// it is why this mode is the only one that reads the field bilinearly.
///
/// The sector is found by rotating rather than by measuring an angle. Testing
/// "is this vector inside the wedge" is two cross products — pure arithmetic,
/// no `atan2`, nothing to disagree about across platforms — and at most four
/// rotations bring any point home.
fn rot72_source(f: &SharedField, r: usize, x: usize, y: usize) -> f64 {
    let c = (r - 1) as f64 / 2.0;
    let (mut vx, mut vy) = (x as f64 - c, y as f64 - c);
    // cos/sin of ±36° and 72°, once.
    const D36: f64 = 36.0 * std::f64::consts::PI / 180.0;
    const D72: f64 = 72.0 * std::f64::consts::PI / 180.0;
    let (c36, s36) = (fdlibm::cos(D36), fdlibm::sin(D36));
    let (c72, s72) = (fdlibm::cos(D72), fdlibm::sin(D72));
    // The wedge runs from -36° to +36° about the +x axis.
    let inside = |vx: f64, vy: f64| {
        // cross(lo, v) >= 0 and cross(v, hi) >= 0, with lo = (c36, -s36) and
        // hi = (c36, s36).
        (c36 * vy - (-s36) * vx) >= 0.0 && (vx * s36 - vy * c36) >= 0.0
    };
    for _ in 0..4 {
        if inside(vx, vy) {
            break;
        }
        // Rotate by -72°.
        let (nx, ny) = (vx * c72 + vy * s72, -vx * s72 + vy * c72);
        vx = nx;
        vy = ny;
    }
    sample_bilinear(
        f,
        (vx + c).clamp(0.0, (r - 1) as f64),
        (vy + c).clamp(0.0, (r - 1) as f64),
    )
}

#[cfg(test)]
mod tests {
    use crate::graph::{registry, Graph, PVal, Stroke, StrokeMode};
    use crate::nodes::stroke_drift;
    use crate::project::{Context, Project};

    /// A step between two plateaus, which is the situation a ramp exists for:
    /// a cliff units cannot climb, and a route that lets them.
    fn stepped(res: usize) -> Graph {
        let mut g = Graph::new();
        // Angle 0 runs the gradient along X; terraced into two flat levels
        // with a sheer join between them.
        let grad = g.add("gradient", 0.0, 0.0, &[("angle", PVal::Num(0.0))]);
        let step = g.add(
            "terrace",
            0.0,
            0.0,
            &[("steps", PVal::Num(2.0)), ("sharpness", PVal::Num(1.0))],
        );
        g.link(&grad, &step, "In");
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&step, &out, "In");
        let _ = res;
        g
    }

    /// Terrain on a non-square map has to come out the same size on the
    /// ground along both axes, not the same size on the lattice.
    ///
    /// The lattice is square and the world need not be, so a 16×8 map packs
    /// twice as many elmos into a column of X as into a column of Z. A
    /// generator that used one frequency for both drew features stretched 2:1
    /// along X — legal, and wrong.
    ///
    /// Measured as mean-crossings per elmo rather than by eye: a feature of a
    /// given size crosses the mean a given number of times per unit of ground,
    /// whichever direction you walk.
    #[test]
    fn a_feature_is_the_same_size_along_both_axes_of_a_rectangular_map() {
        const R: usize = 257;
        let mut g = Graph::new();
        let n = g.add(
            "noise",
            0.0,
            0.0,
            &[("feature", PVal::Num(1024.0)), ("octaves", PVal::Num(1.0))],
        );
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&n, &out, "In");

        // Crossings of the field's own mean, per elmo, along each axis.
        let density = |units_x: u32, units_y: u32| -> (f64, f64) {
            let project = Project {
                units_x,
                units_y,
                ..Default::default()
            };
            let ctx = Context::new(&project, R);
            let f = crate::field::as_gray(&g.evaluate(&n, &ctx));
            let mean = (0..f.len()).map(|i| f.get(i)).sum::<f64>() / f.len() as f64;
            let mut cx = 0usize;
            let mut cy = 0usize;
            for y in 0..R {
                for x in 1..R {
                    if (f.at(x - 1, y) >= mean) != (f.at(x, y) >= mean) {
                        cx += 1;
                    }
                }
            }
            for x in 0..R {
                for y in 1..R {
                    if (f.at(x, y - 1) >= mean) != (f.at(x, y) >= mean) {
                        cy += 1;
                    }
                }
            }
            // Per elmo of ground walked, not per lattice step taken.
            (
                cx as f64 / (R as f64 * ctx.elmos_x),
                cy as f64 / (R as f64 * ctx.elmos_y),
            )
        };

        for (ux, uy) in [(16, 8), (8, 16), (12, 12)] {
            let (dx, dy) = density(ux, uy);
            let ratio = dx / dy;
            assert!(
                (0.8..1.25).contains(&ratio),
                "{ux}x{uy}: {dx:.5} mean-crossings per elmo along X against {dy:.5} along Z \
                 (ratio {ratio:.2}) — features are stretched"
            );
        }
    }

    fn sculpted(res: usize, strokes: Vec<Stroke>, sym: &str) -> crate::field::SharedField {
        let project = Project {
            units_x: 12,
            units_y: 12,
            ..Default::default()
        };
        let ctx = Context::new(&project, res);
        let mut g = Graph::new();
        // A flat half-height plate, so anything that is not flat afterwards is
        // the brush.
        let base = g.add("constant", 0.0, 0.0, &[("value", PVal::Num(0.5))]);
        let sc = g.add(
            "sculpt",
            0.0,
            0.0,
            &[
                ("strokes", PVal::Strokes(strokes)),
                ("symmetry", PVal::Str(sym.into())),
            ],
        );
        g.link(&base, &sc, "In");
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&sc, &out, "In");
        crate::field::as_gray(&g.evaluate(&out, &ctx))
    }

    /// The reason a stroke is a node parameter in elmos rather than a painted
    /// raster: it has to mean the same thing at every resolution the graph is
    /// ever evaluated at.
    ///
    /// A painted layer is locked to the resolution it was painted at, so the
    /// preview and the bake would disagree about the shape of a hand edit —
    /// which is the failure this codebase has already shipped twice by other
    /// routes.
    #[test]
    fn a_stroke_is_the_same_hill_at_every_resolution() {
        let stroke = Stroke {
            x: 3072.0,
            z: 3072.0,
            radius: 900.0,
            strength: 200.0,
            mode: StrokeMode::Raise,
            seat: 0.0,
        };
        // Peak height and the radius at which the brush has died away, both in
        // elmos, read off the field at three resolutions.
        let probe = |res: usize| -> (f64, f64) {
            let f = sculpted(res, vec![stroke], "none");
            let project = Project {
                units_x: 12,
                units_y: 12,
                ..Default::default()
            };
            let ctx = Context::new(&project, res);
            let range = ctx.height_range;
            let mid = res / 2;
            let peak = (f.at(mid, mid) - 0.5) * range;
            // Walk east until the brush stops showing.
            let mut edge = 0.0;
            for x in mid..res {
                if (f.at(x, mid) - 0.5) * range > 0.5 {
                    edge = (x - mid) as f64 * ctx.elmo_per_px_x();
                }
            }
            (peak, edge)
        };
        let (p65, e65) = probe(65);
        let (p257, e257) = probe(257);
        let (p513, e513) = probe(513);
        for (a, b, what) in [
            (p65, p257, "peak"),
            (p257, p513, "peak"),
            (e65, e257, "reach"),
            (e257, e513, "reach"),
        ] {
            assert!(
                (a - b).abs() <= a.abs().max(b.abs()) * 0.06 + 24.0,
                "{what} differs across resolutions: {a:.1} against {b:.1}"
            );
        }
        assert!(
            (p513 - 200.0).abs() < 12.0,
            "a 200 elmo stroke raised the ground {p513:.1}"
        );
    }

    /// A hand edit moves the whole symmetry group. Editing one side without
    /// its images is how a map goes quietly unfair, and the metal side has
    /// `zk::move_group` for exactly this reason.
    #[test]
    fn a_mirrored_stroke_lands_on_both_sides() {
        const R: usize = 257;
        let stroke = Stroke {
            x: 1200.0,
            z: 3072.0,
            radius: 700.0,
            strength: 300.0,
            mode: StrokeMode::Raise,
            seat: 0.0,
        };
        let plain = sculpted(R, vec![stroke], "none");
        let mirrored = sculpted(R, vec![stroke], "mirrorX");
        let mid = R / 2;
        // Where the stroke was put, both agree.
        let near = 1200.0 / 6144.0 * (R - 1) as f64;
        let nx = near.round() as usize;
        assert!(plain.at(nx, mid) > 0.51, "the stroke itself is missing");
        assert!((plain.at(nx, mid) - mirrored.at(nx, mid)).abs() < 1e-6);
        // On the far side, only the mirrored one has anything.
        let fx = R - 1 - nx;
        assert!(
            (plain.at(fx, mid) - 0.5).abs() < 1e-6,
            "unmirrored sculpt touched the far side"
        );
        assert!(
            mirrored.at(fx, mid) > 0.51,
            "mirrored sculpt left the far side untouched — the map is now unfair"
        );
        assert!(
            (mirrored.at(nx, mid) - mirrored.at(fx, mid)).abs() < 1e-3,
            "the two sides got different amounts"
        );
    }

    /// An absolute stroke means something about the ground it was drawn on, so
    /// changing that ground makes it stale — and the tool has to be able to
    /// say so instead of levelling a hilltop to a valley's altitude.
    #[test]
    fn levelling_strokes_go_stale_when_the_ground_moves_and_raising_ones_do_not() {
        const R: usize = 129;
        let project = Project {
            units_x: 12,
            units_y: 12,
            ..Default::default()
        };
        let ctx = Context::new(&project, R);
        let flat = crate::field::as_gray(&{
            let mut g = Graph::new();
            let c = g.add("constant", 0.0, 0.0, &[("value", PVal::Num(0.5))]);
            let o = g.add("out_height", 0.0, 0.0, &[]);
            g.link(&c, &o, "In");
            g.evaluate(&o, &ctx)
        });
        let seat = 0.5 * ctx.height_range;
        let level = Stroke {
            x: 3072.0,
            z: 3072.0,
            radius: 600.0,
            strength: 1.0,
            mode: StrokeMode::Level,
            seat,
        };
        let raise = Stroke {
            mode: StrokeMode::Raise,
            strength: 100.0,
            ..level
        };
        // Drawn on the ground it was drawn on: no drift.
        assert!(stroke_drift(&[level], &flat, &ctx) < 1.0);
        // The same stroke over ground that has moved a long way.
        let moved = crate::field::as_gray(&{
            let mut g = Graph::new();
            let c = g.add("constant", 0.0, 0.0, &[("value", PVal::Num(0.9))]);
            let o = g.add("out_height", 0.0, 0.0, &[]);
            g.link(&c, &o, "In");
            g.evaluate(&o, &ctx)
        });
        let drift = stroke_drift(&[level], &moved, &ctx);
        assert!(
            drift > 0.35 * ctx.height_range,
            "a levelling stroke on ground that moved 40% of the range reported {drift:.0} elmos of drift"
        );
        // A relative stroke is never stale, whatever happens upstream.
        assert!(
            stroke_drift(&[raise], &moved, &ctx) == 0.0,
            "a raise stroke cannot go stale — it does not claim anything about the ground"
        );
    }

    #[test]
    fn a_ramp_grades_between_its_waypoints() {
        const R: usize = 129;
        let project = Project {
            units_x: 8,
            units_y: 8,
            ..Default::default()
        };
        let ctx = Context::new(&project, R);
        let mut g = stepped(R);
        let step = g
            .nodes
            .iter()
            .find(|n| n.type_name == "terrace")
            .unwrap()
            .id
            .clone();
        let out = g.find_terminal("height").unwrap().to_string();

        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));

        // A route straight across the step, west to east at mid depth.
        let ramp = g.add(
            "ramp",
            0.0,
            0.0,
            &[
                (
                    "points",
                    PVal::Points(vec![[600.0, 2048.0], [3496.0, 2048.0]]),
                ),
                ("width", PVal::Num(400.0)),
                ("falloff", PVal::Num(200.0)),
            ],
        );
        g.link(&step, &ramp, "In");
        g.link(&ramp, &out, "In");
        let after = crate::field::as_gray(&g.evaluate(&out, &ctx));

        let px = |e: f64| (e * ctx.px_per_elmo_x()).round() as usize;
        let row = R / 2;
        let sample = |f: &crate::field::SharedField, x: usize| f.at(x, row);

        // The step is gone from the corridor: the biggest one-sample rise
        // along the route has to be far smaller than it was.
        let worst = |f: &crate::field::SharedField| {
            let mut w: f64 = 0.0;
            for x in px(600.0)..px(3496.0) {
                w = w.max((sample(f, x + 1) - sample(f, x)).abs());
            }
            w
        };
        let (was, now) = (worst(&before), worst(&after));
        assert!(
            now < was * 0.25,
            "the ramp barely graded anything: worst step {was:.4} -> {now:.4}"
        );

        // And it meets the terrain at both ends rather than floating.
        for e in [600.0, 3496.0] {
            let d = (sample(&after, px(e)) - sample(&before, px(e))).abs();
            assert!(
                d < 0.02,
                "the ramp does not meet the ground at {e}: off by {d:.4}"
            );
        }
    }

    #[test]
    fn a_ramp_leaves_the_rest_of_the_map_alone() {
        const R: usize = 129;
        let project = Project::default();
        let ctx = Context::new(&project, R);
        let mut g = stepped(R);
        let step = g
            .nodes
            .iter()
            .find(|n| n.type_name == "terrace")
            .unwrap()
            .id
            .clone();
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));

        let ramp = g.add(
            "ramp",
            0.0,
            0.0,
            &[
                (
                    "points",
                    PVal::Points(vec![[1000.0, 1000.0], [2000.0, 1000.0]]),
                ),
                ("width", PVal::Num(200.0)),
                ("falloff", PVal::Num(100.0)),
            ],
        );
        g.link(&step, &ramp, "In");
        g.link(&ramp, &out, "In");
        let after = crate::field::as_gray(&g.evaluate(&out, &ctx));

        // Well clear of the corridor, nothing moved at all.
        let far = (R * 3) / 4;
        for x in 0..R {
            assert_eq!(
                before.at(x, far).to_bits(),
                after.at(x, far).to_bits(),
                "the ramp reached a row it should not have, at x={x}"
            );
        }
    }

    #[test]
    fn a_ramp_with_fewer_than_two_points_is_a_no_op() {
        // Half-placed routes are the normal state while you are placing one.
        const R: usize = 65;
        let ctx = Context::new(&Project::default(), R);
        let mut g = stepped(R);
        let step = g
            .nodes
            .iter()
            .find(|n| n.type_name == "terrace")
            .unwrap()
            .id
            .clone();
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));
        for pts in [vec![], vec![[1000.0, 1000.0]]] {
            let ramp = g.add("ramp", 0.0, 0.0, &[("points", PVal::Points(pts))]);
            g.link(&step, &ramp, "In");
            g.link(&ramp, &out, "In");
            let after = crate::field::as_gray(&g.evaluate(&out, &ctx));
            assert_eq!(before.data, after.data);
        }
    }

    #[test]
    fn cut_only_lowers_and_fill_only_raises() {
        const R: usize = 129;
        let ctx = Context::new(&Project::default(), R);
        let mut g = stepped(R);
        let step = g
            .nodes
            .iter()
            .find(|n| n.type_name == "terrace")
            .unwrap()
            .id
            .clone();
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));

        for (mode, name) in [("cut", "cut"), ("fill", "fill")] {
            let ramp = g.add(
                "ramp",
                0.0,
                0.0,
                &[
                    (
                        "points",
                        PVal::Points(vec![[600.0, 3072.0], [5500.0, 3072.0]]),
                    ),
                    ("width", PVal::Num(500.0)),
                    ("mode", PVal::Str(mode.into())),
                ],
            );
            g.link(&step, &ramp, "In");
            g.link(&ramp, &out, "In");
            let after = crate::field::as_gray(&g.evaluate(&out, &ctx));
            let mut moved = 0;
            for i in 0..before.len() {
                let d = after.get(i) - before.get(i);
                if d.abs() > 1e-9 {
                    moved += 1;
                    match name {
                        "cut" => assert!(d < 0.0, "cut raised ground by {d}"),
                        _ => assert!(d > 0.0, "fill lowered ground by {d}"),
                    }
                }
            }
            assert!(moved > 0, "{name} did nothing at all");
        }
    }

    /// Rough ground with peaks and hollows both, which is what a grade limit
    /// is for and what a `blur` cannot promise anything about.
    fn rough(res: usize) -> Graph {
        let mut g = Graph::new();
        let n = g.add(
            "noise",
            0.0,
            0.0,
            &[
                ("scale", PVal::Num(1400.0)),
                ("octaves", PVal::Num(5.0)),
                ("gain", PVal::Num(0.6)),
            ],
        );
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&n, &out, "In");
        let _ = res;
        g
    }

    fn worst_slope(f: &crate::field::SharedField, ctx: &Context) -> f64 {
        let r = f.res;
        let denom = (r - 1).max(1) as f64;
        let (hx, hy) = (ctx.elmos_x / denom, ctx.elmos_y / denom);
        let mut w: f64 = 0.0;
        for y in 0..r {
            for x in 0..r {
                w = w.max(crate::nodes::slope_degrees_aniso(
                    f,
                    x,
                    y,
                    r,
                    ctx.height_range,
                    hx,
                    hy,
                ));
            }
        }
        w
    }

    /// The steepest single step between neighbouring samples, in degrees.
    /// This is the quantity a grade limit actually bounds.
    fn worst_step(f: &crate::field::SharedField, ctx: &Context) -> f64 {
        let r = f.res;
        let e = ctx.elmo_per_px_x();
        let mut w: f64 = 0.0;
        for y in 0..r {
            for x in 0..r {
                if x + 1 < r {
                    w = w.max((f.at(x + 1, y) - f.at(x, y)).abs());
                }
                if y + 1 < r {
                    w = w.max((f.at(x, y + 1) - f.at(x, y)).abs());
                }
            }
        }
        crate::fdlibm::atan(w * ctx.height_range / e) * 180.0 / std::f64::consts::PI
    }

    fn graded(g: &mut Graph, ctx: &Context, params: &[(&str, PVal)]) -> crate::field::SharedField {
        let src = g
            .nodes
            .iter()
            .find(|n| n.type_name == "noise")
            .unwrap()
            .id
            .clone();
        let out = g.find_terminal("height").unwrap().to_string();
        let node = g.add("grade", 0.0, 0.0, params);
        g.link(&src, &node, "In");
        g.link(&node, &out, "In");
        crate::field::as_gray(&g.evaluate(&out, ctx))
    }

    #[test]
    fn a_grade_limit_actually_limits_the_grade() {
        const R: usize = 129;
        let ctx = Context::new(&Project::default(), R);
        let mut g = rough(R);
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));
        let was = worst_slope(&before, &ctx);
        let after = graded(&mut g, &ctx, &[("grade", PVal::Num(10.0))]);
        assert!(
            was > 20.0,
            "the test terrain is not steep enough: {was:.1}°"
        );

        // The promise, exactly: no step between neighbouring samples is
        // steeper than the grade.
        // The tolerance is the field's own f32 storage, not slack in the cone.
        let step = worst_step(&after, &ctx);
        assert!(
            step <= 10.0 + 1e-3,
            "asked for 10°, steepest step {step:.6}°"
        );

        // And the qualification, measured rather than hand-waved: creases —
        // where two cones meet and the two axis readings add — read higher
        // than the grade under a gradient-magnitude probe. A quarter over is
        // the documented tolerance; more than that is a broken cone.
        let now = worst_slope(&after, &ctx);
        assert!(
            now <= 12.5,
            "crease overshoot beyond the documented quarter: {now:.2}° for a 10° grade"
        );
        assert!(
            now < was * 0.6,
            "grading barely helped: {was:.1}° -> {now:.1}°"
        );
    }

    #[test]
    fn grading_lowers_peaks_and_raises_hollows() {
        const R: usize = 129;
        let ctx = Context::new(&Project::default(), R);
        let mut g = rough(R);
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));
        let after = graded(&mut g, &ctx, &[("grade", PVal::Num(8.0))]);
        let (mut down, mut up) = (0, 0);
        for i in 0..before.len() {
            let d = after.get(i) - before.get(i);
            if d < -1e-9 {
                down += 1;
            } else if d > 1e-9 {
                up += 1;
            }
        }
        assert!(down > 0 && up > 0, "both must happen: {down} down, {up} up");
    }

    #[test]
    fn grade_cut_only_lowers_and_fill_only_raises() {
        const R: usize = 65;
        let ctx = Context::new(&Project::default(), R);
        for (mode, sign) in [("cut", -1.0), ("fill", 1.0)] {
            let mut g = rough(R);
            let out = g.find_terminal("height").unwrap().to_string();
            let before = crate::field::as_gray(&g.evaluate(&out, &ctx));
            let after = graded(
                &mut g,
                &ctx,
                &[("grade", PVal::Num(8.0)), ("mode", PVal::Str(mode.into()))],
            );
            let mut moved = 0;
            for i in 0..before.len() {
                let d = after.get(i) - before.get(i);
                if d.abs() > 1e-9 {
                    moved += 1;
                    assert!(d * sign > 0.0, "{mode} moved ground the wrong way by {d}");
                }
            }
            assert!(moved > 0, "{mode} did nothing at all");
        }
    }

    #[test]
    fn grading_makes_a_map_buildable() {
        // The whole point of the node, stated as the number the inspector
        // shows: a rough map you cannot put a factory on becomes one you can.
        const R: usize = 129;
        let ctx = Context::new(&Project::default(), R);
        let mut g = rough(R);
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));
        let after = graded(&mut g, &ctx, &[("grade", PVal::Num(9.0))]);
        let of_land = |f: &crate::field::SharedField| {
            crate::analysis::flatness(f, &ctx, 96.0, 12.0, -1.0).buildable_of_land
        };
        let (was, now) = (of_land(&before), of_land(&after));
        assert!(
            now > 0.98 && now > was + 0.2,
            "grading should make the map build nearly everywhere: {:.1}% -> {:.1}%",
            was * 100.0,
            now * 100.0
        );
    }

    #[test]
    fn a_grade_of_zero_amount_is_a_no_op() {
        const R: usize = 65;
        let ctx = Context::new(&Project::default(), R);
        let mut g = rough(R);
        let out = g.find_terminal("height").unwrap().to_string();
        let before = crate::field::as_gray(&g.evaluate(&out, &ctx));
        let after = graded(
            &mut g,
            &ctx,
            &[("grade", PVal::Num(5.0)), ("amount", PVal::Num(0.0))],
        );
        assert_eq!(before.data, after.data);
    }

    #[test]
    fn imported_terrain_arrives_as_ordinary_terrain() {
        use crate::raster::Rasters;
        // A ramp of known values, imported and then read at three different
        // resolutions: the node's whole job is to make a raster behave like
        // any other generator, at whatever resolution it is asked for.
        const SRC: usize = 65;
        let mut src = crate::field::Field::gray(SRC);
        for y in 0..SRC {
            for x in 0..SRC {
                src.set(y * SRC + x, x as f64 / (SRC - 1) as f64);
            }
        }
        let mut rasters = Rasters::new();
        rasters.insert(Rasters::TERRAIN, std::sync::Arc::new(src));
        let rasters = std::sync::Arc::new(rasters);

        let mut g = Graph::new();
        let n = g.add("import", 0.0, 0.0, &[]);
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&n, &out, "In");
        let project = Project::default();

        for res in [65, 129, 33] {
            let ctx = Context::with_rasters(&project, res, rasters.clone());
            let f = crate::field::as_gray(&g.evaluate(&out, &ctx));
            assert_eq!(f.res, res);
            // The gradient survives: left edge 0, right edge 1, monotone.
            assert!(f.at(0, res / 2).abs() < 1e-6, "left edge at {res}");
            assert!(
                (f.at(res - 1, res / 2) - 1.0).abs() < 1e-6,
                "right edge at {res}"
            );
            for x in 1..res {
                assert!(
                    f.at(x, res / 2) >= f.at(x - 1, res / 2),
                    "not monotone at {res}, x={x}"
                );
            }
        }
    }

    #[test]
    fn an_import_with_no_raster_is_flat_rather_than_a_panic() {
        // The bake refuses this by name; the node itself must not fall over,
        // because the app evaluates graphs while a project is half-loaded.
        let mut g = Graph::new();
        let n = g.add("import", 0.0, 0.0, &[("name", PVal::Str("absent".into()))]);
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&n, &out, "In");
        let ctx = Context::new(&Project::default(), 33);
        let f = crate::field::as_gray(&g.evaluate(&out, &ctx));
        assert!(f.data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn five_fold_symmetry_repeats_every_seventy_two_degrees() {
        // The property, sampled where it can be checked: a point and its four
        // rotations about the centre must carry the same terrain. Bilinear
        // interpolation means "the same" is not bit-identical here — a fifth
        // of a turn does not map lattice points onto lattice points, which is
        // the whole reason this mode interpolates at all — so the bound is a
        // tolerance rather than an equality, and it is tight.
        const R: usize = 257;
        let project = Project::default();
        let ctx = Context::new(&project, R);
        let mut g = Graph::new();
        let n = g.add(
            "noise",
            0.0,
            0.0,
            &[("feature", PVal::Num(1800.0)), ("octaves", PVal::Num(5.0))],
        );
        let sym = g.add("symmetry", 0.0, 0.0, &[("mode", PVal::Str("rot72".into()))]);
        g.link(&n, &sym, "In");
        let out = g.add("out_height", 0.0, 0.0, &[]);
        g.link(&sym, &out, "In");
        let f = crate::field::as_gray(&g.evaluate(&out, &ctx));

        let c = (R - 1) as f64 / 2.0;
        let mut diffs: Vec<f64> = Vec::new();
        for ring in [30.0_f64, 60.0, 100.0] {
            for step in 0..60 {
                let a = step as f64 * std::f64::consts::TAU / 60.0;
                let here =
                    crate::field::sample_bilinear(&f, c + ring * a.cos(), c + ring * a.sin());
                for k in 1..5 {
                    let b = a + k as f64 * std::f64::consts::TAU / 5.0;
                    let there =
                        crate::field::sample_bilinear(&f, c + ring * b.cos(), c + ring * b.sin());
                    diffs.push((here - there).abs());
                }
            }
        }
        diffs.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let p95 = diffs[diffs.len() * 95 / 100];
        // The 95th percentile rather than the maximum, and the reason is
        // structural: the fold has one real discontinuity, along the boundary
        // of the fundamental wedge, where sector four meets sector zero. The
        // *field* is five-fold symmetric there too, but reconstructing it
        // bilinearly mixes across the jump, and the mixing is not itself
        // rotationally consistent. That is the same seam the `ffa5` starter
        // blurs before eroding. Everywhere else the copies agree closely, and
        // a fold that was actually wrong would miss by this much everywhere,
        // not in one twentieth of the samples.
        assert!(
            p95 < 0.02,
            "five-fold copies disagree by {p95:.4} at the 95th percentile (max {:.4})",
            diffs[diffs.len() - 1]
        );
    }

    #[test]
    fn the_registry_lists_the_ramp_under_layout() {
        let spec = registry().get("ramp").expect("ramp is registered");
        assert_eq!(spec.cat, "Layout");
        assert!(spec.inputs.contains(&"Mask"), "a ramp should be maskable");
        let spec = registry().get("grade").expect("grade is registered");
        assert_eq!(spec.cat, "Layout");
        assert!(spec.inputs.contains(&"Mask"), "a grade should be maskable");
    }
}

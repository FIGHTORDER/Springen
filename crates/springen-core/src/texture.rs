//! Texturing nodes and the remaining SMF output carve-outs.

use std::sync::Arc;

use crate::fdlibm;
use crate::field::{clamp01, hex_to_rgb, Field};
use crate::graph::{
    p_bool, p_color, p_elmos, p_enum, p_float, p_int, p_text, Chan, NodeSpec, RegistryBuilder,
};
use crate::nodes::{inp, spec};
use crate::noise::fbm;
use crate::ramps::{adjust_hsv, ramp_at, ramp_stops};

const RAMP_NAMES: &[&str] = &["verdant", "arid", "arctic", "volcanic", "rust", "slate"];

fn colour(mut s: NodeSpec, in_types: &'static [(&'static str, Chan)]) -> NodeSpec {
    s.produces = Chan::Color;
    s.in_types = in_types;
    s
}

pub fn register(b: &mut RegistryBuilder) {
    b.def(colour(
        spec(
            "tex_ramp",
            "Height ramp",
            "Texture",
            &["In"],
            vec![
                p_enum("ramp", "Palette", "verdant", RAMP_NAMES),
                p_float("low", "Range low", 0.0, 0.0, 1.0),
                p_float("high", "Range high", 1.0, 0.0, 1.0),
                p_float("hue", "Hue shift", 0.0, -0.5, 0.5),
                p_float("sat", "Saturation", 1.0, 0.0, 2.5),
                p_float("val", "Brightness", 1.0, 0.2, 2.0),
            ],
            |ins, p, ctx| {
                let mut out = Field::new(ctx.res, 3);
                let Some(i_in) = inp(ins, "In") else {
                    return Arc::new(out);
                };
                let stops = ramp_stops(p.s("ramp"));
                let (low, high) = (p.f("low"), p.f("high"));
                let span = if high - low == 0.0 { 1e-6 } else { high - low };
                let (hue, sat, val) = (p.f("hue"), p.f("sat"), p.f("val"));
                let cache: Vec<[f64; 3]> = (0..=256)
                    .map(|q| adjust_hsv(ramp_at(stops, q as f64 / 256.0), hue, sat, val))
                    .collect();
                for i in 0..ctx.res * ctx.res {
                    let t = clamp01((i_in.get(i) - low) / span);
                    let col = cache[(t * 256.0).round() as usize];
                    out.set(i * 3, col[0]);
                    out.set(i * 3 + 1, col[1]);
                    out.set(i * 3 + 2, col[2]);
                }
                Arc::new(out)
            },
        ),
        &[],
    ));

    b.def(colour(
        spec(
            "tex_solid",
            "Solid colour",
            "Texture",
            &[],
            vec![p_color("color", "Colour", "#6E7A52")],
            |_ins, p, ctx| {
                let mut out = Field::new(ctx.res, 3);
                let c = hex_to_rgb(p.s("color"));
                for i in 0..ctx.res * ctx.res {
                    out.set(i * 3, c[0]);
                    out.set(i * 3 + 1, c[1]);
                    out.set(i * 3 + 2, c[2]);
                }
                Arc::new(out)
            },
        ),
        &[],
    ));

    b.def(colour(
        spec(
            "tex_blend",
            "Colour blend",
            "Texture",
            &["A", "B", "Mask"],
            vec![
                p_enum(
                    "mode",
                    "Mode",
                    "normal",
                    &[
                        "normal", "multiply", "screen", "overlay", "add", "darken", "lighten",
                    ],
                ),
                p_float("amount", "Amount", 1.0, 0.0, 1.0),
            ],
            |ins, p, ctx| {
                let mut out = Field::new(ctx.res, 3);
                let a_in = inp(ins, "A");
                let b_in = inp(ins, "B");
                let m_in = inp(ins, "Mask");
                if a_in.is_none() && b_in.is_none() {
                    return Arc::new(out);
                }
                let mode = p.s("mode").to_string();
                let amount = p.f("amount");
                for i in 0..ctx.res * ctx.res {
                    let t = amount * m_in.map(|f| clamp01(f.get(i))).unwrap_or(1.0);
                    for c in 0..3 {
                        let a = a_in.map(|f| f.get(i * 3 + c)).unwrap_or(0.0);
                        let bb = b_in.map(|f| f.get(i * 3 + c)).unwrap_or(0.0);
                        let v = match mode.as_str() {
                            "multiply" => a * bb,
                            "screen" => 1.0 - (1.0 - a) * (1.0 - bb),
                            "overlay" => {
                                if a < 0.5 {
                                    2.0 * a * bb
                                } else {
                                    1.0 - 2.0 * (1.0 - a) * (1.0 - bb)
                                }
                            }
                            "add" => a + bb,
                            "darken" => a.min(bb),
                            "lighten" => a.max(bb),
                            _ => bb,
                        };
                        out.set(i * 3 + c, a + (v - a) * t);
                    }
                }
                Arc::new(out)
            },
        ),
        &[("A", Chan::Color), ("B", Chan::Color), ("Mask", Chan::Gray)],
    ));

    b.def(colour(
        spec(
            "tex_detail",
            "Detail breakup",
            "Texture",
            &["In", "Mask"],
            vec![
                p_elmos("feature", "Feature size", 160.0, 8.0, 6000.0),
                p_int("octaves", "Octaves", 3.0, 1.0, 8.0),
                p_float("amount", "Amount", 0.14, 0.0, 1.0),
                p_float("hueVary", "Hue variation", 0.01, 0.0, 0.2),
                p_int("seed", "Seed offset", 23.0, 0.0, 9999.0),
            ],
            |ins, p, ctx| {
                let r = ctx.res;
                let mut out = Field::new(r, 3);
                let Some(i_in) = inp(ins, "In") else {
                    return Arc::new(out);
                };
                let freq = ctx.elmos / p.f("feature").max(1.0);
                // A world length, so Z fits more of them in on a map that is
                // longer in X. Exactly `freq` when the world is square.
                let freq_y = freq * ctx.aspect_y();
                let s = f64::from(ctx.seed) + p.f("seed") * 7919.0;
                let m_in = inp(ins, "Mask");
                let oct = p.i("octaves");
                let amount = p.f("amount");
                let hue_vary = p.f("hueVary");
                let rm1 = (r - 1) as f64;
                out.par_rows(|y, row| {
                    for x in 0..r {
                        let i = y * r + x;
                        let nv = fbm(
                            (x as f64 / rm1) * freq,
                            (y as f64 / rm1) * freq_y,
                            s,
                            oct,
                            0.5,
                            2.0,
                            false,
                        );
                        let amt = amount * m_in.map(|f| clamp01(f.get(i))).unwrap_or(1.0);
                        let base = [i_in.get(i * 3), i_in.get(i * 3 + 1), i_in.get(i * 3 + 2)];
                        let mut lit = [
                            clamp01(base[0] * (1.0 + nv * amt)),
                            clamp01(base[1] * (1.0 + nv * amt)),
                            clamp01(base[2] * (1.0 + nv * amt)),
                        ];
                        if hue_vary > 0.0 {
                            lit = adjust_hsv(lit, nv * hue_vary, 1.0, 1.0);
                        }
                        row[x * 3] = lit[0] as f32;
                        row[x * 3 + 1] = lit[1] as f32;
                        row[x * 3 + 2] = lit[2] as f32;
                    }
                });
                Arc::new(out)
            },
        ),
        &[("In", Chan::Color), ("Mask", Chan::Gray)],
    ));

    b.def(colour(
        spec(
            "tex_slope",
            "Slope material",
            "Texture",
            &["Colour", "Height", "Rock"],
            vec![
                p_float("minDeg", "Rock from", 26.0, 0.0, 89.0),
                p_float("maxDeg", "Full rock at", 46.0, 1.0, 90.0),
                p_color("rockColor", "Rock colour", "#6B6259"),
            ],
            |ins, p, ctx| {
                let r = ctx.res;
                let out_empty = Field::new(r, 3);
                let (c_in, h_in) = (inp(ins, "Colour"), inp(ins, "Height"));
                let (Some(c), Some(h)) = (c_in, h_in) else {
                    return c_in.map(Arc::clone).unwrap_or_else(|| Arc::new(out_empty));
                };
                let mut out = Field::new(r, 3);
                let rock = inp(ins, "Rock");
                let rc = hex_to_rgb(p.s("rockColor"));
                let (min_deg, max_deg) = (p.f("minDeg"), p.f("maxDeg"));
                let span = (max_deg - min_deg).max(0.001);
                let (hx, hy) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
                out.par_rows(|y, row| {
                    for x in 0..r {
                        let i = y * r + x;
                        // The real gradient: rock has to appear on the slopes
                        // the engine will call steep, not the lattice's.
                        let deg =
                            crate::nodes::slope_degrees_aniso(h, x, y, r, ctx.height_range, hx, hy);
                        let t = clamp01((deg - min_deg) / span);
                        for c2 in 0..3 {
                            let target = rock.map(|f| f.get(i * 3 + c2)).unwrap_or(rc[c2]);
                            let base = c.get(i * 3 + c2);
                            row[x * 3 + c2] = (base + (target - base) * t) as f32;
                        }
                    }
                });
                Arc::new(out)
            },
        ),
        &[
            ("Colour", Chan::Color),
            ("Height", Chan::Gray),
            ("Rock", Chan::Color),
        ],
    ));

    b.def(colour(
        spec(
            "tex_shade",
            "Bake lighting",
            "Texture",
            &["Colour", "Height"],
            vec![
                p_float("azimuth", "Sun azimuth", 315.0, 0.0, 360.0),
                p_float("elevation", "Sun elevation", 45.0, 5.0, 89.0),
                p_float("strength", "Shading", 0.45, 0.0, 1.0),
                p_float("ao", "Ambient occlusion", 0.3, 0.0, 1.0),
                p_elmos("aoRadius", "AO radius", 400.0, 32.0, 6000.0),
            ],
            |ins, p, ctx| {
                let r = ctx.res;
                let Some(c) = inp(ins, "Colour") else {
                    return Arc::new(Field::new(r, 3));
                };
                let Some(h) = inp(ins, "Height") else {
                    return Arc::clone(c);
                };
                let mut out = Field::new(r, 3);
                let vs = ctx.height_range;
                let (hsx, hsy) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
                let az = p.f("azimuth") * std::f64::consts::PI / 180.0;
                let el = p.f("elevation") * std::f64::consts::PI / 180.0;
                let lx = fdlibm::cos(az) * fdlibm::cos(el);
                let ly = fdlibm::sin(az) * fdlibm::cos(el);
                let lz = fdlibm::sin(el);
                let (strength, ao_amt) = (p.f("strength"), p.f("ao"));
                // AO proxy: height minus a wide blur of height, i.e. concavity.
                let ao = if ao_amt > 0.0 {
                    let rpx = |per: f64| {
                        (p.f("aoRadius") * per).round().min((r / 3) as f64).max(1.0) as usize
                    };
                    Some(crate::field::box_blur_xy(
                        &h.data,
                        r,
                        rpx(ctx.px_per_elmo_x()),
                        rpx(ctx.px_per_elmo_y()),
                    ))
                } else {
                    None
                };
                out.par_rows(|y, row| {
                    for x in 0..r {
                        let i = y * r + x;
                        let xl = if x > 0 { x - 1 } else { x };
                        let xr = if x < r - 1 { x + 1 } else { x };
                        let yu = if y > 0 { y - 1 } else { y };
                        let yd = if y < r - 1 { y + 1 } else { y };
                        let gx = (h.at(xr, y) - h.at(xl, y)) * vs / ((xr - xl) as f64 * hsx);
                        let gy = (h.at(x, yd) - h.at(x, yu)) * vs / ((yd - yu) as f64 * hsy);
                        let nl = (gx * gx + gy * gy + 1.0).sqrt();
                        let lam = (-gx * lx - gy * ly + lz) / nl;
                        let mut shade = 1.0 + strength * (clamp01(lam) * 2.0 - 1.0);
                        if let Some(ao) = &ao {
                            let conc = (h.get(i) - ao[i] as f64) * 6.0;
                            shade *= 1.0 + ao_amt * conc.clamp(-0.85, 0.25);
                        }
                        for c2 in 0..3 {
                            row[x * 3 + c2] = clamp01(c.get(i * 3 + c2) * shade) as f32;
                        }
                    }
                });
                Arc::new(out)
            },
        ),
        &[("Colour", Chan::Color), ("Height", Chan::Gray)],
    ));

    b.def(colour(
        spec(
            "tex_water",
            "Water tint",
            "Texture",
            &["Colour", "Height"],
            vec![
                // Negative means "wherever the engine will put the water".
                // A fixed value here is a copy of a project setting that
                // nothing keeps in sync: the default starter painted its
                // shoreline at 0.18 while the waterline was at 0.16, which is
                // ten elmos of beach the engine draws sea over. Graphs that
                // carry an explicit value still get exactly that value.
                p_float("sea", "Sea level (-1 = map waterline)", -1.0, -1.0, 0.95),
                p_elmos("seaOffset", "Shore offset", 0.0, -400.0, 400.0),
                p_color("shallow", "Shallow colour", "#2E6E7E"),
                p_color("deep", "Deep colour", "#0C2836"),
                p_float("depthScale", "Depth falloff", 1.0, 0.1, 4.0),
                p_float("shoreBlend", "Shore blend", 0.02, 0.0, 0.2),
            ],
            |ins, p, ctx| {
                let r = ctx.res;
                let Some(c) = inp(ins, "Colour") else {
                    return Arc::new(Field::new(r, 3));
                };
                let Some(h) = inp(ins, "Height") else {
                    return Arc::clone(c);
                };
                let mut out = Field::new(r, 3);
                let sh = hex_to_rgb(p.s("shallow"));
                let dp = hex_to_rgb(p.s("deep"));
                let set = p.f("sea");
                let offset = if ctx.height_range > 0.0 {
                    p.f("seaOffset") / ctx.height_range
                } else {
                    0.0
                };
                let sea = if set < 0.0 { ctx.sea_t + offset } else { set };
                let depth_scale = p.f("depthScale");
                let shore = p.f("shoreBlend");
                for i in 0..r * r {
                    let hv = h.get(i);
                    let below = (sea - hv) / sea.max(1e-6) * depth_scale;
                    let wet = clamp01((sea + shore - hv) / (shore * 2.0 + 1e-6).max(1e-6));
                    let t = clamp01(below);
                    for c2 in 0..3 {
                        let w = sh[c2] + (dp[c2] - sh[c2]) * t;
                        let base = c.get(i * 3 + c2);
                        out.set(i * 3 + c2, base + (w - base) * wet);
                    }
                }
                Arc::new(out)
            },
        ),
        &[("Colour", Chan::Color), ("Height", Chan::Gray)],
    ));

    /* -- outputs --------------------------------------------------------- */
    b.def(colour(
        spec(
            "import_color",
            "Imported diffuse",
            "Generate",
            &[],
            vec![p_text("name", "Raster", crate::raster::Rasters::DIFFUSE)],
            |_ins, p, ctx| {
                let r = ctx.res;
                let Some(src) = ctx.rasters.get(p.s("name")) else {
                    // Mid grey rather than black: a missing diffuse should
                    // look like a mistake, not like a night map.
                    let mut f = Field::new(r, 3);
                    for i in 0..f.len() {
                        f.set(i, 0.5);
                    }
                    return Arc::new(f);
                };
                if src.res == r && src.ch == 3 {
                    return src.clone();
                }
                let mut f = Field::new(r, 3);
                let last = (r - 1).max(1) as f64;
                let sl = (src.res - 1).max(1) as f64;
                let sch = src.ch;
                f.par_rows(|y, row| {
                    let sy = (y as f64 / last) * sl;
                    let mut px = vec![0.0f64; sch];
                    for x in 0..r {
                        let sx = (x as f64 / last) * sl;
                        // Bilinear: a photograph of ground is a continuous
                        // surface. It is emphatically not category data — the
                        // typemap next door must never be resampled this way.
                        crate::field::sample_color(src, sx, sy, &mut px);
                        for c in 0..3 {
                            row[x * 3 + c] = px[c.min(sch - 1)] as f32;
                        }
                    }
                });
                Arc::new(f)
            },
        ),
        &[],
    ));

    let mut out_diffuse = colour(
        spec(
            "out_diffuse",
            "Diffuse out",
            "Output",
            &["In"],
            vec![],
            |ins, _p, ctx| {
                inp(ins, "In")
                    .map(Arc::clone)
                    .unwrap_or_else(|| Arc::new(Field::new(ctx.res, 3)))
            },
        ),
        &[("In", Chan::Color)],
    );
    out_diffuse.output = Some("diffuse");
    b.def(out_diffuse);

    // Splat distribution is RGBA: `splats.texScales` and `texMults` are per
    // channel and there are four splat detail normals, not three. The
    // prototype packed RGB only; with the A port unwired the R, G and B
    // channels are identical to it, so golden parity is preserved.
    let mut out_splat = spec(
        "out_splat",
        "Splat distribution out",
        "Output",
        &["R", "G", "B", "A"],
        vec![p_bool("normalize", "Normalise weights", true)],
        |ins, p, ctx| {
            let r = ctx.res;
            let mut out = Field::new(r, 4);
            let normalize = p.b("normalize");
            for i in 0..r * r {
                let mut a = inp(ins, "R").map(|f| clamp01(f.get(i))).unwrap_or(0.0);
                let mut b = inp(ins, "G").map(|f| clamp01(f.get(i))).unwrap_or(0.0);
                let mut c = inp(ins, "B").map(|f| clamp01(f.get(i))).unwrap_or(0.0);
                let mut d = inp(ins, "A").map(|f| clamp01(f.get(i))).unwrap_or(0.0);
                if normalize {
                    let s = a + b + c + d;
                    if s > 1e-6 {
                        a /= s;
                        b /= s;
                        c /= s;
                        d /= s;
                    }
                }
                out.set(i * 4, a);
                out.set(i * 4 + 1, b);
                out.set(i * 4 + 2, c);
                out.set(i * 4 + 3, d);
            }
            Arc::new(out)
        },
    );
    out_splat.produces = Chan::Color;
    out_splat.output = Some("splat");
    b.def(out_splat);

    // Grass is an 8-bit info layer; grass grows where this is non-zero.
    let mut out_grass = spec(
        "out_grass",
        "Grass map out",
        "Output",
        &["In"],
        vec![p_float("threshold", "Threshold", 0.5, 0.0, 1.0)],
        |ins, p, ctx| {
            let mut out = Field::gray(ctx.res);
            if let Some(i_in) = inp(ins, "In") {
                let threshold = p.f("threshold");
                for i in 0..out.len() {
                    out.set(i, if i_in.get(i) >= threshold { 1.0 } else { 0.0 });
                }
            }
            Arc::new(out)
        },
    );
    out_grass.output = Some("grass");
    b.def(out_grass);
}

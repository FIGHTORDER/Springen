// Texel buffers are written by linear index; iterator adapters obscure the
// stride the PNG encoder expects.
#![allow(clippy::needless_range_loop)]
//! Surface materials for the SSMF detail slots, drawn and photographed.
//!
//! Spring repeats small power-of-two tiles across the whole map — the reference
//! map's `detailTex` is 512², and at `texScales` 0.006 it repeats about thirty
//! times across a 12×12 map. Everything here follows from that one fact.
//!
//! **Eight are generated.** [`crate::noise::perlin2_tiled`] folds the gradient
//! lattice so a tile meets itself on every edge; they are deterministic across
//! platforms like the rest of the tool; and there is nothing in them to
//! licence, so a map made only from these can go anywhere.
//!
//! **Eleven are photographs**, CC0 sets embedded as PNG and decoded on first
//! use. They earn their place by being what real ground looks like, which no
//! amount of noise quite reaches — but they arrive with the tiling problem the
//! generators are immune to by construction, so it is *measured* instead:
//! `photographs_tile_as_well_as_they_claim_to` compares the step across the
//! wrap against the step inside the image. See `assets/materials/SOURCES.md`
//! for where they came from and why they are 512² box-filtered.
//!
//! Note the two tiling properties are different, and only one is ours.
//! `every_material_is_periodic_in_both_axes` proves the *sampling* repeats
//! bit-for-bit and covers both kinds; it says nothing about whether a
//! photograph's own left edge matches its right.

use std::sync::Arc;

use rayon::prelude::*;

use crate::fdlibm;
use crate::field::clamp01;
use crate::noise::{fbm_tiled, perlin2_tiled};
use crate::rng::{hash2i, to_i32};

/// One texel of a material, before it is packed into images.
#[derive(Clone, Copy, Debug)]
pub struct Texel {
    /// Albedo, 0..1 per channel.
    pub albedo: [f64; 3],
    /// Surface relief, 0..1. The normal map is derived from this.
    pub height: f64,
    /// 0 is matte, 1 is a mirror. Feeds the specular slot.
    pub gloss: f64,
}

/// A named material and where its texels come from.
pub struct Material {
    pub key: &'static str,
    pub label: &'static str,
    /// One line on what it is for, shown in the picker.
    pub about: &'static str,
    /// How high the relief stands, in the same units the normal map uses.
    /// Sand ripples are shallow; gravel is not.
    pub relief: f64,
    draw: Draw,
}

impl Material {
    /// Whether this material is a photograph rather than a function.
    pub fn is_photo(&self) -> bool {
        matches!(self.draw, Draw::Photo(_))
    }
    /// Where a photographic material came from. `None` for procedural ones.
    pub fn source(&self) -> Option<&'static str> {
        match &self.draw {
            Draw::Photo(p) => Some(p.source),
            Draw::Procedural(_) => None,
        }
    }
}

enum Draw {
    /// Drawn by a function, at any resolution, from nothing.
    Procedural(fn(u: f64, v: f64, seed: f64, tiles: f64) -> Texel),
    /// Sampled from a photograph compiled into the binary.
    Photo(&'static Photo),
}

/// A photographic material: an albedo image and a height image, as PNG bytes
/// baked into the executable.
///
/// Embedded rather than loaded from a directory beside the binary because an
/// installed copy lives somewhere unwritable and frequently somewhere the
/// working directory is not — the same reason `default_output_dir` exists. A
/// material that silently fails to load is a map that silently loses its
/// ground.
///
/// Decoded once, on first use. That is file-free and side-effect-free by the
/// time any node sees it, so the rule that node evaluation opens no files is
/// untouched.
pub struct Photo {
    color_png: &'static [u8],
    height_png: &'static [u8],
    /// How glossy the surface reads, since the roughness map is not shipped.
    gloss: f64,
    /// Where the texture came from, so a map's ground has provenance.
    pub source: &'static str,
    cache: std::sync::OnceLock<PhotoData>,
}

struct PhotoData {
    res: usize,
    /// `res² · 3` albedo channels, 0..1.
    albedo: Vec<f32>,
    /// `res²` relief samples, 0..1.
    height: Vec<f32>,
}

impl Photo {
    fn data(&self) -> &PhotoData {
        self.cache.get_or_init(|| {
            let c = crate::png::decode(self.color_png).expect("embedded albedo PNG is valid");
            let h = crate::png::decode(self.height_png).expect("embedded height PNG is valid");
            let res = c.width.min(c.height);
            let mut albedo = vec![0.0f32; res * res * 3];
            let mut height = vec![0.0f32; res * res];
            for y in 0..res {
                for x in 0..res {
                    for ch in 0..3 {
                        albedo[(y * res + x) * 3 + ch] = c.value(x, y, ch) as f32;
                    }
                    height[y * res + x] = h.value(x, y, 0) as f32;
                }
            }
            // Stretch the relief to the full range.
            //
            // A photographic displacement map uses whatever slice of 0..1 the
            // capture happened to need — often a narrow band around the
            // middle — while every drawn material here uses all of it. Left
            // alone, the same `relief` number would mean a strong normal map
            // on a generator and a flat blue one on a photograph, which is
            // exactly what the first render of these showed. Stretching is
            // affine and global, so it cannot disturb the tiling.
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for v in &height {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
            if hi - lo > 1e-6 {
                let k = 1.0 / (hi - lo);
                for v in &mut height {
                    *v = (*v - lo) * k;
                }
            }
            PhotoData {
                res,
                albedo,
                height,
            }
        })
    }

    /// Bilinear, wrapping on both axes.
    ///
    /// The wrap is what makes a photograph tile the way a generator does: the
    /// indices come back through a modulo, so the texel at `u` and the texel
    /// at `u + 1` are read from the same place by the same arithmetic, and the
    /// tiling test that covers every material covers these too.
    fn texel(&self, u: f64, v: f64, tiles: f64) -> Texel {
        let d = self.data();
        let r = d.res;
        let (fu, fv) = (u * tiles * r as f64, v * tiles * r as f64);
        let (x0, y0) = (fu.floor(), fv.floor());
        let (tx, ty) = (fu - x0, fv - y0);
        let wrap = |a: f64| {
            let m = r as i64;
            (((a as i64 % m) + m) % m) as usize
        };
        let (xa, xb) = (wrap(x0), wrap(x0 + 1.0));
        let (ya, yb) = (wrap(y0), wrap(y0 + 1.0));
        let mix = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let bil = |get: &dyn Fn(usize, usize) -> f64| {
            mix(
                mix(get(xa, ya), get(xb, ya), tx),
                mix(get(xa, yb), get(xb, yb), tx),
                ty,
            )
        };
        let alb = |c: usize| bil(&|x: usize, y: usize| f64::from(d.albedo[(y * r + x) * 3 + c]));
        Texel {
            albedo: [alb(0), alb(1), alb(2)],
            height: bil(&|x: usize, y: usize| f64::from(d.height[y * r + x])),
            gloss: self.gloss,
        }
    }
}

/* ------------------------------------------------------ photographic set */

// Supplied by the project owner from the CC0 texture libraries their file
// names identify — ambientCG, Poly Haven and cgbookcase. Downsampled from 4K
// to 512², which is the size a Spring detail tile actually is: the reference
// map's is 512², and a 4K source is sixty-four times more pixels than the
// format can carry.
//
// The downsample is a box filter at an exact 8:1 ratio, so each output texel
// is the mean of a whole 8×8 block and nothing is sampled from outside the
// image. That matters: a filter with a wider kernel reads past the edge, and
// reading past the edge is how a seamless texture stops being seamless.
//
// Every one of these was measured for seam continuity before it was committed
// — see `photographs_tile_as_well_as_they_claim_to`.

static ASPHALT_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/asphalt.color.png"),
    height_png: include_bytes!("../../../assets/materials/asphalt.height.png"),
    gloss: 0.22,
    source: "ambientCG Asphalt001",
    cache: std::sync::OnceLock::new(),
};

static CONCRETE_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/concrete.color.png"),
    height_png: include_bytes!("../../../assets/materials/concrete.height.png"),
    gloss: 0.18,
    source: "cgbookcase brushed_concrete",
    cache: std::sync::OnceLock::new(),
};

static PAVEMENT_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/pavement.color.png"),
    height_png: include_bytes!("../../../assets/materials/pavement.height.png"),
    gloss: 0.14,
    source: "cgbookcase granular_concrete",
    cache: std::sync::OnceLock::new(),
};

static LAWN_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/lawn.color.png"),
    height_png: include_bytes!("../../../assets/materials/lawn.height.png"),
    gloss: 0.06,
    source: "ambientCG Grass004",
    cache: std::sync::OnceLock::new(),
};

static STEPPE_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/steppe.color.png"),
    height_png: include_bytes!("../../../assets/materials/steppe.height.png"),
    gloss: 0.08,
    source: "Poly Haven grass_path_3",
    cache: std::sync::OnceLock::new(),
};

static SCRUB_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/scrub.color.png"),
    height_png: include_bytes!("../../../assets/materials/scrub.height.png"),
    gloss: 0.07,
    source: "ambientCG Ground056",
    cache: std::sync::OnceLock::new(),
};

static DUST_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/dust.color.png"),
    height_png: include_bytes!("../../../assets/materials/dust.height.png"),
    gloss: 0.05,
    source: "ambientCG Ground054",
    cache: std::sync::OnceLock::new(),
};

static CLAY_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/clay.color.png"),
    height_png: include_bytes!("../../../assets/materials/clay.height.png"),
    gloss: 0.07,
    source: "cgbookcase brown_sand_plaster",
    cache: std::sync::OnceLock::new(),
};

static SILT_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/silt.color.png"),
    height_png: include_bytes!("../../../assets/materials/silt.height.png"),
    gloss: 0.12,
    source: "Poly Haven coast_sand_04",
    cache: std::sync::OnceLock::new(),
};

static PEAT_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/peat.color.png"),
    height_png: include_bytes!("../../../assets/materials/peat.height.png"),
    gloss: 0.1,
    source: "Poly Haven brown_mud_02",
    cache: std::sync::OnceLock::new(),
};

static TRACK_PHOTO: Photo = Photo {
    color_png: include_bytes!("../../../assets/materials/track.color.png"),
    height_png: include_bytes!("../../../assets/materials/track.height.png"),
    gloss: 0.09,
    source: "Poly Haven aerial_wood_snips",
    cache: std::sync::OnceLock::new(),
};

/// Every material the generator knows.
///
/// Ordered roughly by how much of a map they tend to cover, because that is
/// the order someone picking one scans in.
pub static MATERIALS: &[Material] = &[
    Material {
        key: "rock",
        label: "Rock",
        about: "Grey bedrock with fine fracture veins. The default ground.",
        relief: 0.55,
        draw: Draw::Procedural(rock),
    },
    Material {
        key: "cliff",
        label: "Cliff",
        about: "Stratified stone, banded across the tile. For steep faces.",
        relief: 1.0,
        draw: Draw::Procedural(cliff),
    },
    Material {
        key: "grass",
        label: "Grass",
        about: "Clumped green cover with hue variation.",
        relief: 0.25,
        draw: Draw::Procedural(grass),
    },
    Material {
        key: "sand",
        label: "Sand",
        about: "Pale ripples. Shallow relief, slightly glossy when wet.",
        relief: 0.2,
        draw: Draw::Procedural(sand),
    },
    Material {
        key: "dirt",
        label: "Dirt",
        about: "Blotchy brown earth, the neutral filler between the others.",
        relief: 0.35,
        draw: Draw::Procedural(dirt),
    },
    Material {
        key: "gravel",
        label: "Gravel",
        about: "Loose stones from a jittered cell pattern. Strong relief.",
        relief: 0.9,
        draw: Draw::Procedural(gravel),
    },
    Material {
        key: "snow",
        label: "Snow",
        about: "Wind-drifted white. Bright, glossy, almost flat.",
        relief: 0.3,
        draw: Draw::Procedural(snow),
    },
    Material {
        key: "mud",
        label: "Mud",
        about: "Dried and cracked, with the cracks cut into the relief.",
        relief: 0.7,
        draw: Draw::Procedural(mud),
    },
    Material {
        key: "asphalt",
        label: "Asphalt",
        about: "Fine grey road aggregate. Roads, pads and industrial flats.",
        relief: 0.3,
        draw: Draw::Photo(&ASPHALT_PHOTO),
    },
    Material {
        key: "concrete",
        label: "Concrete",
        about: "Brushed slab with faint tooling marks. Bases and hardstanding.",
        relief: 0.22,
        draw: Draw::Photo(&CONCRETE_PHOTO),
    },
    Material {
        key: "pavement",
        label: "Pavement",
        about: "Plain granular screed. The quietest surface here.",
        relief: 0.18,
        draw: Draw::Photo(&PAVEMENT_PHOTO),
    },
    Material {
        key: "lawn",
        label: "Meadow",
        about: "Dense green cover, photographed. Lusher than the drawn grass.",
        relief: 0.35,
        draw: Draw::Photo(&LAWN_PHOTO),
    },
    Material {
        key: "steppe",
        label: "Steppe",
        about: "Dry grass over loose stone. Temperate open ground.",
        relief: 0.45,
        draw: Draw::Photo(&STEPPE_PHOTO),
    },
    Material {
        key: "scrub",
        label: "Scrub",
        about: "Pale ground littered with twigs and debris.",
        relief: 0.4,
        draw: Draw::Photo(&SCRUB_PHOTO),
    },
    Material {
        key: "dust",
        label: "Dust",
        about: "Smooth pale desert floor, almost featureless.",
        relief: 0.2,
        draw: Draw::Photo(&DUST_PHOTO),
    },
    Material {
        key: "clay",
        label: "Clay",
        about: "Fine tan clay, wind-smoothed. Arid basins.",
        relief: 0.22,
        draw: Draw::Photo(&CLAY_PHOTO),
    },
    Material {
        key: "silt",
        label: "Silt",
        about: "Damp mottled coastal ground. For shorelines.",
        relief: 0.3,
        draw: Draw::Photo(&SILT_PHOTO),
    },
    Material {
        key: "peat",
        label: "Peat",
        about: "Dark wet earth, heavily broken up. Marsh and bog.",
        relief: 0.55,
        draw: Draw::Photo(&PEAT_PHOTO),
    },
    Material {
        key: "track",
        label: "Track",
        about: "Rutted orange dirt with wood litter. Worn routes.",
        relief: 0.5,
        draw: Draw::Photo(&TRACK_PHOTO),
    },
];

/// Sample a material, folding the coordinate into the tile first.
///
/// The fold is what makes periodicity *structural* rather than a property each
/// generator has to be careful to preserve. It also keeps trigonometric
/// strata honest: `sin` reduces a large argument slightly differently from a
/// small one, so `sin((v + 11) * TAU)` is not bit-identical to `sin(v * TAU)`
/// even though it is mathematically equal.
pub fn sample(m: &Material, u: f64, v: f64, seed: f64, tiles: f64) -> Texel {
    let fold = |a: f64| a - a.floor();
    match &m.draw {
        Draw::Procedural(f) => f(fold(u), fold(v), seed, tiles),
        Draw::Photo(p) => p.texel(fold(u), fold(v), tiles),
    }
}

pub fn find(key: &str) -> Option<&'static Material> {
    MATERIALS.iter().find(|m| m.key.eq_ignore_ascii_case(key))
}

pub fn keys() -> Vec<&'static str> {
    MATERIALS.iter().map(|m| m.key).collect()
}

/* ------------------------------------------------------------- generators */

/// Hash a cell to a stable 0..1, for per-pebble and per-clump variation.
fn cell_rand(ix: i32, iy: i32, seed: f64, salt: i32) -> f64 {
    f64::from(hash2i(ix, iy, to_i32(seed) ^ salt) & 0xFFFF) / 65535.0
}

/// Jittered-cell distance field that tiles: the cell grid wraps at `cells`.
///
/// Returns the distance to the nearest site and that site's random value, so
/// a caller gets both the shape and something to colour it with.
fn cells_tiled(u: f64, v: f64, seed: f64, cells: f64, jitter: f64) -> (f64, f64) {
    let n = cells.max(1.0);
    let (px, py) = (u * n, v * n);
    let (cx, cy) = (px.floor(), py.floor());
    let wrap = |i: f64| {
        let m = i % n;
        if m < 0.0 {
            m + n
        } else {
            m
        }
    };
    let mut best = f64::MAX;
    let mut best_id = 0.0;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let gx = cx + f64::from(ox);
            let gy = cy + f64::from(oy);
            let (wx, wy) = (to_i32(wrap(gx)), to_i32(wrap(gy)));
            let jx = gx + 0.5 + (cell_rand(wx, wy, seed, 0x11) - 0.5) * jitter;
            let jy = gy + 0.5 + (cell_rand(wx, wy, seed, 0x22) - 0.5) * jitter;
            let d = ((jx - px) * (jx - px) + (jy - py) * (jy - py)).sqrt();
            if d < best {
                best = d;
                best_id = cell_rand(wx, wy, seed, 0x33);
            }
        }
    }
    (best, best_id)
}

/// Two nearest cell distances, whose difference draws the cell *edges* — which
/// is what a crack pattern is.
fn cell_edges(u: f64, v: f64, seed: f64, cells: f64, jitter: f64) -> f64 {
    let n = cells.max(1.0);
    let (px, py) = (u * n, v * n);
    let (cx, cy) = (px.floor(), py.floor());
    let wrap = |i: f64| {
        let m = i % n;
        if m < 0.0 {
            m + n
        } else {
            m
        }
    };
    let (mut d1, mut d2) = (f64::MAX, f64::MAX);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let gx = cx + f64::from(ox);
            let gy = cy + f64::from(oy);
            let (wx, wy) = (to_i32(wrap(gx)), to_i32(wrap(gy)));
            let jx = gx + 0.5 + (cell_rand(wx, wy, seed, 0x11) - 0.5) * jitter;
            let jy = gy + 0.5 + (cell_rand(wx, wy, seed, 0x22) - 0.5) * jitter;
            let d = ((jx - px) * (jx - px) + (jy - py) * (jy - py)).sqrt();
            if d < d1 {
                d2 = d1;
                d1 = d;
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    d2 - d1
}

/// Blend two colours.
fn mix3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    let t = clamp01(t);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn shade(c: [f64; 3], k: f64) -> [f64; 3] {
    [clamp01(c[0] * k), clamp01(c[1] * k), clamp01(c[2] * k)]
}

fn rock(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    let base = fbm_tiled(u * t * 6.0, v * t * 6.0, seed, 5, 0.5, 2.0, false, t * 6.0);
    let veins = fbm_tiled(
        u * t * 14.0,
        v * t * 14.0,
        seed + 311.0,
        4,
        0.55,
        2.0,
        true,
        t * 14.0,
    );
    let h = clamp01(0.5 + base * 0.45 - veins * 0.3);
    let tone = 0.86 + base * 0.22 - veins * 0.18;
    Texel {
        albedo: shade(mix3([0.36, 0.35, 0.33], [0.55, 0.53, 0.50], h), tone),
        height: h,
        gloss: 0.16 + 0.10 * (1.0 - h),
    }
}

fn cliff(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    // Strata run across the tile. The band coordinate is warped by low
    // frequency noise so the layers are not ruler-straight.
    let warp = perlin2_tiled(u * t * 3.0, v * t * 3.0, seed + 71.0, t * 3.0) * 0.09;
    let band = (v + warp) * t * 11.0;
    let strata = fdlibm::sin(band * std::f64::consts::TAU) * 0.5 + 0.5;
    let grit = fbm_tiled(
        u * t * 18.0,
        v * t * 18.0,
        seed + 913.0,
        4,
        0.5,
        2.0,
        false,
        t * 18.0,
    );
    let h = clamp01(0.35 + strata * 0.45 + grit * 0.3);
    let layer = clamp01(strata * 0.8 + grit * 0.3);
    Texel {
        albedo: shade(
            mix3([0.27, 0.26, 0.26], [0.50, 0.46, 0.42], layer),
            0.9 + grit * 0.25,
        ),
        height: h,
        gloss: 0.12,
    }
}

fn grass(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    let clumps = fbm_tiled(u * t * 9.0, v * t * 9.0, seed, 4, 0.55, 2.0, false, t * 9.0);
    let blades = fbm_tiled(
        u * t * 40.0,
        v * t * 40.0,
        seed + 77.0,
        3,
        0.5,
        2.2,
        false,
        t * 40.0,
    );
    let h = clamp01(0.5 + clumps * 0.4 + blades * 0.35);
    // Hue varies with the clumps: dry patches read yellow, shaded ones blue.
    let dry = clamp01(clumps * 1.4 + 0.5);
    let c = mix3([0.20, 0.31, 0.15], [0.44, 0.47, 0.22], dry);
    Texel {
        albedo: shade(c, 0.88 + blades * 0.3),
        height: h,
        gloss: 0.08,
    }
}

fn sand(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    // Ripples: a wave whose phase is pushed around by noise, which is what
    // makes wind ripples read as ripples rather than corrugated iron.
    let drift = fbm_tiled(u * t * 4.0, v * t * 4.0, seed, 3, 0.5, 2.0, false, t * 4.0);
    // Integer wave numbers on both axes, or the ripples do not meet
    // themselves at the edge: 3 and 2 across the tile, times 7.
    let phase = (u * 3.0 + v * 2.0) * t * 7.0 + drift * 3.4;
    let ripple = fdlibm::sin(phase * std::f64::consts::TAU) * 0.5 + 0.5;
    let grain = fbm_tiled(
        u * t * 64.0,
        v * t * 64.0,
        seed + 5.0,
        2,
        0.5,
        2.0,
        false,
        t * 64.0,
    );
    let h = clamp01(0.45 + ripple * 0.35 + grain * 0.3);
    Texel {
        albedo: shade(
            mix3([0.66, 0.58, 0.42], [0.82, 0.75, 0.58], ripple),
            0.94 + grain * 0.18,
        ),
        height: h,
        gloss: 0.22,
    }
}

fn dirt(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    let blotch = fbm_tiled(u * t * 5.0, v * t * 5.0, seed, 5, 0.6, 2.1, false, t * 5.0);
    let grain = fbm_tiled(
        u * t * 30.0,
        v * t * 30.0,
        seed + 41.0,
        3,
        0.5,
        2.0,
        false,
        t * 30.0,
    );
    let h = clamp01(0.5 + blotch * 0.4 + grain * 0.28);
    let wet = clamp01(0.5 - blotch * 1.2);
    Texel {
        albedo: shade(
            mix3([0.34, 0.26, 0.18], [0.20, 0.15, 0.11], wet),
            0.92 + grain * 0.22,
        ),
        height: h,
        gloss: 0.10 + wet * 0.12,
    }
}

fn gravel(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    let (d, id) = cells_tiled(u, v, seed, t * 22.0, 0.9);
    // A stone is a dome: near the site it stands proud, at the edge it falls
    // away to the bed between them.
    let stone = clamp01(1.0 - d * 1.9);
    let bed = fbm_tiled(
        u * t * 26.0,
        v * t * 26.0,
        seed + 17.0,
        3,
        0.5,
        2.0,
        false,
        t * 26.0,
    );
    let h = clamp01(stone * 0.75 + bed * 0.25 + 0.1);
    let tone = 0.6 + id * 0.55;
    Texel {
        albedo: shade(
            mix3([0.28, 0.27, 0.25], [0.58, 0.56, 0.52], stone),
            tone + bed * 0.15,
        ),
        height: h,
        gloss: 0.18,
    }
}

fn snow(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    let drift = fbm_tiled(u * t * 5.0, v * t * 5.0, seed, 4, 0.55, 2.0, false, t * 5.0);
    let sparkle = fbm_tiled(
        u * t * 70.0,
        v * t * 70.0,
        seed + 29.0,
        2,
        0.5,
        2.0,
        false,
        t * 70.0,
    );
    let h = clamp01(0.55 + drift * 0.4 + sparkle * 0.12);
    Texel {
        albedo: shade(
            mix3([0.78, 0.81, 0.86], [0.96, 0.97, 1.0], h),
            0.97 + sparkle * 0.1,
        ),
        height: h,
        gloss: 0.45 + sparkle * 0.3,
    }
}

fn mud(u: f64, v: f64, seed: f64, t: f64) -> Texel {
    let crack = cell_edges(u, v, seed, t * 9.0, 0.85);
    // Cut the cracks in: narrow, dark and below the plate surface.
    let plate = clamp01(crack * 5.5);
    let grain = fbm_tiled(
        u * t * 24.0,
        v * t * 24.0,
        seed + 63.0,
        3,
        0.5,
        2.0,
        false,
        t * 24.0,
    );
    let h = clamp01(plate * 0.8 + grain * 0.2);
    Texel {
        albedo: shade(
            mix3([0.13, 0.10, 0.08], [0.36, 0.28, 0.20], plate),
            0.92 + grain * 0.2,
        ),
        height: h,
        gloss: 0.14,
    }
}

/* ------------------------------------------------------------ rasterising */

/// A material rendered to the images Spring wants.
pub struct Rendered {
    pub res: usize,
    /// `res² * 3` bytes of albedo.
    pub albedo: Vec<u8>,
    /// `res² * 4`: an RGB tangent-space normal plus a diffuse luminance in
    /// alpha, which is what `splatDetailNormalDiffuseAlpha = true` reads.
    pub normal: Vec<u8>,
    /// `res² * 4` greyscale specular with gloss in alpha.
    pub specular: Vec<u8>,
}

/// Draw a material at `res`, seeded so two maps do not share a tile.
///
/// `tiles` scales the feature density: 1.0 fits the material's natural grain
/// once across the tile, and the caller almost always wants exactly that,
/// because `splats.texScales` already controls how often the tile repeats.
pub fn render(m: &Material, res: usize, seed: f64, tiles: f64) -> Rendered {
    let res = res.max(4);
    let n = res * res;
    // Whole tiles only: every generator's frequencies are integers times this,
    // and a fractional value would put a seam back.
    let s = tiles.round().max(1.0);

    // Height first: the normal map is a derivative, so it needs neighbours.
    let mut height = vec![0.0f64; n];
    let mut albedo = vec![0u8; n * 3];
    let mut specular = vec![0u8; n * 4];
    let sample_row = |y: usize, h: &mut [f64], a: &mut [u8], sp: &mut [u8]| {
        let v = y as f64 / res as f64;
        for x in 0..res {
            let u = x as f64 / res as f64;
            let t = sample(m, u, v, seed, s);
            h[x] = t.height;
            for c in 0..3 {
                a[x * 3 + c] = (clamp01(t.albedo[c]) * 255.0).round() as u8;
            }
            // Spring's specular texture is RGB with the exponent scale in
            // alpha; a neutral grey keyed off gloss is a defensible start.
            let g = (clamp01(t.gloss) * 255.0).round() as u8;
            sp[x * 4] = g;
            sp[x * 4 + 1] = g;
            sp[x * 4 + 2] = g;
            sp[x * 4 + 3] = 255;
        }
    };
    height
        .par_chunks_mut(res)
        .zip(albedo.par_chunks_mut(res * 3))
        .zip(specular.par_chunks_mut(res * 4))
        .enumerate()
        .for_each(|(y, ((h, a), sp))| sample_row(y, h, a, sp));

    // Normals, wrapping at the edges so the normal map tiles with the albedo.
    let mut normal = vec![0u8; n * 4];
    let at = |x: isize, y: isize| {
        let r = res as isize;
        let xi = ((x % r) + r) % r;
        let yi = ((y % r) + r) % r;
        height[yi as usize * res + xi as usize]
    };
    let strength = m.relief * res as f64 / 64.0;
    normal
        .par_chunks_mut(res * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..res {
                let (xi, yi) = (x as isize, y as isize);
                let dx = at(xi + 1, yi) - at(xi - 1, yi);
                let dy = at(xi, yi + 1) - at(xi, yi - 1);
                let (nx, ny, nz) = (-dx * strength, -dy * strength, 1.0);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let put = |v: f64| ((v / len * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
                row[x * 4] = put(nx);
                row[x * 4 + 1] = put(ny);
                row[x * 4 + 2] = put(nz);
                // Alpha is the diffuse modulation Spring multiplies the
                // ground colour by, so it is centred on mid-grey: 128 leaves
                // the terrain's own colour alone.
                let d = 0.5 + (at(xi, yi) - 0.5) * 0.7;
                row[x * 4 + 3] = (clamp01(d) * 255.0).round() as u8;
            }
        });

    Rendered {
        res,
        albedo,
        normal,
        specular,
    }
}

/// Mean of each texel's neighbourhood, wrapping at the edges.
///
/// Separable and running-sum, so the radius is free. Wrapping is not optional:
/// this is subtracted from a tile that has to stay periodic, and a blur that
/// clamped at the edge would put a seam into a tile that did not have one.
fn local_mean_wrapping(v: &[f64], res: usize, radius: usize) -> Vec<f64> {
    let r = radius.min(res / 2).max(1);
    let span = (2 * r + 1) as f64;
    let mut tmp = vec![0.0; v.len()];
    let mut out = vec![0.0; v.len()];
    let wrap = |i: isize, n: isize| (((i % n) + n) % n) as usize;
    let n = res as isize;
    for y in 0..res {
        let row = y * res;
        for x in 0..res {
            let mut s = 0.0;
            for d in -(r as isize)..=(r as isize) {
                s += v[row + wrap(x as isize + d, n)];
            }
            tmp[row + x] = s / span;
        }
    }
    for x in 0..res {
        for y in 0..res {
            let mut s = 0.0;
            for d in -(r as isize)..=(r as isize) {
                s += tmp[wrap(y as isize + d, n) * res + x];
            }
            out[y * res + x] = s / span;
        }
    }
    out
}

/// A neutral detail tile: the engine *adds* `detailTex` to the diffuse as a
/// signed offset -- `detailCol = tex * 2 - 1`, then
/// `fragColor.rgb = (diffuseCol.rgb + detailCol.rgb) * shadeInt.rgb` -- so it
/// has to be centred on mid-grey or it lifts or crushes the whole map.
///
/// Mid-grey is the neutral value under a multiply too, which is why this was
/// right while the reason recorded for it was wrong.
///
/// It is high-passed, not merely mean-centred, and that is the difference
/// between a detail tile and a visible grid. Subtracting the tile's *global*
/// mean removes only the DC term and leaves every slow brightness gradient the
/// capture happened to have — and a slow gradient in a tile that Spring repeats
/// thirty times across a map is a checkerboard thirty squares wide. Subtracting
/// the *local* mean leaves only the grain, which is the one thing this slot is
/// for.
///
/// The drawn materials barely notice, having little low-frequency content to
/// begin with. The photographs are unusable here without it: measured across
/// the set before this existed, their low-frequency spread was five to fifteen
/// times a drawn tile's.
pub fn detail_tile(m: &Material, res: usize, seed: f64) -> Vec<u8> {
    let r = render(m, res, seed, 1.0);
    let n = r.res * r.res;
    let lum: Vec<f64> = (0..n)
        .map(|i| {
            0.299 * f64::from(r.albedo[i * 3])
                + 0.587 * f64::from(r.albedo[i * 3 + 1])
                + 0.114 * f64::from(r.albedo[i * 3 + 2])
        })
        .collect();
    // An eighth of the tile: wide enough to be "the slow part" and narrow
    // enough to leave the grain alone.
    let local = local_mean_wrapping(&lum, r.res, r.res / 8);
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        let v = (128.0 + (lum[i] - local[i]) * 0.55)
            .round()
            .clamp(0.0, 255.0) as u8;
        out[i * 3] = v;
        out[i * 3 + 1] = v;
        out[i * 3 + 2] = v;
    }
    out
}

/// A tiny thumbnail for a picker, as RGB.
pub fn thumbnail(m: &Material, res: usize, seed: f64) -> Vec<u8> {
    render(m, res, seed, 1.0).albedo
}

/// The four splat channels' materials, plus the single detail tile.
///
/// Held as keys rather than resolved materials so a project round-trips
/// through JSON as names, and an unknown name degrades to the default with a
/// warning rather than failing the load.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialSet {
    /// One per RGBA channel of the splat distribution.
    pub splat: [String; 4],
    pub detail: String,
    /// Tile resolution. Spring repeats these forever, so 512 is plenty and
    /// the reference map's own detail tile is exactly that.
    #[serde(default = "default_tile_res")]
    pub tile_res: usize,
    /// How strongly the materials colour the baked diffuse, 0..1.
    ///
    /// The detail normals only add relief to lighting; the ground's *colour*
    /// comes from the map's own texture. Blending the materials into it is
    /// what makes a rock channel look like rock rather than like a slightly
    /// bumpier version of whatever the graph painted. Zero bakes the graph's
    /// diffuse untouched and leaves the materials to the GPU detail slots.
    #[serde(default = "default_blend")]
    pub blend: f64,
}

fn default_tile_res() -> usize {
    512
}

fn default_blend() -> f64 {
    0.55
}

impl Default for MaterialSet {
    /// Matches what the starter graphs actually wire into `out_splat`:
    /// R is the steep-ground rock mask, G the slope mask, B the height mask.
    /// A is unwired, so its material only shows if you wire it.
    fn default() -> Self {
        MaterialSet {
            splat: [
                "rock".into(),
                "gravel".into(),
                "grass".into(),
                "sand".into(),
            ],
            detail: "rock".into(),
            tile_res: default_tile_res(),
            blend: default_blend(),
        }
    }
}

impl MaterialSet {
    /// Resolve a channel, falling back to the default material rather than
    /// failing: a typo in a project file should not stop a bake.
    pub fn channel(&self, i: usize) -> &'static Material {
        let d = MaterialSet::default();
        let key = self.splat.get(i).unwrap_or(&d.splat[0]);
        find(key)
            .or_else(|| find(&d.splat[i.min(3)]))
            .unwrap_or(&MATERIALS[0])
    }

    pub fn detail_material(&self) -> &'static Material {
        find(&self.detail).unwrap_or(&MATERIALS[0])
    }

    /// Names that do not resolve, so the caller can say so once.
    pub fn unknown(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .splat
            .iter()
            .chain(std::iter::once(&self.detail))
            .filter(|k| find(k).is_none())
            .cloned()
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// A rendered material set, ready to be written into an archive.
pub struct SplatMaterials {
    pub albedo: [Arc<Rendered>; 4],
    pub detail: Vec<u8>,
    pub detail_res: usize,
}

/// Render every slot a map needs. Four materials at 512² is about 30 ms.
pub fn render_set(set: &MaterialSet, seed: f64) -> SplatMaterials {
    let res = set.tile_res.clamp(64, 2048).next_power_of_two();
    let rendered: Vec<Arc<Rendered>> = (0..4)
        .into_par_iter()
        .map(|i| Arc::new(render(set.channel(i), res, seed + i as f64 * 1009.0, 1.0)))
        .collect();
    let detail_res = (res / 2).max(64);
    SplatMaterials {
        albedo: [
            Arc::clone(&rendered[0]),
            Arc::clone(&rendered[1]),
            Arc::clone(&rendered[2]),
            Arc::clone(&rendered[3]),
        ],
        detail: detail_tile(set.detail_material(), detail_res, seed + 77.0),
        detail_res,
    }
}

/// How often `detailTex` repeats, in tiles per elmo.
///
/// `SMF_DETAILTEX_RES` in the engine's SMF fragment shader, which builds its
/// coordinate as `vertexWorldPos.xz * vec2(SMF_DETAILTEX_RES)`. One repeat
/// every 50 elmos -- about 123 times across a 12×12 map, not the thirty this
/// file used to claim.
pub const DETAIL_TEX_RES: f64 = 0.02;

/// Successive box-halvings of the detail tile, coarsest last. `[0]` is the
/// tile itself.
///
/// A whole-map preview pixel covers tens of detail repeats. Taking one texel
/// of the tile per pixel would not show the grain, it would show aliasing --
/// the same mistake that once made a sample map look quilted when judged at
/// 400 px. Averaging over the pixel's real footprint is what the GPU does,
/// and it is what makes the view honest at any scale: grain when a pixel is
/// small enough to resolve it, and the tile's mean -- which for a high-passed
/// tile is nothing at all -- when it is not.
fn mip_chain(tile: &[u8], res: usize) -> Vec<(usize, Vec<u8>)> {
    let mut levels = vec![(res, tile.to_vec())];
    while levels.last().map(|(r, _)| *r).unwrap_or(0) > 1 {
        let (r, ref src) = *levels.last().unwrap();
        let half = r / 2;
        let mut out = vec![0u8; half * half * 3];
        for y in 0..half {
            for x in 0..half {
                for c in 0..3 {
                    let s = u32::from(src[((y * 2) * r + x * 2) * 3 + c])
                        + u32::from(src[((y * 2) * r + x * 2 + 1) * 3 + c])
                        + u32::from(src[((y * 2 + 1) * r + x * 2) * 3 + c])
                        + u32::from(src[((y * 2 + 1) * r + x * 2 + 1) * 3 + c]);
                    out[(y * half + x) * 3 + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        levels.push((half, out));
    }
    levels
}

/// Fold a texture coordinate into `0..1`. A detail tile is repeated, so a
/// coordinate outside the tile is a coordinate inside the next copy of it.
fn fold(a: f64) -> f64 {
    let m = a % 1.0;
    if m < 0.0 {
        m + 1.0
    } else {
        m
    }
}

/// `splats.texScales` measured from a shipped map, in tiles per elmo.
///
/// Shared so the baked texture and the tool's preview place the tiles at the
/// same frequency the engine will.
pub const DEFAULT_TEX_SCALES: [f64; 4] = [0.006, 0.003, 0.0058, 0.006];

/// Blends the four material tiles into a ground colour.
///
/// The tile frequency is `splats.texScales`, the same number the GPU uses for
/// the detail normals, so what the baked texture shows and what the engine
/// overlays on top of it are in step rather than two different grains fighting.
pub struct Blender<'a> {
    tiles: [&'a Rendered; 4],
    /// Tiles per elmo, per channel.
    scales: [f64; 4],
    strength: f64,
    /// The detail tile, box-halved all the way down. See `mip_chain`.
    detail: Vec<(usize, Vec<u8>)>,
}

impl<'a> Blender<'a> {
    pub fn new(mats: &'a SplatMaterials, scales: [f64; 4], strength: f64) -> Blender<'a> {
        Blender {
            tiles: [
                &mats.albedo[0],
                &mats.albedo[1],
                &mats.albedo[2],
                &mats.albedo[3],
            ],
            scales,
            strength: clamp01(strength),
            detail: mip_chain(&mats.detail, mats.detail_res),
        }
    }

    pub fn active(&self) -> bool {
        self.strength > 0.0
    }

    /// Add the detail tile the way the engine does, over a pixel covering
    /// `footprint` elmos.
    ///
    /// `detailCol = tex * 2 - 1` and `fragColor.rgb = diffuseCol.rgb +
    /// detailCol.rgb` (before shading), so a mid-grey tile changes nothing and
    /// the grain rides as a signed offset either side of it.
    ///
    /// **The bake must not call this and the preview must.** Spring adds the
    /// tile over the baked diffuse at runtime; a bake that pre-applied it
    /// would lay the grain down twice. The preview stands in for the engine,
    /// so leaving it out there is what made the one surface covering every
    /// texel of a finished map impossible to judge in the tool.
    pub fn detail(&self, base: [f64; 3], ex: f64, ez: f64, footprint: f64) -> [f64; 3] {
        let (res, tile) = self.level_for(footprint);
        let r = res as f64;
        let tx = (fold(ex * DETAIL_TEX_RES) * r) as usize % res;
        let tz = (fold(ez * DETAIL_TEX_RES) * r) as usize % res;
        let o = (tz * res + tx) * 3;
        let mut out = [0.0f64; 3];
        for c in 0..3 {
            out[c] = clamp01(base[c] + (f64::from(tile[o + c]) / 255.0 * 2.0 - 1.0));
        }
        out
    }

    /// The mip level whose texels are closest to the size of the area being
    /// sampled. `footprint` is in elmos; a texel is `1 / (DETAIL_TEX_RES * res)`
    /// elmos across.
    fn level_for(&self, footprint: f64) -> (usize, &[u8]) {
        let base = self.detail[0].0 as f64;
        // Texels of the full-resolution tile spanned by one sample.
        let span = (footprint * DETAIL_TEX_RES * base).max(1.0);
        let level = (span.log2().floor() as usize).min(self.detail.len() - 1);
        let (r, ref t) = self.detail[level];
        (r, t)
    }

    /// `base` is what the graph painted, `w` the splat distribution's four
    /// weights at this texel, and `ex`/`ez` its position in elmos.
    pub fn shade(&self, base: [f64; 3], w: [f64; 4], ex: f64, ez: f64) -> [f64; 3] {
        let total: f64 = w.iter().map(|v| clamp01(*v)).sum();
        if total <= 1e-6 || self.strength <= 0.0 {
            return base;
        }
        let mut mixed = [0.0f64; 3];
        for i in 0..4 {
            let wi = clamp01(w[i]);
            if wi <= 1e-6 {
                continue;
            }
            let t = self.tiles[i];
            let r = t.res as f64;
            let tx = (fold(ex * self.scales[i]) * r) as usize % t.res;
            let tz = (fold(ez * self.scales[i]) * r) as usize % t.res;
            let o = (tz * t.res + tx) * 3;
            for c in 0..3 {
                mixed[c] += f64::from(t.albedo[o + c]) / 255.0 * wi;
            }
        }
        // Weights need not sum to one -- `out_splat` only normalises when it
        // is told to -- so the blend is capped by the coverage there is.
        let k = self.strength * clamp01(total);
        let inv = 1.0 / total.max(1e-6);
        [
            base[0] + (mixed[0] * inv - base[0]) * k,
            base[1] + (mixed[1] * inv - base[1]) * k,
            base[2] + (mixed[2] * inv - base[2]) * k,
        ]
    }
}

/// A patch of ground as the engine will draw it, at a scale where the grain
/// is resolvable.
///
/// No whole-map view can show a detail tile. It repeats every 50 elmos and a
/// pixel of a map-wide preview covers many times that, so the honest thing
/// for that view to show is the tile's mean — nothing. This renders `elmos`
/// of ground across `res` pixels instead, holding the splat weights at `w`,
/// and is the view that answers what a surface looks like underfoot.
///
/// It is the answer to the problem that shipped seven procedural detail tiles
/// on retextured maps without anyone being able to see it: the one surface
/// covering every texel of a finished map had nowhere in the tool to be
/// looked at.
pub fn ground_sample(
    mats: &SplatMaterials,
    scales: [f64; 4],
    strength: f64,
    w: [f64; 4],
    base: [f64; 3],
    res: usize,
    elmos: f64,
) -> Vec<u8> {
    let b = Blender::new(mats, scales, strength);
    let res = res.max(2);
    let step = elmos / res as f64;
    let mut out = vec![0u8; res * res * 3];
    for y in 0..res {
        for x in 0..res {
            let (ex, ez) = (x as f64 * step, y as f64 * step);
            let rgb = b.detail(b.shade(base, w, ex, ez), ex, ez, step);
            for c in 0..3 {
                out[(y * res + x) * 3 + c] = (rgb[c] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason these are generated rather than downloaded: a tile
    /// meets itself exactly, so repeating it across a map draws no grid.
    ///
    /// Tested as the property itself rather than by looking for a seam in the
    /// output — the sample at `u` and the sample at `u + 1` have to be the
    /// same texel, which is exact and needs no threshold.
    #[test]
    fn no_detail_tile_carries_enough_low_frequency_to_grid() {
        // The engine repeats `detailTex` every 50 elmos, so it crosses a
        // 12×12 map about 123 times -- see `DETAIL_TEX_RES`.
        // Fine grain is the point of the slot and disappears into the ground;
        // slow brightness variation does not — it becomes a checkerboard as
        // wide as the map. So the test is on the *low* frequencies only:
        // shrink each tile to 8×8 by averaging, and measure the spread of what
        // is left. That is precisely the signal that repeats visibly.
        const RES: usize = 128;
        const CELLS: usize = 8;
        for m in MATERIALS {
            let t = detail_tile(m, RES, 5.0);
            let step = RES / CELLS;
            let mut cells = Vec::with_capacity(CELLS * CELLS);
            for cy in 0..CELLS {
                for cx in 0..CELLS {
                    let mut s = 0.0;
                    for y in 0..step {
                        for x in 0..step {
                            s += f64::from(t[((cy * step + y) * RES + cx * step + x) * 3]);
                        }
                    }
                    cells.push(s / (step * step) as f64);
                }
            }
            let mean = cells.iter().sum::<f64>() / cells.len() as f64;
            let sd =
                (cells.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / cells.len() as f64).sqrt();
            assert!(
                sd < 3.0,
                "{}: detail tile varies by {sd:.1} levels at low frequency — it will grid across the map",
                m.key
            );
            // And still centred, or it tints the whole map.
            assert!(
                (mean - 128.0).abs() < 4.0,
                "{}: detail tile mean is {mean:.1}, not mid-grey",
                m.key
            );
        }
    }

    #[test]
    fn photographs_tile_as_well_as_they_claim_to() {
        // Periodicity and seamlessness are two different things, and only one
        // of them is ours. `every_material_is_periodic_in_both_axes` proves
        // our sampling repeats exactly; it would pass just as happily on a
        // photograph whose left edge does not match its right, and that
        // photograph would lay a visible grid across the whole map.
        //
        // So this measures the *content*: the step across the wrap against the
        // typical step inside the image. A seamless texture is about 1.0. A
        // photograph that was never tileable runs to several times that.
        for m in MATERIALS {
            if !m.is_photo() {
                continue;
            }
            const N: usize = 128;
            let at = |x: usize, y: usize| {
                sample(m, x as f64 / N as f64, y as f64 / N as f64, 3.0, 1.0).albedo
            };
            let d = |a: [f64; 3], b: [f64; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f64>();

            let mut seam_x = 0.0;
            let mut step_x = 0.0;
            let mut seam_y = 0.0;
            let mut step_y = 0.0;
            for i in 0..N {
                seam_x += d(at(0, i), at(N - 1, i));
                seam_y += d(at(i, 0), at(i, N - 1));
                for j in 1..N {
                    step_x += d(at(j, i), at(j - 1, i));
                    step_y += d(at(i, j), at(i, j - 1));
                }
            }
            let inner = (N - 1) as f64;
            let (rx, ry) = (seam_x / (step_x / inner), seam_y / (step_y / inner));
            assert!(
                rx < 2.0 && ry < 2.0,
                "{}: seam is {rx:.2}× the interior step across X and {ry:.2}× across Y — this texture does not tile, and Spring repeats it over the whole map",
                m.key
            );
        }
    }

    #[test]
    fn every_photograph_says_where_it_came_from() {
        // Ground with no provenance is ground nobody can check the licence of.
        for m in MATERIALS {
            assert_eq!(
                m.is_photo(),
                m.source().is_some(),
                "{}: a photograph must carry a source and a drawn material must not",
                m.key
            );
            if let Some(s) = m.source() {
                assert!(!s.is_empty(), "{}: empty source", m.key);
            }
        }
    }

    #[test]
    fn every_material_is_periodic_in_both_axes() {
        for m in MATERIALS {
            // Sixteenths: exact binary fractions, so `u` and `u + 1` differ
            // only in their integer part and everything downstream of the
            // fold is bit-identical.
            for gy in 0..16 {
                for gx in 0..16 {
                    let (u, v) = (gx as f64 / 16.0, gy as f64 / 16.0);
                    let here = sample(m, u, v, 7.0, 1.0);
                    for (du, dv) in [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (-1.0, 2.0)] {
                        let there = sample(m, u + du, v + dv, 7.0, 1.0);
                        for c in 0..3 {
                            assert_eq!(
                                here.albedo[c].to_bits(),
                                there.albedo[c].to_bits(),
                                "{}: albedo channel {c} differs one tile over at ({u}, {v})",
                                m.key
                            );
                        }
                        assert_eq!(
                            here.height.to_bits(),
                            there.height.to_bits(),
                            "{}: relief differs one tile over at ({u}, {v})",
                            m.key
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rendering_is_deterministic_and_the_right_shape() {
        for m in MATERIALS {
            let a = render(m, 32, 3.0, 1.0);
            let b = render(m, 32, 3.0, 1.0);
            assert_eq!(a.albedo, b.albedo, "{}: albedo", m.key);
            assert_eq!(a.normal, b.normal, "{}: normal", m.key);
            assert_eq!(a.albedo.len(), 32 * 32 * 3);
            assert_eq!(a.normal.len(), 32 * 32 * 4);
            assert_eq!(a.specular.len(), 32 * 32 * 4);
            // A material that came out one flat colour is not a material.
            let mut distinct: Vec<&[u8]> = a.albedo.chunks(3).collect();
            distinct.sort_unstable();
            distinct.dedup();
            assert!(
                distinct.len() > 16,
                "{}: only {} colours",
                m.key,
                distinct.len()
            );
        }
    }

    #[test]
    fn normals_point_outward() {
        // Z is up in tangent space, so the blue channel must dominate or the
        // lighting comes out inside-out.
        for m in MATERIALS {
            let r = render(m, 32, 1.0, 1.0);
            let low = (0..32 * 32).filter(|i| r.normal[i * 4 + 2] < 128).count();
            assert_eq!(low, 0, "{}: {low} texels face into the surface", m.key);
        }
    }

    /// The detail tile has to appear when a pixel can resolve it and vanish
    /// when it cannot — and both halves are the point.
    ///
    /// Showing grain at map scale would be the aliasing that once made a
    /// sample map look quilted at 400 px. Showing none at ground scale is the
    /// bug this replaced, where seven procedural tiles shipped on retextured
    /// maps because nothing in the tool drew them.
    #[test]
    fn the_detail_tile_resolves_underfoot_and_averages_away_across_a_map() {
        let set = MaterialSet::default();
        let mats = render_set(&set, 3.0);
        // Flat weights and a mid grey base, so anything that varies is the
        // material and the tile rather than the terrain.
        let w = [1.0, 0.0, 0.0, 0.0];
        let base = [0.5, 0.5, 0.5];

        let spread = |elmos: f64| {
            let px = ground_sample(&mats, DEFAULT_TEX_SCALES, 0.0, w, base, 64, elmos);
            let v: Vec<f64> = (0..px.len() / 3).map(|i| f64::from(px[i * 3])).collect();
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };

        // 200 elmos across 64 px is about 3 elmos a pixel: four repeats of the
        // tile, its texels comfortably larger than a pixel.
        let near = spread(200.0);
        // A 12×12 map across the same 64 px is 96 elmos a pixel, nearly two
        // whole repeats each.
        let far = spread(6144.0);

        assert!(
            near > 2.0,
            "detail tile is invisible underfoot ({near:.2} levels of spread at 3 elmos/px)"
        );
        assert!(
            far < 0.5,
            "detail tile still varies at map scale ({far:.2} levels at 96 elmos/px) — it is aliasing, not grain"
        );
    }

    #[test]
    fn the_detail_tile_is_centred_on_mid_grey() {
        // Spring multiplies detailTex over the diffuse. Off-centre and the
        // whole map darkens or blows out.
        for m in MATERIALS {
            let t = detail_tile(m, 64, 2.0);
            let mean = t.iter().map(|v| f64::from(*v)).sum::<f64>() / t.len() as f64;
            assert!(
                (mean - 128.0).abs() < 14.0,
                "{}: detail tile mean {mean:.1}",
                m.key
            );
        }
    }

    #[test]
    fn an_unknown_material_falls_back_instead_of_failing() {
        let set = MaterialSet {
            splat: [
                "rock".into(),
                "nonsense".into(),
                "grass".into(),
                "sand".into(),
            ],
            detail: "also-nonsense".into(),
            tile_res: 512,
            blend: default_blend(),
        };
        assert_eq!(set.unknown(), vec!["also-nonsense", "nonsense"]);
        assert_eq!(set.channel(0).key, "rock");
        assert_eq!(set.channel(1).key, "gravel", "falls back to the default");
        assert_eq!(set.detail_material().key, "rock");
    }
}

//! Zero-K metal spot semantics.
//!
//! Derived from `LuaRules/Gadgets/mex_spot_finder.lua` (Niobium, modified by
//! Google Frog) and cross-checked against a shipped map's
//! `mapconfig/map_metal_layout.lua`. A bad metal setup still loads and still
//! looks fine, which is exactly why every rule here is spelled out.

use serde::{Deserialize, Serialize};

use crate::field::SharedField;
use crate::nodes::slope_degrees_aniso;
use crate::project::Context;

/// Engine constants, named so the reason for each threshold survives.
pub struct Zk;

impl Zk {
    /// `Game.metalMapSquareSize` — 16 elmos per metalmap pixel.
    pub const METAL_GRID: f64 = 16.0;
    pub const DEFAULT_MEX_INCOME: f64 = 2.0;
    /// Spots at or below this are discarded silently: `if spot.metal > 0.2`.
    pub const MINIMUM_MEX_INCOME: f64 = 0.2;
    /// Fewer detected blobs than this trips the "build anywhere" fallback.
    pub const INDISCRETE_MIN_SPOTS: usize = 6;
    /// `distance² < R² * 1.7` — merged, certainly.
    pub const MERGE_CERTAIN: f64 = 1.7;
    /// `distance² < R² * 4.0` — merge candidate.
    pub const MERGE_WINDOW: f64 = 4.0;
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetalSpot {
    pub x: f64,
    pub z: f64,
    pub metal: f64,
    #[serde(default)]
    pub id: String,
    /// On-axis spots are their own mirror image and must not be duplicated.
    #[serde(default, skip_serializing_if = "is_false")]
    pub single: bool,
}

impl MetalSpot {
    pub fn new(x: f64, z: f64, metal: f64, id: impl Into<String>) -> MetalSpot {
        MetalSpot {
            x,
            z,
            metal,
            id: id.into(),
            single: false,
        }
    }
    pub fn live(&self) -> bool {
        self.metal > Zk::MINIMUM_MEX_INCOME
    }
}

/// ZK's `AdjustCoordinates`. The even case is round-half-up, which is **not**
/// symmetric about the map centre: `snap(920) = 928` but `5120 - snap(920)`
/// and `snap(5120 - 920)` differ by a whole 16-elmo cell.
pub fn snap_spot(v: f64, odd_footprint: bool) -> f64 {
    let g = Zk::METAL_GRID;
    if odd_footprint {
        ((v / g).floor() + 0.5) * g
    } else {
        (v / g + 0.5).floor() * g
    }
}

/// A point turned `k` fifths of a full turn about the world centre.
///
/// Split out because five-fold is the one operator whose images are not a
/// relabelling of the coordinates — it needs the actual rotation, and both the
/// spot placer and the start boxes want the same one.
pub fn rotate_about_centre(x: f64, z: f64, w: f64, h: f64, k: usize) -> (f64, f64) {
    let a = (k % 5) as f64 * 72.0 * std::f64::consts::PI / 180.0;
    let (ca, sa) = (crate::fdlibm::cos(a), crate::fdlibm::sin(a));
    let (cx, cz) = (w / 2.0, h / 2.0);
    let (dx, dz) = (x - cx, z - cz);
    (cx + dx * ca - dz * sa, cz + dx * sa + dz * ca)
}

/// The images of a point under a symmetry operator, excluding the point itself.
pub fn symmetry_images(x: f64, z: f64, mode: &str, w: f64, h: f64) -> Vec<(f64, f64)> {
    match mode {
        "mirrorX" => vec![(w - x, z)],
        "mirrorY" => vec![(x, h - z)],
        "quad" => vec![(w - x, z), (x, h - z), (w - x, h - z)],
        "rot180" => vec![(w - x, h - z)],
        "rot90" => vec![(z, h - x), (w - x, h - z), (w - z, x)],
        // Five-fold: the four other fifths of a turn about the centre. Unlike
        // every other operator here this one is not exact on the 16-elmo grid,
        // so the images are snapped by the caller like any other candidate and
        // the symmetry report checks them rather than assuming.
        "rot72" => (1..5).map(|k| rotate_about_centre(x, z, w, h, k)).collect(),
        "diagonal" => vec![(h - z, w - x)],
        _ => Vec::new(),
    }
}

/// The fixed set of a symmetry operator — the axis or centre where a spot is
/// its own mirror.
///
/// A mex *near* an axis but not *on* it collides with its own image and cannot
/// be made fair, so such candidates are projected here or rejected outright
/// when the projection is not representable on the grid.
pub fn axis_project(
    x: f64,
    z: f64,
    mode: &str,
    w: f64,
    h: f64,
    odd_fp: bool,
) -> Option<(f64, f64)> {
    let cx = w / 2.0;
    let cz = h / 2.0;
    let p = match mode {
        "mirrorX" => (cx, z),
        "mirrorY" => (x, cz),
        "quad" | "rot180" | "rot90" | "rot72" => (cx, cz),
        "diagonal" => {
            let mid = (x + (h - z)) / 2.0;
            (mid, h - mid)
        }
        _ => return None,
    };
    let p = (snap_spot(p.0, odd_fp), snap_spot(p.1, odd_fp));
    // Verify the projected point really is its own image.
    for (ix, iz) in symmetry_images(p.0, p.1, mode, w, h) {
        let dx = ix - p.0;
        let dz = iz - p.1;
        if (dx * dx + dz * dz).sqrt() > 0.5 {
            return None;
        }
    }
    Some(p)
}

#[derive(Clone, Debug)]
pub struct ProposeOptions {
    pub count: usize,
    pub min_separation: f64,
    pub symmetry: String,
    pub threshold: f64,
    pub amount: f64,
    pub odd_footprint: bool,
    /// ZK scans the metalmap from 1.5 grid cells inwards, so keep clear of edges.
    pub margin: f64,
    /// What the mex footprint has to sit on. `None` places blind, which is
    /// what the prototype did and what makes the buildability check a
    /// post-mortem instead of a constraint.
    pub build: Option<BuildabilityOptions>,
    /// How far a candidate may be nudged to reach buildable ground, in 16-elmo
    /// metal-grid cells. Whole cells only, so the nudged spot is still snapped
    /// and its symmetry images are still exact.
    pub search_cells: i32,
    /// Points already spoken for, which new spots must stay `min_separation`
    /// clear of. Geothermal vents are placed after the mexes and have to
    /// respect them, and filtering afterwards throws away every vent instead:
    /// both sets come from the same mask and want the same peaks.
    pub avoid: Vec<(f64, f64)>,
}

impl Default for ProposeOptions {
    fn default() -> Self {
        ProposeOptions {
            count: 12,
            min_separation: 700.0,
            symmetry: "none".into(),
            threshold: 0.2,
            amount: Zk::DEFAULT_MEX_INCOME,
            odd_footprint: false,
            margin: Zk::METAL_GRID * 16.0,
            build: Some(BuildabilityOptions::default()),
            search_cells: 16,
            avoid: Vec::new(),
        }
    }
}

/// Greedy placement from a suitability mask, symmetry-completed.
///
/// Order matters: **snap first, then mirror.** Because the map width is always
/// a multiple of 512 (hence of the 16-elmo grid), the mirror of an
/// already-snapped point is itself grid-aligned, so deriving images from the
/// snapped primary keeps every symmetry group exactly fair. Snapping each
/// image independently drifts them a cell apart, which is what real maps that
/// author unsnapped coordinates suffer from.
pub fn propose_spots(mask: &SharedField, ctx: &Context, opts: &ProposeOptions) -> Vec<MetalSpot> {
    propose_spots_on(mask, None, ctx, opts)
}

/// As [`propose_spots`], but with the terrain the footprints have to sit on.
///
/// A suitability mask says where metal would be *interesting*. It says nothing
/// about whether a mex can be built there, and on a generated map the two
/// disagree constantly: measured on the default 8×8 starter, six of fourteen
/// blind spots were on ground too steep to build on, and on an island graph
/// eleven of twelve were under water. Checking afterwards produces a list of
/// complaints; checking here produces a layout.
///
/// A candidate that does not fit is nudged outward in whole 16-elmo cells to
/// the nearest one that does — nearest first, so the spot stays where the mask
/// wanted it — and its symmetry images are re-derived from the nudged primary,
/// which keeps every group exactly fair. Both the primary and every image have
/// to fit: generated terrain is only as symmetric as the graph that made it.
pub fn propose_spots_on(
    mask: &SharedField,
    terrain: Option<&SharedField>,
    ctx: &Context,
    opts: &ProposeOptions,
) -> Vec<MetalSpot> {
    let r = mask.res;
    let w = ctx.elmos_x;
    let h = ctx.elmos_y;
    let sep2 = opts.min_separation * opts.min_separation;
    let ground = match (terrain, &opts.build) {
        (Some(t), Some(b)) => Some(BuildMask::new(t, ctx, b)),
        _ => None,
    };
    let fits = |x: f64, z: f64| match &ground {
        Some(g) => g.clear(x, z),
        None => true,
    };
    // No terrain means no reason to move a candidate, and the search would
    // only cost time and change results that are asserted bit-for-bit.
    let rings = if ground.is_some() {
        opts.search_cells.max(0)
    } else {
        0
    };

    let mut cands: Vec<(f64, usize, usize)> = Vec::new();
    for y in 1..r - 1 {
        for x in 1..r - 1 {
            let v = mask.get(y * r + x);
            if v >= opts.threshold {
                cands.push((v, x, y));
            }
        }
    }
    // Deterministic tiebreak, so the same mask always yields the same layout.
    cands.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.2 * r + a.1).cmp(&(b.2 * r + b.1)))
    });

    let mut picked: Vec<MetalSpot> = Vec::new();
    let rm1 = (r - 1) as f64;

    let in_bounds = |ex: f64, ez: f64| {
        ex >= opts.margin && ez >= opts.margin && ex <= w - opts.margin && ez <= h - opts.margin
    };
    let far_enough = |ex: f64, ez: f64, picked: &[MetalSpot], extra: &[(f64, f64)]| {
        for (ax, az) in &opts.avoid {
            let dx = ax - ex;
            let dz = az - ez;
            if dx * dx + dz * dz < sep2 {
                return false;
            }
        }
        for s in picked {
            let dx = s.x - ex;
            let dz = s.z - ez;
            if dx * dx + dz * dz < sep2 {
                return false;
            }
        }
        for (ax, az) in extra {
            let dx = ax - ex;
            let dz = az - ez;
            if dx * dx + dz * dz < sep2 {
                return false;
            }
        }
        true
    };

    // One symmetry group from one snapped primary, or nothing at all.
    let try_place = |mut px: f64, mut pz: f64, picked: &[MetalSpot]| {
        if !in_bounds(px, pz) || !far_enough(px, pz, picked, &[]) || !fits(px, pz) {
            return None;
        }

        let mut imgs = symmetry_images(px, pz, &opts.symmetry, w, h);
        // A candidate close enough to an axis to collide with its own mirror
        // is moved exactly onto the axis, or discarded.
        let mut self_collide = false;
        for (ix, iz) in &imgs {
            let sx = ix - px;
            let sz = iz - pz;
            if sx * sx + sz * sz < sep2 {
                self_collide = true;
                break;
            }
        }
        let mut on_axis = false;
        if self_collide {
            let proj = axis_project(px, pz, &opts.symmetry, w, h, opts.odd_footprint)?;
            px = proj.0;
            pz = proj.1;
            if !in_bounds(px, pz) || !far_enough(px, pz, picked, &[]) || !fits(px, pz) {
                return None;
            }
            imgs = symmetry_images(px, pz, &opts.symmetry, w, h);
            on_axis = true;
        }

        let mut group: Vec<(f64, f64)> = vec![(px, pz)];
        for (ix, iz) in imgs {
            let mut dup = false;
            for (gx, gz) in &group {
                let dx = gx - ix;
                let dz = gz - iz;
                if dx * dx + dz * dz < 1.0 {
                    dup = true; // exact axis image
                    break;
                }
            }
            if dup {
                continue;
            }
            if !in_bounds(ix, iz) || !far_enough(ix, iz, picked, &group) || !fits(ix, iz) {
                return None;
            }
            group.push((ix, iz));
        }
        Some((group, on_axis))
    };

    for (_v, cx, cy) in cands {
        if picked.len() >= opts.count {
            break;
        }
        // Snap the primary into the metal grid before anything else.
        let bx = snap_spot(cx as f64 / rm1 * w, opts.odd_footprint);
        let bz = snap_spot(cy as f64 / rm1 * h, opts.odd_footprint);

        // Rings of whole metal-grid cells outward from where the mask wanted
        // it, nearest first, so a nudged spot is the closest one that works.
        let mut found = None;
        'search: for k in 0..=rings {
            for dz in -k..=k {
                for dx in -k..=k {
                    if dx.abs() != k && dz.abs() != k {
                        continue; // interior of the ring, already tried
                    }
                    found = try_place(
                        bx + f64::from(dx) * Zk::METAL_GRID,
                        bz + f64::from(dz) * Zk::METAL_GRID,
                        &picked,
                    );
                    if found.is_some() {
                        break 'search;
                    }
                }
            }
        }
        let Some((group, on_axis)) = found else {
            continue;
        };
        if picked.len() + group.len() > opts.count {
            continue;
        }
        let single = on_axis || group.len() == 1;
        for (gx, gz) in group {
            let id = format!("s{}", picked.len() + 1);
            picked.push(MetalSpot {
                x: gx,
                z: gz,
                metal: round2(opts.amount),
                id,
                single: single && opts.symmetry != "none",
            });
        }
    }
    picked
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub level: Level,
    pub code: &'static str,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Validation {
    pub issues: Vec<Issue>,
    pub ok_count: usize,
    pub dropped_count: usize,
    pub merge_pairs: Vec<(usize, usize, f64)>,
    pub merge_certain_dist: f64,
    pub merge_window_dist: f64,
}

/// Validate a spot list against what Zero-K will actually do with it.
pub fn validate_spots(spots: &[MetalSpot], ctx: &Context, extractor_radius: f64) -> Validation {
    let r = extractor_radius;
    let w = ctx.elmos_x;
    let h = ctx.elmos_y;
    let mut issues = Vec::new();
    let certain = Zk::MERGE_CERTAIN.sqrt() * r;
    let window = Zk::MERGE_WINDOW.sqrt() * r;

    let dropped = spots.iter().filter(|s| !s.live()).count();
    if dropped > 0 {
        issues.push(Issue {
            level: Level::Error,
            code: "below-minimum",
            text: format!(
                "{dropped} spot(s) at or below {} metal — Zero-K discards these silently",
                Zk::MINIMUM_MEX_INCOME
            ),
        });
    }

    let oob = spots
        .iter()
        .filter(|s| s.x < 0.0 || s.z < 0.0 || s.x > w || s.z > h)
        .count();
    if oob > 0 {
        issues.push(Issue {
            level: Level::Error,
            code: "out-of-bounds",
            text: format!("{oob} spot(s) outside the map"),
        });
    }

    let mut will_merge = 0;
    let mut may_merge = 0;
    let mut pairs = Vec::new();
    for i in 0..spots.len() {
        for j in i + 1..spots.len() {
            let dx = spots[i].x - spots[j].x;
            let dz = spots[i].z - spots[j].z;
            let dist = (dx * dx + dz * dz).sqrt();
            if dist < certain {
                will_merge += 1;
                pairs.push((i, j, dist));
            } else if dist < window {
                may_merge += 1;
                pairs.push((i, j, dist));
            }
        }
    }
    if will_merge > 0 {
        issues.push(Issue {
            level: Level::Error,
            code: "merge-certain",
            text: format!(
                "{will_merge} pair(s) closer than {}e — Zero-K will merge them into single spots",
                certain.round()
            ),
        });
    }
    if may_merge > 0 {
        issues.push(Issue {
            level: Level::Warn,
            code: "merge-possible",
            text: format!(
                "{may_merge} pair(s) within {}e — may merge depending on metalmap integration",
                window.round()
            ),
        });
    }

    let live = spots.len() - dropped;
    if live > 0 && live < Zk::INDISCRETE_MIN_SPOTS {
        issues.push(Issue {
            level: Level::Warn,
            code: "indiscrete-risk",
            text: format!(
                "Only {live} spots. If the Lua config fails to load, Zero-K's fallback blob detection needs {}+ or it lets mexes be built anywhere",
                Zk::INDISCRETE_MIN_SPOTS
            ),
        });
    }

    Validation {
        issues,
        ok_count: live,
        dropped_count: dropped,
        merge_pairs: pairs,
        merge_certain_dist: certain,
        merge_window_dist: window,
    }
}

#[derive(Clone, Debug)]
pub struct SymmetryReport {
    pub symmetric: bool,
    pub unmatched: Vec<usize>,
}

/// Check a spot list is fair under a symmetry operator.
pub fn symmetry_report(spots: &[MetalSpot], ctx: &Context, mode: &str) -> SymmetryReport {
    if mode == "none" || spots.is_empty() {
        return SymmetryReport {
            symmetric: true,
            unmatched: Vec::new(),
        };
    }
    let w = ctx.elmos_x;
    let h = ctx.elmos_y;
    let tol = Zk::METAL_GRID * 1.5;
    let mut unmatched = Vec::new();
    for (i, s) in spots.iter().enumerate() {
        for (ix, iz) in symmetry_images(s.x, s.z, mode, w, h) {
            let found = spots.iter().any(|o| {
                let dx = o.x - ix;
                let dz = o.z - iz;
                (dx * dx + dz * dz).sqrt() <= tol && (o.metal - s.metal).abs() < 1e-6
            });
            if !found {
                unmatched.push(i);
                break;
            }
        }
    }
    SymmetryReport {
        symmetric: unmatched.is_empty(),
        unmatched,
    }
}

#[derive(Clone, Debug)]
pub struct RasterOptions {
    pub blob_radius: f64,
    pub max_metal: f64,
    pub scale_with_value: bool,
    pub full_value: bool,
}

impl Default for RasterOptions {
    fn default() -> Self {
        RasterOptions {
            blob_radius: 48.0,
            max_metal: 6.0,
            scale_with_value: true,
            full_value: true,
        }
    }
}

/// Paint **discrete blobs** into the metalmap raster.
///
/// A smooth or continuous metalmap merges into a handful of giant regions,
/// which trips ZK's `#metalSpots < 6` check and turns the map into
/// build-anywhere. Exporting a suitability mask directly is a serious bug:
/// measured, the raw mask painted 42.9% of the map; blobs paint 0.30%.
pub fn paint_metal_raster(
    spots: &[MetalSpot],
    ctx: &Context,
    w: usize,
    h: usize,
    opts: &RasterOptions,
) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    let ex_per_px_x = ctx.elmos_x / w as f64;
    let ex_per_px_y = ctx.elmos_y / h as f64;
    for sp in spots {
        if !sp.live() {
            continue;
        }
        let rad = if opts.scale_with_value {
            opts.blob_radius * (sp.metal / Zk::DEFAULT_MEX_INCOME).max(0.25).sqrt()
        } else {
            opts.blob_radius
        };
        let mut level = (255.0
            * (sp.metal / opts.max_metal * (opts.max_metal / Zk::DEFAULT_MEX_INCOME) / 3.0)
                .min(1.0))
        .round()
        .clamp(1.0, 255.0) as u8;
        if opts.full_value {
            level = 255;
        }
        let cx = sp.x / ex_per_px_x;
        let cy = sp.z / ex_per_px_y;
        let rx = rad / ex_per_px_x;
        let ry = rad / ex_per_px_y;
        let x0 = (cx - rx).floor().max(0.0) as usize;
        let x1 = ((cx + rx).ceil() as isize).min(w as isize - 1);
        let y0 = (cy - ry).floor().max(0.0) as usize;
        let y1 = ((cy + ry).ceil() as isize).min(h as isize - 1);
        if x1 < 0 || y1 < 0 {
            continue;
        }
        for y in y0..=(y1 as usize) {
            for x in x0..=(x1 as usize) {
                let dx = (x as f64 + 0.5 - cx) / rx;
                let dy = (y as f64 + 0.5 - cy) / ry;
                if dx * dx + dy * dy <= 1.0 {
                    let i = y * w + x;
                    if level > out[i] {
                        out[i] = level;
                    }
                }
            }
        }
    }
    out
}

/// Count discrete blobs the way ZK's detector roughly would.
pub fn count_blobs(raster: &[u8], w: usize, h: usize) -> usize {
    let mut seen = vec![false; w * h];
    let mut n = 0;
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..w * h {
        if raster[i] == 0 || seen[i] {
            continue;
        }
        n += 1;
        stack.clear();
        stack.push(i);
        seen[i] = true;
        while let Some(q) = stack.pop() {
            let qx = q % w;
            let qy = q / w;
            let mut nb: Vec<usize> = Vec::with_capacity(4);
            if qx > 0 {
                nb.push(q - 1);
            }
            if qx < w - 1 {
                nb.push(q + 1);
            }
            if qy > 0 {
                nb.push(q - w);
            }
            if qy < h - 1 {
                nb.push(q + w);
            }
            for k in nb {
                if raster[k] != 0 && !seen[k] {
                    seen[k] = true;
                    stack.push(k);
                }
            }
        }
    }
    n
}

/// Buildability of a mex footprint.
///
/// Every mapping tutorial's test loop is "place a mex, try to build on it".
/// The prototype placed spots from a suitability mask and never checked that
/// the footprint sits on flat, dry ground; this closes that gap.
#[derive(Clone, Debug)]
pub struct BuildabilityOptions {
    /// Mex footprint across, in elmos. ZK's staticmex is 4 build squares.
    pub footprint: f64,
    /// Steepest ground the footprint may sit on.
    pub max_slope_deg: f64,
    /// Normalised sea level; a spot below this is underwater.
    pub sea_level: f64,
}

impl Default for BuildabilityOptions {
    fn default() -> Self {
        BuildabilityOptions {
            footprint: 48.0,
            max_slope_deg: 12.0,
            sea_level: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildabilityReport {
    pub spot_id: String,
    pub max_slope_deg: f64,
    pub underwater: bool,
    pub buildable: bool,
}

pub fn check_buildability(
    spots: &[MetalSpot],
    height: &SharedField,
    ctx: &Context,
    opts: &BuildabilityOptions,
) -> Vec<BuildabilityReport> {
    spots
        .iter()
        .map(|s| {
            let f = footprint_at(height, ctx, s.x, s.z, opts);
            BuildabilityReport {
                spot_id: s.id.clone(),
                max_slope_deg: f.max_slope_deg,
                underwater: f.underwater,
                buildable: f.buildable(opts),
            }
        })
        .collect()
}

/// What a mex footprint sits on, in one pass over the footprint's samples.
#[derive(Clone, Copy, Debug)]
pub struct Footprint {
    pub max_slope_deg: f64,
    pub underwater: bool,
}

impl Footprint {
    pub fn buildable(&self, opts: &BuildabilityOptions) -> bool {
        !self.underwater && self.max_slope_deg <= opts.max_slope_deg
    }
}

/// Sample the ground under a footprint centred on an elmo coordinate.
///
/// Split out of [`check_buildability`] so placement can consult it *before*
/// committing to a spot rather than reporting on one afterwards.
pub fn footprint_at(
    height: &SharedField,
    ctx: &Context,
    x_elmo: f64,
    z_elmo: f64,
    opts: &BuildabilityOptions,
) -> Footprint {
    let r = height.res;
    let half_x = (opts.footprint / 2.0 * ctx.px_per_elmo_x())
        .round()
        .max(1.0) as isize;
    let half_z = (opts.footprint / 2.0 * ctx.px_per_elmo_y())
        .round()
        .max(1.0) as isize;
    let cx = (x_elmo * ctx.px_per_elmo_x()).round() as isize;
    let cz = (z_elmo * ctx.px_per_elmo_y()).round() as isize;
    let (hx, hy) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
    let mut worst = 0.0f64;
    let mut underwater = false;
    for dz in -half_z..=half_z {
        for dx in -half_x..=half_x {
            let x = (cx + dx).clamp(0, r as isize - 1) as usize;
            let y = (cz + dz).clamp(0, r as isize - 1) as usize;
            let deg = slope_degrees_aniso(height, x, y, r, ctx.height_range, hx, hy);
            if deg > worst {
                worst = deg;
            }
            if height.at(x, y) < opts.sea_level {
                underwater = true;
            }
        }
    }
    Footprint {
        max_slope_deg: worst,
        underwater,
    }
}

/// Where a mex footprint may go, as a summed-area table of unbuildable
/// lattice samples.
///
/// Placement asks this question thousands of times while it searches for a
/// spot, so the per-sample slope and water tests are done once and a footprint
/// query is four lookups whatever the footprint's size. The per-sample
/// predicate is the same one [`footprint_at`] uses, so the placer and the
/// report cannot disagree.
pub struct BuildMask {
    res: usize,
    px_per_elmo_x: f64,
    px_per_elmo_y: f64,
    half_x: isize,
    half_z: isize,
    /// `(res + 1)²` prefix sums of "this sample is unbuildable".
    sat: Vec<u32>,
}

impl BuildMask {
    /// The terrain's own resolution is used, not `ctx.res` — the placer runs
    /// its mask at 129 while the height field is the full vertex lattice.
    pub fn new(height: &SharedField, ctx: &Context, opts: &BuildabilityOptions) -> BuildMask {
        let r = height.res;
        let denom = (r - 1).max(1) as f64;
        let (hx, hy) = (ctx.elmos_x / denom, ctx.elmos_y / denom);
        let stride = r + 1;
        let mut sat = vec![0u32; stride * stride];
        for y in 0..r {
            let mut row = 0u32;
            for x in 0..r {
                let bad = height.at(x, y) < opts.sea_level
                    || slope_degrees_aniso(height, x, y, r, ctx.height_range, hx, hy)
                        > opts.max_slope_deg;
                row += u32::from(bad);
                sat[(y + 1) * stride + x + 1] = sat[y * stride + x + 1] + row;
            }
        }
        let px_per_elmo_x = denom / ctx.elmos_x;
        let px_per_elmo_y = denom / ctx.elmos_y;
        BuildMask {
            res: r,
            px_per_elmo_x,
            px_per_elmo_y,
            half_x: (opts.footprint / 2.0 * px_per_elmo_x).round().max(1.0) as isize,
            half_z: (opts.footprint / 2.0 * px_per_elmo_y).round().max(1.0) as isize,
            sat,
        }
    }

    /// Whether every sample under a footprint centred on this elmo coordinate
    /// is dry and flat enough to build on.
    pub fn clear(&self, x_elmo: f64, z_elmo: f64) -> bool {
        self.clear_box(x_elmo, z_elmo, self.half_x, self.half_z)
    }

    /// As [`BuildMask::clear`], for a square of arbitrary half-extent in elmos.
    /// Start positions want a whole base's worth of flat, not a mex footprint's.
    pub fn clear_within(&self, x_elmo: f64, z_elmo: f64, half_elmos: f64) -> bool {
        self.clear_box(
            x_elmo,
            z_elmo,
            (half_elmos * self.px_per_elmo_x).round().max(1.0) as isize,
            (half_elmos * self.px_per_elmo_y).round().max(1.0) as isize,
        )
    }

    /// As [`BuildMask::clear`], addressed by lattice sample rather than by
    /// elmo. A sweep over the whole map asks about every sample in turn, and
    /// converting each one out to elmos only to have `clear` round it back
    /// costs a rounding for nothing.
    pub fn clear_sample(&self, x: usize, y: usize) -> bool {
        self.clear_at(x as isize, y as isize, self.half_x, self.half_z)
    }

    /// The footprint half-extents in lattice samples, so a caller that walks
    /// the lattice knows how wide the mask's window is.
    pub fn half_extent(&self) -> (isize, isize) {
        (self.half_x, self.half_z)
    }

    fn clear_box(&self, x_elmo: f64, z_elmo: f64, half_x: isize, half_z: isize) -> bool {
        self.clear_at(
            (x_elmo * self.px_per_elmo_x).round() as isize,
            (z_elmo * self.px_per_elmo_y).round() as isize,
            half_x,
            half_z,
        )
    }

    fn clear_at(&self, cx: isize, cz: isize, half_x: isize, half_z: isize) -> bool {
        let last = self.res as isize - 1;
        let x0 = (cx - half_x).clamp(0, last);
        let x1 = (cx + half_x).clamp(0, last);
        let z0 = (cz - half_z).clamp(0, last);
        let z1 = (cz + half_z).clamp(0, last);
        let s = self.res + 1;
        let at = |x: isize, z: isize| self.sat[z as usize * s + x as usize];
        at(x1 + 1, z1 + 1) + at(x0, z0) == at(x0, z1 + 1) + at(x1 + 1, z0)
    }
}

/// Why a symmetry operator cannot be used on this world.
///
/// `rot90` and `diagonal` map the X extent onto the Y extent, so they are only
/// defined when the two are equal. Applying them to a 16×8 map sends every
/// image outside the world — silently, because the images are still numbers.
pub fn symmetry_rejection(mode: &str, ctx: &Context) -> Option<String> {
    if ctx.square_world() {
        return None;
    }
    match mode {
        "rot90" | "diagonal" | "rot72" => Some(format!(
            "{mode} symmetry turns the map about its centre, so it needs a square one. This one is {} × {} elmos — use mirrorX, mirrorY, quad or rot180.",
            ctx.elmos_x as u64, ctx.elmos_y as u64
        )),
        _ => None,
    }
}

/* ------------------------------------------------------------ hand edits */

/// How close two spots have to be to count as the same one.
///
/// Generous by half a grid cell: an image derived from a snapped primary is
/// exact, but a spot that came from an older file or a different symmetry may
/// be a rounding off.
const SAME_SPOT: f64 = Zk::METAL_GRID * 0.75;

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    let (dx, dz) = (a.0 - b.0, a.1 - b.1);
    (dx * dx + dz * dz).sqrt() <= SAME_SPOT
}

/// Whether a point is its own mirror under `mode` — that is, whether it sits
/// on the operator's fixed set.
///
/// Worth asking before placing anything. Every operator here fixes the map
/// centre, so a spot dropped there has no images at all and arrives alone; on
/// a fresh map that is exactly where the camera is looking, which made "add a
/// mex" appear to have no mirroring at all.
pub fn is_own_mirror(x: f64, z: f64, mode: &str, w: f64, h: f64) -> bool {
    let images = symmetry_images(x, z, mode, w, h);
    !images.is_empty() && images.iter().all(|img| near((x, z), *img))
}

/// How many distinct spots a symmetry group placed here would have.
///
/// Not simply one plus the image count: images collapse onto the point and
/// onto each other near a fixed set. Under `quad`, a point on the horizontal
/// mirror line but off the vertical one has three images and only two distinct
/// positions — a group half the size the operator promises, and an unfair map
/// if it goes unnoticed.
pub fn group_size(x: f64, z: f64, mode: &str, w: f64, h: f64) -> usize {
    let mut distinct = vec![(x, z)];
    for img in symmetry_images(x, z, mode, w, h) {
        if !distinct.iter().any(|p| near(*p, img)) {
            distinct.push(img);
        }
    }
    distinct.len()
}

/// A point near `at` where a symmetry group reaches the operator's full order.
///
/// Returns `at` unchanged when it already does, and `None` when nothing nearby
/// works. Diagonal offsets are tried first because they are what clears both
/// axes at once, which is what `quad` and `rot90` need — stepping off one axis
/// only leaves the point on the other and gives a half-sized group.
pub fn off_fixed_set(at: (f64, f64), mode: &str, w: f64, h: f64) -> Option<(f64, f64)> {
    let want = 1 + symmetry_images(at.0, at.1, mode, w, h).len();
    if want == 1 || group_size(at.0, at.1, mode, w, h) == want {
        return Some(at);
    }
    // Outward in eighths of the map. A sixteenth is inside the merge window on
    // a small map, so the group would collapse back into one spot anyway.
    for step in 1..=4 {
        let d = w.min(h) * 0.125 * f64::from(step);
        for (dx, dz) in [
            (d, d),
            (-d, d),
            (d, -d),
            (-d, -d),
            (d, 0.0),
            (0.0, d),
            (-d, 0.0),
            (0.0, -d),
        ] {
            let p = (
                snap_spot((at.0 + dx).clamp(0.0, w), false),
                snap_spot((at.1 + dz).clamp(0.0, h), false),
            );
            if group_size(p.0, p.1, mode, w, h) == want {
                return Some(p);
            }
        }
    }
    None
}

/// The indices that make up one symmetry group: the spot itself, plus any
/// spot sitting on one of its images.
///
/// Editing a mex without its images is how a map ends up unfair, and the
/// unfairness is invisible until someone measures it. So the group is the unit
/// of every hand edit, and `mirror` is what turns that off deliberately.
pub fn group_of(
    spots: &[MetalSpot],
    index: usize,
    mode: &str,
    w: f64,
    h: f64,
    mirror: bool,
) -> Vec<usize> {
    let mut out = vec![index];
    if !mirror || mode == "none" || index >= spots.len() {
        return out;
    }
    let here = (spots[index].x, spots[index].z);
    for img in symmetry_images(here.0, here.1, mode, w, h) {
        if near(img, here) {
            continue; // its own image: an on-axis spot
        }
        if let Some(j) = spots
            .iter()
            .enumerate()
            .find(|(j, s)| *j != index && !out.contains(j) && near((s.x, s.z), img))
            .map(|(j, _)| j)
        {
            out.push(j);
        }
    }
    out
}

/// Move a spot, taking its symmetry images with it.
///
/// The destination is snapped and then mirrored, in that order and for the
/// same reason placement is: the map width is a multiple of the 16-elmo grid,
/// so the mirror of a snapped point is grid-aligned too, and the group stays
/// exactly fair. Snapping each image on its own drifts them a cell apart.
pub fn move_group(
    spots: &mut [MetalSpot],
    index: usize,
    to: (f64, f64),
    mode: &str,
    w: f64,
    h: f64,
    mirror: bool,
) {
    if index >= spots.len() {
        return;
    }
    let px = snap_spot(to.0.clamp(0.0, w), false);
    let pz = snap_spot(to.1.clamp(0.0, h), false);
    let group = group_of(spots, index, mode, w, h, mirror);
    let imgs = symmetry_images(px, pz, mode, w, h);
    spots[index].x = px;
    spots[index].z = pz;
    // Images in the order `symmetry_images` emits them, which is the order
    // `group_of` found them in.
    for (k, j) in group.iter().skip(1).enumerate() {
        if let Some((ix, iz)) = imgs.get(k) {
            spots[*j].x = *ix;
            spots[*j].z = *iz;
        }
    }
}

/// Set a spot's metal, and its images' with it — an unequal pair is unfair
/// however carefully it is placed.
pub fn set_group_metal(
    spots: &mut [MetalSpot],
    index: usize,
    metal: f64,
    mode: &str,
    w: f64,
    h: f64,
    mirror: bool,
) {
    let v = round2(metal.max(0.0));
    for j in group_of(spots, index, mode, w, h, mirror) {
        spots[j].metal = v;
    }
}

/// Remove a spot and its images.
pub fn delete_group(
    spots: &mut Vec<MetalSpot>,
    index: usize,
    mode: &str,
    w: f64,
    h: f64,
    mirror: bool,
) {
    let mut group = group_of(spots, index, mode, w, h, mirror);
    group.sort_unstable();
    for j in group.into_iter().rev() {
        if j < spots.len() {
            spots.remove(j);
        }
    }
    renumber(spots);
}

/// Add a spot and its images. Returns the index of the primary.
pub fn add_group(
    spots: &mut Vec<MetalSpot>,
    at: (f64, f64),
    metal: f64,
    mode: &str,
    w: f64,
    h: f64,
    mirror: bool,
) -> usize {
    let px = snap_spot(at.0.clamp(0.0, w), false);
    let pz = snap_spot(at.1.clamp(0.0, h), false);
    let primary = spots.len();
    let mut placed = vec![(px, pz)];
    if mirror {
        for img in symmetry_images(px, pz, mode, w, h) {
            if !placed.iter().any(|p| near(*p, img)) {
                placed.push(img);
            }
        }
    }
    let single = placed.len() == 1;
    for (x, z) in placed {
        spots.push(MetalSpot {
            x,
            z,
            metal: round2(metal),
            id: String::new(),
            single: single && mode != "none",
        });
    }
    renumber(spots);
    primary
}

/// Ids follow position in the list, so a hand-edited layout reads the same
/// way a proposed one does.
pub fn renumber(spots: &mut [MetalSpot]) {
    for (i, s) in spots.iter_mut().enumerate() {
        s.id = format!("s{}", i + 1);
    }
}

/// Total metal each player's share of the map is worth. IceXuick targets
/// roughly 15 metal per player.
pub fn metal_per_player(spots: &[MetalSpot], players: usize) -> f64 {
    if players == 0 {
        return 0.0;
    }
    let total: f64 = spots.iter().filter(|s| s.live()).map(|s| s.metal).sum();
    total / players as f64
}

/// Geothermal vents. Placed like mexes but never clustered — geos next to each
/// other are a known map-design mistake.
#[allow(clippy::too_many_arguments)]
pub fn propose_geo_vents(
    mask: &SharedField,
    terrain: Option<&SharedField>,
    ctx: &Context,
    count: usize,
    min_separation: f64,
    symmetry: &str,
    build: Option<BuildabilityOptions>,
    avoid: &[(f64, f64)],
) -> Vec<MetalSpot> {
    let opts = ProposeOptions {
        count,
        // Geos want to be further apart than mexes.
        min_separation: min_separation.max(900.0),
        symmetry: symmetry.to_string(),
        threshold: 0.35,
        amount: 0.0,
        odd_footprint: false,
        margin: Zk::METAL_GRID * 20.0,
        // A vent under water or on a cliff cannot be built on either.
        build,
        search_cells: 16,
        avoid: avoid.to_vec(),
    };
    let mut out = propose_spots_on(mask, terrain, ctx, &opts);
    for (i, s) in out.iter_mut().enumerate() {
        s.id = format!("geo{}", i + 1);
        s.metal = 0.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use std::sync::Arc;

    #[test]
    fn snapping_is_not_mirror_symmetric() {
        // The documented case: snap(920) = 928 while the mirror lands 16 off.
        assert_eq!(snap_spot(920.0, false), 928.0);
        assert_eq!(snap_spot(5120.0 - 920.0, false), 4208.0);
        assert_eq!(5120.0 - snap_spot(920.0, false), 4192.0);
        // Odd footprints land on cell centres instead.
        assert_eq!(snap_spot(920.0, true), 920.0);
        assert_eq!(snap_spot(921.0, true), 920.0);
    }

    #[test]
    fn snapped_primaries_mirror_exactly() {
        // Map width is a multiple of 512, hence of 16, so mirroring a snapped
        // point keeps it grid-aligned and the group stays fair.
        let w = 6144.0;
        for v in [0.0, 17.0, 383.0, 920.0, 3000.0, 6100.0] {
            let s = snap_spot(v, false);
            assert_eq!((w - s) % Zk::METAL_GRID, 0.0);
        }
    }

    #[test]
    fn near_axis_candidates_project_or_are_rejected() {
        let w = 6144.0;
        // A point near the vertical mirror line projects onto it.
        let p = axis_project(3000.0, 1000.0, "mirrorX", w, w, false).unwrap();
        assert_eq!(p.0, 3072.0);
        let imgs = symmetry_images(p.0, p.1, "mirrorX", w, w);
        assert_eq!(imgs[0], (p.0, p.1));
        // rot90 has only the centre as a fixed point.
        let c = axis_project(100.0, 200.0, "rot90", w, w, false).unwrap();
        assert_eq!(c, (3072.0, 3072.0));
    }

    #[test]
    fn merge_thresholds_follow_the_gadget() {
        let project = Project::default();
        let ctx = Context::new(&project, 65);
        let spots = vec![
            MetalSpot::new(1000.0, 1000.0, 2.0, "a"),
            MetalSpot::new(1100.0, 1000.0, 2.0, "b"),
        ];
        let v = validate_spots(&spots, &ctx, 100.0);
        // 100 elmos apart with R = 100: under sqrt(1.7)*100 = 130, certain merge.
        assert!(v.issues.iter().any(|i| i.code == "merge-certain"));
        let spots = vec![
            MetalSpot::new(1000.0, 1000.0, 2.0, "a"),
            MetalSpot::new(1180.0, 1000.0, 2.0, "b"),
        ];
        let v = validate_spots(&spots, &ctx, 100.0);
        assert!(v.issues.iter().any(|i| i.code == "merge-possible"));
        assert!(!v.issues.iter().any(|i| i.code == "merge-certain"));
    }

    #[test]
    fn spots_at_or_below_the_minimum_are_flagged() {
        let project = Project::default();
        let ctx = Context::new(&project, 65);
        let spots = vec![MetalSpot::new(1000.0, 1000.0, 0.2, "a")];
        let v = validate_spots(&spots, &ctx, 100.0);
        assert_eq!(v.dropped_count, 1);
        assert!(v.issues.iter().any(|i| i.code == "below-minimum"));
    }

    #[test]
    fn blob_painting_stays_discrete() {
        let project = Project::default();
        let ctx = Context::new(&project, 129);
        let spots: Vec<MetalSpot> = (0..8)
            .map(|i| MetalSpot::new(700.0 + i as f64 * 600.0, 1200.0, 2.0, format!("s{i}")))
            .collect();
        let d = crate::spring::derive(12, 12);
        let ras = paint_metal_raster(
            &spots,
            &ctx,
            d.metal_w as usize,
            d.metal_h as usize,
            &RasterOptions::default(),
        );
        assert_eq!(count_blobs(&ras, d.metal_w as usize, d.metal_h as usize), 8);
        let painted = ras.iter().filter(|v| **v > 0).count() as f64;
        let frac = painted / (d.metal_w * d.metal_h) as f64;
        assert!(frac < 0.02, "blobs painted {:.3}% of the map", frac * 100.0);
    }

    #[test]
    fn hand_edits_keep_the_group_fair() {
        let project = Project {
            mex_sym: "rot180".into(),
            ..Default::default()
        };
        let ctx = Context::new(&project, 129);
        let (w, h) = (ctx.elmos_x, ctx.elmos_y);
        let mut spots = Vec::new();
        let primary = add_group(&mut spots, (1000.0, 1400.0), 2.0, "rot180", w, h, true);
        assert_eq!(spots.len(), 2, "rot180 places a pair");
        assert_eq!(spots[primary].id, "s1");
        // Snapped, then mirrored: both land on the 16-elmo grid.
        for s in &spots {
            assert_eq!(s.x % Zk::METAL_GRID, 0.0);
            assert_eq!(s.z % Zk::METAL_GRID, 0.0);
        }
        assert!(symmetry_report(&spots, &ctx, "rot180").symmetric);

        // Dragging the primary drags its image with it.
        move_group(&mut spots, primary, (2333.0, 900.0), "rot180", w, h, true);
        assert_eq!((spots[0].x, spots[0].z), (2336.0, 896.0));
        assert_eq!((spots[1].x, spots[1].z), (w - 2336.0, h - 896.0));
        assert!(symmetry_report(&spots, &ctx, "rot180").symmetric);

        // So does setting its value: an unequal pair is unfair however
        // carefully it is placed.
        set_group_metal(&mut spots, 0, 3.5, "rot180", w, h, true);
        assert_eq!(
            spots.iter().map(|s| s.metal).collect::<Vec<_>>(),
            [3.5, 3.5]
        );

        // And deleting takes the pair.
        add_group(&mut spots, (700.0, 700.0), 2.0, "rot180", w, h, true);
        assert_eq!(spots.len(), 4);
        delete_group(&mut spots, 0, "rot180", w, h, true);
        assert_eq!(spots.len(), 2);
        assert_eq!(spots[0].id, "s1", "ids follow position after a delete");
        assert!(symmetry_report(&spots, &ctx, "rot180").symmetric);
    }

    #[test]
    fn an_unmirrored_edit_moves_only_the_one_spot() {
        // The escape hatch: sometimes a map is deliberately not fair.
        let project = Project {
            mex_sym: "mirrorX".into(),
            ..Default::default()
        };
        let ctx = Context::new(&project, 129);
        let (w, h) = (ctx.elmos_x, ctx.elmos_y);
        let mut spots = Vec::new();
        add_group(&mut spots, (1024.0, 2048.0), 2.0, "mirrorX", w, h, true);
        let before = (spots[1].x, spots[1].z);
        move_group(&mut spots, 0, (1600.0, 2048.0), "mirrorX", w, h, false);
        assert_eq!(spots[0].x, 1600.0);
        assert_eq!((spots[1].x, spots[1].z), before, "the image must not move");
        assert!(!symmetry_report(&spots, &ctx, "mirrorX").symmetric);
    }

    #[test]
    fn an_on_axis_spot_is_not_duplicated() {
        let project = Project {
            mex_sym: "mirrorX".into(),
            ..Default::default()
        };
        let ctx = Context::new(&project, 129);
        let (w, h) = (ctx.elmos_x, ctx.elmos_y);
        let mut spots = Vec::new();
        // Dead centre on the mirror axis: its own image.
        add_group(&mut spots, (w / 2.0, 1024.0), 2.0, "mirrorX", w, h, true);
        assert_eq!(spots.len(), 1, "an on-axis spot is its own image");
        assert!(spots[0].single);
    }

    #[test]
    fn placement_refuses_ground_a_mex_cannot_stand_on() {
        let project = Project::default();
        let ctx = Context::new(&project, 129);
        // West half flooded, east half flat and dry. The mask says the whole
        // map is equally interesting, which is exactly the situation where a
        // suitability mask alone puts mexes in the sea.
        let mut f = crate::field::Field::gray(129);
        for y in 0..129 {
            for x in 0..129 {
                f.set(y * 129 + x, if x < 64 { 0.0 } else { 0.6 });
            }
        }
        let height: SharedField = Arc::new(f);
        let mut m = crate::field::Field::gray(129);
        m.fill(1.0);
        let mask: SharedField = Arc::new(m);
        let opts = ProposeOptions {
            count: 8,
            min_separation: 700.0,
            symmetry: "none".into(),
            build: Some(BuildabilityOptions {
                sea_level: 0.3,
                ..Default::default()
            }),
            ..Default::default()
        };

        let blind = propose_spots(&mask, &ctx, &opts);
        assert!(
            blind.iter().any(|s| s.x < 3072.0),
            "blind placement is supposed to use the flooded half"
        );

        let seeing = propose_spots_on(&mask, Some(&height), &ctx, &opts);
        assert!(!seeing.is_empty(), "the dry half has room for spots");
        for s in &seeing {
            assert!(s.x > 3072.0, "{} at x={} is under water", s.id, s.x);
        }
        // And the check that used to be a post-mortem now agrees with placement.
        let report = check_buildability(
            &seeing,
            &height,
            &ctx,
            &BuildabilityOptions {
                sea_level: 0.3,
                ..Default::default()
            },
        );
        assert!(report.iter().all(|b| b.buildable), "{report:?}");
    }

    #[test]
    fn a_non_square_world_keeps_every_spot_inside_it() {
        // 16x8: 8192 x 4096 elmos. Deriving Z from the X extent puts half the
        // layout past the south edge, and nothing downstream notices.
        let project = Project {
            units_x: 16,
            units_y: 8,
            mex_sym: "rot180".into(),
            ..Default::default()
        };
        let ctx = Context::new(&project, 129);
        assert_eq!((ctx.elmos_x, ctx.elmos_y), (8192.0, 4096.0));
        let mut f = crate::field::Field::gray(129);
        f.fill(1.0);
        let mask: SharedField = Arc::new(f);
        let spots = propose_spots(
            &mask,
            &ctx,
            &ProposeOptions {
                count: 14,
                min_separation: 700.0,
                symmetry: "rot180".into(),
                ..Default::default()
            },
        );
        assert!(!spots.is_empty());
        for s in &spots {
            assert!(
                s.z >= 0.0 && s.z <= ctx.elmos_y,
                "{} at z={} is outside a {}-deep map",
                s.id,
                s.z,
                ctx.elmos_y
            );
            assert!(
                s.x >= 0.0 && s.x <= ctx.elmos_x,
                "{} is off the east edge",
                s.id
            );
        }
        let v = validate_spots(&spots, &ctx, project.extractor_radius);
        assert!(
            !v.issues.iter().any(|i| i.code == "out-of-bounds"),
            "{:?}",
            v.issues
        );
        assert!(symmetry_report(&spots, &ctx, "rot180").symmetric);
    }

    #[test]
    fn five_fold_gives_a_spot_four_images_around_the_centre() {
        let (w, h) = (8192.0, 8192.0);
        let (x, z) = (6400.0, 4096.0);
        let imgs = symmetry_images(x, z, "rot72", w, h);
        assert_eq!(imgs.len(), 4, "five-fold means the spot plus four images");
        // Every image is the same distance from the centre as the original --
        // that is what makes the five players' metal equal.
        let r0 = ((x - w / 2.0).powi(2) + (z - h / 2.0).powi(2)).sqrt();
        for (ix, iz) in &imgs {
            let r = ((ix - w / 2.0).powi(2) + (iz - h / 2.0).powi(2)).sqrt();
            assert!(
                (r - r0).abs() < 1e-6,
                "an image sits {r} from the centre, the original {r0}"
            );
        }
        // And five turns come back to where they started.
        let (bx, bz) = rotate_about_centre(x, z, w, h, 5);
        assert!((bx - x).abs() < 1e-6 && (bz - z).abs() < 1e-6);
    }

    #[test]
    fn rotational_symmetry_is_refused_on_a_non_square_map() {
        let square = Context::new(&Project::default(), 129);
        assert!(symmetry_rejection("rot90", &square).is_none());
        let oblong = Context::new(
            &Project {
                units_x: 16,
                units_y: 8,
                ..Default::default()
            },
            129,
        );
        assert!(symmetry_rejection("rot180", &oblong).is_none());
        assert!(symmetry_rejection("quad", &oblong).is_none());
        for bad in ["rot90", "diagonal", "rot72"] {
            assert!(
                symmetry_rejection(bad, &oblong).unwrap().contains("square"),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn buildability_rejects_a_cliff() {
        let project = Project::default();
        let ctx = Context::new(&project, 129);
        let mut f = crate::field::Field::gray(129);
        for y in 0..129 {
            for x in 0..129 {
                // Left half flat, right half a steep ramp.
                f.set(
                    y * 129 + x,
                    if x < 64 {
                        0.5
                    } else {
                        0.5 + (x - 64) as f64 * 0.06
                    },
                );
            }
        }
        let height: SharedField = Arc::new(f);
        let flat = MetalSpot::new(1000.0, 3000.0, 2.0, "flat");
        let cliff = MetalSpot::new(5000.0, 3000.0, 2.0, "cliff");
        let r = check_buildability(
            &[flat, cliff],
            &height,
            &ctx,
            &BuildabilityOptions::default(),
        );
        assert!(r[0].buildable, "flat ground must be buildable");
        assert!(!r[1].buildable, "a ramp must not be");
    }

    /// The map centre is fixed by every operator here, so a spot dropped
    /// there is its own mirror and arrives alone.
    ///
    /// That is correct, and it is also where a freshly framed camera points —
    /// which made "add a mex" look like it had no mirroring at all, for every
    /// symmetry, on every new map.
    #[test]
    fn the_map_centre_is_its_own_mirror_under_every_operator() {
        let (w, h) = (6144.0, 6144.0);
        for mode in [
            "mirrorX", "mirrorY", "quad", "rot180", "rot90", "rot72", "diagonal",
        ] {
            assert!(
                is_own_mirror(w / 2.0, h / 2.0, mode, w, h),
                "{mode}: the centre should be its own mirror"
            );
            let mut spots: Vec<MetalSpot> = Vec::new();
            add_group(&mut spots, (w / 2.0, h / 2.0), 1.8, mode, w, h, true);
            assert_eq!(
                spots.len(),
                1,
                "{mode}: the centre is one spot, not a group"
            );

            // And somewhere off it makes a real group of the operator's order.
            let want = 1 + symmetry_images(1200.0, 900.0, mode, w, h).len();
            let mut group: Vec<MetalSpot> = Vec::new();
            add_group(&mut group, (1200.0, 900.0), 1.8, mode, w, h, true);
            assert_eq!(
                group.len(),
                want,
                "{mode}: off-centre group is the wrong size"
            );
        }
    }

    /// Nudging off the fixed set has to actually find somewhere, for every
    /// operator — otherwise "add a mex" on a fresh map still cannot mirror.
    #[test]
    fn a_point_can_always_be_moved_off_the_fixed_set() {
        let (w, h) = (6144.0, 6144.0);
        for mode in [
            "mirrorX", "mirrorY", "quad", "rot180", "rot90", "rot72", "diagonal",
        ] {
            let centre = (w / 2.0, h / 2.0);
            let p = off_fixed_set(centre, mode, w, h)
                .unwrap_or_else(|| panic!("{mode}: nowhere near the centre makes a group"));
            assert!(p != centre, "{mode}: it did not move");
            assert!(
                !is_own_mirror(p.0, p.1, mode, w, h),
                "{mode}: still its own mirror"
            );
            assert!(
                p.0 >= 0.0 && p.0 <= w && p.1 >= 0.0 && p.1 <= h,
                "{mode}: moved off the map to {p:?}"
            );
            // And a group placed there is the operator's full order.
            let mut spots: Vec<MetalSpot> = Vec::new();
            add_group(&mut spots, p, 1.8, mode, w, h, true);
            assert_eq!(
                spots.len(),
                1 + symmetry_images(p.0, p.1, mode, w, h).len(),
                "{mode}: the nudged point still did not make a full group"
            );
        }
        // A point that already works is left exactly where it is.
        assert_eq!(
            off_fixed_set((1200.0, 900.0), "mirrorX", w, h),
            Some((1200.0, 900.0))
        );
    }
}

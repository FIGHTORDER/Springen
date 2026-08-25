//! Playability probes.
//!
//! Generic terrain generators make pretty maps that play badly. Pathability,
//! chokepoints and metal fairness are the actual differentiator, so they are
//! measured rather than eyeballed.

use crate::fdlibm;
use crate::field::SharedField;
use crate::nodes::slope_degrees_aniso;
use crate::project::Context;

/// Per-unit-class climb limits, matching the shape of
/// `terrainTypes.moveSpeeds`. A single global slope threshold hides the fact
/// that a map can be fine for kbots and useless for tanks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitClass {
    Tank,
    Kbot,
    Hover,
    Ship,
}

impl UnitClass {
    pub const ALL: [UnitClass; 4] = [
        UnitClass::Tank,
        UnitClass::Kbot,
        UnitClass::Hover,
        UnitClass::Ship,
    ];
    pub fn name(self) -> &'static str {
        match self {
            UnitClass::Tank => "tank",
            UnitClass::Kbot => "kbot",
            UnitClass::Hover => "hover",
            UnitClass::Ship => "ship",
        }
    }
    /// Climb limits in degrees. Ships are the inverse case — they need water.
    pub fn max_slope_deg(self) -> f64 {
        match self {
            UnitClass::Tank => 18.0,
            UnitClass::Kbot => 32.0,
            UnitClass::Hover => 22.0,
            UnitClass::Ship => 90.0,
        }
    }
    pub fn needs_water(self) -> bool {
        matches!(self, UnitClass::Ship)
    }
}

#[derive(Clone, Debug)]
pub struct Pathability {
    pub passable_fraction: f64,
    pub largest_fraction: f64,
    pub component_count: usize,
    pub label: Vec<i32>,
    pub largest_id: i32,
    pub pass: Vec<u8>,
}

/// Flood fill over cells whose slope is under a tolerance, reporting the
/// traversable fraction, the largest connected region and how many regions
/// there are. Disconnected regions are the failure mode that blanket
/// flattening creates: mesas with hard rims are islands.
pub fn pathability(field: &SharedField, ctx: &Context, max_deg: f64, sea_t: f64) -> Pathability {
    pathability_for(field, ctx, max_deg, sea_t, false)
}

/// As [`pathability`], but `water` inverts the sea test for naval classes.
pub fn pathability_for(
    field: &SharedField,
    ctx: &Context,
    max_deg: f64,
    sea_t: f64,
    water: bool,
) -> Pathability {
    let r = field.res;
    let mut pass = vec![0u8; r * r];
    let tan_max = fdlibm::tan(max_deg * std::f64::consts::PI / 180.0);
    let vscale = ctx.height_range;
    // Per axis: a passability sweep that measured the lattice's gradient
    // rather than the ground's would call a 16x8 map's slopes half what they
    // are along Z. Equal on a square map.
    let (hstep_x, hstep_y) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
    let mut passable = 0usize;
    for y in 0..r {
        for x in 0..r {
            let i = y * r + x;
            let below = field.get(i) < sea_t;
            if below != water {
                continue;
            }
            let xl = if x > 0 { x - 1 } else { x };
            let xr = if x < r - 1 { x + 1 } else { x };
            let yu = if y > 0 { y - 1 } else { y };
            let yd = if y < r - 1 { y + 1 } else { y };
            let gx = (field.at(xr, y) - field.at(xl, y)) * vscale / ((xr - xl) as f64 * hstep_x);
            let gy = (field.at(x, yd) - field.at(x, yu)) * vscale / ((yd - yu) as f64 * hstep_y);
            if (gx * gx + gy * gy).sqrt() <= tan_max {
                pass[i] = 1;
                passable += 1;
            }
        }
    }

    let mut label = vec![0i32; r * r];
    let mut best = 0usize;
    let mut best_id = 0i32;
    let mut cur = 0i32;
    let mut stack: Vec<usize> = Vec::new();
    for s in 0..r * r {
        if pass[s] == 0 || label[s] != 0 {
            continue;
        }
        cur += 1;
        let mut size = 0usize;
        stack.clear();
        stack.push(s);
        label[s] = cur;
        while let Some(q) = stack.pop() {
            size += 1;
            let qx = q % r;
            let qy = q / r;
            let mut nb: Vec<usize> = Vec::with_capacity(4);
            if qx > 0 {
                nb.push(q - 1);
            }
            if qx < r - 1 {
                nb.push(q + 1);
            }
            if qy > 0 {
                nb.push(q - r);
            }
            if qy < r - 1 {
                nb.push(q + r);
            }
            for n in nb {
                if pass[n] != 0 && label[n] == 0 {
                    label[n] = cur;
                    stack.push(n);
                }
            }
        }
        if size > best {
            best = size;
            best_id = cur;
        }
    }

    let total = (r * r) as f64;
    Pathability {
        passable_fraction: passable as f64 / total,
        largest_fraction: best as f64 / total,
        component_count: cur as usize,
        label,
        largest_id: best_id,
        pass,
    }
}

/// How wide the ground you can actually move through is, and where it pinches.
///
/// Connectivity says whether two places are joined; it does not say whether an
/// army fits. A map can be 90% traversable in one region and still be decided
/// by a single 60-elmo gap, and that gap is the map.
#[derive(Clone, Debug)]
pub struct Choke {
    /// Full corridor width at every sample, in elmos: twice the distance to
    /// the nearest impassable ground. Zero where impassable.
    pub width: Vec<f64>,
    /// The best route across the largest region is this wide at its narrowest.
    /// This is the number that decides whether the map plays open or funnelled.
    pub bottleneck: f64,
    /// Where that narrowest point is, in elmos.
    pub bottleneck_at: (f64, f64),
    /// Which way the reported route runs — the more constrained of the two.
    pub axis: &'static str,
    /// Each axis on its own, because "the more constrained" is not always the
    /// one that matters. A map built as a west-east corridor with ranges up
    /// the flanks is *supposed* to pinch north-south, and reporting that as
    /// the map's bottleneck calls a deliberate design a funnel. Which axis
    /// play runs along is a question about the team layout, so the caller —
    /// which knows the symmetry — decides. Zero means no route that way.
    pub west_east: f64,
    pub north_south: f64,
    /// Corridor width over the largest region, in elmos.
    pub p10: f64,
    pub median: f64,
    /// The 90th percentile, which is what open ground on this map looks like.
    /// Used to scale the overlay: normalising by the median would saturate
    /// half the map and show no gradient at all.
    pub p90: f64,
}

impl Choke {
    /// The corridor along the axis the teams are laid out on, given the map's
    /// symmetry — which is the one an attack actually has to cross.
    ///
    /// Four-way and five-way layouts have no single answer, so they get the
    /// more constrained of the two.
    pub fn along_play_axis(&self, symmetry: &str) -> (f64, &'static str) {
        match symmetry {
            "mirrorX" => (self.west_east, "west-east"),
            "mirrorY" => (self.north_south, "north-south"),
            _ => (self.bottleneck, self.axis),
        }
    }
}

/// Exact squared Euclidean distance transform, one axis at a time.
///
/// Felzenszwalb's lower envelope of parabolas rather than a chamfer sweep: a
/// chamfer metric is a few percent out in the diagonal directions, and a few
/// percent on a corridor width is the difference between a pass an army fits
/// through and one it does not. `step` is elmos per sample along this axis, so
/// the result is in elmos² and a non-square world measures correctly.
fn edt_1d(f: &[f64], step: f64) -> (Vec<f64>, Vec<usize>) {
    let n = f.len();
    let mut d = vec![0.0f64; n];
    // Which sample along this axis each result came from. Distance alone
    // cannot tell a corridor from open ground beside a wall; knowing *which*
    // obstacle is nearest can.
    let mut src = vec![0usize; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let w2 = step * step;
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            // Where the parabolas rooted at p and q cross.
            let s = ((f[q] + w2 * (q * q) as f64) - (f[p] + w2 * (p * p) as f64))
                / (2.0 * w2 * (q as f64 - p as f64));
            if s <= z[k] {
                if k == 0 {
                    v[0] = q;
                    z[0] = f64::NEG_INFINITY;
                    z[1] = f64::INFINITY;
                    break;
                }
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
                break;
            }
        }
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let dq = q as f64 - p as f64;
        d[q] = w2 * dq * dq + f[p];
        src[q] = p;
    }
    (d, src)
}

/// Distance from every passable sample to the nearest impassable one, in
/// elmos, and which sample that is.
///
/// The second half is what makes a chokepoint findable. Open ground running
/// alongside a long wall is exactly as far from an obstacle as the gap through
/// that wall is, so distance alone cannot tell them apart. Knowing *which*
/// obstacle is nearest can: in a gap, neighbouring samples answer with
/// obstacles on opposite sides.
fn distance_to_edge(pass: &[u8], r: usize, ex: f64, ey: f64) -> (Vec<f64>, Vec<usize>) {
    const FAR: f64 = 1e18;
    // Off-map counts as impassable: a corridor pinned against the south edge
    // is bounded by the edge, not open through it.
    let mut d: Vec<f64> = pass
        .iter()
        .map(|p| if *p == 0 { 0.0 } else { FAR })
        .collect();
    let mut src_y = vec![0usize; r * r];
    let mut col = vec![0.0f64; r];
    for x in 0..r {
        for y in 0..r {
            col[y] = d[y * r + x];
        }
        let (t, sy) = edt_1d(&col, ey);
        for y in 0..r {
            d[y * r + x] = t[y];
            src_y[y * r + x] = sy[y];
        }
    }
    let mut feat = vec![0usize; r * r];
    let mut row = vec![0.0f64; r];
    for y in 0..r {
        row.copy_from_slice(&d[y * r..y * r + r]);
        let (t, sx) = edt_1d(&row, ex);
        for x in 0..r {
            d[y * r + x] = t[x].sqrt();
            // The column pass answered per column, so the row the nearest
            // obstacle sits in is the one recorded for the column chosen here.
            feat[y * r + x] = src_y[y * r + sx[x]] * r + sx[x];
        }
    }
    (d, feat)
}

/// The widest route across the largest traversable region, and the neck that
/// limits it.
///
/// The route is a max-min path: of all the ways across, the one whose
/// narrowest point is widest. That is the corridor an attack would actually
/// use, and its narrowest point is the one worth knowing about — an average
/// width would be dominated by the open ground either side of the pass and
/// would report a funnel as a plain.
pub fn chokepoints(field: &SharedField, ctx: &Context, max_deg: f64, sea_t: f64) -> Choke {
    let r = field.res;
    let p = pathability_for(field, ctx, max_deg, sea_t, false);
    let (ex, ey) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
    let (dist, feat) = distance_to_edge(&p.pass, r, ex, ey);
    let width: Vec<f64> = dist.iter().map(|d| d * 2.0).collect();
    let at = |i: usize| ((i % r) as f64 * ex, (i / r) as f64 * ey);

    let inside: Vec<usize> = (0..r * r)
        .filter(|i| p.largest_id != 0 && p.label[*i] == p.largest_id)
        .collect();
    if inside.len() < 2 {
        return Choke {
            width,
            bottleneck: 0.0,
            bottleneck_at: (0.0, 0.0),
            axis: "neither",
            west_east: 0.0,
            north_south: 0.0,
            p10: 0.0,
            median: 0.0,
            p90: 0.0,
        };
    }

    let mut w: Vec<f64> = inside.iter().map(|i| width[*i]).collect();
    w.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p10 = w[w.len() / 10];
    let median = w[w.len() / 2];
    let p90 = w[w.len() * 9 / 10];

    // Which sides of the map the route has to join.
    //
    // Not the region's topologically most distant pair, which was the first
    // thing tried and was wrong: a region's extremes are the tips of its
    // thinnest dead-end gullies, so every route between them is pinched by a
    // gully mouth and four sample maps in a row reported a bottleneck of one
    // lattice step. What a map is asked is whether an army can cross it, so
    // the route runs edge to edge, and the answer is the more constrained of
    // the two axes.
    let edge = (r / 20).max(1);
    let mine = |i: usize| p.label[i] == p.largest_id;

    // Max-min widest path, multi-source: `best[c]` is the widest corridor any
    // route from the starting edge to `c` manages, measured at its narrowest.
    // Dijkstra with `max` as the relaxation and a max-heap.
    use std::collections::BinaryHeap;
    let key = |v: f64| (v * 1e6) as i64;
    let run = |src: &dyn Fn(usize, usize) -> bool,
               dst: &dyn Fn(usize, usize) -> bool|
     -> (f64, Vec<usize>) {
        let mut best = vec![f64::NEG_INFINITY; r * r];
        let mut from = vec![usize::MAX; r * r];
        let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
        for i in 0..r * r {
            if mine(i) && src(i % r, i / r) {
                best[i] = width[i];
                heap.push((key(width[i]), i));
            }
        }
        let mut end = usize::MAX;
        while let Some((k, c)) = heap.pop() {
            if k < key(best[c]) {
                continue;
            }
            if dst(c % r, c / r) {
                end = c;
                break;
            }
            let (cx, cy) = (c % r, c / r);
            let mut nb = [usize::MAX; 4];
            if cx > 0 {
                nb[0] = c - 1;
            }
            if cx < r - 1 {
                nb[1] = c + 1;
            }
            if cy > 0 {
                nb[2] = c - r;
            }
            if cy < r - 1 {
                nb[3] = c + r;
            }
            for n in nb {
                if n == usize::MAX || !mine(n) {
                    continue;
                }
                let cand = best[c].min(width[n]);
                if cand > best[n] {
                    best[n] = cand;
                    from[n] = c;
                    heap.push((key(cand), n));
                }
            }
        }
        if end == usize::MAX {
            return (0.0, Vec::new());
        }
        let mut route = Vec::new();
        let mut c = end;
        while c != usize::MAX {
            route.push(c);
            c = from[c];
        }
        (best[end], route)
    };

    let (w_we, r_we) = run(&|x, _| x < edge, &|x, _| x >= r - edge);
    let (w_ns, r_ns) = run(&|_, y| y < edge, &|_, y| y >= r - edge);
    // A route that does not exist is not a narrow route; it is the other axis'
    // problem, and only counts if neither crosses.
    let (west_east, north_south) = (w_we, w_ns);
    let (bottleneck, route, axis) = match (w_we > 0.0, w_ns > 0.0) {
        (true, true) if w_we <= w_ns => (w_we, r_we, "west-east"),
        (true, true) => (w_ns, r_ns, "north-south"),
        (true, false) => (w_we, r_we, "west-east"),
        (false, true) => (w_ns, r_ns, "north-south"),
        (false, false) => (0.0, Vec::new(), "neither"),
    };

    const EPS: f64 = 1e-9;
    // Squeezed from two sides: some neighbour's nearest obstacle is most of a
    // corridor's width away from this sample's own. Beside a wall every
    // neighbour answers with the same stretch of wall and this is false; in a
    // gap they answer with the two sides of it and it is true.
    let squeezed = |c: usize| {
        let (cx, cy) = (c % r, c / r);
        let f0 = feat[c];
        let (fx, fy) = ((f0 % r) as f64 * ex, (f0 / r) as f64 * ey);
        let mut nb = [usize::MAX; 4];
        if cx > 0 {
            nb[0] = c - 1;
        }
        if cx < r - 1 {
            nb[1] = c + 1;
        }
        if cy > 0 {
            nb[2] = c - r;
        }
        if cy < r - 1 {
            nb[3] = c + r;
        }
        nb.iter().any(|n| {
            if *n == usize::MAX || p.pass[*n] == 0 {
                return false;
            }
            let g = feat[*n];
            let (gx, gy) = ((g % r) as f64 * ex, (g / r) as f64 * ey);
            let apart = ((gx - fx).powi(2) + (gy - fy).powi(2)).sqrt();
            apart > width[c] * 0.75
        })
    };
    let dips: Vec<usize> = (1..route.len().saturating_sub(1))
        .filter(|k| {
            let c = route[*k];
            width[c] <= bottleneck + EPS && squeezed(c)
        })
        .map(|k| route[k])
        .collect();
    let neck = if !dips.is_empty() {
        // A neck is a stretch of corridor rather than a point; report its
        // middle.
        dips[dips.len() / 2]
    } else {
        route
            .iter()
            .copied()
            .min_by(|a, b| width[*a].partial_cmp(&width[*b]).unwrap())
            .unwrap_or(0)
    };

    Choke {
        width,
        bottleneck,
        bottleneck_at: at(neck),
        axis,
        west_east,
        north_south,
        p10,
        median,
        p90,
    }
}

#[derive(Clone, Debug)]
pub struct ClassReport {
    pub class: UnitClass,
    pub passable_fraction: f64,
    pub largest_fraction: f64,
    pub component_count: usize,
}

/// Run the probe once per unit class.
pub fn pathability_by_class(field: &SharedField, ctx: &Context, sea_t: f64) -> Vec<ClassReport> {
    UnitClass::ALL
        .iter()
        .map(|c| {
            let p = pathability_for(field, ctx, c.max_slope_deg(), sea_t, c.needs_water());
            ClassReport {
                class: *c,
                passable_fraction: p.passable_fraction,
                largest_fraction: p.largest_fraction,
                component_count: p.component_count,
            }
        })
        .collect()
}

/* ------------------------------------------------------------- flatness */

/// How much of a map you can actually put a building on.
///
/// Pathability answers "can a unit cross this"; that is a different question
/// from "can a factory stand here", and a plains map is judged on the second.
/// The engine's own test is a height *spread* under the footprint rather than
/// a slope at a point, so this measures the footprint, not the texel — a field
/// of 40-elmo hummocks is passable everywhere and buildable nowhere, and a
/// per-texel slope average cannot tell you that.
#[derive(Clone, Debug)]
pub struct Flatness {
    /// The building footprint the sweep used, in elmos.
    pub footprint: f64,
    /// The slope limit each sample under a footprint had to be under.
    pub max_slope_deg: f64,
    /// Fraction of the map above the waterline.
    pub land_fraction: f64,
    /// Fraction of the whole map that is dry and flat enough to build on.
    pub buildable_fraction: f64,
    /// The same, as a fraction of the land rather than of the map. This is the
    /// number to watch while flattening: raising the sea hides steep ground
    /// instead of fixing it, and only this one notices.
    pub buildable_of_land: f64,
    /// The largest single connected plain, as a fraction of the map. Blanket
    /// flattening tends to make many small plains separated by the rims it
    /// steepens, and one big plain plays very differently from thirty.
    pub largest_plain_fraction: f64,
    pub plain_count: usize,
    /// Slope percentiles over land, in degrees.
    pub median_slope_deg: f64,
    pub p90_slope_deg: f64,
    /// Height spread over land in elmos, 1st to 99th percentile. The tails are
    /// trimmed because one spire should not describe the whole map.
    pub relief_elmos: f64,
}

/// Sweep the whole map with the buildability test used for metal spots.
///
/// `sea_t` is in field units, the same normalised height everything else
/// compares against.
pub fn flatness(
    field: &SharedField,
    ctx: &Context,
    footprint: f64,
    max_slope_deg: f64,
    sea_t: f64,
) -> Flatness {
    let opts = crate::zk::BuildabilityOptions {
        footprint,
        max_slope_deg,
        sea_level: sea_t,
    };
    let mask = crate::zk::BuildMask::new(field, ctx, &opts);
    let r = field.res;
    let denom = (r - 1).max(1) as f64;
    let (hx, hy) = (ctx.elmos_x / denom, ctx.elmos_y / denom);

    let mut build = vec![0u8; r * r];
    let mut buildable = 0usize;
    let mut land = 0usize;
    let mut slopes: Vec<f64> = Vec::new();
    let mut heights: Vec<f64> = Vec::new();
    for y in 0..r {
        for x in 0..r {
            let h = field.at(x, y);
            if h < sea_t {
                continue;
            }
            land += 1;
            heights.push(h);
            slopes.push(slope_degrees_aniso(
                field,
                x,
                y,
                r,
                ctx.height_range,
                hx,
                hy,
            ));
            if mask.clear_sample(x, y) {
                build[y * r + x] = 1;
                buildable += 1;
            }
        }
    }

    let (largest, count) = largest_component(&build, r);
    let total = (r * r) as f64;
    let pct = |v: &mut Vec<f64>, q: f64| -> f64 {
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let i = ((v.len() - 1) as f64 * q).round() as usize;
        v[i]
    };
    let hi = pct(&mut heights, 0.99);
    let lo = pct(&mut heights, 0.01);

    Flatness {
        footprint,
        max_slope_deg,
        land_fraction: land as f64 / total,
        buildable_fraction: buildable as f64 / total,
        buildable_of_land: if land == 0 {
            0.0
        } else {
            buildable as f64 / land as f64
        },
        largest_plain_fraction: largest as f64 / total,
        plain_count: count,
        median_slope_deg: pct(&mut slopes, 0.5),
        p90_slope_deg: pct(&mut slopes, 0.9),
        relief_elmos: (hi - lo) * ctx.height_range,
    }
}

/// Size of the largest 4-connected run of set cells, and how many runs there
/// are.
fn largest_component(mask: &[u8], r: usize) -> (usize, usize) {
    let mut seen = vec![false; mask.len()];
    let mut stack: Vec<usize> = Vec::new();
    let (mut best, mut count) = (0usize, 0usize);
    for s in 0..mask.len() {
        if mask[s] == 0 || seen[s] {
            continue;
        }
        count += 1;
        let mut size = 0usize;
        seen[s] = true;
        stack.clear();
        stack.push(s);
        while let Some(q) = stack.pop() {
            size += 1;
            let (qx, qy) = (q % r, q / r);
            let mut nb: Vec<usize> = Vec::with_capacity(4);
            if qx > 0 {
                nb.push(q - 1);
            }
            if qx < r - 1 {
                nb.push(q + 1);
            }
            if qy > 0 {
                nb.push(q - r);
            }
            if qy < r - 1 {
                nb.push(q + r);
            }
            for n in nb {
                if mask[n] != 0 && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        best = best.max(size);
    }
    (best, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;
    use crate::project::Project;
    use std::sync::Arc;

    #[test]
    fn a_flat_plane_is_one_region() {
        let project = Project::default();
        let ctx = Context::new(&project, 65);
        let mut f = Field::gray(65);
        f.fill(0.5);
        let shared: SharedField = Arc::new(f);
        let p = pathability(&shared, &ctx, 20.0, 0.0);
        assert_eq!(p.component_count, 1);
        assert!((p.passable_fraction - 1.0).abs() < 1e-9);
        assert!((p.largest_fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_ridge_splits_the_map_in_two() {
        let project = Project::default();
        let ctx = Context::new(&project, 65);
        let mut f = Field::gray(65);
        for y in 0..65 {
            for x in 0..65 {
                // A single impassably steep column down the middle.
                f.set(y * 65 + x, if x == 32 { 1.0 } else { 0.0 });
            }
        }
        let shared: SharedField = Arc::new(f);
        let p = pathability(&shared, &ctx, 10.0, -1.0);
        assert!(
            p.component_count >= 2,
            "an impassable ridge must disconnect the map"
        );
        assert!(p.largest_fraction < p.passable_fraction);
    }

    #[test]
    fn a_flat_plane_is_buildable_everywhere() {
        let project = Project::default();
        let ctx = Context::new(&project, 65);
        let mut f = Field::gray(65);
        f.fill(0.5);
        let shared: SharedField = Arc::new(f);
        let fl = flatness(&shared, &ctx, 48.0, 12.0, 0.0);
        assert!((fl.land_fraction - 1.0).abs() < 1e-9);
        assert!((fl.buildable_of_land - 1.0).abs() < 1e-9);
        assert_eq!(fl.plain_count, 1);
        assert!(fl.relief_elmos < 1e-9, "a plane has no relief");
    }

    #[test]
    fn hummocks_are_passable_but_not_buildable() {
        // The case a per-texel slope average cannot see: everywhere is gentle
        // enough to drive over, nowhere holds a whole footprint level.
        let project = Project::default();
        let ctx = Context::new(&project, 129);
        let mut f = Field::gray(129);
        for y in 0..129 {
            for x in 0..129 {
                let bump = ((x / 4) % 2 == 0) == ((y / 4) % 2 == 0);
                f.set(y * 129 + x, if bump { 0.52 } else { 0.5 });
            }
        }
        let shared: SharedField = Arc::new(f);
        let p = pathability(&shared, &ctx, 32.0, 0.0);
        let fl = flatness(&shared, &ctx, 96.0, 4.0, 0.0);
        assert!(
            p.passable_fraction > 0.9,
            "kbots should cross hummocks: {:.2}",
            p.passable_fraction
        );
        assert!(
            fl.buildable_of_land < 0.1,
            "but a footprint should not fit on them: {:.2}",
            fl.buildable_of_land
        );
    }

    #[test]
    fn flattening_the_terrain_raises_the_buildable_fraction() {
        let project = Project::default();
        let ctx = Context::new(&project, 129);
        let rough = |gain: f64| {
            let mut f = Field::gray(129);
            for y in 0..129 {
                for x in 0..129 {
                    let v = 0.5
                        + gain
                            * (fdlibm::sin(x as f64 * 0.31) * fdlibm::cos(y as f64 * 0.27) * 0.5);
                    f.set(y * 129 + x, v.clamp(0.0, 1.0));
                }
            }
            Arc::new(f) as SharedField
        };
        let steep = flatness(&rough(0.9), &ctx, 48.0, 12.0, 0.0);
        let gentle = flatness(&rough(0.2), &ctx, 48.0, 12.0, 0.0);
        assert!(
            gentle.buildable_of_land > steep.buildable_of_land,
            "flatter must build better: {:.3} vs {:.3}",
            gentle.buildable_of_land,
            steep.buildable_of_land
        );
        assert!(
            gentle.relief_elmos < steep.relief_elmos,
            "and must have less relief"
        );
        assert!(gentle.median_slope_deg < steep.median_slope_deg);
    }

    #[test]
    fn a_sea_hides_steep_ground_from_the_map_fraction_but_not_from_the_land_one() {
        // Raising the water is not flattening. The land ratio is what notices.
        let project = Project::default();
        let ctx = Context::new(&project, 129);
        let mut f = Field::gray(129);
        for y in 0..129 {
            for x in 0..129 {
                // Flat shelf on the left half, saw-toothed on the right.
                let v = if x < 64 {
                    0.6
                } else {
                    0.6 + if x % 2 == 0 { 0.0 } else { 0.25 }
                };
                f.set(y * 129 + x, v);
            }
        }
        let shared: SharedField = Arc::new(f);
        let dry = flatness(&shared, &ctx, 48.0, 12.0, 0.0);
        let flooded = flatness(&shared, &ctx, 48.0, 12.0, 0.55);
        assert!(dry.land_fraction > flooded.land_fraction - 1e-9);
        assert!(
            (dry.buildable_of_land - flooded.buildable_of_land).abs() < 1e-9,
            "flooding below every sample must not change the land ratio"
        );
    }

    /// A wall with one gap in it, where the answer is known by construction.
    ///
    /// Two flat halves joined by a corridor of a chosen width: the bottleneck
    /// has to come back as that width, and the neck has to be found at the
    /// wall rather than out on the open ground either side of it.
    #[test]
    fn a_corridor_of_a_known_width_measures_as_that_width() {
        const R: usize = 257;
        // 12x12 units is 6144 elmos across 257 samples: 24 elmos a step.
        let project = Project {
            units_x: 12,
            units_y: 12,
            ..Default::default()
        };
        let ctx = Context::new(&project, R);
        let step = ctx.elmo_per_px_x();

        for gap_rows in [5usize, 11, 21] {
            let mut f = Field::gray(R);
            // Flat and passable everywhere...
            for i in 0..R * R {
                f.set(i, 0.5);
            }
            // ...except a sheer wall down the middle, with one gap in it. The
            // wall is a single column of spikes, which no unit can cross.
            let mid = R / 2;
            let lo = R / 2 - gap_rows / 2;
            let hi = lo + gap_rows;
            for y in 0..R {
                if y >= lo && y < hi {
                    continue;
                }
                f.set(y * R + mid, 1.0);
            }
            let shared: SharedField = Arc::new(f);
            let c = chokepoints(&shared, &ctx, 18.0, 0.0);

            // The gap is `gap_rows` samples of passable ground; its centre is
            // half that from the nearest blocked sample either side, so the
            // full width comes out as the gap in elmos, within a step.
            let expect = gap_rows as f64 * step;
            assert!(
                (c.bottleneck - expect).abs() <= step * 1.5,
                "gap of {gap_rows} rows ({expect:.0} elmos) measured as {:.0}",
                c.bottleneck
            );
            // And the neck is at the wall, not somewhere in the open.
            let neck_x = c.bottleneck_at.0 / step;
            assert!(
                (neck_x - mid as f64).abs() <= 2.0,
                "neck reported at x={neck_x:.0}, wall is at {mid}"
            );
            // Open ground either side is far wider than the gap.
            assert!(
                c.median > c.bottleneck * 2.0,
                "median corridor {:.0} should dwarf the {:.0} neck",
                c.median,
                c.bottleneck
            );
        }
    }

    /// A map with no wall has no chokepoint worth reporting: the narrowest
    /// point of the best route across open ground is the map itself.
    /// A corridor map must not be called a funnel for pinching across its
    /// corridor.
    ///
    /// A west-east map with ranges up the flanks is *supposed* to be tight
    /// north-south. Judging it on the more constrained axis called `open` at
    /// 16x8 a funnel at 36 elmos while its actual play axis measured 1030.
    #[test]
    fn a_corridor_is_judged_along_the_axis_its_teams_sit_on() {
        const R: usize = 257;
        let project = Project {
            units_x: 12,
            units_y: 12,
            ..Default::default()
        };
        let ctx = Context::new(&project, R);
        let mut f = Field::gray(R);
        // A wide flat band across the middle, walled off north and south —
        // the shape `open` is built to make.
        for y in 0..R {
            for x in 0..R {
                let band = y > R / 3 && y < R * 2 / 3;
                f.set(y * R + x, if band { 0.5 } else { 1.0 });
            }
        }
        let shared: SharedField = Arc::new(f);
        let c = chokepoints(&shared, &ctx, 18.0, 0.0);

        assert!(
            c.west_east > c.north_south,
            "the corridor should run west-east: {:.0} against {:.0}",
            c.west_east,
            c.north_south
        );
        // Teams east and west of each other read the corridor they play along.
        let (play, axis) = c.along_play_axis("mirrorX");
        assert_eq!(axis, "west-east");
        assert!((play - c.west_east).abs() < 1e-9);
        assert!(
            play >= c.median,
            "the play axis reads {play:.0}, tighter than the median {:.0} — this map would be \
             wrongly called a funnel",
            c.median
        );
        // Teams north and south of each other read the other one, which on
        // this map genuinely is the hard way across.
        let (across, axis) = c.along_play_axis("mirrorY");
        assert_eq!(axis, "north-south");
        assert!(across <= c.west_east);
    }

    #[test]
    fn open_ground_reports_no_neck() {
        const R: usize = 129;
        let project = Project::default();
        let ctx = Context::new(&project, R);
        let mut f = Field::gray(R);
        for i in 0..R * R {
            f.set(i, 0.5);
        }
        let shared: SharedField = Arc::new(f);
        let c = chokepoints(&shared, &ctx, 18.0, 0.0);
        // Everything is passable, so the tightest the route ever gets is set
        // by the map edge — half the map, near enough.
        assert!(
            c.bottleneck > ctx.elmos_x * 0.4,
            "open ground bottlenecked at {:.0} elmos on a {:.0} elmo map",
            c.bottleneck,
            ctx.elmos_x
        );
    }

    #[test]
    fn unit_classes_disagree_about_the_same_terrain() {
        let project = Project::default();
        let ctx = Context::new(&project, 65);
        let mut f = Field::gray(65);
        for y in 0..65 {
            for x in 0..65 {
                f.set(y * 65 + x, 0.3 + (x as f64) * 0.004);
            }
        }
        let shared: SharedField = Arc::new(f);
        let reps = pathability_by_class(&shared, &ctx, 0.0);
        let tank = reps.iter().find(|r| r.class == UnitClass::Tank).unwrap();
        let kbot = reps.iter().find(|r| r.class == UnitClass::Kbot).unwrap();
        assert!(
            kbot.passable_fraction >= tank.passable_fraction,
            "kbots climb more than tanks"
        );
    }
}

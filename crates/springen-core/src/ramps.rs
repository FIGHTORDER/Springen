//! Colour ramps.
//!
//! The hypsometric ramp is the brand's colour imagery — saturated colour on a
//! Springen screen belongs to the terrain, never to the interface.

use crate::field::clamp01;

/// Named terrain palettes. Stops are `[t, r, g, b]` over the land range.
pub const RAMPS: &[(&str, &[[f64; 4]])] = &[
    (
        "verdant",
        &[
            [0.00, 0.24, 0.33, 0.20],
            [0.18, 0.31, 0.42, 0.22],
            [0.42, 0.45, 0.48, 0.27],
            [0.62, 0.44, 0.40, 0.30],
            [0.80, 0.52, 0.50, 0.47],
            [1.00, 0.88, 0.90, 0.92],
        ],
    ),
    (
        "arid",
        &[
            [0.00, 0.55, 0.44, 0.28],
            [0.22, 0.66, 0.54, 0.34],
            [0.48, 0.60, 0.46, 0.30],
            [0.70, 0.48, 0.38, 0.28],
            [0.86, 0.42, 0.34, 0.28],
            [1.00, 0.72, 0.68, 0.62],
        ],
    ),
    (
        "arctic",
        &[
            [0.00, 0.42, 0.48, 0.52],
            [0.20, 0.54, 0.60, 0.63],
            [0.45, 0.68, 0.73, 0.76],
            [0.68, 0.80, 0.85, 0.88],
            [1.00, 0.95, 0.97, 1.00],
        ],
    ),
    (
        "volcanic",
        &[
            [0.00, 0.16, 0.14, 0.14],
            [0.24, 0.24, 0.20, 0.19],
            [0.50, 0.34, 0.24, 0.21],
            [0.68, 0.45, 0.24, 0.17],
            [0.84, 0.30, 0.22, 0.22],
            [1.00, 0.62, 0.60, 0.60],
        ],
    ),
    (
        "rust",
        &[
            [0.00, 0.30, 0.22, 0.18],
            [0.25, 0.46, 0.30, 0.20],
            [0.52, 0.58, 0.38, 0.24],
            [0.74, 0.46, 0.32, 0.24],
            [1.00, 0.78, 0.74, 0.70],
        ],
    ),
    (
        "slate",
        &[
            [0.00, 0.20, 0.24, 0.27],
            [0.28, 0.30, 0.34, 0.37],
            [0.55, 0.40, 0.43, 0.45],
            [0.78, 0.52, 0.54, 0.56],
            [1.00, 0.78, 0.80, 0.82],
        ],
    ),
];

pub fn ramp_stops(name: &str) -> &'static [[f64; 4]] {
    RAMPS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or(RAMPS[0].1)
}

pub fn ramp_names() -> Vec<&'static str> {
    RAMPS.iter().map(|(n, _)| *n).collect()
}

pub fn ramp_at(stops: &[[f64; 4]], t: f64) -> [f64; 3] {
    let t = clamp01(t);
    for i in 1..stops.len() {
        if t <= stops[i][0] {
            let a = &stops[i - 1];
            let b = &stops[i];
            let span = b[0] - a[0];
            let k = (t - a[0]) / if span == 0.0 { 1.0 } else { span };
            return [
                a[1] + (b[1] - a[1]) * k,
                a[2] + (b[2] - a[2]) * k,
                a[3] + (b[3] - a[3]) * k,
            ];
        }
    }
    let l = &stops[stops.len() - 1];
    [l[1], l[2], l[3]]
}

/// HSV tweak so one ramp can be pushed around without new presets.
pub fn adjust_hsv(c: [f64; 3], hue_shift: f64, sat: f64, val: f64) -> [f64; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let mut h = 0.0;
    let mut s = if mx > 0.0 { d / mx } else { 0.0 };
    let mut v = mx;
    if d > 1e-9 {
        h = if mx == r {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
        } else if mx == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
    }
    h = (h + hue_shift + 1.0) % 1.0;
    s = clamp01(s * sat);
    v = clamp01(v * val);
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match (i as i64).rem_euclid(6) {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// The hypsometric ramp used by every preview: abyss to snow.
pub const HYPSO: [(f64, [f64; 3]); 9] = [
    (0.00, [18.0, 46.0, 66.0]),
    (0.18, [32.0, 84.0, 104.0]),
    (0.30, [64.0, 130.0, 126.0]),
    (0.36, [126.0, 156.0, 106.0]),
    (0.50, [156.0, 158.0, 96.0]),
    (0.66, [150.0, 124.0, 84.0]),
    (0.80, [132.0, 112.0, 104.0]),
    (0.92, [190.0, 190.0, 188.0]),
    (1.00, [242.0, 244.0, 246.0]),
];

pub fn hypso(t: f64) -> [f64; 3] {
    let t = clamp01(t);
    for i in 1..HYPSO.len() {
        if t <= HYPSO[i].0 {
            let a = &HYPSO[i - 1];
            let b = &HYPSO[i];
            let span = b.0 - a.0;
            let k = (t - a.0) / if span == 0.0 { 1.0 } else { span };
            return [
                a.1[0] + (b.1[0] - a.1[0]) * k,
                a.1[1] + (b.1[1] - a.1[1]) * k,
                a.1[2] + (b.1[2] - a.1[2]) * k,
            ];
        }
    }
    HYPSO[HYPSO.len() - 1].1
}

/* --------------------------------------------------- the relief view */

// `hypso` spends its bottom 36% on blues and teals, and the relief view feeds
// it a `t` that is already zero *at the waterline*. Dry ground from the shore
// up to a third of the map's dry range therefore came out painted as ocean —
// on the `ridge` starter, 5.4% of the map is submerged and about half of it
// read as sea. Players reported the generator as making "maps with an ocean in
// the middle"; most of those oceans were dry land.
//
// The fix is to stop asking one ramp to describe two media. Depth gets its own
// scale below the waterline, height gets its own above it, and the break is
// exactly at the shore rather than a third of the way up the hill.
//
// `hypso` itself is unchanged and still golden-asserted: it is the prototype's
// ramp and this is a new one.

/// Under water, by depth: shallow to abyssal.
pub const RELIEF_SEA: [(f64, [f64; 3]); 4] = [
    (0.00, [96.0, 152.0, 158.0]),
    (0.25, [56.0, 116.0, 136.0]),
    (0.60, [30.0, 78.0, 104.0]),
    (1.00, [14.0, 38.0, 62.0]),
];

/// Above water, by height: shore to snow. Starts green — the first thing above
/// the waterline is land, and it should look like land.
pub const RELIEF_LAND: [(f64, [f64; 3]); 7] = [
    (0.00, [176.0, 178.0, 132.0]),
    (0.06, [124.0, 154.0, 100.0]),
    (0.28, [142.0, 156.0, 92.0]),
    (0.52, [156.0, 140.0, 88.0]),
    (0.72, [146.0, 120.0, 92.0]),
    (0.88, [176.0, 172.0, 168.0]),
    (1.00, [242.0, 244.0, 246.0]),
];

fn between(stops: &[(f64, [f64; 3])], t: f64) -> [f64; 3] {
    let t = clamp01(t);
    for i in 1..stops.len() {
        if t <= stops[i].0 {
            let (a, b) = (&stops[i - 1], &stops[i]);
            let span = b.0 - a.0;
            let k = (t - a.0) / if span == 0.0 { 1.0 } else { span };
            return [
                a.1[0] + (b.1[0] - a.1[0]) * k,
                a.1[1] + (b.1[1] - a.1[1]) * k,
                a.1[2] + (b.1[2] - a.1[2]) * k,
            ];
        }
    }
    stops[stops.len() - 1].1
}

/// Paint a field sample against the map's own waterline.
///
/// `sea` is the waterline as a field value, so the break lands where the
/// engine will actually flood rather than wherever the ramp happens to turn
/// blue.
pub fn relief(v: f64, sea: f64) -> [f64; 3] {
    if v < sea {
        // Depth as a fraction of the water column, so a shallow sea reads
        // shallow instead of every sea reading abyssal.
        let d = if sea > 0.0 { (sea - v) / sea } else { 0.0 };
        between(&RELIEF_SEA, d)
    } else {
        let above = 1.0 - sea;
        between(
            &RELIEF_LAND,
            if above > 0.0 { (v - sea) / above } else { 0.0 },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relief_view_puts_the_shore_at_the_waterline() {
        // The defect this ramp exists for: ground just above the waterline
        // must not be painted as sea. Blue-dominant means water.
        let blue = |c: [f64; 3]| c[2] > c[1] && c[2] > c[0];
        for sea in [0.05, 0.16, 0.34, 0.5] {
            assert!(blue(relief(sea - 0.01, sea)), "just under {sea} is water");
            assert!(
                !blue(relief(sea + 0.001, sea)),
                "just over {sea} must be land, got {:?}",
                relief(sea + 0.001, sea)
            );
            // And a third of the way up the dry range certainly must not be.
            let third = sea + (1.0 - sea) / 3.0;
            assert!(!blue(relief(third, sea)), "a third up {sea} must be land");
        }
    }

    #[test]
    fn depth_reads_as_depth_rather_than_as_one_flat_blue() {
        // A shallow sea should not paint the same as an abyss.
        let sea = 0.3;
        let shallow = relief(sea - 0.02, sea);
        let deep = relief(0.0, sea);
        assert!(
            shallow[1] > deep[1] + 20.0,
            "shallow {shallow:?} should be far lighter than deep {deep:?}"
        );
    }

    #[test]
    fn a_map_with_no_water_at_all_still_paints() {
        // sea = 0 divides by the water column; it must not produce NaN.
        for v in [0.0, 0.5, 1.0] {
            for c in relief(v, 0.0) {
                assert!(c.is_finite(), "sea 0 at v {v} gave {c}");
            }
            for c in relief(v, 1.0) {
                assert!(c.is_finite(), "sea 1 at v {v} gave {c}");
            }
        }
    }

    #[test]
    fn every_ramp_is_monotonic_in_t_and_ends_at_its_last_stop() {
        for (name, stops) in RAMPS {
            assert_eq!(stops[0][0], 0.0, "{name} must start at 0");
            assert_eq!(stops[stops.len() - 1][0], 1.0, "{name} must end at 1");
            let end = ramp_at(stops, 1.0);
            let last = stops[stops.len() - 1];
            assert_eq!(end, [last[1], last[2], last[3]]);
        }
    }

    #[test]
    fn hsv_round_trips_when_untouched() {
        let c = [0.24, 0.33, 0.20];
        let out = adjust_hsv(c, 0.0, 1.0, 1.0);
        for i in 0..3 {
            assert!((out[i] - c[i]).abs() < 1e-12);
        }
    }
}

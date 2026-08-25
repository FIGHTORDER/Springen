//! The one way to get a map's height field out of a graph.
//!
//! Everything that reads terrain — the bake, the preview, the viewport, the
//! playability probes — must read the *same* terrain, or the tool ends up
//! describing a map the engine will not build. That has already happened once
//! here, with the waterline, and the fix was to make one function own the
//! decision. This is that function for anything the project does to the field
//! after the graph has produced it.
//!
//! Today there is one such thing: the depth cap.

use std::sync::Arc;

use crate::field::{as_gray, Field, SharedField};
use crate::graph::Graph;
use crate::project::{water_level_t, Context, Project};

/// Evaluate the graph's Heightmap terminal and apply the project's own
/// post-processing. `None` when the graph has no Heightmap out node.
pub fn height_field(graph: &Graph, project: &Project, ctx: &Context) -> Option<SharedField> {
    let id = graph.find_terminal("height")?.to_string();
    Some(finish(&as_gray(&graph.evaluate(&id, ctx)), project))
}

/// Apply the project's post-graph terrain settings to an already-evaluated
/// field. Split out for the viewport, which re-paints a field it already has.
pub fn finish(height: &SharedField, project: &Project) -> SharedField {
    match project.max_depth {
        Some(d) if d > 0.0 => shoal(
            height,
            water_level_t(project.min_height, project.max_height),
            d,
            project.height_range(),
        ),
        _ => height.clone(),
    }
}

/// Lift the sea floor toward the surface without moving the coastline.
///
/// The naive way to make water shallower is to raise the waterline, and it is
/// wrong: it floods less land, moves every shore, and changes which ground is
/// buildable. What a map author wants is the sea floor up and the coast where
/// it is — because "can a bot walk through this water" is a fact about the
/// game, and where the shore falls is a fact about the map.
///
/// The curve is `d' = d·cap / (d + cap)`. Three properties earn it the job:
/// it is exactly 0 at the waterline, so the coastline does not move by one
/// sample; its slope there is exactly 1, so the beach keeps the gradient the
/// terrain gave it and no ledge appears at the shore; and it approaches `cap`
/// without ever reaching it, so there is no flat plate at the bottom of the
/// sea where a hard clamp would leave one.
///
/// `cap` and `range` are both elmos; `sea` is the waterline as a field value.
pub fn shoal(height: &SharedField, sea: f64, cap: f64, range: f64) -> SharedField {
    if range <= 0.0 || cap <= 0.0 {
        return height.clone();
    }
    let mut f = Field::gray(height.res);
    f.par_rows(|y, row| {
        let base = y * height.res;
        for (x, o) in row.iter_mut().enumerate() {
            let v = height.get(base + x);
            *o = if v >= sea {
                v as f32
            } else {
                let d = (sea - v) * range;
                (sea - (d * cap / (d + cap)) / range) as f32
            };
        }
    });
    Arc::new(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;

    fn field(vals: &[f64]) -> SharedField {
        let r = (vals.len() as f64).sqrt() as usize;
        let mut f = Field::gray(r);
        for (i, v) in vals.iter().enumerate() {
            f.set(i, *v);
        }
        Arc::new(f)
    }

    #[test]
    fn shoaling_never_moves_the_coastline() {
        // The whole point: which samples are wet must not change.
        let sea = 0.25;
        let r = 33;
        let mut f = Field::gray(r);
        for i in 0..r * r {
            f.set(i, (i as f64) / ((r * r - 1) as f64));
        }
        let before: SharedField = Arc::new(f);
        let after = shoal(&before, sea, 20.0, 500.0);
        for i in 0..before.len() {
            assert_eq!(
                before.get(i) < sea,
                after.get(i) < sea,
                "sample {i} changed side of the waterline"
            );
        }
    }

    #[test]
    fn nothing_above_the_waterline_moves_at_all() {
        let before = field(&[0.0, 0.1, 0.5, 0.9]);
        let after = shoal(&before, 0.25, 15.0, 400.0);
        for i in 2..4 {
            assert_eq!(
                before.get(i).to_bits(),
                after.get(i).to_bits(),
                "dry sample {i} was touched"
            );
        }
    }

    #[test]
    fn no_water_ends_up_deeper_than_the_cap() {
        let range = 500.0;
        let sea = 0.4;
        let cap = 18.0;
        let before = field(&[0.0, 0.05, 0.2, 0.39]);
        let after = shoal(&before, sea, cap, range);
        for i in 0..4 {
            let d = (sea - after.get(i)) * range;
            assert!(d < cap, "sample {i} is {d:.2} elmos deep, cap {cap}");
            assert!(d > 0.0, "sample {i} came out of the water");
        }
        // The deepest sample should be near the cap rather than nowhere near:
        // a cap that shoals everything to a puddle is not a cap.
        let deepest = (sea - after.get(0)) * range;
        assert!(deepest > cap * 0.85, "deepest only reached {deepest:.2}");
    }

    #[test]
    fn a_shallow_bottom_is_barely_touched() {
        // Water already well inside the cap should keep its shape, or every
        // map with a sane sea gets flattened by a setting meant for the
        // deep ones.
        let (range, sea, cap) = (500.0, 0.4, 100.0);
        let before = field(&[0.395, 0.397, 0.399, 0.3995]);
        let after = shoal(&before, sea, cap, range);
        for i in 0..4 {
            let d0 = (sea - before.get(i)) * range;
            let d1 = (sea - after.get(i)) * range;
            assert!(
                (d0 - d1).abs() < d0 * 0.05,
                "{d0:.3} elmos deep became {d1:.3}"
            );
        }
    }

    #[test]
    fn an_uncapped_project_gets_its_field_back_untouched() {
        let p = Project::default();
        assert!(p.max_depth.is_none());
        let before = field(&[0.0, 0.1, 0.5, 0.9]);
        let after = finish(&before, &p);
        assert_eq!(before.data, after.data);
    }
}

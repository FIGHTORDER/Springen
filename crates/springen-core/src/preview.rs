// Pixel buffers are written by linear index; iterator adapters obscure the
// stride the callers upload on.
#![allow(clippy::needless_range_loop)]
//! Colouring a graph for a human: the six ways the terrain can be looked at.
//!
//! The desktop viewport uploads these as a texture and the CLI writes them to
//! a PNG. Both need the same answer — a preview that disagrees with the
//! workstation is worse than no preview — so the painting lives here rather
//! than in either front end.

use std::sync::Arc;

use crate::analysis;
use crate::field::{as_color, as_gray, clamp01, Field, SharedField};
use crate::graph::Graph;
use crate::nodes::slope_degrees_aniso;
use crate::project::{water_level_t, Context, Project};
use crate::zk;

/// What the terrain is coloured by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// Hypsometric relief, the tool's own painting.
    Relief,
    /// The baked diffuse, i.e. what the engine will actually show.
    Diffuse,
    /// Metal spots, their extractor radius and the painted raster.
    Metal,
    /// Terrain type indices.
    TerrainType,
    /// Slope against a chosen climb limit.
    Slope,
    /// Traversable regions: the largest one against the islands.
    Pathability,
}

impl ViewMode {
    pub const ALL: [ViewMode; 6] = [
        ViewMode::Relief,
        ViewMode::Diffuse,
        ViewMode::Metal,
        ViewMode::TerrainType,
        ViewMode::Slope,
        ViewMode::Pathability,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Relief => "Relief",
            ViewMode::Diffuse => "Diffuse",
            ViewMode::Metal => "Metal",
            ViewMode::TerrainType => "Type",
            ViewMode::Slope => "Slope",
            ViewMode::Pathability => "Pathability",
        }
    }
    pub fn from_name(name: &str) -> Option<ViewMode> {
        ViewMode::ALL
            .iter()
            .copied()
            .find(|m| m.label().eq_ignore_ascii_case(name))
    }
}

/// A rendered view: the height field it was built from, and RGB samples.
pub struct Preview {
    pub height: SharedField,
    /// `res * res * 3` bytes.
    pub colour: Vec<u8>,
}

/// Everything a view needs beyond the graph itself.
pub struct PreviewOptions<'a> {
    pub res: usize,
    pub mode: ViewMode,
    /// Climb limit the slope and pathability views are drawn against.
    pub climb_limit: f64,
    /// Spots the metal view draws. Empty is fine; the raster is then blank.
    pub spots: &'a [zk::MetalSpot],
    /// Detail materials to blend into the Diffuse view, exactly as the bake
    /// does. Rendering the tiles costs about a second, so the caller owns
    /// them and decides when to re-render; `None` shows the graph's diffuse
    /// alone, which is what the map looked like before materials existed.
    pub materials: Option<&'a crate::material::SplatMaterials>,
    /// Rasters the graph's `import` nodes read.
    pub rasters: std::sync::Arc<crate::raster::Rasters>,
}

impl Default for PreviewOptions<'_> {
    fn default() -> Self {
        PreviewOptions {
            res: 257,
            mode: ViewMode::Relief,
            climb_limit: 18.0,
            spots: &[],
            materials: None,
            rasters: Default::default(),
        }
    }
}

/// The pixel shape a square view should be *shown* at.
///
/// The graph is evaluated on a square lattice and painted into a square
/// buffer, but that buffer stands for a world that need not be square. Drawing
/// it as a square shows a 16×8 map with its Z axis stretched to twice its
/// width — the terrain is right and the picture is not.
///
/// The 3D viewport never had this problem: it maps the same texture onto a
/// mesh built from both world extents. This is for the flat views, which have
/// nothing telling them the shape of the ground.
pub fn view_size(elmos_x: f64, elmos_y: f64, res: usize) -> (usize, usize) {
    if elmos_x >= elmos_y {
        (
            res,
            ((res as f64 * elmos_y / elmos_x).round() as usize).max(1),
        )
    } else {
        (
            ((res as f64 * elmos_x / elmos_y).round() as usize).max(1),
            res,
        )
    }
}

/// Resample a square painted view to the world's aspect, for display.
///
/// Nearest sample: this only ever shrinks one axis, and a view mode like
/// `TerrainType` or `Metal` carries category colours that must not be
/// averaged into ones that mean something else.
pub fn to_view_size(colour: &[u8], res: usize, w: usize, h: usize) -> Vec<u8> {
    if w == res && h == res {
        return colour.to_vec();
    }
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        let sy = (y * res / h).min(res - 1);
        for x in 0..w {
            let sx = (x * res / w).min(res - 1);
            let o = (sy * res + sx) * 3;
            let d = (y * w + x) * 3;
            out[d..d + 3].copy_from_slice(&colour[o..o + 3]);
        }
    }
    out
}

/// Paint a graph. `None` when the graph has no Heightmap out node, which is
/// the one case where there is nothing to show.
pub fn render(graph: &Graph, project: &Project, opts: &PreviewOptions) -> Option<Preview> {
    let res = opts.res.max(2);
    let ctx = Context::with_rasters(project, res, opts.rasters.clone());
    // Through `terrain::height_field`, not `graph.evaluate` directly: the
    // project's own terrain settings have to reach the preview or the tool
    // shows a map the bake will not write.
    let height = crate::terrain::height_field(graph, project, &ctx)?;
    let colour = paint(graph, project, &ctx, &height, opts);
    Some(Preview { height, colour })
}

/// Colour an already-evaluated height field. Split out because the viewport
/// re-paints without re-evaluating when only the view mode changed.
pub fn paint(
    graph: &Graph,
    project: &Project,
    ctx: &Context,
    height: &SharedField,
    opts: &PreviewOptions,
) -> Vec<u8> {
    let res = ctx.res;
    let n = res * res;
    let sea = water_level_t(project.min_height, project.max_height);
    let mut colour = vec![0u8; n * 3];
    let put = |c: &mut Vec<u8>, i: usize, rgb: [f64; 3]| {
        for k in 0..3 {
            c[i * 3 + k] = rgb[k].clamp(0.0, 255.0) as u8;
        }
    };

    match opts.mode {
        ViewMode::Relief => {
            // Split at the waterline rather than remapped through one ramp:
            // `hypso` turns green a third of the way up its range, so low dry
            // ground used to read as ocean. See `ramps::relief`.
            for i in 0..n {
                put(&mut colour, i, crate::ramps::relief(height.get(i), sea));
            }
        }
        ViewMode::Diffuse => match graph.find_terminal("diffuse") {
            Some(did) => {
                let d = as_color(&graph.evaluate(did, ctx));
                // The same material blend the bake applies. Without it this
                // view shows a texture the map will not have.
                let splat = opts
                    .materials
                    .and(graph.find_terminal("splat"))
                    .map(|id| graph.evaluate(id, ctx));
                let blender = opts.materials.map(|m| {
                    crate::material::Blender::new(
                        m,
                        crate::material::DEFAULT_TEX_SCALES,
                        project.materials.blend,
                    )
                });
                let (ex, ez) = (
                    ctx.elmos_x / (res - 1).max(1) as f64,
                    ctx.elmos_y / (res - 1).max(1) as f64,
                );
                // One pixel of this view covers this much ground, which is
                // what decides how much of the detail tile it averages over.
                let footprint = ex.max(ez);
                for i in 0..n {
                    let mut rgb = [d.get(i * 3), d.get(i * 3 + 1), d.get(i * 3 + 2)];
                    let (x, y) = ((i % res) as f64, (i / res) as f64);
                    if let (Some(b), Some(sf)) = (&blender, &splat) {
                        if b.active() {
                            let w = [
                                sf.get(i * sf.ch),
                                sf.get(i * sf.ch + 1),
                                sf.get(i * sf.ch + 2),
                                if sf.ch > 3 {
                                    sf.get(i * sf.ch + 3)
                                } else {
                                    0.0
                                },
                            ];
                            rgb = b.shade(rgb, w, x * ex, y * ez);
                        }
                    }
                    // The detail tile rides on top of everything, and is not
                    // gated on the splat blend: the engine applies it from its
                    // own resource slot whatever the splat weights say.
                    //
                    // At whole-map framing it will average to nothing, because
                    // it repeats every 50 elmos and a pixel here spans many
                    // times that. That is the honest answer -- and it is why
                    // judging a detail tile needs `material::ground_sample`,
                    // not this view.
                    if let Some(b) = &blender {
                        rgb = b.detail(rgb, x * ex, y * ez, footprint);
                    }
                    put(
                        &mut colour,
                        i,
                        [rgb[0] * 255.0, rgb[1] * 255.0, rgb[2] * 255.0],
                    );
                }
            }
            None => {
                for i in 0..n {
                    put(&mut colour, i, [90.0, 92.0, 88.0]);
                }
            }
        },
        ViewMode::TerrainType => {
            let t = graph
                .find_terminal("type")
                .map(|tid| as_gray(&graph.evaluate(tid, ctx)));
            for i in 0..n {
                let v = t.as_ref().map(|f| f.get(i)).unwrap_or(0.0);
                // Index 0 default, index 1 rock, and anything between is a
                // value that should not exist -- so it is shown as alert.
                let rgb = if v < 0.25 {
                    [96.0, 116.0, 96.0]
                } else if v > 0.75 {
                    [150.0, 140.0, 128.0]
                } else {
                    [218.0, 90.0, 78.0]
                };
                put(&mut colour, i, rgb);
            }
        }
        ViewMode::Slope => {
            let (hx, hy) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
            for y in 0..res {
                for x in 0..res {
                    let deg = slope_degrees_aniso(height, x, y, res, ctx.height_range, hx, hy);
                    let t = (deg / opts.climb_limit.max(1e-6)).min(2.0);
                    let rgb = if t <= 1.0 {
                        // Under the limit: green through amber.
                        [110.0 + 130.0 * t, 190.0 - 20.0 * t, 138.0 - 70.0 * t]
                    } else {
                        // Over it: amber into alert red.
                        let u = (t - 1.0).min(1.0);
                        [218.0 + 16.0 * u, 164.0 - 74.0 * u, 65.0 - 10.0 * u]
                    };
                    put(&mut colour, y * res + x, rgb);
                }
            }
        }
        ViewMode::Pathability => {
            let p = analysis::pathability(height, ctx, opts.climb_limit, sea);
            let c = analysis::chokepoints(height, ctx, opts.climb_limit, sea);
            // Shaded by how wide the corridor is, against the widest the best
            // route across the map manages. Connectivity alone says a map is
            // one region; it does not say whether an army fits through the
            // part joining its halves, and that is usually the map.
            // Scaled by what open ground looks like on *this* map, not by the
            // median — normalising by the median saturates half the region and
            // shows no gradient at all.
            let full = c.p90.max(1.0);
            const TIGHT: [f64; 3] = [206.0, 108.0, 84.0];
            const OPEN: [f64; 3] = [111.0, 191.0, 139.0];
            for i in 0..n {
                let rgb = if p.pass[i] == 0 {
                    [58.0, 66.0, 74.0]
                } else if p.label[i] == p.largest_id {
                    let t = clamp01(c.width[i] / full);
                    [
                        TIGHT[0] + (OPEN[0] - TIGHT[0]) * t,
                        TIGHT[1] + (OPEN[1] - TIGHT[1]) * t,
                        TIGHT[2] + (OPEN[2] - TIGHT[2]) * t,
                    ]
                } else {
                    // A traversable island is the failure mode worth seeing.
                    [217.0, 164.0, 65.0]
                };
                put(&mut colour, i, rgb);
            }
            // The neck itself, and the two ends the route was measured
            // between.
            let (ex, ey) = (ctx.elmo_per_px_x(), ctx.elmo_per_px_y());
            let mark = |c2: &mut Vec<u8>, at: (f64, f64), rgb: [f64; 3], rad: isize| {
                let (cx, cy) = ((at.0 / ex) as isize, (at.1 / ey) as isize);
                for dy in -rad..=rad {
                    for dx in -rad..=rad {
                        let d2 = dx * dx + dy * dy;
                        if d2 > rad * rad || d2 < (rad - 1) * (rad - 1) {
                            continue;
                        }
                        let (x, y) = (cx + dx, cy + dy);
                        if x < 0 || y < 0 || x >= res as isize || y >= res as isize {
                            continue;
                        }
                        put(c2, y as usize * res + x as usize, rgb);
                    }
                }
            };
            // A dark ring inside a pale one, so the marker reads against open
            // green, tight salmon and impassable slate alike.
            if c.bottleneck > 0.0 {
                mark(&mut colour, c.bottleneck_at, [24.0, 26.0, 30.0], 7);
                mark(&mut colour, c.bottleneck_at, [246.0, 244.0, 238.0], 9);
                mark(&mut colour, c.bottleneck_at, [24.0, 26.0, 30.0], 11);
            }
        }
        ViewMode::Metal => {
            let d = crate::spring::derive(project.units_x, project.units_y);
            let c129 = Context::new(project, 129);
            let raster = zk::paint_metal_raster(
                opts.spots,
                &c129,
                d.metal_w as usize,
                d.metal_h as usize,
                &zk::RasterOptions::default(),
            );
            let (mw, mh) = (d.metal_w as usize, d.metal_h as usize);
            for y in 0..res {
                for x in 0..res {
                    let i = y * res + x;
                    let v = height.get(i);
                    // Desaturated relief so the metal reads on top of it.
                    let g = 70.0 + 90.0 * v;
                    let mut rgb = [g, g * 1.02, g * 1.06];
                    let mx = (x * mw / res).min(mw - 1);
                    let my = (y * mh / res).min(mh - 1);
                    if raster[my * mw + mx] > 0 {
                        rgb = [224.0, 138.0, 60.0];
                    }
                    put(&mut colour, i, rgb);
                }
            }
        }
    }
    colour
}

/// A flat grey field, for the one case where a graph has no heightmap.
pub fn blank(res: usize) -> SharedField {
    Arc::new(Field::gray(res))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::starter::starter_graph;

    #[test]
    fn every_mode_paints_the_whole_field() {
        let project = Project {
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        let g = starter_graph("textured");
        for mode in ViewMode::ALL {
            let p = render(
                &g,
                &project,
                &PreviewOptions {
                    res: 65,
                    mode,
                    ..Default::default()
                },
            )
            .expect("the textured starter has a heightmap");
            assert_eq!(p.colour.len(), 65 * 65 * 3, "{}", mode.label());
            // A view that came out entirely one colour is a view that is not
            // showing anything.
            let distinct = p.colour.chunks(3).collect::<std::collections::HashSet<_>>();
            assert!(distinct.len() > 1, "{} is flat", mode.label());
        }
    }

    #[test]
    fn modes_round_trip_through_their_labels() {
        for m in ViewMode::ALL {
            assert_eq!(ViewMode::from_name(m.label()), Some(m));
        }
        assert_eq!(ViewMode::from_name("relief"), Some(ViewMode::Relief));
        assert_eq!(ViewMode::from_name("nope"), None);
    }
}

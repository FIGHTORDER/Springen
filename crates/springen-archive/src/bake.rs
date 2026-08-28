// Layers are channel-interleaved and sampled on a stride; index loops state
// the memory layout that iterator adapters would hide.
#![allow(clippy::needless_range_loop)]
//! The full bake: graph in, loadable map archive out.
//!
//! Every layer is written at its true SMF resolution, derived from one integer.
//! Nothing here resamples in pixels — the graph is evaluated once on a square
//! lattice and sampled onto each layer's own grid.

use std::fs;
use std::io;
use std::path::Path;

use rayon::prelude::*;

use springen_core::analysis;
use springen_core::bake::{
    bake_gray, bake_index, bake_rgba, bake_shaded, height_and_range, HeightMode, Resample,
};
use springen_core::field::{as_color, as_gray, sample_color, Field, SharedField};
use springen_core::graph::Graph;
use springen_core::lua::{
    featureplacer_lua, metal_layout_lua_with_sets, startboxes_lua, startboxes_on,
    MetalLayoutOptions, PlacedFeature, StartGround,
};
use springen_core::png::{encode, Compression, PngColor};
use springen_core::project::{water_level_t, Context, Project};
use springen_core::spring::{derive, size_rejection, Derived};
use springen_core::zk;
use springen_smf::{minimap, smf, smt};

use crate::mapinfo::{mapinfo_lua, mapoptions_lua, Resources};
use crate::pack::Blueprint;

/// Which engine's conventions to follow for the metal raster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Game {
    /// Blank raster plus `mapconfig/map_metal_layout.lua`, which is what real
    /// Zero-K maps ship: the reference map's metalmap is 320×320 and entirely
    /// zero.
    #[default]
    ZeroK,
    /// Discrete blobs painted from the spot list, for games that use the
    /// engine's built-in metalmap scheme.
    Spring,
}

#[derive(Clone, Debug)]
pub struct BakeOptions {
    /// Graph evaluation resolution. Defaults to the vertex lattice size.
    pub eval_res: Option<usize>,
    pub height_mode: HeightMode,
    pub game: Game,
    pub spot_count: usize,
    pub min_separation: f64,
    pub metal_amount: f64,
    pub blob_radius: f64,
    /// Steepest ground a mex footprint may sit on.
    pub max_spot_slope_deg: f64,
    /// Players the metal-per-player readout is divided between.
    pub players: usize,
    /// Geothermal vents to place, as `mapconfig/featureplacer/set.lua`. Zero
    /// ships no feature list at all.
    pub geo_count: usize,
    pub map_id: u32,
    /// Also ship metal, type and grass as external images so they can be
    /// iterated without recompiling.
    pub external_info_tex: bool,
    pub emit_startboxes: bool,
    /// Rasters the graph's `import` nodes read. Empty for a purely procedural
    /// project, which is every project that predates importing.
    pub rasters: std::sync::Arc<springen_core::raster::Rasters>,
}

impl Default for BakeOptions {
    fn default() -> Self {
        BakeOptions {
            eval_res: None,
            height_mode: HeightMode::Fit,
            game: Game::ZeroK,
            spot_count: 14,
            min_separation: 700.0,
            metal_amount: 2.0,
            blob_radius: 48.0,
            max_spot_slope_deg: 12.0,
            players: 2,
            geo_count: 0,
            map_id: 1,
            external_info_tex: false,
            emit_startboxes: true,
            rasters: Default::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BakeReport {
    pub derived: Derived,
    pub eval_res: usize,
    pub spots: Vec<zk::MetalSpot>,
    pub issues: Vec<String>,
    pub symmetric: bool,
    /// The symmetry the map was laid out with, so a reader can tell which axis
    /// the teams sit on without re-deriving it.
    pub symmetry: String,
    pub tile_slots: usize,
    pub tiles_stored: u32,
    pub dedup_ratio: f64,
    pub smf_bytes: u64,
    pub smt_bytes: u64,
    pub metal_blobs: usize,
    pub water_fraction: f64,
    /// The height range the heightmap was baked against, which under
    /// `HeightMode::Fit` is tightened from the project's declared one.
    pub declared_range: (f64, f64),
    pub pathability: Vec<(String, f64, f64, usize)>,
    /// How much of the map will hold a building, which is the number to watch
    /// when the goal is a flat map rather than a passable one.
    pub flatness: analysis::Flatness,
    /// How wide the ground you can move through is, and where it pinches.
    /// Traversable fraction says the halves are joined; this says whether an
    /// army fits through the join.
    pub choke: analysis::Choke,
    pub unbuildable_spots: Vec<String>,
    pub metal_per_player: f64,
    pub players: usize,
    pub geos: Vec<zk::MetalSpot>,
    /// Start boxes emitted, which is the order of the symmetry group.
    pub teams: usize,
}

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// Called as each stage of the bake finishes, with the seconds it took.
///
/// A bake is eighteen seconds at 8x8 and three minutes at 24x24, and before
/// this it printed nothing at all until it was done.
pub type OnStage<'a> = &'a dyn Fn(&str, f64);

struct Stages<'a> {
    f: OnStage<'a>,
    last: std::time::Instant,
}

impl Stages<'_> {
    fn mark(&mut self, name: &str) {
        let now = std::time::Instant::now();
        (self.f)(name, now.duration_since(self.last).as_secs_f64());
        self.last = now;
    }
}

/// Evaluate, bake and assemble. Binary layers are written under `work_dir`
/// and referenced from the blueprint by path, so a large map never has to be
/// held in memory twice.
/// How wide the splat distribution and specular maps should be.
///
/// These used to be pinned at 1024² whatever the map, on the strength of a
/// measurement that said SSMF resources are a fixed size rather than
/// map-sized. That is true of the *format* and false as advice: the reference
/// map is 5120 elmos across and ships a 1024² distribution, which is five
/// elmos to a pixel. Holding 1024 for every map means a 24x24 gets twelve
/// elmos to a pixel — the same file, two and a half times blurrier than the
/// map it was measured from.
///
/// So the number that stays fixed is the sampling, not the size. Powers of
/// two because a GPU wants them, floored at 1024 so nothing gets worse than
/// it was, and capped at 4096 because past that the archive grows faster than
/// the map improves.
pub fn ssmf_res(elmos: f64) -> usize {
    /// Elmos per pixel on the reference map's `dist.png`.
    const REFERENCE: f64 = 5.0;
    let want = elmos / REFERENCE;
    let mut r = 1024usize;
    while (r as f64) < want && r < 4096 {
        r *= 2;
    }
    r
}

pub fn bake(
    project: &Project,
    graph: &Graph,
    opts: &BakeOptions,
    work_dir: &Path,
) -> io::Result<(Blueprint, BakeReport)> {
    bake_with_progress(project, graph, opts, work_dir, &|_, _| {})
}

/// As [`bake`], reporting each stage as it finishes.
pub fn bake_with_progress(
    project: &Project,
    graph: &Graph,
    opts: &BakeOptions,
    work_dir: &Path,
    on_stage: OnStage,
) -> io::Result<(Blueprint, BakeReport)> {
    let mut stage = Stages {
        f: on_stage,
        last: std::time::Instant::now(),
    };
    if let Some(reason) = size_rejection(project.units_x, project.units_y) {
        return Err(err(reason));
    }
    let d = derive(project.units_x, project.units_y);
    let short = project.short_name();
    let lower = short.to_lowercase();

    let res = opts.eval_res.unwrap_or(d.height_w.max(d.height_h) as usize);
    let ctx = Context::with_rasters(project, res, opts.rasters.clone());
    if let Some(reason) = zk::symmetry_rejection(&project.mex_sym, &ctx) {
        return Err(err(reason));
    }

    // Refusing an unwired terminal as well as a missing one. Both bake
    // successfully otherwise, and both produce a map that is a level plane
    // from corner to corner with nothing anywhere saying why.
    let height_id = match (
        graph.find_wired_terminal("height"),
        graph.find_terminal("height"),
    ) {
        (Some(id), _) => id.to_string(),
        (None, Some(_)) => {
            return Err(err(
                "The Heightmap out node has nothing connected to it, so the map would be perfectly flat. Wire a field into it.",
            ))
        }
        (None, None) => {
            return Err(err(
                "The graph has no Heightmap out node, so there is nothing to bake.",
            ))
        }
    };
    // An `import` node whose raster was never loaded evaluates to a flat
    // field, and a flat field bakes into a perfectly level map with nothing
    // anywhere saying why — the same failure an unwired heightmap terminal
    // used to be. Named and refused instead.
    for n in &graph.nodes {
        if n.type_name == "import" {
            let name = n.params.s("name");
            if !ctx.rasters.contains(name) {
                return Err(err(format!(
                    "The Imported terrain node wants a raster named `{name}`, which is not loaded. Loaded: {}.",
                    if ctx.rasters.is_empty() {
                        "nothing".to_string()
                    } else {
                        ctx.rasters.names().join(", ")
                    }
                )));
            }
        }
    }

    // `terrain::finish` applies the project's own terrain settings — today
    // the depth cap — and it is applied here, before anything reads the
    // field, so the layers, the spot placement, the probes and the preview
    // are all looking at one terrain.
    let height =
        springen_core::terrain::finish(&as_gray(&graph.evaluate(&height_id, &ctx)), project);
    let height_gray = as_gray(&height);
    stage.mark("Height graph");

    // Colour. Without a Diffuse out node the shaded relief the evaluator
    // already draws is used, so a height-only graph still produces a map.
    let sea_t = water_level_t(project.min_height, project.max_height);
    let diffuse: SharedField = match graph.find_terminal("diffuse") {
        Some(id) => as_color(&graph.evaluate(id, &ctx)),
        None => {
            let rgb = bake_shaded(&height, res, res, &ctx, sea_t, 0.0);
            let mut f = Field::new(res, 3);
            for i in 0..res * res * 3 {
                f.set(i, f64::from(rgb[i]) / 255.0);
            }
            std::sync::Arc::new(f)
        }
    };
    stage.mark("Diffuse graph");

    /* -- heightmap on the vertex lattice ------------------------------- */
    // The field and the range it is declared with are one decision: a raw
    // sample v means minHeight + v * range, and everything painted against
    // the field -- the water tint, every sea test, the slope -- assumes it.
    let (hfield, decl_min, decl_max) = height_and_range(
        &height,
        opts.height_mode,
        project.min_height,
        project.max_height,
    );
    let heightmap = bake_gray(
        &hfield,
        d.height_w as usize,
        d.height_h as usize,
        16,
        Resample::Bilinear,
        None,
    );

    /* -- category layers, nearest-neighbour only ------------------------ */
    let type_levels = graph
        .find_terminal("type")
        .and_then(|id| graph.node(id))
        .map(|n| n.params.f("levels") as u32)
        .unwrap_or(2);
    let typemap: Vec<u8> = match graph.find_terminal("type") {
        Some(id) => bake_index(
            &as_gray(&graph.evaluate(id, &ctx)),
            d.type_w as usize,
            d.type_h as usize,
            type_levels,
        )
        .iter()
        .map(|v| *v as u8)
        .collect(),
        None => vec![0u8; (d.type_w * d.type_h) as usize],
    };
    let grassmap: Option<Vec<u8>> = graph.find_terminal("grass").map(|id| {
        bake_gray(
            &as_gray(&graph.evaluate(id, &ctx)),
            d.grass_w as usize,
            d.grass_h as usize,
            8,
            Resample::Nearest,
            Some(1.0),
        )
        .iter()
        .map(|v| *v as u8)
        .collect()
    });
    stage.mark("Height, type and grass layers");

    /* -- metal spots ---------------------------------------------------- */
    let metal_mask = match graph.find_terminal("metal") {
        Some(id) => as_gray(&graph.evaluate(
            id,
            &Context::with_rasters(project, 129, opts.rasters.clone()),
        )),
        None => std::sync::Arc::new(Field::gray(129)),
    };
    let c129 = Context::with_rasters(project, 129, opts.rasters.clone());
    let build_opts = zk::BuildabilityOptions {
        sea_level: sea_t,
        max_slope_deg: opts.max_spot_slope_deg,
        ..Default::default()
    };
    // A hand-placed layout wins outright. Re-proposing over it would throw
    // away the author's work on every bake, and the whole point of moving a
    // mex by hand is that the generator's answer was not the one you wanted.
    let hand_placed = !project.spots.is_empty();
    let spots = if hand_placed {
        let mut s = project.spots.clone();
        zk::renumber(&mut s);
        s
    } else {
        // The height field, not the mask, decides where a mex can stand.
        zk::propose_spots_on(
            &metal_mask,
            Some(&height_gray),
            &c129,
            &zk::ProposeOptions {
                count: opts.spot_count,
                min_separation: opts.min_separation,
                symmetry: project.mex_sym.clone(),
                amount: opts.metal_amount,
                build: Some(build_opts.clone()),
                ..Default::default()
            },
        )
    };
    let validation = zk::validate_spots(&spots, &c129, project.extractor_radius);
    let sym = zk::symmetry_report(&spots, &c129, &project.mex_sym);
    let mut issues: Vec<String> = validation.issues.iter().map(|i| i.text.clone()).collect();
    if hand_placed {
        issues.push(format!(
            "{} spot(s) placed by hand; the generator did not touch them.",
            spots.len()
        ));
    }
    if !hand_placed && spots.len() < opts.spot_count {
        issues.push(format!(
            "{} of {} spots placed — no buildable ground left that is {:.0} elmos clear of the rest. Loosen the separation, widen the metal mask, or allow more than {:.0}° of slope.",
            spots.len(),
            opts.spot_count,
            opts.min_separation,
            opts.max_spot_slope_deg
        ));
    }
    if !sym.symmetric {
        issues.push(format!(
            "{} spot(s) have no symmetry partner under {}",
            sym.unmatched.len(),
            project.mex_sym
        ));
    }

    // Geothermal vents. Placed from the same mask but kept well apart from
    // each other and from the mexes, since a geo next to a mex or next to
    // another geo is a known map-design mistake.
    let geos = if opts.geo_count > 0 {
        let taken: Vec<(f64, f64)> = spots.iter().map(|s| (s.x, s.z)).collect();
        let g = zk::propose_geo_vents(
            &metal_mask,
            Some(&height_gray),
            &c129,
            opts.geo_count,
            opts.min_separation,
            &project.mex_sym,
            Some(build_opts.clone()),
            &taken,
        );
        if g.len() < opts.geo_count {
            issues.push(format!(
                "{} of {} geothermal vents placed — the rest had nowhere clear of the mexes to go.",
                g.len(),
                opts.geo_count
            ));
        }
        g
    } else {
        Vec::new()
    };

    // A map whose terrain never crosses the waterline has no coast, and one
    // entirely under it has no land. Both are legal and both are almost always
    // a mistake, and neither says anything for itself.
    if decl_min >= 0.0 {
        issues.push(format!(
            "The terrain never goes below the waterline — the map has no water at all. Its lowest point is {decl_min:.0} elmos."
        ));
    } else if decl_max <= 0.0 {
        issues.push(format!(
            "The whole map is below the waterline — its highest point is {decl_max:.0} elmos."
        ));
    }

    let metalmap = match opts.game {
        Game::ZeroK => vec![0u8; (d.metal_w * d.metal_h) as usize],
        Game::Spring => zk::paint_metal_raster(
            &spots,
            &c129,
            d.metal_w as usize,
            d.metal_h as usize,
            &zk::RasterOptions {
                blob_radius: opts.blob_radius,
                max_metal: project.max_metal,
                ..Default::default()
            },
        ),
    };
    let metal_blobs = zk::count_blobs(&metalmap, d.metal_w as usize, d.metal_h as usize);
    if opts.game == Game::Spring && metal_blobs > 0 && metal_blobs < zk::Zk::INDISCRETE_MIN_SPOTS {
        issues.push(format!(
            "Only {metal_blobs} blobs in the metal raster; Zero-K's fallback detection needs {}+ or it lets mexes be built anywhere",
            zk::Zk::INDISCRETE_MIN_SPOTS
        ));
    }

    // Buildability again, now as a check on the result rather than a
    // constraint on it: anything left here is ground the search could not
    // escape, and it gets named.
    let build = zk::check_buildability(&spots, &height, &ctx, &build_opts);
    let unbuildable: Vec<String> = build
        .iter()
        .filter(|b| !b.buildable)
        .map(|b| {
            if b.underwater {
                format!("{} is underwater", b.spot_id)
            } else {
                format!("{} sits on {:.0}° ground", b.spot_id, b.max_slope_deg)
            }
        })
        .collect();

    stage.mark("Metal spots and start boxes");

    /* -- detail materials ------------------------------------------------- */
    // The splat distribution says *how much* of each channel; these say what
    // each channel is made of. Spring repeats them across the whole map, so
    // they are small tiles and they have to meet themselves exactly.
    for bad in project.materials.unknown() {
        issues.push(format!(
            "No material named {bad}; using the default for that slot. Known materials: {}.",
            springen_core::material::keys().join(", ")
        ));
    }
    let mats =
        springen_core::material::render_set(&project.materials, f64::from(ctx.seed) + 4241.0);
    let splat_field: Option<SharedField> = graph
        .find_terminal("splat")
        .map(|id| graph.evaluate(id, &ctx));
    let scales = Resources::default().splat_tex_scales;
    // Splat blend only. `Blender::detail` is deliberately not called here:
    // the engine adds the detail tile over this diffuse at runtime, so baking
    // it in would lay the same grain down twice.
    let blender = springen_core::material::Blender::new(&mats, scales, project.materials.blend);
    stage.mark("Detail materials");

    /* -- SMT tiles, sampled straight from the colour field --------------- */
    let maps_dir = work_dir.join("maps");
    fs::create_dir_all(&maps_dir)?;

    let tex_w = d.tex_w as usize;
    let tex_h = d.tex_h as usize;
    let sx = (res - 1) as f64 / (tex_w - 1) as f64;
    let sy = (res - 1) as f64 / (tex_h - 1) as f64;
    // Elmos per diffuse texel, so a material tile can be placed in the world
    // rather than in texture space -- the same frequency the GPU will use for
    // the detail normals on top.
    let ex_per_tex = ctx.elmos_x / (tex_w - 1).max(1) as f64;
    let ez_per_tex = ctx.elmos_y / (tex_h - 1).max(1) as f64;
    let tile_set = smt::build(d.tiles_x as usize, d.tiles_y as usize, |tx, ty| {
        let mut out = vec![0u8; smt::TILE_SIZE * smt::TILE_SIZE * 3];
        let mut tmp = [0.0f64; 4];
        let mut w = [0.0f64; 4];
        for y in 0..smt::TILE_SIZE {
            for x in 0..smt::TILE_SIZE {
                let (px, py) = (tx * smt::TILE_SIZE + x, ty * smt::TILE_SIZE + y);
                let gx = px as f64 * sx;
                let gy = py as f64 * sy;
                sample_color(&diffuse, gx, gy, &mut tmp);
                let mut rgb = [tmp[0], tmp[1], tmp[2]];
                if blender.active() {
                    if let Some(sf) = &splat_field {
                        sample_color(sf, gx, gy, &mut w);
                        rgb = blender.shade(rgb, w, px as f64 * ex_per_tex, py as f64 * ez_per_tex);
                    }
                }
                let o = (y * smt::TILE_SIZE + x) * 3;
                for c in 0..3 {
                    out[o + c] = (rgb[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
        out
    });

    stage.mark("SMT tiles");

    let smt_name = format!("{lower}.smt");
    let smt_path = maps_dir.join(&smt_name);
    {
        let mut f = io::BufWriter::new(fs::File::create(&smt_path)?);
        smt::write(&mut f, &tile_set)?;
    }

    /* -- minimap --------------------------------------------------------- */
    let mut mm_rgb = vec![0u8; minimap::SIZE * minimap::SIZE * 3];
    {
        let ms = (res - 1) as f64 / (minimap::SIZE - 1) as f64;
        mm_rgb
            .par_chunks_mut(minimap::SIZE * 3)
            .enumerate()
            .for_each(|(y, row)| {
                let mut tmp = [0.0f64; 4];
                let mut w = [0.0f64; 4];
                for x in 0..minimap::SIZE {
                    let (gx, gy) = (x as f64 * ms, y as f64 * ms);
                    sample_color(&diffuse, gx, gy, &mut tmp);
                    let mut rgb = [tmp[0], tmp[1], tmp[2]];
                    // The minimap is a picture of the ground, so it gets the
                    // same treatment or the two disagree at a glance.
                    if blender.active() {
                        if let Some(sf) = &splat_field {
                            sample_color(sf, gx, gy, &mut w);
                            let e = ctx.elmos_x / (minimap::SIZE - 1) as f64;
                            let ez = ctx.elmos_y / (minimap::SIZE - 1) as f64;
                            rgb = blender.shade(rgb, w, x as f64 * e, y as f64 * ez);
                        }
                    }
                    for c in 0..3 {
                        row[x * 3 + c] = (rgb[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            });
    }
    let minimap_block = minimap::encode(&mm_rgb);
    stage.mark("Minimap");

    /* -- SMF ------------------------------------------------------------- */
    let smf_name = format!("{lower}.smf");
    let smf_path = maps_dir.join(&smf_name);
    let smt_refs = [smf::SmtRef {
        file_name: smt_name.clone(),
        tile_count: tile_set.count,
    }];
    let layers = smf::Layers {
        heightmap: &heightmap,
        typemap: &typemap,
        metalmap: &metalmap,
        grassmap: grassmap.as_deref(),
        minimap: &minimap_block,
        tile_index: &tile_set.index,
        smt_files: &smt_refs,
    };
    {
        let mut f = io::BufWriter::new(fs::File::create(&smf_path)?);
        smf::write(
            &mut f,
            &d,
            decl_min as f32,
            decl_max as f32,
            opts.map_id,
            &layers,
        )?;
    }

    stage.mark("SMF");

    /* -- SSMF resources, sized to hold a shipped map's detail ------------ */
    let ssmf = ssmf_res(f64::from(d.elmos_x).max(f64::from(d.elmos_y)));
    let splat_png = {
        let field = match graph.find_terminal("splat") {
            Some(id) => graph.evaluate(id, &ctx),
            None => std::sync::Arc::new(Field::new(res, 4)),
        };
        encode(
            ssmf,
            ssmf,
            PngColor::Rgba,
            8,
            &bake_rgba(&field, ssmf, ssmf),
            Compression::Deflate,
        )
    };
    let spec_png = {
        // Water is specular, land is matte. A real specular map is art; this
        // is a defensible starting point the author can replace.
        let mut samples = vec![0u16; ssmf * ssmf * 4];
        let hs = (res - 1) as f64 / (ssmf - 1) as f64;
        samples
            .par_chunks_mut(ssmf * 4)
            .enumerate()
            .for_each(|(y, row)| {
                for x in 0..ssmf {
                    let v = springen_core::field::sample_bilinear(
                        &height_gray,
                        x as f64 * hs,
                        y as f64 * hs,
                    );
                    let wet = if v < sea_t { 1.0 } else { 0.0 };
                    let s = (26.0 + 200.0 * wet) as u16;
                    row[x * 4] = s;
                    row[x * 4 + 1] = s;
                    row[x * 4 + 2] = s;
                    row[x * 4 + 3] = 255;
                }
            });
        encode(
            ssmf,
            ssmf,
            PngColor::Rgba,
            8,
            &samples,
            Compression::Deflate,
        )
    };

    stage.mark("Splat and specular");

    let tile = |w: usize, data: &[u8], colour: PngColor| {
        let s: Vec<u16> = data.iter().map(|v| u16::from(*v)).collect();
        encode(w, w, colour, 8, &s, Compression::Deflate)
    };

    /* -- blueprint tree --------------------------------------------------- */
    let mut resources = Resources {
        smt_files: vec![smt_name.clone()],
        splat_distr_tex: Some(format!("{lower}_splat.png")),
        specular_tex: Some(format!("{lower}_spec.png")),
        detail_tex: Some(format!("{lower}_detail.png")),
        splat_detail_tex: Some(format!("{lower}_splatdetail.png")),
        splat_detail_normal_tex: std::array::from_fn(|i| Some(format!("{lower}_n{i}.png"))),
        ..Default::default()
    };

    let mut bp = Blueprint::new();
    bp.add_file(format!("maps/{smf_name}"), smf_path.clone());
    bp.add_file(format!("maps/{smt_name}"), smt_path.clone());
    bp.add_bytes(format!("maps/{lower}_splat.png"), splat_png);
    bp.add_bytes(format!("maps/{lower}_spec.png"), spec_png);

    // Only what mapinfo names is shipped: the per-channel albedos are blended
    // into the diffuse above, and Spring has no slot that would read them.
    let tres = mats.albedo[0].res;
    bp.add_bytes(
        format!("maps/{lower}_detail.png"),
        tile(mats.detail_res, &mats.detail, PngColor::Rgb),
    );
    for (i, r) in mats.albedo.iter().enumerate() {
        bp.add_bytes(
            format!("maps/{lower}_n{i}.png"),
            tile(tres, &r.normal, PngColor::Rgba),
        );
    }
    // The legacy splat path wants one RGBA tile whose channels line up with
    // the distribution's, so each channel carries that material's luminance.
    {
        let mut packed = vec![0u8; tres * tres * 4];
        for c in 0..4 {
            let a = &mats.albedo[c].albedo;
            for i in 0..tres * tres {
                let l = 0.299 * f64::from(a[i * 3])
                    + 0.587 * f64::from(a[i * 3 + 1])
                    + 0.114 * f64::from(a[i * 3 + 2]);
                packed[i * 4 + c] = l.round().clamp(0.0, 255.0) as u8;
            }
        }
        bp.add_bytes(
            format!("maps/{lower}_splatdetail.png"),
            tile(tres, &packed, PngColor::Rgba),
        );
    }

    if opts.external_info_tex {
        let gray_png = |w: usize, h: usize, data: &[u8]| {
            let s: Vec<u16> = data.iter().map(|v| u16::from(*v)).collect();
            encode(w, h, PngColor::Gray, 8, &s, Compression::Deflate)
        };
        bp.add_bytes(
            format!("maps/{lower}_metal.png"),
            gray_png(d.metal_w as usize, d.metal_h as usize, &metalmap),
        );
        bp.add_bytes(
            format!("maps/{lower}_type.png"),
            gray_png(d.type_w as usize, d.type_h as usize, &typemap),
        );
        resources.metalmap_tex = Some(format!("{lower}_metal.png"));
        resources.typemap_tex = Some(format!("{lower}_type.png"));
        if let Some(g) = &grassmap {
            bp.add_bytes(
                format!("maps/{lower}_grass.png"),
                gray_png(d.grass_w as usize, d.grass_h as usize, g),
            );
            resources.grassmap_tex = Some(format!("{lower}_grass.png"));
        }
    }

    bp.add_text(
        "mapinfo.lua",
        &mapinfo_lua(project, &d, &resources, (decl_min, decl_max)),
    );
    bp.add_text("mapoptions.lua", &mapoptions_lua());
    bp.add_text(
        "mapconfig/map_metal_layout.lua",
        &metal_layout_lua_with_sets(
            &spots,
            &MetalLayoutOptions {
                symmetry: Some(project.mex_sym.clone()),
                ..Default::default()
            },
        ),
    );
    let mut teams = 0usize;
    if opts.emit_startboxes {
        let ground = StartGround::new(&height_gray, &ctx, build_opts.clone());
        // Hand-edited boxes if the project has any, otherwise whatever the
        // symmetry implies. The start *points* are chosen from the terrain
        // either way — an edited box says where a team starts, not where its
        // commander stands.
        let boxes = match &project.start_boxes {
            Some(areas) => {
                springen_core::lua::startboxes_from(areas, &d, &project.mex_sym, Some(&ground))
            }
            None => startboxes_on(&d, &project.mex_sym, Some(&ground)),
        };
        teams = boxes.len();
        for b in boxes.iter().filter(|b| !b.grounded) {
            issues.push(format!(
                "The {} start box has no buildable ground in it; its start point is the box centre and may be under water.",
                b.name_long
            ));
        }
        // Fair start boxes on landmasses that cannot reach each other is a map
        // where the ground war never happens, and it looks completely fine in
        // every other report. The pathability probe already labels every
        // connected region, so this costs one lookup per team.
        let tanks = analysis::pathability_for(
            &height_gray,
            &ctx,
            analysis::UnitClass::Tank.max_slope_deg(),
            sea_t,
            false,
        );
        let region = |p: (f64, f64)| -> i32 {
            let r = ctx.res as isize;
            let x = ((p.0 * ctx.px_per_elmo_x()).round() as isize).clamp(0, r - 1);
            let z = ((p.1 * ctx.px_per_elmo_y()).round() as isize).clamp(0, r - 1);
            tanks.label[z as usize * ctx.res + x as usize]
        };
        let named: Vec<(&str, i32)> = boxes
            .iter()
            .map(|b| (b.name_short.as_str(), region(b.start_points[0])))
            .collect();
        let stranded: Vec<&str> = named
            .iter()
            .filter(|(_, id)| *id == 0)
            .map(|(n, _)| *n)
            .collect();
        if !stranded.is_empty() {
            issues.push(format!(
                "Start box(es) {} sit on ground tanks cannot stand on.",
                stranded.join(", ")
            ));
        }
        let live: Vec<i32> = named
            .iter()
            .map(|(_, id)| *id)
            .filter(|id| *id != 0)
            .collect();
        if live.len() > 1 && live.iter().any(|id| *id != live[0]) {
            issues.push(format!(
                "The start boxes are on {} separate landmasses — ground units cannot reach each other.",
                {
                    let mut u = live.clone();
                    u.sort_unstable();
                    u.dedup();
                    u.len()
                }
            ));
        }
        bp.add_text("mapconfig/map_startboxes.lua", &startboxes_lua(&boxes));
    }
    if !geos.is_empty() {
        let features: Vec<PlacedFeature> = geos
            .iter()
            .map(|g| PlacedFeature::geovent(g.x, g.z))
            .collect();
        bp.add_text(
            "mapconfig/featureplacer/set.lua",
            &featureplacer_lua(&features),
        );
    }
    // The merge directory mapinfo.lua reads map options from.
    bp.add_text(
        "mapconfig/mapinfo/README.txt",
        "Lua files here are merged over mapinfo, sorted by filename.\n",
    );
    // The graph travels with the map, so any bake can be reproduced or edited.
    bp.add_text(
        "springen/project.json",
        &serde_json::to_string_pretty(&serde_json::json!({
            "project": project,
            "graph": graph.serialize(),
        }))
        .map_err(io::Error::other)?,
    );

    /* -- report ----------------------------------------------------------- */
    let below = &height_gray;
    let water_fraction =
        below.data.iter().filter(|v| (**v as f64) < sea_t).count() as f64 / below.len() as f64;
    let path_reports = analysis::pathability_by_class(&height, &ctx, sea_t)
        .into_iter()
        .map(|r| {
            (
                r.class.name().to_string(),
                r.passable_fraction,
                r.largest_fraction,
                r.component_count,
            )
        })
        .collect();

    // A default factory footprint against a builder's slope tolerance, so the
    // number means "a base fits here", not "a unit can stand here".
    let flat = analysis::flatness(&height, &ctx, 96.0, 12.0, sea_t);
    // Against a tank's climb limit: the class that struggles first is the one
    // a chokepoint is measured for.
    let choke = analysis::chokepoints(
        &height,
        &ctx,
        analysis::UnitClass::Tank.max_slope_deg(),
        sea_t,
    );

    stage.mark("Playability analysis");

    let report = BakeReport {
        derived: d,
        eval_res: res,
        metal_per_player: zk::metal_per_player(&spots, opts.players),
        spots,
        issues,
        symmetric: sym.symmetric,
        symmetry: project.mex_sym.clone(),
        tile_slots: tile_set.slots,
        tiles_stored: tile_set.count,
        dedup_ratio: tile_set.dedup_ratio(),
        smf_bytes: fs::metadata(&smf_path)?.len(),
        smt_bytes: fs::metadata(&smt_path)?.len(),
        metal_blobs,
        water_fraction,
        declared_range: (decl_min, decl_max),
        pathability: path_reports,
        flatness: flat,
        choke,
        unbuildable_spots: unbuildable,
        players: opts.players,
        geos,
        teams,
    };
    Ok((bp, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use springen_core::starter::starter_graph;

    #[test]
    fn a_small_map_bakes_to_a_complete_tree() {
        let project = Project {
            name: "Bake Test".into(),
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        let g = starter_graph("textured");
        let dir = std::env::temp_dir().join("springen-bake-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let opts = BakeOptions {
            spot_count: 6,
            min_separation: 400.0,
            ..Default::default()
        };
        let (bp, report) = bake(&project, &g, &opts, &dir).unwrap();
        assert!(!report.spots.is_empty(), "issues: {:?}", report.issues);

        let names: Vec<&str> = bp.entries.iter().map(|e| e.path.as_str()).collect();
        for want in [
            "maps/baketest.smf",
            "maps/baketest.smt",
            "maps/baketest_splat.png",
            "maps/baketest_spec.png",
            "mapinfo.lua",
            "mapoptions.lua",
            "mapconfig/map_metal_layout.lua",
            "mapconfig/map_startboxes.lua",
            "springen/project.json",
        ] {
            assert!(names.contains(&want), "missing {want} in {names:?}");
        }

        // 4x4 units: mapx 256, height lattice 257, tiles 64x64.
        assert_eq!(report.derived.mapx, 256);
        assert_eq!(report.derived.height_w, 257);
        assert_eq!(report.tile_slots, 64 * 64);
        assert_eq!(
            report.smt_bytes,
            32 + u64::from(report.tiles_stored) * smt::TILE_BYTES as u64
        );

        // The SMF must parse back, with the sizes the header claims.
        let bytes = fs::read(dir.join("maps/baketest.smf")).unwrap();
        let h = smf::read_header(&bytes).unwrap();
        assert_eq!(h.width, 256);
        assert_eq!(h.length, 256);
        assert_eq!(bytes.len() as u64, report.smf_bytes);
        let ho = h.heightmap_ptr as usize;
        assert!(ho + 257 * 257 * 2 <= bytes.len());
        let (refs, _) = smf::read_tile_refs(&bytes, &h).unwrap();
        assert_eq!(refs[0].file_name, "baketest.smt");
        assert_eq!(refs[0].tile_count, report.tiles_stored);

        // Zero-K default: blank raster, metal comes from the Lua layout.
        assert_eq!(report.metal_blobs, 0);
        let layout = bp
            .entries
            .iter()
            .find(|e| e.path == "mapconfig/map_metal_layout.lua")
            .unwrap();
        if let crate::pack::Source::Bytes(b) = &layout.source {
            let text = String::from_utf8(b.clone()).unwrap();
            assert!(text.contains("metal ="), "{text}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn generic_spring_gets_discrete_blobs_instead() {
        let project = Project {
            name: "Blobs".into(),
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        let g = starter_graph("ridge");
        let dir = std::env::temp_dir().join("springen-bake-blobs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let opts = BakeOptions {
            game: Game::Spring,
            spot_count: 6,
            min_separation: 400.0,
            ..Default::default()
        };
        let (_bp, report) = bake(&project, &g, &opts, &dir).unwrap();
        assert!(
            report.metal_blobs > 0,
            "generic Spring needs a painted raster"
        );
        assert_eq!(report.metal_blobs, report.spots.len());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unwired_heightmap_terminal_is_refused() {
        let project = Project {
            name: "Flat".into(),
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        // A Heightmap out node on its own bakes a perfectly level plane, and
        // every other check passes: the archive loads and the map is wrong.
        let mut g = Graph::default();
        g.add("out_height", 0.0, 0.0, &[]);
        let dir = std::env::temp_dir().join("springen-bake-flat");
        let Err(e) = bake(&project, &g, &BakeOptions::default(), &dir) else {
            panic!("an unwired heightmap terminal must be refused");
        };
        assert!(e.to_string().contains("nothing connected"), "{e}");

        // And the message is different from the one for no node at all.
        let Err(e) = bake(&project, &Graph::default(), &BakeOptions::default(), &dir) else {
            panic!("a graph with no heightmap terminal must be refused");
        };
        assert!(e.to_string().contains("no Heightmap out node"), "{e}");
    }

    #[test]
    fn an_odd_size_is_refused_with_the_engine_reason() {
        let project = Project {
            name: "Odd".into(),
            units_x: 9,
            units_y: 10,
            ..Default::default()
        };
        let g = starter_graph("ridge");
        let dir = std::env::temp_dir().join("springen-bake-odd");
        let Err(e) = bake(&project, &g, &BakeOptions::default(), &dir) else {
            panic!("an odd size unit must be refused");
        };
        assert!(e.to_string().contains("divide by 128"), "{e}");
    }

    /// The distribution map has to hold at least as much detail per elmo as a
    /// shipped map's does, at every size we can bake.
    #[test]
    fn the_splat_distribution_is_never_coarser_than_a_shipped_map() {
        // The reference: 5120 elmos across a 1024 texture.
        const REFERENCE: f64 = 5120.0 / 1024.0;
        for units in [2u32, 4, 8, 12, 16, 24, 32, 64] {
            let elmos = f64::from(units) * 512.0;
            let r = ssmf_res(elmos);
            assert!(r.is_power_of_two(), "{units}: {r} is not a power of two");
            assert!(r >= 1024, "{units}: {r} is coarser than the old fixed size");
            assert!(r <= 4096, "{units}: {r} is past the cap");
            if r < 4096 {
                let per_px = elmos / r as f64;
                assert!(
                    per_px <= REFERENCE + 1e-9,
                    "{units}x{units}: {per_px:.1} elmos a pixel against the reference {REFERENCE:.1}"
                );
            }
        }
        // The old behaviour is preserved exactly where it was already good
        // enough, so small maps do not silently grow.
        assert_eq!(ssmf_res(4096.0), 1024);
    }
}

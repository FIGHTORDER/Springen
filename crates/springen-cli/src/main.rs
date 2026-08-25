//! Headless map baking.
//!
//! The graph evaluator is fully decoupled from any UI, which is what makes
//! this possible — and what makes CI-generated map packs plausible.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use springen_archive::{bake_with_progress, BakeOptions, BakeReport, Game};
use springen_core::bake::HeightMode;
use springen_core::graph::{registry, Graph};
use springen_core::lua::{mapconv_command, MapconvFiles};
use springen_core::preview::{render, PreviewOptions, ViewMode};
use springen_core::project::{height_range_for, water_level_t, Project};
use springen_core::raster::Rasters;
use springen_core::spring::{derive, size_rejection};
use springen_core::starter::{starter_graph, STARTERS};

#[derive(Parser)]
#[command(
    name = "springen",
    about = "Procedural map generation for Spring / Recoil and Zero-K",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// `Bake` carries two dozen flags and the rest carry two or three. Boxing it
// to even the variants out would cost an indirection on the one path that
// actually does work.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Write a starter project you can edit and re-bake.
    New {
        /// Where to write the project JSON.
        #[arg(short, long, default_value = "project.json")]
        out: PathBuf,
        #[arg(long, default_value = "textured")]
        starter: String,
        #[arg(long, default_value = "Untitled")]
        name: String,
        #[arg(long, default_value = "1.0")]
        map_version: String,
        /// Size in 512-elmo units, as NxM. Both must be even.
        #[arg(long, default_value = "12x12")]
        size: String,
        #[arg(long, default_value_t = 20250815)]
        seed: i64,
    },
    /// Bake a project into a loadable map.
    Bake(BakeArgs),
    /// Render a view of the terrain to a PNG, without baking a map.
    ///
    /// A full bake is minutes at 24x24; this is seconds, and it is the same
    /// painting the workstation's viewport shows.
    Preview {
        /// Project JSON. Without one, a starter graph is used.
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long)]
        starter: Option<String>,
        #[arg(short, long, default_value = "preview.png")]
        out: PathBuf,
        /// Field resolution. 512 is quick; 1025 is what an 16x16 map bakes at.
        #[arg(long, default_value_t = 512)]
        res: usize,
        /// Relief, Diffuse, Metal, Type, Slope or Pathability.
        #[arg(long, default_value = "relief")]
        view: String,
        #[arg(long)]
        size: Option<String>,
        #[arg(long)]
        seed: Option<i64>,
        /// Climb limit the Slope and Pathability views are drawn against.
        #[arg(long, default_value_t = 18.0)]
        climb: f64,
        #[arg(long, default_value_t = 14)]
        spots: usize,
        #[arg(long, default_value_t = 700.0)]
        separation: f64,
    },
    /// Render a patch of ground at engine scale, with the detail tile on.
    ///
    /// No map-wide view can show a detail tile: it repeats every 50 elmos, so
    /// a pixel of a whole-map preview covers many copies of it and correctly
    /// averages it to nothing. This is the view that shows the surface a unit
    /// actually stands on.
    Ground {
        /// Project JSON. Without one, a starter graph is used.
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long)]
        starter: Option<String>,
        #[arg(short, long, default_value = "ground.png")]
        out: PathBuf,
        #[arg(long, default_value_t = 512)]
        res: usize,
        /// How much ground the image covers, in elmos. The detail tile
        /// repeats every 50, so 120 shows a couple of copies of it.
        #[arg(long, default_value_t = 120.0)]
        elmos: f64,
        /// Which splat channel to stand on: 0-3, or -1 for all four side by
        /// side.
        #[arg(long, default_value_t = -1)]
        channel: i32,
    },
    /// Emit a mapconv command line and script.
    ///
    /// The native SMF/SMT writer is the primary path and needs none of this.
    /// It is the escape hatch for when you want the engine's own converter.
    Mapconv {
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long)]
        size: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// Write a POSIX shell script instead of a Windows batch file.
        #[arg(long)]
        sh: bool,
        /// Where to write the script. Without it, the script goes to stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 1.0)]
        compression: f64,
    },
    /// List the sun, sky, fog and water presets.
    Environments,
    /// List the detail materials, or draw a contact sheet of them.
    Materials {
        /// Write a sheet showing every material tiled 2x2 with its normal map.
        #[arg(long)]
        sheet: Option<PathBuf>,
        /// Tile resolution in the sheet.
        #[arg(long, default_value_t = 160)]
        res: usize,
    },
    /// Report what a size unit implies, without baking anything.
    Size {
        /// Size in 512-elmo units, as NxM.
        size: String,
    },
    /// Read an existing SMF and report its header and block layout.
    Inspect { file: PathBuf },
    /// Import an existing map as an editable project.
    ///
    /// Takes a .sd7 archive, a .sdd folder or a bare .smf, and writes a
    /// project folder: project.json beside a rasters/ directory holding the
    /// terrain. From there it is an ordinary Springen project — grade it,
    /// ramp it, re-texture it, re-bake it.
    Import {
        /// The map to read.
        file: PathBuf,
        /// Project folder to write. Defaults to the map's own name.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Map name for the new project. Defaults to the file stem.
        #[arg(long)]
        name: Option<String>,
    },
    /// List the node inventory.
    Nodes,
}

#[derive(clap::Args, Clone)]
struct BakeArgs {
    /// Project JSON. Without one, a starter graph is used.
    #[arg(short, long)]
    project: Option<PathBuf>,
    #[arg(long)]
    starter: Option<String>,
    /// Output path. The extension picks the format: .sd7, .sdz or .sdd.
    /// Defaults to <documents>/Springen/<name>-v<version>.sd7, because the
    /// working directory may well be somewhere unwritable.
    #[arg(short, long)]
    out: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    author: Option<String>,
    /// The map version. Zero-K refuses an upload when the name and version
    /// already exist, so bump this to publish again. Named `map-version`
    /// because `--version` prints the tool's own.
    #[arg(long)]
    map_version: Option<String>,
    #[arg(long)]
    size: Option<String>,
    #[arg(long)]
    seed: Option<i64>,
    /// Metal raster convention.
    #[arg(long, value_enum, default_value_t = GameArg::Zk)]
    game: GameArg,
    #[arg(long, default_value_t = 14)]
    spots: usize,
    /// Minimum spot separation, in elmos.
    #[arg(long, default_value_t = 700.0)]
    separation: f64,
    /// Metal per spot. Zero-K's DEFAULT_MEX_INCOME is 2.0 and anything at
    /// or below 0.2 is discarded by the gadget.
    #[arg(long, default_value_t = 2.0)]
    metal: f64,
    /// Steepest ground a mex footprint may sit on, in degrees.
    #[arg(long, default_value_t = 12.0)]
    max_spot_slope: f64,
    /// Blob radius in the painted metal raster, in elmos. Only --game
    /// spring paints one.
    #[arg(long, default_value_t = 48.0)]
    blob_radius: f64,
    /// Geothermal vents, written as mapconfig/featureplacer/set.lua.
    #[arg(long, default_value_t = 0)]
    geos: usize,
    /// Players the metal-per-player readout is divided between.
    #[arg(long, default_value_t = 2)]
    players: usize,
    /// Do not write mapconfig/map_startboxes.lua.
    #[arg(long)]
    no_startboxes: bool,
    /// Detail materials for the four splat channels, as
    /// `rock,gravel,grass,sand`. `springen materials` lists them.
    #[arg(long, value_delimiter = ',')]
    materials: Option<Vec<String>>,
    /// The tile Spring multiplies over the whole diffuse.
    #[arg(long)]
    detail_material: Option<String>,
    /// How hard the materials colour the baked ground, 0..1.
    #[arg(long)]
    material_blend: Option<f64>,
    /// Sun, sky, fog and water preset. `springen materials` has a sibling:
    /// run `springen environments` to list these.
    #[arg(long)]
    environment: Option<String>,
    /// Sun bearing and height above the horizon, in degrees.
    #[arg(long)]
    sun_azimuth: Option<f64>,
    #[arg(long)]
    sun_elevation: Option<f64>,
    /// Submerged fraction, 0..1. Sets minHeight and maxHeight from the
    /// current vertical range, since height 0 elmos is the waterline.
    #[arg(long)]
    water: Option<f64>,
    /// Total height in elmos from the map's lowest point to its highest.
    /// Lower it to flatten the whole map: peaks come down and hollows come up
    /// together, and the waterline stays where it is.
    #[arg(long)]
    relief: Option<f64>,
    /// Cap the water depth in elmos below the waterline, without moving the
    /// coastline. Deep water blocks anything that is not a ship; this shoals
    /// the sea floor so bots can ford it. 0 removes the cap.
    #[arg(long)]
    max_depth: Option<f64>,
    #[arg(long, value_enum, default_value_t = HeightArg::Fit)]
    height_mode: HeightArg,
    /// Graph evaluation resolution. Defaults to the vertex lattice size.
    #[arg(long)]
    eval_res: Option<usize>,
    /// Also ship metal, type and grass as external images.
    #[arg(long)]
    external_tex: bool,
    #[arg(long, default_value_t = 1)]
    map_id: u32,
    /// Re-bake whenever the project file changes.
    ///
    /// Point it at a .sdd folder: the engine loads one directly, so the loop
    /// is edit, save, reload the map in game.
    #[arg(long)]
    watch: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum GameArg {
    /// Blank raster plus the Lua layout, as real Zero-K maps ship.
    Zk,
    /// Discrete blobs painted from the spot list.
    Spring,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum HeightArg {
    /// Stretch the terrain to fill the declared range.
    Fit,
    /// Let the terrain sit inside a fixed vertical scale.
    Absolute,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (a, b) = s
        .split_once(['x', 'X', ','])
        .ok_or_else(|| format!("Size {s} is not in NxM form, for example 12x12."))?;
    let x: u32 = a
        .trim()
        .parse()
        .map_err(|_| format!("Size unit {a} is not a whole number."))?;
    let y: u32 = b
        .trim()
        .parse()
        .map_err(|_| format!("Size unit {b} is not a whole number."))?;
    if let Some(reason) = size_rejection(x, y) {
        return Err(reason);
    }
    Ok((x, y))
}

/// Load a project and its graph from JSON, or fall back to a starter graph.
type Loaded = (Project, Graph, std::sync::Arc<Rasters>);

/// Load a project, its graph, and any rasters stored beside it.
///
/// A project that carries an imported map is a folder — `project.json` next to
/// `rasters/` — so loading one means loading both. A purely procedural project
/// simply has no `rasters/` directory and gets an empty store.
fn load(project: &Option<PathBuf>, starter: &Option<String>) -> Result<Loaded, String> {
    match project {
        Some(p) => {
            // Point it at the folder and it finds the project inside.
            let p = &if p.is_dir() {
                p.join("project.json")
            } else {
                p.clone()
            };
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let doc: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            let pj = doc.get("project").unwrap_or(&doc);
            let gj = doc
                .get("graph")
                .ok_or_else(|| format!("{} has no graph key.", p.display()))?;
            let rasters = springen_archive::import::read_project_rasters(p)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            Ok((
                Project::from_json(pj),
                Graph::deserialize(gj),
                std::sync::Arc::new(rasters),
            ))
        }
        None => {
            let kind = starter.clone().unwrap_or_else(|| "textured".into());
            let known: Vec<&str> = STARTERS.iter().map(|(k, _)| *k).collect();
            if !known.contains(&kind.as_str()) {
                return Err(format!(
                    "No starter named {kind}. Available: {}.",
                    known.join(", ")
                ));
            }
            Ok((
                springen_core::starter::starter_project(&kind),
                starter_graph(&kind),
                Default::default(),
            ))
        }
    }
}

fn bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if n as f64 >= MB {
        format!("{:.1} MB", n as f64 / MB)
    } else {
        format!("{:.1} kB", n as f64 / 1024.0)
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Nodes => {
            let reg = registry();
            for cat in reg.categories() {
                println!("{cat}");
                for spec in reg.all().filter(|s| s.cat == cat) {
                    let ports = if spec.inputs.is_empty() {
                        "-".to_string()
                    } else {
                        spec.inputs.join(", ")
                    };
                    println!("  {:<14} {:<26} in: {}", spec.type_name, spec.label, ports);
                }
            }
            println!("\n{} node types.", reg.len());
            Ok(())
        }

        Command::Ground {
            project,
            starter,
            out,
            res,
            elmos,
            channel,
        } => {
            let (proj, _graph, _rasters) = load(&project, &starter)?;
            let res = res.clamp(16, 2048);
            let mats = springen_core::material::render_set(
                &proj.materials,
                f64::from(springen_core::Context::new(&proj, 2).seed) + 4241.0,
            );
            let channels: Vec<usize> = if (0..4).contains(&channel) {
                vec![channel as usize]
            } else {
                (0..4).collect()
            };
            let cols = channels.len();
            let mut px = vec![0u16; res * cols * res * 3];
            for (slot, ch) in channels.iter().enumerate() {
                let mut w = [0.0; 4];
                w[*ch] = 1.0;
                let cell = springen_core::material::ground_sample(
                    &mats,
                    springen_core::material::DEFAULT_TEX_SCALES,
                    1.0,
                    w,
                    [0.5, 0.5, 0.5],
                    res,
                    elmos,
                );
                for y in 0..res {
                    for x in 0..res {
                        for c in 0..3 {
                            px[(y * res * cols + slot * res + x) * 3 + c] =
                                u16::from(cell[(y * res + x) * 3 + c]);
                        }
                    }
                }
            }
            let png = springen_core::png::encode(
                res * cols,
                res,
                springen_core::png::PngColor::Rgb,
                8,
                &px,
                springen_core::png::Compression::Deflate,
            );
            if let Some(dir) = out.parent() {
                if !dir.as_os_str().is_empty() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
            }
            std::fs::write(&out, &png).map_err(|e| format!("{}: {e}", out.display()))?;
            let names: Vec<&str> = channels
                .iter()
                .map(|c| proj.materials.splat[*c].as_str())
                .collect();
            println!(
                "Wrote {} — {elmos:.0} elmos of ground at {res}px, detail tile {}, channels: {}.",
                out.display(),
                proj.materials.detail,
                names.join(", ")
            );
            Ok(())
        }

        Command::Preview {
            project,
            starter,
            out,
            res,
            view,
            size,
            seed,
            climb,
            spots,
            separation,
        } => {
            let (mut proj, graph, rasters) = load(&project, &starter)?;
            if let Some(s) = size {
                let (x, y) = parse_size(&s)?;
                proj.units_x = x;
                proj.units_y = y;
            }
            if let Some(s) = seed {
                proj.seed = s;
            }
            let mode = ViewMode::from_name(&view).ok_or_else(|| {
                let all: Vec<&str> = ViewMode::ALL.iter().map(|m| m.label()).collect();
                format!("No view named {view}. Available: {}.", all.join(", "))
            })?;
            let res = res.clamp(2, 4097);

            // The metal view needs a spot list, and proposing one needs the
            // terrain, so the height field is evaluated first either way.
            let mut placed = Vec::new();
            if mode == ViewMode::Metal {
                let c129 = springen_core::Context::new(&proj, 129);
                let sea = water_level_t(proj.min_height, proj.max_height);
                if let Some(hid) = graph.find_terminal("height").map(str::to_string) {
                    let terrain = springen_core::field::as_gray(&graph.evaluate(&hid, &c129));
                    let mask = match graph.find_terminal("metal") {
                        Some(id) => springen_core::field::as_gray(&graph.evaluate(id, &c129)),
                        None => std::sync::Arc::new(springen_core::Field::gray(129)),
                    };
                    placed = springen_core::zk::propose_spots_on(
                        &mask,
                        Some(&terrain),
                        &c129,
                        &springen_core::zk::ProposeOptions {
                            count: spots,
                            min_separation: separation,
                            symmetry: proj.mex_sym.clone(),
                            build: Some(springen_core::zk::BuildabilityOptions {
                                sea_level: sea,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    );
                }
            }

            let started = std::time::Instant::now();
            // The Diffuse view is the one that needs them, and rendering four
            // tiles is about a second, so it is skipped for the other five.
            let mats = (mode == ViewMode::Diffuse).then(|| {
                springen_core::material::render_set(
                    &proj.materials,
                    f64::from(springen_core::Context::new(&proj, 2).seed) + 4241.0,
                )
            });
            let preview = render(
                &graph,
                &proj,
                &PreviewOptions {
                    rasters: rasters.clone(),
                    res,
                    mode,
                    climb_limit: climb,
                    spots: &placed,
                    materials: mats.as_ref(),
                },
            )
            .ok_or("The graph has no Heightmap out node, so there is nothing to preview.")?;
            // The lattice is square; the world need not be. Written at the
            // world's shape, or a 16x8 map arrives as a square picture with
            // its Z axis stretched to twice its width.
            let d0 = derive(proj.units_x, proj.units_y);
            let (vw, vh) = springen_core::preview::view_size(
                f64::from(d0.units_x * 512),
                f64::from(d0.units_y * 512),
                res,
            );
            let shown = springen_core::preview::to_view_size(&preview.colour, res, vw, vh);
            let samples: Vec<u16> = shown.iter().map(|v| u16::from(*v)).collect();
            let png = springen_core::png::encode(
                vw,
                vh,
                springen_core::png::PngColor::Rgb,
                8,
                &samples,
                springen_core::png::Compression::Deflate,
            );
            if let Some(dir) = out.parent() {
                if !dir.as_os_str().is_empty() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
            }
            std::fs::write(&out, &png).map_err(|e| format!("{}: {e}", out.display()))?;
            let d = derive(proj.units_x, proj.units_y);
            println!(
                "Wrote {} — {} at {vw}x{vh} for a {}x{} map, {:.1}s.",
                out.display(),
                mode.label(),
                d.units_x,
                d.units_y,
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }

        Command::Mapconv {
            project,
            size,
            name,
            sh,
            out,
            compression,
        } => {
            let (mut proj, _graph, _rasters) = load(&project, &None)?;
            if let Some(s) = size {
                let (x, y) = parse_size(&s)?;
                proj.units_x = x;
                proj.units_y = y;
            }
            if let Some(n) = name {
                proj.name = n;
            }
            let d = derive(proj.units_x, proj.units_y);
            let lower = proj.short_name().to_lowercase();
            let files = MapconvFiles {
                texture: Some(format!("{lower}_diffuse.png")),
                height: Some(format!("{lower}_height.png")),
                metal: Some(format!("{lower}_metal.png")),
                type_map: Some(format!("{lower}_type.png")),
                feature: None,
            };
            let script = mapconv_command(&proj, &d, &files, compression, sh);
            match out {
                Some(p) => {
                    std::fs::write(&p, format!("{}\n", script.script))
                        .map_err(|e| format!("{}: {e}", p.display()))?;
                    println!("Wrote {}", p.display());
                }
                None => println!("{}", script.script),
            }
            Ok(())
        }

        Command::Environments => {
            println!("{} environment presets", springen_core::env::PRESETS.len());
            for (k, about) in springen_core::env::PRESETS {
                println!("  {k:<10} {about}");
            }
            println!();
            println!("Set one with --environment on bake, or in the app's inspector.");
            println!("A skyBox cubemap is not generated yet.");
            Ok(())
        }

        Command::Materials { sheet, res } => {
            let all = springen_core::material::MATERIALS;
            let Some(out) = sheet else {
                println!("{} detail materials", all.len());
                for m in all {
                    println!("  {:<8} {:<10} {}", m.key, m.label, m.about);
                }
                println!();
                println!("Set them per splat channel with --materials on bake, or in the app.");
                return Ok(());
            };
            // Each row: the tile repeated 2x2 so any seam would show, then
            // its normal map beside it.
            let t = res.clamp(32, 512);
            let w = t * 3;
            let h = all.len() * t * 2;
            let mut img = vec![0u16; w * h * 3];
            for (i, m) in all.iter().enumerate() {
                let r = springen_core::material::render(m, t, 11.0, 1.0);
                let y0 = i * t * 2;
                for y in 0..t * 2 {
                    for x in 0..t * 2 {
                        let o = ((y0 + y) * w + x) * 3;
                        let si = ((y % t) * t + (x % t)) * 3;
                        for c in 0..3 {
                            img[o + c] = u16::from(r.albedo[si + c]);
                        }
                    }
                }
                for y in 0..t {
                    for x in 0..t {
                        let o = ((y0 + t / 2 + y) * w + t * 2 + x) * 3;
                        for c in 0..3 {
                            img[o + c] = u16::from(r.normal[(y * t + x) * 4 + c]);
                        }
                    }
                }
            }
            let png = springen_core::png::encode(
                w,
                h,
                springen_core::png::PngColor::Rgb,
                8,
                &img,
                springen_core::png::Compression::Deflate,
            );
            std::fs::write(&out, png).map_err(|e| format!("{}: {e}", out.display()))?;
            println!(
                "Wrote {} — {} materials at {t}², each tiled 2x2 with its normal map.",
                out.display(),
                all.len()
            );
            Ok(())
        }

        Command::Size { size } => {
            let (x, y) = parse_size(&size)?;
            let d = derive(x, y);
            println!("Size {}x{} units", d.units_x, d.units_y);
            println!("  World          {} × {} elmos", d.elmos_x, d.elmos_y);
            println!("  mapx / mapy    {} × {}", d.mapx, d.mapy);
            println!(
                "  Heightmap      {} × {} (16-bit, vertex lattice)",
                d.height_w, d.height_h
            );
            println!("  Diffuse        {} × {}", d.tex_w, d.tex_h);
            println!(
                "  Tile grid      {} × {} ({} tiles, worst case {})",
                d.tiles_x,
                d.tiles_y,
                d.tile_count,
                bytes(d.smt_worst_case)
            );
            println!("  Metal / type   {} × {}", d.metal_w, d.metal_h);
            println!("  Grass          {} × {}", d.grass_w, d.grass_h);
            println!("  Minimap        1024 × 1024");
            Ok(())
        }

        Command::New {
            out,
            starter,
            name,
            map_version,
            size,
            seed,
        } => {
            let (x, y) = parse_size(&size)?;
            let known: Vec<&str> = STARTERS.iter().map(|(k, _)| *k).collect();
            if !known.contains(&starter.as_str()) {
                return Err(format!(
                    "No starter named {starter}. Available: {}.",
                    known.join(", ")
                ));
            }
            // A sample map is a whole map: its surfaces and its light are as
            // much a part of it as its terrain.
            let mut project = springen_core::starter::starter_project(&starter);
            project.name = name;
            project.version = map_version;
            project.units_x = x;
            project.units_y = y;
            project.seed = seed;
            let graph = starter_graph(&starter);
            let doc = serde_json::json!({
                "project": project,
                "graph": graph.serialize(),
            });
            if let Some(p) = out.parent() {
                if !p.as_os_str().is_empty() {
                    std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
            }
            std::fs::write(
                &out,
                serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "Wrote {} — starter {starter}, {} nodes, {}x{} units.",
                out.display(),
                graph.nodes.len(),
                x,
                y
            );
            Ok(())
        }

        Command::Inspect { file } => {
            let bytes_in = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let h = springen_smf::smf::read_header(&bytes_in).map_err(|e| e.to_string())?;
            println!("{}", file.display());
            println!("  version          {}", h.version);
            println!("  map id           {}", h.map_id);
            println!(
                "  width x length   {} × {} squares ({} × {} elmos, units {}x{})",
                h.width,
                h.length,
                h.width * 8,
                h.length * 8,
                h.width / 64,
                h.length / 64
            );
            println!("  squareSize       {}", h.square_size);
            println!("  texelsPerSquare  {}", h.texels_per_square);
            println!("  tileSize         {}", h.tile_size);
            println!(
                "  height range     {} .. {} elmos",
                h.min_height, h.max_height
            );
            println!("  extra headers    {}", h.num_extra_headers);

            // Follow offsets: the physical order is not the field order.
            let mut blocks: Vec<(&str, i32)> = vec![
                ("heightmap", h.heightmap_ptr),
                ("typemap", h.typemap_ptr),
                ("tileindex", h.tiles_ptr),
                ("minimap", h.minimap_ptr),
                ("metalmap", h.metalmap_ptr),
                ("features", h.features_ptr),
            ];
            if let Some(g) = h.grass_ptr {
                blocks.push(("grassmap", g));
            }
            blocks.sort_by_key(|(_, o)| *o);
            println!("  blocks, in physical order:");
            for (i, (name, off)) in blocks.iter().enumerate() {
                let end = blocks
                    .get(i + 1)
                    .map(|(_, o)| *o)
                    .unwrap_or(bytes_in.len() as i32);
                println!("    {name:<10} {off:>10} .. {end:<10} = {}", end - off);
            }
            let (refs, _) =
                springen_smf::smf::read_tile_refs(&bytes_in, &h).map_err(|e| e.to_string())?;
            let slots = (h.width / 4) * (h.length / 4);
            let total: u32 = refs.iter().map(|r| r.tile_count).sum();
            for r in &refs {
                println!(
                    "  tile file        {} ({} tiles)",
                    r.file_name, r.tile_count
                );
            }
            if slots > 0 {
                println!(
                    "  dedup            {} slots, {} stored ({:.1}% saved)",
                    slots,
                    total,
                    100.0 * (1.0 - f64::from(total) / f64::from(slots))
                );
            }
            Ok(())
        }

        Command::Import { file, out, name } => {
            let map = springen_archive::read_map(&file)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Imported".into());
            let name = name.unwrap_or(stem.clone());
            let dir = out.unwrap_or_else(|| PathBuf::from(&stem));
            let imported = springen_archive::import::to_project(&map, &name);
            let path = springen_archive::import::write_project_dir(&dir, &imported)
                .map_err(|e| format!("{}: {e}", dir.display()))?;

            let (ex, ey) = map.elmos();
            println!("{}", file.display());
            println!("  SMF              {}", map.smf_path);
            println!(
                "  size             {} × {} squares — {} × {} elmos, units {}x{}",
                map.mapx, map.mapy, ex, ey, imported.project.units_x, imported.project.units_y
            );
            println!(
                "  height range     {} .. {} elmos, waterline {:.1}% up it",
                map.min_height,
                map.max_height,
                100.0 * water_level_t(map.min_height, map.max_height)
            );
            println!("  lattice          {} × {}", map.height_w, map.height_h);
            for n in &imported.notes {
                println!("  note: {n}");
            }
            println!();
            println!("Wrote {}", path.display());
            println!(
                "  Bake it with:  springen bake --project {} --out out/{}.sd7",
                path.display(),
                imported.project.short_name()
            );
            Ok(())
        }

        Command::Bake(args) => {
            bake_once(&args)?;
            if !args.watch {
                return Ok(());
            }
            let Some(path) = args.project.clone() else {
                return Err("--watch needs --project: there is nothing to watch otherwise.".into());
            };
            println!();
            println!(
                "Watching {} — save it to re-bake. Ctrl-C to stop.",
                path.display()
            );
            let stamp = |p: &Path| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
                    .map(|t| (t, std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)))
            };
            let mut last = stamp(&path);
            let mut n = 1u32;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let now = stamp(&path);
                if now == last || now.is_none() {
                    continue;
                }
                last = now;
                n += 1;
                println!();
                println!("--- rebake {n} ---");
                // A broken edit is not a reason to stop watching.
                if let Err(e) = bake_once(&args) {
                    eprintln!("{e}");
                }
            }
        }
    }
}

/// One pass of the bake command: load, override, evaluate, write, report.
fn bake_once(args: &BakeArgs) -> Result<(), String> {
    let BakeArgs {
        project,
        starter,
        out,
        name,
        author,
        map_version,
        size,
        seed,
        game,
        spots,
        separation,
        metal,
        max_spot_slope,
        blob_radius,
        geos,
        players,
        no_startboxes,
        relief,
        max_depth,
        materials,
        detail_material,
        material_blend,
        environment,
        sun_azimuth,
        sun_elevation,
        water,
        height_mode,
        eval_res,
        external_tex,
        map_id,
        watch: _,
    } = args.clone();
    let (mut proj, graph, rasters) = load(&project, &starter)?;
    if let Some(n) = name {
        proj.name = n;
    }
    if let Some(a) = author {
        proj.author = a;
    }
    if let Some(v) = map_version {
        proj.version = v;
    }
    if let Some(s) = size {
        let (x, y) = parse_size(&s)?;
        proj.units_x = x;
        proj.units_y = y;
    }
    if let Some(s) = seed {
        proj.seed = s;
    }
    if let Some(r) = relief {
        if !(1.0..=20000.0).contains(&r) {
            return Err(format!("--relief {r} is not a sensible height in elmos."));
        }
        // Keep the waterline where it is and rescale about it, or lowering the
        // relief would also drain or drown the map.
        let submerged = water_level_t(proj.min_height, proj.max_height);
        let (n, x) = height_range_for(submerged, r);
        proj.min_height = n;
        proj.max_height = x;
    }
    if let Some(d) = max_depth {
        if !(0.0..=20000.0).contains(&d) {
            return Err(format!("--max-depth {d} is not a sensible depth in elmos."));
        }
        proj.max_depth = if d > 0.0 { Some(d) } else { None };
    }
    if let Some(f) = water {
        if !(0.0..1.0).contains(&f) {
            return Err(format!(
                "--water {f} is not a fraction between 0 and 1. It is how much of the vertical range sits below the waterline."
            ));
        }
        let (n, x) = height_range_for(f, proj.height_range());
        proj.min_height = n;
        proj.max_height = x;
    }
    let known = || springen_core::material::keys().join(", ");
    if let Some(list) = materials {
        if list.len() > 4 {
            return Err(format!(
                "--materials takes at most four names, one per splat channel; got {}.",
                list.len()
            ));
        }
        for (i, name) in list.iter().enumerate() {
            if springen_core::material::find(name).is_none() {
                return Err(format!("No material named {name}. Available: {}.", known()));
            }
            proj.materials.splat[i] = name.clone();
        }
    }
    if let Some(name) = detail_material {
        if springen_core::material::find(&name).is_none() {
            return Err(format!("No material named {name}. Available: {}.", known()));
        }
        proj.materials.detail = name;
    }
    if let Some(b) = material_blend {
        if !(0.0..=1.0).contains(&b) {
            return Err(format!("--material-blend {b} is not between 0 and 1."));
        }
        proj.materials.blend = b;
    }
    if let Some(name) = environment {
        proj.environment = springen_core::env::preset(&name).ok_or_else(|| {
            format!(
                "No environment named {name}. Available: {}.",
                springen_core::env::preset_keys().join(", ")
            )
        })?;
    }
    if let Some(a) = sun_azimuth {
        proj.environment.sun_azimuth = a.rem_euclid(360.0);
    }
    if let Some(e) = sun_elevation {
        if !(1.0..=89.0).contains(&e) {
            return Err(format!(
                "--sun-elevation {e} is not between 1 and 89 degrees. Below the horizon lights the map from underneath."
            ));
        }
        proj.environment.sun_elevation = e;
    }
    if let Some(reason) = size_rejection(proj.units_x, proj.units_y) {
        return Err(reason);
    }
    let out = out.unwrap_or_else(|| {
        springen_core::project::default_output_dir().join(format!("{}.sd7", proj.archive_stem()))
    });

    let opts = BakeOptions {
        rasters: rasters.clone(),
        eval_res,
        height_mode: match height_mode {
            HeightArg::Fit => HeightMode::Fit,
            HeightArg::Absolute => HeightMode::Absolute,
        },
        game: match game {
            GameArg::Zk => Game::ZeroK,
            GameArg::Spring => Game::Spring,
        },
        spot_count: spots,
        min_separation: separation,
        metal_amount: metal,
        max_spot_slope_deg: max_spot_slope,
        blob_radius,
        geo_count: geos,
        players,
        emit_startboxes: !no_startboxes,
        external_info_tex: external_tex,
        map_id,
    };

    let ext = out
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let work = out
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!(
            ".springen-work-{}",
            proj.short_name().to_lowercase()
        ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let d = derive(proj.units_x, proj.units_y);
    println!(
        "Baking {} at {}x{} units — {} × {} elmos, heightmap {} × {}.",
        proj.name, d.units_x, d.units_y, d.elmos_x, d.elmos_y, d.height_w, d.height_h
    );
    let started = std::time::Instant::now();
    let (blueprint, report) = bake_with_progress(&proj, &graph, &opts, &work, &|name, secs| {
        println!("  {name:<30} {secs:>6.2}s");
    })
    .map_err(|e| e.to_string())?;
    println!(
        "  {:<30} {:>6.2}s",
        "total",
        started.elapsed().as_secs_f64()
    );

    match ext.as_str() {
        "sd7" => blueprint.write_sd7(&out).map_err(|e| e.to_string())?,
        "sdz" => blueprint.write_sdz(&out).map_err(|e| e.to_string())?,
        "sdd" => blueprint.write_sdd(&out).map_err(|e| e.to_string())?,
        other => {
            return Err(format!(
                "Output extension .{other} is not a map archive. Use .sd7, .sdz or a .sdd folder."
            ))
        }
    }
    let uncompressed = blueprint.uncompressed_len().unwrap_or(0);
    let _ = std::fs::remove_dir_all(&work);

    print_report(&proj, &report, &out, uncompressed, ext == "sdd");
    Ok(())
}

fn print_report(project: &Project, r: &BakeReport, out: &Path, uncompressed: u64, is_folder: bool) {
    let d = &r.derived;
    println!();
    println!("Spring layer manifest");
    println!("  Heightmap      {} × {} (16-bit)", d.height_w, d.height_h);
    println!("  Diffuse        {} × {}", d.tex_w, d.tex_h);
    println!(
        "  Tiles          {} slots, {} stored ({:.1}% saved), {}",
        r.tile_slots,
        r.tiles_stored,
        100.0 * r.dedup_ratio,
        bytes(r.smt_bytes)
    );
    println!("  Metal / type   {} × {}", d.metal_w, d.metal_h);
    println!("  Grass          {} × {}", d.grass_w, d.grass_h);
    println!("  SMF            {}", bytes(r.smf_bytes));
    let (dmin, dmax) = r.declared_range;
    println!(
        "  Height range   {dmin} .. {dmax} elmos (authored {} .. {})",
        project.min_height, project.max_height
    );
    println!(
        "  Waterline      height 0 elmos, {:.1}% up the baked range, {:.1}% of the map submerged",
        100.0 * water_level_t(dmin, dmax),
        100.0 * r.water_fraction
    );

    println!();
    println!("Metal spots");
    println!(
        "  {} spots, {:.1} metal per player at {} players",
        r.spots.len(),
        r.metal_per_player,
        r.players
    );
    println!(
        "  Symmetry {} under {}",
        if r.symmetric { "clean" } else { "broken" },
        project.mex_sym
    );
    println!("  Raster blobs   {}", r.metal_blobs);
    if r.teams > 0 {
        println!("  Start boxes    {} under {}", r.teams, project.mex_sym);
    }
    if !r.geos.is_empty() {
        println!("  Geo vents      {}", r.geos.len());
    }
    for s in &r.unbuildable_spots {
        println!("  Not buildable: {s}");
    }
    for i in &r.issues {
        println!("  {i}");
    }

    println!();
    println!("Traversable fraction, by unit class");
    for (name, passable, largest, regions) in &r.pathability {
        println!(
            "  {name:<6} {:>5.1}% passable, largest region {:>5.1}%, {regions} regions",
            100.0 * passable,
            100.0 * largest
        );
    }

    let f = &r.flatness;
    println!();
    println!(
        "Buildable ground, {:.0}-elmo footprint under {:.0}°",
        f.footprint, f.max_slope_deg
    );
    println!(
        "  {:>5.1}% of the land, {:>5.1}% of the map, largest plain {:>5.1}%, {} plains",
        100.0 * f.buildable_of_land,
        100.0 * f.buildable_fraction,
        100.0 * f.largest_plain_fraction,
        f.plain_count
    );
    println!(
        "  Land slope     {:.1}° median, {:.1}° at the 90th, relief {:.0} elmos",
        f.median_slope_deg, f.p90_slope_deg, f.relief_elmos
    );

    let c = &r.choke;
    println!();
    println!("Corridor width for tanks, across the largest traversable region");
    if c.bottleneck > 0.0 {
        println!(
            "  {:>5.0} elmos at the narrowest of the widest {} route, at {:.0},{:.0}",
            c.bottleneck, c.axis, c.bottleneck_at.0, c.bottleneck_at.1
        );
        for (label, v) in [("west-east", c.west_east), ("north-south", c.north_south)] {
            match v {
                0.0 => println!("  {:>5}  {label}, no route", "—"),
                v => println!("  {v:>5.0} elmos {label}"),
            }
        }
        println!(
            "  {:>5.0} elmos median, {:.0} at the 10th, {:.0} at the 90th",
            c.median, c.p10, c.p90
        );
        // Judged along the axis the teams are laid out on, not the more
        // constrained one. A map built as a west-east corridor with ranges up
        // the flanks is supposed to pinch north-south, and calling that a
        // funnel would be calling a deliberate design a fault.
        let (play, axis) = c.along_play_axis(&r.symmetry);
        if c.median > 0.0 && play > 0.0 && play < c.median * 0.35 {
            println!(
                "  The {axis} route pinches to {:.0}% of the median — this map funnels.",
                100.0 * play / c.median
            );
        }
    } else {
        println!("  No route across, either way: the largest traversable region does not reach from one side of the map to the other.");
    }

    println!();
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if is_folder {
        println!(
            "Wrote {} — {} uncompressed. The engine loads a .sdd folder directly.",
            out.display(),
            bytes(uncompressed)
        );
    } else {
        println!(
            "Wrote {} — {} from {} uncompressed.",
            out.display(),
            bytes(size),
            bytes(uncompressed)
        );
    }
}

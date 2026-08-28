// Pixel buffers are written by linear index; iterator adapters obscure the
// stride the texture upload expects.
#![allow(clippy::needless_range_loop)]
//! The workstation shell.
//!
//! A fixed frame: 44px toolbar, 212px palette, fluid canvas, 344px inspector,
//! 24px status bar. Nothing in the chrome is centred; everything is flush to
//! its rail.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use springen_archive::{bake_with_progress, BakeOptions, BakeReport, Game};
use springen_core::field::as_gray;
use springen_core::graph::{registry, Graph, PType, PVal};
use springen_core::project::{water_level_t, Context, Project};
use springen_core::spring::{derive, nearest_valid_units, size_rejection, valid_units};
use springen_core::starter::{starter_graph, STARTERS};
use springen_core::zk;

use crate::graph_view::{Action, GraphView};
use crate::panels::{Dock, Layout, Pane};
use crate::theme::{self, FontRole};
use crate::view3d::{self, Camera, ViewMode};

const THUMB_RES: usize = 48;
/// Field resolution behind the 3D viewport. High enough that the terrain reads
/// as terrain, low enough that erosion re-evaluates while you drag a slider.
const PREVIEW_RES: usize = 257;
/// Mesh grid. 256 x 256 quads is 131k triangles, which a software rasteriser
/// still manages and any GPU ignores.
const MESH_GRID: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Splash,
    Projects,
    Workspace,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanvasMode {
    Graph,
    /// The 3D terrain viewport.
    Terrain,
}

/// One stage of the preload. Each does real work; the splash is not a timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootStep {
    Fonts,
    NodeTypes,
    Evaluator,
    Previews,
    Viewport,
    Done,
}

impl BootStep {
    const ORDER: [BootStep; 6] = [
        BootStep::Fonts,
        BootStep::NodeTypes,
        BootStep::Evaluator,
        BootStep::Previews,
        BootStep::Viewport,
        BootStep::Done,
    ];
    fn label(self) -> &'static str {
        match self {
            BootStep::Fonts => "Loading fonts",
            BootStep::NodeTypes => "Registering node types",
            BootStep::Evaluator => "Priming the evaluator",
            BootStep::Previews => "Building starter previews",
            BootStep::Viewport => "Preparing the viewport",
            BootStep::Done => "Ready",
        }
    }
    fn index(self) -> usize {
        BootStep::ORDER.iter().position(|s| *s == self).unwrap_or(0)
    }
}

struct Toast {
    text: String,
    level: Level,
    born: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Info,
    Warn,
    Error,
    Ok,
}

impl Level {
    fn colour(self) -> Color32 {
        match self {
            Level::Info => theme::TEXT_SECONDARY,
            Level::Warn => theme::WARN_300,
            Level::Error => theme::ALERT_300,
            Level::Ok => theme::GOOD_300,
        }
    }
}

struct BakeJob {
    rx: mpsc::Receiver<Result<(BakeReport, PathBuf, u64), String>>,
    /// Stage names as the bake finishes them, so the veil says what it is
    /// doing rather than only how long it has been doing it.
    stages: mpsc::Receiver<String>,
    stage: String,
    started: f64,
}

/// The marks on a pane header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    ChevronDown,
    ChevronRight,
    Close,
    Float,
    DockLeft,
    DockRight,
    Up,
    Down,
}

/// A header button, drawn rather than typed.
///
/// The bundled face carries the text this app sets and nothing else, so
/// arrows and box glyphs come out as tofu. Three or four line segments cost
/// less than shipping a symbol font for eight marks.
fn icon_button(ui: &mut egui::Ui, icon: Icon) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    let ink = if resp.hovered() {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_TERTIARY
    };
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, theme::R_CONTROL, theme::SURFACE_HOVER);
    }
    let p = ui.painter();
    let c = rect.center();
    let s = Stroke::new(1.2, ink);
    match icon {
        Icon::ChevronDown => {
            p.line_segment([c + Vec2::new(-3.0, -1.5), c + Vec2::new(0.0, 2.0)], s);
            p.line_segment([c + Vec2::new(0.0, 2.0), c + Vec2::new(3.0, -1.5)], s);
        }
        Icon::ChevronRight => {
            p.line_segment([c + Vec2::new(-1.5, -3.0), c + Vec2::new(2.0, 0.0)], s);
            p.line_segment([c + Vec2::new(2.0, 0.0), c + Vec2::new(-1.5, 3.0)], s);
        }
        Icon::Close => {
            p.line_segment([c + Vec2::new(-3.0, -3.0), c + Vec2::new(3.0, 3.0)], s);
            p.line_segment([c + Vec2::new(-3.0, 3.0), c + Vec2::new(3.0, -3.0)], s);
        }
        Icon::Float => {
            let r = egui::Rect::from_center_size(c, Vec2::splat(7.0));
            p.rect_stroke(r, 0.0, s, egui::StrokeKind::Inside);
            p.line_segment([r.left_top(), r.right_top()], Stroke::new(2.0, ink));
        }
        Icon::DockLeft | Icon::DockRight => {
            let r = egui::Rect::from_center_size(c, Vec2::splat(8.0));
            p.rect_stroke(r, 0.0, s, egui::StrokeKind::Inside);
            let bar = if icon == Icon::DockLeft {
                egui::Rect::from_min_size(r.left_top(), Vec2::new(3.0, r.height()))
            } else {
                egui::Rect::from_min_size(
                    r.left_top() + Vec2::new(5.0, 0.0),
                    Vec2::new(3.0, r.height()),
                )
            };
            p.rect_filled(bar, 0.0, ink);
        }
        Icon::Up | Icon::Down => {
            let d = if icon == Icon::Up { -1.0 } else { 1.0 };
            p.line_segment(
                [c + Vec2::new(0.0, -3.0 * d), c + Vec2::new(0.0, 3.0 * d)],
                s,
            );
            p.line_segment(
                [c + Vec2::new(-2.5, -0.5 * d), c + Vec2::new(0.0, -3.0 * d)],
                s,
            );
            p.line_segment(
                [c + Vec2::new(2.5, -0.5 * d), c + Vec2::new(0.0, -3.0 * d)],
                s,
            );
        }
    }
    resp
}

/// One corner of an area's bounds. Bit 0 picks the west/east side, bit 1 the
/// north/south one, so the opposite corner is simply `k ^ 3`.
fn corner_of(a: &springen_core::lua::StartArea, k: u8) -> (f64, f64) {
    let (x0, z0, x1, z1) = a.bounds();
    (
        if k & 1 == 0 { x0 } else { x1 },
        if k & 2 == 0 { z0 } else { z1 },
    )
}

/// After one box is reshaped, give its symmetry images the same size in the
/// place the operator puts them.
///
/// Size and shape are copied, position is not: an image belongs where the
/// operator says, and a hand edit that changes one team's area without the
/// others is how a map goes quietly unfair.
fn reflow_images(
    areas: &mut [springen_core::lua::StartArea],
    edited: usize,
    sym: &str,
    w: f64,
    h: f64,
) {
    let src = areas[edited].clone();
    let (sx0, sz0, sx1, sz1) = src.bounds();
    let (sw, sh) = (sx1 - sx0, sz1 - sz0);
    let from = src.centre();
    let images = springen_core::zk::symmetry_images(from.0, from.1, sym, w, h);
    let centres: Vec<(f64, f64)> = areas.iter().map(|a| a.centre()).collect();
    let shape = src.shape();
    // One box per image, each claimed once — see `zk::assign_images`.
    let pick = springen_core::zk::assign_images(&centres, edited, &images);
    for ((ix, iz), which) in images.iter().zip(pick) {
        let Some(j) = which else { continue };
        areas[j].set_bounds(
            ix - sw * 0.5,
            iz - sh * 0.5,
            ix + sw * 0.5,
            iz + sh * 0.5,
            w,
            h,
        );
        if shape != springen_core::lua::Shape::Rect {
            areas[j].set_shape(shape, w, h);
        }
    }
}

/// The sculpt brush's settings.
///
/// Radius and strength are elmos, like every other authored length in the
/// project — a brush in pixels would paint a different shape at preview
/// resolution than at bake resolution.
#[derive(Clone, Copy, Debug)]
pub struct Brush {
    /// Armed. While it is, a drag in the terrain view paints instead of
    /// orbiting.
    pub active: bool,
    pub radius: f64,
    /// Elmos of height for Raise; a rate for Smooth and Level.
    pub strength: f64,
    pub mode: springen_core::graph::StrokeMode,
    /// Dig rather than pile. A separate flag rather than asking for a negative
    /// number, because the sign of a brush is a mode you toggle, not a value
    /// you type.
    pub lower: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            active: false,
            radius: 400.0,
            strength: 60.0,
            mode: springen_core::graph::StrokeMode::Raise,
            lower: false,
        }
    }
}

pub struct SpringenApp {
    screen: Screen,
    project: Project,
    graph: Graph,
    view: GraphView,
    mode: CanvasMode,
    thumbs: HashMap<String, egui::TextureHandle>,
    thumb_sig: HashMap<String, String>,
    starter_thumbs: HashMap<String, egui::TextureHandle>,
    renderer: Option<view3d::Shared>,
    camera: Camera,
    view_mode: ViewMode,
    /// Signature of what the viewport textures were built from.
    terrain_sig: String,
    /// The field the viewport is showing, for projecting overlays onto it.
    terrain_field: Option<springen_core::SharedField>,
    /// Climb limit the slope and pathability views are drawn against.
    climb_limit: f64,
    /// Rasters this project carries — an imported map's terrain, and later
    /// brush layers. Loaded when the project is opened, never inside node
    /// evaluation.
    rasters: std::sync::Arc<springen_core::raster::Rasters>,
    /// Whether the graph canvas carries a live map beside it.
    ///
    /// Node thumbnails show what one node produces; they cannot show what the
    /// *map* looks like, which is the thing you are actually editing. Wiring a
    /// node and then switching to the 3D view to find out what it did is a
    /// round trip per edit.
    graph_map: bool,
    graph_map_tex: Option<egui::TextureHandle>,
    graph_map_sig: String,
    /// Corridor width across the terrain in the viewport, keyed like the
    /// buildable sweep so it re-measures when the terrain does.
    choke: Option<springen_core::analysis::Choke>,
    choke_sig: String,
    /// Buildable-ground measurement of the terrain in the viewport, and the
    /// terrain signature it was measured from. Flattening is a thing you do by
    /// eye until you have a number for it, and the bake report is too late a
    /// place to learn that the map only builds on a fifth of its land.
    flatness: Option<springen_core::analysis::Flatness>,
    flatness_sig: String,
    /// The building the buildable sweep is asking about: a footprint in elmos
    /// and the slope a builder will tolerate under it.
    build_footprint: f64,
    build_slope: f64,
    boot: BootStep,
    /// `--screen` pins a screen, so the preload does not run past it.
    hold_screen: bool,
    toasts: Vec<Toast>,
    spots: Vec<zk::MetalSpot>,
    /// What the ground under each spot is, kept beside the spots so the
    /// inspector and the viewport agree with what the bake will say.
    spot_build: Vec<zk::BuildabilityReport>,
    spot_count: usize,
    min_separation: f64,
    /// Steepest ground a mex footprint may sit on.
    max_spot_slope: f64,
    /// The spot the inspector is editing, and the one being dragged.
    selected_spot: Option<usize>,
    drag_spot: Option<usize>,
    /// Which waypoint of the selected node is being dragged.
    route_drag: Option<usize>,
    /// The sculpt brush, and where it last laid a stroke down so a drag lays
    /// a track of them rather than one per frame.
    brush: Brush,
    brush_last: Option<(f64, f64)>,
    /// Which start box is being dragged, and which is selected.
    /// What happened to the last spot placed, when it is worth saying.
    spot_note: Option<String>,
    box_drag: Option<usize>,
    /// Which corner of the dragged box, if a handle was grabbed.
    box_corner: Option<u8>,
    selected_box: Option<usize>,
    show_boxes: bool,
    /// Whether a hand edit carries the spot's symmetry images with it.
    /// On by default: editing a mex without its images is how a map ends up
    /// quietly unfair.
    mirror_edits: bool,
    /// Detail material tiles for the Diffuse view, rendered at a lower
    /// resolution than the bake uses because four 512² tiles is over a second
    /// and this has to keep up with a combo box.
    materials: Option<springen_core::material::SplatMaterials>,
    materials_sig: String,
    /// Thumbnails for the material picker, keyed by material name.
    mat_thumbs: HashMap<String, egui::TextureHandle>,
    /// The ground-underfoot strip, and the material signature it was built
    /// for. Keyed rather than cleared so changing a channel rebuilds it.
    ground_strip: Option<(String, egui::TextureHandle)>,
    game: Game,
    out_dir: PathBuf,
    /// The project file on disk, once there is one.
    project_path: Option<PathBuf>,
    /// Where every inspector pane sits.
    layout: Layout,
    /// What the node palette is filtered to.
    palette_filter: String,
    /// Recently opened project files, most recent first.
    recent: Vec<PathBuf>,
    /// The document as it was last written or read, for the unsaved check.
    saved_doc: String,
    /// Waiting on an answer about discarding unsaved work.
    confirm_home: bool,
    /// Undo history, as whole documents. A graph is small and a document is
    /// the only snapshot that also captures size, seed and name.
    undo: Vec<String>,
    redo: Vec<String>,
    /// The document as it was before the edit currently under the pointer.
    /// Held until the interaction settles so a slider drag is one undo step.
    pending_undo: Option<String>,
    last_doc: String,
    job: Option<BakeJob>,
    last_report: Option<BakeReport>,
    fit_pending: bool,
    /// Set by `--smoke`: bake once, report, and quit.
    smoke: Option<PathBuf>,
    /// Frames rendered, used by the headless screenshot path.
    pub frames: u64,
}

impl SpringenApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<&str>) -> Self {
        theme::install(&cc.egui_ctx);
        let kind = start.unwrap_or("textured");
        let mut app = SpringenApp {
            screen: if start.is_some() {
                Screen::Workspace
            } else {
                Screen::Splash
            },
            project: Project {
                name: "Untitled Map".into(),
                author: String::new(),
                units_x: 12,
                units_y: 12,
                ..springen_core::starter::starter_project(kind)
            },
            graph: starter_graph(kind),
            view: GraphView::default(),
            mode: CanvasMode::Graph,
            thumbs: HashMap::new(),
            thumb_sig: HashMap::new(),
            starter_thumbs: HashMap::new(),
            renderer: None,
            camera: Camera::default(),
            view_mode: ViewMode::Relief,
            terrain_sig: String::new(),
            terrain_field: None,
            climb_limit: 18.0,
            rasters: Default::default(),
            graph_map: true,
            graph_map_tex: None,
            graph_map_sig: String::new(),
            flatness: None,
            flatness_sig: String::new(),
            build_footprint: 96.0,
            build_slope: 12.0,
            boot: BootStep::Fonts,
            hold_screen: false,
            toasts: Vec::new(),
            spots: Vec::new(),
            spot_build: Vec::new(),
            spot_count: 14,
            min_separation: 700.0,
            max_spot_slope: 12.0,
            selected_spot: None,
            drag_spot: None,
            route_drag: None,
            brush: Brush::default(),
            brush_last: None,
            spot_note: None,
            box_drag: None,
            box_corner: None,
            selected_box: None,
            show_boxes: true,
            mirror_edits: true,
            materials: None,
            materials_sig: String::new(),
            mat_thumbs: HashMap::new(),
            ground_strip: None,
            choke: None,
            choke_sig: String::new(),
            game: Game::ZeroK,
            out_dir: springen_core::project::default_output_dir(),
            project_path: None,
            layout: Layout::stock(),
            palette_filter: String::new(),
            recent: Vec::new(),
            saved_doc: String::new(),
            confirm_home: false,
            undo: Vec::new(),
            redo: Vec::new(),
            pending_undo: None,
            last_doc: String::new(),
            job: None,
            last_report: None,
            fit_pending: true,
            smoke: None,
            frames: 0,
        };
        app.camera.frame_map([
            (app.project.units_x * 512) as f32,
            (app.project.units_y * 512) as f32,
        ]);
        if let Some(gl) = &cc.gl {
            match view3d::Renderer::new(gl, MESH_GRID) {
                Ok(r) => app.renderer = Some(Arc::new(Mutex::new(r))),
                // Without GL the graph still works; only the viewport is lost.
                Err(e) => eprintln!("3D viewport unavailable: {e}"),
            }
        }
        app.repropose_spots();
        app.load_settings();
        app.last_doc = app.document();
        app.saved_doc = app.document();
        app
    }

    /// Pick the terrain view mode by name.
    pub fn set_view(&mut self, name: &str) {
        if let Some(m) = ViewMode::ALL
            .iter()
            .find(|m| m.label().eq_ignore_ascii_case(name))
        {
            self.view_mode = *m;
            self.mode = CanvasMode::Terrain;
        }
    }

    /// Bake once without a click, so the interactive path can be smoke-tested
    /// on a machine with no display.
    pub fn smoke_test(&mut self, out: PathBuf) {
        self.smoke = Some(out);
        self.screen = Screen::Workspace;
    }

    /// Skip straight to a screen, for screenshots.
    pub fn goto(&mut self, screen: &str) {
        self.hold_screen = true;
        self.screen = match screen {
            "splash" => Screen::Splash,
            "projects" => Screen::Projects,
            "floating" => {
                // A screenshot target: two panes off the rail, so the floating
                // case is checked on a machine with no one to drag them.
                self.layout.move_to(Pane::Metal, Dock::Float);
                self.layout.move_to(Pane::Materials, Dock::Float);
                self.layout.move_to(Pane::Measure, Dock::Left);
                Screen::Workspace
            }
            _ => Screen::Workspace,
        };
        if screen == "terrain" {
            self.mode = CanvasMode::Terrain;
        }
    }

    fn toast(&mut self, ctx: &egui::Context, level: Level, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            level,
            born: ctx.input(|i| i.time),
        });
    }

    /* ------------------------------------------------------- project file */

    /// The whole project as JSON: settings and graph, the same shape the CLI
    /// reads and the same shape the bake ships inside the archive.
    fn document(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "project": self.project,
            "graph": self.graph.serialize(),
        }))
        .unwrap_or_default()
    }

    /// Replace the whole session from a document. Returns the reason on
    /// failure rather than leaving the app half-loaded.
    fn load_document(&mut self, text: &str) -> Result<(), String> {
        let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let pj = doc.get("project").unwrap_or(&doc);
        let gj = doc.get("graph").ok_or("Not a Springen project")?;
        self.project = Project::from_json(pj);
        self.graph = Graph::deserialize(gj);
        self.view = GraphView::default();
        self.thumbs.clear();
        self.thumb_sig.clear();
        self.terrain_sig.clear();
        self.fit_pending = true;
        self.camera.frame_map([
            (self.project.units_x * 512) as f32,
            (self.project.units_y * 512) as f32,
        ]);
        self.repropose_spots();
        self.last_doc = self.document();
        self.pending_undo = None;
        Ok(())
    }

    fn open_project(&mut self, ctx: &egui::Context) {
        let start = self
            .project_path
            .as_ref()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| self.out_dir.clone());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Springen project", &["json"])
            .set_directory(start)
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()) {
            Ok(text) => match self.load_document(&text) {
                Ok(()) => {
                    // A different project is a different history.
                    self.undo.clear();
                    self.redo.clear();
                    // Rasters live in `rasters/` beside the project file, and
                    // an imported map is nothing without them.
                    match springen_archive::import::read_project_rasters(&path) {
                        Ok(r) => self.rasters = std::sync::Arc::new(r),
                        Err(e) => self.toast(
                            ctx,
                            Level::Error,
                            format!("rasters beside {}: {e}", path.display()),
                        ),
                    }
                    self.terrain_sig.clear();
                    self.project_path = Some(path.clone());
                    self.saved_doc = self.document();
                    self.remember(&path);
                    self.toast(ctx, Level::Ok, format!("Opened {}", path.display()));
                }
                Err(e) => self.toast(ctx, Level::Error, format!("{}: {e}", path.display())),
            },
            Err(e) => self.toast(ctx, Level::Error, format!("{}: {e}", path.display())),
        }
    }

    /// `ask` forces the file dialog; otherwise a project that already has a
    /// path saves straight over itself.
    fn save_project(&mut self, ctx: &egui::Context, ask: bool) {
        let path = match (&self.project_path, ask) {
            (Some(p), false) => Some(p.clone()),
            _ => {
                let name = format!("{}.json", self.project.short_name());
                rfd::FileDialog::new()
                    .add_filter("Springen project", &["json"])
                    .set_directory(&self.out_dir)
                    .set_file_name(name)
                    .save_file()
            }
        };
        let Some(path) = path else { return };
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                self.toast(ctx, Level::Error, format!("{}: {e}", dir.display()));
                return;
            }
        }
        match std::fs::write(&path, self.document()) {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.saved_doc = self.document();
                self.remember(&path);
                self.toast(ctx, Level::Ok, format!("Saved {}", path.display()));
            }
            Err(e) => self.toast(ctx, Level::Error, format!("{}: {e}", path.display())),
        }
    }

    /// The one modal in the app: leaving unsaved work.
    ///
    /// Three ways out and all of them stated, rather than a yes/no that hides
    /// what "no" does.
    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_home {
            return;
        }
        let mut close = false;
        egui::Modal::new(egui::Id::new("confirm-home")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(
                RichText::new("Unsaved changes")
                    .font(theme::font(FontRole::Ui, 13.0))
                    .color(theme::TEXT_PRIMARY),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ghost_button(ui, "Save and leave").clicked() {
                    self.save_project(ctx, false);
                    if !self.unsaved() {
                        self.screen = Screen::Projects;
                        close = true;
                    }
                }
                if ghost_button(ui, "Discard").clicked() {
                    self.screen = Screen::Projects;
                    close = true;
                }
                if ghost_button(ui, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.confirm_home = false;
        }
    }

    /// Bring in an existing map: a `.sd7`, a `.sdd` folder or a bare `.smf`.
    ///
    /// The importer was command-line only, which meant the one workflow that
    /// starts from somebody else's map could not be reached from the tool that
    /// edits maps.
    fn import_map(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Spring map", &["sd7", "sdd", "smf"])
            .set_directory(&self.out_dir)
            .pick_file()
        else {
            return;
        };
        // `read_map` already sorts out an archive from a folder from a bare
        // `.smf`, so whatever was picked goes straight to it.
        let target = path;
        let map = match springen_archive::import::read_map(&target) {
            Ok(m) => m,
            Err(e) => {
                self.toast(ctx, Level::Error, format!("{}: {e}", target.display()));
                return;
            }
        };
        let name = target
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Imported".into());
        let imported = springen_archive::import::to_project(&map, &name);
        // Anything the importer could not bring across is worth one line each;
        // they are findings about this map, not instructions.
        for note in &imported.notes {
            self.toast(ctx, Level::Info, note.clone());
        }
        self.project = imported.project;
        self.graph = imported.graph;
        self.rasters = std::sync::Arc::new(imported.rasters);
        self.project_path = None;
        self.saved_doc = String::new();
        self.undo.clear();
        self.redo.clear();
        self.selected_spot = None;
        self.selected_box = None;
        self.materials = None;
        self.mat_thumbs.clear();
        self.thumb_sig.clear();
        self.terrain_sig.clear();
        self.repropose_spots();
        self.fit_pending = true;
        self.screen = Screen::Workspace;
    }

    /* --------------------------------------------------- session settings */

    /// Where the workstation keeps what it remembers between runs.
    ///
    /// Beside the maps rather than in a platform config directory: the output
    /// folder is already the place this tool owns, and a user who moves their
    /// Springen folder to another machine should find their layout there too.
    fn settings_path() -> std::path::PathBuf {
        springen_core::project::default_output_dir().join("workstation.json")
    }

    fn load_settings(&mut self) {
        let Ok(text) = std::fs::read_to_string(Self::settings_path()) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        if let Some(l) = v.get("layout") {
            self.layout = Layout::from_json(l);
        }
        if let Some(dir) = v.get("outDir").and_then(|x| x.as_str()) {
            self.out_dir = std::path::PathBuf::from(dir);
        }
        if let Some(list) = v.get("recent").and_then(|x| x.as_array()) {
            self.recent = list
                .iter()
                .filter_map(|x| x.as_str().map(std::path::PathBuf::from))
                .collect();
        }
    }

    /// Best effort: a settings file that cannot be written is not worth
    /// interrupting anyone over.
    fn save_settings(&self) {
        let v = serde_json::json!({
            "layout": self.layout.to_json(),
            "outDir": self.out_dir.to_string_lossy(),
            "recent": self.recent.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        });
        let path = Self::settings_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    /// Remember a project file, most recent first, without duplicates.
    fn remember(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(8);
        self.save_settings();
    }

    /// Whether there is work that would be lost.
    fn unsaved(&self) -> bool {
        self.document() != self.saved_doc
    }

    /// Back to the project browser.
    ///
    /// Asks first when there is unsaved work, because the browser replaces the
    /// session and there is no undo across that.
    fn go_home(&mut self, ctx: &egui::Context) {
        if self.unsaved() {
            self.confirm_home = true;
            return;
        }
        self.screen = Screen::Projects;
        let _ = ctx;
    }

    /// Load a project from a known path, for the recent list.
    fn open_path(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.toast(ctx, Level::Error, format!("{}: {e}", path.display()));
                self.recent.retain(|p| *p != path);
                self.save_settings();
                return;
            }
        };
        if let Err(e) = self.load_document(&text) {
            self.toast(ctx, Level::Error, format!("{}: {e}", path.display()));
            return;
        }
        self.undo.clear();
        self.redo.clear();
        if let Ok(r) = springen_archive::import::read_project_rasters(&path) {
            self.rasters = std::sync::Arc::new(r);
        }
        self.terrain_sig.clear();
        self.thumb_sig.clear();
        self.materials = None;
        self.repropose_spots();
        self.fit_pending = true;
        self.project_path = Some(path.clone());
        self.saved_doc = self.document();
        self.remember(&path);
        self.screen = Screen::Workspace;
    }

    /* -------------------------------------------------------------- undo */

    /// Notice an edit, and commit it as one undo step once the interaction
    /// that made it has settled.
    ///
    /// Watching the document rather than every call site means a change made
    /// from the palette, the canvas, the inspector or the toolbar is caught
    /// the same way. Waiting for the pointer to come up is what keeps a slider
    /// drag from becoming forty undo steps.
    fn track_edits(&mut self, ctx: &egui::Context) {
        const DEPTH: usize = 64;
        let now = self.document();
        if now != self.last_doc {
            if self.pending_undo.is_none() {
                self.pending_undo = Some(std::mem::replace(&mut self.last_doc, now));
            } else {
                self.last_doc = now;
            }
        }
        let settled = ctx.input(|i| !i.pointer.any_down() && i.keys_down.is_empty());
        if settled {
            if let Some(before) = self.pending_undo.take() {
                self.undo.push(before);
                if self.undo.len() > DEPTH {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
        }
    }

    fn undo(&mut self, ctx: &egui::Context) {
        let Some(before) = self.undo.pop() else {
            self.toast(ctx, Level::Info, "Nothing left to undo.");
            return;
        };
        let current = self.document();
        if self.load_document(&before).is_ok() {
            self.redo.push(current);
        }
    }

    fn redo(&mut self, ctx: &egui::Context) {
        let Some(next) = self.redo.pop() else {
            self.toast(ctx, Level::Info, "Nothing left to redo.");
            return;
        };
        let current = self.document();
        if self.load_document(&next).is_ok() {
            self.undo.push(current);
        }
    }

    /// The keyboard half of the file and edit commands.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let hit = |k: Key, mods: Modifiers| ctx.input_mut(|i| i.consume_key(mods, k));
        if hit(Key::O, Modifiers::COMMAND) {
            self.open_project(ctx);
        }
        if hit(Key::S, Modifiers::COMMAND | Modifiers::SHIFT) {
            self.save_project(ctx, true);
        } else if hit(Key::S, Modifiers::COMMAND) {
            self.save_project(ctx, false);
        }
        if hit(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT) || hit(Key::Y, Modifiers::COMMAND) {
            self.redo(ctx);
        } else if hit(Key::Z, Modifiers::COMMAND) {
            self.undo(ctx);
        }
        if hit(Key::B, Modifiers::COMMAND) {
            self.start_bake(ctx);
        }
        // Duplicate the selected node, offset so it does not hide under the
        // original, with its parameters but not its wires — a copy that
        // inherited inputs would quietly double whatever they feed.
        if hit(Key::D, Modifiers::COMMAND) {
            if let Some(id) = self.view.selected.clone() {
                if let Some(src) = self.graph.node(&id).cloned() {
                    let new_id = self
                        .graph
                        .add(&src.type_name, src.x + 32.0, src.y + 32.0, &[]);
                    if let Some(dst) = self.graph.node_mut(&new_id) {
                        dst.params = src.params.clone();
                    }
                    self.view.selected = Some(new_id);
                    self.terrain_sig.clear();
                    self.thumb_sig.clear();
                }
            }
        }
        if hit(Key::F, Modifiers::NONE) {
            self.fit_pending = true;
        }
        if hit(Key::Tab, Modifiers::NONE) {
            self.mode = match self.mode {
                CanvasMode::Graph => CanvasMode::Terrain,
                CanvasMode::Terrain => CanvasMode::Graph,
            };
        }
        // 1-6 pick a view, which is the switch you reach for most while
        // looking at terrain.
        for (i, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
        ]
        .iter()
        .enumerate()
        {
            if hit(*key, Modifiers::NONE) {
                if let Some(m) = ViewMode::ALL.get(i) {
                    self.view_mode = *m;
                }
            }
        }
    }

    /// Re-place the metal spots against the current terrain, and keep the
    /// buildability report the inspector and the viewport both read.
    fn repropose_spots(&mut self) {
        let ctx = Context::new(&self.project, 129);
        // A hand-placed layout is the author's, and re-proposing over it on
        // every graph edit would throw the work away.
        if !self.project.spots.is_empty() {
            self.spots = self.project.spots.clone();
            self.refresh_spot_ground(&ctx);
            return;
        }
        let mask = match self.graph.find_terminal("metal") {
            Some(id) => as_gray(&self.graph.evaluate(id, &ctx)),
            None => {
                self.spots.clear();
                self.spot_build.clear();
                return;
            }
        };
        let terrain = self
            .graph
            .find_terminal("height")
            .map(|id| as_gray(&self.graph.evaluate(id, &ctx)));
        let build = zk::BuildabilityOptions {
            sea_level: water_level_t(self.project.min_height, self.project.max_height),
            max_slope_deg: self.max_spot_slope,
            ..Default::default()
        };
        self.spots = zk::propose_spots_on(
            &mask,
            terrain.as_ref(),
            &ctx,
            &zk::ProposeOptions {
                count: self.spot_count,
                min_separation: self.min_separation,
                symmetry: self.project.mex_sym.clone(),
                build: Some(build.clone()),
                ..Default::default()
            },
        );
        // The same check the CLI runs. The app used to show an unqualified
        // green panel while the bake refused the layout it was describing.
        self.spot_build = match &terrain {
            Some(t) => zk::check_buildability(&self.spots, t, &ctx, &build),
            None => Vec::new(),
        };
    }

    /// Re-run the buildability check without re-placing anything, which is
    /// what a hand edit needs: the answer changes, the layout does not.
    fn refresh_spot_ground(&mut self, ctx: &Context) {
        let build = zk::BuildabilityOptions {
            sea_level: water_level_t(self.project.min_height, self.project.max_height),
            max_slope_deg: self.max_spot_slope,
            ..Default::default()
        };
        self.spot_build = match self
            .graph
            .find_terminal("height")
            .map(|id| as_gray(&self.graph.evaluate(id, ctx)))
        {
            Some(t) => zk::check_buildability(&self.spots, &t, ctx, &build),
            None => Vec::new(),
        };
    }

    /// Take ownership of the layout on the first hand edit.
    ///
    /// Until this happens the spots are the generator's and are replaced
    /// whenever the graph changes; afterwards they are the project's, they
    /// travel in its file, and the generator leaves them alone.
    fn adopt_spots(&mut self) {
        if self.project.spots.is_empty() {
            zk::renumber(&mut self.spots);
            self.project.spots = self.spots.clone();
        }
    }

    /// Write an edited list back and re-check the ground under it.
    fn commit_spots(&mut self) {
        zk::renumber(&mut self.spots);
        self.project.spots = self.spots.clone();
        let ctx = Context::new(&self.project, 129);
        self.refresh_spot_ground(&ctx);
        self.terrain_sig.clear();
    }

    /// Place a metal spot and its symmetry images.
    ///
    /// `at` is where to put it, or `None` for wherever the camera is looking.
    /// Returns a note when the placement was not exactly what was asked for.
    ///
    /// The nudge is the point of this. Every symmetry operator fixes the map
    /// centre, and a freshly framed camera looks straight at it — so "Add"
    /// used to drop a spot that was its own mirror, arriving alone, on every
    /// new map and under every symmetry. It looked exactly like mirroring was
    /// broken. `quad` and `rot90` need clearing off *both* axes, not one, or
    /// the group comes out half the size the operator promises.
    fn add_spot_at(&mut self, at: Option<(f64, f64)>) -> Option<String> {
        self.adopt_spots();
        let c = Context::new(&self.project, 129);
        let want = at.unwrap_or((
            f64::from(self.camera.target[0]),
            f64::from(self.camera.target[2]),
        ));
        let want = (want.0.clamp(0.0, c.elmos_x), want.1.clamp(0.0, c.elmos_y));
        let mut note = None;
        let place = if self.mirror_edits && self.project.mex_sym != "none" {
            match zk::off_fixed_set(want, &self.project.mex_sym, c.elmos_x, c.elmos_y) {
                Some(p) => {
                    if p != want {
                        note = Some(format!(
                            "On the {} axis — moved to {:.0},{:.0} for a group of {}",
                            self.project.mex_sym,
                            p.0,
                            p.1,
                            zk::group_size(p.0, p.1, &self.project.mex_sym, c.elmos_x, c.elmos_y)
                        ));
                    }
                    p
                }
                None => {
                    note = Some("No full group nearby — placed alone".into());
                    want
                }
            }
        } else {
            want
        };
        let i = zk::add_group(
            &mut self.spots,
            place,
            springen_core::Zk::DEFAULT_MEX_INCOME,
            &self.project.mex_sym,
            c.elmos_x,
            c.elmos_y,
            self.mirror_edits,
        );
        self.selected_spot = Some(i);
        self.commit_spots();
        note
    }

    /// Hand back to the generator.
    fn repropose_from_graph(&mut self) {
        self.project.spots.clear();
        self.selected_spot = None;
        self.drag_spot = None;
        self.repropose_spots();
        self.terrain_sig.clear();
    }

    /* ------------------------------------------------------------ previews */

    fn refresh_thumbs(&mut self, ctx: &egui::Context) {
        let rctx = Context::with_rasters(&self.project, THUMB_RES, self.rasters.clone());
        let ids: Vec<String> = self.graph.nodes.iter().map(|n| n.id.clone()).collect();
        for id in ids {
            let sig = self.graph.signature(&id, &rctx);
            if self.thumb_sig.get(&id) == Some(&sig) {
                continue;
            }
            let field = self.graph.evaluate(&id, &rctx);
            let mut px = vec![Color32::BLACK; THUMB_RES * THUMB_RES];
            if field.ch >= 3 {
                for i in 0..THUMB_RES * THUMB_RES {
                    px[i] = Color32::from_rgb(
                        (field.get(i * field.ch).clamp(0.0, 1.0) * 255.0) as u8,
                        (field.get(i * field.ch + 1).clamp(0.0, 1.0) * 255.0) as u8,
                        (field.get(i * field.ch + 2).clamp(0.0, 1.0) * 255.0) as u8,
                    );
                }
            } else {
                // Grayscale fields read as the hypsometric ramp, so a
                // thumbnail always looks like the terrain it describes.
                for i in 0..THUMB_RES * THUMB_RES {
                    let c = springen_core::ramps::hypso(field.get(i));
                    px[i] = Color32::from_rgb(c[0] as u8, c[1] as u8, c[2] as u8);
                }
            }
            let image = egui::ColorImage {
                size: [THUMB_RES, THUMB_RES],
                pixels: px,
                source_size: Vec2::splat(THUMB_RES as f32),
            };
            let tex = ctx.load_texture(format!("thumb-{id}"), image, egui::TextureOptions::LINEAR);
            self.thumbs.insert(id.clone(), tex);
            self.thumb_sig.insert(id, sig);
        }
        self.thumbs.retain(|k, _| self.graph.node(k).is_some());
    }

    /// Starter tiles carry the starter's own baked diffuse — its palette, its
    /// detail materials and its waterline, not a grey relief of its height.
    ///
    /// Evaluated against the starter's *own* project, because each one now
    /// brings its own surfaces and its own sea level: rendering an island
    /// archipelago against a desert's waterline shows neither.
    fn starter_thumb(&mut self, ctx: &egui::Context, kind: &str) -> egui::TextureHandle {
        if let Some(t) = self.starter_thumbs.get(kind) {
            return t.clone();
        }
        const R: usize = 96;
        let g = starter_graph(kind);
        let project = springen_core::starter::starter_project(kind);
        let mut set = project.materials.clone();
        set.tile_res = 128;
        let mats = springen_core::material::render_set(
            &set,
            f64::from(Context::new(&project, 2).seed) + 4241.0,
        );
        let px: Vec<Color32> = match springen_core::preview::render(
            &g,
            &project,
            &springen_core::PreviewOptions {
                res: R,
                mode: ViewMode::Diffuse,
                materials: Some(&mats),
                ..Default::default()
            },
        ) {
            Some(p) => p
                .colour
                .chunks(3)
                .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
                .collect(),
            None => vec![theme::GRAY_900; R * R],
        };
        let tex = ctx.load_texture(
            format!("starter-{kind}"),
            egui::ColorImage {
                size: [R, R],
                pixels: px,
                source_size: Vec2::splat(R as f32),
            },
            egui::TextureOptions::LINEAR,
        );
        self.starter_thumbs.insert(kind.to_string(), tex.clone());
        tex
    }

    /// Rebuild the viewport's height and colour textures when the graph, the
    /// view mode or the climb limit changes. Uploading two textures is the
    /// whole update: the mesh is static and displaced in the vertex shader.
    /// Re-render the detail tiles when the material choice changes.
    ///
    /// Preview resolution, not bake resolution: the tiles repeat every 167
    /// elmos, so at a 257² preview a 256² tile is already far more detail than
    /// a screen pixel can hold.
    fn refresh_materials(&mut self) {
        let sig = format!("{:?}", self.project.materials);
        if self.materials_sig == sig && self.materials.is_some() {
            return;
        }
        let mut set = self.project.materials.clone();
        set.tile_res = 256;
        self.materials = Some(springen_core::material::render_set(
            &set,
            f64::from(Context::new(&self.project, 2).seed) + 4241.0,
        ));
        self.materials_sig = sig;
    }

    fn refresh_terrain(&mut self, gl: &eframe::glow::Context) {
        let Some(renderer) = self.renderer.clone() else {
            return;
        };
        let Some(id) = self.graph.find_terminal("height").map(str::to_string) else {
            return;
        };
        let rctx = Context::new(&self.project, PREVIEW_RES);
        let sig = format!(
            "{}|{:?}|{}|{}|{:?}|{}",
            self.graph.signature(&id, &rctx),
            self.view_mode,
            self.climb_limit,
            self.spots.len(),
            self.project.materials,
            self.rasters.signature()
        );
        if self.terrain_sig == sig {
            return;
        }
        if self.view_mode == ViewMode::Diffuse {
            self.refresh_materials();
        }
        let opts = springen_core::PreviewOptions {
            rasters: self.rasters.clone(),
            res: PREVIEW_RES,
            mode: self.view_mode,
            climb_limit: self.climb_limit,
            spots: &self.spots,
            materials: self.materials.as_ref(),
        };
        let Some(preview) = springen_core::preview::render(&self.graph, &self.project, &opts)
        else {
            return;
        };
        let hdata: Vec<f32> = preview.height.data.clone();
        if let Ok(mut r) = renderer.lock() {
            r.upload(gl, PREVIEW_RES, &hdata, &preview.colour);
        }
        self.terrain_field = Some(preview.height);
        self.terrain_sig = sig;
    }

    /// A live map beside the graph, in whichever view mode is selected.
    ///
    /// The node thumbnails answer "what does this node produce"; this answers
    /// "what does the map look like", and until it existed the only way to ask
    /// that was to switch to the 3D view and switch back. It is the same
    /// painting `springen preview` writes and the viewport shows, at a lower
    /// resolution, so it cannot disagree with either.
    fn graph_map_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("graph_map")
            .exact_size(theme::GRAPH_MAP_W)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_PANEL)
                    .inner_margin(egui::Margin::same(theme::PANEL_PAD as i8))
                    .stroke(Stroke::new(1.0, theme::BORDER_HAIRLINE)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::micro_label(ui, self.view_mode.label());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ghost_button(ui, "Open in 3D").clicked() {
                            self.mode = CanvasMode::Terrain;
                        }
                    });
                });
                self.refresh_graph_map(ui.ctx());
                let side = ui.available_width().min(ui.available_height()).max(32.0);
                // Shown at the world's shape, not the lattice's. The texture is
                // square because the graph is evaluated on a square lattice; a
                // 16x8 map drawn square has its Z axis at twice its width.
                let (ex, ey) = (
                    f64::from(self.project.units_x * 512),
                    f64::from(self.project.units_y * 512),
                );
                let shape = if ex >= ey {
                    egui::vec2(side, side * (ey / ex) as f32)
                } else {
                    egui::vec2(side * (ex / ey) as f32, side)
                };
                if let Some(tex) = &self.graph_map_tex {
                    let r = ui.add(
                        egui::Image::new(tex)
                            .fit_to_exact_size(shape)
                            .corner_radius(theme::R_CONTROL),
                    );
                    // Clicking it is the obvious thing to try.
                    if r.interact(Sense::click()).clicked() {
                        self.mode = CanvasMode::Terrain;
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} × {} elmos",
                        self.project.units_x * 512,
                        self.project.units_y * 512
                    ))
                    .font(theme::font(FontRole::Mono, 11.0))
                    .color(theme::TEXT_TERTIARY),
                );
            });
    }

    /// Re-render the graph canvas's map when anything it depends on moves.
    ///
    /// Keyed on the same things the 3D viewport keys on. The Diffuse mode
    /// evaluates the whole texture chain, so this is not free and must not run
    /// per frame.
    fn refresh_graph_map(&mut self, ctx: &egui::Context) {
        const RES: usize = 257;
        let Some(id) = self.graph.find_terminal("height").map(str::to_string) else {
            self.graph_map_tex = None;
            return;
        };
        let rctx = Context::with_rasters(&self.project, RES, self.rasters.clone());
        let sig = format!(
            "{}|{:?}|{}|{}|{:?}|{}",
            self.graph.signature(&id, &rctx),
            self.view_mode,
            self.climb_limit,
            self.spots.len(),
            self.project.materials,
            self.rasters.signature()
        );
        if self.graph_map_sig == sig && self.graph_map_tex.is_some() {
            return;
        }
        if self.view_mode == ViewMode::Diffuse {
            self.refresh_materials();
        }
        let opts = springen_core::PreviewOptions {
            rasters: self.rasters.clone(),
            res: RES,
            mode: self.view_mode,
            climb_limit: self.climb_limit,
            spots: &self.spots,
            materials: self.materials.as_ref(),
        };
        let Some(p) = springen_core::preview::render(&self.graph, &self.project, &opts) else {
            self.graph_map_tex = None;
            return;
        };
        // The buildable-ground sweep reads whatever field the tool last
        // rendered. Feeding it from here means the measurement is live while
        // you are wiring nodes, rather than only after a visit to the 3D view.
        // Both paths render at the same resolution from the same graph, so
        // they cannot report different numbers.
        self.terrain_field = Some(p.height.clone());
        let px: Vec<Color32> = (0..RES * RES)
            .map(|i| Color32::from_rgb(p.colour[i * 3], p.colour[i * 3 + 1], p.colour[i * 3 + 2]))
            .collect();
        self.graph_map_tex = Some(ctx.load_texture(
            "graph-map",
            egui::ColorImage {
                size: [RES, RES],
                pixels: px,
                source_size: Vec2::splat(RES as f32),
            },
            egui::TextureOptions::LINEAR,
        ));
        self.graph_map_sig = sig;
    }

    /* --------------------------------------------------------------- bake */

    fn start_bake(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        if let Some(reason) = size_rejection(self.project.units_x, self.project.units_y) {
            self.toast(ctx, Level::Error, reason);
            return;
        }
        // `--smoke` names the exact file it wants; everything else follows the
        // name-and-version convention.
        let target = self.smoke.clone().unwrap_or_else(|| {
            self.out_dir
                .join(format!("{}.sd7", self.project.archive_stem()))
        });
        if let Err(e) = std::fs::create_dir_all(&self.out_dir) {
            self.toast(
                ctx,
                Level::Error,
                format!("Cannot create {}: {e}", self.out_dir.display()),
            );
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (stage_tx, stage_rx) = mpsc::channel();
        let project = self.project.clone();
        let graph = self.graph.clone();
        let opts = BakeOptions {
            rasters: self.rasters.clone(),
            game: self.game,
            spot_count: self.spot_count,
            min_separation: self.min_separation,
            max_spot_slope_deg: self.max_spot_slope,
            ..Default::default()
        };
        let out = target.clone();
        std::thread::spawn(move || {
            let work = out
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(".springen-work");
            let _ = std::fs::remove_dir_all(&work);
            let done = |name: &str, _secs: f64| {
                let _ = stage_tx.send(name.to_string());
            };
            let result = std::fs::create_dir_all(&work)
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    bake_with_progress(&project, &graph, &opts, &work, &done)
                        .map_err(|e| e.to_string())
                })
                .and_then(|(bp, report)| {
                    done("Packing the archive", 0.0);
                    bp.write_sd7(&out).map_err(|e| e.to_string())?;
                    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                    Ok((report, out.clone(), size))
                });
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(result);
        });
        self.job = Some(BakeJob {
            rx,
            stages: stage_rx,
            stage: "Starting".into(),
            started: ctx.input(|i| i.time),
        });
    }

    fn poll_bake(&mut self, ctx: &egui::Context) {
        if let Some(job) = &mut self.job {
            while let Ok(name) = job.stages.try_recv() {
                job.stage = name;
            }
        }
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Ok(Ok((report, path, size))) => {
                let spots = report.spots.len();
                self.last_report = Some(report);
                self.job = None;
                self.toast(
                    ctx,
                    Level::Ok,
                    format!(
                        "Wrote {} — {:.1} MB, {} spots",
                        path.display(),
                        size as f64 / 1048576.0,
                        spots
                    ),
                );
            }
            Ok(Err(e)) => {
                self.job = None;
                let hint = if e.contains("denied") || e.contains("os error 5") {
                    e.to_string()
                } else {
                    e
                };
                self.toast(ctx, Level::Error, hint);
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.toast(ctx, Level::Error, "The bake thread stopped unexpectedly.");
            }
        }
    }
}

/* ============================================================ rendering */

impl eframe::App for SpringenApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        if let Some(out) = self.smoke.clone() {
            if self.frames == 3 {
                self.out_dir = out
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                self.project.name = out
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Smoke".into());
                self.start_bake(&ctx);
            }
            ctx.request_repaint();
        }
        let was_baking = self.job.is_some();
        self.poll_bake(&ctx);
        if self.smoke.is_some() && was_baking && self.job.is_none() {
            for t in &self.toasts {
                println!("{}", t.text);
            }
            let ok = self.last_report.is_some();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            if !ok {
                std::process::exit(1);
            }
        }
        if self.screen == Screen::Workspace && self.smoke.is_none() {
            self.shortcuts(&ctx);
        }
        match self.screen {
            Screen::Splash => self.splash(ui),
            Screen::Projects => self.projects(ui),
            Screen::Workspace => self.workspace(ui, _frame.gl()),
        }
        if self.screen == Screen::Workspace {
            self.track_edits(&ctx);
        }
        self.confirm_dialog(&ctx);
        self.draw_toasts(&ctx);
        if let Some(job) = &self.job {
            let elapsed = ctx.input(|i| i.time) - job.started;
            let stage = job.stage.clone();
            self.veil(&ctx, elapsed, &stage);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.save_settings();
        if let (Some(gl), Some(r)) = (gl, self.renderer.take()) {
            if let Ok(r) = r.lock() {
                r.destroy(gl);
            }
        }
    }
}

impl SpringenApp {
    /* ------------------------------------------------------------- splash */

    /// One preload stage per frame, so the window paints between them.
    fn advance_boot(&mut self, ctx: &egui::Context, painter: &egui::Painter) {
        match self.boot {
            BootStep::Fonts => {
                // Force the atlas for every face before the workspace needs it.
                // Laying the glyphs out is what builds the atlas. Doing it
                // here keeps the first workspace frame from stalling on it.
                for role in [
                    FontRole::Display,
                    FontRole::Ui,
                    FontRole::UiStrong,
                    FontRole::Mono,
                ] {
                    for size in [10.0, 11.0, 12.0, 13.0, 15.0, 18.0, 26.0] {
                        let _ = painter.layout_no_wrap(
                            "0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz ×²°–…"
                                .to_owned(),
                            theme::font(role, size),
                            theme::TEXT_PRIMARY,
                        );
                    }
                }
            }
            BootStep::NodeTypes => {
                registry();
            }
            BootStep::Evaluator => {
                // Warm the cache at thumbnail size; the workspace opens with
                // every node already evaluated.
                let rctx = Context::new(&self.project, THUMB_RES);
                let ids: Vec<String> = self.graph.nodes.iter().map(|n| n.id.clone()).collect();
                for id in ids {
                    self.graph.evaluate(&id, &rctx);
                }
            }
            BootStep::Previews => {
                for (kind, _) in STARTERS {
                    let _ = self.starter_thumb(ctx, kind);
                }
                self.refresh_thumbs(ctx);
            }
            BootStep::Viewport => {
                self.repropose_spots();
            }
            BootStep::Done => {}
        }
        let i = self.boot.index();
        if i + 1 < BootStep::ORDER.len() {
            self.boot = BootStep::ORDER[i + 1];
        }
    }

    /// The preload window: a placeholder that primes the systems behind it,
    /// then hands over to the project browser. No controls, no statistics --
    /// there is nothing here to decide.
    fn splash(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let mut painter: Option<egui::Painter> = None;
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(theme::SURFACE_PANEL))
            .show(root, |ui| {
                let rect = ui.available_rect_before_wrap();
                painter = Some(ui.painter().clone());
                let p = ui.painter();
                p.rect_stroke(
                    rect.shrink(0.5),
                    0.0,
                    Stroke::new(1.0, theme::BORDER_PANEL),
                    egui::StrokeKind::Inside,
                );

                let pad = 26.0;
                contour_mark_at(
                    p,
                    egui::Rect::from_min_size(rect.min + Vec2::splat(pad), Vec2::splat(38.0)),
                );
                p.text(
                    rect.min + Vec2::new(pad + 52.0, pad + 6.0),
                    egui::Align2::LEFT_TOP,
                    "SPRINGEN",
                    theme::font(FontRole::Display, 26.0),
                    theme::TEXT_PRIMARY,
                );
                p.text(
                    rect.min + Vec2::new(pad + 53.0, pad + 34.0),
                    egui::Align2::LEFT_TOP,
                    "Map design for Spring / Recoil",
                    theme::font(FontRole::Ui, 12.0),
                    theme::TEXT_TERTIARY,
                );

                // A determinate bar: the stages are known, so show real progress.
                let done = self.boot.index() as f32 / (BootStep::ORDER.len() - 1) as f32;
                let bar = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + pad, rect.bottom() - pad - 26.0),
                    Vec2::new(rect.width() - pad * 2.0, 2.0),
                );
                p.rect_filled(bar, 0.0, theme::GRAY_800);
                p.rect_filled(
                    egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * done, 2.0)),
                    0.0,
                    theme::ACCENT,
                );
                p.text(
                    egui::pos2(bar.left(), bar.bottom() + 8.0),
                    egui::Align2::LEFT_TOP,
                    self.boot.label(),
                    theme::font(FontRole::Ui, 11.0),
                    theme::TEXT_SECONDARY,
                );
                p.text(
                    egui::pos2(bar.right(), bar.bottom() + 8.0),
                    egui::Align2::RIGHT_TOP,
                    format!("{}%", (done * 100.0).round()),
                    theme::font(FontRole::Mono, 11.0),
                    theme::TEXT_TERTIARY,
                );
            });

        if self.hold_screen {
            return;
        }
        if self.boot == BootStep::Done {
            // Grow into the real window and open the project browser.
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(1600.0, 1000.0)));
            self.screen = Screen::Projects;
        } else if let Some(p) = painter {
            self.advance_boot(&ctx, &p);
        }
        ctx.request_repaint();
    }

    /* ----------------------------------------------------------- projects */

    fn projects(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let mut open_file = false;
        let mut open_recent: Option<PathBuf> = None;
        let mut import_map = false;
        for (kind, _) in STARTERS {
            let _ = self.starter_thumb(&ctx, kind);
        }
        egui::Panel::top("projects-title")
            .exact_size(theme::TOOLBAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_CHROME)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    contour_mark(ui, 18.0);
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("SPRINGEN")
                            .font(theme::font(FontRole::Display, 15.0))
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ghost_button(ui, "Open project").clicked() {
                            open_file = true;
                        }
                        ui.add_space(6.0);
                        if ghost_button(ui, "Import map").clicked() {
                            import_map = true;
                        }
                        ui.add_space(6.0);
                        if ghost_button(ui, "Continue").clicked() {
                            self.screen = Screen::Workspace;
                        }
                    });
                });
            });
        egui::CentralPanel::no_frame()
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_APP)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(root, |ui| {
                if !self.recent.is_empty() {
                    theme::micro_label(ui, "Recent");
                    ui.add_space(6.0);
                    let recent = self.recent.clone();
                    for path in recent {
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let row = ui.allocate_response(
                            Vec2::new(ui.available_width().min(720.0), theme::ROW_H),
                            Sense::click(),
                        );
                        let r = row.rect;
                        if row.hovered() {
                            ui.painter()
                                .rect_filled(r, theme::R_CONTROL, theme::SURFACE_HOVER);
                        }
                        ui.painter().text(
                            r.left_center() + Vec2::new(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &name,
                            theme::font(FontRole::Ui, 13.0),
                            theme::TEXT_PRIMARY,
                        );
                        ui.painter().text(
                            r.right_center() - Vec2::new(8.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            path.parent()
                                .map(|d| d.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            theme::font(FontRole::Mono, 11.0),
                            theme::TEXT_DATA,
                        );
                        if row.clicked() {
                            open_recent = Some(path.clone());
                        }
                    }
                    ui.add_space(16.0);
                }
                theme::micro_label(ui, "Starters");
                ui.add_space(6.0);
                let mut chosen: Option<&str> = None;
                ui.horizontal_wrapped(|ui| {
                    for (kind, label) in STARTERS {
                        // Square, because the map is: a wide tile can only
                        // show a band across the middle, which hides the one
                        // thing you are choosing between -- an archipelago
                        // cropped to its centre looks like a continent.
                        let size = Vec2::new(184.0, 236.0);
                        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                        let hovered = resp.hovered();
                        ui.painter().rect_filled(
                            rect,
                            theme::R_CONTROL,
                            if hovered {
                                theme::SURFACE_HOVER
                            } else {
                                theme::SURFACE_PANEL
                            },
                        );
                        ui.painter().rect_stroke(
                            rect,
                            theme::R_CONTROL,
                            Stroke::new(
                                1.0,
                                if hovered {
                                    theme::ACCENT
                                } else {
                                    theme::BORDER_PANEL
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        // The baked ground, not an icon: this is the map.
                        let strip = egui::Rect::from_min_size(
                            rect.min + Vec2::new(1.0, 1.0),
                            Vec2::splat(rect.width() - 2.0),
                        );
                        if let Some(tex) = self.starter_thumbs.get(*kind) {
                            ui.painter().image(
                                tex.id(),
                                strip,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        ui.painter().text(
                            rect.min + Vec2::new(12.0, 192.0),
                            egui::Align2::LEFT_TOP,
                            label,
                            theme::font(FontRole::Ui, 13.0),
                            theme::TEXT_PRIMARY,
                        );
                        let g = starter_graph(kind);
                        ui.painter().text(
                            rect.min + Vec2::new(12.0, 213.0),
                            egui::Align2::LEFT_TOP,
                            format!("{} nodes", g.nodes.len()),
                            theme::font(FontRole::Mono, 11.0),
                            theme::TEXT_DATA,
                        );
                        if resp.clicked() {
                            chosen = Some(kind);
                        }
                    }
                });
                if let Some(kind) = chosen {
                    self.graph = starter_graph(kind);
                    // The starter's surfaces, light and symmetry come with its
                    // terrain; the name, size and seed already chosen do not.
                    springen_core::starter::apply_starter(&mut self.project, kind);
                    self.project.spots.clear();
                    self.selected_spot = None;
                    self.thumb_sig.clear();
                    self.terrain_sig.clear();
                    self.materials = None;
                    self.mat_thumbs.clear();
                    self.repropose_spots();
                    self.fit_pending = true;
                    self.saved_doc = String::new();
                    self.project_path = None;
                    self.screen = Screen::Workspace;
                }
            });
        if open_file {
            self.open_project(&ctx);
            if self.project_path.is_some() {
                self.screen = Screen::Workspace;
            }
        }
        if let Some(path) = open_recent {
            self.open_path(&ctx, path);
        }
        if import_map {
            self.import_map(&ctx);
        }
    }

    /* ---------------------------------------------------------- workspace */

    fn workspace(&mut self, root: &mut egui::Ui, gl: Option<&Arc<eframe::glow::Context>>) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        self.refresh_thumbs(ctx);
        if self.mode == CanvasMode::Terrain {
            if let Some(gl) = gl {
                self.refresh_terrain(gl);
            }
        }
        let d = derive(self.project.units_x, self.project.units_y);

        /* toolbar */
        let mut want_bake = false;
        let mut want_fit = false;
        let mut want_open = false;
        let mut want_save = false;
        let mut want_home = false;
        let mut want_save_as = false;
        let mut want_import = false;
        let mut dirty = false;
        egui::Panel::top("toolbar")
            .exact_size(theme::TOOLBAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_CHROME)
                    .inner_margin(egui::Margin::symmetric(12, 0))
                    .stroke(Stroke::new(1.0, theme::BORDER_HAIRLINE)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    contour_mark(ui, 18.0);
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("SPRINGEN")
                            .font(theme::font(FontRole::Display, 15.0))
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.add_space(16.0);
                    toolbar_rule(ui);
                    ui.add_space(12.0);
                    if ghost_button(ui, "Open").clicked() {
                        want_open = true;
                    }
                    ui.add_space(6.0);
                    if ghost_button(ui, "Save").clicked() {
                        want_save = true;
                    }
                    ui.add_space(6.0);
                    if ghost_button(ui, "Save as").clicked() {
                        want_save_as = true;
                    }
                    ui.add_space(6.0);
                    if ghost_button(ui, "Import map").clicked() {
                        want_import = true;
                    }
                    ui.add_space(6.0);
                    if ghost_button(ui, "Home").clicked() {
                        want_home = true;
                    }

                    ui.add_space(12.0);
                    toolbar_rule(ui);
                    ui.add_space(12.0);
                    egui::containers::menu::MenuButton::from_button(
                        egui::Button::new(
                            RichText::new("Panels")
                                .font(theme::font(FontRole::Ui, 12.0))
                                .color(theme::TEXT_SECONDARY),
                        )
                        .frame(false),
                    )
                    .ui(ui, |ui| {
                        ui.set_min_width(170.0);
                        for pane in Pane::ALL {
                            let mut on = self.layout.is_open(pane);
                            if ui.checkbox(&mut on, pane.title()).clicked() {
                                self.layout.set_open(pane, on);
                            }
                        }
                        ui.separator();
                        if ui.button("All to right").clicked() {
                            for pane in Pane::ALL {
                                self.layout.move_to(pane, Dock::Right);
                            }
                        }
                        if ui.button("Reset layout").clicked() {
                            self.layout = Layout::stock();
                        }
                    });

                    ui.add_space(12.0);
                    toolbar_rule(ui);
                    ui.add_space(12.0);

                    ui.label(
                        RichText::new("Size")
                            .font(theme::font(FontRole::Ui, 12.0))
                            .color(theme::TEXT_SECONDARY),
                    );
                    let mut ux = self.project.units_x as i32;
                    let mut uy = self.project.units_y as i32;
                    if ui
                        .add(egui::DragValue::new(&mut ux).range(2..=64).speed(0.1))
                        .changed()
                    {
                        self.project.units_x = nearest_valid_units(f64::from(ux));
                        dirty = true;
                    }
                    ui.label(RichText::new("×").color(theme::TEXT_TERTIARY));
                    if ui
                        .add(egui::DragValue::new(&mut uy).range(2..=64).speed(0.1))
                        .changed()
                    {
                        self.project.units_y = nearest_valid_units(f64::from(uy));
                        dirty = true;
                    }
                    ui.label(
                        RichText::new(format!("{} × {} elmos", d.elmos_x, d.elmos_y))
                            .font(theme::font(FontRole::Mono, 12.0))
                            .color(theme::TEXT_DATA),
                    );

                    ui.add_space(12.0);
                    toolbar_rule(ui);
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Seed")
                            .font(theme::font(FontRole::Ui, 12.0))
                            .color(theme::TEXT_SECONDARY),
                    );
                    let mut seed = self.project.seed;
                    if ui.add(egui::DragValue::new(&mut seed).speed(1.0)).changed() {
                        self.project.seed = seed;
                        dirty = true;
                    }

                    ui.add_space(12.0);
                    toolbar_rule(ui);
                    ui.add_space(12.0);
                    for (mode, label) in [(CanvasMode::Graph, "Graph"), (CanvasMode::Terrain, "3D")]
                    {
                        if segmented(ui, label, self.mode == mode).clicked() {
                            self.mode = mode;
                        }
                    }
                    if self.mode == CanvasMode::Terrain || self.graph_map {
                        ui.add_space(10.0);
                        toolbar_rule(ui);
                        ui.add_space(10.0);
                    }
                    if self.mode == CanvasMode::Graph {
                        ui.add_space(10.0);
                        toolbar_rule(ui);
                        ui.add_space(10.0);
                        if segmented(ui, "Map", self.graph_map).clicked() {
                            self.graph_map = !self.graph_map;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Bake layers").clicked() {
                            want_bake = true;
                        }
                        ui.add_space(6.0);
                        if ghost_button(ui, "Fit").clicked() {
                            want_fit = true;
                        }
                    });
                });
            });

        /* status bar */
        egui::Panel::bottom("statusbar")
            .exact_size(theme::STATUSBAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_CHROME)
                    .inner_margin(egui::Margin::symmetric(12, 0))
                    .stroke(Stroke::new(1.0, theme::BORDER_HAIRLINE)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    let mono = |ui: &mut egui::Ui, s: String| {
                        ui.label(
                            RichText::new(s)
                                .font(theme::font(FontRole::Mono, 11.0))
                                .color(theme::TEXT_TERTIARY),
                        );
                    };
                    mono(ui, format!("{}x{} units", d.units_x, d.units_y));
                    status_sep(ui);
                    mono(ui, format!("mapx {}", d.mapx));
                    status_sep(ui);
                    mono(ui, format!("heightmap {} × {}", d.height_w, d.height_h));
                    status_sep(ui);
                    mono(ui, format!("{} nodes", self.graph.nodes.len()));
                    status_sep(ui);
                    mono(
                        ui,
                        match &self.project_path {
                            Some(p) => p
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| p.display().to_string()),
                            None => "unsaved".into(),
                        },
                    );
                    status_sep(ui);
                    mono(ui, format!("preview {PREVIEW_RES}²"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        mono(ui, "glow".into());
                        status_sep(ui);
                        let valid = valid_units(d.units_x) && valid_units(d.units_y);
                        ui.label(
                            RichText::new(if valid { "size legal" } else { "size illegal" })
                                .font(theme::font(FontRole::Mono, 11.0))
                                .color(if valid {
                                    theme::GOOD_500
                                } else {
                                    theme::ALERT_500
                                }),
                        );
                    });
                });
            });

        /* palette */
        egui::Panel::left("palette")
            .exact_size(theme::PALETTE_W)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_PANEL)
                    .inner_margin(egui::Margin::same(theme::PANEL_PAD as i8))
                    .stroke(Stroke::new(1.0, theme::BORDER_HAIRLINE)),
            )
            .show(root, |ui| {
                let mut add: Option<&'static str> = None;
                ui.add(
                    egui::TextEdit::singleline(&mut self.palette_filter)
                        .hint_text("Filter")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                let needle = self.palette_filter.trim().to_ascii_lowercase();
                let shows = |spec: &springen_core::graph::NodeSpec| {
                    needle.is_empty()
                        || spec.label.to_ascii_lowercase().contains(&needle)
                        || spec.type_name.contains(&needle)
                        || spec.cat.to_ascii_lowercase().contains(&needle)
                };
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for cat in registry().categories() {
                        if !registry().all().any(|s| s.cat == cat && shows(s)) {
                            continue;
                        }
                        theme::micro_label(ui, cat);
                        for spec in registry().all().filter(|s| s.cat == cat && shows(s)) {
                            let (rect, resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), theme::ROW_H),
                                Sense::click(),
                            );
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, 0.0, theme::SURFACE_HOVER);
                                // Hover adds a 2px accent left edge on list items.
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_size(
                                        rect.min,
                                        Vec2::new(2.0, rect.height()),
                                    ),
                                    0.0,
                                    theme::ACCENT,
                                );
                            }
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min + Vec2::new(8.0, rect.height() / 2.0 - 3.0),
                                    Vec2::splat(6.0),
                                ),
                                0.0,
                                theme::class_colour(spec.cat),
                            );
                            ui.painter().text(
                                rect.min + Vec2::new(22.0, rect.height() / 2.0),
                                egui::Align2::LEFT_CENTER,
                                spec.label,
                                theme::font(FontRole::Ui, 12.0),
                                theme::TEXT_PRIMARY,
                            );
                            if resp.clicked() {
                                add = Some(spec.type_name);
                            }
                        }
                        ui.add_space(6.0);
                    }
                });
                if let Some(type_name) = add {
                    let id = self.graph.add(type_name, 80.0, 80.0, &[]);
                    self.view.selected = Some(id);
                    dirty = true;
                }
            });

        /* panes */
        self.rails(root, &d, &mut dirty);

        /* canvas */
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(theme::SURFACE_CANVAS))
            .show(root, |ui| match self.mode {
                CanvasMode::Graph => {
                    // The map, beside the graph. Added before the graph view
                    // so egui gives it its width first and the canvas takes
                    // what is left.
                    if self.graph_map {
                        self.graph_map_panel(ui);
                    }
                    if self.fit_pending {
                        self.view.fit(&self.graph, ui.available_rect_before_wrap());
                        self.fit_pending = false;
                    }
                    if want_fit {
                        self.view.fit(&self.graph, ui.available_rect_before_wrap());
                    }
                    let action = self.view.show(ui, &mut self.graph, &self.thumbs);
                    match action {
                        Action::Rejected(reason) => self.toast(ctx, Level::Error, reason),
                        Action::Connected | Action::Disconnected | Action::Deleted(_) => {
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                CanvasMode::Terrain => {
                    let rect = ui.available_rect_before_wrap();
                    let response = ui.allocate_rect(rect, Sense::click_and_drag());
                    let world = [d.elmos_x as f32, d.elmos_y as f32];
                    // Spots come first: a drag that starts on one moves it,
                    // and only a drag that starts on open ground orbits.
                    // The brush comes first: while it is armed a drag paints
                    // rather than picking a mex or orbiting.
                    let painting = self.sculpt_interaction(&response, rect, &d);
                    if painting {
                        dirty = true;
                    }
                    let on_spot = painting
                        || self.mex_interaction(ui, &response, rect, &d)
                        || self.route_interaction(ui, &response, rect, &d)
                        || self.box_interaction(&response, rect, &d);
                    if !on_spot && self.camera.interact(ui, &response, world[0].max(world[1])) {
                        ui.ctx().request_repaint();
                    }

                    match self.renderer.clone() {
                        Some(renderer) => {
                            let camera = self.camera;
                            let range = (self.project.max_height - self.project.min_height) as f32;
                            let sea =
                                water_level_t(self.project.min_height, self.project.max_height)
                                    as f32;
                            let aspect = rect.width() / rect.height().max(1.0);
                            // The sun and water mapinfo declares, so the light
                            // here is the light the engine will use.
                            let e = &self.project.environment;
                            let sd = e.sun_dir();
                            let sun = [sd[0] as f32, sd[1] as f32, sd[2] as f32];
                            let water = [
                                e.water_surface[0] as f32,
                                e.water_surface[1] as f32,
                                e.water_surface[2] as f32,
                            ];
                            let cb = egui::PaintCallback {
                                rect,
                                callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                                    move |_info, painter| {
                                        if let Ok(r) = renderer.lock() {
                                            r.paint(
                                                painter.gl(),
                                                &camera,
                                                aspect,
                                                world,
                                                range,
                                                sea,
                                                sun,
                                                water,
                                            );
                                        }
                                    },
                                )),
                            };
                            ui.painter().add(cb);
                            self.draw_overlays(ui, rect, &d);
                        }
                        None => {
                            ui.painter().rect_filled(rect, 0.0, theme::SURFACE_CANVAS);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "No OpenGL context",
                                theme::font(FontRole::Ui, 13.0),
                                theme::TEXT_TERTIARY,
                            );
                        }
                    }
                }
            });

        if dirty {
            self.repropose_spots();
        }
        if want_open {
            self.open_project(ctx);
        }
        if want_save {
            self.save_project(ctx, false);
        }
        if want_save_as {
            self.save_project(ctx, true);
        }
        if want_import {
            self.import_map(ctx);
        }
        if want_home {
            self.go_home(ctx);
        }
        if want_bake {
            self.start_bake(ctx);
        }
    }

    /// An elmo coordinate lifted onto the terrain the viewport is showing.
    fn ground_point(&self, x: f64, z: f64, d: &springen_core::Derived) -> [f32; 3] {
        let range = (self.project.max_height - self.project.min_height) as f32;
        let sea = water_level_t(self.project.min_height, self.project.max_height) as f32;
        let y = match &self.terrain_field {
            Some(f) => {
                let r = (f.res - 1) as f64;
                let h = springen_core::field::sample_bilinear(
                    f,
                    (x / f64::from(d.elmos_x) * r).clamp(0.0, r),
                    (z / f64::from(d.elmos_y) * r).clamp(0.0, r),
                ) as f32;
                (h.max(sea) - sea) * range * self.camera.exaggeration
            }
            None => 0.0,
        };
        [x as f32, y, z as f32]
    }

    /// Where a spot sits in the viewport's world space.
    fn spot_world(&self, s: &zk::MetalSpot, d: &springen_core::Derived) -> [f32; 3] {
        self.ground_point(s.x, s.z, d)
    }

    /// The brush's own controls, shown with the sculpt node's stroke list.
    fn brush_controls(&mut self, ui: &mut egui::Ui) {
        use springen_core::graph::StrokeMode;
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            let label = if self.brush.active {
                "Painting — drag on the terrain"
            } else {
                "Paint strokes"
            };
            if ui.selectable_label(self.brush.active, label).clicked() {
                self.brush.active = !self.brush.active;
            }
        });
        if !self.brush.active {
            return;
        }
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            for (m, name) in [
                (StrokeMode::Raise, "Raise"),
                (StrokeMode::Smooth, "Smooth"),
                (StrokeMode::Level, "Level"),
            ] {
                if ui.selectable_label(self.brush.mode == m, name).clicked() {
                    self.brush.mode = m;
                }
            }
            if self.brush.mode == StrokeMode::Raise
                && ui.selectable_label(self.brush.lower, "Dig").clicked()
            {
                self.brush.lower = !self.brush.lower;
            }
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Radius")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.brush.radius)
                        .range(32.0..=4000.0)
                        .speed(4.0)
                        .max_decimals(0)
                        .suffix(" elmos"),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            let (label, suffix, range) = match self.brush.mode {
                StrokeMode::Raise => ("Height", " elmos", 1.0..=600.0),
                _ => ("Rate", "", 0.05..=1.0),
            };
            ui.label(
                RichText::new(label)
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The two meanings do not share a scale, so switching mode
                // brings the value into its new range rather than leaving a
                // rate of 60 or a height of 0.3.
                if self.brush.mode == StrokeMode::Raise {
                    if self.brush.strength < 1.0 {
                        self.brush.strength = 60.0;
                    }
                } else if self.brush.strength > 1.0 {
                    self.brush.strength = 0.5;
                }
                ui.add(
                    egui::DragValue::new(&mut self.brush.strength)
                        .range(range)
                        .speed(if self.brush.mode == StrokeMode::Raise {
                            1.0
                        } else {
                            0.01
                        })
                        .suffix(suffix),
                );
            });
        });
    }

    /// The project's start boxes, or the ones its symmetry implies.
    ///
    /// Derived until someone moves one, so changing the symmetry keeps
    /// re-deriving them rather than leaving a two-team layout on a map that is
    /// now four-way symmetric.
    fn start_areas(&self, d: &springen_core::Derived) -> Vec<springen_core::lua::StartArea> {
        self.project
            .start_boxes
            .clone()
            .unwrap_or_else(|| springen_core::lua::default_areas(d, &self.project.mex_sym))
    }

    /// Move one start box and carry its symmetry images with it.
    ///
    /// The same rule metal spots and sculpt strokes follow: a hand edit moves
    /// the whole group. Moving one team's box without the others is how a map
    /// goes quietly unfair, and it is worse here than for a mex — a base
    /// position is the first thing anyone notices and the last thing anyone
    /// re-checks.
    ///
    /// Images are matched to boxes by position rather than by index, because
    /// an operator's image order is not the box list's order for `quad`,
    /// `rot90` or `rot72`.
    fn drag_start_box(&mut self, i: usize, dx: f64, dz: f64, d: &springen_core::Derived) {
        let (w, h) = (f64::from(d.elmos_x), f64::from(d.elmos_y));
        let mut areas = self.start_areas(d);
        if i >= areas.len() {
            return;
        }
        let before = areas[i].centre();
        areas[i].translate(dx, dz, w, h);
        let after = areas[i].centre();
        if self.mirror_edits {
            let images =
                springen_core::zk::symmetry_images(before.0, before.1, &self.project.mex_sym, w, h);
            let moved =
                springen_core::zk::symmetry_images(after.0, after.1, &self.project.mex_sym, w, h);
            // Matched on where each image *was*, and claimed once, so no box
            // is moved twice while another is left behind.
            let centres: Vec<(f64, f64)> = areas.iter().map(|a| a.centre()).collect();
            let pick = springen_core::zk::assign_images(&centres, i, &images);
            for (now, which) in moved.iter().zip(pick) {
                let Some(j) = which else { continue };
                let c = areas[j].centre();
                areas[j].translate(now.0 - c.0, now.1 - c.1, w, h);
            }
        }
        self.project.start_boxes = Some(areas);
    }

    /// Select and drag start boxes in the viewport.
    fn box_interaction(
        &mut self,
        response: &egui::Response,
        rect: Rect,
        d: &springen_core::Derived,
    ) -> bool {
        if !self.show_boxes || self.mode != CanvasMode::Terrain {
            return false;
        }
        let areas = self.start_areas(d);
        if areas.is_empty() {
            return false;
        }
        if response.drag_stopped() {
            self.box_drag = None;
            self.box_corner = None;
        }
        if response.drag_started() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let hit = self.pick_ground(pos, rect, d).and_then(|(x, z)| {
                    (0..areas.len()).find(|i| {
                        let (x0, z0, x1, z1) = areas[*i].bounds();
                        x >= x0 && x <= x1 && z >= z0 && z <= z1
                    })
                });
                // A corner handle wins over the body it belongs to, so a
                // grab near the edge resizes rather than slides.
                let far = f64::from(d.elmos_x.max(d.elmos_y)) as f32 * 8.0;
                let mut corner = None;
                if let Some(sel) = self.selected_box.filter(|s| *s < areas.len()) {
                    for k in 0..4 {
                        let (cx, cz) = corner_of(&areas[sel], k);
                        let world = self.ground_point(cx, cz, d);
                        if let Some(p) = self.camera.project(world, rect, far) {
                            if (p - pos).length() < 12.0 {
                                corner = Some((sel, k));
                                break;
                            }
                        }
                    }
                }
                if let Some((sel, k)) = corner {
                    self.selected_box = Some(sel);
                    if response.drag_started() {
                        self.box_drag = Some(sel);
                        self.box_corner = Some(k);
                    }
                } else if hit.is_some() {
                    self.selected_box = hit;
                    if response.drag_started() {
                        self.box_drag = hit;
                        self.box_corner = None;
                    }
                } else if response.clicked() {
                    self.selected_box = None;
                }
            }
        }
        let Some(i) = self.box_drag else {
            return false;
        };
        let delta = response.drag_delta();
        if delta == Vec2::ZERO {
            return true;
        }
        let far = f64::from(d.elmos_x.max(d.elmos_y)) as f32 * 8.0;
        let anchor = match self.box_corner {
            Some(k) => corner_of(&areas[i], k),
            None => areas[i].centre(),
        };
        let at = self.ground_point(anchor.0, anchor.1, d);
        let Some((dx, dz)) = self.camera.screen_to_ground(at, delta, rect, far) else {
            return true;
        };
        match self.box_corner {
            // Dragging a corner moves that corner only, so the opposite one
            // stays put and the box grows from where you grabbed it.
            Some(k) => {
                let (w, h) = (f64::from(d.elmos_x), f64::from(d.elmos_y));
                let (x0, z0, x1, z1) = areas[i].bounds();
                let (nx, nz) = (anchor.0 + f64::from(dx), anchor.1 + f64::from(dz));
                let (fx, fz) = (
                    if k & 1 == 0 { x1 } else { x0 },
                    if k & 2 == 0 { z1 } else { z0 },
                );
                let mut next = areas.clone();
                next[i].set_bounds(nx, nz, fx, fz, w, h);
                if self.mirror_edits {
                    reflow_images(&mut next, i, &self.project.mex_sym, w, h);
                }
                self.project.start_boxes = Some(next);
            }
            None => self.drag_start_box(i, f64::from(dx), f64::from(dz), d),
        }
        true
    }

    /// Where the cursor is pointing on the terrain, in elmos.
    ///
    /// Reads the same field the viewport displaces its mesh from and applies
    /// the same waterline lift the vertex shader does, so the brush lands
    /// where the ground looks like it is rather than where the raw field says.
    fn pick_ground(&self, at: Pos2, rect: Rect, d: &springen_core::Derived) -> Option<(f64, f64)> {
        let field = self.terrain_field.as_ref()?;
        let range = (self.project.max_height - self.project.min_height) as f32;
        let sea = water_level_t(self.project.min_height, self.project.max_height) as f32;
        let world = [d.elmos_x as f32, d.elmos_y as f32];
        let exaggeration = self.camera.exaggeration;
        let r = field.res;
        let height = |x: f32, z: f32| -> f32 {
            let u = (x / world[0]).clamp(0.0, 1.0) * (r - 1) as f32;
            let v = (z / world[1]).clamp(0.0, 1.0) * (r - 1) as f32;
            let h = field.at(u.round() as usize, v.round() as usize) as f32;
            (h.max(sea) - sea) * range * exaggeration
        };
        self.camera
            .pick_terrain(at, rect, world, &height)
            .map(|(x, z)| (f64::from(x), f64::from(z)))
    }

    /// The selected node's stroke history, if it has one.
    fn selected_sculpt(&self) -> Option<(String, Vec<springen_core::graph::Stroke>)> {
        let id = self.view.selected.clone()?;
        let node = self.graph.node(&id)?;
        let spec = registry().get(&node.type_name)?;
        spec.params.iter().find(|p| p.ptype == PType::Strokes)?;
        Some((id, node.params.strokes("strokes").to_vec()))
    }

    /// Paint strokes onto the terrain.
    ///
    /// Returns whether the pointer is busy so the camera does not orbit under
    /// the brush. A stroke is only laid when the cursor has moved a third of a
    /// radius, which turns a drag into a track of overlapping discs rather than
    /// one stroke per frame — the history stays short enough to read, and the
    /// graph is not re-evaluated sixty times a second.
    fn sculpt_interaction(
        &mut self,
        response: &egui::Response,
        rect: Rect,
        d: &springen_core::Derived,
    ) -> bool {
        use springen_core::graph::{PVal, Stroke};
        if !self.brush.active {
            return false;
        }
        let Some((id, strokes)) = self.selected_sculpt() else {
            return false;
        };
        if response.drag_stopped() {
            self.brush_last = None;
        }
        if !(response.dragged() || response.drag_started()) {
            return false;
        }
        let Some(pos) = response.interact_pointer_pos() else {
            return true;
        };
        let Some((x, z)) = self.pick_ground(pos, rect, d) else {
            // Pointing at the sky. Still ours — releasing the pointer here
            // should not spin the camera.
            return true;
        };
        if let Some((lx, lz)) = self.brush_last {
            let moved = ((x - lx).powi(2) + (z - lz).powi(2)).sqrt();
            if moved < self.brush.radius / 3.0 {
                return true;
            }
        }
        // The height under the brush, which is what a levelling stroke is
        // levelling *to*. Sampled now rather than at replay time, so the stroke
        // records the ground it was drawn on and can be told later that the
        // ground has moved.
        let seat = self
            .terrain_field
            .as_ref()
            .map(|f| {
                let r = f.res;
                let u =
                    ((x / f64::from(d.elmos_x)).clamp(0.0, 1.0) * (r - 1) as f64).round() as usize;
                let v =
                    ((z / f64::from(d.elmos_y)).clamp(0.0, 1.0) * (r - 1) as f64).round() as usize;
                self.project.min_height
                    + f.at(u, v) * (self.project.max_height - self.project.min_height)
            })
            .unwrap_or(0.0);
        let mut next = strokes;
        next.push(Stroke {
            x,
            z,
            radius: self.brush.radius,
            strength: if self.brush.lower {
                -self.brush.strength
            } else {
                self.brush.strength
            },
            mode: self.brush.mode,
            seat,
        });
        if let Some(node) = self.graph.node_mut(&id) {
            node.params.set("strokes", PVal::Strokes(next));
        }
        self.brush_last = Some((x, z));
        true
    }

    /// The selected node's waypoint list, if it has one.
    ///
    /// Only the `points` key: a node with two point lists would need the
    /// inspector to say which one the viewport is editing, and none has.
    fn selected_route(&self) -> Option<(String, Vec<[f64; 2]>)> {
        let id = self.view.selected.clone()?;
        let node = self.graph.node(&id)?;
        let spec = registry().get(&node.type_name)?;
        spec.params.iter().find(|p| p.ptype == PType::Points)?;
        let pts = node.params.points("points").to_vec();
        Some((id, pts))
    }

    /// Drag a selected node's waypoints across the terrain.
    ///
    /// Works in every view mode, not just one: the view you want while routing
    /// a ramp is Slope, because it shows you the ground you are trying to get
    /// past. Returns whether the pointer is busy, so the camera does not orbit.
    fn route_interaction(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        d: &springen_core::Derived,
    ) -> bool {
        let Some((id, pts)) = self.selected_route() else {
            self.route_drag = None;
            return false;
        };
        if pts.is_empty() {
            return false;
        }
        let far = f64::from(d.elmos_x.max(d.elmos_y)) as f32 * 8.0;

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let mut best: Option<(f32, usize)> = None;
                for (i, q) in pts.iter().enumerate() {
                    let Some(p) = self
                        .camera
                        .project(self.ground_point(q[0], q[1], d), rect, far)
                    else {
                        continue;
                    };
                    let dist = (p - pos).length();
                    if dist < 16.0 && best.is_none_or(|(b, _)| dist < b) {
                        best = Some((dist, i));
                    }
                }
                self.route_drag = best.map(|(_, i)| i);
            }
        }
        if response.drag_stopped() {
            self.route_drag = None;
        }
        let Some(i) = self.route_drag.filter(|i| *i < pts.len()) else {
            return false;
        };
        let delta = response.drag_delta();
        if delta == Vec2::ZERO {
            return true;
        }
        let at = self.ground_point(pts[i][0], pts[i][1], d);
        let Some((dx, dz)) = self.camera.screen_to_ground(at, delta, rect, far) else {
            return true;
        };
        let mut next = pts;
        next[i] = [
            (next[i][0] + f64::from(dx)).clamp(0.0, f64::from(d.elmos_x)),
            (next[i][1] + f64::from(dz)).clamp(0.0, f64::from(d.elmos_y)),
        ];
        if let Some(node) = self.graph.node_mut(&id) {
            node.params.set("points", PVal::Points(next));
        }
        self.terrain_sig.clear();
        ui.ctx().request_repaint();
        true
    }

    /// Select and drag metal spots in the viewport.
    ///
    /// Returns whether the pointer is busy with a spot, so the camera does not
    /// also orbit. Only in the Metal view: everywhere else a drag is a look.
    fn mex_interaction(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        d: &springen_core::Derived,
    ) -> bool {
        if self.mode != CanvasMode::Terrain || self.view_mode != ViewMode::Metal {
            return false;
        }
        let far = f64::from(d.elmos_x.max(d.elmos_y)) as f32 * 8.0;
        let (w, h) = (f64::from(d.elmos_x), f64::from(d.elmos_y));

        // Pick by screen distance to the marker, which is where the eye is.
        let hit = |pos: Pos2, me: &SpringenApp| -> Option<usize> {
            let mut best: Option<(f32, usize)> = None;
            for (i, s) in me.spots.iter().enumerate() {
                let world = me.spot_world(s, d);
                let Some(p) = me.camera.project(world, rect, far) else {
                    continue;
                };
                let dist = (p - pos).length();
                if dist < 16.0 && best.is_none_or(|(b, _)| dist < b) {
                    best = Some((dist, i));
                }
            }
            best.map(|(_, i)| i)
        };

        if response.drag_started() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let picked = hit(pos, self);
                if picked.is_some() {
                    self.selected_spot = picked;
                    self.drag_spot = picked;
                } else if response.clicked() {
                    // Ctrl-click on open ground places a spot there, with its
                    // symmetry images. Placing a mex where you are looking is
                    // the obvious thing to want, and until this existed the
                    // only way in was a button that dropped one at the map
                    // centre — the one point every operator fixes, so it
                    // always arrived alone.
                    if ui.input(|i| i.modifiers.command) {
                        if let Some(at) = self.pick_ground(pos, rect, d) {
                            self.spot_note = self.add_spot_at(Some(at));
                        }
                    } else {
                        // Clicking open ground clears the selection, which is
                        // how every canvas in the product behaves.
                        self.selected_spot = None;
                    }
                }
            }
        }
        if response.drag_stopped() {
            self.drag_spot = None;
        }
        let Some(i) = self.drag_spot else {
            return false;
        };
        if i >= self.spots.len() {
            self.drag_spot = None;
            return false;
        }
        let delta = response.drag_delta();
        if delta == Vec2::ZERO {
            return true;
        }
        let at = self.spot_world(&self.spots[i], d);
        let Some((dx, dz)) = self.camera.screen_to_ground(at, delta, rect, far) else {
            return true;
        };
        self.adopt_spots();
        let to = (
            self.spots[i].x + f64::from(dx),
            self.spots[i].z + f64::from(dz),
        );
        zk::move_group(
            &mut self.spots,
            i,
            to,
            &self.project.mex_sym,
            w,
            h,
            self.mirror_edits,
        );
        self.commit_spots();
        ui.ctx().request_repaint();
        true
    }

    /// Map extent, and in the metal view the spots with their real extractor
    /// radius drawn on the terrain rather than flat on the screen.
    fn draw_overlays(&self, ui: &egui::Ui, rect: Rect, d: &springen_core::Derived) {
        let (w, h_world) = (f64::from(d.elmos_x), f64::from(d.elmos_y));
        let far = w.max(h_world) as f32 * 8.0;
        let range = (self.project.max_height - self.project.min_height) as f32;
        let sea = water_level_t(self.project.min_height, self.project.max_height) as f32;
        let field = self.terrain_field.clone();
        let height_at = |x: f64, z: f64| -> f32 {
            let Some(f) = &field else { return 0.0 };
            let r = (f.res - 1) as f64;
            let h = springen_core::field::sample_bilinear(
                f,
                (x / w * r).clamp(0.0, r),
                (z / h_world * r).clamp(0.0, r),
            ) as f32;
            (h.max(sea) - sea) * range * self.camera.exaggeration
        };
        let painter = ui.painter_at(rect);

        // The brush, drawn on the ground it will actually touch: a ring of
        // world points at the brush radius, each lifted to the terrain under
        // it. A flat screen-space circle would lie about where a stroke lands
        // on a slope, which is exactly where you need to know.
        if self.brush.active && self.selected_sculpt().is_some() {
            if let Some(pos) = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p)) {
                if let Some((bx, bz)) = self.pick_ground(pos, rect, d) {
                    let mut ring: Vec<Pos2> = Vec::new();
                    const SEGMENTS: usize = 48;
                    for i in 0..=SEGMENTS {
                        let a = i as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
                        let (x, z) = (
                            bx + self.brush.radius * a.cos(),
                            bz + self.brush.radius * a.sin(),
                        );
                        if let Some(p) =
                            self.camera
                                .project([x as f32, height_at(x, z), z as f32], rect, far)
                        {
                            ring.push(p);
                        }
                    }
                    let ink = if self.brush.lower {
                        theme::ALERT_300
                    } else {
                        theme::ACCENT
                    };
                    for seg in ring.windows(2) {
                        painter.line_segment([seg[0], seg[1]], Stroke::new(1.6, ink));
                    }
                    if let Some(c) =
                        self.camera
                            .project([bx as f32, height_at(bx, bz), bz as f32], rect, far)
                    {
                        painter.circle_filled(c, 2.5, ink);
                    }
                }
            }
        }

        // Start boxes, drawn on the ground rather than as flat rectangles:
        // a base sitting on a slope needs to be seen to be on that slope.
        if self.show_boxes {
            let areas = self.start_areas(d);
            let boxes = springen_core::lua::startboxes_from(&areas, d, &self.project.mex_sym, None);
            for (i, a) in areas.iter().enumerate() {
                let selected = self.selected_box == Some(i);
                let ink = if selected {
                    theme::ACCENT
                } else {
                    theme::CONTOUR_300
                };
                // Each edge subdivided, so it follows the terrain instead of
                // cutting through a hill between its corners.
                let n = a.poly.len();
                let mut screen: Vec<Pos2> = Vec::new();
                for k in 0..n {
                    let (ax, az) = a.poly[k];
                    let (bx, bz) = a.poly[(k + 1) % n];
                    const STEPS: usize = 12;
                    for t in 0..STEPS {
                        let f = t as f64 / STEPS as f64;
                        let (x, z) = (ax + (bx - ax) * f, az + (bz - az) * f);
                        if let Some(p) =
                            self.camera
                                .project([x as f32, height_at(x, z), z as f32], rect, far)
                        {
                            screen.push(p);
                        }
                    }
                }
                if screen.len() > 2 {
                    screen.push(screen[0]);
                    for seg in screen.windows(2) {
                        painter.line_segment(
                            [seg[0], seg[1]],
                            Stroke::new(if selected { 2.4 } else { 1.4 }, ink),
                        );
                    }
                }
                // Corner handles on the selected box, so it can be resized
                // where it is rather than only by typing bounds.
                if selected {
                    for k in 0..4u8 {
                        let (cx, cz) = corner_of(a, k);
                        if let Some(p) = self.camera.project(
                            [cx as f32, height_at(cx, cz), cz as f32],
                            rect,
                            far,
                        ) {
                            let r = egui::Rect::from_center_size(p, Vec2::splat(8.0));
                            painter.rect_filled(r, 1.0, theme::SURFACE_PANEL);
                            painter.rect_stroke(
                                r,
                                1.0,
                                Stroke::new(1.5, theme::ACCENT),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                }
                // The start point, which is where a commander actually lands.
                if let Some(b) = boxes.get(i) {
                    if let Some((sx, sz)) = b.start_points.first() {
                        if let Some(p) = self.camera.project(
                            [*sx as f32, height_at(*sx, *sz), *sz as f32],
                            rect,
                            far,
                        ) {
                            painter.circle_filled(p, 4.0, ink);
                            painter.circle_stroke(p, 7.0, Stroke::new(1.0, theme::CONTOUR_300));
                            painter.text(
                                p + Vec2::new(10.0, -4.0),
                                egui::Align2::LEFT_CENTER,
                                &a.short,
                                theme::font(FontRole::Mono, 11.0),
                                ink,
                            );
                        }
                    }
                }
            }
        }

        // The map border, so the extent is legible from any angle.
        let mut border: Vec<Pos2> = Vec::new();
        let steps = 24;
        for i in 0..=steps * 4 {
            let t = (i % (steps * 4)) as f64 / steps as f64;
            let (x, z) = match t as usize {
                0 => (t.fract() * w, 0.0),
                1 => (w, t.fract() * h_world),
                2 => (w - t.fract() * w, h_world),
                _ => (0.0, h_world - t.fract() * h_world),
            };
            if let Some(p) = self
                .camera
                .project([x as f32, height_at(x, z), z as f32], rect, far)
            {
                border.push(p);
            }
        }
        for seg in border.windows(2) {
            painter.line_segment([seg[0], seg[1]], Stroke::new(1.0, theme::BORDER_STRONG));
        }

        // The selected node's route, in every view mode: the one you want
        // while placing a ramp is Slope, because it shows the ground you are
        // trying to get past.
        if let Some((_, pts)) = self.selected_route() {
            let mut screen: Vec<Option<Pos2>> = Vec::with_capacity(pts.len());
            for q in &pts {
                let world = self.ground_point(q[0], q[1], d);
                screen.push(self.camera.project(world, rect, far));
            }
            for pair in screen.windows(2) {
                if let (Some(a), Some(b)) = (pair[0], pair[1]) {
                    painter.line_segment([a, b], Stroke::new(2.0, theme::ACCENT));
                }
            }
            for (i, p) in screen.iter().enumerate() {
                let Some(p) = p else { continue };
                let held = self.route_drag == Some(i);
                painter.circle_filled(*p, if held { 7.0 } else { 5.0 }, theme::ACCENT);
                painter.circle_stroke(*p, 9.0, Stroke::new(1.0, theme::CONTOUR_300));
                painter.text(
                    *p + Vec2::new(11.0, -3.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{}", i + 1),
                    theme::font(FontRole::Mono, 10.0),
                    theme::CONTOUR_300,
                );
            }
        }

        if self.view_mode != ViewMode::Metal {
            return;
        }
        let radius = self.project.extractor_radius;
        for (idx, s) in self.spots.iter().enumerate() {
            let selected = self.selected_spot == Some(idx);
            // A spot the engine will not let you build on is drawn in alert,
            // so the problem is visible in the view rather than only in a list.
            let bad = self
                .spot_build
                .iter()
                .any(|b| b.spot_id == s.id && !b.buildable);
            let ink = if bad {
                theme::ALERT_300
            } else if selected {
                theme::ACCENT
            } else {
                theme::CONTOUR_400
            };
            // The extractor radius ring, following the ground.
            let mut ring: Vec<Pos2> = Vec::with_capacity(28);
            for i in 0..=28 {
                let a = i as f64 / 28.0 * std::f64::consts::TAU;
                let x = (s.x + springen_core::fdlibm::cos(a) * radius).clamp(0.0, w);
                let z = (s.z + springen_core::fdlibm::sin(a) * radius).clamp(0.0, h_world);
                if let Some(p) =
                    self.camera
                        .project([x as f32, height_at(x, z), z as f32], rect, far)
                {
                    ring.push(p);
                }
            }
            for seg in ring.windows(2) {
                painter.line_segment(
                    [seg[0], seg[1]],
                    Stroke::new(if selected { 2.0 } else { 1.0 }, ink),
                );
            }
            let y = height_at(s.x, s.z);
            let Some(base) = self.camera.project([s.x as f32, y, s.z as f32], rect, far) else {
                continue;
            };
            // A short stalk so a spot in a valley is still findable.
            if let Some(top) =
                self.camera
                    .project([s.x as f32, y + range * 0.06, s.z as f32], rect, far)
            {
                painter.line_segment([base, top], Stroke::new(1.0, ink));
                painter.circle_filled(top, if selected { 5.0 } else { 3.0 }, ink);
                if selected {
                    painter.circle_stroke(top, 8.0, Stroke::new(1.0, theme::CONTOUR_300));
                }
                painter.text(
                    top + Vec2::new(6.0, -2.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{:.1}", s.metal),
                    theme::font(FontRole::Mono, 10.0),
                    theme::CONTOUR_300,
                );
            }
        }
    }

    /* ---------------------------------------------------------- inspector */

    /* ------------------------------------------------------------- panes */

    /// Draw one pane's contents. Every pane is one arm; nothing here knows
    /// where the pane is sitting.
    fn pane_body(&mut self, pane: Pane, ui: &mut egui::Ui, d: &springen_core::Derived) -> bool {
        match pane {
            Pane::Project => self.inspector_project(ui),
            Pane::Node => self.inspector_node(ui),
            Pane::Viewport => self.inspector_viewport(ui),
            Pane::Measure => {
                self.inspector_flatness(ui);
                false
            }
            Pane::Manifest => {
                self.inspector_manifest(ui, d);
                false
            }
            Pane::Metal => self.inspector_metal(ui),
            Pane::StartBoxes => self.inspector_startboxes(ui),
            Pane::Materials => self.inspector_materials(ui),
            Pane::Environment => self.inspector_environment(ui),
        }
    }

    /// A pane's header: its name, and the controls that move it.
    ///
    /// Clicking the name collapses the pane. The buttons on the right send it
    /// to the other rail, set it loose, or close it; the arrows move it within
    /// its own rail. All of it is explicit rather than drag-and-drop, because
    /// a layout you rearranged by accident is worse than one that takes two
    /// clicks.
    fn pane_header(&mut self, ui: &mut egui::Ui, pane: Pane, docked: bool) {
        let st = self.layout.get(pane);
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            if docked {
                let chevron = if st.collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                };
                if icon_button(ui, chevron).clicked() {
                    self.layout.set_collapsed(pane, !st.collapsed);
                }
            }
            let title = ui.add(
                egui::Label::new(
                    RichText::new(pane.title())
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_PRIMARY),
                )
                .sense(Sense::click()),
            );
            if title.clicked() && docked {
                self.layout.set_collapsed(pane, !st.collapsed);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, Icon::Close).clicked() {
                    self.layout.set_open(pane, false);
                }
                match st.dock {
                    Dock::Float => {
                        if icon_button(ui, Icon::DockLeft).clicked() {
                            self.layout.move_to(pane, Dock::Left);
                        }
                        if icon_button(ui, Icon::DockRight).clicked() {
                            self.layout.move_to(pane, Dock::Right);
                        }
                    }
                    _ => {
                        if icon_button(ui, Icon::Float).clicked() {
                            self.layout.move_to(pane, Dock::Float);
                        }
                        let (other, mark) = if st.dock == Dock::Left {
                            (Dock::Right, Icon::DockRight)
                        } else {
                            (Dock::Left, Icon::DockLeft)
                        };
                        if icon_button(ui, mark).clicked() {
                            self.layout.move_to(pane, other);
                        }
                        if icon_button(ui, Icon::Down).clicked() {
                            self.layout.shift(pane, 1);
                        }
                        if icon_button(ui, Icon::Up).clicked() {
                            self.layout.shift(pane, -1);
                        }
                    }
                }
            });
        });
    }

    /// The two rails and every floating pane.
    fn rails(&mut self, root: &mut egui::Ui, d: &springen_core::Derived, dirty: &mut bool) {
        for (dock, id) in [(Dock::Left, "rail-left"), (Dock::Right, "rail-right")] {
            let panes = self.layout.rail(dock);
            if panes.is_empty() {
                continue;
            }
            let width = if dock == Dock::Left {
                self.layout.left_w
            } else {
                self.layout.right_w
            };
            let frame = egui::Frame::new()
                .fill(theme::SURFACE_PANEL)
                .inner_margin(egui::Margin::same(theme::PANEL_PAD as i8))
                .stroke(Stroke::new(1.0, theme::BORDER_HAIRLINE));
            let build = |ui: &mut egui::Ui, me: &mut Self, dirty: &mut bool| {
                egui::ScrollArea::vertical().id_salt(id).show(ui, |ui| {
                    for (i, pane) in panes.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                            let (r, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 1.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(r, 0.0, theme::BORDER_HAIRLINE);
                            ui.add_space(6.0);
                        }
                        me.pane_header(ui, *pane, true);
                        if !me.layout.get(*pane).collapsed {
                            *dirty |= me.pane_body(*pane, ui, d);
                        }
                    }
                });
            };
            let resp = if dock == Dock::Left {
                egui::Panel::left(id)
                    .default_size(width)
                    .min_size(180.0)
                    .max_size(640.0)
                    .resizable(true)
                    .frame(frame)
                    .show(root, |ui| build(ui, self, dirty))
            } else {
                egui::Panel::right(id)
                    .default_size(width)
                    .min_size(180.0)
                    .max_size(640.0)
                    .resizable(true)
                    .frame(frame)
                    .show(root, |ui| build(ui, self, dirty))
            };
            // Remember the width the user dragged to.
            let w = resp.response.rect.width();
            if dock == Dock::Left {
                self.layout.left_w = w;
            } else {
                self.layout.right_w = w;
            }
        }

        let ctx = root.ctx().clone();
        for pane in self.layout.floating() {
            let st = self.layout.get(pane);
            let mut open = true;
            let area = egui::Window::new(pane.title())
                .id(egui::Id::new(("float", pane.key())))
                .open(&mut open)
                .title_bar(false)
                .resizable(true)
                .default_pos(st.pos)
                .default_size(st.size)
                // Flat, square, hairline-separated. A floating pane is still a
                // panel, and the design system reserves shadow and radius for
                // popovers and dialogs — which is also the difference between
                // a tool window and a card.
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE_PANEL)
                        .inner_margin(egui::Margin::same(theme::PANEL_PAD as i8))
                        .stroke(Stroke::new(1.0, theme::BORDER_PANEL))
                        .corner_radius(egui::CornerRadius::ZERO)
                        .shadow(egui::epaint::Shadow::NONE),
                )
                .show(&ctx, |ui| {
                    self.pane_header(ui, pane, false);
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt(pane.key())
                        .show(ui, |ui| {
                            *dirty |= self.pane_body(pane, ui, d);
                        });
                });
            if !open {
                self.layout.set_open(pane, false);
            }
            if let Some(a) = area {
                let r = a.response.rect;
                self.layout.set_float_rect(pane, r.min, r.size());
            }
        }
    }

    /// Name, version, author and destination.
    ///
    /// The name and version are what Zero-K identifies a map by, so both have
    /// to be editable and the resulting file name has to be visible before a
    /// bake rather than discovered afterwards.
    fn inspector_project(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let mut text_row = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.label(
                    RichText::new(label)
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let w = ui.available_width().min(196.0);
                    if ui
                        .add_sized(
                            [w, theme::CTL_H],
                            egui::TextEdit::singleline(value)
                                .hint_text(hint)
                                .font(theme::font(FontRole::Ui, 12.0)),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
            });
        };
        text_row(ui, "Name", &mut self.project.name, "Untitled Map");
        text_row(ui, "Version", &mut self.project.version, "1.0");
        text_row(ui, "Author", &mut self.project.author, "");

        ui.add_space(6.0);
        theme::micro_label(ui, "Vertical scale");
        // Relief and waterline rather than raw minHeight/maxHeight: the two
        // numbers people actually think in are "how tall is this map" and "how
        // much of it is sea", and the engine's pair is derived from them.
        // Height 0 elmos is the waterline, so they are not independent.
        let mut relief = self.project.height_range();
        let mut submerged = water_level_t(self.project.min_height, self.project.max_height);
        let mut scale_changed = false;
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Relief")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::DragValue::new(&mut relief)
                            .range(40.0..=4000.0)
                            .speed(4.0)
                            .max_decimals(0)
                            .suffix(" elmos"),
                    )
                    .changed()
                {
                    scale_changed = true;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Submerged")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut pct = submerged * 100.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut pct)
                            .range(0.0..=95.0)
                            .speed(0.25)
                            .max_decimals(1)
                            .suffix("%"),
                    )
                    .changed()
                {
                    submerged = pct / 100.0;
                    scale_changed = true;
                }
            });
        });
        if scale_changed {
            let (mn, mx) = springen_core::project::height_range_for(submerged, relief);
            self.project.min_height = mn;
            self.project.max_height = mx;
            self.terrain_sig.clear();
            changed = true;
        }
        // Depth is not the same lever as Submerged. Submerged moves the
        // waterline and therefore every shore; this lifts the sea floor and
        // leaves the coast alone, which is what you want when the water is
        // simply too deep to walk through.
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Water depth")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut d = self.project.max_depth.unwrap_or(0.0);
                if ui
                    .add(
                        egui::DragValue::new(&mut d)
                            .range(0.0..=2000.0)
                            .speed(1.0)
                            .max_decimals(0)
                            .suffix(" elmos"),
                    )
                    .changed()
                {
                    self.project.max_depth = if d > 0.0 { Some(d) } else { None };
                    self.terrain_sig.clear();
                    changed = true;
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "{} .. {} elmos",
                self.project.min_height, self.project.max_height
            ))
            .font(theme::font(FontRole::Mono, 11.0))
            .color(theme::TEXT_DATA),
        );

        ui.add_space(6.0);
        theme::micro_label(ui, "Output folder");
        let mut dir = self.out_dir.display().to_string();
        if ui
            .add_sized(
                [ui.available_width(), theme::CTL_H],
                egui::TextEdit::singleline(&mut dir).font(theme::font(FontRole::Mono, 11.0)),
            )
            .changed()
        {
            self.out_dir = PathBuf::from(dir);
        }
        // `--smoke` names the exact file it wants; everything else follows the
        // name-and-version convention.
        let target = self.smoke.clone().unwrap_or_else(|| {
            self.out_dir
                .join(format!("{}.sd7", self.project.archive_stem()))
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new(target.display().to_string())
                .font(theme::font(FontRole::Mono, 11.0))
                .color(theme::TEXT_DATA),
        );
        if target.exists() {
            issue_row(ui, Level::Warn, "File exists — baking replaces it");
        }
        // The one graph mistake that bakes successfully and ships a map that
        // is a level plane from corner to corner.
        if self.graph.find_wired_terminal("height").is_none() {
            issue_row(
                ui,
                Level::Error,
                if self.graph.find_terminal("height").is_some() {
                    "Heightmap out is unconnected"
                } else {
                    "No Heightmap out node"
                },
            );
        }
        changed
    }

    /// A 28px swatch of a material, generated once and cached.
    fn mat_thumb(&mut self, ctx: &egui::Context, key: &str) -> egui::TextureHandle {
        if let Some(t) = self.mat_thumbs.get(key) {
            return t.clone();
        }
        const R: usize = 28;
        let m =
            springen_core::material::find(key).unwrap_or(&springen_core::material::MATERIALS[0]);
        let px: Vec<Color32> = springen_core::material::thumbnail(m, R, 11.0)
            .chunks(3)
            .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
            .collect();
        let tex = ctx.load_texture(
            format!("mat-{key}"),
            egui::ColorImage {
                size: [R, R],
                pixels: px,
                source_size: Vec2::splat(R as f32),
            },
            egui::TextureOptions::LINEAR,
        );
        self.mat_thumbs.insert(key.to_string(), tex.clone());
        tex
    }

    /// Four cells of ground at engine scale, one per splat channel, each with
    /// the detail tile added the way the engine adds it.
    ///
    /// No map-wide view can show a detail tile — it repeats every 50 elmos, so
    /// a pixel of the Diffuse view covers many copies of it and correctly
    /// averages it to nothing. That left the one surface covering every texel
    /// of a finished map with nowhere in the tool to be looked at, and it is
    /// how seven procedural tiles once shipped on maps that were supposed to
    /// have been retextured. This is the view that shows it.
    fn ground_strip(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        const CELL: usize = 56;
        // A little over a hundred elmos across a cell: two repeats of the
        // detail tile, its texels still bigger than a pixel.
        const ELMOS: f64 = 120.0;
        let mats = self.materials.as_ref()?;
        let sig = format!("{}|{:.3}", self.materials_sig, self.project.materials.blend);
        if let Some((have, tex)) = &self.ground_strip {
            if *have == sig {
                return Some(tex.clone());
            }
        }
        let mut px = vec![Color32::BLACK; CELL * 4 * CELL];
        for ch in 0..4 {
            let mut w = [0.0; 4];
            w[ch] = 1.0;
            let cell = springen_core::material::ground_sample(
                mats,
                springen_core::material::DEFAULT_TEX_SCALES,
                1.0,
                w,
                [0.5, 0.5, 0.5],
                CELL,
                ELMOS,
            );
            for y in 0..CELL {
                for x in 0..CELL {
                    let o = (y * CELL + x) * 3;
                    px[y * (CELL * 4) + ch * CELL + x] =
                        Color32::from_rgb(cell[o], cell[o + 1], cell[o + 2]);
                }
            }
        }
        let tex = ctx.load_texture(
            "ground-strip",
            egui::ColorImage {
                size: [CELL * 4, CELL],
                pixels: px,
                source_size: Vec2::new((CELL * 4) as f32, CELL as f32),
            },
            egui::TextureOptions::NEAREST,
        );
        self.ground_strip = Some((sig, tex.clone()));
        Some(tex)
    }

    /// Sun, sky and water. The viewport reads the same numbers, so what you
    /// light the terrain with here is what the engine lights it with.
    fn inspector_environment(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Preset")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let current = self.project.environment.preset.clone();
                egui::ComboBox::from_id_salt("envpreset")
                    .selected_text(RichText::new(&current).font(theme::font(FontRole::Ui, 12.0)))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for (k, about) in springen_core::env::PRESETS {
                            if ui
                                .selectable_label(current == *k, *k)
                                .on_hover_text(*about)
                                .clicked()
                            {
                                if let Some(e) = springen_core::env::preset(k) {
                                    self.project.environment = e;
                                    changed = true;
                                }
                            }
                        }
                    });
            });
        });
        for (label, value, range, suffix) in [
            (
                "Sun bearing",
                &mut self.project.environment.sun_azimuth,
                0.0..=360.0,
                "°",
            ),
            (
                "Sun elevation",
                &mut self.project.environment.sun_elevation,
                1.0..=89.0,
                "°",
            ),
        ] {
            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.label(
                    RichText::new(label)
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut v = *value;
                    if ui
                        .add(
                            egui::DragValue::new(&mut v)
                                .range(range)
                                .speed(0.5)
                                .max_decimals(0)
                                .suffix(suffix),
                        )
                        .changed()
                    {
                        *value = v;
                        changed = true;
                    }
                });
            });
        }
        if changed {
            // A hand-turned dial is no longer the stock preset, and saying so
            // is better than a preset name that means nothing.
            let stock = springen_core::env::preset(&self.project.environment.preset);
            if stock.as_ref() != Some(&self.project.environment)
                && !self.project.environment.preset.ends_with(" (edited)")
            {
                self.project.environment.preset =
                    format!("{} (edited)", self.project.environment.preset);
            }
            self.terrain_sig.clear();
        }
        changed
    }

    /// Which surface goes in each splat channel, and how hard the materials
    /// colour the ground.
    ///
    /// The channels are the RGBA of the splat distribution the graph paints,
    /// so what a channel *means* is whatever the graph wired into it -- the
    /// starters use R for steep rock, G for slope and B for height.
    fn inspector_materials(&mut self, ui: &mut egui::Ui) -> bool {
        theme::micro_label(ui, "Channels");
        let mut changed = false;
        let ctx = ui.ctx().clone();
        let keys: Vec<&'static str> = springen_core::material::keys();
        for (i, label) in ["R channel", "G channel", "B channel", "A channel"]
            .iter()
            .enumerate()
        {
            let current = self.project.materials.splat[i].clone();
            let thumb = self.mat_thumb(&ctx, &current);
            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.add(egui::Image::new(&thumb).fit_to_exact_size(Vec2::splat(16.0)));
                ui.label(
                    RichText::new(*label)
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt(format!("mat{i}"))
                        .selected_text(
                            RichText::new(&current).font(theme::font(FontRole::Ui, 12.0)),
                        )
                        .width(132.0)
                        .show_ui(ui, |ui| {
                            for k in &keys {
                                let m = springen_core::material::find(k).unwrap();
                                if ui
                                    .selectable_label(current == *k, m.label)
                                    .on_hover_text(material_hover(m))
                                    .clicked()
                                {
                                    self.project.materials.splat[i] = (*k).to_string();
                                    changed = true;
                                }
                            }
                        });
                });
            });
        }

        let detail = self.project.materials.detail.clone();
        let thumb = self.mat_thumb(&ctx, &detail);
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.add(egui::Image::new(&thumb).fit_to_exact_size(Vec2::splat(16.0)));
            ui.label(
                RichText::new("Detail tile")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("matdetail")
                    .selected_text(RichText::new(&detail).font(theme::font(FontRole::Ui, 12.0)))
                    .width(132.0)
                    .show_ui(ui, |ui| {
                        for k in &keys {
                            let m = springen_core::material::find(k).unwrap();
                            if ui
                                .selectable_label(detail == *k, m.label)
                                .on_hover_text(material_hover(m))
                                .clicked()
                            {
                                self.project.materials.detail = (*k).to_string();
                                changed = true;
                            }
                        }
                    });
            });
        });

        if let Some(strip) = self.ground_strip(&ctx) {
            ui.add_space(4.0);
            theme::micro_label(ui, "Underfoot");
            let w = ui.available_width();
            ui.add(
                egui::Image::new(&strip)
                    .fit_to_exact_size(Vec2::new(w, w / 4.0))
                    .corner_radius(3.0),
            );
            ui.add_space(4.0);
        }

        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Blend into ground")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut b = self.project.materials.blend;
                if ui
                    .add(egui::Slider::new(&mut b, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    self.project.materials.blend = b;
                    changed = true;
                }
                ui.label(
                    RichText::new(format!("{:.0}%", self.project.materials.blend * 100.0))
                        .font(theme::font(FontRole::Mono, 11.0))
                        .color(theme::TEXT_DATA),
                );
            });
        });
        changed
    }

    fn inspector_node(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(id) = self.view.selected.clone() else {
            return false;
        };
        let Some(node) = self.graph.node(&id) else {
            return false;
        };
        let type_name = node.type_name.clone();
        let Some(spec) = registry().get(&type_name) else {
            return false;
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(spec.label)
                    .font(theme::font(FontRole::UiStrong, 13.0))
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(&id)
                        .font(theme::font(FontRole::Mono, 11.0))
                        .color(theme::TEXT_TERTIARY),
                );
            });
        });
        ui.add_space(4.0);
        theme::hairline(ui);
        ui.add_space(6.0);

        let mut changed = false;
        let params: Vec<_> = spec.params.clone();
        for p in &params {
            let Some(node) = self.graph.node_mut(&id) else {
                break;
            };
            let cur = node.params.0.get(p.key).cloned().unwrap_or(p.def.clone());
            // A list does not fit the label-and-control row the rest use, and
            // it is the one parameter you edit in the viewport rather than
            // here, so it gets its own block.
            if p.ptype == PType::Strokes {
                let strokes = cur.as_strokes().to_vec();
                ui.horizontal(|ui| {
                    ui.set_min_height(theme::ROW_H);
                    ui.label(
                        RichText::new(p.label)
                            .font(theme::font(FontRole::Ui, 12.0))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !strokes.is_empty() && ghost_button(ui, "Clear").clicked() {
                            node.params.set(p.key, PVal::Strokes(Vec::new()));
                            changed = true;
                        }
                        if !strokes.is_empty() && ghost_button(ui, "Undo one").clicked() {
                            let mut next = strokes.clone();
                            next.pop();
                            node.params.set(p.key, PVal::Strokes(next));
                            changed = true;
                        }
                        ui.label(
                            RichText::new(format!("{} strokes", strokes.len()))
                                .font(theme::font(FontRole::Mono, 11.0))
                                .color(theme::TEXT_DATA),
                        );
                    });
                });
                // A levelling stroke claims something about the ground it was
                // drawn on. If that ground has moved, say so — silently
                // levelling a hilltop to the altitude of a valley that used to
                // be there is the failure mode this whole `seat` field exists
                // to catch.
                if let Some(field) = &self.terrain_field {
                    let ctx = Context::new(&self.project, field.res);
                    let drift = springen_core::nodes::stroke_drift(&strokes, field, &ctx);
                    if drift > 24.0 {
                        ui.label(
                            RichText::new(
                                format!("Level strokes {drift:.0} elmos off their seat",),
                            )
                            .font(theme::font(FontRole::Ui, 11.0))
                            .color(theme::WARN_500),
                        );
                    }
                }
                self.brush_controls(ui);
                continue;
            }
            if p.ptype == PType::Points {
                let mut pts = cur.as_points().to_vec();
                let mut edited = false;
                ui.horizontal(|ui| {
                    ui.set_min_height(theme::ROW_H);
                    ui.label(
                        RichText::new(p.label)
                            .font(theme::font(FontRole::Ui, 12.0))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ghost_button(ui, "Add").clicked() {
                            // At the camera's target, which is what you are
                            // looking at when you press it -- then drag it.
                            let c = Context::new(&self.project, 129);
                            pts.push([
                                f64::from(self.camera.target[0]).clamp(0.0, c.elmos_x),
                                f64::from(self.camera.target[2]).clamp(0.0, c.elmos_y),
                            ]);
                            edited = true;
                        }
                        ui.label(
                            RichText::new(format!("{}", pts.len()))
                                .font(theme::font(FontRole::Mono, 11.0))
                                .color(theme::TEXT_DATA),
                        );
                    });
                });
                let mut remove: Option<usize> = None;
                for i in 0..pts.len() {
                    ui.horizontal(|ui| {
                        ui.set_min_height(theme::ROW_H);
                        let lim = Context::new(&self.project, 129);
                        for (k, max) in [(0usize, lim.elmos_x), (1, lim.elmos_y)] {
                            let mut v = pts[i][k];
                            if ui
                                .add(
                                    egui::DragValue::new(&mut v)
                                        .range(0.0..=max)
                                        .speed(8.0)
                                        .max_decimals(0),
                                )
                                .changed()
                            {
                                pts[i][k] = v;
                                edited = true;
                            }
                        }
                        if ghost_button(ui, "×").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    pts.remove(i);
                    edited = true;
                }
                if pts.len() < 2 {
                    issue_row(ui, Level::Info, "Needs two waypoints");
                }
                if edited {
                    if let Some(node) = self.graph.node_mut(&id) {
                        node.params.set(p.key, PVal::Points(pts));
                    }
                    changed = true;
                }
                continue;
            }
            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.label(
                    RichText::new(p.label)
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match p.ptype {
                        PType::Bool => {
                            let mut b = cur.as_bool();
                            if ui.checkbox(&mut b, "").changed() {
                                node.params.set(p.key, PVal::Bool(b));
                                changed = true;
                            }
                        }
                        PType::Enum => {
                            let mut sel = cur.as_str().to_string();
                            egui::ComboBox::from_id_salt(format!("{id}-{}", p.key))
                                .selected_text(
                                    RichText::new(&sel).font(theme::font(FontRole::Ui, 12.0)),
                                )
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for opt in p.options {
                                        if ui.selectable_label(sel == *opt, *opt).clicked() {
                                            sel = (*opt).to_string();
                                            node.params.set(p.key, PVal::Str(sel.clone()));
                                            changed = true;
                                        }
                                    }
                                });
                        }
                        // Handled above, out of the row layout.
                        PType::Points => {}
                        // Painted in the viewport, listed above; there is no
                        // sensible way to type a brush stroke into a row.
                        PType::Strokes => {}
                        PType::Text => {
                            let mut s = cur.as_str().to_string();
                            if ui
                                .add(egui::TextEdit::singleline(&mut s).desired_width(120.0))
                                .changed()
                            {
                                node.params.set(p.key, PVal::Str(s));
                                changed = true;
                            }
                        }
                        PType::Color => {
                            let mut s = cur.as_str().to_string();
                            let c = springen_core::field::hex_to_rgb(&s);
                            let mut rgb = [c[0] as f32, c[1] as f32, c[2] as f32];
                            if ui.color_edit_button_rgb(&mut rgb).changed() {
                                s = springen_core::field::rgb_to_hex(
                                    f64::from(rgb[0]),
                                    f64::from(rgb[1]),
                                    f64::from(rgb[2]),
                                );
                                node.params.set(p.key, PVal::Str(s));
                                changed = true;
                            }
                        }
                        PType::Int => {
                            let mut v = cur.as_f64() as i64;
                            let mut dv = egui::DragValue::new(&mut v).speed(0.25);
                            if let (Some(lo), Some(hi)) = (p.min, p.max) {
                                dv = dv.range(lo as i64..=hi as i64);
                            }
                            if ui.add(dv).changed() {
                                node.params.set(p.key, PVal::Num(v as f64));
                                changed = true;
                            }
                        }
                        PType::Elmos | PType::Float => {
                            let mut v = cur.as_f64();
                            let elmos = p.ptype == PType::Elmos;
                            let span = p.max.unwrap_or(1.0) - p.min.unwrap_or(0.0);
                            let mut dv = egui::DragValue::new(&mut v)
                                .speed(span / 400.0)
                                .max_decimals(if elmos { 0 } else { 3 });
                            if elmos {
                                // Units are spelled with the value.
                                dv = dv.suffix(" elmos");
                            }
                            if let (Some(lo), Some(hi)) = (p.min, p.max) {
                                dv = dv.range(lo..=hi);
                            }
                            if ui.add(dv).changed() {
                                node.params.set(p.key, PVal::Num(v));
                                changed = true;
                            }
                        }
                    }
                });
            });
        }
        if params.iter().any(|p| p.ptype == PType::Elmos) {
            ui.add_space(4.0);
        }
        changed
    }

    /// Controls that only mean anything while the 3D viewport is showing.
    fn inspector_viewport(&mut self, ui: &mut egui::Ui) -> bool {
        // The view switch lives with the view. It was the widest thing in the
        // toolbar and the first casualty on a narrow window.
        ui.horizontal_wrapped(|ui| {
            for m in ViewMode::ALL {
                if segmented(ui, m.label(), self.view_mode == m).clicked() {
                    self.view_mode = m;
                }
            }
        });
        ui.add_space(6.0);
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Vertical exaggeration")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut e = self.camera.exaggeration;
                if ui
                    .add(
                        egui::DragValue::new(&mut e)
                            .range(0.5..=8.0)
                            .speed(0.02)
                            .max_decimals(2)
                            .suffix(" ×"),
                    )
                    .changed()
                {
                    self.camera.exaggeration = e;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Climb limit")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut c = self.climb_limit;
                if ui
                    .add(
                        egui::DragValue::new(&mut c)
                            .range(2.0..=60.0)
                            .speed(0.2)
                            .max_decimals(0)
                            .suffix("°"),
                    )
                    .changed()
                {
                    self.climb_limit = c;
                    changed = true;
                }
            });
        });
        changed
    }

    fn inspector_manifest(&mut self, ui: &mut egui::Ui, d: &springen_core::Derived) {
        theme::micro_label(ui, "Layers");
        theme::stat_row(
            ui,
            "Heightmap",
            &format!("{} × {} (16-bit)", d.height_w, d.height_h),
            true,
        );
        theme::stat_row(ui, "Diffuse", &format!("{} × {}", d.tex_w, d.tex_h), false);
        theme::stat_row(
            ui,
            "Tile grid",
            &format!("{} × {} ({} tiles)", d.tiles_x, d.tiles_y, d.tile_count),
            false,
        );
        theme::stat_row(
            ui,
            "Metal / type",
            &format!("{} × {}", d.metal_w, d.metal_h),
            false,
        );
        theme::stat_row(
            ui,
            "Grass",
            &format!("{} × {}", d.grass_w, d.grass_h),
            false,
        );
        theme::stat_row(ui, "Minimap", "1024 × 1024", false);
        theme::stat_row(
            ui,
            "SMT worst case",
            &format!("{:.1} MB", d.smt_worst_case as f64 / 1048576.0),
            false,
        );
        // The range the bake will declare, not the one the project authors.
        // Fitting the terrain to the full 16-bit range tightens it, and it is
        // what the engine reads every height back through -- including where
        // it puts the water plane.
        let (dmin, dmax) = match &self.terrain_field {
            Some(f) => {
                let (_, a, b) = springen_core::bake::height_and_range(
                    f,
                    springen_core::bake::HeightMode::Fit,
                    self.project.min_height,
                    self.project.max_height,
                );
                (a, b)
            }
            None => (self.project.min_height, self.project.max_height),
        };
        theme::stat_row(
            ui,
            "Height range",
            &format!("{dmin} .. {dmax} elmos"),
            false,
        );
        let wt = water_level_t(dmin, dmax);
        theme::stat_row(
            ui,
            "Waterline",
            &format!("0 elmos, {:.1}% up it", wt * 100.0),
            false,
        );
        ui.add_space(4.0);
    }

    /// How much of the map will hold a building, measured live.
    ///
    /// Pathability answers a different question, and answering it well is no
    /// comfort on a map where a factory does not fit: hummocks are crossable
    /// everywhere and buildable nowhere. The sweep runs against the preview
    /// field, so it moves while you drag a `flatten` or a `terrace` rather
    /// than waiting for a bake to tell you.
    fn inspector_flatness(&mut self, ui: &mut egui::Ui) {
        theme::micro_label(ui, "Buildable ground");
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Footprint")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut s = self.build_slope;
                if ui
                    .add(
                        egui::DragValue::new(&mut s)
                            .range(2.0..=45.0)
                            .speed(0.2)
                            .max_decimals(0)
                            .suffix("°"),
                    )
                    .changed()
                {
                    self.build_slope = s;
                }
                let mut f = self.build_footprint;
                if ui
                    .add(
                        egui::DragValue::new(&mut f)
                            .range(16.0..=512.0)
                            .speed(1.0)
                            .max_decimals(0)
                            .suffix(" elmos"),
                    )
                    .changed()
                {
                    self.build_footprint = f;
                }
            });
        });

        let sig = format!(
            "{}|{}|{}",
            self.terrain_sig, self.build_footprint, self.build_slope
        );
        if self.flatness_sig != sig {
            if let Some(field) = self.terrain_field.clone() {
                let ctx = Context::new(&self.project, field.res);
                let (_, dmin, dmax) = springen_core::bake::height_and_range(
                    &field,
                    springen_core::bake::HeightMode::Fit,
                    self.project.min_height,
                    self.project.max_height,
                );
                self.flatness = Some(springen_core::analysis::flatness(
                    &field,
                    &ctx,
                    self.build_footprint,
                    self.build_slope,
                    water_level_t(dmin, dmax),
                ));
                self.flatness_sig = sig;
            }
        }

        let Some(f) = &self.flatness else {
            return;
        };
        theme::stat_row(
            ui,
            "Of the land",
            &format!("{:.1}%", f.buildable_of_land * 100.0),
            f.buildable_of_land < 0.25,
        );
        theme::stat_row(
            ui,
            "Largest plain",
            &format!(
                "{:.1}% of the map, {} plains",
                f.largest_plain_fraction * 100.0,
                f.plain_count
            ),
            false,
        );
        theme::stat_row(
            ui,
            "Land slope",
            &format!(
                "{:.1}° median, {:.1}° p90",
                f.median_slope_deg, f.p90_slope_deg
            ),
            false,
        );
        theme::stat_row(ui, "Relief", &format!("{:.0} elmos", f.relief_elmos), false);

        self.inspector_choke(ui);
    }

    /// How wide the ground an army can move through is, and where it pinches.
    ///
    /// Traversable fraction says the halves of a map are joined. It does not
    /// say whether anything fits through the join, and a map is often decided
    /// by exactly that — so the narrowest point of the widest route across is
    /// its own number rather than a footnote to connectivity.
    fn inspector_choke(&mut self, ui: &mut egui::Ui) {
        let sig = format!("{}|{}", self.terrain_sig, self.build_slope);
        if self.choke_sig != sig {
            if let Some(field) = self.terrain_field.clone() {
                let ctx = Context::new(&self.project, field.res);
                let (_, dmin, dmax) = springen_core::bake::height_and_range(
                    &field,
                    springen_core::bake::HeightMode::Fit,
                    self.project.min_height,
                    self.project.max_height,
                );
                self.choke = Some(springen_core::analysis::chokepoints(
                    &field,
                    &ctx,
                    springen_core::analysis::UnitClass::Tank.max_slope_deg(),
                    water_level_t(dmin, dmax),
                ));
                self.choke_sig = sig;
            }
        }
        let Some(c) = &self.choke else {
            return;
        };
        ui.add_space(6.0);
        theme::micro_label(ui, "Corridor width");
        if c.bottleneck <= 0.0 {
            ui.label(
                RichText::new("No route across")
                    .font(theme::font(FontRole::Ui, 11.0))
                    .color(theme::TEXT_TERTIARY),
            );
            return;
        }
        // Judged along the axis the teams are laid out on, not the more
        // constrained one: a map built as a west-east corridor with ranges up
        // the flanks is supposed to pinch north-south, and calling that a
        // funnel would be calling a deliberate design a fault.
        let (play, axis) = c.along_play_axis(&self.project.mex_sym);
        let funnels = c.median > 0.0 && play > 0.0 && play < c.median * 0.35;
        theme::stat_row(
            ui,
            "Narrowest",
            &format!("{:.0} elmos, {}", c.bottleneck, c.axis),
            funnels,
        );
        if (play - c.bottleneck).abs() > 1.0 {
            theme::stat_row(
                ui,
                "Across play",
                &format!("{play:.0} elmos, {axis}"),
                false,
            );
        }
        theme::stat_row(
            ui,
            "Typical",
            &format!("{:.0} elmos median, {:.0} p10", c.median, c.p10),
            false,
        );
    }

    /// Start boxes: what they are, where they are, and how to put them back.
    fn inspector_startboxes(&mut self, ui: &mut egui::Ui) -> bool {
        let d = springen_core::derive(self.project.units_x, self.project.units_y);
        let (w, h) = (f64::from(d.elmos_x), f64::from(d.elmos_y));
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            if ui.selectable_label(self.show_boxes, "Show in 3D").clicked() {
                self.show_boxes = !self.show_boxes;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let edited = self.project.start_boxes.is_some();
                if edited && ghost_button(ui, "Reset").clicked() {
                    // Back to derived, so changing the symmetry starts
                    // deciding the layout again.
                    self.project.start_boxes = None;
                    self.selected_box = None;
                    changed = true;
                }
                ui.label(
                    RichText::new(if edited {
                        "hand-edited"
                    } else {
                        "from symmetry"
                    })
                    .font(theme::font(FontRole::Mono, 11.0))
                    .color(theme::TEXT_DATA),
                );
            });
        });

        let areas = self.start_areas(&d);
        for (i, a) in areas.iter().enumerate() {
            let (x0, z0, x1, z1) = a.bounds();
            let selected = self.selected_box == Some(i);
            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                if ui.selectable_label(selected, &a.long).clicked() {
                    self.selected_box = if selected { None } else { Some(i) };
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{:.0}×{:.0} at {:.0},{:.0}",
                            x1 - x0,
                            z1 - z0,
                            (x0 + x1) * 0.5,
                            (z0 + z1) * 0.5
                        ))
                        .font(theme::font(FontRole::Mono, 11.0))
                        .color(theme::TEXT_DATA),
                    );
                });
            });
        }

        if let Some(i) = self.selected_box.filter(|i| *i < areas.len()) {
            let a = areas[i].clone();
            let (x0, z0, x1, z1) = a.bounds();
            ui.add_space(4.0);

            // Exact bounds, because "somewhere around there" is not a spawn
            // area — it decides who reaches the middle first.
            let grid = springen_core::Zk::METAL_GRID;
            let mut edited: Option<(f64, f64, f64, f64)> = None;
            for (label, lo, hi, limit, horizontal) in
                [("X", x0, x1, w, true), ("Z", z0, z1, h, false)]
            {
                ui.horizontal(|ui| {
                    ui.set_min_height(theme::ROW_H);
                    ui.label(
                        RichText::new(label)
                            .font(theme::font(FontRole::Ui, 12.0))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (mut a1, mut a0) = (hi, lo);
                        let r1 = ui.add(
                            egui::DragValue::new(&mut a1)
                                .range(0.0..=limit)
                                .speed(grid * 0.5)
                                .max_decimals(0),
                        );
                        ui.label(
                            RichText::new("to")
                                .font(theme::font(FontRole::Ui, 11.0))
                                .color(theme::TEXT_TERTIARY),
                        );
                        let r0 = ui.add(
                            egui::DragValue::new(&mut a0)
                                .range(0.0..=limit)
                                .speed(grid * 0.5)
                                .max_decimals(0),
                        );
                        if r0.changed() || r1.changed() {
                            edited = Some(if horizontal {
                                (a0, z0, a1, z1)
                            } else {
                                (x0, a0, x1, a1)
                            });
                        }
                    });
                });
            }
            if let Some((nx0, nz0, nx1, nz1)) = edited {
                let mut next = areas.clone();
                next[i].set_bounds(nx0, nz0, nx1, nz1, w, h);
                if self.mirror_edits {
                    reflow_images(&mut next, i, &self.project.mex_sym, w, h);
                }
                self.project.start_boxes = Some(next);
                changed = true;
            }

            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.label(
                    RichText::new("Shape")
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let now = a.shape();
                    for sh in springen_core::lua::Shape::ALL.iter().rev() {
                        if segmented(ui, sh.label(), now == *sh).clicked() && now != *sh {
                            let mut next = areas.clone();
                            // Every box takes the shape: a layout where one
                            // team has a wedge and the rest have rectangles is
                            // not a symmetric map.
                            for b in next.iter_mut() {
                                b.set_shape(*sh, w, h);
                            }
                            self.project.start_boxes = Some(next);
                            changed = true;
                        }
                    }
                });
            });

            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.label(
                    RichText::new("Size")
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (label, k, all) in [
                        ("+", 1.1, false),
                        ("\u{2212}", 1.0 / 1.1, false),
                        ("+ all", 1.1, true),
                        ("\u{2212} all", 1.0 / 1.1, true),
                    ] {
                        if ghost_button(ui, label).clicked() {
                            let mut next = areas.clone();
                            if all {
                                for b in next.iter_mut() {
                                    b.scale(k, w, h);
                                }
                            } else {
                                next[i].scale(k, w, h);
                            }
                            self.project.start_boxes = Some(next);
                            changed = true;
                        }
                    }
                });
            });

            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                if ghost_button(ui, "Duplicate").clicked() {
                    let mut next = areas.clone();
                    let mut copy = next[i].clone();
                    copy.short = format!("{}{}", copy.short, next.len() + 1);
                    copy.long = format!("{} {}", copy.long, next.len() + 1);
                    copy.translate(w * 0.06, h * 0.06, w, h);
                    next.push(copy);
                    self.project.start_boxes = Some(next);
                    self.selected_box = Some(areas.len());
                    changed = true;
                }
                if areas.len() > 1 && ghost_button(ui, "Remove").clicked() {
                    let mut next = areas.clone();
                    next.remove(i);
                    self.project.start_boxes = Some(next);
                    self.selected_box = None;
                    changed = true;
                }
            });
        }

        changed
    }

    fn inspector_metal(&mut self, ui: &mut egui::Ui) -> bool {
        theme::micro_label(ui, "Spots");
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Target count")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut n = self.spot_count as i64;
                if ui
                    .add(egui::DragValue::new(&mut n).range(2..=64).speed(0.2))
                    .changed()
                {
                    self.spot_count = n as usize;
                    changed = true;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Minimum separation")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut s = self.min_separation;
                if ui
                    .add(
                        egui::DragValue::new(&mut s)
                            .range(200.0..=4000.0)
                            .speed(4.0)
                            .max_decimals(0)
                            .suffix(" elmos"),
                    )
                    .changed()
                {
                    self.min_separation = s;
                    changed = true;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Buildable up to")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut deg = self.max_spot_slope;
                if ui
                    .add(
                        egui::DragValue::new(&mut deg)
                            .range(1.0..=45.0)
                            .speed(0.2)
                            .max_decimals(0)
                            .suffix("°"),
                    )
                    .changed()
                {
                    self.max_spot_slope = deg;
                    changed = true;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Symmetry")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut sel = self.project.mex_sym.clone();
                egui::ComboBox::from_id_salt("mexsym")
                    .selected_text(RichText::new(&sel).font(theme::font(FontRole::Ui, 12.0)))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        // Every operator `zk::symmetry_images` knows. rot72
                        // was missing, so a five-way FFA map could not be
                        // given five-fold metal at all.
                        for opt in [
                            "none", "mirrorX", "mirrorY", "quad", "rot180", "rot90", "rot72",
                            "diagonal",
                        ] {
                            if ui.selectable_label(sel == opt, opt).clicked() {
                                sel = opt.to_string();
                                self.project.mex_sym = sel.clone();
                                changed = true;
                            }
                        }
                    });
            });
        });
        ui.add_space(6.0);

        let ctx = Context::new(&self.project, 129);
        let v = zk::validate_spots(&self.spots, &ctx, self.project.extractor_radius);
        let sym = zk::symmetry_report(&self.spots, &ctx, &self.project.mex_sym);
        theme::stat_row(ui, "Proposed", &self.spots.len().to_string(), true);
        theme::stat_row(
            ui,
            "Metal per player",
            &format!("{:.1}", zk::metal_per_player(&self.spots, 2)),
            false,
        );
        theme::stat_row(
            ui,
            "Merge distance",
            &format!("{:.0} elmos", v.merge_certain_dist),
            false,
        );

        ui.add_space(4.0);
        if self.spots.len() < self.spot_count {
            issue_row(
                ui,
                Level::Warn,
                &format!(
                    "{} of {} — spacing too tight",
                    self.spots.len(),
                    self.spot_count
                ),
            );
        }
        for i in &v.issues {
            let level = match i.level {
                zk::Level::Error => Level::Error,
                zk::Level::Warn => Level::Warn,
                zk::Level::Info => Level::Info,
            };
            issue_row(ui, level, &i.text);
        }
        // Buildability, the check the app used to leave entirely to the CLI:
        // the panel could read green while the bake was refusing the same
        // layout. Placement now avoids these, so anything here is ground the
        // search could not get out of.
        let unbuildable: Vec<&zk::BuildabilityReport> =
            self.spot_build.iter().filter(|b| !b.buildable).collect();
        for b in &unbuildable {
            issue_row(
                ui,
                Level::Error,
                &if b.underwater {
                    format!("{} is under water", b.spot_id)
                } else {
                    format!(
                        "{} on {:.0}° ground, limit {:.0}°",
                        b.spot_id, b.max_slope_deg, self.max_spot_slope
                    )
                },
            );
        }
        if let Some(reason) = zk::symmetry_rejection(&self.project.mex_sym, &ctx) {
            issue_row(ui, Level::Error, &reason);
        }
        if !sym.symmetric {
            issue_row(
                ui,
                Level::Error,
                &format!(
                    "{} spot(s) have no partner under {}",
                    sym.unmatched.len(),
                    self.project.mex_sym
                ),
            );
        }
        if v.issues.is_empty() && sym.symmetric && unbuildable.is_empty() && !self.spots.is_empty()
        {
            issue_row(ui, Level::Ok, "All checks pass");
        }

        ui.add_space(8.0);
        changed |= self.inspector_spot_editor(ui);

        if let Some(r) = &self.last_report {
            ui.add_space(10.0);
            theme::micro_label(ui, "Last bake");
            theme::stat_row(
                ui,
                "Tiles stored",
                &format!("{} of {}", r.tiles_stored, r.tile_slots),
                false,
            );
            theme::stat_row(
                ui,
                "SMF",
                &format!("{:.1} MB", r.smf_bytes as f64 / 1048576.0),
                false,
            );
            theme::stat_row(
                ui,
                "SMT",
                &format!("{:.1} MB", r.smt_bytes as f64 / 1048576.0),
                false,
            );
            for (name, passable, _largest, regions) in &r.pathability {
                theme::stat_row(
                    ui,
                    name,
                    &format!("{:.1}% passable, {regions} regions", passable * 100.0),
                    false,
                );
            }
        }
        changed
    }

    /// Move a mex by hand, and set what it is worth.
    ///
    /// The generator's answer is a starting point, not a verdict: a spot that
    /// wants to be on that ridge rather than beside it is a judgement no mask
    /// makes. Touching anything here adopts the layout into the project, so it
    /// travels in the file and the generator stops overwriting it.
    fn inspector_spot_editor(&mut self, ui: &mut egui::Ui) -> bool {
        let hand = !self.project.spots.is_empty();
        let mut changed = false;
        ui.horizontal(|ui| {
            theme::micro_label(
                ui,
                if hand {
                    "Spots — by hand"
                } else {
                    "Spots — generated"
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if hand && ghost_button(ui, "Re-propose").clicked() {
                    self.repropose_from_graph();
                    changed = true;
                }
            });
        });

        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            if ui.checkbox(&mut self.mirror_edits, "").changed() {
                changed = true;
            }
            ui.label(
                RichText::new("Mirror edits")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ghost_button(ui, "Add").clicked() {
                    let moved = self.add_spot_at(None);
                    self.spot_note = moved;
                    changed = true;
                }
            });
        });

        if let Some(note) = &self.spot_note {
            ui.label(
                RichText::new(note)
                    .font(theme::font(FontRole::Ui, 11.0))
                    .color(theme::WARN_500),
            );
        }

        let Some(i) = self.selected_spot.filter(|i| *i < self.spots.len()) else {
            return changed;
        };

        let c = Context::new(&self.project, 129);
        let (w, h) = (c.elmos_x, c.elmos_y);
        let spot = self.spots[i].clone();
        let group = zk::group_of(
            &self.spots,
            i,
            &self.project.mex_sym,
            w,
            h,
            self.mirror_edits,
        );

        ui.add_space(2.0);
        theme::stat_row(
            ui,
            "Selected",
            &if group.len() > 1 {
                format!("{} (+{} mirrored)", spot.id, group.len() - 1)
            } else {
                spot.id.clone()
            },
            true,
        );
        // Why a spot is alone when the symmetry says it should not be. Without
        // this the answer looks like a broken tool rather than a point sitting
        // on the operator's fixed set.
        if group.len() == 1 && self.project.mex_sym != "none" {
            let full = zk::group_size(spot.x, spot.z, &self.project.mex_sym, w, h);
            let why = if spot.single {
                "Marked unmirrored"
            } else if full == 1 {
                "On the symmetry axis"
            } else if !self.mirror_edits {
                "Mirror edits off"
            } else {
                "Images missing"
            };
            ui.label(
                RichText::new(format!("Alone. {why}"))
                    .font(theme::font(FontRole::Ui, 11.0))
                    .color(theme::TEXT_TERTIARY),
            );
        }

        // Coordinates are on the 16-elmo metal grid, so they step by a cell.
        let grid = springen_core::Zk::METAL_GRID;
        let mut moved: Option<(f64, f64)> = None;
        for (label, value, limit) in [("X", spot.x, w), ("Z", spot.z, h)] {
            ui.horizontal(|ui| {
                ui.set_min_height(theme::ROW_H);
                ui.label(
                    RichText::new(label)
                        .font(theme::font(FontRole::Ui, 12.0))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut v = value;
                    if ui
                        .add(
                            egui::DragValue::new(&mut v)
                                .range(0.0..=limit)
                                .speed(grid * 0.25)
                                .max_decimals(0)
                                .suffix(" e"),
                        )
                        .changed()
                    {
                        moved = Some(if label == "X" {
                            (v, spot.z)
                        } else {
                            (spot.x, v)
                        });
                    }
                });
            });
        }

        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_H);
            ui.label(
                RichText::new("Metal")
                    .font(theme::font(FontRole::Ui, 12.0))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut m = spot.metal;
                if ui
                    .add(
                        egui::DragValue::new(&mut m)
                            .range(0.0..=16.0)
                            .speed(0.05)
                            .max_decimals(2),
                    )
                    .changed()
                {
                    self.adopt_spots();
                    zk::set_group_metal(
                        &mut self.spots,
                        i,
                        m,
                        &self.project.mex_sym,
                        w,
                        h,
                        self.mirror_edits,
                    );
                    self.commit_spots();
                    changed = true;
                }
            });
        });
        if spot.metal <= springen_core::Zk::MINIMUM_MEX_INCOME {
            issue_row(
                ui,
                Level::Error,
                &format!(
                    "Below {} metal is discarded",
                    springen_core::Zk::MINIMUM_MEX_INCOME
                ),
            );
        }

        if let Some(to) = moved {
            self.adopt_spots();
            zk::move_group(
                &mut self.spots,
                i,
                to,
                &self.project.mex_sym,
                w,
                h,
                self.mirror_edits,
            );
            self.commit_spots();
            changed = true;
        }

        ui.horizontal(|ui| {
            if ghost_button(ui, "Delete").clicked() {
                self.adopt_spots();
                zk::delete_group(
                    &mut self.spots,
                    i,
                    &self.project.mex_sym,
                    w,
                    h,
                    self.mirror_edits,
                );
                self.selected_spot = None;
                self.commit_spots();
                changed = true;
            }
        });
        changed
    }

    /* --------------------------------------------------------- feedback */

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.toasts.retain(|t| now - t.born < 7.0);
        if self.toasts.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("toasts"),
        ));
        let mut y = screen.bottom() - theme::STATUSBAR_H - 12.0;
        for t in self.toasts.iter().rev().take(4) {
            let galley = painter.layout(
                t.text.clone(),
                theme::font(FontRole::Ui, 12.0),
                t.level.colour(),
                420.0,
            );
            let rect = egui::Rect::from_min_size(
                egui::pos2(screen.right() - 460.0, y - galley.size().y - 16.0),
                Vec2::new(448.0, galley.size().y + 16.0),
            );
            painter.rect_filled(rect, theme::R_POPOVER, theme::SURFACE_RAISED);
            painter.rect_stroke(
                rect,
                theme::R_POPOVER,
                Stroke::new(1.0, theme::BORDER_PANEL),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
                0.0,
                t.level.colour(),
            );
            painter.galley(rect.min + Vec2::new(12.0, 8.0), galley, t.level.colour());
            y = rect.top() - 8.0;
        }
        ctx.request_repaint();
    }

    /// The bake veil: a 78% scrim and the one looping animation in the product.
    fn veil(&self, ctx: &egui::Context, elapsed: f64, stage: &str) {
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("veil"),
        ));
        painter.rect_filled(screen, 0.0, theme::SCRIM);
        let bar = egui::Rect::from_center_size(screen.center(), Vec2::new(320.0, 2.0));
        painter.rect_filled(bar, 0.0, theme::GRAY_800);
        // The only looping animation in the product, and it stops under
        // prefers-reduced-motion.
        let reduced = ctx.style_of(egui::Theme::Dark).animation_time <= 0.0;
        let sweep = if reduced {
            0.5
        } else {
            (elapsed * 0.6).fract() as f32
        };
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(bar.left() + sweep * (bar.width() - 80.0), bar.top()),
                Vec2::new(80.0, 2.0),
            ),
            0.0,
            theme::ACCENT,
        );
        painter.text(
            screen.center() - Vec2::new(160.0, 22.0),
            egui::Align2::LEFT_BOTTOM,
            format!("Baking layers — {stage}"),
            theme::font(FontRole::Ui, 13.0),
            theme::TEXT_PRIMARY,
        );
        painter.text(
            screen.center() + Vec2::new(160.0, 18.0),
            egui::Align2::RIGHT_TOP,
            format!("{elapsed:.0} s"),
            theme::font(FontRole::Mono, 12.0),
            theme::TEXT_TERTIARY,
        );
        ctx.request_repaint();
    }
}

/* ----------------------------------------------------------- small parts */

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        theme::font(FontRole::UiStrong, 13.0),
        theme::ON_CONTOUR,
    );
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 24.0, theme::CTL_H_LG),
        Sense::click(),
    );
    let fill = if resp.is_pointer_button_down_on() {
        theme::CONTOUR_600
    } else if resp.hovered() {
        theme::CONTOUR_400
    } else {
        theme::ACCENT
    };
    ui.painter().rect_filled(rect, theme::R_CONTROL, fill);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::ON_CONTOUR,
    );
    resp
}

fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        theme::font(FontRole::Ui, 13.0),
        theme::TEXT_PRIMARY,
    );
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 20.0, theme::CTL_H),
        Sense::click(),
    );
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, theme::R_CONTROL, theme::SURFACE_HOVER);
    }
    ui.painter().rect_stroke(
        rect,
        theme::R_CONTROL,
        Stroke::new(1.0, theme::BORDER_CONTROL),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::TEXT_PRIMARY,
    );
    resp
}

fn segmented(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        theme::font(FontRole::Ui, 12.0),
        theme::TEXT_PRIMARY,
    );
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 20.0, theme::CTL_H_SM),
        Sense::click(),
    );
    ui.painter().rect_filled(
        rect,
        theme::R_CONTROL,
        if active {
            theme::CONTOUR_TINT
        } else if resp.hovered() {
            theme::SURFACE_HOVER
        } else {
            theme::SURFACE_CONTROL
        },
    );
    if active {
        ui.painter().rect_stroke(
            rect,
            theme::R_CONTROL,
            Stroke::new(1.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        if active {
            theme::ACCENT
        } else {
            theme::TEXT_SECONDARY
        },
    );
    resp
}

fn toolbar_rule(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::BORDER_PANEL);
}

fn status_sep(ui: &mut egui::Ui) {
    ui.label(
        RichText::new("·")
            .font(theme::font(FontRole::Mono, 11.0))
            .color(theme::GRAY_700),
    );
}

fn issue_row(ui: &mut egui::Ui, level: Level, text: &str) {
    let colour = level.colour();
    let tint = Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 30);
    let galley = ui.painter().layout(
        text.to_owned(),
        theme::font(FontRole::Ui, 12.0),
        colour,
        ui.available_width() - 20.0,
    );
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), galley.size().y + 10.0),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, theme::R_CONTROL, tint);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
        0.0,
        colour,
    );
    ui.painter()
        .galley(rect.min + Vec2::new(10.0, 5.0), galley, colour);
    ui.add_space(4.0);
}

/// The contour peak: nested contour rings clipped to a square tile, so the
/// outer rings run off the edge. A map fragment, not a badge.
fn contour_mark(ui: &mut egui::Ui, side: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    contour_mark_at(ui.painter(), rect);
}

/// The contour peak: nested contour rings clipped to a square tile, so the
/// outer rings run off the edge. A map fragment, not a badge.
fn contour_mark_at(painter: &egui::Painter, rect: egui::Rect) {
    let side = rect.width();
    let p = painter.with_clip_rect(rect);
    p.rect_filled(rect, theme::R_CONTROL, theme::GRAY_900);
    let c = rect.center() + Vec2::new(side * 0.04, side * 0.06);
    for (i, f) in [1.05f32, 0.82, 0.60, 0.40].iter().enumerate() {
        p.circle_stroke(
            c,
            side * f * 0.5,
            Stroke::new(
                (side / 26.0).max(1.0),
                if i == 0 {
                    theme::GRAY_700
                } else {
                    theme::GRAY_650
                },
            ),
        );
    }
    p.circle_filled(c, side * 0.20 * 0.5 + side * 0.06, theme::ACCENT);
}

/// What a material's tooltip says.
///
/// Photographs carry their source: a map's ground should be traceable to
/// whatever it was taken from, and the licence question is answered at the
/// point someone is choosing one rather than buried in a file they will not
/// read.
fn material_hover(m: &'static springen_core::material::Material) -> String {
    match m.source() {
        Some(s) => format!("{}\n\nPhotographic — {s}", m.about),
        None => m.about.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preload_runs_every_stage_once_and_ends_ready() {
        let mut seen = Vec::new();
        let mut step = BootStep::Fonts;
        for _ in 0..20 {
            seen.push(step);
            let i = step.index();
            if i + 1 < BootStep::ORDER.len() {
                step = BootStep::ORDER[i + 1];
            } else {
                break;
            }
        }
        assert_eq!(seen.len(), BootStep::ORDER.len());
        assert_eq!(*seen.last().unwrap(), BootStep::Done);
        // Every stage says what it is doing; a blank splash explains nothing.
        for s in BootStep::ORDER {
            assert!(!s.label().is_empty());
        }
    }

    #[test]
    fn progress_reaches_exactly_one() {
        let last = BootStep::Done.index() as f32 / (BootStep::ORDER.len() - 1) as f32;
        assert!((last - 1.0).abs() < 1e-6);
        assert_eq!(BootStep::Fonts.index(), 0);
    }

    #[test]
    fn every_view_mode_is_reachable_by_name() {
        for m in ViewMode::ALL {
            let found = ViewMode::ALL
                .iter()
                .find(|o| o.label().eq_ignore_ascii_case(m.label()));
            assert_eq!(found, Some(&m));
        }
    }
}

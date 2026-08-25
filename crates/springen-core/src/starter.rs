//! Starter graphs.
//!
//! Symmetry sits **upstream of erosion** in all three. Applied afterwards it
//! copies one half verbatim over terrain that eroded independently and leaves a
//! hard fold line down the map. The trade-off is that symmetry is then only
//! near-exact; for pixel-exact competitive halves you put it last and deal with
//! the seam yourself.

use crate::env;
use crate::graph::{Graph, PVal};
use crate::material::MaterialSet;
use crate::project::Project;

fn f(v: f64) -> PVal {
    PVal::Num(v)
}
fn s(v: &str) -> PVal {
    PVal::Str(v.to_string())
}

pub const STARTERS: &[(&str, &str)] = &[
    ("ridge", "Ridge and valley"),
    ("islands", "Islands"),
    ("textured", "Eroded ridges"),
    ("mesa", "Desert mesas"),
    ("glacier", "Glacial highland"),
    ("plains", "Open plains"),
    ("open", "Open ground, flanking ranges"),
    ("ffa5", "Five-way free-for-all"),
];

pub fn starter_graph(kind: &str) -> Graph {
    match kind {
        "islands" => islands(),
        "textured" => textured(),
        "mesa" => mesa(),
        "glacier" => glacier(),
        "plains" => plains(),
        "open" => open(),
        "ffa5" => ffa5(),
        _ => ridge(),
    }
}

/* ------------------------------------------------------------ the look */

/// How a starter is painted and what its splat channels mean.
///
/// The channel scheme is the same in every starter, which is the point: a
/// material picked for the R slot behaves the same way whichever sample map
/// you started from.
///
/// - **R** — steep ground. Cliffs and crags.
/// - **G** — flat ground at middle heights. The surface you fight on.
/// - **B** — high ground. Peaks and plateaux.
/// - **A** — the shoreline band, just above the waterline.
struct Look {
    ramp: &'static str,
    /// Where the hypsometric ramp starts and stops, as field values.
    low: f64,
    high: f64,
    /// Slope in degrees at which rock takes over, and where it is total.
    rock_from: f64,
    rock_full: f64,
    rock_color: &'static str,
    detail_feature: f64,
    detail_amount: f64,
    shallow: &'static str,
    deep: &'static str,
    sun: f64,
    ao: f64,
    /// Steepest ground still counted as flat, for the G channel and grass.
    flat_max: f64,
    /// The middle band, as field values.
    mid_low: f64,
    mid_high: f64,
    grass_threshold: f64,
    /// How far above the waterline the shore band reaches, as a field value.
    shore_band: f64,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            ramp: "verdant",
            low: 0.18,
            high: 0.95,
            rock_from: 24.0,
            rock_full: 44.0,
            rock_color: "#6B6259",
            detail_feature: 180.0,
            detail_amount: 0.16,
            shallow: "#2E6E7E",
            deep: "#0C2836",
            sun: 0.42,
            ao: 0.32,
            flat_max: 12.0,
            mid_low: 0.3,
            mid_high: 0.8,
            grass_threshold: 0.45,
            shore_band: 0.06,
        }
    }
}

/// Append every output layer a full bake wants: diffuse, metal, type, grass
/// and the four splat channels.
///
/// One implementation for every starter. Before this, two of the three sample
/// maps had a heightmap and nothing else, so they baked to flat shaded relief
/// with no splat distribution at all — which meant the detail materials had
/// no weights to blend with and did nothing.
//
/// The standard texture chain with the default look, for callers outside this
/// module.
///
/// An imported map needs the full layer set as much as a starter does: without
/// it the bake writes a heightmap with no splat distribution, and the detail
/// materials have nothing to blend with.
pub fn texture_chain_for(g: &mut Graph, terrain: &str, x: f64) {
    texture_chain(g, terrain, &Look::default(), x);
}

fn texture_chain(g: &mut Graph, terrain: &str, look: &Look, x: f64) {
    let ramp = g.add(
        "tex_ramp",
        x,
        250.0,
        &[
            ("ramp", s(look.ramp)),
            ("low", f(look.low)),
            ("high", f(look.high)),
        ],
    );
    g.link(terrain, &ramp, "In");
    let slope = g.add(
        "tex_slope",
        x + 230.0,
        250.0,
        &[
            ("minDeg", f(look.rock_from)),
            ("maxDeg", f(look.rock_full)),
            ("rockColor", s(look.rock_color)),
        ],
    );
    g.link(&ramp, &slope, "Colour");
    g.link(terrain, &slope, "Height");
    let detail = g.add(
        "tex_detail",
        x + 460.0,
        250.0,
        &[
            ("feature", f(look.detail_feature)),
            ("amount", f(look.detail_amount)),
        ],
    );
    g.link(&slope, &detail, "In");
    // -1: follow the map's waterline rather than carrying a copy of it.
    let water = g.add(
        "tex_water",
        x + 690.0,
        250.0,
        &[
            ("sea", f(-1.0)),
            ("shallow", s(look.shallow)),
            ("deep", s(look.deep)),
        ],
    );
    g.link(&detail, &water, "Colour");
    g.link(terrain, &water, "Height");
    let shade = g.add(
        "tex_shade",
        x + 920.0,
        250.0,
        &[("strength", f(look.sun)), ("ao", f(look.ao))],
    );
    g.link(&water, &shade, "Colour");
    g.link(terrain, &shade, "Height");
    let d_out = g.add("out_diffuse", x + 1150.0, 250.0, &[]);
    g.link(&shade, &d_out, "In");

    /* -- the masks every remaining layer is cut from -------------------- */
    let flat = g.add(
        "slopemask",
        x,
        470.0,
        &[
            ("minDeg", f(0.0)),
            ("maxDeg", f(look.flat_max)),
            ("falloff", f(4.0)),
        ],
    );
    g.link(terrain, &flat, "In");
    let steep = g.add(
        "slopemask",
        x,
        810.0,
        &[
            ("minDeg", f(look.rock_from + 2.0)),
            ("maxDeg", f(90.0)),
            ("falloff", f(8.0)),
        ],
    );
    g.link(terrain, &steep, "In");
    let mid = g.add(
        "heightmask",
        x,
        640.0,
        &[
            ("min", f(look.mid_low)),
            ("max", f(look.mid_high)),
            ("falloff", f(0.06)),
        ],
    );
    g.link(terrain, &mid, "In");
    let high = g.add(
        "heightmask",
        x,
        980.0,
        &[
            ("min", f(look.mid_high)),
            ("max", f(1.0)),
            ("falloff", f(0.08)),
        ],
    );
    g.link(terrain, &high, "In");
    let shore = g.add(
        "heightmask",
        x,
        1150.0,
        &[
            ("min", f(look.low)),
            ("max", f(look.low + look.shore_band)),
            ("falloff", f(0.04)),
        ],
    );
    g.link(terrain, &shore, "In");

    // Flat and mid: the walkable surface, and where metal and grass go.
    let walk = g.add(
        "mix",
        x + 230.0,
        550.0,
        &[("mode", s("multiply")), ("amount", f(1.0))],
    );
    g.link(&flat, &walk, "A");
    g.link(&mid, &walk, "B");

    let m_out = g.add("out_metal", x + 460.0, 550.0, &[]);
    g.link(&walk, &m_out, "In");
    let t_out = g.add("out_type", x + 230.0, 810.0, &[("levels", f(2.0))]);
    g.link(&steep, &t_out, "In");
    let g_out = g.add(
        "out_grass",
        x + 460.0,
        700.0,
        &[("threshold", f(look.grass_threshold))],
    );
    g.link(&walk, &g_out, "In");

    let sp_out = g.add(
        "out_splat",
        x + 690.0,
        880.0,
        &[("normalize", PVal::Bool(true))],
    );
    g.link(&steep, &sp_out, "R");
    g.link(&walk, &sp_out, "G");
    g.link(&high, &sp_out, "B");
    g.link(&shore, &sp_out, "A");
}

/// Open plains: broad country you can build almost anywhere on, with the
/// drainage left in and a graded route through the one real climb.
///
/// The first version of this fought the terrain — `flatten` against a height
/// mask, tiny relief, everything pulled toward one level — and the result was
/// a green bedsheet with no shape to fight over. `grade` is the better
/// instrument: it bounds the slope and leaves the *pattern* alone, so the
/// terrain here is ordinary noise with ordinary erosion and the flatness is
/// imposed at the end. That is the demonstration as much as the map: a plains
/// map is not terrain with the interest removed, it is terrain with the
/// gradients bounded.
fn plains() -> Graph {
    let mut g = Graph::new();
    // Ordinary terrain, deliberately. What makes this a plain happens later.
    let base = g.add(
        "noise",
        40.0,
        60.0,
        &[
            ("feature", f(4600.0)),
            ("octaves", f(6.0)),
            ("gain", f(0.48)),
        ],
    );
    // One broad rise across the map so the halves are not each other's
    // featureless twin, kept gentle enough that grading barely touches it.
    let tilt = g.add(
        "radial",
        40.0,
        300.0,
        &[
            ("radius", f(7000.0)),
            ("falloff", f(1.2)),
            ("invert", PVal::Bool(true)),
        ],
    );
    let mix = g.add(
        "mix",
        300.0,
        150.0,
        &[("mode", s("blend")), ("amount", f(0.28))],
    );
    g.link(&base, &mix, "A");
    g.link(&tilt, &mix, "B");
    let sym = g.add("symmetry", 540.0, 150.0, &[("mode", s("mirrorX"))]);
    g.link(&mix, &sym, "In");
    // Real erosion, not a token amount: the drainage is the whole reason a
    // plain reads as country rather than as a table, and grading keeps the
    // channels while taking their banks down to something buildable.
    let hyd = g.add(
        "hydraulic",
        780.0,
        150.0,
        &[("density", f(0.6)), ("capacity", f(3.6))],
    );
    g.link(&sym, &hyd, "In");
    // The instrument. 7° is inside a tank's 18° climb and inside the 12° a
    // builder wants, with room for the crease overshoot at the channel edges.
    let grade = g.add("grade", 1020.0, 150.0, &[("grade", f(7.0))]);
    g.link(&hyd, &grade, "In");
    // A whisper of small-feature noise on top. Grading leaves long flats that
    // read as sheet metal in the 3D view; this is a couple of elmos over a
    // 700-elmo feature, far under a degree, and it puts the ground back.
    let grain = g.add(
        "noise",
        1020.0,
        340.0,
        &[
            ("feature", f(760.0)),
            ("octaves", f(3.0)),
            ("gain", f(0.5)),
            ("seed", f(21.0)),
        ],
    );
    let dust = g.add(
        "mix",
        1260.0,
        150.0,
        &[("mode", s("blend")), ("amount", f(0.05))],
    );
    g.link(&grade, &dust, "A");
    g.link(&grain, &dust, "B");
    let lev = g.add("normalize", 1500.0, 150.0, &[]);
    g.link(&dust, &lev, "In");
    // A graded route across the middle, so whatever relief survives has a way
    // through it. Drag the waypoints in the 3D view to re-route it.
    let route = g.add(
        "ramp",
        1980.0,
        150.0,
        &[
            // Symmetric about the mirror axis on purpose: the ramp is
            // applied after `symmetry`, so an asymmetric route would hand one
            // side a road the other does not have.
            (
                "points",
                PVal::Points(vec![
                    [700.0, 3072.0],
                    [2100.0, 2760.0],
                    [3072.0, 3072.0],
                    [4044.0, 2760.0],
                    [5444.0, 3072.0],
                ]),
            ),
            ("width", f(300.0)),
            ("falloff", f(480.0)),
            ("ends", f(0.12)),
        ],
    );
    g.link(&lev, &route, "In");
    let out = g.add("out_height", 2220.0, 60.0, &[]);
    g.link(&route, &out, "In");
    texture_chain(
        &mut g,
        &route,
        &Look {
            ramp: "verdant",
            low: 0.12,
            // Past 1: the pale top of the ramp is snow, and a 130-elmo rise
            // in temperate country does not have snow on it.
            high: 1.35,
            rock_from: 14.0,
            rock_full: 30.0,
            detail_feature: 220.0,
            detail_amount: 0.13,
            sun: 0.34,
            ao: 0.22,
            // Everything here is flat, so the flat threshold has to be tight
            // or the G channel simply covers the map.
            flat_max: 5.0,
            mid_low: 0.14,
            mid_high: 0.86,
            grass_threshold: 0.3,
            // Narrow. A height band is a *area* on flat ground: at the usual
            // 0.08 the shore material covered the whole low basin, and the
            // map read as beach with lakes in it.
            shore_band: 0.03,
            ..Default::default()
        },
        2220.0,
    );
    g
}

/// Open ground between two ranges: a wide buildable corridor across the middle
/// with the high country pushed out to the north and south flanks.
///
/// This is the layout Zero-K actually plays on and the one the generator could
/// not previously express. Every other starter here builds terrain that is the
/// same *kind* of terrain everywhere and lets the noise decide where the flat
/// is; a real map decides first, and puts the fight in the flat part.
///
/// The mechanism is one mask used twice, in opposite directions: a flank
/// weight, zero along the centreline and one at the north and south edges,
/// multiplies the ridged noise so hills can only grow at the edges — and its
/// inverse drives a `grade` limit so the middle is bound to something you can
/// build a base on while the flanks keep their crags. The mask is built from a
/// gradient folded about its own middle, which is what `difference` against a
/// constant 0.5 does.
fn open() -> Graph {
    let mut g = Graph::new();

    /* -- where the high ground is allowed to be --------------------------- */
    let across = g.add("gradient", 40.0, 60.0, &[("angle", f(90.0))]);
    let mid = g.add("constant", 40.0, 220.0, &[("value", f(0.5))]);
    // |y - 0.5|: zero on the centreline, 0.5 at both edges.
    let fold = g.add(
        "mix",
        280.0,
        120.0,
        &[("mode", s("difference")), ("amount", f(1.0))],
    );
    g.link(&across, &fold, "A");
    g.link(&mid, &fold, "B");
    // Stretched to 0..1 and held off the very middle, so the corridor has a
    // flat floor rather than a single flat line.
    let flank = g.add(
        "remap",
        520.0,
        120.0,
        &[
            // The corridor is the middle quarter of the map, the ranges own
            // the outer third of each flank, and the rest is the climb
            // between them.
            ("inMin", f(0.12)),
            ("inMax", f(0.34)),
            ("outMin", f(0.0)),
            ("outMax", f(1.0)),
        ],
    );
    g.link(&fold, &flank, "In");

    /* -- the two ranges --------------------------------------------------- */
    let hills = g.add(
        "ridged",
        520.0,
        330.0,
        &[
            ("feature", f(2600.0)),
            ("octaves", f(6.0)),
            ("gain", f(0.52)),
        ],
    );
    // Ridges only where the flank mask lets them: multiply, do not blend.
    let ranged = g.add(
        "mix",
        760.0,
        250.0,
        &[("mode", s("multiply")), ("amount", f(1.0))],
    );
    g.link(&hills, &ranged, "A");
    g.link(&flank, &ranged, "B");
    // A gentle floor everywhere, held in a narrow band well clear of the
    // waterline. The ranges are *added* to this rather than blended with it:
    // blending drags the corridor down toward zero, and a corridor at the
    // bottom of the range is a channel of sea, which is exactly the map
    // everyone was already complaining about.
    let base = g.add(
        "noise",
        760.0,
        460.0,
        &[
            ("feature", f(3400.0)),
            ("octaves", f(5.0)),
            ("gain", f(0.45)),
            ("seed", f(9.0)),
        ],
    );
    let floor = g.add(
        "remap",
        1000.0,
        460.0,
        &[
            ("inMin", f(0.15)),
            ("inMax", f(0.85)),
            ("outMin", f(0.2)),
            ("outMax", f(0.42)),
        ],
    );
    g.link(&base, &floor, "In");
    let land = g.add(
        "mix",
        1240.0,
        340.0,
        &[("mode", s("add")), ("amount", f(0.72))],
    );
    g.link(&floor, &land, "A");
    g.link(&ranged, &land, "B");

    let sym = g.add("symmetry", 1480.0, 250.0, &[("mode", s("mirrorX"))]);
    g.link(&land, &sym, "In");
    let hyd = g.add(
        "hydraulic",
        1720.0,
        250.0,
        &[("density", f(0.45)), ("capacity", f(3.0))],
    );
    g.link(&sym, &hyd, "In");

    /* -- bind the corridor, leave the ranges alone ------------------------ */
    // The same flank mask inverted: full grading on the centreline, none at
    // the edges. A crag on the flank is scenery; a crag in the corridor is a
    // base that cannot be built.
    let corridor = g.add("invert", 1720.0, 600.0, &[]);
    g.link(&flank, &corridor, "In");
    let grade = g.add("grade", 1960.0, 250.0, &[("grade", f(8.0))]);
    g.link(&hyd, &grade, "In");
    g.link(&corridor, &grade, "Mask");

    let lev = g.add("normalize", 2200.0, 250.0, &[]);
    g.link(&grade, &lev, "In");
    let out = g.add("out_height", 2440.0, 160.0, &[]);
    g.link(&lev, &out, "In");
    texture_chain(
        &mut g,
        &lev,
        &Look {
            ramp: "verdant",
            low: 0.1,
            high: 1.3,
            rock_from: 22.0,
            rock_full: 40.0,
            detail_feature: 200.0,
            detail_amount: 0.15,
            sun: 0.4,
            ao: 0.3,
            flat_max: 8.0,
            mid_low: 0.12,
            mid_high: 0.7,
            grass_threshold: 0.35,
            shore_band: 0.04,
            ..Default::default()
        },
        2680.0,
    );
    g
}

/// Five-way free-for-all: five base plateaux around a contested middle.
///
/// The layout is the point. Five arms of open ground radiate from a central
/// bowl, separated by ridges that are crossable rather than sealed — the map
/// should reward taking the middle without making the flanks impassable, which
/// means the dividing ground has to be *graded*, not walled.
///
/// `rot72` does the fairness. It is the only operator here that interpolates,
/// because a fifth of a turn carries lattice points to places where no lattice
/// point is; everything downstream of it is identical for all five players by
/// construction, and the metal placer and start boxes use the same rotation.
///
/// Grading runs after erosion and before the normalise, as everywhere else:
/// erosion is what stops five identical arms reading as a logo, and grading is
/// what stops its banks being cliffs.
fn ffa5() -> Graph {
    let mut g = Graph::new();

    // Rolling ground, deliberately compressed into a narrow band. The noise
    // is the *texture* of this map, not its shape — left at full range it
    // buries the five bases and the middle under peaks, which is what the
    // first two attempts did.
    let grain = g.add(
        "noise",
        40.0,
        60.0,
        &[
            ("feature", f(2600.0)),
            ("octaves", f(6.0)),
            ("gain", f(0.52)),
        ],
    );
    let rolling = g.add(
        "remap",
        300.0,
        60.0,
        &[
            ("inMin", f(0.0)),
            ("inMax", f(1.0)),
            ("outMin", f(0.34)),
            ("outMax", f(0.64)),
        ],
    );
    g.link(&grain, &rolling, "In");

    // The middle, as a broad flat-bottomed basin rather than a funnel. A
    // falloff under 1 gives the radial a flat top, so subtracting it gives a
    // flat *floor* — an arena five players can fight over. With a falloff
    // above 1 the minimum is a single point at the exact centre, which
    // normalises to zero, floods, and puts a puddle where the prize should be.
    let dip = g.add(
        "radial",
        40.0,
        320.0,
        &[("radius", f(3600.0)), ("falloff", f(0.8))],
    );
    // Clipped flat on top before it is subtracted. A radial's falloff alone
    // never gives a truly flat summit -- even below 1 the gradient at the
    // centre is non-zero, so the low point stays a single point, `normalize`
    // pins it to zero and the arena everyone is meant to fight over comes out
    // as a puddle. Remapping with the input maximum pulled in clamps the top
    // third to one, which is a mesa, and a subtracted mesa is a floor.
    let floored = g.add(
        "remap",
        300.0,
        320.0,
        &[
            ("inMin", f(0.0)),
            ("inMax", f(0.62)),
            ("outMin", f(0.0)),
            ("outMax", f(1.0)),
        ],
    );
    g.link(&dip, &floored, "In");
    let hollow = g.add(
        "mix",
        560.0,
        150.0,
        &[("mode", s("subtract")), ("amount", f(0.22))],
    );
    g.link(&rolling, &hollow, "A");
    g.link(&floored, &hollow, "B");
    let land = hollow;

    // One base platform, placed **due east** — inside the fundamental wedge of
    // the `rot72` node below, which folds about the +x axis. Put it anywhere
    // else and the fold overwrites it with whatever the east wedge held. The
    // five copies, and the five start boxes, all come from that one decision.
    let plate = g.add(
        "radial",
        40.0,
        560.0,
        &[
            ("cx", f(0.5 + 0.29)),
            ("cy", f(0.5)),
            ("radius", f(2100.0)),
            // Under 1 gives a flat top with a quick shoulder: a plateau, not
            // a dome. A dome is not somewhere you put a factory.
            ("falloff", f(0.6)),
        ],
    );
    let seat = g.add(
        "remap",
        300.0,
        560.0,
        &[
            ("inMin", f(0.0)),
            ("inMax", f(1.0)),
            ("outMin", f(0.0)),
            ("outMax", f(0.3)),
        ],
    );
    g.link(&plate, &seat, "In");
    // Added, not maxed: the platform lifts the ground it sits on and keeps
    // that ground's own texture, instead of replacing it with a bare dome.
    let seated = g.add(
        "mix",
        820.0,
        150.0,
        &[("mode", s("add")), ("amount", f(1.0))],
    );
    g.link(&land, &seated, "A");
    g.link(&seat, &seated, "B");

    // Five-fold, before anything erodes, so all five arms erode as one does.
    let sym = g.add("symmetry", 1080.0, 150.0, &[("mode", s("rot72"))]);
    g.link(&seated, &sym, "In");

    // The wedge join is a real discontinuity for the same reason rot180's is:
    // the fixed set is a point, so the two edges of the fundamental domain
    // meet at an angle. Unhealed, erosion sharpens it into five spokes.
    let heal = g.add("blur", 1320.0, 150.0, &[("radius", f(90.0))]);
    g.link(&sym, &heal, "In");

    let hyd = g.add(
        "hydraulic",
        1560.0,
        150.0,
        &[("density", f(0.4)), ("capacity", f(3.0))],
    );
    g.link(&heal, &hyd, "In");

    // 15 degrees, not 10: the brief is hills that are crossable, not an
    // absence of hills. A tank climbs 18, so this leaves the whole map
    // traversable while keeping the relief that makes it a map.
    let grade = g.add("grade", 1800.0, 150.0, &[("grade", f(16.0))]);
    g.link(&hyd, &grade, "In");

    // A fill-only pass to close the sinks erosion dug.
    //
    // Water runs downhill and this map's downhill is one place: the middle.
    // Hydraulic erosion duly excavated a pit at the exact centre, which
    // `normalize` pinned to zero and the sea filled — a puddle where the prize
    // is meant to be. `fill` is the upper envelope of cones, so it closes any
    // depression whose walls are steeper than the limit and leaves gentle ones
    // untouched: the sink goes, the arena stays, and nothing else on the map
    // moves.
    let unpit = g.add(
        "grade",
        2040.0,
        150.0,
        &[("grade", f(9.0)), ("mode", s("fill"))],
    );
    g.link(&grade, &unpit, "In");

    let norm = g.add("normalize", 2280.0, 150.0, &[]);
    g.link(&unpit, &norm, "In");
    // Held clear of the waterline. `normalize` puts the lowest ground at zero
    // and the lowest ground here is the arena floor, so at a zero submerged
    // fraction the floor lands *exactly* on the waterline — dry in the engine,
    // but painted as sea by `tex_water`, which is a lake as far as anyone
    // looking at the map is concerned. Lifting the whole field a twentieth of
    // its range puts the floor a few elmos up and costs nothing else.
    let lev = g.add(
        "remap",
        2520.0,
        150.0,
        &[
            ("inMin", f(0.0)),
            ("inMax", f(1.0)),
            ("outMin", f(0.05)),
            ("outMax", f(1.0)),
        ],
    );
    g.link(&norm, &lev, "In");
    let out = g.add("out_height", 2760.0, 60.0, &[]);
    g.link(&lev, &out, "In");

    texture_chain(
        &mut g,
        &lev,
        &Look {
            ramp: "verdant",
            // Below the arena floor, which the remap put at 0.05. A ramp that
            // starts above the map's own lowest ground paints that ground in
            // the palette's darkest colour, and a black disc in the middle of
            // a map reads as a hole whatever the heightmap says.
            low: 0.02,
            high: 1.2,
            rock_from: 20.0,
            rock_full: 38.0,
            detail_feature: 210.0,
            detail_amount: 0.15,
            sun: 0.4,
            ao: 0.3,
            flat_max: 10.0,
            mid_low: 0.1,
            mid_high: 0.72,
            grass_threshold: 0.34,
            shore_band: 0.04,
            ..Default::default()
        },
        3000.0,
    );
    g
}

/// The materials and light a starter comes with.
///
/// A sample map is a whole map, not a heightfield: the surfaces and the hour
/// of the day are as much a part of it as the terrain. Applied on top of an
/// existing project so a name, size or seed already chosen survives.
pub fn apply_starter(project: &mut Project, kind: &str) {
    // How much of the vertical range sits below the waterline. A mesa field
    // with a sea in it is a swamp, and an archipelago without one is a plain.
    let (submerged, range) = match kind {
        "islands" => (0.34, 460.0),
        "mesa" => (0.02, 520.0),
        "glacier" => (0.14, 620.0),
        // A third of the usual relief: on a plain the shapes matter more than
        // the drop, and 500 elmos of it would not be a plain.
        "plains" => (0.1, 170.0),
        // Nearly dry. The corridor holds the map's lowest ground, and
        // `normalize` pins the lowest ground to zero, so any real submerged
        // fraction floods the very part of the map you are meant to fight
        // over -- which is how the generator got its reputation for putting
        // an ocean in the middle of everything.
        "open" => (0.02, 420.0),
        // Bone dry, and that is not a stylistic choice. `normalize` pins the
        // lowest ground to zero and on this layout the lowest ground is the
        // central arena — so *any* submerged fraction puts a lake exactly
        // where five players are meant to fight. Measured before this was
        // fixed: the arena floor sat at -14 elmos under a 3% fraction and
        // flooded. Generous relief instead, so the ground between the bases
        // still reads as hills after a 16-degree grade limit.
        "ffa5" => (0.04, 520.0),
        _ => (0.16, 500.0),
    };
    let (mn, mx) = crate::project::height_range_for(submerged, range);
    project.min_height = mn;
    project.max_height = mx;

    // The ground the engine actually shows.
    //
    // Steep channels stay drawn: a photograph of flat ground stretched up a
    // cliff face reads as a smear, and `rock` and `cliff` are built to be
    // looked at edge-on. Everything a unit stands on is photographic, and so
    // is every detail tile — that last one matters most, because Spring
    // multiplies `detailTex` over the whole map at runtime, so it is the only
    // surface present on every texel of the finished thing.
    let (splat, detail, environment, sym) = match kind {
        // Coastal flats lusher than the plateau above them, and a damp shore.
        "islands" => (
            ["rock", "lawn", "steppe", "silt"],
            "silt",
            "temperate",
            "mirrorX",
        ),
        // Dry clay basins, pale dust on the mesa tops, and no beach worth the
        // name -- A carries almost no weight under a 2% submerged fraction.
        "mesa" => (["cliff", "clay", "dust", "clay"], "clay", "arid", "mirrorX"),
        // Snow is drawn -- there is no photographic snow in the set, and a
        // grey surface on the high ground would stop it being a glacier. The
        // shore band is ice-free till instead of the coarsest drawn gravel,
        // which read as dithering against a snowfield.
        "glacier" => (
            ["cliff", "snow", "snow", "pavement"],
            "pavement",
            "arctic",
            "rot180",
        ),
        // Dry upland grass on the fight surface and pale debris above it.
        "textured" => (
            ["rock", "steppe", "scrub", "silt"],
            "steppe",
            "temperate",
            "rot180",
        ),
        // A meadow with dry upland at its edges, which is what the terrain
        // actually is once `grade` has bounded it.
        "plains" => (
            ["gravel", "lawn", "steppe", "silt"],
            "steppe",
            "temperate",
            "mirrorX",
        ),
        // The corridor is meadow, the flanking ranges are scrub over rock.
        "open" => (
            ["rock", "lawn", "scrub", "silt"],
            "steppe",
            "temperate",
            "mirrorX",
        ),
        "ffa5" => (
            ["rock", "lawn", "steppe", "silt"],
            "steppe",
            "temperate",
            "rot72",
        ),
        // ridge, and anything unknown.
        _ => (
            ["cliff", "steppe", "scrub", "silt"],
            "steppe",
            "temperate",
            "rot180",
        ),
    };
    project.materials = MaterialSet {
        splat: splat.map(str::to_string),
        detail: detail.to_string(),
        ..MaterialSet::default()
    };
    if let Some(e) = env::preset(environment) {
        project.environment = e;
    }
    project.mex_sym = sym.to_string();
}

/// A project carrying a starter's graph settings, for a caller that has none.
pub fn starter_project(kind: &str) -> Project {
    let mut p = Project::default();
    apply_starter(&mut p, kind);
    p
}

fn islands() -> Graph {
    let mut g = Graph::new();
    let base = g.add(
        "noise",
        40.0,
        60.0,
        &[("feature", f(2600.0)), ("octaves", f(7.0))],
    );
    let shape = g.add(
        "radial",
        40.0,
        250.0,
        &[("radius", f(4200.0)), ("falloff", f(1.1))],
    );
    let mul = g.add(
        "mix",
        300.0,
        130.0,
        &[("mode", s("multiply")), ("amount", f(1.0))],
    );
    g.link(&base, &mul, "A");
    g.link(&shape, &mul, "B");
    let warp = g.add(
        "warp",
        540.0,
        130.0,
        &[("strength", f(700.0)), ("feature", f(2200.0))],
    );
    g.link(&mul, &warp, "In");
    let sym = g.add("symmetry", 780.0, 130.0, &[("mode", s("mirrorX"))]);
    g.link(&warp, &sym, "In");
    let hyd = g.add("hydraulic", 1020.0, 130.0, &[("density", f(0.7))]);
    g.link(&sym, &hyd, "In");
    // Normalise last, so the field is exactly 0..1 at every resolution and
    // the waterline lands where the project says it does. Without it the
    // terrain's own minimum decides, and that moves with resolution: the
    // preview showed lakes the bake did not have.
    let lev = g.add("normalize", 1260.0, 130.0, &[]);
    g.link(&hyd, &lev, "In");
    let out = g.add("out_height", 1400.0, 130.0, &[]);
    g.link(&lev, &out, "In");
    // An archipelago is mostly coastline, so the shore band is wide and the
    // ramp starts low: there is very little high ground to spend it on.
    texture_chain(
        &mut g,
        &lev,
        &Look {
            ramp: "verdant",
            low: 0.2,
            high: 1.0,
            rock_from: 20.0,
            rock_full: 40.0,
            detail_feature: 150.0,
            flat_max: 14.0,
            mid_low: 0.2,
            mid_high: 0.46,
            shore_band: 0.1,
            shallow: "#3A7E86",
            deep: "#0A2230",
            ..Default::default()
        },
        1500.0,
    );
    g
}

fn ridge() -> Graph {
    let mut g = Graph::new();
    let r1 = g.add(
        "ridged",
        40.0,
        60.0,
        &[("feature", f(5200.0)), ("octaves", f(7.0))],
    );
    let n1 = g.add(
        "noise",
        40.0,
        300.0,
        &[
            ("feature", f(1400.0)),
            ("octaves", f(6.0)),
            ("seed", f(5.0)),
        ],
    );
    let mix = g.add(
        "mix",
        300.0,
        150.0,
        &[("mode", s("blend")), ("amount", f(0.32))],
    );
    g.link(&r1, &mix, "A");
    g.link(&n1, &mix, "B");
    let curve = g.add(
        "curve",
        540.0,
        150.0,
        &[("mode", s("gain")), ("amount", f(0.62))],
    );
    g.link(&mix, &curve, "In");
    let sym = g.add("symmetry", 780.0, 150.0, &[("mode", s("rot180"))]);
    g.link(&curve, &sym, "In");
    // rot180's fixed set is a point, not a line, so the two halves meet along
    // a real height discontinuity and erosion turns it into a ridge down the
    // middle of the map -- measurably the steepest row on it. A short blur
    // before erosion heals the join; erosion then puts the detail back.
    let heal = g.add("blur", 900.0, 150.0, &[("radius", f(96.0))]);
    g.link(&sym, &heal, "In");
    let hyd = g.add(
        "hydraulic",
        1020.0,
        150.0,
        &[("density", f(0.8)), ("capacity", f(5.0))],
    );
    g.link(&heal, &hyd, "In");
    let th = g.add(
        "thermal",
        1260.0,
        150.0,
        &[("iterations", f(24.0)), ("talus", f(33.0))],
    );
    g.link(&hyd, &th, "In");
    // See `islands`: normalise last so the waterline is where it is declared.
    let lev = g.add("normalize", 1440.0, 150.0, &[]);
    g.link(&th, &lev, "In");
    let out = g.add("out_height", 1580.0, 150.0, &[]);
    g.link(&lev, &out, "In");
    let sl = g.add(
        "slopemask",
        1260.0,
        360.0,
        &[("minDeg", f(0.0)), ("maxDeg", f(14.0)), ("falloff", f(5.0))],
    );
    g.link(&lev, &sl, "In");
    let hm = g.add(
        "heightmask",
        1260.0,
        530.0,
        &[("min", f(0.34)), ("max", f(0.72)), ("falloff", f(0.06))],
    );
    g.link(&lev, &hm, "In");
    let mm = g.add(
        "mix",
        1500.0,
        430.0,
        &[("mode", s("multiply")), ("amount", f(1.0))],
    );
    g.link(&sl, &mm, "A");
    g.link(&hm, &mm, "B");
    let mo = g.add("out_metal", 1740.0, 430.0, &[]);
    g.link(&mm, &mo, "In");
    // Ridge keeps its own metal mask -- it is tuned for this terrain -- and
    // takes the rest of the output set from the shared chain.
    texture_chain(
        &mut g,
        &lev,
        &Look {
            rock_from: 26.0,
            mid_low: 0.34,
            mid_high: 0.74,
            ..Default::default()
        },
        1980.0,
    );
    // Two metal terminals would be ambiguous, so the chain's is removed and
    // the hand-tuned one above stands.
    let chain_metal: Vec<String> = g
        .nodes
        .iter()
        .filter(|n| n.type_name == "out_metal" && n.id != mo)
        .map(|n| n.id.clone())
        .collect();
    for id in chain_metal {
        g.remove(&id);
    }
    g
}

/// Flat-topped desert mesas: terraced height, almost no water, and a shore
/// band that reads as dry salt flat rather than beach.
fn mesa() -> Graph {
    let mut g = Graph::new();
    // Several distinct buttes rather than two continental plateaux: at 2600
    // elmos a feature spans a third of the map and the terrace risers read as
    // contour lines on a map rather than as cliffs.
    let base = g.add(
        "noise",
        40.0,
        60.0,
        &[("feature", f(1500.0)), ("octaves", f(5.0))],
    );
    let warp = g.add(
        "warp",
        290.0,
        60.0,
        &[("strength", f(300.0)), ("feature", f(1100.0))],
    );
    g.link(&base, &warp, "In");
    // Terracing is what makes a mesa a mesa: flat tops, sheer sides.
    let terrace = g.add(
        "terrace",
        540.0,
        60.0,
        &[("steps", f(4.0)), ("sharpness", f(0.96))],
    );
    g.link(&warp, &terrace, "In");
    let grit = g.add(
        "noise",
        290.0,
        320.0,
        &[
            ("feature", f(520.0)),
            ("octaves", f(5.0)),
            ("seed", f(17.0)),
        ],
    );
    let mix = g.add(
        "mix",
        780.0,
        150.0,
        &[("mode", s("blend")), ("amount", f(0.12))],
    );
    g.link(&terrace, &mix, "A");
    g.link(&grit, &mix, "B");
    // mirrorX, not rot180: its fixed set is the centre line, so the halves
    // meet continuously and no healing blur is needed -- which matters here,
    // because a blur would round the very edges the terracing exists to make.
    let sym = g.add("symmetry", 1020.0, 150.0, &[("mode", s("mirrorX"))]);
    g.link(&mix, &sym, "In");
    // Light erosion only: a mesa that has been rained flat is a hill.
    let th = g.add(
        "thermal",
        1260.0,
        150.0,
        &[("iterations", f(8.0)), ("talus", f(52.0))],
    );
    g.link(&sym, &th, "In");
    // See `islands`: normalise last so the waterline is where it is declared.
    let lev = g.add("normalize", 1500.0, 150.0, &[]);
    g.link(&th, &lev, "In");
    let out = g.add("out_height", 1740.0, 60.0, &[]);
    g.link(&lev, &out, "In");
    texture_chain(
        &mut g,
        &lev,
        &Look {
            ramp: "rust",
            low: 0.0,
            high: 1.0,
            rock_from: 22.0,
            rock_full: 40.0,
            rock_color: "#8A6E4E",
            detail_feature: 240.0,
            detail_amount: 0.12,
            shallow: "#4C6A5E",
            deep: "#16281F",
            sun: 0.5,
            ao: 0.24,
            flat_max: 9.0,
            mid_low: 0.1,
            mid_high: 0.55,
            grass_threshold: 0.9,
            shore_band: 0.05,
        },
        1980.0,
    );
    g
}

/// High glacial ground: broad ridges, deep valleys, snow on everything that
/// is not a cliff face.
fn glacier() -> Graph {
    let mut g = Graph::new();
    let r1 = g.add(
        "ridged",
        40.0,
        60.0,
        &[("feature", f(4400.0)), ("octaves", f(8.0))],
    );
    let n1 = g.add(
        "noise",
        40.0,
        300.0,
        &[
            ("feature", f(1200.0)),
            ("octaves", f(7.0)),
            ("seed", f(23.0)),
        ],
    );
    let mix = g.add(
        "mix",
        300.0,
        150.0,
        &[("mode", s("blend")), ("amount", f(0.3))],
    );
    g.link(&r1, &mix, "A");
    g.link(&n1, &mix, "B");
    let curve = g.add(
        "curve",
        540.0,
        150.0,
        &[("mode", s("gain")), ("amount", f(0.58))],
    );
    g.link(&mix, &curve, "In");
    let sym = g.add("symmetry", 780.0, 150.0, &[("mode", s("rot180"))]);
    g.link(&curve, &sym, "In");
    // See `ridge`: rot180 leaves a join that erosion sharpens into a ridge.
    let heal = g.add("blur", 900.0, 150.0, &[("radius", f(96.0))]);
    g.link(&sym, &heal, "In");
    // A wide talus is what makes a glacial valley read as U-shaped rather
    // than V-shaped. The hydraulic pass stays moderate: carve it much harder
    // and the whole map arrives pre-flattened, which is what happened first
    // time round.
    let hyd = g.add(
        "hydraulic",
        1020.0,
        150.0,
        &[("density", f(0.5)), ("capacity", f(4.0))],
    );
    g.link(&heal, &hyd, "In");
    let th = g.add(
        "thermal",
        1260.0,
        150.0,
        &[("iterations", f(16.0)), ("talus", f(36.0))],
    );
    g.link(&hyd, &th, "In");
    // See `islands`: normalise last so the waterline is where it is declared.
    let lev = g.add("normalize", 1440.0, 150.0, &[]);
    g.link(&th, &lev, "In");
    let out = g.add("out_height", 1580.0, 60.0, &[]);
    g.link(&lev, &out, "In");
    texture_chain(
        &mut g,
        &lev,
        &Look {
            ramp: "slate",
            low: 0.16,
            high: 0.86,
            rock_from: 24.0,
            rock_full: 42.0,
            rock_color: "#4B515C",
            detail_feature: 200.0,
            detail_amount: 0.14,
            shallow: "#3E6E85",
            deep: "#08202E",
            sun: 0.4,
            ao: 0.42,
            flat_max: 13.0,
            mid_low: 0.24,
            mid_high: 0.6,
            grass_threshold: 0.95,
            shore_band: 0.07,
        },
        1740.0,
    );
    g
}

fn textured() -> Graph {
    let mut g = Graph::new();
    let ridge = g.add(
        "ridged",
        40.0,
        60.0,
        &[("feature", f(5200.0)), ("octaves", f(7.0))],
    );
    let fine = g.add(
        "noise",
        40.0,
        300.0,
        &[
            ("feature", f(1300.0)),
            ("octaves", f(6.0)),
            ("seed", f(5.0)),
        ],
    );
    let mix = g.add(
        "mix",
        290.0,
        150.0,
        &[("mode", s("blend")), ("amount", f(0.3))],
    );
    g.link(&ridge, &mix, "A");
    g.link(&fine, &mix, "B");
    let curve = g.add(
        "curve",
        520.0,
        150.0,
        &[("mode", s("gain")), ("amount", f(0.6))],
    );
    g.link(&mix, &curve, "In");
    let sym = g.add("symmetry", 750.0, 150.0, &[("mode", s("rot180"))]);
    g.link(&curve, &sym, "In");
    // See `ridge`: rot180 leaves a join that erosion sharpens into a ridge.
    let heal = g.add("blur", 865.0, 150.0, &[("radius", f(96.0))]);
    g.link(&sym, &heal, "In");
    let hyd = g.add(
        "hydraulic",
        980.0,
        150.0,
        &[("density", f(0.8)), ("capacity", f(5.0))],
    );
    g.link(&heal, &hyd, "In");
    let th = g.add(
        "thermal",
        1210.0,
        150.0,
        &[("iterations", f(22.0)), ("talus", f(33.0))],
    );
    g.link(&hyd, &th, "In");
    // See `islands`: normalise last so the waterline is where it is declared.
    let lev = g.add("normalize", 1440.0, 150.0, &[]);
    g.link(&th, &lev, "In");
    let h_out = g.add("out_height", 1580.0, 60.0, &[]);
    g.link(&lev, &h_out, "In");
    // The same chain every other starter uses. It had its own copy, which is
    // how its A channel ended up unwired: a fourth detail material with no
    // weights to appear under.
    texture_chain(&mut g, &lev, &Look::default(), 1720.0);
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout claim in the `open` starter's name, measured.
    ///
    /// Players asked for the map shape Zero-K actually plays on — a wide
    /// buildable middle with the high country out on the flanks — and a
    /// starter that merely *intends* that is worth nothing. So the corridor
    /// has to come out flatter than the ranges by a wide margin, and it has
    /// to be dry.
    #[test]
    fn the_open_starter_leaves_the_middle_open() {
        const R: usize = 129;
        let mut p = Project::default();
        apply_starter(&mut p, "open");
        let ctx = crate::project::Context::new(&p, R);
        let g = starter_graph("open");
        let id = g.find_terminal("height").unwrap().to_string();
        let field = crate::field::as_gray(&g.evaluate(&id, &ctx));

        let e = ctx.elmo_per_px_x();
        let band_slope = |y0: usize, y1: usize| {
            let mut sum = 0.0;
            let mut n = 0;
            for y in y0..y1 {
                for x in 0..R {
                    sum +=
                        crate::nodes::slope_degrees_aniso(&field, x, y, R, ctx.height_range, e, e);
                    n += 1;
                }
            }
            sum / n as f64
        };
        // The middle eighth against the outer eighth of each flank.
        let middle = band_slope(R * 7 / 16, R * 9 / 16);
        let flanks = (band_slope(0, R / 8) + band_slope(R * 7 / 8, R)) / 2.0;
        assert!(
            flanks > middle * 2.5,
            "the flanks must be the hilly part: middle {middle:.1}°, flanks {flanks:.1}°"
        );

        // And dry. `normalize` pins the map's lowest ground to zero and the
        // corridor holds it, so a careless submerged fraction floods exactly
        // the ground this starter exists to provide.
        let sea = crate::project::water_level_t(p.min_height, p.max_height);
        let wet =
            (0..field.len()).filter(|i| field.get(*i) < sea).count() as f64 / field.len() as f64;
        assert!(wet < 0.1, "{:.1}% of the map is under water", wet * 100.0);
    }

    #[test]
    fn every_starter_wires_every_output_layer() {
        // A sample map with only a heightmap bakes to flat shaded relief with
        // no splat distribution, which means the detail materials have no
        // weights to blend with and do nothing at all. Two of the three
        // starters were in exactly that state.
        for (kind, _) in STARTERS {
            let g = starter_graph(kind);
            for term in ["height", "diffuse", "metal", "type", "grass", "splat"] {
                let id = g
                    .find_wired_terminal(term)
                    .unwrap_or_else(|| panic!("{kind}: no wired {term} terminal"));
                assert!(!id.is_empty());
            }
            // Exactly one of each, or which one wins is an accident of order.
            for term in ["height", "diffuse", "metal", "type", "grass", "splat"] {
                let n = g
                    .nodes
                    .iter()
                    .filter(|n| {
                        crate::graph::registry()
                            .get(&n.type_name)
                            .and_then(|s| s.output)
                            == Some(term)
                    })
                    .count();
                assert_eq!(n, 1, "{kind}: {n} {term} terminals");
            }
        }
    }

    #[test]
    fn every_splat_channel_carries_something() {
        // The four channels are the four detail materials. An unwired one is
        // a material slot that can never appear on the map.
        for (kind, _) in STARTERS {
            let g = starter_graph(kind);
            let id = g.find_terminal("splat").unwrap().to_string();
            let node = g.node(&id).unwrap();
            for port in ["R", "G", "B", "A"] {
                assert!(
                    node.inputs.contains_key(port),
                    "{kind}: splat channel {port} is unwired"
                );
            }
        }
    }

    #[test]
    fn every_starter_names_materials_that_exist() {
        for (kind, _) in STARTERS {
            let p = starter_project(kind);
            assert!(
                p.materials.unknown().is_empty(),
                "{kind}: unknown materials {:?}",
                p.materials.unknown()
            );
            assert!(
                crate::env::preset(&p.environment.preset).is_some(),
                "{kind}: unknown environment {}",
                p.environment.preset
            );
        }
    }

    #[test]
    fn every_starter_names_a_palette_that_exists() {
        // A misspelled ramp silently falls back to the default, so the map
        // still bakes and simply looks like a different starter.
        let names = crate::ramps::ramp_names();
        for (kind, _) in STARTERS {
            let g = starter_graph(kind);
            for n in g.nodes.iter().filter(|n| n.type_name == "tex_ramp") {
                let r = n.params.s("ramp").to_string();
                assert!(names.contains(&r.as_str()), "{kind}: no ramp named {r}");
            }
        }
    }

    #[test]
    fn symmetry_sits_upstream_of_erosion() {
        for (kind, _) in STARTERS {
            let g = starter_graph(kind);
            let Some(sym) = g
                .nodes
                .iter()
                .find(|n| n.type_name == "symmetry")
                .map(|n| n.id.clone())
            else {
                continue;
            };
            let erosion: Vec<String> = g
                .nodes
                .iter()
                .filter(|n| n.type_name == "hydraulic" || n.type_name == "thermal")
                .map(|n| n.id.clone())
                .collect();
            assert!(!erosion.is_empty(), "{kind} should erode");
            // No erosion node may feed the symmetry node, directly or not.
            for e in erosion {
                assert!(
                    !g.would_cycle(&sym, &e),
                    "{kind}: symmetry must come before erosion"
                );
            }
        }
    }
}

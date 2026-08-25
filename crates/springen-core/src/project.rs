//! Project settings and the render context nodes are evaluated against.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where generated maps go by default.
///
/// Never next to the executable: an installed copy lives in Program Files,
/// which is not writable without elevation, so a relative `out/` turns every
/// bake into an access-denied error.
pub fn default_output_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    match home {
        Some(h) => {
            let docs = h.join("Documents");
            if docs.is_dir() {
                docs.join("Springen")
            } else {
                h.join("Springen")
            }
        }
        None => std::env::temp_dir().join("Springen"),
    }
}

/// Everything about a map that is not the node graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub units_x: u32,
    pub units_y: u32,
    pub seed: i64,
    /// Height 0 elmos **is** the waterline, so these two also fix the sea level.
    pub min_height: f64,
    pub max_height: f64,
    #[serde(default = "default_hardness")]
    pub hardness: f64,
    #[serde(default = "default_gravity")]
    pub gravity: f64,
    #[serde(default)]
    pub tidal: f64,
    #[serde(default = "default_max_metal")]
    pub max_metal: f64,
    #[serde(default = "default_extractor_radius")]
    pub extractor_radius: f64,
    #[serde(default = "default_mex_sym")]
    pub mex_sym: String,
    /// Which surface goes in each SSMF detail slot.
    #[serde(default)]
    pub materials: crate::material::MaterialSet,
    /// Hand-edited start boxes, in elmos.
    ///
    /// `None` means "whatever the symmetry implies", which is what every
    /// project did before they could be edited and is still the default — so
    /// changing `mex_sym` keeps re-deriving them until someone actually moves
    /// one. The start *points* are deliberately not stored: they depend on the
    /// terrain, and freezing one would leave a commander on ground a later
    /// edit moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_boxes: Option<Vec<crate::lua::StartArea>>,
    /// How deep the water is allowed to get, in elmos below the waterline.
    ///
    /// `None` leaves the terrain alone, which is what every project did before
    /// this existed. A value shoals the sea floor toward the surface without
    /// moving the coastline: the depth a bot can ford is a property of the
    /// game, not of whatever the noise happened to do under the waterline, and
    /// lowering the declared range instead would raise the whole shore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<f64>,
    /// Sun, sky, fog and water.
    #[serde(default)]
    pub environment: crate::env::Environment,
    /// Metal spots placed by hand. Empty means "propose them from the graph",
    /// which is what a fresh project does; once you move one, the whole list
    /// is yours and the proposer stops overwriting it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spots: Vec<crate::zk::MetalSpot>,
}

fn default_version() -> String {
    "1.0".into()
}
fn default_hardness() -> f64 {
    350.0
}
fn default_gravity() -> f64 {
    100.0
}
fn default_max_metal() -> f64 {
    1.7
}
fn default_extractor_radius() -> f64 {
    100.0
}
fn default_mex_sym() -> String {
    "rot180".into()
}

impl Default for Project {
    /// Defaults follow the measured shipped map, not the blueprint's
    /// `maxMetal = 0.02, extractorRadius = 500`, which no real map uses.
    fn default() -> Self {
        Project {
            name: "Untitled".into(),
            description: String::new(),
            author: String::new(),
            version: default_version(),
            units_x: 12,
            units_y: 12,
            seed: 20250815,
            min_height: -80.0,
            max_height: 420.0,
            hardness: default_hardness(),
            gravity: default_gravity(),
            tidal: 0.0,
            max_metal: default_max_metal(),
            extractor_radius: default_extractor_radius(),
            mex_sym: default_mex_sym(),
            max_depth: None,
            materials: crate::material::MaterialSet::default(),
            start_boxes: None,
            environment: crate::env::Environment::default(),
            spots: Vec::new(),
        }
    }
}

impl Project {
    pub fn height_range(&self) -> f64 {
        self.max_height - self.min_height
    }

    /// The archive's file stem, carrying the version.
    ///
    /// Zero-K identifies a map by the name in `mapinfo.lua` and refuses an
    /// upload when that name already exists, so the version has to be visible
    /// and changeable. Keeping it in the file name too means a re-bake does not
    /// silently overwrite the archive you already published.
    pub fn archive_stem(&self) -> String {
        let v: String = self
            .version
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect();
        if v.is_empty() {
            self.short_name()
        } else {
            format!("{}-v{}", self.short_name(), v)
        }
    }

    /// A filesystem- and engine-safe short name.
    pub fn short_name(&self) -> String {
        let s: String = self
            .name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if s.is_empty() {
            "map".into()
        } else {
            s
        }
    }
}

impl Project {
    /// Read from JSON through `Value`.
    ///
    /// Not `serde_json::from_str` on purpose: its fast float parser is off by
    /// one ulp on some 17-digit decimals, while `Value::as_f64` under
    /// `arbitrary_precision` goes through Rust's correctly-rounded parser. A
    /// single ulp in `minHeight` shifts the whole height range.
    pub fn from_json(v: &serde_json::Value) -> Project {
        let d = Project::default();
        let s = |k: &str, def: &str| {
            v.get(k)
                .and_then(serde_json::Value::as_str)
                .unwrap_or(def)
                .to_string()
        };
        let f = |k: &str, def: f64| v.get(k).and_then(serde_json::Value::as_f64).unwrap_or(def);
        Project {
            name: s("name", &d.name),
            description: s("description", ""),
            author: s("author", ""),
            version: s("version", &d.version),
            units_x: f("unitsX", f64::from(d.units_x)) as u32,
            units_y: f("unitsY", f64::from(d.units_y)) as u32,
            seed: f("seed", d.seed as f64) as i64,
            min_height: f("minHeight", d.min_height),
            max_height: f("maxHeight", d.max_height),
            hardness: f("hardness", d.hardness),
            gravity: f("gravity", d.gravity),
            tidal: f("tidal", d.tidal),
            max_metal: f("maxMetal", d.max_metal),
            extractor_radius: f("extractorRadius", d.extractor_radius),
            mex_sym: s("mexSym", &d.mex_sym),
            // Absent means uncapped, which is what every project written
            // before this field said.
            max_depth: v.get("maxDepth").and_then(serde_json::Value::as_f64),
            materials: v
                .get("materials")
                .and_then(|m| serde_json::from_value(m.clone()).ok())
                .unwrap_or_default(),
            // Through `Value` like the spots, and for the same reason: a box
            // corner is elmos.
            start_boxes: v
                .get("startBoxes")
                .and_then(|b| serde_json::from_value(b.clone()).ok()),
            environment: v
                .get("environment")
                .and_then(|e| serde_json::from_value(e.clone()).ok())
                .unwrap_or_default(),
            // Spots go through `Value` like everything else: their
            // coordinates are elmos and a one-ulp shift moves a mex off the
            // 16-elmo grid.
            spots: v
                .get("spots")
                .and_then(|sp| serde_json::from_value(sp.clone()).ok())
                .unwrap_or_default(),
        }
    }
}

/// A render request. Nodes never see pixels in their parameters — every
/// scale-dependent value is authored in elmos and converted here.
///
/// Two different lengths live here and confusing them ships a broken map.
/// [`Context::elmos`] is the **evaluation domain**: the graph is evaluated on a
/// square lattice, so one number scales every node parameter. `elmos_x` and
/// `elmos_y` are the **world**, and anything that produces a coordinate the
/// engine will read — a metal spot, a start box, a metalmap pixel — must use
/// those. On a 16×8 map they differ by a factor of two, and using `elmos` for
/// the Z axis puts half the metal layout past the south edge of the map.
/// Not `Copy`: it carries the raster store, which is shared rather than
/// duplicated. Everything already passes it by reference.
#[derive(Clone, Debug)]
pub struct Context {
    pub res: usize,
    /// The square evaluation domain, in elmos. Equal to `elmos_x`; on a
    /// non-square map the Y axis is stretched onto it, which is gap 11.
    pub elmos: f64,
    /// True world width in elmos, `unitsX * 512`.
    pub elmos_x: f64,
    /// True world depth in elmos, `unitsY * 512`.
    pub elmos_y: f64,
    pub seed: i32,
    pub height_range: f64,
    /// The waterline as a normalised field value, `-minHeight / range`.
    ///
    /// Carried here for the same reason `elmos_x` is: a node that paints water
    /// has to paint it where the engine will put it, and a node cannot be
    /// trusted to hold a copy of a project setting in sync.
    pub sea_t: f64,
    pub px_per_elmo: f64,
    pub elmo_per_px: f64,
    /// Rasters the graph can read — imported terrain, and later brush layers.
    /// Empty for a purely procedural project, which is every project that
    /// existed before importing did.
    pub rasters: std::sync::Arc<crate::raster::Rasters>,
}

impl Context {
    pub fn new(project: &Project, res: usize) -> Context {
        Context::with_rasters(project, res, Default::default())
    }

    /// As [`Context::new`], with rasters the graph's `import` nodes can read.
    pub fn with_rasters(
        project: &Project,
        res: usize,
        rasters: std::sync::Arc<crate::raster::Rasters>,
    ) -> Context {
        let elmos = f64::from(project.units_x * 512);
        Context {
            res,
            elmos,
            elmos_x: elmos,
            elmos_y: f64::from(project.units_y * 512),
            seed: crate::rng::to_i32(project.seed as f64),
            height_range: project.max_height - project.min_height,
            sea_t: water_level_t(project.min_height, project.max_height),
            px_per_elmo: (res - 1) as f64 / elmos,
            elmo_per_px: elmos / (res - 1) as f64,
            rasters,
        }
    }

    /// Lattice samples per elmo along X. Distinct from `px_per_elmo` only on a
    /// non-square map, which is exactly when it matters.
    pub fn px_per_elmo_x(&self) -> f64 {
        (self.res - 1) as f64 / self.elmos_x
    }
    pub fn px_per_elmo_y(&self) -> f64 {
        (self.res - 1) as f64 / self.elmos_y
    }
    pub fn elmo_per_px_x(&self) -> f64 {
        self.elmos_x / (self.res - 1) as f64
    }
    pub fn elmo_per_px_y(&self) -> f64 {
        self.elmos_y / (self.res - 1) as f64
    }
    /// The world's Z extent as a multiple of its X extent.
    ///
    /// Every aspect correction in the node library is written as a factor on
    /// the Y axis relative to X, and this is that factor. On a square map it
    /// is exactly 1.0 — both extents are `units * 512` — so multiplying by it
    /// changes no bits and the arithmetic the golden suite asserts is the
    /// arithmetic it always was. That is the whole reason the corrections are
    /// phrased this way round rather than as a pair of independent scales.
    pub fn aspect_y(&self) -> f64 {
        self.elmos_y / self.elmos_x
    }

    /// Whether the world is square. Rotational and diagonal symmetry operators
    /// are only defined when it is.
    pub fn square_world(&self) -> bool {
        self.elmos_x == self.elmos_y
    }
}

/// Height 0 elmos is the waterline, so the normalised sea level is derived,
/// never an independent setting.
pub fn water_level_t(min_height: f64, max_height: f64) -> f64 {
    let range = max_height - min_height;
    if range <= 0.0 {
        return 0.0;
    }
    let t = (-min_height / range).clamp(0.0, 1.0);
    // A zero waterline must be positive zero: minHeight 0 yields -0.0 here,
    // which is a different bit pattern and would fail golden comparison.
    if t == 0.0 {
        0.0
    } else {
        t
    }
}

/// Move the waterline while preserving the total vertical range.
pub fn set_water_level_t(project: &mut Project, t: f64) -> f64 {
    let t = t.clamp(0.0, 0.98);
    let mut range = project.max_height - project.min_height;
    if range <= 0.0 {
        range = 500.0;
    }
    project.min_height = (-t * range).round();
    project.max_height = (project.min_height + range).round();
    water_level_t(project.min_height, project.max_height)
}

/// IceXuick's method stated forwards: a submerged fraction and a total vertical
/// range produce the mapconv `-n` / `-x` pair.
pub fn height_range_for(submerged_fraction: f64, total_range: f64) -> (f64, f64) {
    let min = (-total_range * submerged_fraction).round();
    (min, (min + total_range).round())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_name_carries_the_version() {
        let p = Project {
            name: "Ridge Valley".into(),
            version: "1.2".into(),
            ..Default::default()
        };
        assert_eq!(p.short_name(), "RidgeValley");
        assert_eq!(p.archive_stem(), "RidgeValley-v1.2");
        // Bumping the version changes the file, so a re-bake cannot overwrite
        // a published archive.
        let q = Project {
            version: "1.3".into(),
            ..p.clone()
        };
        assert_ne!(p.archive_stem(), q.archive_stem());
    }

    #[test]
    fn the_default_output_directory_is_absolute_and_not_the_executables() {
        let d = default_output_dir();
        assert!(d.is_absolute(), "{d:?}");
        assert!(d.ends_with("Springen"), "{d:?}");
    }

    #[test]
    fn waterline_is_derived_from_the_height_range() {
        assert!((water_level_t(-60.0, 440.0) - 0.12).abs() < 1e-12);
        // IceXuick's worked example: 31.4% submerged over a 600 range.
        let (n, x) = height_range_for(0.3133, 600.0);
        assert_eq!((n, x), (-188.0, 412.0));
        assert!((water_level_t(n, x) - 0.31333333333333335).abs() < 1e-12);
    }
}

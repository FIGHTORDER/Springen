//! Sky, sun, fog and water: the `atmosphere`, `lighting` and `water` blocks.
//!
//! These were hard-coded constants in the emitter, which meant every Springen
//! map was lit at the same hour of the same overcast day. They are settings
//! now, with presets, and the viewport reads the same numbers the engine will
//! — a sun direction the tool disagrees with is the waterline bug again in a
//! different costume.
//!
//! What is *not* here is the skybox itself: `atmosphere.skyBox` names a DDS
//! cubemap, and generating one is scoped but not built
//! rather than half-built here. Without it Spring renders its own sky from
//! `skyColor`, `sunColor` and the cloud settings below, which is what these
//! presets are tuned for.

use serde::{Deserialize, Serialize};

use crate::fdlibm;

fn deg(d: f64) -> f64 {
    d * std::f64::consts::PI / 180.0
}

/// Everything about a map's light and air.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    /// The preset this started from, kept so the UI can show it and so a
    /// hand-tuned map does not silently claim to be a stock one.
    #[serde(default = "default_preset")]
    pub preset: String,
    /// Sun position in degrees. Authored as a compass bearing and a height
    /// above the horizon because that is how anyone thinks about a sun;
    /// `sun_dir` turns it into the vector mapinfo wants.
    pub sun_azimuth: f64,
    pub sun_elevation: f64,
    pub sun_color: [f64; 3],
    pub sky_color: [f64; 3],
    pub cloud_color: [f64; 3],
    pub cloud_density: f64,
    pub fog_color: [f64; 3],
    pub fog_start: f64,
    pub fog_end: f64,
    pub ground_ambient: [f64; 3],
    pub ground_diffuse: [f64; 3],
    pub ground_specular: [f64; 3],
    /// How dark shadows go, 0..1.
    pub shadow_density: f64,
    pub water_base: [f64; 3],
    pub water_min: [f64; 3],
    pub water_surface: [f64; 3],
    /// Per-channel absorption with depth. Red goes first in real water, which
    /// is why these three are not equal.
    pub water_absorb: [f64; 3],
    pub min_wind: f64,
    pub max_wind: f64,
    /// A DDS cubemap for `atmosphere.skyBox`, if one is supplied. Springen
    /// does not generate these yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skybox: Option<String>,
}

fn default_preset() -> String {
    "temperate".into()
}

impl Default for Environment {
    fn default() -> Self {
        preset("temperate").expect("the default preset exists")
    }
}

impl Environment {
    /// The unit vector mapinfo's `lighting.sunDir` wants.
    ///
    /// Azimuth is a compass bearing: 0 is north, 90 is east, measured the way
    /// the engine's own Z axis runs.
    pub fn sun_dir(&self) -> [f64; 3] {
        let (a, e) = (
            deg(self.sun_azimuth),
            deg(self.sun_elevation.clamp(1.0, 89.0)),
        );
        let ce = fdlibm::cos(e);
        [fdlibm::sin(a) * ce, fdlibm::sin(e), -fdlibm::cos(a) * ce]
    }
}

/// Named starting points, tuned for Spring's own procedural sky.
pub static PRESETS: &[(&str, &str)] = &[
    ("temperate", "Overcast noon. The neutral default."),
    ("arid", "High desert sun, dusty air, shallow warm water."),
    ("arctic", "Low blue sun, hard shadows, cold clear water."),
    ("volcanic", "Heavy red sky, dense low fog, dark water."),
    ("dusk", "Low amber sun and a long fog fade."),
];

pub fn preset_keys() -> Vec<&'static str> {
    PRESETS.iter().map(|(k, _)| *k).collect()
}

/// Build a preset by name.
pub fn preset(key: &str) -> Option<Environment> {
    let base = |k: &str| Environment {
        preset: k.to_string(),
        sun_azimuth: 315.0,
        sun_elevation: 45.0,
        sun_color: [0.6, 0.6, 0.55],
        sky_color: [0.35, 0.5, 0.66],
        cloud_color: [0.9, 0.9, 0.9],
        cloud_density: 0.5,
        fog_color: [0.7, 0.75, 0.8],
        fog_start: 0.4,
        fog_end: 1.0,
        ground_ambient: [0.55, 0.57, 0.6],
        ground_diffuse: [0.8, 0.8, 0.75],
        ground_specular: [0.1, 0.1, 0.1],
        shadow_density: 0.72,
        water_base: [0.4, 0.7, 0.8],
        water_min: [0.1, 0.2, 0.3],
        water_surface: [0.15, 0.3, 0.45],
        water_absorb: [0.0007, 0.00045, 0.0003],
        min_wind: 5.0,
        max_wind: 25.0,
        skybox: None,
    };
    let mut e = base(key);
    match key {
        "temperate" => {}
        "arid" => {
            e.sun_azimuth = 200.0;
            e.sun_elevation = 62.0;
            e.sun_color = [1.0, 0.94, 0.78];
            e.sky_color = [0.55, 0.62, 0.72];
            e.cloud_color = [0.95, 0.92, 0.85];
            e.cloud_density = 0.18;
            e.fog_color = [0.86, 0.79, 0.63];
            e.fog_start = 0.55;
            e.ground_ambient = [0.62, 0.58, 0.5];
            e.ground_diffuse = [0.95, 0.9, 0.78];
            e.shadow_density = 0.82;
            e.water_base = [0.35, 0.62, 0.62];
            e.water_surface = [0.22, 0.4, 0.42];
            e.max_wind = 32.0;
        }
        "arctic" => {
            e.sun_azimuth = 160.0;
            e.sun_elevation = 18.0;
            e.sun_color = [0.82, 0.87, 1.0];
            e.sky_color = [0.42, 0.56, 0.78];
            e.cloud_color = [0.85, 0.89, 0.96];
            e.cloud_density = 0.35;
            e.fog_color = [0.78, 0.84, 0.92];
            e.ground_ambient = [0.5, 0.56, 0.68];
            e.ground_diffuse = [0.78, 0.84, 0.95];
            e.ground_specular = [0.2, 0.22, 0.26];
            e.shadow_density = 0.88;
            e.water_base = [0.22, 0.44, 0.58];
            e.water_min = [0.05, 0.12, 0.2];
            e.water_surface = [0.1, 0.24, 0.36];
            e.water_absorb = [0.0009, 0.0005, 0.00028];
            e.min_wind = 8.0;
            e.max_wind = 34.0;
        }
        "volcanic" => {
            e.sun_azimuth = 285.0;
            e.sun_elevation = 26.0;
            e.sun_color = [1.0, 0.62, 0.4];
            e.sky_color = [0.28, 0.17, 0.16];
            e.cloud_color = [0.42, 0.3, 0.28];
            e.cloud_density = 0.8;
            e.fog_color = [0.45, 0.24, 0.18];
            e.fog_start = 0.1;
            e.fog_end = 0.85;
            e.ground_ambient = [0.4, 0.32, 0.3];
            e.ground_diffuse = [0.9, 0.62, 0.45];
            e.ground_specular = [0.16, 0.1, 0.08];
            e.shadow_density = 0.9;
            e.water_base = [0.3, 0.2, 0.18];
            e.water_min = [0.06, 0.04, 0.04];
            e.water_surface = [0.2, 0.11, 0.1];
            e.min_wind = 2.0;
            e.max_wind = 14.0;
        }
        "dusk" => {
            e.sun_azimuth = 260.0;
            e.sun_elevation = 8.0;
            e.sun_color = [1.0, 0.72, 0.46];
            e.sky_color = [0.3, 0.34, 0.52];
            e.cloud_color = [0.85, 0.66, 0.55];
            e.cloud_density = 0.45;
            e.fog_color = [0.72, 0.55, 0.45];
            e.fog_start = 0.15;
            e.ground_ambient = [0.42, 0.42, 0.52];
            e.ground_diffuse = [0.95, 0.75, 0.55];
            e.shadow_density = 0.86;
            e.water_base = [0.3, 0.42, 0.58];
            e.water_surface = [0.2, 0.24, 0.38];
        }
        _ => return None,
    }
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_builds_and_names_itself() {
        for (k, about) in PRESETS {
            let e = preset(k).unwrap_or_else(|| panic!("{k} is listed but does not build"));
            assert_eq!(&e.preset, k);
            assert!(!about.is_empty());
        }
        assert!(preset("nonsense").is_none());
    }

    #[test]
    fn the_sun_is_a_unit_vector_above_the_horizon() {
        for (k, _) in PRESETS {
            let e = preset(k).unwrap();
            let d = e.sun_dir();
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-9, "{k}: sunDir length {len}");
            // Below the horizon means the map is lit from underneath.
            assert!(d[1] > 0.0, "{k}: the sun is below the horizon");
        }
    }

    #[test]
    fn azimuth_reads_as_a_compass_bearing() {
        let mut e = preset("temperate").unwrap();
        e.sun_elevation = 0.0; // clamped to 1 degree, near enough to flat
        e.sun_azimuth = 90.0;
        let east = e.sun_dir();
        assert!(east[0] > 0.99, "90 degrees must point east, got {east:?}");
        e.sun_azimuth = 0.0;
        let north = e.sun_dir();
        assert!(
            north[2] < -0.99,
            "0 degrees must point north, got {north:?}"
        );
    }

    #[test]
    fn water_absorbs_red_fastest() {
        // Not a style choice: it is why deep water reads blue.
        for (k, _) in PRESETS {
            let a = preset(k).unwrap().water_absorb;
            assert!(a[0] > a[1] && a[1] > a[2], "{k}: absorb {a:?}");
        }
    }
}

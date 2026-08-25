//! Spring/Recoil size math — the one piece of engine truth everything else
//! hangs off.
//!
//! `mapx`/`mapy` live in 8-elmo heightmap squares and must divide by 128. One
//! lobby-facing size unit is 512 elmos = 64 squares, so **the unit on each axis
//! has to be even**: the real quantum is 1024 elmos, not 512. 8×8 and 12×16 are
//! legal; 9×9 and 15×10 are not.

use serde::{Deserialize, Serialize};

pub const SQUARE_SIZE: u32 = 8;
pub const TEXELS_PER_SQUARE: u32 = 8;
pub const TILE_SIZE: u32 = 32;
/// 32×32 DXT1 with mips down to 4×4: 512 + 128 + 32 + 8.
pub const TILE_BYTES: u64 = 680;
/// 1024² DXT1 mip chain stopping at 4×4, not 1×1.
pub const MINIMAP_BYTES: u64 = 699_048;
pub const MIN_UNITS: u32 = 2;
pub const MAX_UNITS: u32 = 64;

/// Full derived layer manifest for a map size given in 512-elmo units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Derived {
    pub valid: bool,
    pub units_x: u32,
    pub units_y: u32,
    pub elmos_x: u32,
    pub elmos_y: u32,
    pub mapx: u32,
    pub mapy: u32,
    /// Vertex lattice, hence the +1. A 1024² heightmap is wrong for a 16×16 map.
    pub height_w: u32,
    pub height_h: u32,
    pub tex_w: u32,
    pub tex_h: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub tile_count: u32,
    pub smt_worst_case: u64,
    pub metal_w: u32,
    pub metal_h: u32,
    pub type_w: u32,
    pub type_h: u32,
    pub grass_w: u32,
    pub grass_h: u32,
    pub minimap_w: u32,
    pub minimap_h: u32,
    pub big_tex_x: u32,
    pub big_tex_y: u32,
}

pub fn valid_units(u: u32) -> bool {
    (MIN_UNITS..=MAX_UNITS).contains(&u) && u.is_multiple_of(2)
}

pub fn nearest_valid_units(u: f64) -> u32 {
    let r = ((u / 2.0).round() * 2.0) as i64;
    r.clamp(MIN_UNITS as i64, MAX_UNITS as i64) as u32
}

/// Why a size was refused, phrased the way the UI states it.
pub fn size_rejection(units_x: u32, units_y: u32) -> Option<String> {
    for (axis, u) in [("X", units_x), ("Y", units_y)] {
        if !u.is_multiple_of(2) {
            return Some(format!(
                "Size unit {u} on {axis} is odd. mapx must divide by 128 and one unit is only 64 squares, so the size unit has to be even."
            ));
        }
        if !(MIN_UNITS..=MAX_UNITS).contains(&u) {
            return Some(format!(
                "Size unit {u} on {axis} is outside the engine range {MIN_UNITS}–{MAX_UNITS}."
            ));
        }
    }
    None
}

pub fn derive(units_x: u32, units_y: u32) -> Derived {
    let ok = valid_units(units_x) && valid_units(units_y);
    let mapx = units_x * 64;
    let mapy = units_y * 64;
    let tiles_x = mapx / 4;
    let tiles_y = mapy / 4;
    Derived {
        valid: ok,
        units_x,
        units_y,
        elmos_x: units_x * 512,
        elmos_y: units_y * 512,
        mapx,
        mapy,
        height_w: mapx + 1,
        height_h: mapy + 1,
        tex_w: mapx * 8,
        tex_h: mapy * 8,
        tiles_x,
        tiles_y,
        tile_count: tiles_x * tiles_y,
        smt_worst_case: u64::from(tiles_x) * u64::from(tiles_y) * TILE_BYTES,
        metal_w: mapx / 2,
        metal_h: mapy / 2,
        type_w: mapx / 2,
        type_h: mapy / 2,
        grass_w: mapx / 4,
        grass_h: mapy / 4,
        minimap_w: 1024,
        minimap_h: 1024,
        big_tex_x: mapx / 128,
        big_tex_y: mapy / 128,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_units_are_refused_with_a_reason() {
        assert!(!valid_units(9));
        assert!(valid_units(10));
        assert!(size_rejection(9, 10).unwrap().contains("divide by 128"));
        assert!(size_rejection(10, 10).is_none());
    }

    #[test]
    fn derived_layer_table_matches_the_reference() {
        // Mapdev:metal lists smu 6 -> 256 px; its own width/2 formula gives 192.
        assert_eq!(derive(6, 6).metal_w, 192);
        let d = derive(10, 10);
        assert_eq!((d.mapx, d.mapy), (640, 640));
        assert_eq!((d.height_w, d.height_h), (641, 641));
        assert_eq!(d.tex_w, 5120);
        assert_eq!(d.metal_w, 320);
        assert_eq!(d.grass_w, 160);
        assert_eq!(d.tile_count, 25600);
        let d16 = derive(16, 16);
        assert_eq!(d16.height_w, 1025);
        assert_eq!(d16.tex_w, 8192);
    }

    #[test]
    fn minimap_constant_is_the_mip_chain_sum() {
        let mut total = 0u64;
        let mut s = 1024u64;
        while s >= 4 {
            total += (s / 4) * (s / 4) * 8;
            s /= 2;
        }
        assert_eq!(total, MINIMAP_BYTES);
    }
}

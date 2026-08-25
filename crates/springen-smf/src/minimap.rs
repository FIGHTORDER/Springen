//! Minimap block.
//!
//! Raw DXT1 at 1024², mipped down to 4×4 and **not** to 1×1. That is where the
//! 699048 constant comes from:
//! `sum over s in {1024, 512, ..., 4} of (s/4)² * 8`.

use crate::bc1;
use springen_core::spring::MINIMAP_BYTES;

pub const SIZE: usize = 1024;

/// Encode a 1024² RGB image into the exact minimap block.
pub fn encode(rgb: &[u8]) -> Vec<u8> {
    assert_eq!(rgb.len(), SIZE * SIZE * 3, "minimap source must be 1024²");
    let mut out = Vec::with_capacity(MINIMAP_BYTES as usize);
    let mut level = rgb.to_vec();
    let mut size = SIZE;
    loop {
        bc1::encode_image(&level, size, size, &mut out);
        if size == 4 {
            break;
        }
        level = bc1::halve(&level, size, size);
        size /= 2;
    }
    debug_assert_eq!(out.len(), MINIMAP_BYTES as usize);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_exactly_the_documented_constant() {
        let rgb = vec![100u8; SIZE * SIZE * 3];
        assert_eq!(encode(&rgb).len(), MINIMAP_BYTES as usize);
        assert_eq!(MINIMAP_BYTES, 699_048);
    }

    #[test]
    fn stopping_at_one_would_be_wrong() {
        // The chain that stops at 1x1 is longer, and the engine rejects it.
        let mut to_four = 0u64;
        let mut to_one = 0u64;
        let mut s = SIZE as u64;
        while s >= 1 {
            let bytes = (s.div_ceil(4)) * (s.div_ceil(4)) * 8;
            if s >= 4 {
                to_four += bytes;
            }
            to_one += bytes;
            s /= 2;
        }
        assert_eq!(to_four, MINIMAP_BYTES);
        assert!(to_one > to_four);
    }
}

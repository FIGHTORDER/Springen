//! SMT tile store.
//!
//! Magic `"spring tilefile\0"`, a 32-byte header, then `numTiles` tiles of
//! **exactly 680 bytes**: a 32×32 DXT1 image with four mip levels
//! (32, 16, 8, 4), i.e. 512 + 128 + 32 + 8. Measured against a shipped file:
//! `32 + 25553 * 680 = 17376072`, no padding.

use std::collections::HashMap;

use rayon::prelude::*;
use std::io::{self, Write};

use crate::bc1;

pub const MAGIC: &[u8; 16] = b"spring tilefile\0";
pub const TILE_SIZE: usize = 32;
pub const TILE_BYTES: usize = 680;
pub const COMPRESSION_DXT1: i32 = 1;

/// Compress one 32×32 RGB tile into its 680-byte mip chain.
pub fn encode_tile(rgb: &[u8]) -> Vec<u8> {
    debug_assert_eq!(rgb.len(), TILE_SIZE * TILE_SIZE * 3);
    let mut out = Vec::with_capacity(TILE_BYTES);
    let mut level = rgb.to_vec();
    let mut size = TILE_SIZE;
    while size >= 4 {
        bc1::encode_image(&level, size, size, &mut out);
        if size == 4 {
            break;
        }
        level = bc1::halve(&level, size, size);
        size /= 2;
    }
    debug_assert_eq!(out.len(), TILE_BYTES);
    out
}

/// The deduplicated tile pool plus the per-slot index the SMF carries.
pub struct TileSet {
    pub data: Vec<u8>,
    pub count: u32,
    pub index: Vec<u32>,
    pub slots: usize,
}

impl TileSet {
    /// Fraction of slots that reused an existing tile.
    ///
    /// Worth reporting honestly: measured on a real hand-made map this is
    /// 0.2% (25600 slots, 25553 stored). Once a texture is rendered rather
    /// than hand-tiled, dedup effectively vanishes for everyone, so a 17 MB
    /// SMT for a 10×10 is normal and not a bug.
    pub fn dedup_ratio(&self) -> f64 {
        if self.slots == 0 {
            return 0.0;
        }
        1.0 - self.count as f64 / self.slots as f64
    }

    pub fn byte_len(&self) -> usize {
        32 + self.data.len()
    }
}

/// Build the tile pool. `tile_rgb` is called once per slot and must return a
/// 32×32 RGB buffer, which keeps the caller free to sample the diffuse
/// lazily instead of materialising it — a 32×32 map's diffuse is 16384².
/// Sampling and BC1-encoding a tile is the bulk of a bake and every tile is
/// independent, so a row of tiles is encoded in parallel. Deduplication stays
/// sequential and in scan order: the index a tile gets is its position in the
/// order tiles were first seen, and reordering that would change the file.
pub fn build<F>(tiles_x: usize, tiles_y: usize, tile_rgb: F) -> TileSet
where
    F: Fn(usize, usize) -> Vec<u8> + Sync,
{
    let slots = tiles_x * tiles_y;
    let mut data: Vec<u8> = Vec::with_capacity(slots * TILE_BYTES / 4);
    let mut index = Vec::with_capacity(slots);
    let mut seen: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut count = 0u32;
    let mut row: Vec<Vec<u8>> = Vec::with_capacity(tiles_x);
    for ty in 0..tiles_y {
        row.clear();
        (0..tiles_x)
            .into_par_iter()
            .map(|tx| encode_tile(&tile_rgb(tx, ty)))
            .collect_into_vec(&mut row);
        for enc in row.drain(..) {
            match seen.get(&enc) {
                Some(i) => index.push(*i),
                None => {
                    seen.insert(enc.clone(), count);
                    data.extend_from_slice(&enc);
                    index.push(count);
                    count += 1;
                }
            }
        }
    }
    TileSet {
        data,
        count,
        index,
        slots,
    }
}

/// A tile store read back from a file: how many tiles, and their raw bytes.
pub struct TileStore {
    pub count: usize,
    pub tile_size: usize,
    /// `count * TILE_BYTES` of DXT1 mip chains.
    pub data: Vec<u8>,
}

impl TileStore {
    /// Decode one tile's top mip level to a `TILE_SIZE²` RGB image.
    ///
    /// Only the top level: the rest of the 680 bytes are mips the engine
    /// generates the same way we do, and nothing here wants them.
    pub fn tile_rgb(&self, index: usize) -> Option<Vec<u8>> {
        let start = index.checked_mul(TILE_BYTES)?;
        let top = self.data.get(start..start + TILE_SIZE * TILE_SIZE / 2)?;
        let mut out = vec![0u8; TILE_SIZE * TILE_SIZE * 3];
        let blocks = TILE_SIZE / 4;
        for by in 0..blocks {
            for bx in 0..blocks {
                let o = (by * blocks + bx) * 8;
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&top[o..o + 8]);
                let texels = crate::bc1::decode_block(&b8);
                for ty in 0..4 {
                    for tx in 0..4 {
                        let (x, y) = (bx * 4 + tx, by * 4 + ty);
                        let d = (y * TILE_SIZE + x) * 3;
                        out[d..d + 3].copy_from_slice(&texels[ty * 4 + tx]);
                    }
                }
            }
        }
        Some(out)
    }
}

/// Read a `.smt`.
///
/// Rejects anything that is not DXT1 at 32×32 rather than guessing: the format
/// allows other combinations, nothing in the wild uses them, and a silent
/// misread would put garbage on a map.
pub fn read(bytes: &[u8]) -> Result<TileStore, String> {
    if bytes.len() < 32 {
        return Err("SMT is too short to hold a header.".into());
    }
    if &bytes[0..16] != MAGIC {
        return Err("Not an SMT: the magic does not say \"spring tilefile\".".into());
    }
    let i32_at =
        |o: usize| i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let version = i32_at(16);
    let count = i32_at(20);
    let tile_size = i32_at(24);
    let compression = i32_at(28);
    if version != 1 {
        return Err(format!("SMT version {version}, and only 1 is understood."));
    }
    if tile_size != TILE_SIZE as i32 {
        return Err(format!(
            "SMT tiles are {tile_size}px, and only {TILE_SIZE} is understood."
        ));
    }
    if compression != COMPRESSION_DXT1 {
        return Err(format!(
            "SMT compression type {compression}, and only DXT1 ({COMPRESSION_DXT1}) is understood."
        ));
    }
    if count < 0 {
        return Err("SMT declares a negative tile count.".into());
    }
    let count = count as usize;
    let want = count * TILE_BYTES;
    let data = bytes.get(32..32 + want).ok_or_else(|| {
        format!(
            "SMT declares {count} tiles ({want} bytes) but only {} follow the header.",
            bytes.len().saturating_sub(32)
        )
    })?;
    Ok(TileStore {
        count,
        tile_size: TILE_SIZE,
        data: data.to_vec(),
    })
}

pub fn write<W: Write>(w: &mut W, set: &TileSet) -> io::Result<()> {
    w.write_all(MAGIC)?;
    w.write_all(&1i32.to_le_bytes())?;
    w.write_all(&(set.count as i32).to_le_bytes())?;
    w.write_all(&(TILE_SIZE as i32).to_le_bytes())?;
    w.write_all(&COMPRESSION_DXT1.to_le_bytes())?;
    w.write_all(&set.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_tile(v: u8) -> Vec<u8> {
        vec![v; TILE_SIZE * TILE_SIZE * 3]
    }

    #[test]
    fn a_tile_is_exactly_680_bytes() {
        assert_eq!(encode_tile(&flat_tile(120)).len(), TILE_BYTES);
        // 512 + 128 + 32 + 8, the mip chain stopping at 4x4.
        assert_eq!(512 + 128 + 32 + 8, TILE_BYTES);
    }

    #[test]
    fn identical_tiles_are_stored_once() {
        let set = build(4, 4, |_, _| flat_tile(90));
        assert_eq!(set.slots, 16);
        assert_eq!(set.count, 1);
        assert_eq!(set.index, vec![0; 16]);
        assert!((set.dedup_ratio() - 15.0 / 16.0).abs() < 1e-12);
    }

    #[test]
    fn distinct_tiles_are_all_stored() {
        let set = build(4, 4, |tx, ty| flat_tile((tx * 40 + ty * 7 + 3) as u8));
        assert_eq!(set.count as usize, set.slots);
        assert_eq!(set.dedup_ratio(), 0.0);
    }

    #[test]
    fn file_size_is_header_plus_tiles_with_no_padding() {
        let set = build(8, 8, |tx, ty| flat_tile((tx * 17 + ty * 5) as u8));
        let mut out = Vec::new();
        write(&mut out, &set).unwrap();
        assert_eq!(out.len(), 32 + set.count as usize * TILE_BYTES);
        assert_eq!(&out[..16], MAGIC);
        assert_eq!(
            i32::from_le_bytes(out[20..24].try_into().unwrap()),
            set.count as i32
        );
        assert_eq!(i32::from_le_bytes(out[24..28].try_into().unwrap()), 32);
        assert_eq!(i32::from_le_bytes(out[28..32].try_into().unwrap()), 1);
    }
}

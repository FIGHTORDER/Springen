//! SMF writer and reader.
//!
//! Magic `"spring map file\0"`, an 80-byte header carrying **six** offsets —
//! the features offset is the one that is easy to miss — then any number of
//! extra headers, then the blocks.
//!
//! The header field order is *not* the physical block order. A real file
//! writes the grass map first, immediately after the header and extra header,
//! and the rest in a different sequence again. A reader must follow offsets and
//! never assume order; a writer may pick any order it likes. This one writes in
//! the measured physical order so its output sits next to a real file cleanly.

use std::io::{self, Write};

use springen_core::spring::{Derived, MINIMAP_BYTES};

pub const MAGIC: &[u8; 16] = b"spring map file\0";
pub const HEADER_BYTES: usize = 80;
pub const EXTRA_GRASS_BYTES: usize = 12;
pub const EXTRA_TYPE_GRASS: i32 = 1;

/// One entry in the tile index header.
#[derive(Clone, Debug)]
pub struct SmtRef {
    pub file_name: String,
    pub tile_count: u32,
}

/// Everything the writer needs, already at the right resolutions.
pub struct Layers<'a> {
    /// `(mapx + 1) * (mapy + 1)` unsigned shorts — a vertex lattice, hence the +1.
    pub heightmap: &'a [u16],
    /// `(mapx / 2)²` bytes, each an index into `mapinfo.terrainTypes`.
    pub typemap: &'a [u8],
    /// `(mapx / 2)²` bytes, read from the red channel of the source image.
    pub metalmap: &'a [u8],
    /// `(mapx / 4)²` bytes. Optional: without it no extra header is written.
    pub grassmap: Option<&'a [u8]>,
    /// Exactly 699048 bytes of DXT1.
    pub minimap: &'a [u8],
    /// `(mapx / 4) * (mapy / 4)` slots indexing into the SMT tile pool.
    pub tile_index: &'a [u32],
    pub smt_files: &'a [SmtRef],
}

fn tile_index_block_len(layers: &Layers) -> usize {
    let mut n = 8; // numTileFiles, totalTiles
    for f in layers.smt_files {
        n += 4 + f.file_name.len() + 1;
    }
    n + layers.tile_index.len() * 4
}

/// Byte length of the file the writer will produce.
pub fn file_len(derived: &Derived, layers: &Layers) -> usize {
    let mut n = HEADER_BYTES;
    if layers.grassmap.is_some() {
        n += EXTRA_GRASS_BYTES;
        n += (derived.grass_w * derived.grass_h) as usize;
    }
    n += (derived.height_w * derived.height_h) as usize * 2;
    n += (derived.type_w * derived.type_h) as usize;
    n += MINIMAP_BYTES as usize;
    n += (derived.metal_w * derived.metal_h) as usize;
    n += tile_index_block_len(layers);
    n + 8 // empty features block
}

fn check(derived: &Derived, layers: &Layers) -> io::Result<()> {
    let want_h = (derived.height_w * derived.height_h) as usize;
    if layers.heightmap.len() != want_h {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "heightmap is {} samples; the vertex lattice for mapx {} needs {} x {} = {}",
                layers.heightmap.len(),
                derived.mapx,
                derived.height_w,
                derived.height_h,
                want_h
            ),
        ));
    }
    let want_info = (derived.metal_w * derived.metal_h) as usize;
    for (name, got) in [
        ("typemap", layers.typemap.len()),
        ("metalmap", layers.metalmap.len()),
    ] {
        if got != want_info {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{name} is {got} bytes; info maps are mapx/2 squared = {} x {}",
                    derived.metal_w, derived.metal_h
                ),
            ));
        }
    }
    if let Some(g) = layers.grassmap {
        let want = (derived.grass_w * derived.grass_h) as usize;
        if g.len() != want {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "grass map is {} bytes; it is mapx/4 squared = {} x {}",
                    g.len(),
                    derived.grass_w,
                    derived.grass_h
                ),
            ));
        }
    }
    if layers.minimap.len() != MINIMAP_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "minimap is {} bytes; the block is exactly {MINIMAP_BYTES}",
                layers.minimap.len()
            ),
        ));
    }
    if layers.tile_index.len() != derived.tile_count as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "tile index has {} slots; the grid is {} x {}",
                layers.tile_index.len(),
                derived.tiles_x,
                derived.tiles_y
            ),
        ));
    }
    Ok(())
}

pub fn write<W: Write>(
    w: &mut W,
    derived: &Derived,
    min_height: f32,
    max_height: f32,
    map_id: u32,
    layers: &Layers,
) -> io::Result<()> {
    check(derived, layers)?;

    let grass_len = layers
        .grassmap
        .map(|_| (derived.grass_w * derived.grass_h) as usize)
        .unwrap_or(0);
    let extra_len = if layers.grassmap.is_some() {
        EXTRA_GRASS_BYTES
    } else {
        0
    };

    // Physical layout, in the order a real file uses.
    let grass_off = HEADER_BYTES + extra_len;
    let height_off = grass_off + grass_len;
    let type_off = height_off + (derived.height_w * derived.height_h) as usize * 2;
    let minimap_off = type_off + (derived.type_w * derived.type_h) as usize;
    let metal_off = minimap_off + MINIMAP_BYTES as usize;
    let tiles_off = metal_off + (derived.metal_w * derived.metal_h) as usize;
    let features_off = tiles_off + tile_index_block_len(layers);

    w.write_all(MAGIC)?;
    w.write_all(&1i32.to_le_bytes())?; // version
    w.write_all(&map_id.to_le_bytes())?;
    w.write_all(&(derived.mapx as i32).to_le_bytes())?; // width, in 8-elmo squares
    w.write_all(&(derived.mapy as i32).to_le_bytes())?; // length
    w.write_all(&8i32.to_le_bytes())?; // squareSize
    w.write_all(&8i32.to_le_bytes())?; // texelsPerSquare
    w.write_all(&32i32.to_le_bytes())?; // tileSize
    w.write_all(&min_height.to_le_bytes())?;
    w.write_all(&max_height.to_le_bytes())?;
    w.write_all(&(height_off as i32).to_le_bytes())?;
    w.write_all(&(type_off as i32).to_le_bytes())?;
    w.write_all(&(tiles_off as i32).to_le_bytes())?;
    w.write_all(&(minimap_off as i32).to_le_bytes())?;
    w.write_all(&(metal_off as i32).to_le_bytes())?;
    w.write_all(&(features_off as i32).to_le_bytes())?;
    w.write_all(&(if extra_len > 0 { 1i32 } else { 0i32 }).to_le_bytes())?;

    if let Some(grass) = layers.grassmap {
        w.write_all(&(EXTRA_GRASS_BYTES as i32).to_le_bytes())?;
        w.write_all(&EXTRA_TYPE_GRASS.to_le_bytes())?;
        w.write_all(&(grass_off as i32).to_le_bytes())?;
        w.write_all(grass)?;
    }

    let mut hb = Vec::with_capacity(layers.heightmap.len() * 2);
    for v in layers.heightmap {
        hb.extend_from_slice(&v.to_le_bytes());
    }
    w.write_all(&hb)?;
    w.write_all(layers.typemap)?;
    w.write_all(layers.minimap)?;
    w.write_all(layers.metalmap)?;

    w.write_all(&(layers.smt_files.len() as i32).to_le_bytes())?;
    let total: u32 = layers.smt_files.iter().map(|f| f.tile_count).sum();
    w.write_all(&(total as i32).to_le_bytes())?;
    for f in layers.smt_files {
        w.write_all(&(f.tile_count as i32).to_le_bytes())?;
        w.write_all(f.file_name.as_bytes())?;
        w.write_all(&[0])?;
    }
    let mut ib = Vec::with_capacity(layers.tile_index.len() * 4);
    for v in layers.tile_index {
        ib.extend_from_slice(&(*v as i32).to_le_bytes());
    }
    w.write_all(&ib)?;

    // Empty features block.
    //
    // The wiki says the two leading ints are {numFeatures, numFeatureTypes};
    // the one file anyone has measured reads as types-first, and one file
    // cannot settle it (doc 03, open question C1). Writing an empty block
    // sidesteps the ambiguity entirely, since both readings agree on {0, 0}.
    // Modern maps place features through s11n or FeaturePlacer anyway.
    w.write_all(&0i32.to_le_bytes())?;
    w.write_all(&0i32.to_le_bytes())?;
    Ok(())
}

/* --------------------------------------------------------------- reading */

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub version: i32,
    pub map_id: u32,
    pub width: i32,
    pub length: i32,
    pub square_size: i32,
    pub texels_per_square: i32,
    pub tile_size: i32,
    pub min_height: f32,
    pub max_height: f32,
    pub heightmap_ptr: i32,
    pub typemap_ptr: i32,
    pub tiles_ptr: i32,
    pub minimap_ptr: i32,
    pub metalmap_ptr: i32,
    pub features_ptr: i32,
    pub num_extra_headers: i32,
    pub grass_ptr: Option<i32>,
}

fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub fn read_header(bytes: &[u8]) -> io::Result<Header> {
    if bytes.len() < HEADER_BYTES || &bytes[..16] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a Spring map file",
        ));
    }
    let mut h = Header {
        version: i32_at(bytes, 16),
        map_id: i32_at(bytes, 20) as u32,
        width: i32_at(bytes, 24),
        length: i32_at(bytes, 28),
        square_size: i32_at(bytes, 32),
        texels_per_square: i32_at(bytes, 36),
        tile_size: i32_at(bytes, 40),
        min_height: f32_at(bytes, 44),
        max_height: f32_at(bytes, 48),
        heightmap_ptr: i32_at(bytes, 52),
        typemap_ptr: i32_at(bytes, 56),
        tiles_ptr: i32_at(bytes, 60),
        minimap_ptr: i32_at(bytes, 64),
        metalmap_ptr: i32_at(bytes, 68),
        features_ptr: i32_at(bytes, 72),
        num_extra_headers: i32_at(bytes, 76),
        grass_ptr: None,
    };
    let mut off = HEADER_BYTES;
    for _ in 0..h.num_extra_headers.max(0) {
        if off + 8 > bytes.len() {
            break;
        }
        let size = i32_at(bytes, off);
        let kind = i32_at(bytes, off + 4);
        if kind == EXTRA_TYPE_GRASS && size >= 12 {
            h.grass_ptr = Some(i32_at(bytes, off + 8));
        }
        if size <= 0 {
            break;
        }
        off += size as usize;
    }
    Ok(h)
}

/// The tile index header: how many SMTs, how many tiles each, and their names.
pub fn read_tile_refs(bytes: &[u8], header: &Header) -> io::Result<(Vec<SmtRef>, usize)> {
    let mut o = header.tiles_ptr as usize;
    let files = i32_at(bytes, o);
    o += 8; // skip totalTiles, which is the sum of the per-file counts
    let mut refs = Vec::new();
    for _ in 0..files.max(0) {
        let count = i32_at(bytes, o) as u32;
        o += 4;
        let start = o;
        while o < bytes.len() && bytes[o] != 0 {
            o += 1;
        }
        let name = String::from_utf8_lossy(&bytes[start..o]).into_owned();
        o += 1;
        refs.push(SmtRef {
            file_name: name,
            tile_count: count,
        });
    }
    Ok((refs, o))
}

/// The tile index array, `(mapx / 4) × (mapy / 4)` entries of `u32`.
///
/// `at` is the offset [`read_tile_refs`] stopped at, which is where the index
/// starts — the SMT names are variable length, so it cannot be computed from
/// the header alone.
pub fn read_tile_index(bytes: &[u8], header: &Header, at: usize) -> io::Result<Vec<u32>> {
    let w = (header.width / 4) as usize;
    let h = (header.length / 4) as usize;
    let want = w * h;
    let end = at + want * 4;
    if bytes.len() < end {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "tile index wants {want} entries ({} bytes) from {at}, file is {}",
                want * 4,
                bytes.len()
            ),
        ));
    }
    Ok((0..want)
        .map(|i| {
            let o = at + i * 4;
            u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
        })
        .collect())
}

/// The vertex lattice a header describes: `(width + 1) × (length + 1)`.
///
/// `width` and `length` are in map squares — `mapx` and `mapy` — and the
/// heightmap has one more sample than squares on each axis. Getting this off by
/// one reads the whole block sheared, which looks like corrupt terrain rather
/// than like an indexing bug.
pub fn height_dims(header: &Header) -> (usize, usize) {
    ((header.width + 1) as usize, (header.length + 1) as usize)
}

/// Read the heightmap block as raw 16-bit samples, row-major from the north
/// west corner.
///
/// The samples are unsigned and span the header's own `min_height` to
/// `max_height`: sample `v` is `min + (v / 65535) · (max - min)` elmos, which
/// is exactly the field convention here, so `v / 65535` *is* the field value
/// and no rescaling is needed on the way in.
pub fn read_heightmap(bytes: &[u8], header: &Header) -> io::Result<Vec<u16>> {
    let (w, h) = height_dims(header);
    read_block_u16(bytes, header.heightmap_ptr, w * h, "heightmap")
}

/// Read the metal block, one byte per sample over `mapx/2 × mapy/2`.
pub fn read_metalmap(bytes: &[u8], header: &Header) -> io::Result<Vec<u8>> {
    let (w, h) = (header.width as usize / 2, header.length as usize / 2);
    read_block_u8(bytes, header.metalmap_ptr, w * h, "metalmap")
}

/// Read the terrain type block, one index per sample over `mapx/2 × mapy/2`.
pub fn read_typemap(bytes: &[u8], header: &Header) -> io::Result<Vec<u8>> {
    let (w, h) = (header.width as usize / 2, header.length as usize / 2);
    read_block_u8(bytes, header.typemap_ptr, w * h, "typemap")
}

fn block_range(ptr: i32, len: usize, total: usize, what: &str) -> io::Result<(usize, usize)> {
    if ptr <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("this map has no {what} block"),
        ));
    }
    let start = ptr as usize;
    let end = start.checked_add(len).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{what} offset wraps"))
    })?;
    if end > total {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "the {what} block runs past the end of the file: {end} bytes needed, {total} present"
            ),
        ));
    }
    Ok((start, end))
}

fn read_block_u16(bytes: &[u8], ptr: i32, count: usize, what: &str) -> io::Result<Vec<u16>> {
    let (start, _) = block_range(ptr, count * 2, bytes.len(), what)?;
    Ok((0..count)
        .map(|i| {
            let o = start + i * 2;
            u16::from_le_bytes([bytes[o], bytes[o + 1]])
        })
        .collect())
}

fn read_block_u8(bytes: &[u8], ptr: i32, count: usize, what: &str) -> io::Result<Vec<u8>> {
    let (start, end) = block_range(ptr, count, bytes.len(), what)?;
    Ok(bytes[start..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use springen_core::spring::derive;

    #[allow(clippy::type_complexity)]
    fn sample(
        units: u32,
    ) -> (
        Derived,
        Vec<u16>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u32>,
    ) {
        let d = derive(units, units);
        let height = vec![1234u16; (d.height_w * d.height_h) as usize];
        let typemap = vec![0u8; (d.type_w * d.type_h) as usize];
        let metal = vec![0u8; (d.metal_w * d.metal_h) as usize];
        let grass = vec![1u8; (d.grass_w * d.grass_h) as usize];
        let minimap = vec![7u8; MINIMAP_BYTES as usize];
        let index: Vec<u32> = (0..d.tile_count).collect();
        (d, height, typemap, metal, grass, minimap, index)
    }

    #[test]
    fn header_is_eighty_bytes_and_round_trips() {
        let (d, height, typemap, metal, grass, minimap, index) = sample(2);
        let smt = [SmtRef {
            file_name: "test.smt".into(),
            tile_count: d.tile_count,
        }];
        let layers = Layers {
            heightmap: &height,
            typemap: &typemap,
            metalmap: &metal,
            grassmap: Some(&grass),
            minimap: &minimap,
            tile_index: &index,
            smt_files: &smt,
        };
        let mut out = Vec::new();
        write(&mut out, &d, -60.0, 440.0, 54, &layers).unwrap();
        assert_eq!(out.len(), file_len(&d, &layers));

        let h = read_header(&out).unwrap();
        assert_eq!(h.version, 1);
        assert_eq!(h.map_id, 54);
        assert_eq!(h.width, d.mapx as i32);
        assert_eq!(h.square_size, 8);
        assert_eq!(h.texels_per_square, 8);
        assert_eq!(h.tile_size, 32);
        assert_eq!(h.min_height, -60.0);
        assert_eq!(h.max_height, 440.0);
        assert_eq!(h.num_extra_headers, 1);
        // The grass map is written first, right after header + extra header.
        assert_eq!(h.grass_ptr, Some(92));

        let (refs, after) = read_tile_refs(&out, &h).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].file_name, "test.smt");
        assert_eq!(refs[0].tile_count, d.tile_count);
        // The per-slot index follows the header immediately.
        assert_eq!(i32_at(&out, after), 0);
    }

    #[test]
    fn every_block_reads_back_exactly_what_was_written() {
        // The writer is the only fixture the reader needs, and using it means
        // the two cannot drift apart silently.
        let (d, height, typemap, metal, grass, minimap, index) = sample(2);
        let smt = [SmtRef {
            file_name: "rt.smt".into(),
            tile_count: d.tile_count,
        }];
        let layers = Layers {
            heightmap: &height,
            typemap: &typemap,
            metalmap: &metal,
            grassmap: Some(&grass),
            minimap: &minimap,
            tile_index: &index,
            smt_files: &smt,
        };
        let mut out = Vec::new();
        write(&mut out, &d, -60.0, 440.0, 1, &layers).unwrap();
        let h = read_header(&out).unwrap();

        assert_eq!(height_dims(&h), (d.height_w as usize, d.height_h as usize));
        assert_eq!(read_heightmap(&out, &h).unwrap(), height);
        assert_eq!(read_metalmap(&out, &h).unwrap(), metal);
        assert_eq!(read_typemap(&out, &h).unwrap(), typemap);
    }

    #[test]
    fn a_truncated_file_is_refused_rather_than_read_as_terrain() {
        let (d, height, typemap, metal, grass, minimap, index) = sample(2);
        let smt = [SmtRef {
            file_name: "t.smt".into(),
            tile_count: d.tile_count,
        }];
        let layers = Layers {
            heightmap: &height,
            typemap: &typemap,
            metalmap: &metal,
            grassmap: Some(&grass),
            minimap: &minimap,
            tile_index: &index,
            smt_files: &smt,
        };
        let mut out = Vec::new();
        write(&mut out, &d, -60.0, 440.0, 1, &layers).unwrap();
        let h = read_header(&out).unwrap();
        out.truncate(h.heightmap_ptr as usize + 16);
        let e = read_heightmap(&out, &h).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn blocks_are_contiguous_and_reachable_by_offset() {
        let (d, height, typemap, metal, grass, minimap, index) = sample(2);
        let smt = [SmtRef {
            file_name: "m.smt".into(),
            tile_count: d.tile_count,
        }];
        let layers = Layers {
            heightmap: &height,
            typemap: &typemap,
            metalmap: &metal,
            grassmap: Some(&grass),
            minimap: &minimap,
            tile_index: &index,
            smt_files: &smt,
        };
        let mut out = Vec::new();
        write(&mut out, &d, -60.0, 440.0, 1, &layers).unwrap();
        let h = read_header(&out).unwrap();

        // Follow offsets rather than assuming order, and check the data.
        let ho = h.heightmap_ptr as usize;
        assert_eq!(
            u16::from_le_bytes(out[ho..ho + 2].try_into().unwrap()),
            1234
        );
        let go = h.grass_ptr.unwrap() as usize;
        assert_eq!(out[go], 1);
        let mo = h.minimap_ptr as usize;
        assert_eq!(out[mo], 7);
        assert_eq!(&out[mo..mo + 4], &[7, 7, 7, 7]);

        // Features block is empty under either reading of its first two ints.
        let fo = h.features_ptr as usize;
        assert_eq!(i32_at(&out, fo), 0);
        assert_eq!(i32_at(&out, fo + 4), 0);
        assert_eq!(fo + 8, out.len());
    }

    #[test]
    fn wrong_layer_sizes_are_refused_by_name() {
        let (d, _h, typemap, metal, grass, minimap, index) = sample(2);
        // A 1024² heightmap for a 16x16 map is the classic mistake; here the
        // equivalent is dropping the +1.
        let bad = vec![0u16; (d.mapx * d.mapy) as usize];
        let smt = [SmtRef {
            file_name: "m.smt".into(),
            tile_count: d.tile_count,
        }];
        let layers = Layers {
            heightmap: &bad,
            typemap: &typemap,
            metalmap: &metal,
            grassmap: Some(&grass),
            minimap: &minimap,
            tile_index: &index,
            smt_files: &smt,
        };
        let err = write(&mut Vec::new(), &d, -60.0, 440.0, 1, &layers).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("vertex lattice"), "{msg}");
        assert!(msg.contains("129"), "{msg}");
    }
}

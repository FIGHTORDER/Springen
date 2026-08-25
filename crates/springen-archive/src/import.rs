//! Reading somebody else's map back in.
//!
//! Phase 1 of import: open a `.sd7`, `.sdz` or
//! `.sdd`, find the SMF inside it, and hand back the terrain as a field the
//! rest of the tool already understands.
//!
//! The design decision this rests on is in doc 08 and worth repeating, because
//! everything downstream depends on it: an imported map does **not** get its
//! own parallel editing mode. It becomes a raster in the graph's raster store,
//! reachable through the `import` node, and from that point on it is terrain
//! like any other terrain — `grade` flattens it, `ramp` cuts a road through it,
//! the probes measure it, the bake writes it. Nothing else in the tool has to
//! learn what an imported map is.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use springen_core::field::{Field, SharedField};
use springen_core::graph::Graph;
use springen_core::project::Project;
use springen_core::raster::Rasters;
use springen_smf::{smf, smt};

/// A map read off disk, before anything has been decided about it.
#[derive(Clone, Debug)]
pub struct ImportedMap {
    /// The archive or folder it came from, for messages and provenance.
    pub source: PathBuf,
    /// Path of the SMF within the archive.
    pub smf_path: String,
    pub map_id: u32,
    /// Map squares on each axis — `mapx` and `mapy`, always a multiple of 128.
    pub mapx: u32,
    pub mapy: u32,
    /// The height range the file declares. Height 0 elmos is the waterline, so
    /// these two also say where its sea was.
    pub min_height: f64,
    pub max_height: f64,
    /// The vertex lattice, `(mapx + 1) × (mapy + 1)`, as raw 16-bit samples.
    pub height: Vec<u16>,
    pub height_w: usize,
    pub height_h: usize,
    /// `mapx/2 × mapy/2`, when the blocks were readable.
    pub metal: Option<Vec<u8>>,
    pub typemap: Option<Vec<u8>>,
    /// The SMT files the tile index names.
    pub smt_files: Vec<String>,
    /// The reassembled diffuse, RGB, `mapx * 8` by `mapy * 8`, when the tile
    /// store was found and readable.
    ///
    /// Decoded from DXT1, which is lossy — but only in the encoding direction.
    /// Decoding is exact, so what is here is exactly what the engine would
    /// have shown. Re-encoding it on the way out is the lossy step, and it is
    /// why an untouched tile should be copied rather than round-tripped; see
    /// hand editing.
    pub diffuse: Option<Vec<u8>>,
    pub diffuse_w: usize,
    pub diffuse_h: usize,
    /// Anything the reader wants the user to know: things it could not read,
    /// and things about the map that this tool will not reproduce faithfully.
    pub notes: Vec<String>,
}

impl ImportedMap {
    /// Elmo extent on each axis. A map square is 8 elmos.
    pub fn elmos(&self) -> (f64, f64) {
        (f64::from(self.mapx) * 8.0, f64::from(self.mapy) * 8.0)
    }

    /// The terrain as a field on the square lattice the graph evaluates on.
    ///
    /// `v / 65535` is the field value directly: a raw SMF sample already means
    /// `min + (v / 65535) · range`, which is exactly this project's convention,
    /// so an import needs no rescaling and loses nothing on the way in.
    ///
    /// The lattice is square because `Field` is, which for the usual square map
    /// is an exact copy — the sample points land on integers and nothing is
    /// interpolated. A non-square map is bilinearly resampled onto the longer
    /// side and comes back stretched, which is the same limitation the rest of
    /// the tool has and is reported in `notes`.
    /// The diffuse as a square 3-channel field, or `None` if it was not read.
    ///
    /// Square and bilinear, like `height_field` and for the same reason:
    /// `Field` is square. A diffuse is a photograph of ground, so unlike the
    /// typemap it is the one thing here that *should* be interpolated.
    pub fn diffuse_field(&self) -> Option<SharedField> {
        let px = self.diffuse.as_ref()?;
        let (sw, sh) = (self.diffuse_w, self.diffuse_h);
        if sw < 2 || sh < 2 {
            return None;
        }
        let side = sw.max(sh).min(MAX_DIFFUSE_SIDE);
        let mut f = Field::new(side, 3);
        // An integer ratio when there is one, so the common case — a square
        // map at a power of two — is an exact box filter rather than a
        // resample that invents texels between the ones it was given.
        let box_ratio = if sw == sh && sw > side && sw.is_multiple_of(side) {
            sw / side
        } else {
            1
        };
        let last = (side - 1) as f64;
        for y in 0..side {
            let sy = ((y as f64 / last) * (sh - 1) as f64).clamp(0.0, (sh - 1) as f64);
            let (y0, fy) = (sy.floor() as usize, sy - sy.floor());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..side {
                let sx = ((x as f64 / last) * (sw - 1) as f64).clamp(0.0, (sw - 1) as f64);
                let (x0, fx) = (sx.floor() as usize, sx - sx.floor());
                let x1 = (x0 + 1).min(sw - 1);
                for c in 0..3 {
                    let at = |px_x: usize, px_y: usize| {
                        f64::from(px[(px_y * sw + px_x) * 3 + c]) / 255.0
                    };
                    let v = if box_ratio > 1 {
                        // Shrinking: average the block that maps to this
                        // sample. A wider kernel would read past it and a
                        // point sample would alias — the same rule the
                        // material downsampler follows.
                        let (bx, by) = (x * box_ratio, y * box_ratio);
                        let mut sum = 0.0;
                        let mut n = 0.0;
                        for oy in 0..box_ratio {
                            for ox in 0..box_ratio {
                                sum += at((bx + ox).min(sw - 1), (by + oy).min(sh - 1));
                                n += 1.0;
                            }
                        }
                        sum / n
                    } else {
                        let (a, b) = (at(x0, y0), at(x1, y0));
                        let (cc, d) = (at(x0, y1), at(x1, y1));
                        let top = a + (b - a) * fx;
                        top + ((cc + (d - cc) * fx) - top) * fy
                    };
                    f.set((y * side + x) * 3 + c, v);
                }
            }
        }
        Some(std::sync::Arc::new(f))
    }

    pub fn height_field(&self) -> SharedField {
        let side = self.height_w.max(self.height_h).max(2);
        let mut f = Field::new(side, 1);
        let (sw, sh) = (self.height_w, self.height_h);
        let at = |x: usize, y: usize| {
            f64::from(self.height[y.min(sh - 1) * sw + x.min(sw - 1)]) / 65535.0
        };
        let last = (side - 1) as f64;
        for y in 0..side {
            let sy = (y as f64 / last) * (sh - 1) as f64;
            let (y0, fy) = (sy.floor() as usize, sy - sy.floor());
            for x in 0..side {
                let sx = (x as f64 / last) * (sw - 1) as f64;
                let (x0, fx) = (sx.floor() as usize, sx - sx.floor());
                let v = if fx == 0.0 && fy == 0.0 {
                    at(x0, y0)
                } else {
                    let (a, b) = (at(x0, y0), at(x0 + 1, y0));
                    let (c, d) = (at(x0, y0 + 1), at(x0 + 1, y0 + 1));
                    (a + (b - a) * fx) + ((c + (d - c) * fx) - (a + (b - a) * fx)) * fy
                };
                f.set(y * side + x, v);
            }
        }
        std::sync::Arc::new(f)
    }
}

/// The largest side a diffuse raster is kept at.
///
/// A 24×24 map's diffuse is 12288², which is 453 MB of RGB and 1.8 GB once it
/// is an f32 `Field` — enough to bring the tool down on a machine that could
/// otherwise bake the map. Above this it is box-downsampled.
///
/// Little is actually lost, and what is, was going to be. A raster enters
/// through the graph, so an imported diffuse is resampled to the evaluation
/// lattice on the way through and written back out at map size — detail
/// finer than that lattice does not survive a re-bake whatever this number
/// is. That is a real cost of the graph being the only way in, and it is
/// stated rather than hidden.
const MAX_DIFFUSE_SIDE: usize = 4096;

/// Read a map from a `.sd7`, `.sdz` or `.sdd`.
pub fn read_map(path: &Path) -> io::Result<ImportedMap> {
    let (smf_path, bytes) = find_smf(path)?;
    let header = smf::read_header(&bytes)?;
    let mut notes = Vec::new();

    if header.square_size != 8 {
        notes.push(format!(
            "This map declares squareSize {}, not 8. Every elmo measurement here assumes 8.",
            header.square_size
        ));
    }
    if header.width != header.length {
        notes.push(format!(
            "This map is {} × {} squares. Springen evaluates on a square lattice and resamples per axis, so its features will be stretched along the longer side. This is still open.",
            header.width, header.length
        ));
    }

    let height = smf::read_heightmap(&bytes, &header)?;
    let (height_w, height_h) = smf::height_dims(&header);

    // The info blocks are wanted but not required: a map that is unreadable
    // here should still import its terrain rather than fail outright.
    let metal = smf::read_metalmap(&bytes, &header)
        .map_err(|e| notes.push(format!("Metal block not read: {e}")))
        .ok();
    let typemap = smf::read_typemap(&bytes, &header)
        .map_err(|e| notes.push(format!("Terrain type block not read: {e}")))
        .ok();

    let (smt_files, tile_index) = match smf::read_tile_refs(&bytes, &header) {
        Ok((refs, at)) => {
            let names: Vec<String> = refs.into_iter().map(|r| r.file_name).collect();
            let index = smf::read_tile_index(&bytes, &header, at)
                .map_err(|e| notes.push(format!("Tile index not read: {e}")))
                .ok();
            (names, index)
        }
        Err(e) => {
            notes.push(format!("Tile index not read: {e}"));
            (Vec::new(), None)
        }
    };

    // The diffuse, reassembled from the tile store the index points at.
    let (mut diffuse, mut diffuse_w, mut diffuse_h) = (None, 0usize, 0usize);
    if let Some(index) = &tile_index {
        match read_diffuse(path, &smt_files, index, &header) {
            Ok((px, w, h)) => {
                diffuse = Some(px);
                diffuse_w = w;
                diffuse_h = h;
            }
            Err(e) => notes.push(format!(
                "The diffuse in {} was not read ({e}); the map will be repainted by the graph.",
                smt_files.join(", ")
            )),
        }
    }
    if header.features_ptr > 0 {
        notes.push(
            "Features are not imported. If this map has real features in its SMF block it is also the sample open question C1 has been waiting for.".into(),
        );
    }

    Ok(ImportedMap {
        source: path.to_path_buf(),
        smf_path,
        map_id: header.map_id,
        mapx: header.width.max(0) as u32,
        mapy: header.length.max(0) as u32,
        min_height: f64::from(header.min_height),
        max_height: f64::from(header.max_height),
        height,
        height_w,
        height_h,
        metal,
        typemap,
        smt_files,
        diffuse,
        diffuse_w,
        diffuse_h,
        notes,
    })
}

/* --------------------------------------------------- becoming a project */

/// What an import turns into: a project, a graph that reads the raster, and
/// the raster itself.
pub struct ImportedProject {
    pub project: Project,
    pub graph: Graph,
    pub rasters: Rasters,
    pub notes: Vec<String>,
}

/// Build a project around an imported map.
///
/// The size is taken from the file and the height range with it, so the
/// terrain means the same number of elmos as it did in the map it came from —
/// an import that quietly rescaled the vertical would be the waterline bug
/// again, and this time on somebody else's map.
///
/// The graph is the standard texture chain hung off an `import` node, so an
/// imported map bakes into a complete, loadable archive on the first try
/// rather than a heightmap with no splat distribution.
pub fn to_project(map: &ImportedMap, name: &str) -> ImportedProject {
    let (ex, ey) = map.elmos();
    let mut project = Project {
        name: name.to_string(),
        min_height: map.min_height,
        max_height: map.max_height,
        units_x: ((ex / 512.0).round() as u32).max(2),
        units_y: ((ey / 512.0).round() as u32).max(2),
        ..Default::default()
    };
    let mut notes = map.notes.clone();

    // Size units are even by engine rule. A map whose extent is not a whole
    // even number of them cannot be re-baked at its own size.
    for (axis, units, elmos) in [("X", project.units_x, ex), ("Y", project.units_y, ey)] {
        if (f64::from(units) * 512.0 - elmos).abs() > 0.5 || !units.is_multiple_of(2) {
            notes.push(format!(
                "This map is {elmos} elmos on {axis}, which is not a whole even number of 512-elmo size units. It has been rounded to {units}, so a re-bake will not be the same size as the original."
            ));
        }
    }
    if !project.units_x.is_multiple_of(2) {
        project.units_x += 1;
    }
    if !project.units_y.is_multiple_of(2) {
        project.units_y += 1;
    }

    let mut rasters = Rasters::new();
    rasters.insert(Rasters::TERRAIN, map.height_field());
    if let Some(field) = map.diffuse_field() {
        rasters.insert(Rasters::DIFFUSE, field);
    }

    let mut graph = Graph::new();
    let src = graph.add("import", 40.0, 60.0, &[]);
    let out = graph.add("out_height", 320.0, 60.0, &[]);
    graph.link(&src, &out, "In");
    if rasters.contains(Rasters::DIFFUSE) {
        // The map keeps its own texture. Repainting an imported map from a
        // procedural chain is how an import stops looking like the map it
        // came from, and the whole point of Phase 2 is that it should not.
        let tex = graph.add("import_color", 40.0, 320.0, &[]);
        let d_out = graph.add("out_diffuse", 320.0, 320.0, &[]);
        graph.link(&tex, &d_out, "In");
        notes.push(
            "The diffuse was read from the tile store and enters through an `import_color` node. Re-baking recompresses it: DXT1 is lossy, so a round trip is not free even for tiles nobody touched.".into(),
        );
        let side = map.diffuse_w.max(map.diffuse_h);
        if side > MAX_DIFFUSE_SIDE {
            notes.push(format!(
                "Its diffuse is {side}px and has been box-downsampled to {MAX_DIFFUSE_SIDE} to fit in memory. Detail finer than the evaluation lattice would not have survived a re-bake in any case, because a raster reaches the bake through the graph."
            ));
        }
    } else {
        springen_core::starter::texture_chain_for(&mut graph, &src, 560.0);
    }

    ImportedProject {
        project,
        graph,
        rasters,
        notes,
    }
}

/// Write an imported project out as a folder: `project.json` beside a
/// `rasters/` directory.
///
/// A project stops being one file the moment it carries a raster, and doc 08
/// argues that is worth settling in Phase 1 rather than discovering in Phase 4
/// when brush strokes need somewhere to live. The raster is a 16-bit PNG, which
/// is the same precision the SMF stores and the same format the rest of the
/// tool already reads and writes.
pub fn write_project_dir(dir: &Path, imported: &ImportedProject) -> io::Result<PathBuf> {
    fs::create_dir_all(dir.join("rasters"))?;
    for name in imported.rasters.names() {
        let f = imported.rasters.get(name).expect("just listed");
        // Height at 16 bits, which is the precision the SMF stores. A diffuse
        // at 8, which is all DXT1 ever held — storing it at 16 would suggest
        // a precision the source never had.
        let png = if f.ch >= 3 {
            springen_core::png::encode(
                f.res,
                f.res,
                springen_core::png::PngColor::Rgb,
                8,
                &rgb8(f),
                springen_core::png::Compression::Deflate,
            )
        } else {
            springen_core::png::encode(
                f.res,
                f.res,
                springen_core::png::PngColor::Gray,
                16,
                &gray16(f),
                springen_core::png::Compression::Deflate,
            )
        };
        fs::write(dir.join("rasters").join(format!("{name}.png")), png)?;
    }
    let doc = serde_json::json!({
        "project": imported.project,
        "graph": imported.graph.serialize(),
    });
    let path = dir.join("project.json");
    fs::write(
        &path,
        serde_json::to_string_pretty(&doc).map_err(io::Error::other)?,
    )?;
    Ok(path)
}

/// Load the rasters a project folder carries, if it is a folder at all.
///
/// Takes the path of the `project.json`; rasters live in `rasters/` beside it.
/// A project with no such directory simply has none, which is every purely
/// procedural project.
pub fn read_project_rasters(project_path: &Path) -> io::Result<Rasters> {
    let mut out = Rasters::new();
    let Some(dir) = project_path.parent().map(|p| p.join("rasters")) else {
        return Ok(out);
    };
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let img = springen_core::png::decode(&fs::read(&p)?)?;
        // Square, because `Field` is. A raster edited elsewhere and saved at a
        // different aspect is taken on its shorter side rather than skewed.
        let side = img.width.min(img.height).max(2);
        // Three channels for a colour raster, one for anything else — a
        // diffuse read back as grayscale would come out of the graph as a
        // monochrome map with nothing saying why.
        let ch = if img.color.channels() >= 3 { 3 } else { 1 };
        let mut f = Field::new(side, ch);
        for y in 0..side {
            for x in 0..side {
                for c in 0..ch {
                    f.set((y * side + x) * ch + c, img.value(x, y, c));
                }
            }
        }
        out.insert(name, std::sync::Arc::new(f));
    }
    Ok(out)
}

fn rgb8(f: &SharedField) -> Vec<u16> {
    let n = f.res * f.res;
    let mut out = Vec::with_capacity(n * 3);
    for i in 0..n {
        for c in 0..3 {
            let v = f.get(i * f.ch + c.min(f.ch - 1));
            out.push((v.clamp(0.0, 1.0) * 255.0).round() as u16);
        }
    }
    out
}

fn gray16(f: &SharedField) -> Vec<u16> {
    (0..f.res * f.res)
        .map(|i| (f.get(i).clamp(0.0, 1.0) * 65535.0).round() as u16)
        .collect()
}

/* ------------------------------------------------------------- locating */

fn is_smf(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".smf")
}

/// Find the SMF inside a map, whatever the map is packaged as.
///
/// Engine map archives put it under `maps/`, but the search is not restricted
/// to that: hand-assembled `.sdd` folders in the wild put it in odd places and
/// refusing them for a directory name helps nobody.
fn find_smf(path: &Path) -> io::Result<(String, Vec<u8>)> {
    if path.is_dir() {
        return find_smf_in_dir(path);
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "smf" => Ok((
            path.file_name().unwrap_or_default().to_string_lossy().into(),
            fs::read(path)?,
        )),
        "sd7" => find_smf_in_sd7(path),
        "sdz" => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "`.sdz` is a zip and Springen has no zip reader yet. Unpack it, or point this at the `.sdd` folder or the `.smf` directly.",
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is not a map. Give this a .sd7 archive, a .sdd folder, or a .smf file.",
                path.display()
            ),
        )),
    }
}

fn find_smf_in_dir(dir: &Path) -> io::Result<(String, Vec<u8>)> {
    let mut found: Option<PathBuf> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&d)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        // Deterministic: a folder with two SMFs must import the same one every
        // time, not whichever the filesystem happened to hand back first.
        entries.sort();
        for p in entries {
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().map(|n| is_smf(&n.to_string_lossy())) == Some(true)
                && (found.is_none() || Some(&p) < found.as_ref())
            {
                found = Some(p);
            }
        }
    }
    match found {
        Some(p) => {
            let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().into();
            Ok((rel, fs::read(&p)?))
        }
        None => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no .smf anywhere under {}", dir.display()),
        )),
    }
}

fn find_smf_in_sd7(path: &Path) -> io::Result<(String, Vec<u8>)> {
    use sevenz_rust2::ArchiveReader;
    let mut reader = ArchiveReader::open(path, "".into()).map_err(to_io)?;
    let names: Vec<String> = reader
        .archive()
        .files
        .iter()
        .filter(|f| !f.has_stream() || is_smf(&f.name))
        .filter(|f| is_smf(&f.name))
        .map(|f| f.name.clone())
        .collect();
    let mut names = names;
    names.sort();
    let Some(name) = names.first().cloned() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no .smf inside {}", path.display()),
        ));
    };
    let bytes = reader.read_file(&name).map_err(to_io)?;
    Ok((name, bytes))
}

/// Rebuild the diffuse from the tile store.
///
/// The index is one `u32` per 32×32 tile, laid out `mapx/4` across. Tiles are
/// numbered across all the SMT files the header names, in order, which is why
/// the stores are concatenated rather than searched.
fn read_diffuse(
    path: &Path,
    smt_files: &[String],
    index: &[u32],
    header: &smf::Header,
) -> Result<(Vec<u8>, usize, usize), String> {
    if smt_files.is_empty() {
        return Err("the header names no tile store".into());
    }
    let mut stores = Vec::new();
    for name in smt_files {
        let bytes = read_sibling(path, name)?;
        stores.push(smt::read(&bytes)?);
    }
    let tiles_x = (header.width / 4) as usize;
    let tiles_y = (header.length / 4) as usize;
    if index.len() < tiles_x * tiles_y {
        return Err(format!(
            "the index holds {} entries and the map needs {}",
            index.len(),
            tiles_x * tiles_y
        ));
    }
    let ts = smt::TILE_SIZE;
    let (w, h) = (tiles_x * ts, tiles_y * ts);
    let mut out = vec![0u8; w * h * 3];
    let mut missing = 0usize;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let mut n = index[ty * tiles_x + tx] as usize;
            // Tile numbers run on across the stores in the order the header
            // names them.
            let mut rgb = None;
            for st in &stores {
                if n < st.count {
                    rgb = st.tile_rgb(n);
                    break;
                }
                n -= st.count;
            }
            let Some(rgb) = rgb else {
                missing += 1;
                continue;
            };
            for y in 0..ts {
                let d = ((ty * ts + y) * w + tx * ts) * 3;
                out[d..d + ts * 3].copy_from_slice(&rgb[y * ts * 3..(y + 1) * ts * 3]);
            }
        }
    }
    if missing > 0 {
        return Err(format!(
            "{missing} of {} tiles pointed past the end of the tile store",
            tiles_x * tiles_y
        ));
    }
    Ok((out, w, h))
}

/// Read a file that sits beside the SMF, in whatever the map is packaged as.
fn read_sibling(path: &Path, name: &str) -> Result<Vec<u8>, String> {
    let wanted = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if path.is_dir() {
        // Sorted at each level, like `find_smf_in_dir`: a folder with two
        // files of the same name must resolve the same way every time.
        let mut stack = vec![path.to_path_buf()];
        while let Some(d) = stack.pop() {
            let mut entries: Vec<PathBuf> = fs::read_dir(&d)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .file_name()
                    .map(|f| f.to_string_lossy().to_ascii_lowercase() == wanted)
                    .unwrap_or(false)
                {
                    return fs::read(&p).map_err(|e| e.to_string());
                }
            }
        }
        return Err(format!("{name} is not in the folder"));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "smf" {
        let dir = path.parent().unwrap_or(Path::new("."));
        return fs::read(dir.join(name))
            .or_else(|_| fs::read(dir.join(&wanted)))
            .map_err(|e| format!("{name}: {e}"));
    }
    if ext != "sd7" {
        return Err(format!("cannot reach {name} inside a .{ext}"));
    }
    use sevenz_rust2::ArchiveReader;
    let mut reader = ArchiveReader::open(path, "".into()).map_err(|e| e.to_string())?;
    let found = reader
        .archive()
        .files
        .iter()
        .find(|f| {
            f.has_stream()
                && f.name
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&f.name)
                    .to_ascii_lowercase()
                    == wanted
        })
        .map(|f| f.name.clone())
        .ok_or_else(|| format!("{name} is not in the archive"))?;
    reader.read_file(&found).map_err(|e| e.to_string())
}

fn to_io(e: sevenz_rust2::Error) -> io::Error {
    io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{bake, BakeOptions};
    use springen_core::project::Project;
    use springen_core::starter::starter_graph;

    /// Bake a real map, read it back, and check the terrain survived.
    ///
    /// The writer is the fixture. That is the whole point: an importer tested
    /// against a file the same repository produced cannot quietly disagree with
    /// the exporter, and the assertion is the one that matters — the terrain
    /// that comes back is the terrain that went out, to within the 16-bit
    /// quantisation the format imposes and nothing more.
    fn round_trip(dir_name: &str, sd7: bool) -> (ImportedMap, Project) {
        let project = Project {
            name: "RoundTrip".into(),
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        let graph = starter_graph("plains");
        let dir = std::env::temp_dir().join(dir_name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (bp, report) = bake(&project, &graph, &BakeOptions::default(), &dir).unwrap();

        let read = if sd7 {
            let path = dir.join("rt.sd7");
            bp.write_sd7(&path).unwrap();
            read_map(&path).unwrap()
        } else {
            let folder = dir.join("rt.sdd");
            bp.write_sdd(&folder).unwrap();
            read_map(&folder).unwrap()
        };
        // The bake tightens the declared range under HeightMode::Fit, and the
        // file carries the tightened one — so that is what must come back.
        let mut expected = project.clone();
        expected.min_height = report.declared_range.0;
        expected.max_height = report.declared_range.1;
        (read, expected)
    }

    #[test]
    fn a_map_we_baked_reads_back_with_its_terrain_intact() {
        let (m, expected) = round_trip("springen-import-sdd", false);
        // 4 size units is 2048 elmos, mapx 256, and a 257-sample lattice.
        assert_eq!((m.mapx, m.mapy), (256, 256));
        assert_eq!((m.height_w, m.height_h), (257, 257));
        assert_eq!(m.elmos(), (2048.0, 2048.0));
        assert!(
            (m.min_height - expected.min_height).abs() < 0.01
                && (m.max_height - expected.max_height).abs() < 0.01,
            "declared range came back as {} .. {}, expected {} .. {}",
            m.min_height,
            m.max_height,
            expected.min_height,
            expected.max_height
        );

        // The terrain itself: not flat, spanning the range, and every sample a
        // legal field value.
        let f = m.height_field();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for i in 0..f.len() {
            let v = f.get(i);
            assert!((0.0..=1.0).contains(&v), "sample {i} is {v}");
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(lo < 0.01, "the low end did not survive: {lo}");
        assert!(hi > 0.99, "the high end did not survive: {hi}");
        let _ = fs::remove_dir_all(std::env::temp_dir().join("springen-import-sdd"));
    }

    #[test]
    fn an_sd7_archive_imports_the_same_map_a_folder_does() {
        // Packaging must not change the terrain by one sample.
        let (from_dir, _) = round_trip("springen-import-cmp-sdd", false);
        let (from_7z, _) = round_trip("springen-import-cmp-sd7", true);
        assert_eq!(from_dir.height, from_7z.height);
        assert_eq!(from_dir.mapx, from_7z.mapx);
        for d in ["springen-import-cmp-sdd", "springen-import-cmp-sd7"] {
            let _ = fs::remove_dir_all(std::env::temp_dir().join(d));
        }
    }

    #[test]
    fn the_import_says_what_it_could_not_bring_with_it() {
        let (m, _) = round_trip("springen-import-notes", false);
        assert!(!m.smt_files.is_empty());
        // The diffuse now comes across, so the note that used to warn it did
        // not has moved into `to_project` — where it warns about the thing
        // that *is* still true, which is that re-baking recompresses it.
        assert!(
            m.diffuse.is_some(),
            "the diffuse was not read: {:?}",
            m.notes
        );
        let imported = to_project(&m, "Notes");
        assert!(
            imported
                .notes
                .iter()
                .any(|n| n.contains("DXT1") || n.contains("recompress")),
            "nothing warned that a round trip is lossy: {:?}",
            imported.notes
        );
        // Features are still not imported, and silence about that would mean
        // discovering it by baking.
        assert!(
            m.notes.iter().any(|n| n.contains("Features")),
            "notes did not mention features: {:?}",
            m.notes
        );
        let _ = fs::remove_dir_all(std::env::temp_dir().join("springen-import-notes"));
    }

    /// The whole chain: bake a map, import it, bake the import, and compare
    /// the two heightmap blocks sample for sample.
    ///
    /// This is the assertion the import suite lives or dies on. Every other
    /// test checks a piece; this one says the terrain that comes out the far
    /// end is the terrain that went in, through the SMF writer, the archive,
    /// the reader, a 16-bit PNG, the raster store, the `import` node and the
    /// writer again.
    #[test]
    fn a_map_survives_being_baked_imported_and_baked_again() {
        let dir = std::env::temp_dir().join("springen-import-chain");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let project = Project {
            name: "Chain".into(),
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        let (bp, _) = bake(
            &project,
            &starter_graph("open"),
            &BakeOptions::default(),
            &dir.join("first"),
        )
        .unwrap();
        let source = dir.join("source.sd7");
        bp.write_sd7(&source).unwrap();
        let first = read_map(&source).unwrap();

        // Import it, write the project folder, load it back the way the CLI
        // does — so the PNG round trip is under test too, not bypassed.
        let imported = to_project(&first, "Chain");
        let proj_dir = dir.join("project");
        let proj_path = write_project_dir(&proj_dir, &imported).unwrap();
        let rasters = read_project_rasters(&proj_path).unwrap();
        assert!(rasters.contains(Rasters::TERRAIN));
        let has_diffuse = rasters.contains(Rasters::DIFFUSE);

        let (bp2, _) = bake(
            &imported.project,
            &imported.graph,
            &BakeOptions {
                rasters: std::sync::Arc::new(rasters),
                ..Default::default()
            },
            &dir.join("second"),
        )
        .unwrap();
        let out2 = dir.join("again.sd7");
        bp2.write_sd7(&out2).unwrap();
        let second = read_map(&out2).unwrap();

        assert_eq!(
            (second.mapx, second.mapy),
            (first.mapx, first.mapy),
            "the map changed size"
        );
        assert!(
            (second.min_height - first.min_height).abs() < 0.01
                && (second.max_height - first.max_height).abs() < 0.01,
            "the height range moved: {} .. {} became {} .. {}",
            first.min_height,
            first.max_height,
            second.min_height,
            second.max_height
        );

        // The terrain. Not "close enough on average" — a per-sample bound,
        // because an import that drifts a little every round trip is an import
        // that destroys a map slowly.
        let mut worst = 0i32;
        for (a, b) in first.height.iter().zip(second.height.iter()) {
            worst = worst.max((i32::from(*a) - i32::from(*b)).abs());
        }
        assert!(
            worst <= 2,
            "terrain drifted by {worst} of 65535 through one round trip"
        );

        // The diffuse. It came back from the tile store, went through the
        // graph as a raster, and was written again — so this is the whole
        // Phase 2 path, PNG round trip included.
        assert!(has_diffuse, "no diffuse raster");
        let a = first.diffuse.as_ref().expect("first bake has a diffuse");
        let b = second.diffuse.as_ref().expect("second bake has a diffuse");
        assert_eq!(
            (first.diffuse_w, first.diffuse_h),
            (second.diffuse_w, second.diffuse_h),
            "the diffuse changed size"
        );
        // Bounded on the mean, not per texel. DXT1 is lossy in the encoding
        // direction, so a round trip *cannot* be exact — the honest assertion
        // is that it does not visibly shift, and this is what says so. A
        // per-texel bound here would only be recording how bad the worst block
        // in a 4x4 map happens to be.
        let err: f64 = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs())
            .sum::<f64>()
            / a.len() as f64;
        assert!(
            err < 6.0,
            "the diffuse shifted by {err:.1} levels a texel through one round trip"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A diffuse that came back has to be the map's own, not a repaint.
    ///
    /// The trap this guards is quiet: an importer that failed to read the tile
    /// store and fell back to the procedural texture chain would still produce
    /// a map, still bake, and still pass every other test here — it would just
    /// no longer look like the map it came from.
    #[test]
    fn an_imported_map_keeps_its_own_texture_rather_than_being_repainted() {
        let dir = std::env::temp_dir().join("springen-import-diffuse");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let project = Project {
            name: "Tex".into(),
            units_x: 4,
            units_y: 4,
            ..Default::default()
        };
        let (bp, _) = bake(
            &project,
            &starter_graph("textured"),
            &BakeOptions::default(),
            &dir.join("first"),
        )
        .unwrap();
        let source = dir.join("tex.sd7");
        bp.write_sd7(&source).unwrap();
        let map = read_map(&source).unwrap();

        let px = map.diffuse.as_ref().expect("the diffuse was not read");
        assert_eq!(px.len(), map.diffuse_w * map.diffuse_h * 3);
        assert_eq!(map.diffuse_w, map.mapx as usize * 8);
        // Not a flat fill: a tile store that read as one colour would mean the
        // index or the block decode is wrong.
        let mean = px.iter().map(|v| f64::from(*v)).sum::<f64>() / px.len() as f64;
        let sd = (px
            .iter()
            .map(|v| (f64::from(*v) - mean).powi(2))
            .sum::<f64>()
            / px.len() as f64)
            .sqrt();
        assert!(sd > 8.0, "the decoded diffuse is nearly flat (sd {sd:.1})");

        // And the graph reads it rather than repainting.
        let imported = to_project(&map, "Tex");
        assert!(
            imported
                .graph
                .nodes
                .iter()
                .any(|n| n.type_name == "import_color"),
            "an imported map with a diffuse was wired to a procedural texture chain"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn something_that_is_not_a_map_is_refused_with_a_reason() {
        let p = std::env::temp_dir().join("springen-import-nonsense.txt");
        fs::write(&p, b"not a map").unwrap();
        let e = read_map(&p).unwrap_err();
        assert!(
            e.to_string().contains(".sd7"),
            "the error should say what it does accept: {e}"
        );
        let _ = fs::remove_file(&p);
    }
}

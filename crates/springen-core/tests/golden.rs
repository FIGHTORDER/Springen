//! Golden-file parity with the browser prototype.
//!
//! The prototype is the correctness oracle: 367 assertions' worth of hard-won
//! knowledge about snapping asymmetry, the discrete-blob requirement,
//! nearest-neighbour category layers and the +1 lattice. Rather than re-derive
//! it and hope, this suite asserts the port produces **bit-identical** results
//! against frozen reference outputs.
//!
//! Run it in release; the 769² bake evaluates hydraulic erosion at full size.
//!
//! Two deliberate divergences are asserted rather than hidden.
//!
//! `out_splat` emits four channels because `splats.texScales`/`texMults` are
//! per RGBA channel (doc 03 §D.1). With the A port unwired its R, G and B are
//! identical to the prototype's, which is what the splat check verifies.
//!
//! Springen's own starter graphs have diverged from the prototype's and are
//! no longer compared against them — they carry a full texture chain, four
//! wired splat channels and a `tex_water` that follows the map's waterline
//! rather than a hard-coded 0.18. The frozen files are untouched and are what
//! everything below evaluates, so the parity claim is unaffected: it was
//! always a claim about these files, not about `starter.rs`.

#![allow(clippy::needless_range_loop)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};

use springen_core::bake::{bake_gray, bake_index, normalised, Resample};
use springen_core::field::{as_gray, Field, SharedField};
use springen_core::graph::Graph;
use springen_core::lua::{mapinfo_lua, metal_layout_lua, MetalLayoutOptions};
use springen_core::png::{encode, Compression, PngColor};
use springen_core::project::{water_level_t, Context, Project};
use springen_core::ramps::{hypso, ramp_at, ramp_names, ramp_stops};
use springen_core::rng::Mulberry32;
use springen_core::spring;
use springen_core::zk;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden")
        .canonicalize()
        .expect("golden/ must be present")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(golden_dir().join(name)).unwrap_or_else(|e| panic!("golden/{name}: {e}"))
}

fn read_json(name: &str) -> Value {
    serde_json::from_slice(&read(name)).unwrap_or_else(|e| panic!("golden/{name}: {e}"))
}

fn sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The project `tools/make_golden.js` froze the reference outputs with.
///
/// `hardness`, `gravity` and `tidal` are absent from that project, so the
/// prototype's `||` fallbacks supplied 100 / 120 / 18. Springen's own defaults
/// are the measured 350 / 100 / 0 from a shipped map instead (doc 03 §B5), so
/// they are restated here to reproduce the frozen text.
fn golden_project() -> Project {
    Project {
        name: "Golden".into(),
        description: String::new(),
        author: String::new(),
        version: "1.0".into(),
        units_x: 12,
        units_y: 12,
        seed: 20250815,
        min_height: -80.0,
        max_height: 420.0,
        hardness: 100.0,
        gravity: 120.0,
        tidal: 18.0,
        max_metal: 1.7,
        extractor_radius: 100.0,
        mex_sym: "rot180".into(),
        // The prototype had no depth cap, so the golden terrain must be
        // uncapped: a cap would shoal every submerged sample and no frozen
        // field would match.
        max_depth: None,
        // Derived from the symmetry, as every frozen graph's were.
        start_boxes: None,
        // Neither existed in the prototype, and neither reaches the graph:
        // materials are an archive-time overlay and spots are a hand-edit
        // override that the golden project does not use.
        materials: springen_core::material::MaterialSet::default(),
        environment: springen_core::env::Environment::default(),
        spots: Vec::new(),
    }
}

fn as_f64(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

/* ------------------------------------------------------------- primitives */

/// Port these first: if any is wrong, nothing above it can be right.
#[test]
fn primitives_are_bit_identical() {
    let p = read_json("primitives.json");

    let want = p["perlin2"].as_array().unwrap();
    for (i, w) in want.iter().enumerate() {
        let got = springen_core::noise::perlin2(i as f64 * 0.137, i as f64 * 0.079, 7.0);
        assert_eq!(
            got.to_bits(),
            as_f64(w).to_bits(),
            "perlin2[{i}]: {got:?} vs {:?}",
            as_f64(w)
        );
    }

    let want = p["mulberry32"].as_array().unwrap();
    let mut rng = Mulberry32::new(20250815);
    for (i, w) in want.iter().enumerate() {
        let got = rng.next();
        assert_eq!(got.to_bits(), as_f64(w).to_bits(), "mulberry32[{i}]");
    }

    for (key, odd) in [("snapEven", false), ("snapOdd", true)] {
        let want = p[key].as_array().unwrap();
        let mut k = 0usize;
        let mut v = 0i64;
        while v <= 2048 {
            let got = zk::snap_spot(v as f64, odd);
            assert_eq!(got, as_f64(&want[k]), "{key} at {v}");
            k += 1;
            v += 17;
        }
        assert_eq!(k, want.len(), "{key} length");
    }

    let want = p["hypso"].as_array().unwrap();
    for i in 0..=16usize {
        let got = hypso(i as f64 / 16.0);
        let w = want[i].as_array().unwrap();
        for c in 0..3 {
            assert_eq!(got[c].to_bits(), as_f64(&w[c]).to_bits(), "hypso[{i}][{c}]");
        }
    }

    let want = &p["ramps"];
    for name in ramp_names() {
        let stops = ramp_stops(name);
        let w = want[name].as_array().unwrap();
        for (k, t) in [0.0, 0.25, 0.5, 0.75, 1.0].iter().enumerate() {
            let got = ramp_at(stops, *t);
            let wk = w[k].as_array().unwrap();
            for c in 0..3 {
                assert_eq!(
                    got[c].to_bits(),
                    as_f64(&wk[c]).to_bits(),
                    "ramp {name} at {t} channel {c}"
                );
            }
        }
    }

    let want = p["waterLevels"].as_array().unwrap();
    let inputs = [
        (-80.0, 420.0),
        (-60.0, 440.0),
        (-188.0, 412.0),
        (0.0, 400.0),
        (-400.0, 0.0),
    ];
    for (i, (mn, mx)) in inputs.iter().enumerate() {
        let got = water_level_t(*mn, *mx);
        assert_eq!(got.to_bits(), as_f64(&want[i]).to_bits(), "waterLevel[{i}]");
    }
}

/* ------------------------------------------------------------------ graphs */

#[test]
fn the_frozen_graphs_still_load_into_what_they_describe() {
    // The oracle is the frozen JSON, not `starter.rs`. Every field dump, PNG
    // hash and Lua check below evaluates `Graph::deserialize` of these files,
    // so what matters is that they load into exactly the graph they describe.
    //
    // This used to also assert that Springen's starters were byte-identical
    // to the prototype's. They are not any more and should not be: two of the
    // three shipped with a heightmap and nothing else, which meant they baked
    // to flat shaded relief with no splat distribution, and the detail
    // materials had no weights to blend with. The sample maps are Springen's
    // now. The parity claim lives here, on the files.
    for kind in ["ridge", "islands", "textured"] {
        let want = read_json(&format!("graph_{kind}.json"));
        let loaded = Graph::deserialize(&want);
        let nodes = want["nodes"].as_array().unwrap();
        assert_eq!(loaded.nodes.len(), nodes.len(), "{kind}: node count");
        for (n, w) in loaded.nodes.iter().zip(nodes) {
            assert_eq!(n.id, w["id"].as_str().unwrap(), "{kind}: ids and order");
            assert_eq!(n.type_name, w["type"].as_str().unwrap(), "{kind}: types");
            let wired = w["inputs"].as_object().map(|o| o.len()).unwrap_or(0);
            assert_eq!(n.inputs.len(), wired, "{kind}: {} wiring", n.id);
        }
        // And every parameter the file names survives the round trip, which
        // is what would silently change an evaluated field.
        let back = loaded.serialize();
        for (n, w) in back["nodes"].as_array().unwrap().iter().zip(nodes) {
            for (k, v) in w["params"].as_object().unwrap() {
                assert_eq!(
                    n["params"][k].to_string(),
                    v.to_string(),
                    "{kind}: {} param {k}",
                    w["type"]
                );
            }
        }
    }
}

/* ------------------------------------------------------------ field dumps */

fn f32_bytes(field: &SharedField) -> Vec<u8> {
    field.to_le_bytes()
}

#[test]
fn every_terminal_field_matches_byte_for_byte() {
    let project = golden_project();
    let mut checked = 0;
    for kind in ["ridge", "islands", "textured"] {
        let g = Graph::deserialize(&read_json(&format!("graph_{kind}.json")));
        for res in [65usize, 129, 257] {
            let ctx = Context::new(&project, res);
            for term in ["height", "metal", "type", "diffuse", "grass", "splat"] {
                let Some(id) = g.find_terminal(term) else {
                    continue;
                };
                let name = format!("{kind}_{term}_{res}.f32");
                let want = read(&name);
                let got = g.evaluate(id, &ctx);
                checked += 1;

                if term == "splat" {
                    // Deliberate divergence: four channels instead of three.
                    assert_eq!(got.ch, 4, "{name}: splat must carry RGBA");
                    assert_eq!(
                        want.len(),
                        res * res * 3 * 4,
                        "{name}: prototype packed RGB"
                    );
                    for i in 0..res * res {
                        for c in 0..3 {
                            let off = (i * 3 + c) * 4;
                            let w = f32::from_le_bytes(want[off..off + 4].try_into().unwrap());
                            assert_eq!(
                                got.data[i * 4 + c].to_bits(),
                                w.to_bits(),
                                "{name}: RGB channel {c} at texel {i}"
                            );
                        }
                    }
                    continue;
                }

                let got_bytes = f32_bytes(&got);
                assert_eq!(got_bytes.len(), want.len(), "{name}: length");
                if got_bytes != want {
                    let diff = got_bytes
                        .chunks(4)
                        .zip(want.chunks(4))
                        .enumerate()
                        .filter(|(_, (a, b))| a != b)
                        .take(3)
                        .map(|(i, (a, b))| {
                            format!(
                                "  sample {i}: {:?} vs {:?}",
                                f32::from_le_bytes(a.try_into().unwrap()),
                                f32::from_le_bytes(b.try_into().unwrap())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    panic!("{name} differs from the prototype:\n{diff}");
                }
            }
        }
    }
    assert!(checked >= 30, "expected the full matrix, checked {checked}");
}

#[test]
fn field_statistics_match_the_manifest() {
    let manifest = read_json("MANIFEST.json");
    let project = golden_project();
    for kind in ["ridge", "islands", "textured"] {
        let g = Graph::deserialize(&read_json(&format!("graph_{kind}.json")));
        for res in [65usize, 129, 257] {
            let ctx = Context::new(&project, res);
            for term in ["height", "metal", "type", "diffuse", "grass", "splat"] {
                let Some(id) = g.find_terminal(term) else {
                    continue;
                };
                let key = format!("{kind}_{term}_{res}");
                let want = &manifest["scalars"][&key];
                if want.is_null() {
                    continue;
                }
                let field = g.evaluate(id, &ctx);
                assert_eq!(
                    field.res,
                    want["res"].as_u64().unwrap() as usize,
                    "{key} res"
                );
                if term == "splat" {
                    continue; // channel count intentionally differs
                }
                assert_eq!(field.ch, want["ch"].as_u64().unwrap() as usize, "{key} ch");
                let st = as_gray(&field).stats();
                assert_eq!(
                    st.min.to_bits(),
                    as_f64(&want["min"]).to_bits(),
                    "{key} min"
                );
                assert_eq!(
                    st.max.to_bits(),
                    as_f64(&want["max"]).to_bits(),
                    "{key} max"
                );
            }
        }
    }
}

/* ------------------------------------------------------------ baked layers */

#[test]
fn baked_layers_hash_identically() {
    let manifest = read_json("MANIFEST.json");
    let project = golden_project();
    let d = spring::derive(12, 12);
    let g = Graph::deserialize(&read_json("graph_textured.json"));
    let ctx = Context::new(&project, d.height_w.max(d.height_h) as usize);

    let h = g.evaluate(g.find_terminal("height").unwrap(), &ctx);
    let norm = normalised(&h);
    let png = encode(
        d.height_w as usize,
        d.height_h as usize,
        PngColor::Gray,
        16,
        &bake_gray(
            &norm,
            d.height_w as usize,
            d.height_h as usize,
            16,
            Resample::Bilinear,
            None,
        ),
        Compression::Stored,
    );
    let want = &manifest["files"]["bake_height_769_16bit.png"];
    assert_eq!(png.len(), want["bytes"].as_u64().unwrap() as usize);
    assert_eq!(sha256(&png), want["sha256"].as_str().unwrap());

    // Typemap as indices, nearest-neighbour. Type 1 must be byte 1, not 255.
    let t = g.evaluate(g.find_terminal("type").unwrap(), &ctx);
    let png = encode(
        d.type_w as usize,
        d.type_h as usize,
        PngColor::Gray,
        8,
        &bake_index(&as_gray(&t), d.type_w as usize, d.type_h as usize, 2),
        Compression::Stored,
    );
    let want = &manifest["files"]["bake_type_384_indices.png"];
    assert_eq!(sha256(&png), want["sha256"].as_str().unwrap());

    // Metal raster: discrete blobs painted from the spot list.
    let c129 = Context::new(&project, 129);
    let mask = as_gray(&g.evaluate(g.find_terminal("metal").unwrap(), &c129));
    let spots = zk::propose_spots(
        &mask,
        &c129,
        &zk::ProposeOptions {
            count: 14,
            min_separation: 700.0,
            symmetry: "rot180".into(),
            threshold: 0.2,
            amount: 2.0,
            odd_footprint: false,
            ..Default::default()
        },
    );
    let ras = zk::paint_metal_raster(
        &spots,
        &c129,
        d.metal_w as usize,
        d.metal_h as usize,
        &zk::RasterOptions {
            blob_radius: 48.0,
            max_metal: 1.7,
            ..Default::default()
        },
    );
    let samples: Vec<u16> = ras.iter().map(|v| u16::from(*v)).collect();
    let png = encode(
        d.metal_w as usize,
        d.metal_h as usize,
        PngColor::Gray,
        8,
        &samples,
        Compression::Stored,
    );
    let want = &manifest["files"]["bake_metal_384.png"];
    assert_eq!(sha256(&png), want["sha256"].as_str().unwrap());

    // Recorded invariants: one blob per spot, symmetry clean, no validation codes.
    assert_eq!(
        zk::count_blobs(&ras, d.metal_w as usize, d.metal_h as usize),
        manifest["scalars"]["metalBlobs"].as_u64().unwrap() as usize
    );
    assert_eq!(
        spots.len(),
        manifest["scalars"]["spots"]["n"].as_u64().unwrap() as usize
    );
    assert!(zk::symmetry_report(&spots, &c129, "rot180").symmetric);
    let codes: Vec<&str> = zk::validate_spots(&spots, &c129, 100.0)
        .issues
        .iter()
        .map(|i| i.code)
        .collect();
    let want_codes: Vec<String> = manifest["scalars"]["spots"]["validation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes, want_codes);
}

/* ------------------------------------------------------------ text outputs */

#[test]
fn text_outputs_match_character_for_character() {
    let project = golden_project();
    let d = spring::derive(12, 12);

    let want = String::from_utf8(read("mapinfo.lua")).unwrap();
    assert_eq!(mapinfo_lua(&project, &d), want);

    let g = Graph::deserialize(&read_json("graph_textured.json"));
    let c129 = Context::new(&project, 129);
    let mask = as_gray(&g.evaluate(g.find_terminal("metal").unwrap(), &c129));
    let spots = zk::propose_spots(
        &mask,
        &c129,
        &zk::ProposeOptions {
            count: 14,
            min_separation: 700.0,
            symmetry: "rot180".into(),
            threshold: 0.2,
            amount: 2.0,
            odd_footprint: false,
            ..Default::default()
        },
    );

    let want = String::from_utf8(read("map_metal_layout.lua")).unwrap();
    let got = metal_layout_lua(
        &spots,
        &MetalLayoutOptions {
            symmetry: Some("rot180".into()),
            ..Default::default()
        },
    );
    assert_eq!(got, want);

    // spots.json: same positions, order and ids.
    let want: Value = read_json("spots.json");
    let want = want.as_array().unwrap();
    assert_eq!(spots.len(), want.len());
    for (s, w) in spots.iter().zip(want) {
        assert_eq!(s.x, as_f64(&w["x"]), "spot {} x", s.id);
        assert_eq!(s.z, as_f64(&w["z"]), "spot {} z", s.id);
        assert_eq!(s.metal, as_f64(&w["metal"]), "spot {} metal", s.id);
        assert_eq!(s.id, w["id"].as_str().unwrap());
    }
}

/* ---------------------------------------------------- resolution invariance */

#[test]
fn noise_is_resolution_independent() {
    // Every distance is authored in elmos, so changing working resolution must
    // not change the shape of the terrain. Measured on the prototype: a mean
    // difference of 0.00005 between 65² and 513².
    let project = golden_project();
    let mut g = Graph::new();
    let n = g.add("noise", 0.0, 0.0, &[]);
    let low = g.evaluate(&n, &Context::new(&project, 65));
    let high = g.evaluate(&n, &Context::new(&project, 513));
    let mut sum = 0.0;
    for y in 0..65 {
        for x in 0..65 {
            let a = low.at(x, y);
            let b = high.at(x * 8, y * 8);
            sum += (a - b).abs();
        }
    }
    let mean = sum / (65.0 * 65.0);
    assert!(mean < 1e-4, "mean difference {mean} across resolutions");
}

#[test]
fn blur_radius_is_resolution_independent() {
    let project = golden_project();
    let mut g = Graph::new();
    let n = g.add(
        "noise",
        0.0,
        0.0,
        &[("feature", springen_core::PVal::Num(4096.0))],
    );
    let b = g.add(
        "blur",
        0.0,
        0.0,
        &[("radius", springen_core::PVal::Num(512.0))],
    );
    g.link(&n, &b, "In");
    let low = g.evaluate(&b, &Context::new(&project, 65));
    let high = g.evaluate(&b, &Context::new(&project, 513));
    let mut worst: f64 = 0.0;
    for y in 0..65 {
        for x in 0..65 {
            worst = worst.max((low.at(x, y) - high.at(x * 8, y * 8)).abs());
        }
    }
    assert!(worst < 0.05, "blur disagreed by {worst} across resolutions");
}

#[test]
fn symmetry_modes_are_exact() {
    // A2: the C4 fundamental domain must be half-open, or the centre row and
    // column get two representatives and the output is not symmetric.
    let project = golden_project();
    for mode in ["mirrorX", "mirrorY", "quad", "rot180", "rot90", "diagonal"] {
        for res in [64usize, 65] {
            let mut g = Graph::new();
            let n = g.add("noise", 0.0, 0.0, &[]);
            let s = g.add(
                "symmetry",
                0.0,
                0.0,
                &[("mode", springen_core::PVal::Str(mode.into()))],
            );
            g.link(&n, &s, "In");
            let out = g.evaluate(&s, &Context::new(&project, res));
            let r = res;
            for y in 0..r {
                for x in 0..r {
                    let (mx, my) = match mode {
                        "mirrorX" => (r - 1 - x, y),
                        "mirrorY" => (x, r - 1 - y),
                        "rot180" | "quad" => (r - 1 - x, r - 1 - y),
                        "rot90" => (y, r - 1 - x),
                        _ => (r - 1 - y, r - 1 - x),
                    };
                    assert_eq!(
                        out.at(x, y),
                        out.at(mx, my),
                        "{mode} at {res}²: ({x},{y}) vs ({mx},{my})"
                    );
                }
            }
        }
    }
}

#[test]
fn a_continuous_metalmap_really_is_unusable() {
    // A5: exporting the raw suitability mask paints most of the map and trips
    // ZK's indiscrete-metalmap fallback. Pinned so nobody "simplifies" the
    // blob painter away.
    let project = golden_project();
    let d = spring::derive(12, 12);
    let g = Graph::deserialize(&read_json("graph_textured.json"));
    let ctx = Context::new(&project, 129);
    let mask = as_gray(&g.evaluate(g.find_terminal("metal").unwrap(), &ctx));
    let raw = bake_gray(
        &mask,
        d.metal_w as usize,
        d.metal_h as usize,
        8,
        Resample::Bilinear,
        None,
    );
    let painted = raw.iter().filter(|v| **v > 0).count() as f64;
    let frac = painted / (d.metal_w * d.metal_h) as f64;
    assert!(
        frac > 0.2,
        "the raw mask should paint a large fraction, got {frac}"
    );

    let spots = zk::propose_spots(
        &mask,
        &ctx,
        &zk::ProposeOptions {
            count: 14,
            min_separation: 700.0,
            symmetry: "rot180".into(),
            amount: 2.0,
            ..Default::default()
        },
    );
    let blobs = zk::paint_metal_raster(
        &spots,
        &ctx,
        d.metal_w as usize,
        d.metal_h as usize,
        &zk::RasterOptions::default(),
    );
    let painted = blobs.iter().filter(|v| **v > 0).count() as f64;
    let blob_frac = painted / (d.metal_w * d.metal_h) as f64;
    assert!(blob_frac < 0.01, "blobs painted {blob_frac}");
    assert!(
        zk::count_blobs(&blobs, d.metal_w as usize, d.metal_h as usize)
            >= zk::Zk::INDISCRETE_MIN_SPOTS,
        "fewer blobs than ZK's fallback threshold would be a build-anywhere map"
    );
}

#[test]
fn category_layers_are_never_interpolated() {
    // A6/A7: bilinear resampling of a typemap invents terrain types.
    let project = golden_project();
    let d = spring::derive(12, 12);
    let g = Graph::deserialize(&read_json("graph_textured.json"));
    let ctx = Context::new(&project, 257);
    let t = as_gray(&g.evaluate(g.find_terminal("type").unwrap(), &ctx));

    let nearest = bake_index(&t, d.type_w as usize, d.type_h as usize, 2);
    let mut set: Vec<u16> = nearest.clone();
    set.sort_unstable();
    set.dedup();
    assert_eq!(
        set,
        vec![0, 1],
        "typemap must only contain declared indices"
    );

    let scaled = bake_gray(
        &t,
        d.type_w as usize,
        d.type_h as usize,
        8,
        Resample::Bilinear,
        None,
    );
    let mut bad: Vec<u16> = scaled;
    bad.sort_unstable();
    bad.dedup();
    assert!(
        bad.len() > 2,
        "the wrong bake really does produce more than two values"
    );
}

#[test]
fn grass_and_typemap_bake_at_the_documented_resolutions() {
    let d = spring::derive(12, 12);
    let mut f = Field::gray(65);
    f.fill(1.0);
    let field: SharedField = Arc::new(f);
    assert_eq!(
        bake_index(&field, d.type_w as usize, d.type_h as usize, 2).len(),
        384 * 384
    );
    assert_eq!(
        bake_gray(
            &field,
            d.grass_w as usize,
            d.grass_h as usize,
            8,
            Resample::Nearest,
            Some(1.0)
        )
        .len(),
        192 * 192
    );
}

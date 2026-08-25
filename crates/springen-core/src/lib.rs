//! # springen-core
//!
//! Resolution-independent field graph, Spring/Recoil size math and Zero-K
//! gameplay semantics. No GPU, no UI, no filesystem assumptions — the CLI and
//! the desktop app both drive this crate, and the golden suite pins it to the
//! browser prototype it was ported from.
//!
//! Three rules run through everything here and are worth knowing before
//! reading any of it:
//!
//! - **No node parameter is expressed in pixels.** Distances are elmos and are
//!   converted per render request, so changing working resolution never
//!   changes the shape of the terrain.
//! - **Category layers are never interpolated.** Typemap and grass indices are
//!   nearest-neighbour resampled; bilinear invents terrain types that do not
//!   exist in `mapinfo.terrainTypes`.
//! - **Determinism is a feature.** Same seed, same output, bit for bit, on
//!   every platform — which is why `fdlibm` is vendored rather than trusting
//!   the host libm.

// Fields are channel-interleaved and layers are sampled on a stride, so index
// loops state the memory layout that iterator adapters would hide.
#![allow(clippy::needless_range_loop)]

pub mod analysis;
pub mod bake;
pub mod env;
pub mod fdlibm;
pub mod field;
pub mod graph;
pub mod lua;
pub mod material;
pub mod nodes;
pub mod noise;
pub mod png;
pub mod preview;
pub mod project;
pub mod ramps;
pub mod raster;
pub mod rng;
pub mod spring;
pub mod starter;
pub mod terrain;
pub mod texture;
pub mod zk;

pub use env::Environment;
pub use field::{clamp01, Field, SharedField, Stats};
pub use graph::{Graph, Node, NodeSpec, PVal, Params, Registry};
pub use material::{Material, MaterialSet};
pub use preview::{Preview, PreviewOptions, ViewMode};
pub use project::{water_level_t, Context, Project};
pub use spring::{derive, Derived};
pub use zk::{MetalSpot, Zk};

//! # springen-archive
//!
//! Blueprint tree generation and packing. This is what turns a node graph into
//! something the engine will actually load: `mapinfo.lua` with the real key
//! set, `mapconfig/` overrides, the SMF and SMT, the SSMF resources, and a
//! `.sdd` folder, `.sd7` or `.sdz` around them.

pub mod bake;
pub mod import;
pub mod mapinfo;
pub mod pack;

pub use bake::{bake, bake_with_progress, BakeOptions, BakeReport, Game, OnStage};
pub use import::{read_map, ImportedMap};
pub use mapinfo::{mapinfo_lua, mapoptions_lua, Resources};
pub use pack::{Blueprint, Source};

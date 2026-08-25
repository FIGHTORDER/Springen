//! # springen-smf
//!
//! Native SMF/SMT writing and reading. This is what replaces mapconv: the
//! prototype could only emit a script and hope the right compiler was
//! installed, with all the input-rescaling surprises that brings.
//!
//! Two facts drive everything here:
//!
//! - The heightmap is a **vertex lattice**, `(mapx + 1)²`. A 1024² heightmap
//!   is wrong for a 16×16 map.
//! - The header's field order is **not** the physical block order. Readers
//!   follow offsets.

pub mod bc1;
pub mod minimap;
pub mod smf;
pub mod smt;

pub use smf::{Header, Layers, SmtRef};
pub use smt::TileSet;

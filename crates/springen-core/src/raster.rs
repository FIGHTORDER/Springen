//! Named rasters a graph can read.
//!
//! A procedural graph produces terrain from parameters. Some terrain does not
//! come from parameters: a map imported from a `.sd7` is a raster, and so is a
//! brush stroke. Rather than give those their own editing mode beside the
//! graph — two halves that would drift apart, which this project has been
//! bitten by twice — they enter the graph through here and become ordinary
//! terrain the moment they arrive.
//!
//! Loading is deliberately *not* done inside node evaluation. A node's `eval`
//! is a pure function of its inputs, parameters and context, which is what
//! makes signature caching and golden parity possible; a node that opened a
//! file would evaluate differently depending on the disk. So whoever builds the
//! `Context` loads the rasters, and the node only looks them up.

use std::collections::BTreeMap;

use crate::field::SharedField;

/// Rasters available to a graph, by name.
///
/// A `BTreeMap` rather than a hash map so iteration order is fixed: anything
/// that can reach a baked file has to be deterministic, and "which raster did
/// we report first" is exactly the kind of thing that silently is not.
#[derive(Clone, Debug, Default)]
pub struct Rasters {
    map: BTreeMap<String, SharedField>,
}

impl Rasters {
    pub fn new() -> Rasters {
        Rasters::default()
    }

    /// The name an imported map's terrain is stored under, and the `import`
    /// node's default.
    pub const TERRAIN: &'static str = "terrain";

    /// The name an imported map's diffuse is stored under, and the
    /// `import_color` node's default. Three channels rather than one.
    pub const DIFFUSE: &'static str = "diffuse";

    pub fn insert(&mut self, name: impl Into<String>, field: SharedField) {
        self.map.insert(name.into(), field);
    }

    pub fn get(&self, name: &str) -> Option<&SharedField> {
        self.map.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn names(&self) -> Vec<&str> {
        self.map.keys().map(String::as_str).collect()
    }

    /// A short signature of what is loaded, for the viewport's cache key.
    ///
    /// Names and sizes only — hashing megabytes of samples on every frame to
    /// discover that nothing changed would cost more than the re-render.
    pub fn signature(&self) -> String {
        self.map
            .iter()
            .map(|(k, v)| format!("{k}:{}", v.res))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;
    use std::sync::Arc;

    #[test]
    fn a_store_reports_what_it_holds() {
        let mut r = Rasters::new();
        assert!(r.is_empty());
        r.insert(Rasters::TERRAIN, Arc::new(Field::gray(65)));
        assert!(r.contains(Rasters::TERRAIN));
        assert_eq!(r.len(), 1);
        assert_eq!(r.names(), vec!["terrain"]);
        assert!(r.get("nothing").is_none());
    }

    #[test]
    fn the_signature_is_stable_whatever_order_things_arrived_in() {
        let build = |order: [&str; 3]| {
            let mut r = Rasters::new();
            for n in order {
                r.insert(n, Arc::new(Field::gray(33)));
            }
            r.signature()
        };
        assert_eq!(
            build(["a", "b", "c"]),
            build(["c", "a", "b"]),
            "iteration order must not depend on insertion order"
        );
    }
}

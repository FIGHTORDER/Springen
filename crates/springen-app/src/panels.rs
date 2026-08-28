//! Panel layout: which inspector panes exist, where each one sits, and how
//! that survives a restart.
//!
//! A pane is either docked in the left rail, docked in the right rail, or
//! floating in its own window. The rails stack their panes in an order the
//! user controls; a floating pane can go anywhere and be resized. Nothing
//! here draws a pane's contents — the app owns those — so adding a pane is a
//! new variant and one match arm, not a new panel.

use std::collections::BTreeMap;

use eframe::egui::{Pos2, Vec2};

/// One inspector pane.
///
/// The discriminants are stable because a saved layout refers to them by
/// `key`. Renaming a variant is fine; changing its key silently resets that
/// pane's position for everyone who has a layout on disk.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Pane {
    Project,
    Node,
    Viewport,
    Measure,
    Manifest,
    Metal,
    StartBoxes,
    Materials,
    Environment,
}

impl Pane {
    pub const ALL: [Pane; 9] = [
        Pane::Project,
        Pane::Node,
        Pane::Viewport,
        Pane::Measure,
        Pane::Manifest,
        Pane::Metal,
        Pane::StartBoxes,
        Pane::Materials,
        Pane::Environment,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Project => "Map",
            Pane::Node => "Node",
            Pane::Viewport => "Viewport",
            Pane::Measure => "Measurements",
            Pane::Manifest => "Manifest",
            Pane::Metal => "Metal",
            Pane::StartBoxes => "Start boxes",
            Pane::Materials => "Materials",
            Pane::Environment => "Environment",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Pane::Project => "project",
            Pane::Node => "node",
            Pane::Viewport => "viewport",
            Pane::Measure => "measure",
            Pane::Manifest => "manifest",
            Pane::Metal => "metal",
            Pane::StartBoxes => "startboxes",
            Pane::Materials => "materials",
            Pane::Environment => "environment",
        }
    }

    fn from_key(k: &str) -> Option<Pane> {
        Pane::ALL.iter().copied().find(|p| p.key() == k)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dock {
    Left,
    Right,
    Float,
}

impl Dock {
    fn key(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Float => "float",
        }
    }
    fn from_key(k: &str) -> Dock {
        match k {
            "left" => Dock::Left,
            "float" => Dock::Float,
            _ => Dock::Right,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PaneState {
    pub dock: Dock,
    pub open: bool,
    pub collapsed: bool,
    /// Position within its rail. Only the relative order matters.
    pub order: i32,
    /// Where a floating pane sits, and how big it is.
    pub pos: Pos2,
    pub size: Vec2,
}

/// Where every pane is.
#[derive(Clone, Debug)]
pub struct Layout {
    panes: BTreeMap<&'static str, PaneState>,
    /// Rail widths, dragged by the user.
    pub left_w: f32,
    pub right_w: f32,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout::stock()
    }
}

impl Layout {
    /// The layout a fresh install opens with: everything in the right rail,
    /// in the order the work tends to go — what the map is, what is selected,
    /// how it measures, then the surfaces it is dressed in.
    pub fn stock() -> Layout {
        let order = [
            Pane::Project,
            Pane::Node,
            Pane::Viewport,
            Pane::Measure,
            Pane::Metal,
            Pane::StartBoxes,
            Pane::Materials,
            Pane::Environment,
            Pane::Manifest,
        ];
        let mut panes = BTreeMap::new();
        for (i, p) in order.iter().enumerate() {
            panes.insert(
                p.key(),
                PaneState {
                    dock: Dock::Right,
                    open: true,
                    collapsed: false,
                    order: i as i32,
                    // Cascaded by more than a title bar: at 344 wide, a 26px
                    // step leaves each pane almost entirely behind the last.
                    pos: Pos2::new(300.0 + 46.0 * i as f32, 96.0 + 38.0 * i as f32),
                    size: Vec2::new(344.0, 340.0),
                },
            );
        }
        Layout {
            panes,
            left_w: 344.0,
            right_w: 344.0,
        }
    }

    pub fn get(&self, p: Pane) -> PaneState {
        self.panes.get(p.key()).copied().unwrap_or(PaneState {
            dock: Dock::Right,
            open: true,
            collapsed: false,
            order: 0,
            pos: Pos2::new(340.0, 140.0),
            size: Vec2::new(344.0, 340.0),
        })
    }

    pub fn set(&mut self, p: Pane, s: PaneState) {
        self.panes.insert(p.key(), s);
    }

    pub fn is_open(&self, p: Pane) -> bool {
        self.get(p).open
    }

    pub fn set_open(&mut self, p: Pane, open: bool) {
        let mut s = self.get(p);
        s.open = open;
        self.set(p, s);
    }

    #[cfg(test)]
    pub fn toggle(&mut self, p: Pane) {
        self.set_open(p, !self.is_open(p));
    }

    /// The open panes docked to one side, in the order they should stack.
    pub fn rail(&self, dock: Dock) -> Vec<Pane> {
        let mut v: Vec<Pane> = Pane::ALL
            .iter()
            .copied()
            .filter(|p| {
                let s = self.get(*p);
                s.open && s.dock == dock
            })
            .collect();
        v.sort_by_key(|p| (self.get(*p).order, p.key()));
        v
    }

    /// Every open floating pane.
    pub fn floating(&self) -> Vec<Pane> {
        self.rail(Dock::Float)
    }

    /// Send a pane to a rail, or set it loose.
    ///
    /// A pane arriving in a rail goes to the bottom rather than to whatever
    /// index it happened to hold somewhere else, which is where the eye
    /// expects it after a move it just made.
    pub fn move_to(&mut self, p: Pane, dock: Dock) {
        let mut s = self.get(p);
        if s.dock == dock && s.open {
            return;
        }
        if dock != Dock::Float {
            let last = self
                .rail(dock)
                .iter()
                .map(|q| self.get(*q).order)
                .max()
                .unwrap_or(-1);
            s.order = last + 1;
        }
        s.dock = dock;
        s.open = true;
        self.set(p, s);
    }

    /// Move a pane up or down its rail by one place.
    pub fn shift(&mut self, p: Pane, delta: i32) {
        let dock = self.get(p).dock;
        if dock == Dock::Float {
            return;
        }
        let rail = self.rail(dock);
        let Some(at) = rail.iter().position(|q| *q == p) else {
            return;
        };
        let to = at as i32 + delta;
        if to < 0 || to as usize >= rail.len() {
            return;
        }
        // Renumber the whole rail so orders stay dense and comparisons stay
        // meaningful after any number of moves.
        let mut next = rail.clone();
        next.swap(at, to as usize);
        for (i, q) in next.iter().enumerate() {
            let mut s = self.get(*q);
            s.order = i as i32;
            self.set(*q, s);
        }
    }

    pub fn set_collapsed(&mut self, p: Pane, collapsed: bool) {
        let mut s = self.get(p);
        s.collapsed = collapsed;
        self.set(p, s);
    }

    pub fn set_float_rect(&mut self, p: Pane, pos: Pos2, size: Vec2) {
        let mut s = self.get(p);
        s.pos = pos;
        s.size = size;
        self.set(p, s);
    }

    /* ------------------------------------------------------- persistence */

    pub fn to_json(&self) -> serde_json::Value {
        let mut panes = serde_json::Map::new();
        for p in Pane::ALL {
            let s = self.get(p);
            panes.insert(
                p.key().to_string(),
                serde_json::json!({
                    "dock": s.dock.key(),
                    "open": s.open,
                    "collapsed": s.collapsed,
                    "order": s.order,
                    "x": s.pos.x, "y": s.pos.y,
                    "w": s.size.x, "h": s.size.y,
                }),
            );
        }
        serde_json::json!({
            "leftWidth": self.left_w,
            "rightWidth": self.right_w,
            "panes": panes,
        })
    }

    /// Read a layout back, filling anything missing from the stock one.
    ///
    /// Tolerant on purpose: a layout is a convenience, and a half-written or
    /// older file should cost the user their pane positions, not their
    /// session.
    pub fn from_json(v: &serde_json::Value) -> Layout {
        let mut out = Layout::stock();
        if let Some(w) = v.get("leftWidth").and_then(|x| x.as_f64()) {
            out.left_w = (w as f32).clamp(180.0, 640.0);
        }
        if let Some(w) = v.get("rightWidth").and_then(|x| x.as_f64()) {
            out.right_w = (w as f32).clamp(180.0, 640.0);
        }
        let Some(panes) = v.get("panes").and_then(|x| x.as_object()) else {
            return out;
        };
        for (k, pv) in panes {
            let Some(p) = Pane::from_key(k) else { continue };
            let mut s = out.get(p);
            if let Some(d) = pv.get("dock").and_then(|x| x.as_str()) {
                s.dock = Dock::from_key(d);
            }
            if let Some(b) = pv.get("open").and_then(|x| x.as_bool()) {
                s.open = b;
            }
            if let Some(b) = pv.get("collapsed").and_then(|x| x.as_bool()) {
                s.collapsed = b;
            }
            if let Some(n) = pv.get("order").and_then(|x| x.as_i64()) {
                s.order = n as i32;
            }
            let f = |key: &str, fallback: f32| {
                pv.get(key)
                    .and_then(|x| x.as_f64())
                    .unwrap_or(f64::from(fallback)) as f32
            };
            s.pos = Pos2::new(f("x", s.pos.x), f("y", s.pos.y));
            s.size = Vec2::new(
                f("w", s.size.x).clamp(180.0, 1200.0),
                f("h", s.size.y).clamp(120.0, 1400.0),
            );
            out.set(p, s);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_survives_a_round_trip() {
        let mut l = Layout::stock();
        l.move_to(Pane::Metal, Dock::Left);
        l.move_to(Pane::Materials, Dock::Float);
        l.set_float_rect(
            Pane::Materials,
            Pos2::new(410.0, 260.0),
            Vec2::new(360.0, 420.0),
        );
        l.set_open(Pane::Manifest, false);
        l.set_collapsed(Pane::Environment, true);
        l.left_w = 240.0;

        let back = Layout::from_json(&l.to_json());
        assert_eq!(back.get(Pane::Metal).dock, Dock::Left);
        assert_eq!(back.get(Pane::Materials).dock, Dock::Float);
        assert_eq!(back.get(Pane::Materials).pos, Pos2::new(410.0, 260.0));
        assert_eq!(back.get(Pane::Materials).size, Vec2::new(360.0, 420.0));
        assert!(!back.is_open(Pane::Manifest));
        assert!(back.get(Pane::Environment).collapsed);
        assert_eq!(back.left_w, 240.0);
        // And the panes that were not touched are still where they were.
        assert_eq!(back.rail(Dock::Right), l.rail(Dock::Right));
    }

    /// Nonsense on disk must not cost anything but the layout.
    #[test]
    fn a_broken_layout_falls_back_instead_of_failing() {
        let junk = serde_json::json!({"panes": {"nope": 3, "metal": "left"}});
        let l = Layout::from_json(&junk);
        assert_eq!(
            l.get(Pane::Metal).dock,
            Layout::stock().get(Pane::Metal).dock
        );
        assert_eq!(
            l.rail(Dock::Right).len(),
            Layout::stock().rail(Dock::Right).len()
        );
    }

    /// A pane sent to a rail lands at the bottom of it, which is where the eye
    /// looks after a move.
    #[test]
    fn a_moved_pane_lands_at_the_end_of_its_new_rail() {
        let mut l = Layout::stock();
        l.move_to(Pane::Project, Dock::Left);
        l.move_to(Pane::Metal, Dock::Left);
        assert_eq!(l.rail(Dock::Left), vec![Pane::Project, Pane::Metal]);
        // And reordering swaps neighbours rather than renumbering wildly.
        l.shift(Pane::Metal, -1);
        assert_eq!(l.rail(Dock::Left), vec![Pane::Metal, Pane::Project]);
        // Off the end is a no-op, not a panic or a wrap.
        l.shift(Pane::Metal, -1);
        assert_eq!(l.rail(Dock::Left), vec![Pane::Metal, Pane::Project]);
    }

    #[test]
    fn closing_a_pane_takes_it_out_of_its_rail() {
        let mut l = Layout::stock();
        let before = l.rail(Dock::Right).len();
        l.set_open(Pane::Metal, false);
        assert_eq!(l.rail(Dock::Right).len(), before - 1);
        l.toggle(Pane::Metal);
        assert_eq!(l.rail(Dock::Right).len(), before);
    }
}

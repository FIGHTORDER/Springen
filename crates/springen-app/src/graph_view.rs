//! The node canvas.
//!
//! The graph node is the one card-like object in Springen: 184px wide, 2px
//! radius, 1px border, a 2px coloured top edge encoding its class, a 22px drag
//! header, and its own evaluated 48² thumbnail. Everything else in the chrome
//! is flat and flush to its rail.

use std::collections::HashMap;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use springen_core::graph::{registry, Graph};

use crate::theme;

/// Pure geometry, so node layout and hit testing are testable without a GPU.
pub mod layout {
    use super::*;

    pub fn body_height(input_count: usize) -> f32 {
        let ports = (input_count as f32) * 16.0;
        8.0 + theme::NODE_THUMB.max(ports) + 8.0
    }

    pub fn node_size(input_count: usize) -> Vec2 {
        Vec2::new(
            theme::NODE_W,
            theme::NODE_HEADER_H + body_height(input_count),
        )
    }

    /// Input ports run down the left edge, evenly spaced inside the body.
    pub fn input_port(origin: Pos2, index: usize, count: usize) -> Pos2 {
        let body_top = origin.y + theme::NODE_HEADER_H;
        let h = body_height(count);
        let step = h / (count as f32 + 1.0);
        Pos2::new(origin.x, body_top + step * (index as f32 + 1.0))
    }

    /// One output port, centred on the right edge.
    pub fn output_port(origin: Pos2, count: usize) -> Pos2 {
        Pos2::new(
            origin.x + theme::NODE_W,
            origin.y + theme::NODE_HEADER_H + body_height(count) / 2.0,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Selected(String),
    /// A link the graph refused, with the reason to surface.
    Rejected(String),
    Connected,
    Disconnected,
    Deleted(String),
}

#[derive(Clone, Debug)]
enum Drag {
    None,
    Node { id: String, grab: Vec2 },
    Wire { from: String },
    Pan,
}

pub struct GraphView {
    pub pan: Vec2,
    pub zoom: f32,
    pub selected: Option<String>,
    drag: Drag,
}

impl Default for GraphView {
    fn default() -> Self {
        GraphView {
            pan: Vec2::new(40.0, 40.0),
            zoom: 0.85,
            selected: None,
            drag: Drag::None,
        }
    }
}

impl GraphView {
    /// Below this the 12px node label is no longer legible.
    pub const MIN_READABLE_ZOOM: f32 = 0.55;

    fn to_screen(&self, canvas: Rect, p: Pos2) -> Pos2 {
        canvas.min + (p.to_vec2() * self.zoom + self.pan)
    }
    fn to_graph(&self, canvas: Rect, p: Pos2) -> Pos2 {
        (((p - canvas.min) - self.pan) / self.zoom).to_pos2()
    }

    /// Frame the whole graph in the available canvas.
    pub fn fit(&mut self, graph: &Graph, canvas: Rect) {
        if graph.nodes.is_empty() {
            return;
        }
        let (mut min, mut max) = (Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN));
        for n in &graph.nodes {
            let inputs = registry()
                .get(&n.type_name)
                .map(|s| s.inputs.len())
                .unwrap_or(0);
            let size = layout::node_size(inputs);
            min.x = min.x.min(n.x as f32);
            min.y = min.y.min(n.y as f32);
            max.x = max.x.max(n.x as f32 + size.x);
            max.y = max.y.max(n.y as f32 + size.y);
        }
        let span = max - min;
        let pad = 48.0;
        let zx = (canvas.width() - pad * 2.0) / span.x.max(1.0);
        let zy = (canvas.height() - pad * 2.0) / span.y.max(1.0);
        // Never zoom out past the point where a node header stops being
        // readable. A wide graph is easier to pan than to squint at.
        self.zoom = zx.min(zy).clamp(Self::MIN_READABLE_ZOOM, 1.6);
        let scaled = span * self.zoom;
        self.pan = Vec2::new(
            (canvas.width() - scaled.x) / 2.0 - min.x * self.zoom,
            (canvas.height() - scaled.y) / 2.0 - min.y * self.zoom,
        );
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &mut Graph,
        thumbs: &HashMap<String, egui::TextureHandle>,
    ) -> Action {
        let canvas = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, theme::SURFACE_CANVAS);
        self.draw_grid(&painter, canvas);

        let mut action = Action::None;

        // Zoom about the pointer, so the graph does not slide away under it.
        if let Some(hover) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                let before = self.to_graph(canvas, hover);
                self.zoom = (self.zoom * (1.0 + scroll * 0.0015)).clamp(0.12, 2.0);
                let after = self.to_graph(canvas, hover);
                self.pan += (after - before) * self.zoom;
            }
        }

        let port_r = 3.5 * self.zoom.clamp(0.5, 1.2);
        let pointer = ui.input(|i| i.pointer.interact_pos());

        // Which port is under the pointer, if any.
        let mut hover_in: Option<(String, &'static str)> = None;
        let mut hover_out: Option<String> = None;
        if let Some(p) = pointer {
            for n in &graph.nodes {
                let Some(spec) = registry().get(&n.type_name) else {
                    continue;
                };
                let origin = Pos2::new(n.x as f32, n.y as f32);
                for (i, port) in spec.inputs.iter().enumerate() {
                    let pp =
                        self.to_screen(canvas, layout::input_port(origin, i, spec.inputs.len()));
                    if (pp - p).length() < port_r + 5.0 {
                        hover_in = Some((n.id.clone(), port));
                    }
                }
                let op = self.to_screen(canvas, layout::output_port(origin, spec.inputs.len()));
                if (op - p).length() < port_r + 5.0 {
                    hover_out = Some(n.id.clone());
                }
            }
        }

        /* -- drag handling ------------------------------------------------ */
        if response.drag_started() {
            self.drag = Drag::Pan;
            if let Some(p) = pointer {
                if let Some(id) = hover_out.clone() {
                    self.drag = Drag::Wire { from: id };
                } else if let Some(hit) = self.node_at(graph, canvas, p) {
                    let node = graph.node(&hit).unwrap();
                    let origin = self.to_screen(canvas, Pos2::new(node.x as f32, node.y as f32));
                    self.drag = Drag::Node {
                        id: hit.clone(),
                        grab: p - origin,
                    };
                    self.selected = Some(hit.clone());
                    action = Action::Selected(hit);
                }
            }
        }
        if response.dragged() {
            match &self.drag {
                Drag::Pan => self.pan += response.drag_delta(),
                Drag::Node { id, grab } => {
                    if let (Some(p), Some(node)) = (pointer, graph.node_mut(id)) {
                        let g = (((p - *grab) - canvas.min.to_vec2()) - self.pan) / self.zoom;
                        node.x = g.x as f64;
                        node.y = g.y as f64;
                    }
                }
                _ => {}
            }
        }
        if response.drag_stopped() {
            if let Drag::Wire { from } = self.drag.clone() {
                if let Some((to, port)) = hover_in.clone() {
                    match graph.connect(&from, &to, port) {
                        Ok(()) => action = Action::Connected,
                        Err(e) => action = Action::Rejected(e.to_string()),
                    }
                }
            }
            self.drag = Drag::None;
        }

        // Click an occupied input port to unwire it.
        if response.clicked() {
            if let Some((id, port)) = hover_in.clone() {
                if graph.node(&id).map(|n| n.inputs.contains_key(port)) == Some(true) {
                    graph.disconnect(&id, port);
                    action = Action::Disconnected;
                }
            } else if let Some(p) = pointer {
                match self.node_at(graph, canvas, p) {
                    Some(hit) => {
                        self.selected = Some(hit.clone());
                        action = Action::Selected(hit);
                    }
                    None => self.selected = None,
                }
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            if let Some(id) = self.selected.clone() {
                graph.remove(&id);
                self.selected = None;
                action = Action::Deleted(id);
            }
        }

        /* -- wires, under the nodes --------------------------------------- */
        for n in &graph.nodes {
            let Some(spec) = registry().get(&n.type_name) else {
                continue;
            };
            let origin = Pos2::new(n.x as f32, n.y as f32);
            for (i, port) in spec.inputs.iter().enumerate() {
                let Some(src_id) = n.inputs.get(*port) else {
                    continue;
                };
                let Some(src) = graph.node(src_id) else {
                    continue;
                };
                let src_spec = registry().get(&src.type_name);
                let src_in = src_spec.map(|s| s.inputs.len()).unwrap_or(0);
                let a = self.to_screen(
                    canvas,
                    layout::output_port(Pos2::new(src.x as f32, src.y as f32), src_in),
                );
                let b = self.to_screen(canvas, layout::input_port(origin, i, spec.inputs.len()));
                self.draw_wire(&painter, a, b, theme::GRAY_650);
            }
        }
        if let Drag::Wire { from } = &self.drag {
            if let (Some(src), Some(p)) = (graph.node(from), pointer) {
                let n_in = registry()
                    .get(&src.type_name)
                    .map(|s| s.inputs.len())
                    .unwrap_or(0);
                let a = self.to_screen(
                    canvas,
                    layout::output_port(Pos2::new(src.x as f32, src.y as f32), n_in),
                );
                self.draw_wire(&painter, a, p, theme::ACCENT);
            }
        }

        /* -- nodes --------------------------------------------------------- */
        let ids: Vec<String> = graph.nodes.iter().map(|n| n.id.clone()).collect();
        for id in ids {
            let Some(node) = graph.node(&id) else {
                continue;
            };
            let Some(spec) = registry().get(&node.type_name) else {
                continue;
            };
            let origin = Pos2::new(node.x as f32, node.y as f32);
            let top_left = self.to_screen(canvas, origin);
            let size = layout::node_size(spec.inputs.len()) * self.zoom;
            let rect = Rect::from_min_size(top_left, size);
            if !canvas.intersects(rect) {
                continue;
            }
            let selected = self.selected.as_deref() == Some(id.as_str());

            painter.rect_filled(
                rect.translate(Vec2::new(0.0, 6.0 * self.zoom)),
                theme::R_CONTROL,
                Color32::from_black_alpha(96),
            );
            painter.rect_filled(rect, theme::R_CONTROL, theme::SURFACE_RAISED);
            painter.rect_stroke(
                rect,
                theme::R_CONTROL,
                Stroke::new(
                    1.0,
                    if selected {
                        theme::ACCENT
                    } else {
                        theme::BORDER_PANEL
                    },
                ),
                egui::StrokeKind::Inside,
            );
            // The 2px coloured top edge encodes the node's class.
            painter.rect_filled(
                Rect::from_min_size(
                    rect.min,
                    Vec2::new(rect.width(), theme::NODE_CLASS_EDGE * self.zoom.max(0.6)),
                ),
                0.0,
                theme::class_colour(spec.cat),
            );

            let header_h = theme::NODE_HEADER_H * self.zoom;
            painter.hline(
                rect.x_range(),
                rect.top() + header_h,
                Stroke::new(1.0, theme::BORDER_HAIRLINE),
            );
            if self.zoom > 0.35 {
                painter.text(
                    Pos2::new(
                        rect.left() + 8.0 * self.zoom,
                        rect.top() + header_h / 2.0 + 1.0,
                    ),
                    egui::Align2::LEFT_CENTER,
                    spec.label,
                    theme::font(theme::FontRole::Ui, 12.0 * self.zoom.clamp(0.6, 1.3)),
                    if selected {
                        theme::ACCENT
                    } else {
                        theme::TEXT_PRIMARY
                    },
                );
            }

            // The node's own evaluated thumbnail.
            let thumb_side = theme::NODE_THUMB * self.zoom;
            let thumb_rect = Rect::from_min_size(
                Pos2::new(
                    rect.left() + 8.0 * self.zoom,
                    rect.top() + header_h + 8.0 * self.zoom,
                ),
                Vec2::splat(thumb_side),
            );
            match thumbs.get(&id) {
                Some(tex) => {
                    painter.image(
                        tex.id(),
                        thumb_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    painter.rect_filled(thumb_rect, 0.0, theme::GRAY_900);
                }
            }

            if self.zoom > 0.4 {
                let tx = thumb_rect.right() + 8.0 * self.zoom;
                painter.text(
                    Pos2::new(tx, thumb_rect.top() + 6.0 * self.zoom),
                    egui::Align2::LEFT_CENTER,
                    &node.type_name,
                    theme::font(theme::FontRole::Mono, 10.0 * self.zoom.clamp(0.6, 1.2)),
                    theme::TEXT_TERTIARY,
                );
            }

            // Ports: pill dots, filled when wired.
            for (i, port) in spec.inputs.iter().enumerate() {
                let pp = self.to_screen(canvas, layout::input_port(origin, i, spec.inputs.len()));
                let wired = node.inputs.contains_key(*port);
                painter.circle(
                    pp,
                    port_r,
                    if wired {
                        theme::SHOAL_500
                    } else {
                        theme::GRAY_900
                    },
                    Stroke::new(1.0, theme::BORDER_STRONG),
                );
                if self.zoom > 0.55 {
                    painter.text(
                        Pos2::new(pp.x + 7.0, pp.y),
                        egui::Align2::LEFT_CENTER,
                        *port,
                        theme::font(theme::FontRole::Ui, 10.0 * self.zoom.clamp(0.7, 1.1)),
                        theme::TEXT_TERTIARY,
                    );
                }
            }
            let op = self.to_screen(canvas, layout::output_port(origin, spec.inputs.len()));
            painter.circle(
                op,
                port_r,
                theme::class_colour(spec.cat),
                Stroke::new(1.0, theme::BORDER_STRONG),
            );
        }

        action
    }

    fn node_at(&self, graph: &Graph, canvas: Rect, p: Pos2) -> Option<String> {
        for n in graph.nodes.iter().rev() {
            let inputs = registry()
                .get(&n.type_name)
                .map(|s| s.inputs.len())
                .unwrap_or(0);
            let rect = Rect::from_min_size(
                self.to_screen(canvas, Pos2::new(n.x as f32, n.y as f32)),
                layout::node_size(inputs) * self.zoom,
            );
            if rect.contains(p) {
                return Some(n.id.clone());
            }
        }
        None
    }

    fn draw_grid(&self, painter: &egui::Painter, canvas: Rect) {
        for (step, colour) in [
            (theme::GRID_MINOR, theme::RULE_1),
            (theme::GRID_MAJOR, theme::RULE_2),
        ] {
            let s = step * self.zoom;
            if s < 6.0 {
                continue;
            }
            let ox = self.pan.x.rem_euclid(s);
            let oy = self.pan.y.rem_euclid(s);
            let mut x = canvas.left() + ox;
            while x < canvas.right() {
                painter.vline(x, canvas.y_range(), Stroke::new(1.0, colour));
                x += s;
            }
            let mut y = canvas.top() + oy;
            while y < canvas.bottom() {
                painter.hline(canvas.x_range(), y, Stroke::new(1.0, colour));
                y += s;
            }
        }
    }

    fn draw_wire(&self, painter: &egui::Painter, a: Pos2, b: Pos2, colour: Color32) {
        let dx = ((b.x - a.x).abs() * 0.5).clamp(24.0, 140.0);
        let shape = egui::epaint::CubicBezierShape::from_points_stroke(
            [a, Pos2::new(a.x + dx, a.y), Pos2::new(b.x - dx, b.y), b],
            false,
            Color32::TRANSPARENT,
            Stroke::new(1.4, colour),
        );
        painter.add(shape);
    }
}

#[cfg(test)]
mod tests {
    use super::layout::*;
    use super::*;

    #[test]
    fn a_node_is_184_wide_with_a_22px_header() {
        let s = node_size(3);
        assert_eq!(s.x, theme::NODE_W);
        assert!(s.y > theme::NODE_HEADER_H + theme::NODE_THUMB);
    }

    #[test]
    fn ports_stay_inside_the_body_and_never_overlap() {
        let origin = Pos2::new(100.0, 50.0);
        for count in 1..=4 {
            let body_top = origin.y + theme::NODE_HEADER_H;
            let body_bottom = body_top + body_height(count);
            let mut prev = f32::MIN;
            for i in 0..count {
                let p = input_port(origin, i, count);
                assert_eq!(p.x, origin.x);
                assert!(p.y > body_top && p.y < body_bottom, "port {i} of {count}");
                assert!(p.y > prev, "ports must be ordered");
                prev = p.y;
            }
            let o = output_port(origin, count);
            assert_eq!(o.x, origin.x + theme::NODE_W);
        }
    }

    #[test]
    fn screen_and_graph_coordinates_round_trip() {
        let v = GraphView {
            zoom: 0.6,
            pan: Vec2::new(31.0, -12.0),
            ..Default::default()
        };
        let canvas = Rect::from_min_size(Pos2::new(212.0, 44.0), Vec2::new(900.0, 600.0));
        let p = Pos2::new(1234.0, 567.0);
        let back = v.to_graph(canvas, v.to_screen(canvas, p));
        assert!((back.x - p.x).abs() < 1e-3 && (back.y - p.y).abs() < 1e-3);
    }

    #[test]
    fn fitting_frames_every_node_when_it_can() {
        let mut v = GraphView::default();
        let g = springen_core::starter::starter_graph("textured");
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(3600.0, 1400.0));
        v.fit(&g, canvas);
        assert!(v.zoom > GraphView::MIN_READABLE_ZOOM);
        for n in &g.nodes {
            let inputs = registry().get(&n.type_name).unwrap().inputs.len();
            let tl = v.to_screen(canvas, Pos2::new(n.x as f32, n.y as f32));
            let rect = Rect::from_min_size(tl, node_size(inputs) * v.zoom);
            assert!(canvas.contains_rect(rect), "{} is off canvas", n.id);
        }
    }

    #[test]
    fn fitting_never_zooms_past_legibility() {
        let mut v = GraphView::default();
        let g = springen_core::starter::starter_graph("textured");
        // A canvas far too small for the whole graph.
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0));
        v.fit(&g, canvas);
        assert_eq!(v.zoom, GraphView::MIN_READABLE_ZOOM);
        // The graph is still centred on the canvas, so panning finds the rest.
        let centre = v.to_graph(canvas, canvas.center());
        let xs: Vec<f32> = g.nodes.iter().map(|n| n.x as f32).collect();
        let mid = (xs.iter().cloned().fold(f32::MAX, f32::min)
            + xs.iter().cloned().fold(f32::MIN, f32::max))
            / 2.0;
        assert!((centre.x - mid).abs() < theme::NODE_W * 1.5);
    }
}

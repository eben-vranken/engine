//! The graph canvas: nodes as boxes, connections as curves. Drag the
//! background (or middle mouse) to pan, wheel to zoom, drag a node to move
//! it, drag from an output (right side) onto another node to connect.
//! The same drawing serves the read-only view of running graphs.

use crate::graph::{Graph, NodeId, NodeKind};
use crate::ui::theme;
use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

const NODE_WIDTH: f32 = 200.0;
const HEADER: f32 = 20.0;
const ROW: f32 = 15.0;
const PAD: f32 = 6.0;
const PORT: f32 = 5.0;

/// Where the canvas looks and what is being dragged.
#[derive(Debug, Clone)]
pub struct CanvasView {
    /// Screen offset of the canvas origin from the canvas' top-left
    /// corner, in points.
    pub pan: Vec2,
    pub zoom: f32,
    /// Dragging a new connection from this output.
    linking: Option<(NodeId, u32)>,
    /// Where the add menu was opened, in canvas units.
    menu_at: glam::Vec2,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self {
            pan: vec2(40.0, 40.0),
            zoom: 1.0,
            linking: None,
            menu_at: glam::Vec2::ZERO,
        }
    }
}

/// What happened on the canvas this frame (the graph itself is changed in
/// place when editable).
#[derive(Default)]
pub struct CanvasEvents {
    /// Click on a node or the background: select it (or nothing).
    pub select: Option<Option<NodeId>>,
    /// A connection was clicked: its index in `links`.
    pub select_link: Option<usize>,
    /// Add this node where the menu was opened.
    pub add: Option<(NodeKind, glam::Vec2)>,
    pub delete: Option<NodeId>,
    pub duplicate: Option<NodeId>,
}

/// What to show highlighted.
pub struct Marks<'a> {
    pub selected: Option<NodeId>,
    pub selected_link: Option<usize>,
    /// Running nodes (the debug view).
    pub active: &'a [NodeId],
    /// Nodes matching FIND.
    pub found: &'a [NodeId],
}

/// Moves the view so `id` sits in the middle of a canvas of `size`.
pub fn center_on(graph: &Graph, view: &mut CanvasView, id: NodeId, size: Vec2) {
    if let Some(node) = graph.node(id) {
        let half = node_size(&node.kind) * 0.5;
        let middle = vec2(node.position.x, node.position.y) + half;
        view.pan = size * 0.5 - middle * view.zoom;
    }
}

fn node_size(kind: &NodeKind) -> Vec2 {
    let rows = kind.summary().len().max(kind.outputs().len()).max(1);
    vec2(NODE_WIDTH, HEADER + PAD * 2.0 + rows as f32 * ROW)
}

/// Draws `graph` in `rect`; with `editable`, nodes can be moved and
/// connected (the graph changes in place) and menus offered.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    graph: &mut Graph,
    view: &mut CanvasView,
    marks: &Marks,
    editable: bool,
) -> CanvasEvents {
    let mut events = CanvasEvents::default();
    let id = ui.scope_id().with("graph_canvas");
    let background = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BACKGROUND);

    let pointer = ui.input(|i| i.pointer.hover_pos());
    // Zoom about the pointer.
    if let Some(p) = pointer.filter(|p| rect.contains(*p)) {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let old = view.zoom;
            view.zoom = (old * (scroll * 0.002).exp()).clamp(0.3, 2.5);
            let anchor = p - rect.min;
            view.pan = anchor - (anchor - view.pan) * (view.zoom / old);
        }
    }
    // Pan: drag the background, or middle mouse anywhere on the canvas.
    if background.dragged() && view.linking.is_none() {
        view.pan += background.drag_delta();
    }
    let middle = ui.input(|i| i.pointer.middle_down());
    if middle && pointer.is_some_and(|p| rect.contains(p)) {
        view.pan += ui.input(|i| i.pointer.delta());
    }

    let zoom = view.zoom;
    let origin = rect.min + view.pan;
    let to_screen = |p: glam::Vec2| origin + vec2(p.x, p.y) * zoom;
    let to_canvas = |p: Pos2| {
        let v = (p - origin) / zoom;
        glam::Vec2::new(v.x, v.y)
    };

    draw_grid(&painter, rect, origin, zoom);

    // Where everything is on screen.
    let rects: Vec<(NodeId, Rect)> = graph
        .nodes
        .iter()
        .map(|n| {
            (
                n.id,
                Rect::from_min_size(to_screen(n.position), node_size(&n.kind) * zoom),
            )
        })
        .collect();
    let rect_of = |id: NodeId| rects.iter().find(|(n, _)| *n == id).map(|(_, r)| *r);
    let output_at = |r: Rect, output: u32| {
        pos2(
            r.right(),
            r.top() + (HEADER + PAD + ROW * (output as f32 + 0.5)) * zoom,
        )
    };
    let input_at = |r: Rect| pos2(r.left(), r.top() + HEADER * 0.5 * zoom);

    // Connections.
    let mut curves = Vec::new();
    for (i, link) in graph.links.iter().enumerate() {
        let (Some(a), Some(b)) = (rect_of(link.from), rect_of(link.to)) else {
            continue;
        };
        let from = output_at(a, link.output);
        let to = input_at(b);
        let selected = marks.selected_link == Some(i);
        let live = marks.active.contains(&link.to);
        let color = if selected || live {
            theme::AMBER
        } else {
            theme::AMBER_DIM
        };
        let width = if selected { 2.5 } else { 1.2 };
        let points = curve(from, to, zoom);
        painter.add(egui::Shape::line(points.clone(), Stroke::new(width, color)));
        curves.push((i, points));
    }

    // Nodes, in order (later ones on top).
    let font = egui::FontId::monospace(11.0 * zoom);
    let small = egui::FontId::monospace(10.0 * zoom);
    let mut hovered_node = None;
    for node in &mut graph.nodes {
        let Some(r) = rect_of(node.id) else { continue };
        let selected = marks.selected == Some(node.id);
        let active = marks.active.contains(&node.id);
        let found = marks.found.contains(&node.id);
        let note = matches!(node.kind, NodeKind::Note { .. });
        let response = ui.interact(r, id.with(node.id.0), Sense::click_and_drag());
        if response.hovered() {
            hovered_node = Some(node.id);
        }
        if editable {
            if response.drag_started() {
                events.select = Some(Some(node.id));
            }
            if response.dragged() && view.linking.is_none() {
                let d = response.drag_delta() / zoom;
                node.position += glam::Vec2::new(d.x, d.y);
            }
            response.context_menu(|ui| {
                if ui.button("DUPLICATE").clicked() {
                    events.duplicate = Some(node.id);
                    ui.close();
                }
                if ui.button("DELETE").clicked() {
                    events.delete = Some(node.id);
                    ui.close();
                }
            });
        }
        if response.clicked() || response.secondary_clicked() {
            events.select = Some(Some(node.id));
        }

        let fill = if active {
            theme::AMBER.gamma_multiply(0.25)
        } else if note {
            theme::BACKGROUND
        } else {
            Color32::from_rgb(18, 14, 10)
        };
        let stroke = if selected {
            Stroke::new(2.0, theme::AMBER)
        } else if found {
            Stroke::new(2.0, theme::AMBER_DIM)
        } else if active {
            Stroke::new(1.5, theme::AMBER)
        } else {
            Stroke::new(1.0, theme::LINE)
        };
        painter.rect_filled(r, 0.0, fill);
        painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
        let header = Rect::from_min_size(r.min, vec2(r.width(), HEADER * zoom));
        if !note {
            painter.line_segment(
                [header.left_bottom(), header.right_bottom()],
                Stroke::new(1.0, theme::AMBER_DIM),
            );
        }
        let title_color = if note { theme::AMBER_DIM } else { theme::AMBER };
        painter.text(
            header.left_center() + vec2(PAD * zoom + if note { 0.0 } else { 6.0 * zoom }, 0.0),
            egui::Align2::LEFT_CENTER,
            node.kind.title(),
            font.clone(),
            title_color,
        );
        if !note {
            painter.text(
                header.right_center() - vec2(PAD * zoom, 0.0),
                egui::Align2::RIGHT_CENTER,
                node.kind.category().tag(),
                small.clone(),
                theme::AMBER_DIM,
            );
        }
        for (i, line) in node.kind.summary().iter().enumerate() {
            painter.text(
                r.min + vec2(PAD, HEADER + PAD + ROW * i as f32) * zoom,
                egui::Align2::LEFT_TOP,
                line,
                small.clone(),
                if note { theme::AMBER_DIM } else { theme::AMBER },
            );
        }
        if node.kind.has_input() {
            painter.circle_filled(input_at(r), PORT * zoom, theme::AMBER_DIM);
        }
    }

    // Outputs: drawn last and on top so they can be grabbed.
    for node in &graph.nodes {
        let Some(r) = rect_of(node.id) else { continue };
        let outputs = node.kind.outputs();
        for (i, name) in outputs.iter().enumerate() {
            let at = output_at(r, i as u32);
            let port = Rect::from_center_size(at, Vec2::splat(PORT * 3.0 * zoom));
            let linking = view.linking == Some((node.id, i as u32));
            let color = if linking {
                theme::AMBER
            } else {
                theme::AMBER_DIM
            };
            painter.circle_filled(at, PORT * zoom, color);
            if outputs.len() > 1 {
                painter.text(
                    at - vec2(PORT * 2.0 * zoom, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    *name,
                    small.clone(),
                    theme::AMBER_DIM,
                );
            }
            if !editable {
                continue;
            }
            let response = ui
                .interact(port, id.with(("out", node.id.0, i)), Sense::drag())
                .on_hover_cursor(CursorIcon::Crosshair)
                .on_hover_text("DRAG ONTO A NODE TO CONNECT");
            if response.drag_started() {
                view.linking = Some((node.id, i as u32));
            }
        }
    }

    // A connection being dragged: follows the pointer, made on release
    // over a node.
    if let Some((from, output)) = view.linking {
        if let (Some(a), Some(p)) = (rect_of(from), pointer) {
            let points = curve(output_at(a, output), p, zoom);
            painter.add(egui::Shape::line(points, Stroke::new(1.5, theme::AMBER)));
        }
        if ui.input(|i| i.pointer.any_released()) {
            let target = pointer.and_then(|p| {
                rects
                    .iter()
                    .rev()
                    .find(|(_, r)| r.contains(p))
                    .map(|(id, _)| *id)
            });
            if let Some(to) = target {
                graph.connect(from, output, to);
            }
            view.linking = None;
        }
    }

    // An overview in the corner: every node, and what the canvas shows.
    if !graph.nodes.is_empty() && rect.width() > 400.0 {
        let map = Rect::from_min_size(rect.right_bottom() - vec2(190.0, 120.0), vec2(180.0, 110.0));
        let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
        for node in &graph.nodes {
            let s = node_size(&node.kind);
            lo = lo.min(node.position);
            hi = hi.max(node.position + glam::Vec2::new(s.x, s.y));
        }
        let shown_lo = to_canvas(rect.min);
        let shown_hi = to_canvas(rect.max);
        lo = lo.min(shown_lo);
        hi = hi.max(shown_hi);
        let span = (hi - lo).max(glam::Vec2::splat(1.0));
        let inner = map.shrink(6.0);
        let scale = (inner.width() / span.x).min(inner.height() / span.y);
        let at = |p: glam::Vec2| inner.min + vec2((p.x - lo.x) * scale, (p.y - lo.y) * scale);
        painter.rect_filled(map, 0.0, theme::BACKGROUND);
        painter.rect_stroke(
            map,
            0.0,
            Stroke::new(1.0, theme::LINE),
            egui::StrokeKind::Inside,
        );
        for node in &graph.nodes {
            let s = node_size(&node.kind);
            let r = Rect::from_min_max(
                at(node.position),
                at(node.position + glam::Vec2::new(s.x, s.y)),
            );
            let color = if marks.selected == Some(node.id) || marks.active.contains(&node.id) {
                theme::AMBER
            } else {
                theme::LINE
            };
            painter.rect_filled(r, 0.0, color);
        }
        painter.rect_stroke(
            Rect::from_min_max(at(shown_lo), at(shown_hi)),
            0.0,
            Stroke::new(1.0, theme::AMBER_DIM),
            egui::StrokeKind::Inside,
        );
        let response = ui.interact(map, id.with("overview"), Sense::click_and_drag());
        if (response.clicked() || response.dragged())
            && let Some(p) = response.interact_pointer_pos()
        {
            let target =
                lo + glam::Vec2::new((p.x - inner.min.x) / scale, (p.y - inner.min.y) / scale);
            view.pan = rect.size() * 0.5 - vec2(target.x, target.y) * zoom;
        }
    }

    // Background clicks: a connection near the pointer, or nothing.
    if background.clicked() && hovered_node.is_none() {
        let near = pointer.and_then(|p| {
            curves
                .iter()
                .filter_map(|(i, points)| {
                    let d = points
                        .windows(2)
                        .map(|w| distance_to_segment(p, w[0], w[1]))
                        .fold(f32::MAX, f32::min);
                    (d < 6.0).then_some((*i, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
        match near {
            Some(i) => events.select_link = Some(i),
            None => events.select = Some(None),
        }
    }
    if editable {
        if background.secondary_clicked()
            && let Some(p) = pointer
        {
            view.menu_at = to_canvas(p);
        }
        let at = view.menu_at;
        background.context_menu(|ui| {
            if let Some(kind) = add_menu(ui) {
                events.add = Some((kind, at));
            }
        });
    }
    events
}

/// The node types by category; returns the one picked.
pub fn add_menu(ui: &mut Ui) -> Option<NodeKind> {
    let mut picked = None;
    ui.set_min_width(180.0);
    for category in crate::graph::Category::ALL {
        ui.menu_button(category.title(), |ui| {
            ui.set_min_width(180.0);
            for kind in NodeKind::all()
                .into_iter()
                .filter(|k| k.category() == category)
            {
                if ui.button(kind.title()).clicked() {
                    picked = Some(kind);
                    ui.close();
                }
            }
        });
    }
    picked
}

/// A connection from an output to an input: leaves to the right, arrives
/// from the left.
fn curve(from: Pos2, to: Pos2, zoom: f32) -> Vec<Pos2> {
    let reach = ((to.x - from.x).abs() * 0.5).max(40.0 * zoom);
    let (c1, c2) = (from + vec2(reach, 0.0), to - vec2(reach, 0.0));
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let p = from.to_vec2() * (u * u * u)
                + c1.to_vec2() * (3.0 * u * u * t)
                + c2.to_vec2() * (3.0 * u * t * t)
                + to.to_vec2() * (t * t * t);
            p.to_pos2()
        })
        .collect()
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Faint dots every 40 canvas units.
fn draw_grid(painter: &egui::Painter, rect: Rect, origin: Pos2, zoom: f32) {
    let step = 40.0 * zoom;
    if step < 8.0 {
        return;
    }
    let color = theme::AMBER_DIM.gamma_multiply(0.25);
    let start_x = rect.left() + (origin.x - rect.left()).rem_euclid(step);
    let start_y = rect.top() + (origin.y - rect.top()).rem_euclid(step);
    let mut x = start_x;
    while x < rect.right() {
        let mut y = start_y;
        while y < rect.bottom() {
            painter.circle_filled(pos2(x, y), 0.8, color);
            y += step;
        }
        x += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_run_from_output_to_input() {
        let points = curve(pos2(0.0, 0.0), pos2(100.0, 50.0), 1.0);
        assert_eq!(points.first(), Some(&pos2(0.0, 0.0)));
        assert_eq!(points.last(), Some(&pos2(100.0, 50.0)));
        assert!(distance_to_segment(pos2(5.0, 5.0), pos2(0.0, 0.0), pos2(10.0, 0.0)) == 5.0);
    }
}

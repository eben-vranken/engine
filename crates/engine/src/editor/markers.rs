//! Markers for objects with no geometry of their own (the PlayerStart, zone
//! volumes, portals), drawn over the viewport so they can be seen and
//! clicked.

use super::gizmo::View;
use crate::scene::{Component, Scene};
use crate::ui::theme;
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui};
use glam::{Mat4, Vec2, Vec3, Vec4};

/// About eye height, for the marker's post (meters).
pub const POST_HEIGHT: f32 = 1.7;
const ARROW_LENGTH: f32 = 0.7;

/// Draws every PlayerStart as a post with an arrow along its facing.
/// Returns the object clicked, if any, and whether the pointer is over a
/// marker (so the click isn't also a viewport pick).
pub fn player_starts(
    ui: &Ui,
    view: &View,
    scene: &Scene,
    selected: Option<usize>,
) -> (Option<usize>, bool) {
    let painter = ui.painter().with_clip_rect(view.image);
    let (mut clicked, mut hovered) = (None, false);
    for (index, object) in scene.objects.iter().enumerate() {
        if !object.components.contains(&Component::PlayerStart) {
            continue;
        }
        let t = &object.transform;
        let forward = (t.quat() * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
        let (Some(base), Some(head), Some(tip)) = (
            project(view, t.position),
            project(view, t.position + Vec3::Y * POST_HEIGHT),
            project(view, t.position + forward * ARROW_LENGTH),
        ) else {
            continue;
        };
        let is_selected = selected == Some(index);
        let color = if is_selected {
            theme::AMBER
        } else {
            theme::AMBER_DIM
        };
        let stroke = Stroke::new(if is_selected { 2.0 } else { 1.0 }, color);
        painter.line_segment([base, head], stroke);
        painter.circle_stroke(base, 4.0, stroke);
        painter.arrow(base, tip - base, stroke);
        let label = painter.text(
            head,
            Align2::CENTER_BOTTOM,
            "START",
            FontId::monospace(11.0),
            color,
        );
        // Selected: the gizmo sits on the marker and takes the clicks.
        if is_selected || view.looking {
            continue;
        }
        let area = Rect::from_points(&[base, head, tip])
            .union(label)
            .expand(6.0);
        let response = ui.interact(
            area,
            egui::Id::unique(("player_start", index)),
            Sense::click(),
        );
        hovered |= response.hovered();
        if response.clicked() {
            clicked = Some(index);
        }
    }
    (clicked, hovered)
}

/// Size of a portal's frame (meters): a doorway, standing on its origin.
pub const PORTAL_SIZE: Vec2 = Vec2::new(1.0, 2.0);

/// Draws every zone volume as a wire box labelled with its zone, and every
/// portal as a doorway frame labelled with the zone it leads to. Markers in
/// `active_zones` (the simulation radius) are bright. Clicking a label selects the object. Returns
/// the object clicked, if any, and whether the pointer is over a label.
pub fn zones(
    ui: &Ui,
    view: &View,
    scene: &Scene,
    selected: Option<usize>,
    active_zones: &[String],
) -> (Option<usize>, bool) {
    let painter = ui.painter().with_clip_rect(view.image);
    let (mut clicked, mut hovered) = (None, false);
    for (index, object) in scene.objects.iter().enumerate() {
        for component in &object.components {
            let (corners, edges, label_at, text): (Vec<Vec3>, &[(usize, usize)], Vec3, String) =
                match component {
                    Component::ZoneVolume(volume) => {
                        let matrix = object.transform.matrix() * Mat4::from_scale(volume.size);
                        let corners = (0..8)
                            .map(|i| {
                                let unit = Vec3::new(
                                    (i & 1) as f32 - 0.5,
                                    ((i >> 1) & 1) as f32 - 0.5,
                                    ((i >> 2) & 1) as f32 - 0.5,
                                );
                                matrix.transform_point3(unit)
                            })
                            .collect();
                        let top = matrix.transform_point3(Vec3::Y * 0.5);
                        (corners, &BOX_EDGES, top, object.zone.to_uppercase())
                    }
                    Component::Portal(portal) => {
                        let matrix = object.transform.matrix();
                        let half = PORTAL_SIZE.x / 2.0;
                        let corners = [
                            Vec3::new(-half, 0.0, 0.0),
                            Vec3::new(half, 0.0, 0.0),
                            Vec3::new(half, PORTAL_SIZE.y, 0.0),
                            Vec3::new(-half, PORTAL_SIZE.y, 0.0),
                        ]
                        .map(|c| matrix.transform_point3(c))
                        .to_vec();
                        let top = matrix.transform_point3(Vec3::Y * PORTAL_SIZE.y);
                        let to = if portal.to.0.is_empty() {
                            "?".to_owned()
                        } else {
                            portal.to.0.to_uppercase()
                        };
                        let text = format!("{} <> {to}", object.zone.to_uppercase());
                        (corners, &FRAME_EDGES, top, text)
                    }
                    _ => continue,
                };
            let is_selected = selected == Some(index);
            let bright = is_selected || active_zones.contains(&object.zone);
            let color = if bright {
                theme::AMBER
            } else {
                theme::AMBER_DIM
            };
            let stroke = Stroke::new(if is_selected { 2.0 } else { 1.0 }, color);
            for &(a, b) in edges {
                if let Some(segment) = project_segment(view, corners[a], corners[b]) {
                    painter.line_segment(segment, stroke);
                }
            }
            let Some(at) = project(view, label_at) else {
                continue;
            };
            let label = painter.text(
                at,
                Align2::CENTER_BOTTOM,
                text,
                FontId::monospace(11.0),
                color,
            );
            if is_selected || view.looking || !view.image.intersects(label) {
                continue;
            }
            let response = ui.interact(
                label.expand(4.0),
                egui::Id::unique(("zone_marker", index)),
                Sense::click(),
            );
            hovered |= response.hovered();
            if response.clicked() {
                clicked = Some(index);
            }
        }
    }
    (clicked, hovered)
}

/// The 12 edges of a box whose corners are numbered by bits (x, y, z).
const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];
const FRAME_EDGES: [(usize, usize); 4] = [(0, 1), (1, 2), (2, 3), (3, 0)];

/// A world point on screen (egui points); `None` if it's behind the camera.
fn project(view: &View, point: Vec3) -> Option<Pos2> {
    let clip = clip_space(view) * point.extend(1.0);
    (clip.w > view.camera.near).then(|| to_screen(view, clip))
}

/// A world line on screen, cut off where it passes behind the camera (so
/// the edges of a box around the camera still show).
fn project_segment(view: &View, a: Vec3, b: Vec3) -> Option<[Pos2; 2]> {
    let matrix = clip_space(view);
    let (mut a, mut b) = (matrix * a.extend(1.0), matrix * b.extend(1.0));
    let near = view.camera.near;
    if a.w <= near && b.w <= near {
        return None;
    }
    if a.w <= near {
        a = a.lerp(b, (near - a.w) / (b.w - a.w));
    } else if b.w <= near {
        b = b.lerp(a, (near - b.w) / (a.w - b.w));
    }
    Some([to_screen(view, a), to_screen(view, b)])
}

fn clip_space(view: &View) -> Mat4 {
    let aspect = view.image.width() / view.image.height().max(1.0);
    view.camera.view_projection(aspect)
}

fn to_screen(view: &View, clip: Vec4) -> Pos2 {
    let image = view.image;
    let ndc = clip.truncate() / clip.w;
    Pos2::new(
        image.min.x + (ndc.x + 1.0) * 0.5 * image.width(),
        image.min.y + (1.0 - ndc.y) * 0.5 * image.height(),
    )
}

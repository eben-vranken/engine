//! Markers for objects with no geometry of their own (the PlayerStart),
//! drawn over the viewport so they can be seen and clicked.

use super::gizmo::View;
use crate::scene::{Component, Scene};
use crate::ui::theme;
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui};
use glam::Vec3;

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

/// A world point on screen (egui points); `None` if it's behind the camera.
fn project(view: &View, point: Vec3) -> Option<Pos2> {
    let image = view.image;
    let aspect = image.width() / image.height().max(1.0);
    let clip = view.camera.view_projection(aspect) * point.extend(1.0);
    if clip.w <= view.camera.near {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Pos2::new(
        image.min.x + (ndc.x + 1.0) * 0.5 * image.width(),
        image.min.y + (1.0 - ndc.y) * 0.5 * image.height(),
    ))
}

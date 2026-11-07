//! The modeler's view, Blender style: the camera turns around a point with
//! the middle mouse button, pans with Shift held, zooms with the wheel (or
//! Ctrl held), and the numpad or the axis ball in the top-right corner
//! jumps to front / side / top views, orthographic. Instead of a floor,
//! the world axes run through the origin.

use super::gizmo::{AXIS_RGB, View};
use super::markers::{project, project_segment};
use super::model_tab::ModelDoc;
use crate::camera::Camera;
use crate::input::Input;
use crate::ui::theme;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui};
use glam::Vec3;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use winit::keyboard::KeyCode;

/// How the modeler's camera looks at the model.
#[derive(Debug, Clone, Copy)]
pub struct Orbit {
    /// The point it turns around.
    pub target: Vec3,
    /// How far back it is (orthographic: how much it shows).
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub ortho: bool,
    /// Orthographic only because an axis view was picked: turning away
    /// goes back to perspective (Blender's "auto perspective").
    auto_ortho: bool,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 3.5,
            yaw: 0.0,
            pitch: 0.0,
            ortho: false,
            auto_ortho: false,
        }
    }
}

/// Radians per pixel of mouse movement.
const TURN: f32 = 0.005;
/// Numpad 2/4/6/8 turn this far.
const STEP: f32 = PI / 12.0;
/// Just short of straight down, where "up" stops making sense.
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.002;
/// Each wheel step zooms by this much.
const ZOOM: f32 = 0.85;
/// An orthographic camera stands at least this far back, so the whole
/// model is in front of it however far in it's zoomed.
const ORTHO_BACK: f32 = 100.0;

/// The six axis views: the axis looked from, which side, and the
/// camera's yaw and pitch for it. Looking from +Z is the front.
const VIEWS: [(usize, bool, f32, f32); 6] = [
    (0, true, -FRAC_PI_2, 0.0),
    (0, false, FRAC_PI_2, 0.0),
    (1, true, 0.0, -PITCH_LIMIT),
    (1, false, 0.0, PITCH_LIMIT),
    (2, true, 0.0, 0.0),
    (2, false, PI, 0.0),
];

impl Orbit {
    /// Turns by `yaw` and `pitch` radians; leaves an automatic ortho view.
    fn turn(&mut self, yaw: f32, pitch: f32) {
        self.yaw += yaw;
        self.pitch = (self.pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        if self.auto_ortho {
            self.ortho = false;
            self.auto_ortho = false;
        }
    }

    /// Jumps to an axis view, orthographic.
    fn snap(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
        if !self.ortho {
            self.ortho = true;
            self.auto_ortho = true;
        }
    }

    fn toggle_ortho(&mut self) {
        self.ortho = !self.ortho;
        self.auto_ortho = false;
    }

    /// Whether the view is this yaw and pitch (whole turns aside).
    fn is(&self, yaw: f32, pitch: f32) -> bool {
        let dy = (self.yaw - yaw).rem_euclid(TAU);
        dy.min(TAU - dy) < 1e-3 && (self.pitch - pitch).abs() < 1e-3
    }

    /// Puts `camera` where this says.
    pub fn apply(&self, camera: &mut Camera) {
        camera.yaw = self.yaw;
        camera.pitch = self.pitch;
        camera.ortho = self
            .ortho
            .then(|| self.distance * (camera.fov_y.to_radians() / 2.0).tan());
        let back = if self.ortho {
            self.distance.max(ORTHO_BACK)
        } else {
            self.distance
        };
        camera.position = self.target - camera.forward() * back;
    }
}

impl ModelDoc {
    /// Moves `camera` from this frame's input: the middle mouse button
    /// (while it's held the runtime grabs the mouse, `input.looking`), the
    /// wheel and the numpad. `image_height` is the viewport's, in pixels.
    pub fn update_camera(&mut self, camera: &mut Camera, input: &Input, image_height: f32) {
        let held = |keys: &[KeyCode]| keys.iter().any(|&k| input.held(k));
        let shift = held(&[KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let ctrl = held(&[KeyCode::ControlLeft, KeyCode::ControlRight]);
        if input.looking {
            let d = input.mouse_delta;
            if shift {
                // The model follows the mouse.
                let per_pixel =
                    2.0 * camera.half_height(self.orbit.distance) / image_height.max(1.0);
                let right = camera.right();
                let up = right.cross(camera.forward());
                self.orbit.target += (up * d.y - right * d.x) * per_pixel;
            } else if ctrl {
                self.orbit.distance *= 1.01f32.powf(d.y);
            } else {
                self.orbit.turn(d.x * TURN, -d.y * TURN);
            }
        }
        if input.scroll != 0.0 {
            self.orbit.distance *= ZOOM.powf(input.scroll);
        }
        // While transforming, the numpad types numbers instead.
        if !self.transforming() {
            // Front, right and top; Ctrl for the opposite side.
            for (key, plain, opposite) in [
                (KeyCode::Numpad1, 4, 5),
                (KeyCode::Numpad3, 0, 1),
                (KeyCode::Numpad7, 2, 3),
            ] {
                if input.pressed(key) {
                    let (_, _, yaw, pitch) = VIEWS[if ctrl { opposite } else { plain }];
                    self.orbit.snap(yaw, pitch);
                }
            }
            for (key, yaw, pitch) in [
                (KeyCode::Numpad4, STEP, 0.0),
                (KeyCode::Numpad6, -STEP, 0.0),
                (KeyCode::Numpad8, 0.0, -STEP),
                (KeyCode::Numpad2, 0.0, STEP),
            ] {
                if input.pressed(key) {
                    self.orbit.turn(yaw, pitch);
                }
            }
            if input.pressed(KeyCode::Numpad5) {
                self.orbit.toggle_ortho();
            }
            if input.pressed(KeyCode::NumpadAdd) {
                self.orbit.distance *= ZOOM;
            }
            if input.pressed(KeyCode::NumpadSubtract) {
                self.orbit.distance /= ZOOM;
            }
            if input.pressed(KeyCode::NumpadDecimal) {
                self.frame(camera, true);
            }
            if input.pressed(KeyCode::Home) {
                self.frame(camera, false);
            }
        }
        self.orbit.distance = self.orbit.distance.clamp(0.02, 1000.0);
        self.orbit.apply(camera);
    }

    /// Centers the view on the selection (or the whole model) and backs
    /// off until it fits.
    pub fn frame(&mut self, camera: &Camera, selection: bool) {
        let points: Vec<Vec3> = if selection {
            self.selected_verts()
                .iter()
                .map(|&v| self.mesh.positions[v as usize])
                .collect()
        } else {
            self.mesh.positions.clone()
        };
        let Some(&first) = points.first() else {
            return;
        };
        let (min, max) = points
            .iter()
            .fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        let radius = ((max - min).length() / 2.0).max(0.05);
        self.orbit.target = (min + max) / 2.0;
        self.orbit.distance = radius / (camera.fov_y.to_radians() / 2.0).sin() * 1.2;
    }

    /// The axis ball in the viewport's top-right corner, as in Blender:
    /// click an axis to look from that side (again for the opposite side),
    /// drag to turn. Below it, PERSP / ORTHO and FRAME. Also names the view
    /// in the top-left corner. Returns whether the pointer is on any of it.
    pub(super) fn view_gizmo(&mut self, ui: &mut Ui, view: &View) -> bool {
        const RADIUS: f32 = 48.0;
        const ARM: f32 = 36.0;
        const BALL: f32 = 9.0;
        let center = view.image.right_top() + egui::vec2(-RADIUS - 14.0, RADIUS + 14.0);
        let rect = Rect::from_center_size(center, egui::Vec2::splat(RADIUS * 2.0));
        let response = ui.interact(
            rect,
            egui::Id::unique("model_view_gizmo"),
            Sense::click_and_drag(),
        );

        let camera = view.camera;
        let (right, forward) = (camera.right(), camera.forward());
        let up = right.cross(forward);
        // The six axis ends, the ones pointing away drawn first.
        let mut ends: Vec<(usize, Vec3, Pos2)> = (0..6)
            .map(|i| {
                let (axis, positive, _, _) = VIEWS[i];
                let d = Vec3::AXES[axis] * if positive { 1.0 } else { -1.0 };
                (i, d, center + egui::vec2(d.dot(right), -d.dot(up)) * ARM)
            })
            .collect();
        ends.sort_by(|a, b| b.1.dot(forward).total_cmp(&a.1.dot(forward)));
        let hit = response
            .hover_pos()
            .and_then(|p| ends.iter().rev().find(|e| e.2.distance(p) <= BALL))
            .map(|e| e.0);

        if response.dragged() {
            let d = response.drag_delta();
            self.orbit.turn(d.x * TURN * 1.6, -d.y * TURN * 1.6);
        } else if response.clicked()
            && let Some(i) = hit
        {
            // The view already showing: the opposite one (its pair).
            let (_, _, yaw, pitch) = VIEWS[i];
            let pick = if self.orbit.is(yaw, pitch) { i ^ 1 } else { i };
            let (_, _, yaw, pitch) = VIEWS[pick];
            self.orbit.snap(yaw, pitch);
        }

        let painter = ui.painter().with_clip_rect(view.image);
        if response.hovered() || response.dragged() {
            painter.circle_filled(
                center,
                RADIUS,
                Color32::from_rgba_unmultiplied(255, 158, 44, 16),
            );
        }
        for &(i, _, at) in &ends {
            let (axis, positive, _, _) = VIEWS[i];
            let [r, g, b] = AXIS_RGB[axis];
            let color = Color32::from_rgb(r, g, b);
            let edge = if hit == Some(i) { theme::AMBER } else { color };
            if positive {
                painter.line_segment([center, at], Stroke::new(2.0, color));
                painter.circle_filled(at, BALL, edge);
                painter.text(
                    at,
                    Align2::CENTER_CENTER,
                    ["X", "Y", "Z"][axis],
                    FontId::monospace(11.0),
                    theme::BACKGROUND,
                );
            } else {
                painter.circle_filled(at, BALL, color.gamma_multiply(0.3));
                painter.circle_stroke(at, BALL, Stroke::new(1.0, edge));
            }
        }

        // PERSP / ORTHO and FRAME under the ball.
        let mut on_buttons = false;
        let below = egui::pos2(center.x, rect.bottom() + 6.0);
        let button = |ui: &mut Ui, x: f32, width: f32, text: &str, hint: &str| {
            let rect = Rect::from_min_size(egui::pos2(x, below.y), egui::vec2(width, 18.0));
            ui.put(rect, egui::Button::new(egui::RichText::new(text).small()))
                .on_hover_text(hint)
        };
        let (mode, hint) = if self.orbit.ortho {
            ("ORTHO", "ORTHOGRAPHIC: CLICK FOR PERSPECTIVE (NUMPAD 5)")
        } else {
            ("PERSP", "PERSPECTIVE: CLICK FOR ORTHOGRAPHIC (NUMPAD 5)")
        };
        let toggle = button(ui, below.x - 52.0, 50.0, mode, hint);
        let frame = button(
            ui,
            below.x + 2.0,
            50.0,
            "FRAME",
            "SHOW THE SELECTION, OR ALL OF IT (NUMPAD . / HOME)",
        );
        on_buttons |= toggle.hovered() || frame.hovered();
        if toggle.clicked() {
            self.orbit.toggle_ortho();
        }
        if frame.clicked() {
            let any = self.selection_count() > 0;
            self.frame(camera, any);
        }

        painter.text(
            view.image.left_top() + egui::vec2(10.0, 10.0),
            Align2::LEFT_TOP,
            format!("{}  {}", view_name(forward), mode),
            FontId::monospace(12.0),
            theme::AMBER_DIM,
        );
        response.hovered() || response.dragged() || on_buttons
    }

    /// The X (red), Y (green) and Z (blue) axes through the origin, hidden
    /// where the model is in front of them.
    pub(super) fn draw_axes(&self, ui: &Ui, view: &View) {
        let painter = ui.painter().with_clip_rect(view.image);
        // Only bits of axis inside the model's outline on screen can be
        // behind it; the rest skip the ray test.
        let outline = self.screen_box(view);
        // Short pieces near the origin, longer ones further out.
        let mut steps = vec![0.0];
        let mut t = 0.01f32;
        while t < 1000.0 {
            steps.push(t);
            t *= 1.1;
        }
        for (i, [r, g, b]) in AXIS_RGB.into_iter().enumerate() {
            let stroke = Stroke::new(1.0, Color32::from_rgb(r, g, b).gamma_multiply(0.8));
            let axis = Vec3::AXES[i];
            for side in [1.0, -1.0] {
                for pair in steps.windows(2) {
                    let (a, b) = (axis * pair[0] * side, axis * pair[1] * side);
                    let Some(segment) = project_segment(view, a, b) else {
                        continue;
                    };
                    let middle = (a + b) / 2.0;
                    let maybe_hidden = match outline {
                        Some(outline) => project(view, middle).is_some_and(|p| outline.contains(p)),
                        None => true,
                    };
                    if maybe_hidden && !self.visible(view, middle) {
                        continue;
                    }
                    painter.line_segment(segment, stroke);
                }
            }
        }
    }

    /// The model's bounding box on screen; `None` if it's empty or partly
    /// behind the camera.
    fn screen_box(&self, view: &View) -> Option<Rect> {
        let first = *self.mesh.positions.first()?;
        let (min, max) = self
            .mesh
            .positions
            .iter()
            .fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        let mut rect = Rect::NOTHING;
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            rect.extend_with(project(view, corner)?);
        }
        Some(rect)
    }
}

/// FRONT, TOP, … when looking straight along an axis, else USER.
fn view_name(forward: Vec3) -> &'static str {
    const NAMES: [(Vec3, &str); 6] = [
        (Vec3::NEG_Z, "FRONT"),
        (Vec3::Z, "BACK"),
        (Vec3::NEG_X, "RIGHT"),
        (Vec3::X, "LEFT"),
        (Vec3::NEG_Y, "TOP"),
        (Vec3::Y, "BOTTOM"),
    ];
    NAMES
        .iter()
        .find(|(d, _)| forward.dot(*d) > 0.9999)
        .map_or("USER", |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modeler::mesh::EditMesh;

    #[test]
    fn camera_stays_on_its_orbit_and_frames_the_model() {
        let mut doc = ModelDoc::new(EditMesh::cube(Vec3::ONE), "test".into());
        let mut camera = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0, 0.0).camera;
        let input = Input::default();
        doc.orbit.target = Vec3::new(1.0, 2.0, 3.0);
        doc.orbit.turn(0.3, -0.2);
        doc.update_camera(&mut camera, &input, 600.0);
        let to_target = doc.orbit.target - camera.position;
        assert!((to_target.length() - doc.orbit.distance).abs() < 1e-4);
        assert!((to_target.normalize() - camera.forward()).length() < 1e-4);

        doc.frame(&camera, false);
        assert!(doc.orbit.target.length() < 1e-5, "the cube is centered");
        assert!(doc.orbit.distance > 0.5);
    }

    #[test]
    fn axis_views_are_orthographic_until_turned_away() {
        let mut orbit = Orbit::default();
        let mut camera = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0, 0.0).camera;
        for (i, (axis, positive, yaw, pitch)) in VIEWS.into_iter().enumerate() {
            orbit.snap(yaw, pitch);
            orbit.apply(&mut camera);
            // Looking from the +axis side means looking along -axis.
            let from = Vec3::AXES[axis] * if positive { 1.0 } else { -1.0 };
            assert!(camera.forward().dot(-from) > 0.999, "view {i}");
            assert!(camera.ortho.is_some());
            assert!(orbit.is(yaw, pitch));
        }
        assert_eq!(view_name(camera.forward()), "BACK");
        orbit.turn(0.1, 0.0);
        orbit.apply(&mut camera);
        assert!(
            camera.ortho.is_none(),
            "turning leaves the automatic ortho view"
        );
        assert_eq!(view_name(camera.forward()), "USER");

        // Chosen on purpose, it stays.
        orbit.toggle_ortho();
        orbit.turn(0.1, 0.0);
        assert!(orbit.ortho);
    }

    #[test]
    fn orthographic_rays_run_forward() {
        let mut camera = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0, 0.0).camera;
        let mut orbit = Orbit::default();
        orbit.snap(0.0, 0.0);
        orbit.apply(&mut camera);
        let (o1, d1) = camera.ray(glam::Vec2::new(-0.5, 0.2), 1.5);
        let (o2, d2) = camera.ray(glam::Vec2::new(0.7, -0.4), 1.5);
        assert!((d1 - camera.forward()).length() < 1e-3);
        assert!((d2 - camera.forward()).length() < 1e-3);
        assert!((o1 - o2).length() > 0.1, "from different points");
    }
}

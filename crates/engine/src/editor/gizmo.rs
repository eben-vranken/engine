//! Move / rotate / scale handles on the selected object, via the
//! `transform-gizmo` crate (which does the hit-testing and math; we draw its
//! triangles with egui). Ctrl snaps, like the inspector fields.

use super::inspector::snap_step;
use crate::camera::Camera;
use crate::scene::Transform;
use crate::ui::theme;
use egui::{Rect, Ui};
use glam::{DQuat, EulerRot, Mat4, Quat, Vec3};
use transform_gizmo::config::GizmoModeKind;
use transform_gizmo::mint;
use transform_gizmo::{
    Gizmo, GizmoConfig, GizmoInteraction, GizmoMode, GizmoOrientation, GizmoResult, GizmoVisuals,
};

pub struct TransformGizmo {
    gizmo: Gizmo,
    pub kind: GizmoModeKind,
    /// A handle is being dragged.
    dragging: bool,
}

impl Default for TransformGizmo {
    fn default() -> Self {
        Self {
            gizmo: Gizmo::default(),
            kind: GizmoModeKind::Translate,
            dragging: false,
        }
    }
}

/// What the gizmo needs to know about the 3D view.
pub struct View<'a> {
    pub camera: &'a Camera,
    /// Where the scene image is, in egui points.
    pub image: Rect,
    /// The camera is in mouse-look mode; the gizmo stays out of the way.
    pub looking: bool,
    /// The surface point and normal straight ahead of the camera, if one
    /// is near enough to place new objects on.
    pub aim: Option<(Vec3, Vec3)>,
}

impl TransformGizmo {
    /// The pointer is over a handle (so a click there isn't a selection).
    pub fn hovered(&self) -> bool {
        self.gizmo.is_focused()
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// Draws the handles around `transform` and returns the new transform
    /// while one is being dragged.
    pub fn ui(&mut self, ui: &Ui, view: &View, transform: &Transform) -> Option<Transform> {
        let image = view.image;
        let aspect = image.width() / image.height().max(1.0);
        let snapping = ui.input(|i| i.modifiers.ctrl);
        self.gizmo.update_config(GizmoConfig {
            view_matrix: rows(view.camera.view()),
            projection_matrix: rows(view.camera.projection(aspect)),
            viewport: transform_gizmo::Rect::from_x_y_ranges(
                image.min.x..=image.max.x,
                image.min.y..=image.max.y,
            ),
            modes: GizmoMode::all_from_kind(self.kind),
            orientation: GizmoOrientation::Global,
            snapping,
            snap_distance: snap_step("position"),
            snap_angle: snap_step("rotation").to_radians(),
            snap_scale: snap_step("scale"),
            visuals: visuals(),
            pixels_per_point: ui.ctx().pixels_per_point(),
            ..Default::default()
        });

        let (pointer, pressed, down) = ui.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
            )
        });
        let pointer = pointer.unwrap_or_default();
        let interaction = GizmoInteraction {
            cursor_pos: (pointer.x, pointer.y),
            hovered: !view.looking && (image.contains(pointer) || self.dragging),
            drag_started: pressed,
            dragging: down,
        };

        let target = to_gizmo(transform);
        let result = self.gizmo.update(interaction, &[target]);
        self.dragging = down && self.gizmo.is_focused();
        self.draw(ui, image);

        let (result, targets) = result?;
        let moved = targets.first()?;
        let mut updated = *transform;
        // Only touch the part being edited, so e.g. moving never rewrites
        // the rotation's Euler angles.
        match result {
            GizmoResult::Translation { .. } => {
                updated.position = vec3(moved.translation);
            }
            GizmoResult::Rotation { .. } | GizmoResult::Arcball { .. } => {
                let q = moved.rotation;
                let rotation = DQuat::from_xyzw(q.v.x, q.v.y, q.v.z, q.s).as_quat();
                updated.rotation = euler_degrees(rotation);
            }
            GizmoResult::Scale { .. } => {
                updated.scale = vec3(moved.scale);
            }
        }
        Some(updated)
    }

    fn draw(&self, ui: &Ui, image: Rect) {
        let data = self.gizmo.draw();
        let mut mesh = egui::Mesh::default();
        for (pos, color) in data.vertices.iter().zip(&data.colors) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(pos[0], pos[1]),
                uv: egui::epaint::WHITE_UV,
                color: egui::Rgba::from_rgba_premultiplied(color[0], color[1], color[2], color[3])
                    .into(),
            });
        }
        mesh.indices = data.indices;
        ui.painter()
            .with_clip_rect(image)
            .add(egui::Shape::mesh(mesh));
    }
}

/// Muted X, Y and Z colors that sit with the amber UI.
pub const AXIS_RGB: [[u8; 3]; 3] = [[220, 70, 60], [110, 190, 80], [70, 130, 230]];

/// The axis colors; amber when hovered.
fn visuals() -> GizmoVisuals {
    let amber = theme::AMBER;
    let [x, y, z] = AXIS_RGB.map(|[r, g, b]| transform_gizmo::Color32::from_rgb(r, g, b));
    GizmoVisuals {
        x_color: x,
        y_color: y,
        z_color: z,
        s_color: transform_gizmo::Color32::from_rgb(210, 200, 190),
        highlight_color: Some(transform_gizmo::Color32::from_rgb(
            amber.r(),
            amber.g(),
            amber.b(),
        )),
        ..Default::default()
    }
}

/// glam matrices are column-major; the gizmo wants rows.
fn rows(m: Mat4) -> mint::RowMatrix4<f64> {
    let row = |i: usize| {
        let r = m.row(i).as_dvec4();
        mint::Vector4 {
            x: r.x,
            y: r.y,
            z: r.z,
            w: r.w,
        }
    };
    mint::RowMatrix4 {
        x: row(0),
        y: row(1),
        z: row(2),
        w: row(3),
    }
}

fn to_gizmo(t: &Transform) -> transform_gizmo::math::Transform {
    let q = t.quat().as_dquat();
    let v = |v: Vec3| mint::Vector3 {
        x: v.x as f64,
        y: v.y as f64,
        z: v.z as f64,
    };
    transform_gizmo::math::Transform {
        scale: v(t.scale),
        rotation: mint::Quaternion {
            v: mint::Vector3 {
                x: q.x,
                y: q.y,
                z: q.z,
            },
            s: q.w,
        },
        translation: v(t.position),
    }
}

fn vec3(v: mint::Vector3<f64>) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Back to the scene's Euler degrees (Y, then X, then Z).
fn euler_degrees(q: Quat) -> Vec3 {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    // Tidy float noise so the inspector shows 45.0, not 44.99999.
    (Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI) * 1000.0).round() / 1000.0 + Vec3::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_round_trips_through_the_gizmo_types() {
        let t = Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Vec3::new(10.0, 45.0, -20.0),
            scale: Vec3::new(1.0, 2.0, 0.5),
        };
        let g = to_gizmo(&t);
        assert_eq!(vec3(g.translation), t.position);
        assert_eq!(vec3(g.scale), t.scale);
        let q = DQuat::from_xyzw(g.rotation.v.x, g.rotation.v.y, g.rotation.v.z, g.rotation.s);
        let back = euler_degrees(q.as_quat());
        assert!((back - t.rotation).length() < 0.01, "{back}");
    }

    #[test]
    fn rows_match_glam() {
        let m = Mat4::from_cols_array(&std::array::from_fn(|i| i as f32));
        let r = rows(m);
        assert_eq!(r.x.y, m.row(0).y as f64);
        assert_eq!(r.w.x, m.row(3).x as f64);
    }
}

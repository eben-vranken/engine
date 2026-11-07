//! Moving, turning and scaling in the modeler, Blender style: G, R or S
//! starts it and the selection follows the mouse until a click (or Enter)
//! keeps it, or a right-click (or Esc) puts it back. X, Y or Z locks it to
//! that axis (again to unlock), typed numbers set the amount exactly, and
//! Ctrl snaps.

use super::gizmo::{AXIS_RGB, View};
use super::inspector::snap_step;
use super::markers::{project, project_segment};
use crate::modeler::mesh::EditMesh;
use crate::ui::theme;
use egui::{Color32, FontId, Pos2, Stroke, Ui};
use glam::{Quat, Vec3};
use std::collections::BTreeSet;
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Move,
    Rotate,
    Scale,
}

/// What the transform is locked to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Axis {
    /// 0, 1, 2: X, Y, Z.
    World(usize),
    /// A direction of its own (unit length), e.g. the normal of extruded
    /// faces.
    Normal(Vec3),
}

impl Axis {
    fn direction(self) -> Vec3 {
        match self {
            Axis::World(i) => Vec3::AXES[i],
            Axis::Normal(n) => n,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Axis::World(0) => "X",
            Axis::World(1) => "Y",
            Axis::World(_) => "Z",
            Axis::Normal(_) => "NORMAL",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Axis::World(i) => {
                let [r, g, b] = AXIS_RGB[i];
                Color32::from_rgb(r, g, b)
            }
            Axis::Normal(_) => theme::AMBER,
        }
    }
}

/// A transform in progress.
pub(super) struct Modal {
    pub kind: Kind,
    /// The mesh before it: put back on cancel, the undo step on confirm.
    pub before: EditMesh,
    /// The corners it moves, and where they started.
    corners: Vec<(u32, Vec3)>,
    pivot: Vec3,
    /// The pointer when it started (egui points).
    start: Pos2,
    pub axis: Option<Axis>,
    /// Digits typed so far.
    typed: String,
    /// Rotation: the angle turned so far and the pointer's last angle, so
    /// whole turns add up.
    turned: f32,
    last_angle: Option<f32>,
    /// What it's doing now, for the viewport.
    readout: String,
}

impl Modal {
    /// Starts moving `verts` of `mesh`; `before` is the mesh to go back to.
    pub fn new(
        kind: Kind,
        before: EditMesh,
        mesh: &EditMesh,
        verts: &BTreeSet<u32>,
        pointer: Pos2,
    ) -> Self {
        let corners: Vec<(u32, Vec3)> = verts
            .iter()
            .map(|&v| (v, mesh.positions[v as usize]))
            .collect();
        let pivot = corners.iter().map(|c| c.1).sum::<Vec3>() / corners.len().max(1) as f32;
        Self {
            kind,
            before,
            corners,
            pivot,
            start: pointer,
            axis: None,
            typed: String::new(),
            turned: 0.0,
            last_angle: None,
            readout: String::new(),
        }
    }

    /// X / Y / Z pressed: lock to that axis, or unlock if already locked.
    pub fn lock(&mut self, axis: usize) {
        self.axis = if self.axis == Some(Axis::World(axis)) {
            None
        } else {
            Some(Axis::World(axis))
        };
    }

    /// Typed text: digits and a point add to the number, minus flips it.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '0'..='9' => self.typed.push(c),
                '.' if !self.typed.contains('.') => self.typed.push(c),
                '-' => match self.typed.strip_prefix('-') {
                    Some(rest) => self.typed = rest.to_string(),
                    None => self.typed.insert(0, '-'),
                },
                _ => {}
            }
        }
    }

    pub fn backspace(&mut self) {
        self.typed.pop();
    }

    fn typed_value(&self) -> Option<f32> {
        match self.typed.as_str() {
            "" => None,
            "-" => Some(-1.0),
            text => text.parse().ok(),
        }
    }

    /// Puts the corners where the pointer (or the typed number) says.
    pub fn update(&mut self, mesh: &mut EditMesh, view: &View, pointer: Pos2, snap: bool) {
        let center = project(view, self.pivot).unwrap_or(view.image.center());
        let typed = self.typed_value();
        let axis = self.axis.map(Axis::direction);
        let along = self
            .axis
            .map_or(String::new(), |a| format!("  ALONG {}", a.name()));
        let typed_text = if self.typed.is_empty() {
            String::new()
        } else {
            format!("  [{}]", self.typed)
        };
        let pivot = self.pivot;
        let mut place = |f: &dyn Fn(Vec3) -> Vec3| {
            for &(v, p) in &self.corners {
                mesh.positions[v as usize] = f(p);
            }
        };
        match self.kind {
            Kind::Move => {
                let offset = match typed {
                    // Typed without an axis: along X, as in Blender.
                    Some(v) => axis.unwrap_or(Vec3::X) * v,
                    None => drag_offset(view, pivot, self.start, center, pointer, axis, snap),
                };
                place(&|p| p + offset);
                self.readout = format!(
                    "MOVE  {:.3}  {:.3}  {:.3}{along}{typed_text}",
                    offset.x, offset.y, offset.z
                );
            }
            Kind::Rotate => {
                let toward_viewer = (view.camera.position - pivot).normalize_or(Vec3::Z);
                let (direction, angle, shown) = match typed {
                    Some(degrees) => (axis.unwrap_or(toward_viewer), degrees.to_radians(), degrees),
                    None => {
                        let now = angle_of(pointer - center);
                        let last = self
                            .last_angle
                            .unwrap_or_else(|| angle_of(self.start - center));
                        self.turned += wrap(now - last);
                        self.last_angle = Some(now);
                        // Screen y points down, so a turn that looks
                        // counter-clockwise has a negative angle there.
                        let mut angle = -self.turned;
                        if snap {
                            let step = snap_step("rotation").to_radians();
                            angle = (angle / step).round() * step;
                        }
                        // Turn the way the pointer goes, as seen from here.
                        let direction = axis.unwrap_or(toward_viewer);
                        let seen = if direction.dot(toward_viewer) < 0.0 {
                            -angle
                        } else {
                            angle
                        };
                        (direction, seen, angle.to_degrees())
                    }
                };
                let q = Quat::from_axis_angle(direction, angle);
                place(&|p| pivot + q * (p - pivot));
                self.readout = format!("ROTATE  {shown:.1}°{along}{typed_text}");
            }
            Kind::Scale => {
                let factor = match typed {
                    Some(f) => f,
                    None => {
                        let f =
                            (pointer - center).length() / (self.start - center).length().max(1.0);
                        if snap {
                            let step = snap_step("scale");
                            (f / step).round() * step
                        } else {
                            f
                        }
                    }
                };
                place(&|p| {
                    let d = p - pivot;
                    pivot
                        + match axis {
                            Some(a) => d + a * d.dot(a) * (factor - 1.0),
                            None => d * factor,
                        }
                });
                self.readout = format!("SCALE  {factor:.3}{along}{typed_text}");
            }
        }
    }

    /// The locked axis through the pivot, a line from the pivot to the
    /// pointer when turning or scaling, and what it's doing.
    pub fn draw(&self, ui: &Ui, view: &View, pointer: Option<Pos2>) {
        let painter = ui.painter().with_clip_rect(view.image);
        if let Some(axis) = self.axis {
            let far = axis.direction() * 1000.0;
            if let Some(segment) = project_segment(view, self.pivot - far, self.pivot + far) {
                painter.line_segment(segment, Stroke::new(1.0, axis.color()));
            }
        }
        if self.kind != Kind::Move
            && let (Some(center), Some(pointer)) = (project(view, self.pivot), pointer)
        {
            painter.add(egui::Shape::dashed_line(
                &[center, pointer],
                Stroke::new(1.0, theme::AMBER_DIM),
                4.0,
                4.0,
            ));
        }
        painter.text(
            // Under the view's name.
            view.image.left_top() + egui::vec2(10.0, 28.0),
            egui::Align2::LEFT_TOP,
            &self.readout,
            FontId::monospace(12.0),
            theme::AMBER,
        );
    }
}

/// How far the pointer's move since `start` takes the pivot: along `axis`
/// as it's drawn on screen, or else in the plane facing the camera.
fn drag_offset(
    view: &View,
    pivot: Vec3,
    start: Pos2,
    center: Pos2,
    pointer: Pos2,
    axis: Option<Vec3>,
    snap: bool,
) -> Vec3 {
    let moved = pointer - start;
    let camera = view.camera;
    let depth = (pivot - camera.position)
        .dot(camera.forward())
        .max(camera.near);
    let step = snap_step("position");
    let round = |x: f32| if snap { (x / step).round() * step } else { x };
    match axis {
        Some(direction) => {
            // Points per meter along the axis, on screen.
            let h = depth * 0.1;
            let Some(tip) = project(view, pivot + direction * h) else {
                return Vec3::ZERO;
            };
            let along = (tip - center) / h;
            if along.length_sq() < 1e-6 {
                return Vec3::ZERO;
            }
            direction * round(moved.dot(along) / along.length_sq())
        }
        None => {
            // Meters per point at the pivot's depth.
            let per_point = 2.0 * camera.half_height(depth) / view.image.height().max(1.0);
            let right = camera.right();
            let up = right.cross(camera.forward());
            let o = (right * moved.x - up * moved.y) * per_point;
            Vec3::new(round(o.x), round(o.y), round(o.z))
        }
    }
}

fn angle_of(v: egui::Vec2) -> f32 {
    v.y.atan2(v.x)
}

/// An angle difference brought into -π..π.
fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    fn view(camera: &Camera) -> View<'_> {
        View {
            camera,
            image: egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)),
            looking: false,
            aim: None,
        }
    }

    fn camera() -> Camera {
        // At +Z looking at the origin.
        Camera {
            position: Vec3::new(0.0, 0.0, 5.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60.0,
            near: 0.05,
            far: 100.0,
            ortho: None,
        }
    }

    fn modal(kind: Kind, mesh: &EditMesh, pointer: Pos2) -> Modal {
        let all: BTreeSet<u32> = (0..mesh.positions.len() as u32).collect();
        Modal::new(kind, mesh.clone(), mesh, &all, pointer)
    }

    #[test]
    fn typed_numbers_set_the_amount() {
        let camera = camera();
        let view = view(&camera);
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let start = mesh.clone();
        let mut m = modal(Kind::Scale, &mesh, Pos2::new(500.0, 300.0));
        m.type_text("2");
        m.update(&mut mesh, &view, Pos2::new(600.0, 300.0), false);
        for (a, b) in mesh.positions.iter().zip(&start.positions) {
            assert!((*a - *b * 2.0).length() < 1e-5);
        }

        let mut mesh = start.clone();
        let mut m = modal(Kind::Move, &mesh, Pos2::new(500.0, 300.0));
        m.lock(1);
        m.type_text("1.5");
        m.type_text("-");
        m.update(&mut mesh, &view, Pos2::new(600.0, 300.0), false);
        assert!((mesh.positions[0] - (start.positions[0] - Vec3::Y * 1.5)).length() < 1e-5);
        m.lock(1);
        assert_eq!(m.axis, None, "the same key again unlocks");
    }

    #[test]
    fn scale_follows_the_pointer_distance_and_axis() {
        let camera = camera();
        let view = view(&camera);
        let center = project(&view, Vec3::ZERO).unwrap();
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let start = mesh.clone();
        let mut m = modal(Kind::Scale, &mesh, center + egui::vec2(50.0, 0.0));
        m.lock(0);
        m.update(&mut mesh, &view, center + egui::vec2(0.0, 100.0), false);
        for (a, b) in mesh.positions.iter().zip(&start.positions) {
            assert!((a.x - b.x * 2.0).abs() < 1e-3, "{a} {b}");
            assert_eq!((a.y, a.z), (b.y, b.z));
        }
    }

    #[test]
    fn moving_right_moves_right_and_rotation_follows_the_pointer() {
        let camera = camera();
        let view = view(&camera);
        let center = project(&view, Vec3::ZERO).unwrap();
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let start = mesh.clone();
        let mut m = modal(Kind::Move, &mesh, center);
        m.update(&mut mesh, &view, center + egui::vec2(40.0, 0.0), false);
        let moved = mesh.positions[0] - start.positions[0];
        assert!(moved.x > 0.0 && moved.y.abs() < 1e-5 && moved.z.abs() < 1e-5);

        // A quarter turn counter-clockwise on screen, seen from +Z, is +90°
        // about Z: +X goes to +Y.
        let mut mesh = start.clone();
        let mut m = modal(Kind::Rotate, &mesh, center + egui::vec2(100.0, 0.0));
        m.update(&mut mesh, &view, center + egui::vec2(70.0, -70.0), false);
        m.update(&mut mesh, &view, center + egui::vec2(0.0, -100.0), false);
        let q = Quat::from_rotation_z(PI / 2.0);
        for (a, b) in mesh.positions.iter().zip(&start.positions) {
            assert!((*a - q * *b).length() < 1e-3, "{a} {b}");
        }
    }
}

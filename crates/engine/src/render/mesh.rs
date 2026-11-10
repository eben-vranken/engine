//! Primitive meshes generated in code, enough to grey-box a level.
//!
//! All shapes are centered on the origin and flat-shaded (every face has its
//! own vertices and normal), which is what makes low-poly read as low-poly.
//! UVs are in meters, so a texture that covers N meters tiles the same on
//! every shape regardless of its size.

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3, Vec3Swizzles};
use std::f32::consts::{PI, TAU};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA, multiplied into the surface color (white = none).
    pub color: [f32; 4],
}

/// No vertex color.
pub const WHITE: [f32; 4] = [1.0; 4];

/// A primitive shape. Sizes are full extents in meters.
#[derive(
    Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, bevy_reflect::Reflect,
)]
pub enum Shape {
    Box {
        size: Vec3,
    },
    /// Horizontal quad facing up.
    Plane {
        size: Vec2,
    },
    /// Ramp: full height at -Z, zero height at +Z.
    Wedge {
        size: Vec3,
    },
    Cylinder {
        radius: f32,
        height: f32,
        segments: u32,
    },
    Sphere {
        radius: f32,
        segments: u32,
        rings: u32,
    },
    /// A solid flight of steps, climbed toward -Z from its +Z end. With a
    /// turn it curves around a center on its left (a negative turn: on its
    /// right).
    Stairs {
        /// Across the flight (meters).
        width: f32,
        /// From the floor to the top of the last step (meters).
        height: f32,
        /// How many steps; each is `height / steps` high.
        steps: u32,
        /// The run of each step (meters) on a straight flight. A turning
        /// flight's run comes from the turn and the inner radius.
        depth: f32,
        /// Degrees turned over the whole flight: 0 straight, 90 a quarter
        /// turn, 360 or more a spiral.
        turn: f32,
        /// From the turn's center to the inside edge (meters); 0 winds the
        /// steps around a central post.
        inner_radius: f32,
    },
}

/// One step of a flight, in the shape's own (centered) space: the four
/// corners of its footprint at floor height (inside and outside at its
/// front, then at its back), and how high its underside and top are above
/// the floor. Steps are solid to the floor, except where a flight turns
/// more than a full circle: a step with another a full turn below it is a
/// slab (two rises thick), so the one below can be walked on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub corners: [Vec3; 4],
    pub bottom: f32,
    pub top: f32,
}

impl Step {
    /// The step as a solid: its footprint at its underside and its top.
    pub fn points(&self) -> [Vec3; 8] {
        let (low, up) = (Vec3::Y * self.bottom, Vec3::Y * self.top);
        let c = self.corners;
        [
            c[0] + low,
            c[1] + low,
            c[2] + low,
            c[3] + low,
            c[0] + up,
            c[1] + up,
            c[2] + up,
            c[3] + up,
        ]
    }
}

impl Shape {
    /// Half the size of the shape's bounding box (shapes are centered on
    /// it).
    pub fn half_extents(&self) -> Vec3 {
        match *self {
            Shape::Box { size } | Shape::Wedge { size } => size * 0.5,
            Shape::Plane { size } => Vec3::new(size.x, 0.0, size.y) * 0.5,
            Shape::Cylinder { radius, height, .. } => Vec3::new(radius, height * 0.5, radius),
            Shape::Sphere { radius, .. } => Vec3::splat(radius),
            Shape::Stairs { .. } => {
                let (min, max) = bounds_of(&self.raw_steps());
                (max - min) * 0.5
            }
        }
    }

    pub const KINDS: [&'static str; 6] = ["box", "plane", "wedge", "cylinder", "sphere", "stairs"];

    /// A flight's steps, bottom first, centered on the flight's bounding
    /// box like every shape. Empty for other shapes.
    pub fn stair_steps(&self) -> Vec<Step> {
        let raw = self.raw_steps();
        let (min, max) = bounds_of(&raw);
        let center = (min + max) * 0.5;
        raw.into_iter()
            .map(|s| Step {
                corners: s.corners.map(|c| c - center),
                ..s
            })
            .collect()
    }

    /// Steps with the bottom front of the walking line at the origin.
    fn raw_steps(&self) -> Vec<Step> {
        let Shape::Stairs {
            width,
            height,
            steps,
            depth,
            turn,
            inner_radius,
        } = *self
        else {
            return Vec::new();
        };
        let n = steps.clamp(1, 512);
        let width = width.max(0.01);
        let rise = height.max(0.0) / n as f32;
        let inner = inner_radius.max(0.0);
        let angle = turn.to_radians();
        // Step boundary `k` (0 = the bottom front), on the inside (`t` 0)
        // or outside (`t` 1) edge.
        let corner = |t: f32, k: u32| -> Vec3 {
            if angle.abs() < 1e-4 {
                let x = -width * 0.5 + t * width;
                return Vec3::new(x, 0.0, -(k as f32) * depth.max(0.01));
            }
            // Around a center on the left (x < 0); a right turn mirrors it.
            let phi = angle.abs() * k as f32 / n as f32;
            let r = inner + t * width;
            let center = -(inner + width * 0.5);
            let p = Vec3::new(center + r * phi.cos(), 0.0, -r * phi.sin());
            if angle < 0.0 {
                p * Vec3::new(-1.0, 1.0, 1.0)
            } else {
                p
            }
        };
        // Steps per full turn: a step that many above another is over it.
        let per_turn = if angle.abs() > TAU + 1e-4 {
            (n as f32 * TAU / angle.abs()).floor() as u32
        } else {
            u32::MAX
        };
        (0..n)
            .map(|i| {
                let top = rise * (i + 1) as f32;
                Step {
                    corners: [
                        corner(0.0, i),
                        corner(1.0, i),
                        corner(0.0, i + 1),
                        corner(1.0, i + 1),
                    ],
                    bottom: if i >= per_turn { top - 2.0 * rise } else { 0.0 },
                    top,
                }
            })
            .collect()
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Box { .. } => "box",
            Shape::Plane { .. } => "plane",
            Shape::Wedge { .. } => "wedge",
            Shape::Cylinder { .. } => "cylinder",
            Shape::Sphere { .. } => "sphere",
            Shape::Stairs { .. } => "stairs",
        }
    }

    /// This shape as another kind (one of [`Shape::KINDS`]) that fills about
    /// the same bounding box. Segment counts carry over between round shapes.
    pub fn with_kind(&self, kind: &str) -> Shape {
        let mut size = self.half_extents() * 2.0;
        if size.y < 0.01 {
            // From a plane: give it some height.
            size.y = 1.0;
        }
        let segments = match *self {
            Shape::Cylinder { segments, .. } | Shape::Sphere { segments, .. } => segments,
            _ => 12,
        };
        match kind {
            "plane" => Shape::Plane { size: size.xz() },
            "wedge" => Shape::Wedge { size },
            "cylinder" => Shape::Cylinder {
                radius: size.x.max(size.z) * 0.5,
                height: size.y,
                segments,
            },
            "sphere" => Shape::Sphere {
                radius: size.max_element() * 0.5,
                segments,
                rings: (segments * 2 / 3).max(2),
            },
            "stairs" => {
                // Steps about 18 cm high, filling the box.
                let steps = (size.y / 0.18).round().max(1.0) as u32;
                Shape::Stairs {
                    width: size.x,
                    height: size.y,
                    steps,
                    depth: size.z / steps as f32,
                    turn: 0.0,
                    inner_radius: 0.0,
                }
            }
            _ => Shape::Box { size },
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Smallest and largest corner of the axis-aligned bounding box.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        self.vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(min, max), v| {
                let p = Vec3::from(v.position);
                (min.min(p), max.max(p))
            },
        )
    }

    pub fn from_shape(shape: Shape) -> Self {
        let mut mesh = Self::default();
        match shape {
            Shape::Box { size } => mesh.add_box(size * 0.5),
            Shape::Plane { size } => mesh.add_quad(
                Vec3::ZERO,
                Vec3::X * size.x * 0.5,
                Vec3::NEG_Z * size.y * 0.5,
            ),
            Shape::Wedge { size } => mesh.add_wedge(size * 0.5),
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => mesh.add_cylinder(radius, height * 0.5, segments.max(3)),
            Shape::Sphere {
                radius,
                segments,
                rings,
            } => mesh.add_sphere(radius, segments.max(3), rings.max(2)),
            Shape::Stairs { .. } => mesh.add_stairs(&shape.stair_steps()),
        }
        mesh
    }

    /// A solid flight: per step its tread, riser, underside and sides, and
    /// the back of the last one. Faces between steps are left out, except
    /// where a step stands higher off the floor than the one before (a
    /// spiral's slabs): there the lower one's back shows.
    fn add_stairs(&mut self, steps: &[Step]) {
        let at = |p: Vec3, y: f32| p + Vec3::Y * y;
        for (i, step) in steps.iter().enumerate() {
            let [l0, r0, l1, r1] = step.corners;
            let (bottom, top) = (step.bottom, step.top);
            let front = (l0 + r0 - l1 - r1) * 0.5;
            let out = (r0 + r1 - l0 - l1) * 0.5;
            self.add_face(
                &[at(l0, top), at(r0, top), at(r1, top), at(l1, top)],
                Vec3::Y,
            );
            self.add_face(
                &[
                    at(l0, bottom),
                    at(r0, bottom),
                    at(r1, bottom),
                    at(l1, bottom),
                ],
                Vec3::NEG_Y,
            );
            let riser = match i {
                0 => bottom,
                _ => steps[i - 1].top.max(bottom),
            };
            self.add_face(
                &[at(l0, riser), at(r0, riser), at(r0, top), at(l0, top)],
                front,
            );
            self.add_face(
                &[at(r0, bottom), at(r1, bottom), at(r1, top), at(r0, top)],
                out,
            );
            self.add_face(
                &[at(l0, bottom), at(l1, bottom), at(l1, top), at(l0, top)],
                -out,
            );
            if let Some(next) = steps.get(i + 1) {
                // Under the next step, if it starts higher than this one.
                let up_to = next.bottom.min(top);
                if up_to > bottom + 1e-5 {
                    self.add_face(
                        &[at(l1, bottom), at(r1, bottom), at(r1, up_to), at(l1, up_to)],
                        -front,
                    );
                }
            } else {
                self.add_face(
                    &[at(l1, bottom), at(r1, bottom), at(r1, top), at(l1, top)],
                    -front,
                );
            }
        }
    }

    /// A flat convex face through `corners` (in order around it), facing
    /// the side `outward` points to. Corners that coincide are merged (a
    /// step around a central post is a triangle); nothing is added if fewer
    /// than three are left. UVs are in meters along the face.
    fn add_face(&mut self, corners: &[Vec3], outward: Vec3) {
        let mut points: Vec<Vec3> = Vec::new();
        for &c in corners {
            let new = points.last().is_none_or(|p| p.distance(c) > 1e-5)
                && points.first().is_none_or(|p| p.distance(c) > 1e-5);
            if new {
                points.push(c);
            }
        }
        if points.len() < 3 {
            return;
        }
        let mut normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize_or_zero();
        if normal == Vec3::ZERO {
            return;
        }
        if normal.dot(outward) < 0.0 {
            points.reverse();
            normal = -normal;
        }
        let u = (points[1] - points[0]).normalize_or(Vec3::X);
        let v = normal.cross(u);
        let base = self.vertices.len() as u32;
        for p in &points {
            let d = *p - points[0];
            self.vertices.push(Vertex {
                position: (*p).into(),
                normal: normal.into(),
                uv: [d.dot(u), d.dot(v)],
                color: WHITE,
            });
        }
        for k in 1..points.len() as u32 - 1 {
            self.indices.extend([base, base + k, base + k + 1]);
        }
    }

    /// Quad spanning `center ± u ± v`; it faces `u × v`.
    fn add_quad(&mut self, center: Vec3, u: Vec3, v: Vec3) {
        let normal = u.cross(v).normalize();
        let (len_u, len_v) = (u.length(), v.length());
        let base = self.vertices.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            self.vertices.push(Vertex {
                position: (center + u * a + v * b).into(),
                normal: normal.into(),
                uv: [(a + 1.0) * len_u, (1.0 - b) * len_v],
                color: WHITE,
            });
        }
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Triangle with counter-clockwise winding; UVs given in meters.
    fn add_triangle(&mut self, corners: [Vec3; 3], uvs: [Vec2; 3]) {
        let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        if normal.length_squared() < 1e-12 {
            return; // degenerate, e.g. at a sphere pole
        }
        let normal = normal.normalize();
        let base = self.vertices.len() as u32;
        for (p, uv) in corners.into_iter().zip(uvs) {
            self.vertices.push(Vertex {
                position: p.into(),
                normal: normal.into(),
                uv: uv.into(),
                color: WHITE,
            });
        }
        self.indices.extend([base, base + 1, base + 2]);
    }

    fn add_box(&mut self, h: Vec3) {
        let (x, y, z) = (Vec3::X * h.x, Vec3::Y * h.y, Vec3::Z * h.z);
        self.add_quad(x, -z, y);
        self.add_quad(-x, z, y);
        self.add_quad(y, x, -z);
        self.add_quad(-y, x, z);
        self.add_quad(z, x, y);
        self.add_quad(-z, -x, y);
    }

    fn add_wedge(&mut self, h: Vec3) {
        let (x, y, z) = (Vec3::X * h.x, Vec3::Y * h.y, Vec3::Z * h.z);
        self.add_quad(-y, x, z); // bottom
        self.add_quad(-z, -x, y); // tall back face
        self.add_quad(Vec3::ZERO, x, y - z); // slope

        // Side UVs: meters along Z and up from the bottom.
        let side_uv = |p: Vec3| Vec2::new(p.z + h.z, h.y - p.y);
        let (back_low, front_low, back_high) = (-y - z, -y + z, y - z);
        for side in [
            [x + front_low, x + back_low, x + back_high],
            [-x + back_low, -x + front_low, -x + back_high],
        ] {
            self.add_triangle(side, side.map(side_uv));
        }
    }

    fn add_cylinder(&mut self, radius: f32, half_height: f32, segments: u32) {
        let point = |i: u32| {
            let angle = i as f32 / segments as f32 * TAU;
            Vec3::new(angle.cos() * radius, 0.0, -angle.sin() * radius)
        };
        let (top, bottom) = (Vec3::Y * half_height, -Vec3::Y * half_height);
        let arc = TAU * radius / segments as f32;
        let height = 2.0 * half_height;
        for i in 0..segments {
            let (a, b) = (point(i), point(i + 1));
            let (ua, ub) = (i as f32 * arc, (i + 1) as f32 * arc);
            self.add_triangle(
                [a + bottom, b + bottom, b + top],
                [
                    Vec2::new(ua, height),
                    Vec2::new(ub, height),
                    Vec2::new(ub, 0.0),
                ],
            );
            self.add_triangle(
                [a + bottom, b + top, a + top],
                [
                    Vec2::new(ua, height),
                    Vec2::new(ub, 0.0),
                    Vec2::new(ua, 0.0),
                ],
            );
            self.add_triangle([top, a + top, b + top], [top.xz(), a.xz(), b.xz()]);
            self.add_triangle(
                [bottom, b + bottom, a + bottom],
                [bottom.xz(), b.xz(), a.xz()],
            );
        }
    }

    fn add_sphere(&mut self, radius: f32, segments: u32, rings: u32) {
        let point = |seg: u32, ring: u32| {
            let lon = seg as f32 / segments as f32 * TAU;
            let lat = ring as f32 / rings as f32 * PI - PI * 0.5;
            Vec3::new(lat.cos() * lon.cos(), lat.sin(), -lat.cos() * lon.sin()) * radius
        };
        let uv = |seg: u32, ring: u32| {
            Vec2::new(
                seg as f32 / segments as f32 * TAU * radius,
                (1.0 - ring as f32 / rings as f32) * PI * radius,
            )
        };
        for ring in 0..rings {
            for seg in 0..segments {
                let quad = [
                    (seg, ring),
                    (seg + 1, ring),
                    (seg + 1, ring + 1),
                    (seg, ring + 1),
                ];
                let p = quad.map(|(s, r)| point(s, r));
                let t = quad.map(|(s, r)| uv(s, r));
                self.add_triangle([p[0], p[1], p[2]], [t[0], t[1], t[2]]);
                self.add_triangle([p[0], p[2], p[3]], [t[0], t[2], t[3]]);
            }
        }
    }
}

/// The box around steps (their footprints, floor to top).
fn bounds_of(steps: &[Step]) -> (Vec3, Vec3) {
    if steps.is_empty() {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let points = steps.iter().flat_map(Step::points);
    let min = points.clone().fold(Vec3::splat(f32::MAX), Vec3::min);
    let max = points.fold(Vec3::splat(f32::MIN), Vec3::max);
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stairs(steps: u32, turn: f32, inner_radius: f32) -> Shape {
        Shape::Stairs {
            width: 1.0,
            height: 2.0,
            steps,
            depth: 0.3,
            turn,
            inner_radius,
        }
    }

    fn triangles(mesh: &MeshData) -> usize {
        mesh.indices.len() / 3
    }

    /// Per step a tread, a bottom, a riser and two sides (quads), and the
    /// back of the last step.
    #[test]
    fn a_straight_flight_is_closed_quads() {
        let mesh = MeshData::from_shape(stairs(10, 0.0, 0.0));
        assert_eq!(mesh.vertices.len(), 10 * 5 * 4 + 4);
        assert_eq!(triangles(&mesh), 10 * 5 * 2 + 2);
        let half = stairs(10, 0.0, 0.0).half_extents();
        assert!((half - Vec3::new(0.5, 1.0, 1.5)).length() < 1e-5, "{half}");
        // Centered on its box, like every shape.
        let (min, max) = mesh.bounds();
        assert!((min + half).length() < 1e-5 && (max - half).length() < 1e-5);
        // Treads face up, bottoms down, nothing faces sideways wrongly:
        // every face's normal points away from the flight's middle line.
        let up = mesh.vertices.iter().filter(|v| v.normal[1] > 0.99).count();
        let down = mesh.vertices.iter().filter(|v| v.normal[1] < -0.99).count();
        assert_eq!((up, down), (40, 40));
    }

    /// Around a central post the inside edge is a point: treads and
    /// bottoms are triangles and there is no inside wall.
    #[test]
    fn a_spiral_around_a_post_ends_at_its_height_and_angle() {
        let shape = stairs(16, 360.0, 0.0);
        let mesh = MeshData::from_shape(shape);
        assert_eq!(mesh.vertices.len(), 16 * (3 + 3 + 4 + 4) + 4);
        assert_eq!(triangles(&mesh), 16 * (1 + 1 + 2 + 2) + 2);
        let steps = shape.stair_steps();
        assert_eq!(steps.len(), 16);
        let last = steps.last().unwrap();
        assert!((last.top - 2.0).abs() < 1e-5);
        // A full turn: the last step's back edge lies along the first
        // step's front edge, from the post outward.
        let first = steps[0];
        let front = (first.corners[1] - first.corners[0]).normalize();
        let back = (last.corners[3] - last.corners[2]).normalize();
        assert!(front.dot(back) > 0.999, "{front} {back}");
        assert!(first.corners[0].distance(last.corners[2]) < 1e-4);
        // A quarter turn's back edge is at right angles to its front.
        let quarter = stairs(6, 90.0, 0.5).stair_steps();
        let front = (quarter[0].corners[1] - quarter[0].corners[0]).normalize();
        let back = (quarter[5].corners[3] - quarter[5].corners[2]).normalize();
        assert!(front.dot(back).abs() < 1e-4, "{front} {back}");
    }

    /// Past a full turn, the steps above others are slabs with headroom
    /// under them; up to a full turn every step stands on the floor.
    #[test]
    fn a_spiral_past_a_full_turn_leaves_headroom() {
        let one = stairs(16, 360.0, 0.0).stair_steps();
        assert!(one.iter().all(|s| s.bottom == 0.0));
        let shape = Shape::Stairs {
            width: 1.0,
            height: 6.0,
            steps: 30,
            depth: 0.3,
            turn: 720.0,
            inner_radius: 0.3,
        };
        let steps = shape.stair_steps();
        let rise = 0.2;
        for (i, s) in steps.iter().enumerate() {
            if i >= 15 {
                assert!((s.top - s.bottom - 2.0 * rise).abs() < 1e-4);
                assert!(s.bottom - steps[i - 15].top > 2.5, "{i}: no headroom");
            } else {
                assert_eq!(s.bottom, 0.0);
            }
        }
        // Still closed: every face faces out (see below), bounds hold.
        let half = shape.half_extents();
        let (min, max) = MeshData::from_shape(shape).bounds();
        assert!((min + half).length() < 1e-4 && (max - half).length() < 1e-4);
    }

    /// The box of a curved flight covers every step, not just the straight
    /// footprint: the gizmo and FRAME use it.
    #[test]
    fn a_curved_flight_has_the_box_of_its_steps() {
        for (turn, inner) in [(90.0, 0.5), (-90.0, 0.5), (270.0, 0.0), (720.0, 0.3)] {
            let shape = stairs(12, turn, inner);
            let half = shape.half_extents();
            let (min, max) = MeshData::from_shape(shape).bounds();
            assert!((min + half).length() < 1e-4, "{turn}: {min} {half}");
            assert!((max - half).length() < 1e-4, "{turn}: {max} {half}");
        }
        // A spiral around a post is as wide as twice its width.
        let half = stairs(16, 360.0, 0.0).half_extents();
        assert!(
            (half.x - 1.0).abs() < 0.02 && (half.z - 1.0).abs() < 0.02,
            "{half}"
        );
    }

    /// Every face faces out of the flight: its normal points away from a
    /// point just inside the step it belongs to.
    #[test]
    fn stair_faces_face_outward() {
        let double = Shape::Stairs {
            width: 1.0,
            height: 6.0,
            steps: 30,
            depth: 0.3,
            turn: 720.0,
            inner_radius: 0.3,
        };
        for shape in [
            stairs(5, 0.0, 0.0),
            stairs(8, 120.0, 0.4),
            stairs(8, -360.0, 0.0),
            double,
        ] {
            let mesh = MeshData::from_shape(shape);
            let steps = shape.stair_steps();
            for t in mesh.indices.chunks_exact(3) {
                let p: Vec<Vec3> = t
                    .iter()
                    .map(|&i| Vec3::from(mesh.vertices[i as usize].position))
                    .collect();
                let centroid = (p[0] + p[1] + p[2]) / 3.0;
                let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
                // The step whose solid this face is on.
                let inside = steps.iter().any(|s| {
                    let mid =
                        s.corners.iter().sum::<Vec3>() / 4.0 + Vec3::Y * (s.top + s.bottom) * 0.5;
                    (centroid - mid).dot(n) > 0.0 && centroid.distance(mid) < 3.0
                });
                assert!(inside, "{shape:?}: face at {centroid} faces {n}");
            }
        }
    }

    #[test]
    fn changing_kind_keeps_the_size() {
        let wide = Shape::Box {
            size: Vec3::new(2.0, 3.0, 1.0),
        };
        assert_eq!(
            wide.with_kind("cylinder"),
            Shape::Cylinder {
                radius: 1.0,
                height: 3.0,
                segments: 12
            }
        );
        assert_eq!(wide.with_kind("cylinder").with_kind("box").kind(), "box");
        let plane = Shape::Plane {
            size: Vec2::new(4.0, 2.0),
        };
        assert_eq!(
            plane.with_kind("box"),
            Shape::Box {
                size: Vec3::new(4.0, 1.0, 2.0)
            }
        );
        assert_eq!(plane.with_kind("box").with_kind("plane"), plane);
        for kind in Shape::KINDS {
            assert_eq!(wide.with_kind(kind).kind(), kind);
        }
    }

    fn all_shapes() -> Vec<Shape> {
        vec![
            Shape::Box {
                size: Vec3::new(1.0, 2.0, 3.0),
            },
            Shape::Plane {
                size: Vec2::new(4.0, 2.0),
            },
            Shape::Wedge {
                size: Vec3::new(2.0, 1.0, 4.0),
            },
            Shape::Cylinder {
                radius: 0.5,
                height: 2.0,
                segments: 6,
            },
            Shape::Sphere {
                radius: 1.0,
                segments: 8,
                rings: 5,
            },
        ]
    }

    #[test]
    fn indices_are_in_range() {
        for shape in all_shapes() {
            let mesh = MeshData::from_shape(shape);
            assert!(!mesh.indices.is_empty(), "{shape:?}");
            assert_eq!(mesh.indices.len() % 3, 0);
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.vertices.len())
            );
        }
    }

    /// Winding must agree with the stored normal (so back-face culling keeps
    /// the right side), and on these convex shapes every face must point away
    /// from the center.
    #[test]
    fn faces_point_outward() {
        for shape in all_shapes() {
            let mesh = MeshData::from_shape(shape);
            for tri in mesh.indices.chunks(3) {
                let [a, b, c] =
                    [0, 1, 2].map(|i| Vec3::from(mesh.vertices[tri[i] as usize].position));
                let geometric = (b - a).cross(c - a).normalize();
                let stored = Vec3::from(mesh.vertices[tri[0] as usize].normal);
                assert!(geometric.dot(stored) > 0.99, "{shape:?}: winding vs normal");
                if !matches!(shape, Shape::Plane { .. }) {
                    let center = (a + b + c) / 3.0;
                    // The wedge is not centered on its centroid; use a point
                    // known to be inside it (its centroid is at -size/6 in y and z).
                    let inside = if let Shape::Wedge { size } = shape {
                        Vec3::new(0.0, -size.y / 6.0, -size.z / 6.0)
                    } else {
                        Vec3::ZERO
                    };
                    assert!(
                        (center - inside).dot(stored) > 0.0,
                        "{shape:?}: faces inward"
                    );
                }
            }
        }
    }

    #[test]
    fn plane_faces_up() {
        let mesh = MeshData::from_shape(Shape::Plane {
            size: Vec2::splat(2.0),
        });
        assert!(mesh.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]));
    }
}

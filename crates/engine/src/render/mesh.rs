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
}

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
}

impl Shape {
    /// Half the size of the shape's bounding box.
    pub fn half_extents(&self) -> Vec3 {
        match *self {
            Shape::Box { size } | Shape::Wedge { size } => size * 0.5,
            Shape::Plane { size } => Vec3::new(size.x, 0.0, size.y) * 0.5,
            Shape::Cylinder { radius, height, .. } => Vec3::new(radius, height * 0.5, radius),
            Shape::Sphere { radius, .. } => Vec3::splat(radius),
        }
    }

    pub const KINDS: [&'static str; 5] = ["box", "plane", "wedge", "cylinder", "sphere"];

    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Box { .. } => "box",
            Shape::Plane { .. } => "plane",
            Shape::Wedge { .. } => "wedge",
            Shape::Cylinder { .. } => "cylinder",
            Shape::Sphere { .. } => "sphere",
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
        }
        mesh
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

#[cfg(test)]
mod tests {
    use super::*;

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

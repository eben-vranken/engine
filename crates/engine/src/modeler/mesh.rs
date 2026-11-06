//! The modeler's editable mesh: shared corner positions and polygon faces
//! (quads mostly), the way a modeler thinks about a mesh. It's turned into
//! flat-shaded triangles to draw or save.

use crate::assets::{ModelData, Part};
use crate::render::mesh::{MeshData, Vertex};
use crate::render::texture::Image;
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EditMesh {
    pub positions: Vec<Vec3>,
    pub faces: Vec<Face>,
}

/// A polygon: indices into `positions`, counter-clockwise seen from the
/// side it faces. Assumed roughly flat and convex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub verts: Vec<u32>,
}

/// An edge between two corners, smaller index first.
pub type Edge = [u32; 2];

pub fn edge(a: u32, b: u32) -> Edge {
    if a < b { [a, b] } else { [b, a] }
}

/// The color models from the modeler have until they get their own.
const CLAY: [u8; 4] = [204, 204, 204, 255];

impl EditMesh {
    /// A box centered on the origin; `size` is its full extents.
    pub fn cube(size: Vec3) -> Self {
        let h = size / 2.0;
        let positions = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { -h.x } else { h.x },
                    if i & 2 == 0 { -h.y } else { h.y },
                    if i & 4 == 0 { -h.z } else { h.z },
                )
            })
            .collect();
        let faces = [
            [0, 4, 6, 2], // -X
            [1, 3, 7, 5], // +X
            [0, 1, 5, 4], // -Y
            [2, 6, 7, 3], // +Y
            [0, 2, 3, 1], // -Z
            [4, 5, 7, 6], // +Z
        ];
        Self {
            positions,
            faces: faces.iter().map(|f| Face { verts: f.to_vec() }).collect(),
        }
    }

    /// A horizontal quad facing up.
    pub fn plane(size: Vec2) -> Self {
        let (x, z) = (size.x / 2.0, size.y / 2.0);
        Self {
            positions: vec![
                Vec3::new(-x, 0.0, -z),
                Vec3::new(-x, 0.0, z),
                Vec3::new(x, 0.0, z),
                Vec3::new(x, 0.0, -z),
            ],
            faces: vec![Face {
                verts: vec![0, 1, 2, 3],
            }],
        }
    }

    /// An upright cylinder with `segments` sides, standing on the origin's
    /// level centered on it.
    pub fn cylinder(radius: f32, height: f32, segments: u32) -> Self {
        let n = segments.max(3);
        let h = height / 2.0;
        let mut positions = Vec::new();
        for y in [-h, h] {
            for i in 0..n {
                let a = i as f32 / n as f32 * TAU;
                positions.push(Vec3::new(a.cos() * radius, y, -a.sin() * radius));
            }
        }
        let mut faces: Vec<Face> = (0..n)
            .map(|i| {
                let j = (i + 1) % n;
                Face {
                    verts: vec![i, j, n + j, n + i],
                }
            })
            .collect();
        faces.push(Face {
            verts: (0..n).rev().collect(),
        });
        faces.push(Face {
            verts: (n..2 * n).collect(),
        });
        Self { positions, faces }
    }

    /// A UV sphere: `segments` around, `rings` from pole to pole.
    pub fn sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let (n, rings) = (segments.max(3), rings.max(2));
        let mut positions = vec![Vec3::new(0.0, -radius, 0.0)];
        for r in 1..rings {
            let polar = PI - r as f32 / rings as f32 * PI;
            let (y, ring) = (polar.cos() * radius, polar.sin() * radius);
            for i in 0..n {
                let a = i as f32 / n as f32 * TAU;
                positions.push(Vec3::new(a.cos() * ring, y, -a.sin() * ring));
            }
        }
        positions.push(Vec3::new(0.0, radius, 0.0));
        let top = positions.len() as u32 - 1;
        let at = |ring: u32, i: u32| 1 + ring * n + i % n;
        let mut faces = Vec::new();
        for i in 0..n {
            faces.push(Face {
                verts: vec![0, at(0, i + 1), at(0, i)],
            });
        }
        for ring in 0..rings - 2 {
            for i in 0..n {
                faces.push(Face {
                    verts: vec![
                        at(ring, i),
                        at(ring, i + 1),
                        at(ring + 1, i + 1),
                        at(ring + 1, i),
                    ],
                });
            }
        }
        for i in 0..n {
            faces.push(Face {
                verts: vec![at(rings - 2, i), at(rings - 2, i + 1), top],
            });
        }
        Self { positions, faces }
    }

    /// The face's corner positions.
    pub fn corners(&self, face: usize) -> impl Iterator<Item = Vec3> + '_ {
        self.faces[face]
            .verts
            .iter()
            .map(|&v| self.positions[v as usize])
    }

    /// The face's normal (Newell's method, which works for any polygon).
    pub fn face_normal(&self, face: usize) -> Vec3 {
        let corners: Vec<Vec3> = self.corners(face).collect();
        let mut normal = Vec3::ZERO;
        for (i, a) in corners.iter().enumerate() {
            let b = corners[(i + 1) % corners.len()];
            normal += Vec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        normal.normalize_or_zero()
    }

    pub fn face_center(&self, face: usize) -> Vec3 {
        let n = self.faces[face].verts.len().max(1) as f32;
        self.corners(face).sum::<Vec3>() / n
    }

    /// Every edge once, in a stable order.
    pub fn edges(&self) -> Vec<Edge> {
        let mut edges: Vec<Edge> = self
            .faces
            .iter()
            .flat_map(|f| {
                let n = f.verts.len();
                (0..n).map(move |i| edge(f.verts[i], f.verts[(i + 1) % n]))
            })
            .collect();
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// The face's triangles as corner positions (a fan; faces are convex).
    fn triangles(&self, face: usize) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        let verts = &self.faces[face].verts;
        let p = |i: usize| self.positions[verts[i] as usize];
        (1..verts.len().saturating_sub(1)).map(move |i| [p(0), p(i), p(i + 1)])
    }

    /// The nearest face the ray hits, and how far along the ray.
    pub fn pick_face(&self, origin: Vec3, direction: Vec3) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32)> = None;
        for face in 0..self.faces.len() {
            for [a, b, c] in self.triangles(face) {
                if let Some(t) = ray_triangle(origin, direction, a, b, c)
                    && best.is_none_or(|(_, d)| t < d)
                {
                    best = Some((face, t));
                }
            }
        }
        best
    }

    /// Flat-shaded triangles to draw or save: every face has its own
    /// corners and normal. UVs are in meters, projected along the face's
    /// main axis, like the engine's primitives.
    pub fn to_model(&self) -> ModelData {
        let mut mesh = MeshData::default();
        for face in 0..self.faces.len() {
            let normal = self.face_normal(face);
            for [a, b, c] in self.triangles(face) {
                let base = mesh.vertices.len() as u32;
                for p in [a, b, c] {
                    mesh.vertices.push(Vertex {
                        position: p.into(),
                        normal: normal.into(),
                        uv: box_uv(p, normal).into(),
                    });
                }
                mesh.indices.extend([base, base + 1, base + 2]);
            }
        }
        let count = mesh.indices.len() as u32;
        ModelData {
            mesh,
            parts: vec![Part {
                indices: 0..count,
                image: 0,
            }],
            images: vec![Image::new(1, 1, CLAY.to_vec())],
            ..Default::default()
        }
    }
}

/// Planar UVs along the normal's dominant axis, in meters.
fn box_uv(p: Vec3, normal: Vec3) -> Vec2 {
    let a = normal.abs();
    if a.x >= a.y && a.x >= a.z {
        Vec2::new(p.z, -p.y)
    } else if a.y >= a.z {
        Vec2::new(p.x, p.z)
    } else {
        Vec2::new(p.x, -p.y)
    }
}

/// Möller–Trumbore; hits from either side.
fn ray_triangle(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-5).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every face's normal points away from the center, and every edge is
    /// shared by exactly two faces (the surface is closed).
    fn assert_closed_and_outward(mesh: &EditMesh) {
        for face in 0..mesh.faces.len() {
            let out = mesh.face_center(face).normalize_or_zero();
            assert!(
                mesh.face_normal(face).dot(out) > 0.0,
                "face {face} faces inward"
            );
        }
        for e in mesh.edges() {
            let uses = mesh
                .faces
                .iter()
                .filter(|f| {
                    let n = f.verts.len();
                    (0..n).any(|i| edge(f.verts[i], f.verts[(i + 1) % n]) == e)
                })
                .count();
            assert_eq!(uses, 2, "edge {e:?}");
        }
    }

    #[test]
    fn primitives_are_closed_and_face_outward() {
        let cube = EditMesh::cube(Vec3::ONE);
        assert_eq!((cube.positions.len(), cube.faces.len()), (8, 6));
        assert_eq!(cube.edges().len(), 12);
        assert_closed_and_outward(&cube);
        assert_closed_and_outward(&EditMesh::cylinder(0.5, 1.0, 8));
        assert_closed_and_outward(&EditMesh::sphere(0.5, 8, 6));
        let plane = EditMesh::plane(Vec2::ONE);
        assert!(plane.face_normal(0).abs_diff_eq(Vec3::Y, 1e-6));
    }

    #[test]
    fn picking_hits_the_nearest_face() {
        let cube = EditMesh::cube(Vec3::splat(2.0));
        let (face, t) = cube
            .pick_face(Vec3::new(0.2, 0.3, 5.0), Vec3::NEG_Z)
            .unwrap();
        assert!(cube.face_normal(face).abs_diff_eq(Vec3::Z, 1e-6));
        assert!((t - 4.0).abs() < 1e-5);
        assert!(
            cube.pick_face(Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z)
                .is_none()
        );
    }

    #[test]
    fn triangles_cover_every_face() {
        let model = EditMesh::cylinder(0.5, 1.0, 6).to_model();
        // 6 quads (2 triangles each) and two hexagons (4 each).
        assert_eq!(model.mesh.indices.len(), (6 * 2 + 2 * 4) * 3);
    }
}

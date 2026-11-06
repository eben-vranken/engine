//! The modeler's editable mesh: shared corner positions and polygon faces
//! (quads mostly), the way a modeler thinks about a mesh. It's turned into
//! triangles to draw or save.

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
    /// The model's one texture, relative to the project (`None` = plain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// Names of the model's parts (`Face::part` indexes this); scenes give
    /// parts settings of their own by name. Empty = one part.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
}

/// What the only part of a model is called until it's renamed.
pub const FIRST_PART: &str = "body";

/// A polygon: indices into `positions`, counter-clockwise seen from the
/// side it faces. Assumed roughly flat and convex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub verts: Vec<u32>,
    /// Texture coordinates per corner (1 = the whole texture); empty =
    /// projected automatically, one texture per meter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uvs: Vec<Vec2>,
    /// Linear RGB per corner, multiplied into the texture; empty =
    /// unpainted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<Vec3>,
    /// Smooth shading: normals blend with neighbouring smooth faces.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub smooth: bool,
    /// Which of the mesh's parts the face belongs to.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub part: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Face {
    pub fn new(verts: Vec<u32>) -> Self {
        Self {
            verts,
            uvs: Vec::new(),
            colors: Vec::new(),
            smooth: false,
            part: 0,
        }
    }

    /// A new face made from this one keeps its shading and part.
    pub fn like(&self, verts: Vec<u32>) -> Self {
        Self {
            smooth: self.smooth,
            part: self.part,
            ..Self::new(verts)
        }
    }

    /// Reverses the winding (and so the side it faces).
    pub fn reverse(&mut self) {
        self.verts.reverse();
        self.uvs.reverse();
        self.colors.reverse();
    }
}

/// An edge between two corners, smaller index first.
pub type Edge = [u32; 2];

pub fn edge(a: u32, b: u32) -> Edge {
    if a < b { [a, b] } else { [b, a] }
}

/// The color of unpainted corners on a model without a texture: clay.
pub const CLAY: Vec3 = Vec3::splat(0.6);

impl EditMesh {
    fn from_faces(positions: Vec<Vec3>, faces: Vec<Vec<u32>>) -> Self {
        Self {
            positions,
            faces: faces.into_iter().map(Face::new).collect(),
            texture: None,
            parts: Vec::new(),
        }
    }

    /// The parts' names; a mesh without any has one.
    pub fn part_names(&self) -> Vec<String> {
        if self.parts.is_empty() {
            vec![FIRST_PART.to_owned()]
        } else {
            self.parts.clone()
        }
    }

    /// Which part `face` is in (faces pointing past the list go to the last).
    pub fn part_of(&self, face: usize) -> u32 {
        let last = self.parts.len().saturating_sub(1) as u32;
        self.faces[face].part.min(last)
    }

    /// Adds `other` as a new part named `name` (its faces join it).
    /// Returns the new part's index.
    pub fn add_part(&mut self, name: String, other: EditMesh) -> u32 {
        if self.parts.is_empty() {
            self.parts.push(FIRST_PART.to_owned());
        }
        let part = self.parts.len() as u32;
        self.parts.push(name);
        let base = self.positions.len() as u32;
        self.positions.extend(other.positions);
        for mut face in other.faces {
            face.verts.iter_mut().for_each(|v| *v += base);
            face.part = part;
            self.faces.push(face);
        }
        part
    }

    /// A name for a new part: `base`, or `base 2`, `base 3`… if taken.
    pub fn free_part_name(&self, base: &str) -> String {
        let names = self.part_names();
        std::iter::once(base.to_owned())
            .chain((2..).map(|n| format!("{base} {n}")))
            .find(|name| !names.contains(name))
            .expect("some number is free")
    }

    /// Removes part `part` and its faces (not the last part). Faces of
    /// later parts move down one.
    pub fn remove_part(&mut self, part: u32) -> bool {
        if self.parts.len() < 2 || part as usize >= self.parts.len() {
            return false;
        }
        self.faces.retain(|f| f.part != part);
        for face in &mut self.faces {
            if face.part > part {
                face.part -= 1;
            }
        }
        self.parts.remove(part as usize);
        self.remove_unused_positions();
        true
    }

    /// Drops positions no face uses, renumbering the faces' corners.
    pub fn remove_unused_positions(&mut self) {
        let mut used = vec![false; self.positions.len()];
        for face in &self.faces {
            for &v in &face.verts {
                used[v as usize] = true;
            }
        }
        let mut new_index = vec![0u32; self.positions.len()];
        let mut kept = Vec::new();
        for (i, p) in self.positions.iter().enumerate() {
            if used[i] {
                new_index[i] = kept.len() as u32;
                kept.push(*p);
            }
        }
        self.positions = kept;
        for face in &mut self.faces {
            face.verts
                .iter_mut()
                .for_each(|v| *v = new_index[*v as usize]);
        }
    }

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
        let faces = vec![
            vec![0, 4, 6, 2], // -X
            vec![1, 3, 7, 5], // +X
            vec![0, 1, 5, 4], // -Y
            vec![2, 6, 7, 3], // +Y
            vec![0, 2, 3, 1], // -Z
            vec![4, 5, 7, 6], // +Z
        ];
        Self::from_faces(positions, faces)
    }

    /// A horizontal quad facing up.
    pub fn plane(size: Vec2) -> Self {
        let (x, z) = (size.x / 2.0, size.y / 2.0);
        let positions = vec![
            Vec3::new(-x, 0.0, -z),
            Vec3::new(-x, 0.0, z),
            Vec3::new(x, 0.0, z),
            Vec3::new(x, 0.0, -z),
        ];
        Self::from_faces(positions, vec![vec![0, 1, 2, 3]])
    }

    /// An upright cylinder with `segments` sides, centered on the origin.
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
        let mut faces: Vec<Vec<u32>> = (0..n)
            .map(|i| {
                let j = (i + 1) % n;
                vec![i, j, n + j, n + i]
            })
            .collect();
        faces.push((0..n).rev().collect());
        faces.push((n..2 * n).collect());
        Self::from_faces(positions, faces)
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
            faces.push(vec![0, at(0, i + 1), at(0, i)]);
        }
        for ring in 0..rings - 2 {
            for i in 0..n {
                faces.push(vec![
                    at(ring, i),
                    at(ring, i + 1),
                    at(ring + 1, i + 1),
                    at(ring + 1, i),
                ]);
            }
        }
        for i in 0..n {
            faces.push(vec![at(rings - 2, i), at(rings - 2, i + 1), top]);
        }
        Self::from_faces(positions, faces)
    }

    /// The face's corner positions.
    pub fn corners(&self, face: usize) -> impl Iterator<Item = Vec3> + '_ {
        self.faces[face]
            .verts
            .iter()
            .map(|&v| self.positions[v as usize])
    }

    /// The face's normal times twice its area (Newell's method, which works
    /// for any polygon).
    fn area_normal(&self, face: usize) -> Vec3 {
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
        normal
    }

    pub fn face_normal(&self, face: usize) -> Vec3 {
        self.area_normal(face).normalize_or_zero()
    }

    pub fn face_center(&self, face: usize) -> Vec3 {
        let n = self.faces[face].verts.len().max(1) as f32;
        self.corners(face).sum::<Vec3>() / n
    }

    /// The face's edges, in winding order.
    pub fn face_edges(&self, face: usize) -> impl Iterator<Item = Edge> + '_ {
        let v = &self.faces[face].verts;
        (0..v.len()).map(move |i| edge(v[i], v[(i + 1) % v.len()]))
    }

    /// Every edge once, in a stable order.
    pub fn edges(&self) -> Vec<Edge> {
        let mut edges: Vec<Edge> = (0..self.faces.len())
            .flat_map(|f| self.face_edges(f))
            .collect();
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// Corner `i` of `face`'s texture coordinate: its own, or projected.
    pub fn corner_uv(&self, face: usize, i: usize) -> Vec2 {
        let f = &self.faces[face];
        match f.uvs.get(i) {
            Some(&uv) => uv,
            None => box_uv(self.positions[f.verts[i] as usize], self.face_normal(face)),
        }
    }

    /// Corner `i` of `face`'s color, unpainted corners as `unpainted`.
    pub fn corner_color(&self, face: usize, i: usize, unpainted: Vec3) -> Vec3 {
        self.faces[face].colors.get(i).copied().unwrap_or(unpainted)
    }

    /// The color unpainted corners show (white over a texture).
    pub fn unpainted(&self) -> Vec3 {
        if self.texture.is_some() {
            Vec3::ONE
        } else {
            CLAY
        }
    }

    /// The face's triangles as corner numbers (a fan; faces are convex).
    fn fan(&self, face: usize) -> impl Iterator<Item = [usize; 3]> + use<> {
        let n = self.faces[face].verts.len();
        (1..n.saturating_sub(1)).map(|i| [0, i, i + 1])
    }

    /// The nearest face the ray hits, and how far along the ray.
    pub fn pick_face(&self, origin: Vec3, direction: Vec3) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32)> = None;
        for face in 0..self.faces.len() {
            let verts = &self.faces[face].verts;
            let p = |i: usize| self.positions[verts[i] as usize];
            for [a, b, c] in self.fan(face) {
                if let Some(t) = ray_triangle(origin, direction, p(a), p(b), p(c))
                    && best.is_none_or(|(_, d)| t < d)
                {
                    best = Some((face, t));
                }
            }
        }
        best
    }

    /// Per position, the blended normal of the smooth faces around it.
    fn smooth_normals(&self) -> Vec<Vec3> {
        let mut normals = vec![Vec3::ZERO; self.positions.len()];
        for face in 0..self.faces.len() {
            if self.faces[face].smooth {
                // Weighted by area, so slivers don't bend the shading.
                let n = self.area_normal(face);
                for &v in &self.faces[face].verts {
                    normals[v as usize] += n;
                }
            }
        }
        normals.iter().map(|n| n.normalize_or_zero()).collect()
    }

    /// Triangles to draw or save: every face has its own corners, with the
    /// face's normal (or the blended one where smooth), UV and color.
    /// `texture` is the model's texture image, if it has one.
    pub fn to_model(&self, texture: Option<&Image>) -> ModelData {
        let smooth = self.smooth_normals();
        let unpainted = self.unpainted();
        let mut mesh = MeshData::default();
        let mut parts = Vec::new();
        // Part by part, so each part's triangles are one range.
        for (part, name) in self.part_names().into_iter().enumerate() {
            let start = mesh.indices.len() as u32;
            for face in (0..self.faces.len()).filter(|&f| self.part_of(f) == part as u32) {
                self.face_triangles(face, &smooth, unpainted, &mut mesh);
            }
            let end = mesh.indices.len() as u32;
            if end > start {
                parts.push(Part {
                    indices: start..end,
                    image: 0,
                    name,
                });
            }
        }
        let image = texture
            .cloned()
            .unwrap_or_else(|| Image::new(1, 1, vec![255; 4]));
        ModelData {
            mesh,
            parts,
            images: vec![image],
            ..Default::default()
        }
    }

    /// Appends `face`'s triangles to `mesh`, each corner its own vertex.
    fn face_triangles(&self, face: usize, smooth: &[Vec3], unpainted: Vec3, mesh: &mut MeshData) {
        {
            let f = &self.faces[face];
            let flat = self.face_normal(face);
            for corners in self.fan(face) {
                let base = mesh.vertices.len() as u32;
                for i in corners {
                    let v = f.verts[i] as usize;
                    let normal = if f.smooth { smooth[v] } else { flat };
                    mesh.vertices.push(Vertex {
                        position: self.positions[v].into(),
                        normal: normal.into(),
                        uv: self.corner_uv(face, i).into(),
                        color: self.corner_color(face, i, unpainted).extend(1.0).into(),
                    });
                }
                mesh.indices.extend([base, base + 1, base + 2]);
            }
        }
    }
}

/// Planar UVs along the normal's dominant axis, one texture per meter.
pub fn box_uv(p: Vec3, normal: Vec3) -> Vec2 {
    let a = normal.abs();
    if a.x >= a.y && a.x >= a.z {
        Vec2::new(p.z * -normal.x.signum(), -p.y)
    } else if a.y >= a.z {
        Vec2::new(p.x, p.z * normal.y.signum())
    } else {
        Vec2::new(p.x * normal.z.signum(), -p.y)
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
pub(crate) mod tests {
    use super::*;

    /// Every face's normal points away from the center, and every edge is
    /// shared by exactly two faces (the surface is closed).
    pub(crate) fn assert_closed_and_outward(mesh: &EditMesh) {
        let center = mesh.positions.iter().sum::<Vec3>() / mesh.positions.len() as f32;
        for face in 0..mesh.faces.len() {
            let out = (mesh.face_center(face) - center).normalize_or_zero();
            assert!(
                mesh.face_normal(face).dot(out) > 0.0,
                "face {face} faces inward"
            );
        }
        for e in mesh.edges() {
            let uses = (0..mesh.faces.len())
                .filter(|&f| mesh.face_edges(f).any(|x| x == e))
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
        let model = EditMesh::cylinder(0.5, 1.0, 6).to_model(None);
        // 6 quads (2 triangles each) and two hexagons (4 each).
        assert_eq!(model.mesh.indices.len(), (6 * 2 + 2 * 4) * 3);
    }

    #[test]
    fn smooth_faces_blend_normals_and_flat_ones_dont() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let flat = mesh.to_model(None);
        assert!(flat.mesh.vertices.iter().all(|v| {
            let n = Vec3::from(v.normal);
            n.abs().max_element() > 0.999
        }));
        for f in &mut mesh.faces {
            f.smooth = true;
        }
        let smooth = mesh.to_model(None);
        // Every corner of a smooth cube points diagonally out.
        assert!(smooth.mesh.vertices.iter().all(|v| {
            let n = Vec3::from(v.normal);
            n.abs().abs_diff_eq(Vec3::splat(1.0 / 3f32.sqrt()), 1e-5)
        }));
    }

    #[test]
    fn colors_default_to_clay_or_white_over_a_texture() {
        let mut mesh = EditMesh::plane(Vec2::ONE);
        let color = |m: &EditMesh| Vec3::from_slice(&m.to_model(None).mesh.vertices[0].color[..3]);
        assert_eq!(color(&mesh), CLAY);
        mesh.texture = Some("textures/stone.png".into());
        assert_eq!(color(&mesh), Vec3::ONE);
        mesh.faces[0].colors = vec![Vec3::X; 4];
        assert_eq!(color(&mesh), Vec3::X);
    }
}

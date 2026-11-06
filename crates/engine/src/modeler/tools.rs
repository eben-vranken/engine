//! Editing operations on an [`EditMesh`]: extrude, inset, delete, merge,
//! subdivide, loop cut, flip, mirror, and the surface ones (shading, color,
//! UVs). Each keeps per-corner UVs and colors in step with the corners.

use super::mesh::{Edge, EditMesh, Face, box_uv, edge};
use glam::{Vec2, Vec3};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Positions closer than this are the same point (meters).
pub const WELD_DISTANCE: f32 = 1e-3;

impl EditMesh {
    /// Drops degenerate faces (fewer than three distinct corners in a row)
    /// and positions no face uses, renumbering the rest. Faces keep their
    /// order unless one is dropped. Returns old → new position indices.
    pub fn compact(&mut self) -> Vec<Option<u32>> {
        for face in &mut self.faces {
            dedupe_corners(face);
        }
        self.faces.retain(|f| f.verts.len() >= 3);
        let mut used = vec![false; self.positions.len()];
        for f in &self.faces {
            for &v in &f.verts {
                used[v as usize] = true;
            }
        }
        let mut remap = vec![None; self.positions.len()];
        let mut positions = Vec::new();
        for (i, &p) in self.positions.iter().enumerate() {
            if used[i] {
                remap[i] = Some(positions.len() as u32);
                positions.push(p);
            }
        }
        self.positions = positions;
        for f in &mut self.faces {
            for v in &mut f.verts {
                *v = remap[*v as usize].expect("used positions are kept");
            }
        }
        remap
    }

    /// Faces that use any of `verts`.
    pub fn faces_with_verts(&self, verts: &BTreeSet<u32>) -> BTreeSet<usize> {
        (0..self.faces.len())
            .filter(|&f| self.faces[f].verts.iter().any(|v| verts.contains(v)))
            .collect()
    }

    /// Faces that have any of `edges`.
    pub fn faces_with_edges(&self, edges: &BTreeSet<Edge>) -> BTreeSet<usize> {
        (0..self.faces.len())
            .filter(|&f| self.face_edges(f).any(|e| edges.contains(&e)))
            .collect()
    }

    /// Removes faces (and positions left unused).
    pub fn delete_faces(&mut self, faces: &BTreeSet<usize>) {
        let mut i = 0;
        self.faces.retain(|_| {
            i += 1;
            !faces.contains(&(i - 1))
        });
        self.compact();
    }

    /// Pulls a copy of the faces out along their normals by `distance`,
    /// joined to where they were by side walls (a region: faces sharing an
    /// edge stay connected). The faces keep their indices.
    pub fn extrude(&mut self, faces: &BTreeSet<usize>, distance: f32) {
        // Edges used once within the region are its outline.
        let mut uses: HashMap<Edge, usize> = HashMap::new();
        for &f in faces {
            for e in self.face_edges(f) {
                *uses.entry(e).or_default() += 1;
            }
        }
        // Each corner of the region gets a copy, moved along the average
        // normal of the region's faces around it.
        let mut copies: BTreeMap<u32, u32> = BTreeMap::new();
        let mut normals: BTreeMap<u32, Vec3> = BTreeMap::new();
        for &f in faces {
            let n = self.face_normal(f);
            for &v in &self.faces[f].verts {
                *normals.entry(v).or_default() += n;
            }
        }
        for (&v, n) in &normals {
            copies.insert(v, self.positions.len() as u32);
            let p = self.positions[v as usize] + n.normalize_or_zero() * distance;
            self.positions.push(p);
        }
        let mut walls = Vec::new();
        for &f in faces {
            let face = &self.faces[f];
            let n = face.verts.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (a, b) = (face.verts[i], face.verts[j]);
                if uses[&edge(a, b)] == 1 {
                    let mut wall = Face::new(vec![a, b, copies[&b], copies[&a]]);
                    if !face.colors.is_empty() {
                        let (ca, cb) = (face.colors[i], face.colors[j]);
                        wall.colors = vec![ca, cb, cb, ca];
                    }
                    wall.smooth = face.smooth;
                    walls.push(wall);
                }
            }
        }
        for &f in faces {
            for v in &mut self.faces[f].verts {
                *v = copies[v];
            }
        }
        self.faces.extend(walls);
        self.compact();
    }

    /// Shrinks each face toward its center by `amount` (0–1), joined to its
    /// old outline by a ring of new faces. The faces keep their indices.
    pub fn inset(&mut self, faces: &BTreeSet<usize>, amount: f32) {
        let amount = amount.clamp(0.0, 0.99);
        let mut ring = Vec::new();
        for &f in faces {
            let center = self.face_center(f);
            let n = self.faces[f].verts.len();
            let uv_center = (0..n).map(|i| self.corner_uv(f, i)).sum::<Vec2>() / n as f32;
            let corners: Vec<Vec3> = self.corners(f).collect();
            let inner: Vec<u32> = corners
                .into_iter()
                .map(|p| {
                    self.positions.push(p.lerp(center, amount));
                    self.positions.len() as u32 - 1
                })
                .collect();
            let face = &self.faces[f];
            for i in 0..n {
                let j = (i + 1) % n;
                let mut quad = Face::new(vec![face.verts[i], face.verts[j], inner[j], inner[i]]);
                if !face.uvs.is_empty() {
                    let (a, b) = (face.uvs[i], face.uvs[j]);
                    quad.uvs = vec![a, b, b.lerp(uv_center, amount), a.lerp(uv_center, amount)];
                }
                if !face.colors.is_empty() {
                    let (a, b) = (face.colors[i], face.colors[j]);
                    quad.colors = vec![a, b, b, a];
                }
                quad.smooth = face.smooth;
                ring.push(quad);
            }
            let face = &mut self.faces[f];
            for uv in &mut face.uvs {
                *uv = uv.lerp(uv_center, amount);
            }
            face.verts = inner;
        }
        self.faces.extend(ring);
    }

    /// Merges `verts` into one at their center. Returns its index.
    pub fn merge(&mut self, verts: &BTreeSet<u32>) -> Option<u32> {
        let &target = verts.first()?;
        let center = verts
            .iter()
            .map(|&v| self.positions[v as usize])
            .sum::<Vec3>()
            / verts.len() as f32;
        self.positions[target as usize] = center;
        for f in &mut self.faces {
            for v in &mut f.verts {
                if verts.contains(v) {
                    *v = target;
                }
            }
        }
        self.compact()[target as usize]
    }

    /// Merges corners closer than [`WELD_DISTANCE`]. Returns how many went.
    pub fn weld(&mut self) -> usize {
        let n = self.positions.len();
        let mut into: Vec<u32> = (0..n as u32).collect();
        for i in 0..n {
            if into[i] != i as u32 {
                continue;
            }
            let p = self.positions[i];
            for (j, target) in into.iter_mut().enumerate().skip(i + 1) {
                if *target == j as u32 && p.distance(self.positions[j]) < WELD_DISTANCE {
                    *target = i as u32;
                }
            }
        }
        for f in &mut self.faces {
            for v in &mut f.verts {
                *v = into[*v as usize];
            }
        }
        let before = self.positions.len();
        self.compact();
        before - self.positions.len()
    }

    /// Splits each of `edges` in the middle (every face along it gets the
    /// new corner). Returns the new corners.
    pub fn subdivide(&mut self, edges: &BTreeSet<Edge>) -> BTreeSet<u32> {
        let mids: BTreeMap<Edge, u32> = edges
            .iter()
            .map(|&e| {
                let p = (self.positions[e[0] as usize] + self.positions[e[1] as usize]) / 2.0;
                self.positions.push(p);
                (e, self.positions.len() as u32 - 1)
            })
            .collect();
        for f in 0..self.faces.len() {
            self.split_face_edges(f, &mids);
        }
        mids.values().copied().collect()
    }

    /// Inserts the corner `mids[e]` into face `f` on each of its edges `e`
    /// that has one, interpolating UVs and colors.
    fn split_face_edges(&mut self, f: usize, mids: &BTreeMap<Edge, u32>) {
        let unpainted = self.unpainted();
        let n = self.faces[f].verts.len();
        if !self.face_edges(f).any(|e| mids.contains_key(&e)) {
            return;
        }
        let had_uvs = !self.faces[f].uvs.is_empty();
        let had_colors = !self.faces[f].colors.is_empty();
        let mut out = Face::new(Vec::new());
        out.smooth = self.faces[f].smooth;
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b) = (self.faces[f].verts[i], self.faces[f].verts[j]);
            out.verts.push(a);
            if had_uvs {
                out.uvs.push(self.corner_uv(f, i));
            }
            if had_colors {
                out.colors.push(self.corner_color(f, i, unpainted));
            }
            if let Some(&m) = mids.get(&edge(a, b)) {
                out.verts.push(m);
                if had_uvs {
                    out.uvs
                        .push((self.corner_uv(f, i) + self.corner_uv(f, j)) / 2.0);
                }
                if had_colors {
                    let (ca, cb) = (
                        self.corner_color(f, i, unpainted),
                        self.corner_color(f, j, unpainted),
                    );
                    out.colors.push((ca + cb) / 2.0);
                }
            }
        }
        self.faces[f] = out;
    }

    /// Cuts a new edge loop across `start`: through the strip of quads it
    /// belongs to, splitting each in half (all the way round if the strip
    /// closes). Returns the new corners.
    pub fn loop_cut(&mut self, start: Edge) -> BTreeSet<u32> {
        let mut faces_of: HashMap<Edge, Vec<usize>> = HashMap::new();
        for f in 0..self.faces.len() {
            for e in self.face_edges(f) {
                faces_of.entry(e).or_default().push(f);
            }
        }
        // The strip: (quad, edge we entered by, edge we leave by).
        let mut strip: Vec<(usize, Edge, Edge)> = Vec::new();
        let mut visited = BTreeSet::new();
        for &first in faces_of.get(&start).into_iter().flatten() {
            let (mut face, mut entry) = (first, start);
            while self.faces[face].verts.len() == 4 && visited.insert(face) {
                let exit = opposite(&self.faces[face].verts, entry);
                strip.push((face, entry, exit));
                let Some(&next) = faces_of[&exit].iter().find(|&&g| g != face) else {
                    break;
                };
                (face, entry) = (next, exit);
            }
        }
        let crossed: BTreeSet<Edge> = strip.iter().flat_map(|&(_, a, b)| [a, b]).collect();
        let mids: BTreeMap<Edge, u32> = crossed
            .iter()
            .map(|&e| {
                let p = (self.positions[e[0] as usize] + self.positions[e[1] as usize]) / 2.0;
                self.positions.push(p);
                (e, self.positions.len() as u32 - 1)
            })
            .collect();
        let unpainted = self.unpainted();
        for &(f, entry, _) in &strip {
            // Rotate so corners 0–1 are the entry edge.
            let verts = &self.faces[f].verts;
            let r = (0..4)
                .find(|&i| edge(verts[i], verts[(i + 1) % 4]) == entry)
                .expect("the quad has its entry edge");
            let k = |i: usize| (r + i) % 4;
            let v = |i: usize| self.faces[f].verts[k(i)];
            let (m1, m2) = (mids[&edge(v(0), v(1))], mids[&edge(v(2), v(3))]);
            let face = &self.faces[f];
            let mut a = Face::new(vec![v(0), m1, m2, v(3)]);
            let mut b = Face::new(vec![m1, v(1), v(2), m2]);
            a.smooth = face.smooth;
            b.smooth = face.smooth;
            if !face.uvs.is_empty() {
                let uv = |i: usize| self.corner_uv(f, k(i));
                let (u1, u2) = ((uv(0) + uv(1)) / 2.0, (uv(2) + uv(3)) / 2.0);
                a.uvs = vec![uv(0), u1, u2, uv(3)];
                b.uvs = vec![u1, uv(1), uv(2), u2];
            }
            if !face.colors.is_empty() {
                let c = |i: usize| self.corner_color(f, k(i), unpainted);
                let (c1, c2) = ((c(0) + c(1)) / 2.0, (c(2) + c(3)) / 2.0);
                a.colors = vec![c(0), c1, c2, c(3)];
                b.colors = vec![c1, c(1), c(2), c2];
            }
            self.faces[f] = a;
            self.faces.push(b);
        }
        // Faces at the ends of an open strip (caps, n-gons) get the new
        // corner on their edge too, so the surface has no cracks.
        let cut: BTreeSet<usize> = strip.iter().map(|s| s.0).collect();
        for f in 0..self.faces.len() {
            if !cut.contains(&f) {
                self.split_face_edges(f, &mids);
            }
        }
        mids.values().copied().collect()
    }

    /// Turns faces to face the other way.
    pub fn flip(&mut self, faces: &BTreeSet<usize>) {
        for &f in faces {
            self.faces[f].reverse();
        }
    }

    /// Adds a mirror image of the whole mesh across the plane through the
    /// origin square to `axis` (0 = X, 1 = Y, 2 = Z), joined to the
    /// original where corners lie on that plane.
    pub fn mirror(&mut self, axis: usize) {
        let n = self.positions.len() as u32;
        let on_plane = |p: Vec3| p[axis].abs() < WELD_DISTANCE;
        let map: Vec<u32> = (0..n)
            .map(|i| {
                if on_plane(self.positions[i as usize]) {
                    i
                } else {
                    n + i
                }
            })
            .collect();
        let mirrored: Vec<Vec3> = self
            .positions
            .iter()
            .map(|&p| {
                let mut q = p;
                q[axis] = -q[axis];
                q
            })
            .collect();
        self.positions.extend(mirrored);
        let copies: Vec<Face> = self
            .faces
            .iter()
            .map(|f| {
                let mut copy = f.clone();
                for v in &mut copy.verts {
                    *v = map[*v as usize];
                }
                // Mirroring turns the winding inside out; reverse it back.
                copy.reverse();
                copy
            })
            .collect();
        self.faces.extend(copies);
        self.compact();
    }

    pub fn set_smooth(&mut self, faces: &BTreeSet<usize>, smooth: bool) {
        for &f in faces {
            self.faces[f].smooth = smooth;
        }
    }

    /// Paints the corners `(face, corner number)` for which `pick` is true.
    pub fn paint(&mut self, color: Vec3, pick: impl Fn(usize, u32) -> bool) {
        let unpainted = self.unpainted();
        for (f, face) in self.faces.iter_mut().enumerate() {
            let hits: Vec<usize> = (0..face.verts.len())
                .filter(|&i| pick(f, face.verts[i]))
                .collect();
            if hits.is_empty() {
                continue;
            }
            if face.colors.is_empty() {
                face.colors = vec![unpainted; face.verts.len()];
            }
            for i in hits {
                face.colors[i] = color;
            }
        }
    }

    /// Gives the faces box-projected UVs: `repeats` textures per meter,
    /// along each face's main axis.
    pub fn project_uvs(&mut self, faces: &BTreeSet<usize>, repeats: f32) {
        for &f in faces {
            let normal = self.face_normal(f);
            let uvs = self.faces[f]
                .verts
                .iter()
                .map(|&v| box_uv(self.positions[v as usize], normal) * repeats)
                .collect();
            self.faces[f].uvs = uvs;
        }
    }

    /// Changes the faces' UVs with `change(uv, center of their UVs)`.
    pub fn transform_uvs(&mut self, faces: &BTreeSet<usize>, change: impl Fn(Vec2, Vec2) -> Vec2) {
        let mut all = Vec::new();
        for &f in faces {
            let uvs: Vec<Vec2> = (0..self.faces[f].verts.len())
                .map(|i| self.corner_uv(f, i))
                .collect();
            all.extend(uvs.iter().copied());
            self.faces[f].uvs = uvs;
        }
        if all.is_empty() {
            return;
        }
        let center = all.iter().sum::<Vec2>() / all.len() as f32;
        for &f in faces {
            for uv in &mut self.faces[f].uvs {
                *uv = change(*uv, center);
            }
        }
    }
}

/// The edge across a quad from `entry`.
fn opposite(quad: &[u32], entry: Edge) -> Edge {
    let i = (0..4)
        .find(|&i| edge(quad[i], quad[(i + 1) % 4]) == entry)
        .expect("the quad has the edge");
    edge(quad[(i + 2) % 4], quad[(i + 3) % 4])
}

/// Drops repeated corners in a row (going round), with their UVs/colors.
fn dedupe_corners(face: &mut Face) {
    let n = face.verts.len();
    if n < 2 {
        return;
    }
    let keep: Vec<bool> = (0..n)
        .map(|i| face.verts[i] != face.verts[(i + 1) % n])
        .collect();
    fn retain<T>(items: &mut Vec<T>, keep: &[bool]) {
        if items.len() == keep.len() {
            let mut k = keep.iter();
            items.retain(|_| *k.next().expect("one flag per corner"));
        }
    }
    retain(&mut face.uvs, &keep);
    retain(&mut face.colors, &keep);
    retain(&mut face.verts, &keep);
}

#[cfg(test)]
mod tests {
    use super::super::mesh::tests::assert_closed_and_outward;
    use super::*;

    fn top(mesh: &EditMesh) -> usize {
        (0..mesh.faces.len())
            .find(|&f| mesh.face_normal(f).y > 0.99)
            .unwrap()
    }

    #[test]
    fn extruding_the_top_of_a_box_makes_it_taller_and_stays_closed() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let t = top(&mesh);
        mesh.extrude(&BTreeSet::from([t]), 0.5);
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (12, 10));
        assert!((mesh.face_center(t).y - 1.0).abs() < 1e-6);
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn extruding_two_neighbouring_faces_keeps_them_joined() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let t = top(&mesh);
        let side = (0..6).find(|&f| mesh.face_normal(f).x > 0.99).unwrap();
        mesh.extrude(&BTreeSet::from([t, side]), 0.2);
        // 6 new corners (the two faces share an edge), 6 walls.
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (14, 12));
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn inset_adds_a_ring_and_stays_closed() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let t = top(&mesh);
        mesh.inset(&BTreeSet::from([t]), 0.25);
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (12, 10));
        assert!(mesh.corners(t).all(|p| p.x.abs() < 0.4 && p.z.abs() < 0.4));
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn deleting_and_merging_clean_up_after_themselves() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        mesh.delete_faces(&BTreeSet::from([0, 1]));
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (8, 4));

        // Merging the top four corners turns the box into a pyramid.
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let top_corners: BTreeSet<u32> = (0..8u32)
            .filter(|&v| mesh.positions[v as usize].y > 0.0)
            .collect();
        let apex = mesh.merge(&top_corners).unwrap();
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (5, 5));
        assert!(mesh.positions[apex as usize].abs_diff_eq(Vec3::new(0.0, 0.5, 0.0), 1e-6));
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn a_loop_cut_goes_all_the_way_round_a_box() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        // A vertical edge: the cut runs horizontally round the four sides.
        let vertical = edge(0, 2);
        let new = mesh.loop_cut(vertical);
        assert_eq!(new.len(), 4);
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (12, 10));
        assert!(
            new.iter()
                .all(|&v| mesh.positions[v as usize].y.abs() < 1e-6)
        );
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn a_loop_cut_stopped_by_caps_splits_their_edges_too() {
        let mut mesh = EditMesh::cylinder(0.5, 1.0, 6);
        // A bottom edge of a side quad: the strip is that one quad, ending
        // at the caps.
        let bottom = edge(0, 1);
        mesh.loop_cut(bottom);
        assert_closed_and_outward(&mesh);
        assert!(
            mesh.faces.iter().any(|f| f.verts.len() == 7),
            "a cap gained a corner"
        );
    }

    #[test]
    fn subdividing_an_edge_keeps_the_box_closed() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let new = mesh.subdivide(&BTreeSet::from([edge(0, 1)]));
        assert_eq!(new.len(), 1);
        assert_eq!(mesh.positions.len(), 9);
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn mirroring_half_a_box_makes_a_whole_one() {
        // A box from x = 0 to 1 with its face on the mirror plane removed.
        let mut mesh = EditMesh::cube(Vec3::ONE);
        for p in &mut mesh.positions {
            p.x += 0.5;
        }
        let inner = (0..6).find(|&f| mesh.face_normal(f).x < -0.99).unwrap();
        mesh.delete_faces(&BTreeSet::from([inner]));
        mesh.mirror(0);
        assert_eq!((mesh.positions.len(), mesh.faces.len()), (12, 10));
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn welding_joins_duplicate_corners() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        // Give one face its own copies of its corners.
        let n = mesh.positions.len() as u32;
        let verts = mesh.faces[0].verts.clone();
        for (i, v) in verts.iter().enumerate() {
            mesh.positions.push(mesh.positions[*v as usize]);
            mesh.faces[0].verts[i] = n + i as u32;
        }
        assert_eq!(mesh.weld(), 4);
        assert_closed_and_outward(&mesh);
    }

    #[test]
    fn uvs_and_colors_follow_the_corners() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        let t = top(&mesh);
        mesh.project_uvs(&BTreeSet::from([t]), 2.0);
        assert!(
            mesh.faces[t]
                .uvs
                .iter()
                .all(|uv| uv.abs().max_element() <= 1.0 + 1e-6)
        );
        mesh.transform_uvs(&BTreeSet::from([t]), |uv, _| uv + Vec2::X);
        assert!(mesh.faces[t].uvs.iter().all(|uv| uv.x >= -1e-6));

        mesh.paint(Vec3::X, |f, _| f == t);
        mesh.inset(&BTreeSet::from([t]), 0.5);
        mesh.loop_cut(edge(0, 2));
        for f in &mesh.faces {
            for list_len in [f.uvs.len(), f.colors.len()] {
                assert!(list_len == 0 || list_len == f.verts.len());
            }
        }
        assert!(mesh.faces.iter().any(|f| f.colors.contains(&Vec3::X)));
    }
}

//! Lightmap UVs: every static surface gets its own patch of texture.
//!
//! Each draw's triangles are grouped into charts: flat-ish pieces of
//! surface (triangles that share a vertex and face almost the same way).
//! A chart is projected onto its own plane at a fixed number of texels per
//! meter and packed into its zone's atlas with a border of padding texels,
//! so bilinear filtering never reads a neighbouring chart.
//!
//! Charts are laid out in world space, so a scaled box gets more texels,
//! and vertices shared between charts are duplicated (each needs its own
//! lightmap UV).

use crate::render::mesh::MeshData;
use glam::{Mat4, Vec2, Vec3};

/// Texels of padding around each chart.
pub const PAD: u32 = 2;
/// Largest chart side in texels; bigger faces get fewer texels per meter.
const MAX_CHART: u32 = 512;
/// Triangles whose normals are closer than this (cosine) can share a chart.
const SAME_PLANE: f32 = 0.98;

/// One chart: triangles of one draw, projected onto one plane.
#[derive(Debug, Clone)]
pub struct Chart {
    /// Indices into the draw's triangles.
    pub triangles: Vec<usize>,
    /// The plane: a point on it and two axes along it (meters).
    pub origin: Vec3,
    pub axes: [Vec3; 2],
    /// Smallest projected coordinate (meters along the axes).
    pub min: Vec2,
    /// Texels per meter in this chart.
    pub scale: f32,
    /// Size in texels, padding included.
    pub size: [u32; 2],
    /// Top-left in the atlas, once packed.
    pub rect: [u32; 2],
}

impl Chart {
    /// A point on the plane (meters along the axes) → atlas texel coords.
    pub fn texel_of(&self, q: Vec2) -> Vec2 {
        Vec2::new(self.rect[0] as f32, self.rect[1] as f32)
            + (q - self.min) * self.scale
            + Vec2::splat(PAD as f32)
    }

    /// The center of a texel of this chart (local x, y) → plane coords.
    pub fn plane_of(&self, x: u32, y: u32) -> Vec2 {
        self.min + (Vec2::new(x as f32, y as f32) + 0.5 - Vec2::splat(PAD as f32)) / self.scale
    }

    pub fn project(&self, p: Vec3) -> Vec2 {
        let d = p - self.origin;
        Vec2::new(d.dot(self.axes[0]), d.dot(self.axes[1]))
    }
}

/// A draw split into charts, with its vertices re-made per chart.
#[derive(Debug, Clone, Default)]
pub struct Unwrapped {
    pub charts: Vec<Chart>,
    /// New vertices: the original vertex, its chart, and where it is on
    /// the chart's plane.
    pub vertices: Vec<(u32, usize, Vec2)>,
    /// New indices, same triangles in the same order.
    pub indices: Vec<u32>,
    /// World positions of each triangle's corners and its normal.
    pub triangles: Vec<([Vec3; 3], Vec3)>,
}

/// Splits a mesh (placed by `model`) into charts at `density` texels per
/// meter. Degenerate triangles go into charts of their own and are skipped
/// when baking.
pub fn unwrap(mesh: &MeshData, model: Mat4, density: f32) -> Unwrapped {
    let world: Vec<Vec3> = mesh
        .vertices
        .iter()
        .map(|v| model.transform_point3(Vec3::from(v.position)))
        .collect();
    let triangles: Vec<([Vec3; 3], Vec3)> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| {
            let p = [
                world[t[0] as usize],
                world[t[1] as usize],
                world[t[2] as usize],
            ];
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            (p, n)
        })
        .collect();

    // Group triangles: share a vertex and face the same way.
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut by_vertex: Vec<Vec<usize>> = vec![Vec::new(); mesh.vertices.len()];
    for (t, corners) in mesh.indices.chunks_exact(3).enumerate() {
        for &v in corners {
            by_vertex[v as usize].push(t);
        }
    }
    for shared in &by_vertex {
        for pair in shared.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if triangles[a].1.dot(triangles[b].1) > SAME_PLANE {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut chart_of_root = vec![usize::MAX; triangles.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for t in 0..triangles.len() {
        let r = root(&mut parent, t);
        if chart_of_root[r] == usize::MAX {
            chart_of_root[r] = groups.len();
            groups.push(Vec::new());
        }
        groups[chart_of_root[r]].push(t);
    }

    let mut out = Unwrapped {
        triangles,
        indices: vec![0; mesh.indices.len()],
        ..Default::default()
    };
    for group in groups {
        let area_normal: Vec3 = group
            .iter()
            .map(|&t| {
                let (p, _) = out.triangles[t];
                (p[1] - p[0]).cross(p[2] - p[0])
            })
            .sum();
        let normal = area_normal.normalize_or(Vec3::Y);
        let helper = if normal.y.abs() < 0.9 {
            Vec3::Y
        } else {
            Vec3::X
        };
        let u = helper.cross(normal).normalize();
        let v = normal.cross(u);
        let origin = out.triangles[group[0]].0[0];
        let mut chart = Chart {
            triangles: group,
            origin,
            axes: [u, v],
            min: Vec2::ZERO,
            scale: density,
            size: [0, 0],
            rect: [0, 0],
        };
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &t in &chart.triangles {
            for p in out.triangles[t].0 {
                let q = chart.project(p);
                lo = lo.min(q);
                hi = hi.max(q);
            }
        }
        let extent = (hi - lo).max(Vec2::splat(1e-3));
        let biggest = extent.max_element() * density;
        if biggest > (MAX_CHART - 2 * PAD - 1) as f32 {
            chart.scale = (MAX_CHART - 2 * PAD - 1) as f32 / extent.max_element();
        }
        chart.min = lo;
        let texels = (extent * chart.scale).ceil();
        chart.size = [texels.x as u32 + 2 * PAD + 1, texels.y as u32 + 2 * PAD + 1];

        // New vertices for this chart.
        let chart_index = out.charts.len();
        let mut made: Vec<(u32, u32)> = Vec::new();
        for &t in &chart.triangles {
            for corner in 0..3 {
                let original = mesh.indices[t * 3 + corner];
                let index = match made.iter().find(|(o, _)| *o == original) {
                    Some(&(_, i)) => i,
                    None => {
                        let i = out.vertices.len() as u32;
                        let q = chart.project(world[original as usize]);
                        out.vertices.push((original, chart_index, q));
                        made.push((original, i));
                        i
                    }
                };
                out.indices[t * 3 + corner] = index;
            }
        }
        out.charts.push(chart);
    }
    out
}

/// Packs charts into rows ("shelves"), tallest first. Sets each chart's
/// `rect` and returns the atlas size.
pub fn pack(charts: &mut [&mut Chart]) -> [u32; 2] {
    if charts.is_empty() {
        return [0, 0];
    }
    let area: u64 = charts
        .iter()
        .map(|c| c.size[0] as u64 * c.size[1] as u64)
        .sum();
    let widest = charts.iter().map(|c| c.size[0]).max().unwrap_or(1);
    let width = ((area as f64 * 1.2).sqrt().ceil() as u32)
        .next_power_of_two()
        .max(widest)
        .max(16);
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((charts[i].size[1], charts[i].size[0])));
    let (mut x, mut y, mut shelf) = (0, 0, 0);
    for i in order {
        let [w, h] = charts[i].size;
        if x + w > width {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        charts[i].rect = [x, y];
        x += w;
        shelf = shelf.max(h);
    }
    [width, (y + shelf).next_multiple_of(4)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;

    #[test]
    fn a_box_has_six_charts_sized_by_its_faces() {
        let mesh = MeshData::from_shape(Shape::Box {
            size: Vec3::new(4.0, 2.0, 1.0),
        });
        let u = unwrap(&mesh, Mat4::IDENTITY, 4.0);
        assert_eq!(u.charts.len(), 6);
        // The 4 × 2 m faces: 16 × 8 texels plus padding.
        assert!(
            u.charts
                .iter()
                .any(|c| (c.size == [17 + 2 * PAD, 9 + 2 * PAD])
                    || (c.size == [9 + 2 * PAD, 17 + 2 * PAD]))
        );
        assert_eq!(u.indices.len(), mesh.indices.len());
    }

    #[test]
    fn scale_gives_more_texels() {
        let mesh = MeshData::from_shape(Shape::Box { size: Vec3::ONE });
        let small = unwrap(&mesh, Mat4::IDENTITY, 4.0);
        let big = unwrap(&mesh, Mat4::from_scale(Vec3::splat(3.0)), 4.0);
        assert!(big.charts[0].size[0] > small.charts[0].size[0]);
    }

    #[test]
    fn packed_charts_never_overlap() {
        let mesh = MeshData::from_shape(Shape::Cylinder {
            radius: 1.0,
            height: 3.0,
            segments: 8,
        });
        let mut u = unwrap(&mesh, Mat4::IDENTITY, 4.0);
        let mut refs: Vec<&mut Chart> = u.charts.iter_mut().collect();
        let [w, h] = pack(&mut refs);
        let rects: Vec<[u32; 4]> = u
            .charts
            .iter()
            .map(|c| [c.rect[0], c.rect[1], c.size[0], c.size[1]])
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(a[0] + a[2] <= w && a[1] + a[3] <= h);
            for b in &rects[i + 1..] {
                let apart = a[0] + a[2] <= b[0]
                    || b[0] + b[2] <= a[0]
                    || a[1] + a[3] <= b[1]
                    || b[1] + b[3] <= a[1];
                assert!(apart, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn every_corner_lands_inside_its_chart() {
        let mesh = MeshData::from_shape(Shape::Wedge {
            size: Vec3::new(2.0, 1.0, 3.0),
        });
        let mut u = unwrap(&mesh, Mat4::IDENTITY, 8.0);
        let mut refs: Vec<&mut Chart> = u.charts.iter_mut().collect();
        pack(&mut refs);
        for &(_, chart, q) in &u.vertices {
            let c = &u.charts[chart];
            let t = c.texel_of(q);
            let lo = Vec2::new(c.rect[0] as f32, c.rect[1] as f32) + PAD as f32 - 1e-3;
            let hi = lo + Vec2::new(c.size[0] as f32, c.size[1] as f32) - 2.0 * PAD as f32 + 2e-3;
            assert!(t.cmpge(lo).all() && t.cmple(hi).all(), "{t} {lo} {hi}");
        }
    }
}

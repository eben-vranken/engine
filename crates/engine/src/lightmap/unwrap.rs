//! Lightmap UVs: every static surface gets its own patch of texture.
//!
//! Each draw's triangles are grouped into charts: flat-ish pieces of
//! surface (triangles that share a vertex and face almost the same way).
//! A chart is projected onto its own plane at a fixed number of texels per
//! meter and packed into its zone's atlas with a border of padding texels,
//! so bilinear filtering never reads a neighbouring chart.
//!
//! A mesh is unwrapped once, in its own space (with the shape of its
//! scale), into a layout shared by every placement of it; each placement
//! gets that layout scaled to its size in its zone's atlas (see
//! `lightmap::bake`). Vertices shared between charts are duplicated (each
//! needs its own lightmap UV).

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

    /// A point in the layout (texels at the unwrap's density) → plane
    /// coords; the inverse of `texel_of`.
    pub fn plane_at(&self, layout: Vec2) -> Vec2 {
        let rect = Vec2::new(self.rect[0] as f32, self.rect[1] as f32);
        self.min + (layout - rect - Vec2::splat(PAD as f32)) / self.scale
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
    let sizes: Vec<[u32; 2]> = charts.iter().map(|c| c.size).collect();
    let (corners, size) = pack_sizes(&sizes, true);
    for (chart, corner) in charts.iter_mut().zip(corners) {
        chart.rect = corner;
    }
    size
}

/// Packs one mesh's charts as tightly as shelves allow (no power-of-two
/// width): the layout every placement of the mesh scales into its atlas.
pub fn pack_layout(charts: &mut [Chart]) -> [u32; 2] {
    let sizes: Vec<[u32; 2]> = charts.iter().map(|c| c.size).collect();
    // Shelves waste what's above shorter charts in a row; a layout is
    // packed twice (its charts, then it into the atlas), so try a few
    // widths and keep the smallest. Only roughly square ones: a long thin
    // layout makes its atlas as long.
    let area: u64 = sizes.iter().map(|s| s[0] as u64 * s[1] as u64).sum();
    let widest = sizes.iter().map(|s| s[0]).max().unwrap_or(1);
    let square = (area as f64).sqrt().ceil() as u32;
    let (corners, size) = (0..=16)
        .map(|k| (square / 2 + square * 3 * k / 32).max(widest))
        .map(|width| shelves(&sizes, width))
        .min_by_key(|(_, [w, h])| (*w as u64 * *h as u64, w.abs_diff(*h)))
        .unwrap_or_default();
    for (chart, corner) in charts.iter_mut().zip(corners) {
        chart.rect = corner;
    }
    size
}

/// Rectangles packed into rows ("shelves"), tallest first: each one's
/// top-left corner, and the size of the whole. `atlas`: a power-of-two
/// width and a height rounded up to 4, as atlases are made.
pub fn pack_sizes(sizes: &[[u32; 2]], atlas: bool) -> (Vec<[u32; 2]>, [u32; 2]) {
    if sizes.is_empty() {
        return (Vec::new(), [0, 0]);
    }
    let area: u64 = sizes.iter().map(|s| s[0] as u64 * s[1] as u64).sum();
    let widest = sizes.iter().map(|s| s[0]).max().unwrap_or(1);
    if !atlas {
        let width = ((area as f64 * 1.1).sqrt().ceil() as u32).max(widest);
        return shelves(sizes, width);
    }
    // Power-of-two widths around a square; the one with the least area.
    let first = ((area as f64).sqrt().ceil() as u32)
        .next_power_of_two()
        .max(widest.next_power_of_two())
        .max(16);
    [first / 2, first, first * 2]
        .into_iter()
        .filter(|&w| w >= widest && w >= 16)
        .map(|width| {
            let (corners, height) = skyline(sizes, width);
            (corners, [width, height.next_multiple_of(4)])
        })
        .min_by_key(|(_, [w, h])| (*w as u64 * *h as u64, w.abs_diff(*h)))
        .unwrap_or_default()
}

/// Skyline packing ("bottom-left"): tallest first, each rectangle where its
/// top ends lowest along the outline of what's placed. Wastes far less than
/// shelves on rectangles of many sizes. Corners, and the height used.
fn skyline(sizes: &[[u32; 2]], width: u32) -> (Vec<[u32; 2]>, u32) {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((sizes[i][1], sizes[i][0])));
    // The outline: (x, height) steps, left to right, covering the width.
    let mut steps: Vec<(u32, u32)> = vec![(0, 0)];
    let mut corners = vec![[0, 0]; sizes.len()];
    let mut top = 0;
    for i in order {
        let [w, h] = sizes[i];
        // The lowest place to put it, leftmost on ties.
        let mut best: Option<(u32, u32, usize)> = None;
        for start in 0..steps.len() {
            let x = steps[start].0;
            if x + w > width {
                break;
            }
            let mut y = 0;
            for &(sx, sy) in &steps[start..] {
                if sx >= x + w {
                    break;
                }
                y = y.max(sy);
            }
            if best.is_none_or(|(by, bx, _)| (y + h, x) < (by + h, bx)) {
                best = Some((y, x, start));
            }
        }
        let Some((y, x, start)) = best else {
            // Wider than the atlas: on top of everything, at the left.
            corners[i] = [0, top];
            top += h;
            steps = vec![(0, top)];
            continue;
        };
        corners[i] = [x, y];
        top = top.max(y + h);
        // The outline under it becomes its top; the step that continued
        // past its right edge starts again there.
        let end = x + w;
        let after = steps[start..]
            .iter()
            .take_while(|(sx, _)| *sx < end)
            .last()
            .map_or(0, |&(_, sy)| sy);
        steps.retain(|&(sx, _)| sx < x || sx >= end);
        let at = steps.partition_point(|&(sx, _)| sx < x);
        steps.insert(at, (x, y + h));
        if end < width && steps.get(at + 1).is_none_or(|&(sx, _)| sx > end) {
            steps.insert(at + 1, (end, after));
        }
        steps.dedup_by(|b, a| a.1 == b.1);
    }
    (corners, top)
}

/// Shelf packing at a given width: corners, and the width and height used.
fn shelves(sizes: &[[u32; 2]], width: u32) -> (Vec<[u32; 2]>, [u32; 2]) {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((sizes[i][1], sizes[i][0])));
    let mut corners = vec![[0, 0]; sizes.len()];
    let (mut x, mut y, mut shelf, mut right) = (0, 0, 0, 0);
    for i in order {
        let [w, h] = sizes[i];
        if x + w > width {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        corners[i] = [x, y];
        x += w;
        right = right.max(x);
        shelf = shelf.max(h);
    }
    (corners, [right, y + shelf])
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

    /// Rectangles of many sizes (and some wider than a square atlas)
    /// never overlap and stay inside the atlas.
    #[test]
    fn atlas_packing_keeps_rectangles_apart() {
        let mut seed = 12345u32;
        let mut next = |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) % n + 1
        };
        let mut sizes: Vec<[u32; 2]> = (0..400).map(|_| [next(60), next(60)]).collect();
        sizes.push([300, 7]);
        sizes.push([5, 200]);
        let (corners, [w, h]) = pack_sizes(&sizes, true);
        assert!(w.is_power_of_two());
        let rects: Vec<[u32; 4]> = corners
            .iter()
            .zip(&sizes)
            .map(|(c, s)| [c[0], c[1], s[0], s[1]])
            .collect();
        let used: u64 = sizes.iter().map(|s| s[0] as u64 * s[1] as u64).sum();
        assert!(used as f64 / (w as f64 * h as f64) > 0.7, "{w} x {h}");
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a[0] + a[2] <= w && a[1] + a[3] <= h,
                "{a:?} outside {w} x {h}"
            );
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

//! A bounding volume hierarchy over triangles, built on the CPU and traced
//! in compute shaders (`shaders/occlusion.wgsl`) for the live ray-traced
//! effects.
//!
//! A binary tree of boxes: each node's box holds everything below it, and a
//! leaf lists a few triangles. A ray only looks inside the boxes it passes
//! through, so a trace costs about log(triangles) steps. Built by halving
//! the triangles at the median along the longest axis: quick to build and
//! good enough for short rays in low-poly scenes.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// Most triangles in a leaf.
const LEAF: usize = 4;

/// Matches `Node` in `occlusion.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Node {
    pub min: [f32; 3],
    /// A leaf's first triangle; otherwise its first child (the second
    /// follows it).
    pub first: u32,
    pub max: [f32; 3],
    /// Triangles in a leaf; 0 = not a leaf.
    pub count: u32,
}

impl Node {
    /// Filled in by `split`.
    const UNSET: Node = Node {
        min: [0.0; 3],
        first: 0,
        max: [0.0; 3],
        count: 0,
    };
}

/// A corner and the two edges from it; matches `Triangle` in
/// `occlusion.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Triangle {
    pub a: [f32; 3],
    _pad0: f32,
    pub ab: [f32; 3],
    _pad1: f32,
    pub ac: [f32; 3],
    _pad2: f32,
}

/// The tree: node 0 is the root.
#[derive(Debug, Clone, PartialEq)]
pub struct Bvh {
    pub nodes: Vec<Node>,
    pub triangles: Vec<Triangle>,
}

impl Default for Bvh {
    fn default() -> Self {
        Self::build(Vec::new())
    }
}

struct Item {
    corners: [Vec3; 3],
    center: Vec3,
}

impl Bvh {
    pub fn build(triangles: Vec<[Vec3; 3]>) -> Self {
        let mut items: Vec<Item> = triangles
            .into_iter()
            .map(|corners| Item {
                corners,
                center: (corners[0] + corners[1] + corners[2]) / 3.0,
            })
            .collect();
        // A tree always has a triangle, so tracing needs no special case:
        // an empty one gets a single point, which no ray hits.
        if items.is_empty() {
            items.push(Item {
                corners: [Vec3::ZERO; 3],
                center: Vec3::ZERO,
            });
        }
        let mut nodes = vec![Node::UNSET];
        split(&mut nodes, 0, &mut items, 0);
        let triangles = items
            .iter()
            .map(|item| {
                let [a, b, c] = item.corners;
                Triangle {
                    a: a.to_array(),
                    _pad0: 0.0,
                    ab: (b - a).to_array(),
                    _pad1: 0.0,
                    ac: (c - a).to_array(),
                    _pad2: 0.0,
                }
            })
            .collect();
        Self { nodes, triangles }
    }

    /// Distance to the nearest triangle along a ray, within `max`. The
    /// shader's trace, on the CPU: for tests.
    #[cfg(test)]
    pub fn trace(&self, origin: Vec3, direction: Vec3, max: f32) -> Option<f32> {
        let safe = |d: f32| if d.abs() < 1e-8 { 1e-8 } else { d };
        let inverse = Vec3::new(safe(direction.x), safe(direction.y), safe(direction.z)).recip();
        let mut nearest = max;
        let mut hit = false;
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index as usize];
            let t0 = (Vec3::from_array(node.min) - origin) * inverse;
            let t1 = (Vec3::from_array(node.max) - origin) * inverse;
            let near = t0.min(t1).max_element().max(0.0);
            let far = t0.max(t1).min_element().min(nearest);
            if near > far {
                continue;
            }
            if node.count == 0 {
                stack.extend([node.first, node.first + 1]);
                continue;
            }
            for triangle in &self.triangles[node.first as usize..][..node.count as usize] {
                if let Some(t) = hit_triangle(triangle, origin, direction)
                    && t < nearest
                {
                    nearest = t;
                    hit = true;
                }
            }
        }
        hit.then_some(nearest)
    }
}

/// Fills node `at` with `items` (whose first triangle will be number
/// `first`), splitting them into two children while there are too many for
/// a leaf.
fn split(nodes: &mut Vec<Node>, at: usize, items: &mut [Item], first: usize) {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut center_min = min;
    let mut center_max = max;
    for item in items.iter() {
        for corner in item.corners {
            min = min.min(corner);
            max = max.max(corner);
        }
        center_min = center_min.min(item.center);
        center_max = center_max.max(item.center);
    }
    if items.len() <= LEAF {
        nodes[at] = Node {
            min: min.to_array(),
            first: first as u32,
            max: max.to_array(),
            count: items.len() as u32,
        };
        return;
    }
    let spread = center_max - center_min;
    let axis = if spread.x >= spread.y && spread.x >= spread.z {
        0
    } else if spread.y >= spread.z {
        1
    } else {
        2
    };
    let mid = items.len() / 2;
    items.select_nth_unstable_by(mid, |a, b| a.center[axis].total_cmp(&b.center[axis]));
    let children = nodes.len();
    nodes.extend([Node::UNSET; 2]);
    nodes[at] = Node {
        min: min.to_array(),
        first: children as u32,
        max: max.to_array(),
        count: 0,
    };
    let (left, right) = items.split_at_mut(mid);
    split(nodes, children, left, first);
    split(nodes, children + 1, right, first + mid);
}

/// Möller–Trumbore, either side of the triangle.
#[cfg(test)]
fn hit_triangle(triangle: &Triangle, origin: Vec3, direction: Vec3) -> Option<f32> {
    let ab = Vec3::from_array(triangle.ab);
    let ac = Vec3::from_array(triangle.ac);
    let p = direction.cross(ac);
    let det = ab.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let s = origin - Vec3::from_array(triangle.a);
    let u = s.dot(p) / det;
    let q = s.cross(ab);
    let v = direction.dot(q) / det;
    let t = ac.dot(q) / det;
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of unit floor tiles (two triangles each) at height 0.
    fn floor(side: i32) -> Vec<[Vec3; 3]> {
        let mut triangles = Vec::new();
        for x in 0..side {
            for z in 0..side {
                let p = |dx: i32, dz: i32| Vec3::new((x + dx) as f32, 0.0, (z + dz) as f32);
                triangles.push([p(0, 0), p(1, 0), p(1, 1)]);
                triangles.push([p(0, 0), p(1, 1), p(0, 1)]);
            }
        }
        triangles
    }

    #[test]
    fn an_empty_tree_stops_nothing() {
        let bvh = Bvh::default();
        assert_eq!((bvh.nodes.len(), bvh.triangles.len()), (1, 1));
        assert_eq!(bvh.trace(Vec3::ZERO, Vec3::NEG_Y, 100.0), None);
    }

    #[test]
    fn rays_find_the_nearest_triangle() {
        let mut triangles = floor(16);
        // A second floor two meters up, over part of the first.
        triangles.extend(floor(4).into_iter().map(|t| t.map(|p| p + Vec3::Y * 2.0)));
        let bvh = Bvh::build(triangles);
        let down = |x: f32, z: f32| bvh.trace(Vec3::new(x, 5.0, z), Vec3::NEG_Y, 100.0);
        assert!((down(1.3, 2.6).unwrap() - 3.0).abs() < 1e-4, "upper floor");
        assert!((down(9.3, 12.6).unwrap() - 5.0).abs() < 1e-4, "lower floor");
        assert_eq!(down(20.0, 3.0), None, "past the edge");
        // From below, the triangles' other side.
        let up = bvh.trace(Vec3::new(3.5, -1.0, 3.5), Vec3::Y, 100.0);
        assert!((up.unwrap() - 1.0).abs() < 1e-4);
        // Too short to reach.
        assert_eq!(bvh.trace(Vec3::new(9.3, 5.0, 12.6), Vec3::NEG_Y, 4.0), None);
    }

    #[test]
    fn every_triangle_is_in_exactly_one_leaf() {
        let bvh = Bvh::build(floor(9));
        let mut seen = vec![0; bvh.triangles.len()];
        for node in bvh.nodes.iter().filter(|n| n.count > 0) {
            assert!(node.count as usize <= LEAF);
            for i in node.first..node.first + node.count {
                seen[i as usize] += 1;
            }
        }
        assert!(seen.iter().all(|&n| n == 1));
    }

    /// Triangles all in one place still split (by count, not by position).
    #[test]
    fn stacked_triangles_still_build() {
        let one = [Vec3::ZERO, Vec3::X, Vec3::Z];
        let bvh = Bvh::build(vec![one; 100]);
        assert_eq!(bvh.triangles.len(), 100);
        assert!(
            bvh.trace(Vec3::new(0.2, 1.0, 0.2), Vec3::NEG_Y, 5.0)
                .is_some()
        );
    }
}

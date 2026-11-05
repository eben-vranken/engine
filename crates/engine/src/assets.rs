//! Assets loaded from the game project: glTF models for now. Paths are
//! relative to the project folder and loaded once, then cached.

use crate::render::mesh::{MeshData, Shape, Vertex};
use crate::scene::Geometry;
use glam::{Mat3, Mat4, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{path}: {source}")]
    Gltf { path: PathBuf, source: gltf::Error },
    #[error("{0}: no triangles found")]
    Empty(PathBuf),
}

#[derive(Default)]
pub struct Assets {
    /// The game project folder.
    root: PathBuf,
    /// `None` = failed to load (already reported).
    models: HashMap<String, Option<Arc<MeshData>>>,
}

impl Assets {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            models: HashMap::new(),
        }
    }

    /// The model at `path` (relative to the project), loading it on first use.
    /// Returns `None` if it can't be loaded; the error is logged once.
    pub fn model(&mut self, path: &str) -> Option<Arc<MeshData>> {
        let root = &self.root;
        self.models
            .entry(path.to_owned())
            .or_insert_with(|| match load_gltf(&root.join(path)) {
                Ok(mesh) => Some(Arc::new(mesh)),
                Err(err) => {
                    log::error!("{err}");
                    None
                }
            })
            .clone()
    }
}

impl Assets {
    /// Forgets a cached model, so the next use loads the file again.
    pub fn forget(&mut self, path: &str) {
        self.models.remove(path);
    }
}

/// Geometry ready to draw or collide with.
pub enum Resolved {
    Shape(Shape),
    Mesh(Arc<MeshData>),
}

/// Stands in for a model that failed to load, so the object stays visible
/// and selectable.
const PLACEHOLDER: Shape = Shape::Box {
    size: Vec3::splat(0.5),
};

impl Assets {
    pub fn resolve(&mut self, geometry: Geometry) -> Resolved {
        match geometry {
            Geometry::Shape(shape) => Resolved::Shape(shape),
            Geometry::Model(path) => match self.model(path) {
                Some(mesh) => Resolved::Mesh(mesh),
                None => Resolved::Shape(PLACEHOLDER),
            },
        }
    }
}

/// All triangles of a glTF file's default scene as one mesh, with node
/// transforms applied. Missing normals become flat normals.
pub fn load_gltf(path: &Path) -> Result<MeshData, AssetError> {
    let (document, buffers, _images) = gltf::import(path).map_err(|source| AssetError::Gltf {
        path: path.to_owned(),
        source,
    })?;
    let mut mesh = MeshData::default();
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    for node in scene.iter().flat_map(|s| s.nodes()) {
        add_node(&node, Mat4::IDENTITY, &buffers, &mut mesh);
    }
    if mesh.indices.is_empty() {
        return Err(AssetError::Empty(path.to_owned()));
    }
    Ok(mesh)
}

fn add_node(node: &gltf::Node, parent: Mat4, buffers: &[gltf::buffer::Data], mesh: &mut MeshData) {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
    if let Some(node_mesh) = node.mesh() {
        for primitive in node_mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<Vec3> = positions
                .map(|p| world.transform_point3(Vec3::from(p)))
                .collect();
            let normals: Option<Vec<Vec3>> = reader.read_normals().map(|n| {
                n.map(|n| (normal_matrix * Vec3::from(n)).normalize_or_zero())
                    .collect()
            });
            let uvs: Option<Vec<[f32; 2]>> =
                reader.read_tex_coords(0).map(|t| t.into_f32().collect());
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };

            let base = mesh.vertices.len() as u32;
            match normals {
                Some(normals) => {
                    for (i, p) in positions.iter().enumerate() {
                        mesh.vertices.push(Vertex {
                            position: (*p).into(),
                            normal: normals.get(i).copied().unwrap_or(Vec3::Y).into(),
                            uv: uvs
                                .as_ref()
                                .and_then(|u| u.get(i))
                                .copied()
                                .unwrap_or_default(),
                        });
                    }
                    mesh.indices.extend(indices.iter().map(|i| base + i));
                }
                None => {
                    // Flat shading: own vertices per triangle.
                    for tri in indices.chunks_exact(3) {
                        let [a, b, c] = [0, 1, 2].map(|k| positions[tri[k] as usize]);
                        let normal = (b - a).cross(c - a).normalize_or_zero();
                        for &i in tri {
                            mesh.indices.push(mesh.vertices.len() as u32);
                            mesh.vertices.push(Vertex {
                                position: positions[i as usize].into(),
                                normal: normal.into(),
                                uv: uvs
                                    .as_ref()
                                    .and_then(|u| u.get(i as usize))
                                    .copied()
                                    .unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }
    }
    for child in node.children() {
        add_node(&child, world, buffers, mesh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_sandbox_arch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let mesh = load_gltf(&root.join("models/arch.gltf")).expect("arch.gltf");
        assert_eq!(mesh.indices.len() / 3, 84);
        let top = mesh
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(0.0, f32::max);
        assert!((top - 3.5).abs() < 1e-4, "arch is 3.5 m tall, got {top}");
        assert!(
            mesh.vertices
                .iter()
                .all(|v| (Vec3::from(v.normal).length() - 1.0).abs() < 1e-3)
        );
    }

    #[test]
    fn missing_model_is_none_not_a_panic() {
        let mut assets = Assets::new(PathBuf::from("nowhere"));
        assert!(assets.model("models/missing.gltf").is_none());
        // Cached: asking again doesn't retry or panic.
        assert!(assets.model("models/missing.gltf").is_none());
    }
}

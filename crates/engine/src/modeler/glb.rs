//! Saving and opening the modeler's models as binary glTF (`.glb`).
//!
//! The file is an ordinary glTF any tool can read: flat-shaded triangles
//! with normals and UVs, and one matte material. The editable polygons are
//! kept in the mesh's `extras` (glTF's slot for app data), so the modeler
//! can open the file again with its quads intact.

use super::mesh::EditMesh;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The key in the mesh's `extras`.
const EXTRAS_KEY: &str = "modeler";
const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum GlbError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Gltf(PathBuf, gltf::Error),
    #[error("{0} wasn't made with the modeler (it has no editable mesh)")]
    NotEditable(PathBuf),
    #[error("{0}: bad modeler data: {1}")]
    Data(PathBuf, serde_json::Error),
}

/// What goes in the extras.
#[derive(Serialize, Deserialize)]
struct Extras {
    version: u32,
    mesh: EditMesh,
}

#[derive(Deserialize)]
struct MeshExtras {
    modeler: Extras,
}

/// Writes `mesh` to `path` as a `.glb`.
pub fn save(path: &Path, mesh: &EditMesh) -> Result<(), GlbError> {
    let io = |err| GlbError::Io(path.to_owned(), err);
    let bytes = encode(mesh);
    // Write next to it, then replace: the engine's hot reload never sees
    // half a file.
    let partial = path.with_extension("glb.partial");
    std::fs::write(&partial, bytes).map_err(io)?;
    std::fs::rename(&partial, path).map_err(io)
}

/// The editable mesh in a `.glb` the modeler saved.
pub fn load(path: &Path) -> Result<EditMesh, GlbError> {
    let gltf = gltf::Gltf::open(path).map_err(|e| GlbError::Gltf(path.to_owned(), e))?;
    let extras = gltf
        .meshes()
        .find_map(|m| m.extras().as_ref().map(|e| e.get().to_owned()))
        .ok_or_else(|| GlbError::NotEditable(path.to_owned()))?;
    let extras: MeshExtras =
        serde_json::from_str(&extras).map_err(|e| GlbError::Data(path.to_owned(), e))?;
    Ok(extras.modeler.mesh)
}

/// Whether the modeler can open the file (cheap check: reads only the
/// JSON part).
pub fn is_editable(path: &Path) -> bool {
    load(path).is_ok()
}

fn encode(mesh: &EditMesh) -> Vec<u8> {
    let model = mesh.to_model();
    let vertices = &model.mesh.vertices;
    let indices = &model.mesh.indices;
    let (min, max) = model.mesh.bounds();
    let count = vertices.len();

    // The binary buffer: positions, normals, UVs, then indices.
    let mut bin: Vec<u8> = Vec::new();
    let view = |bin: &mut Vec<u8>, data: &[u8]| {
        let offset = bin.len();
        bin.extend_from_slice(data);
        (offset, data.len())
    };
    let pick = |f: fn(&crate::render::mesh::Vertex) -> &[f32]| -> Vec<u8> {
        vertices
            .iter()
            .flat_map(|v| f(v).iter().flat_map(|x| x.to_le_bytes()))
            .collect()
    };
    let positions = view(&mut bin, &pick(|v| &v.position));
    let normals = view(&mut bin, &pick(|v| &v.normal));
    let uvs = view(&mut bin, &pick(|v| &v.uv));
    let index_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let index_view = view(&mut bin, &index_bytes);

    const ARRAY_BUFFER: u32 = 34962;
    const ELEMENT_ARRAY_BUFFER: u32 = 34963;
    const FLOAT: u32 = 5126;
    const UNSIGNED_INT: u32 = 5125;
    let buffer_view = |(offset, length): (usize, usize), target| json!({"buffer": 0, "byteOffset": offset, "byteLength": length, "target": target});
    let extras = Extras {
        version: VERSION,
        mesh: mesh.clone(),
    };
    let document = json!({
        "asset": {"version": "2.0", "generator": format!("{} modeler {}", crate::NAME, crate::VERSION)},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{
            "primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
                "indices": 3,
                "material": 0,
            }],
            "extras": {EXTRAS_KEY: extras},
        }],
        "materials": [{
            "pbrMetallicRoughness": {
                "baseColorFactor": [0.6, 0.6, 0.6, 1.0],
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
        }],
        "accessors": [
            {"bufferView": 0, "componentType": FLOAT, "count": count, "type": "VEC3",
             "min": [min.x, min.y, min.z], "max": [max.x, max.y, max.z]},
            {"bufferView": 1, "componentType": FLOAT, "count": count, "type": "VEC3"},
            {"bufferView": 2, "componentType": FLOAT, "count": count, "type": "VEC2"},
            {"bufferView": 3, "componentType": UNSIGNED_INT, "count": indices.len(), "type": "SCALAR"},
        ],
        "bufferViews": [
            buffer_view(positions, ARRAY_BUFFER),
            buffer_view(normals, ARRAY_BUFFER),
            buffer_view(uvs, ARRAY_BUFFER),
            buffer_view(index_view, ELEMENT_ARRAY_BUFFER),
        ],
        "buffers": [{"byteLength": bin.len()}],
    });

    // GLB: a 12-byte header, then a JSON chunk and a binary chunk, each
    // padded to 4 bytes (JSON with spaces, binary with zeros).
    let mut text = serde_json::to_vec(&document).expect("the document serializes");
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_models_open_in_the_engine_and_the_modeler() {
        let dir = std::env::temp_dir().join(format!("engine-glb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pillar.glb");
        let mesh = EditMesh::cylinder(0.3, 2.0, 6);
        save(&path, &mesh).unwrap();

        assert_eq!(load(&path).unwrap(), mesh);
        let model = crate::assets::load_gltf(&path).unwrap();
        assert_eq!(model.mesh.indices.len(), mesh.to_model().mesh.indices.len());
        let (min, max) = model.mesh.bounds();
        assert!((max.y - min.y - 2.0).abs() < 1e-5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_models_are_not_editable() {
        let arch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/models/arch.gltf");
        assert!(matches!(load(&arch), Err(GlbError::NotEditable(_))));
    }
}

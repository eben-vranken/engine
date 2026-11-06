//! Saving and opening the modeler's models as binary glTF (`.glb`).
//!
//! The file is an ordinary glTF any tool can read: triangles with normals,
//! UVs and vertex colors, one material, and the model's texture embedded
//! (point-sampled). The editable polygons are kept in the mesh's `extras`
//! (glTF's slot for app data), so the modeler can open the file again with
//! its quads, UVs and colors intact.

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

/// Writes `mesh` to `path` as a `.glb`, embedding its texture (a path in
/// `project_dir`) so the file stands on its own.
pub fn save(path: &Path, mesh: &EditMesh, project_dir: &Path) -> Result<(), GlbError> {
    let io = |err| GlbError::Io(path.to_owned(), err);
    let texture = match &mesh.texture {
        Some(texture) => {
            let file = project_dir.join(texture);
            let bytes = std::fs::read(&file).map_err(|e| GlbError::Io(file.clone(), e))?;
            let lower = texture.to_ascii_lowercase();
            let jpeg = lower.ends_with(".jpg") || lower.ends_with(".jpeg");
            Some((bytes, if jpeg { "image/jpeg" } else { "image/png" }))
        }
        None => None,
    };
    let bytes = encode(mesh, texture);
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

/// Whether the modeler can open the file.
pub fn is_editable(path: &Path) -> bool {
    load(path).is_ok()
}

/// The GLB bytes: `texture` is the image file's bytes and media type.
fn encode(mesh: &EditMesh, texture: Option<(Vec<u8>, &str)>) -> Vec<u8> {
    let model = mesh.to_model(None);
    let vertices = &model.mesh.vertices;
    let indices = &model.mesh.indices;
    let (min, max) = model.mesh.bounds();
    let count = vertices.len();

    // The binary buffer: positions, normals, UVs, colors, indices, image.
    let mut bin: Vec<u8> = Vec::new();
    let view = |bin: &mut Vec<u8>, data: &[u8]| {
        // Every view starts 4-byte aligned, as glTF wants.
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
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
    let colors = view(&mut bin, &pick(|v| &v.color));
    let index_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let index_view = view(&mut bin, &index_bytes);

    const ARRAY_BUFFER: u32 = 34962;
    const ELEMENT_ARRAY_BUFFER: u32 = 34963;
    const FLOAT: u32 = 5126;
    const UNSIGNED_INT: u32 = 5125;
    const NEAREST: u32 = 9728;
    const REPEAT: u32 = 10497;
    let buffer_view = |(offset, length): (usize, usize), target: Option<u32>| {
        let mut v = json!({"buffer": 0, "byteOffset": offset, "byteLength": length});
        if let Some(target) = target {
            v["target"] = json!(target);
        }
        v
    };
    let mut buffer_views = vec![
        buffer_view(positions, Some(ARRAY_BUFFER)),
        buffer_view(normals, Some(ARRAY_BUFFER)),
        buffer_view(uvs, Some(ARRAY_BUFFER)),
        buffer_view(colors, Some(ARRAY_BUFFER)),
        buffer_view(index_view, Some(ELEMENT_ARRAY_BUFFER)),
    ];
    let mut material = json!({
        "pbrMetallicRoughness": {
            "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
            "metallicFactor": 0.0,
            "roughnessFactor": 1.0,
        },
    });
    let mut images = Vec::new();
    if let Some((bytes, mime)) = &texture {
        let image = view(&mut bin, bytes);
        buffer_views.push(buffer_view(image, None));
        images.push(json!({"bufferView": buffer_views.len() - 1, "mimeType": mime}));
        material["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": 0});
    }

    let extras = Extras {
        version: VERSION,
        mesh: mesh.clone(),
    };
    // One node and mesh per part, named after it; the parts share the
    // vertex data and each has its own stretch of the indices. The
    // editable mesh rides along on the first.
    let mut accessors = vec![
        json!({"bufferView": 0, "componentType": FLOAT, "count": count, "type": "VEC3",
               "min": [min.x, min.y, min.z], "max": [max.x, max.y, max.z]}),
        json!({"bufferView": 1, "componentType": FLOAT, "count": count, "type": "VEC3"}),
        json!({"bufferView": 2, "componentType": FLOAT, "count": count, "type": "VEC2"}),
        json!({"bufferView": 3, "componentType": FLOAT, "count": count, "type": "VEC4"}),
    ];
    let mut nodes = Vec::new();
    let mut meshes = Vec::new();
    for part in &model.parts {
        accessors.push(json!({
            "bufferView": 4,
            "byteOffset": part.indices.start * 4,
            "componentType": UNSIGNED_INT,
            "count": part.indices.len(),
            "type": "SCALAR",
        }));
        let mut gltf_mesh = json!({
            "name": part.name,
            "primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2, "COLOR_0": 3},
                "indices": accessors.len() - 1,
                "material": 0,
            }],
        });
        if meshes.is_empty() {
            gltf_mesh["extras"] = json!({EXTRAS_KEY: extras});
        }
        nodes.push(json!({"name": part.name, "mesh": meshes.len()}));
        meshes.push(gltf_mesh);
    }
    let mut document = json!({
        "asset": {"version": "2.0", "generator": format!("{} modeler {}", crate::NAME, crate::VERSION)},
        "scene": 0,
        "scenes": [{"nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": [material],
        "accessors": accessors,
        "bufferViews": buffer_views,
    });
    if !images.is_empty() {
        document["images"] = json!(images);
        document["samplers"] =
            json!([{"magFilter": NEAREST, "minFilter": NEAREST, "wrapS": REPEAT, "wrapT": REPEAT}]);
        document["textures"] = json!([{"source": 0, "sampler": 0}]);
    }

    // GLB: a 12-byte header, then a JSON chunk and a binary chunk, each
    // padded to 4 bytes (JSON with spaces, binary with zeros).
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    document["buffers"] = json!([{"byteLength": bin.len()}]);
    let mut text = serde_json::to_vec(&document).expect("the document serializes");
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
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
    use glam::Vec3;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("engine-glb-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saved_models_open_in_the_engine_and_the_modeler() {
        let dir = temp("plain");
        let path = dir.join("pillar.glb");
        let mut mesh = EditMesh::cylinder(0.3, 2.0, 6);
        mesh.faces[0].colors = vec![Vec3::new(1.0, 0.0, 0.0); 4];
        save(&path, &mesh, &dir).unwrap();

        assert_eq!(load(&path).unwrap(), mesh);
        let model = crate::assets::load_gltf(&path).unwrap();
        assert_eq!(
            model.mesh.indices.len(),
            mesh.to_model(None).mesh.indices.len()
        );
        let (min, max) = model.mesh.bounds();
        assert!((max.y - min.y - 2.0).abs() < 1e-5);
        assert!(
            model
                .mesh
                .vertices
                .iter()
                .any(|v| v.color == [1.0, 0.0, 0.0, 1.0])
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_texture_is_embedded() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let dir = temp("textured");
        let path = dir.join("crate.glb");
        let mut mesh = EditMesh::cube(Vec3::ONE);
        mesh.texture = Some("textures/stone.png".into());
        save(&path, &mesh, &sandbox).unwrap();
        let model = crate::assets::load_gltf(&path).unwrap();
        assert!(
            model.images[0].width > 1,
            "the stone texture, not a plain texel"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_models_are_not_editable() {
        let arch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/models/arch.gltf");
        assert!(matches!(load(&arch), Err(GlbError::NotEditable(_))));
    }
}

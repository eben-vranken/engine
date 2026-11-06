//! A small built-in modeler for very low-poly models: an editable polygon
//! mesh, saved as glTF the engine (and any other tool) can load.

pub mod glb;
pub mod mesh;
pub mod tools;

use crate::assets::{Assets, ModelData};
use mesh::EditMesh;

/// The model to draw for `mesh`, with its texture from the project.
pub fn model_data(mesh: &EditMesh, assets: &mut Assets) -> ModelData {
    let texture = mesh.texture.as_deref().and_then(|t| assets.texture(t));
    mesh.to_model(texture.as_deref())
}

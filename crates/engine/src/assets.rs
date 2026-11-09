//! Assets loaded from the game project: glTF models for now. Paths are
//! relative to the project folder and loaded once, then cached.

use crate::render::mesh::{MeshData, Shape, Vertex, WHITE};
use crate::render::texture::{Image, linear_to_srgb, srgb_to_linear};
use crate::scene::{Geometry, Parts};
use glam::{Mat3, Mat4, Vec3};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// A model ready to draw: one mesh, split into parts that each use one
/// image.
#[derive(Debug)]
pub struct ModelData {
    pub mesh: MeshData,
    pub parts: Vec<Part>,
    /// One per material: its base color texture times its base color
    /// factor, or a single texel of the factor if it has no texture.
    pub images: Vec<Image>,
    /// The materials' other maps, by the same index as `images` (missing
    /// = none).
    pub maps: Vec<SurfaceMaps>,
    /// Roughness and metalness of the first material (glTF factors); a
    /// model draws with one value for both.
    pub surface: [f32; 2],
}

impl Default for ModelData {
    fn default() -> Self {
        Self {
            mesh: MeshData::default(),
            parts: Vec::new(),
            images: Vec::new(),
            maps: Vec::new(),
            // Matte, not metal (glTF's own default metalness is 1, which
            // would turn models without materials black).
            surface: [1.0, 0.0],
        }
    }
}

/// A material's maps besides its base color, as glTF stores them. Each is
/// data, not color.
#[derive(Debug, Clone, Default)]
pub struct SurfaceMaps {
    /// Tangent-space normals in rgb (OpenGL convention: green = up),
    /// already scaled by the material's normal strength.
    pub normal: Option<Image>,
    /// Roughness in green, metalness in blue; the model's roughness and
    /// metalness multiply them.
    pub metal_rough: Option<Image>,
    /// Ambient occlusion in red, already scaled by its strength.
    pub occlusion: Option<Image>,
}

/// A range of `mesh.indices` drawn with one image.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub indices: Range<u32>,
    pub image: usize,
    /// The part's name in the file (its glTF node's, or mesh's). Several
    /// ranges share a name when a part uses several materials.
    pub name: String,
}

impl ModelData {
    /// The names of the model's parts, in file order, each once.
    pub fn part_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for part in &self.parts {
            if !names.contains(&part.name) {
                names.push(part.name.clone());
            }
        }
        names
    }

    /// The center of the part named `name`'s bounding box, in the model's
    /// own space.
    pub fn part_center(&self, name: &str) -> Option<Vec3> {
        let mut bounds: Option<(Vec3, Vec3)> = None;
        for part in self.parts.iter().filter(|p| p.name == name) {
            for &i in &self.mesh.indices[part.indices.start as usize..part.indices.end as usize] {
                let p = Vec3::from(self.mesh.vertices[i as usize].position);
                bounds = Some(bounds.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
            }
        }
        bounds.map(|(lo, hi)| (lo + hi) / 2.0)
    }

    /// The model with only the parts `parts` includes. With none of them
    /// left it has no parts, and one empty triangle so it can still be
    /// uploaded.
    pub fn subset(&self, parts: Parts) -> ModelData {
        let mut indices = Vec::new();
        let mut kept = Vec::new();
        for part in self.parts.iter().filter(|p| parts.includes(&p.name)) {
            let start = indices.len() as u32;
            indices.extend_from_slice(
                &self.mesh.indices[part.indices.start as usize..part.indices.end as usize],
            );
            kept.push(Part {
                indices: start..indices.len() as u32,
                ..part.clone()
            });
        }
        let mut vertices = self.mesh.vertices.clone();
        if indices.is_empty() {
            indices = vec![0; 3];
            if vertices.is_empty() {
                vertices.push(Vertex {
                    position: [0.0; 3],
                    normal: [0.0, 1.0, 0.0],
                    uv: [0.0; 2],
                    color: WHITE,
                });
            }
        }
        ModelData {
            mesh: MeshData { vertices, indices },
            parts: kept,
            images: self.images.clone(),
            maps: self.maps.clone(),
            surface: self.surface,
        }
    }
}

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
    models: HashMap<String, CachedModel>,
    /// Material textures by path; `None` = failed to load (reported once).
    textures: HashMap<String, Option<Arc<Image>>>,
}

struct CachedModel {
    /// `None` = failed to load (already reported).
    model: Option<Arc<ModelData>>,
    /// The file's modified time when it was loaded.
    loaded: Option<SystemTime>,
    /// The file's modified time at the last poll.
    seen: Option<SystemTime>,
}

impl Assets {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            models: HashMap::new(),
            textures: HashMap::new(),
        }
    }

    /// The game project folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A material texture (relative to the project), loaded on first use;
    /// `None` if it can't be loaded (logged once).
    pub fn texture(&mut self, path: &str) -> Option<Arc<Image>> {
        let root = &self.root;
        self.textures
            .entry(path.to_owned())
            .or_insert_with(|| match image::open(root.join(path)) {
                Ok(decoded) => {
                    let rgba = decoded.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    Some(Arc::new(Image::new(w, h, rgba.into_raw())))
                }
                Err(err) => {
                    log::warn!("{path}: {err}");
                    None
                }
            })
            .clone()
    }

    /// The model at `path` (relative to the project), loading it on first use.
    /// Returns `None` if it can't be loaded; the error is logged once.
    pub fn model(&mut self, path: &str) -> Option<Arc<ModelData>> {
        let root = &self.root;
        self.models
            .entry(path.to_owned())
            .or_insert_with(|| {
                let file = root.join(path);
                let modified = modified_time(&file);
                let model = match load_gltf(&file) {
                    Ok(model) => Some(Arc::new(model)),
                    Err(err) => {
                        log::error!("{err}");
                        None
                    }
                };
                CachedModel {
                    model,
                    loaded: modified,
                    seen: modified,
                }
            })
            .model
            .clone()
    }

    /// Forgets models whose file changed on disk since they were loaded and
    /// has since stopped changing (exporters may write in several steps),
    /// so the next use loads them again. Returns their paths. Call it
    /// periodically.
    pub fn changed_models(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        for (path, cached) in &mut self.models {
            let now = modified_time(&self.root.join(path));
            if now.is_some() && now != cached.loaded && now == cached.seen {
                changed.push(path.clone());
            }
            cached.seen = now;
        }
        for path in &changed {
            self.models.remove(path);
        }
        changed
    }
}

impl Assets {
    /// Forgets a cached model, so the next use loads the file again.
    pub fn forget(&mut self, path: &str) {
        self.models.remove(path);
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Geometry ready to draw or collide with.
pub enum Resolved {
    Shape(Shape),
    Model(Arc<ModelData>),
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
            Geometry::Model(path, parts) => match self.model(path) {
                Some(model) if parts == Parts::All => Resolved::Model(model),
                Some(model) => Resolved::Model(Arc::new(model.subset(parts))),
                // The placeholder stands in once, for the whole model.
                None if matches!(parts, Parts::All | Parts::Except(_)) => {
                    Resolved::Shape(PLACEHOLDER)
                }
                None => Resolved::Model(Arc::new(ModelData::default().subset(parts))),
            },
        }
    }
}

/// All triangles of a glTF file's default scene as one mesh, with node
/// transforms applied, and each material's base color as an image. Missing
/// normals become flat normals.
pub fn load_gltf(path: &Path) -> Result<ModelData, AssetError> {
    let (document, buffers, images) = gltf::import(path).map_err(|source| AssetError::Gltf {
        path: path.to_owned(),
        source,
    })?;
    let mut loader = Loader {
        buffers: &buffers,
        images: &images,
        model: ModelData::default(),
        materials: HashMap::new(),
    };
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    for node in scene.iter().flat_map(|s| s.nodes()) {
        loader.add_node(&node, Mat4::IDENTITY);
    }
    if loader.model.mesh.indices.is_empty() {
        return Err(AssetError::Empty(path.to_owned()));
    }
    Ok(loader.model)
}

struct Loader<'a> {
    buffers: &'a [gltf::buffer::Data],
    images: &'a [gltf::image::Data],
    model: ModelData,
    /// glTF material index (`None` = the default material) → image index.
    materials: HashMap<Option<usize>, usize>,
}

impl Loader<'_> {
    fn add_node(&mut self, node: &gltf::Node, parent: Mat4) {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
        let buffers = self.buffers;
        if let Some(node_mesh) = node.mesh() {
            let name = node
                .name()
                .or(node_mesh.name())
                .map_or_else(|| format!("part {}", node.index() + 1), str::to_owned);
            for primitive in node_mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let mesh = &mut self.model.mesh;
                let start = mesh.indices.len() as u32;
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
                let colors: Option<Vec<[f32; 4]>> =
                    reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
                let color = |i: usize| {
                    colors
                        .as_ref()
                        .and_then(|c| c.get(i))
                        .copied()
                        .unwrap_or(WHITE)
                };
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
                                color: color(i),
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
                                    color: color(i as usize),
                                });
                            }
                        }
                    }
                }
                let end = mesh.indices.len() as u32;
                let image = self.material(&primitive.material());
                self.model.parts.push(Part {
                    indices: start..end,
                    image,
                    name: name.clone(),
                });
            }
        }
        for child in node.children() {
            self.add_node(&child, world);
        }
    }

    /// The image for a material, made on first use.
    fn material(&mut self, material: &gltf::Material) -> usize {
        if let Some(&image) = self.materials.get(&material.index()) {
            return image;
        }
        let pbr = material.pbr_metallic_roughness();
        if self.materials.is_empty() {
            self.model.surface = [pbr.roughness_factor(), pbr.metallic_factor()];
        }
        let [r, g, b, _] = pbr.base_color_factor();
        let image = pbr
            .base_color_texture()
            .and_then(|info| self.images.get(info.texture().source().index()))
            .and_then(|data| base_color_image(data, [r, g, b]))
            .unwrap_or_else(|| Image::solid([r, g, b]));
        let data = |texture: gltf::Texture| self.images.get(texture.source().index());
        let maps = SurfaceMaps {
            normal: material.normal_texture().and_then(|info| {
                let scale = info.scale();
                data_image(data(info.texture())?, |texel| {
                    if scale != 1.0 {
                        for c in &mut texel[..2] {
                            let v = (*c as f32 / 255.0 * 2.0 - 1.0) * scale;
                            *c = ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0).round() as u8;
                        }
                    }
                })
            }),
            metal_rough: pbr
                .metallic_roughness_texture()
                .and_then(|info| data_image(data(info.texture())?, |_| {})),
            occlusion: material.occlusion_texture().and_then(|info| {
                let strength = info.strength().clamp(0.0, 1.0);
                data_image(data(info.texture())?, |texel| {
                    let ao = 1.0 - strength * (1.0 - texel[0] as f32 / 255.0);
                    texel[0] = (ao * 255.0).round() as u8;
                })
            }),
        };
        self.model.images.push(image);
        self.model.maps.push(maps);
        let index = self.model.images.len() - 1;
        self.materials.insert(material.index(), index);
        index
    }
}

/// A glTF image as RGBA8 data, each texel passed through `edit`. `None`
/// for pixel formats we don't read (16-bit and float).
fn data_image(data: &gltf::image::Data, edit: impl Fn(&mut [u8])) -> Option<Image> {
    let mut pixels = rgba8(data)?;
    for texel in pixels.chunks_exact_mut(4) {
        edit(texel);
    }
    Some(Image::data(data.width, data.height, pixels))
}

/// A glTF image as sRGB RGBA8, multiplied by `factor` (linear RGB). `None`
/// for pixel formats we don't read (16-bit and float).
fn base_color_image(data: &gltf::image::Data, factor: [f32; 3]) -> Option<Image> {
    let mut pixels = rgba8(data)?;
    if factor != [1.0; 3] {
        for texel in pixels.chunks_exact_mut(4) {
            for c in 0..3 {
                texel[c] = linear_to_srgb(srgb_to_linear(texel[c]) * factor[c]);
            }
        }
    }
    Some(Image::new(data.width, data.height, pixels))
}

/// A glTF image's texels as RGBA8 (grey fills rgb, alpha defaults to
/// opaque). `None` for 16-bit and float formats.
fn rgba8(data: &gltf::image::Data) -> Option<Vec<u8>> {
    use gltf::image::Format;
    let channels = match data.format {
        Format::R8 => 1,
        Format::R8G8 => 2,
        Format::R8G8B8 => 3,
        Format::R8G8B8A8 => 4,
        other => {
            log::warn!("unsupported texture format {other:?}; leaving the texture out");
            return None;
        }
    };
    let mut pixels = Vec::with_capacity((data.width * data.height * 4) as usize);
    for texel in data.pixels.chunks_exact(channels) {
        let rgb = if channels < 3 {
            [texel[0]; 3]
        } else {
            [texel[0], texel[1], texel[2]]
        };
        let alpha = match channels {
            2 => texel[1],
            4 => texel[3],
            _ => 255,
        };
        pixels.extend(rgb);
        pixels.push(alpha);
    }
    Some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_sandbox_arch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let model = load_gltf(&root.join("models/arch.gltf")).expect("arch.gltf");
        // No materials: one part with a white texel.
        assert_eq!(model.parts.len(), 1);
        assert_eq!(model.images[0].mips[0], vec![255; 4]);
        let mesh = model.mesh;
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
    fn base_color_texture_is_multiplied_by_the_factor() {
        // One triangle; a 2×1 texture (red, grey) with a factor halving green.
        let gltf = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"TEXCOORD_0":1},"material":0}]}],"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1,0.5,1,1],"baseColorTexture":{"index":0}}}],"textures":[{"source":0}],"images":[{"uri":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAAD0lEQVR4nGP4z8DQ0NAAAAkBAoCZxGkyAAAAAElFTkSuQmCC"}],"buffers":[{"byteLength":60,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/"}],"bufferViews":[{"buffer":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":24}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},{"bufferView":1,"componentType":5126,"count":3,"type":"VEC2"}]}"#;
        let path = std::env::temp_dir().join("engine-textured-triangle.gltf");
        std::fs::write(&path, gltf).unwrap();
        let model = load_gltf(&path).unwrap();

        assert_eq!(
            model.parts,
            vec![Part {
                indices: 0..3,
                image: 0,
                name: "part 1".into(),
            }]
        );
        let image = &model.images[0];
        assert_eq!((image.width, image.height), (2, 1));
        let half_grey = linear_to_srgb(srgb_to_linear(128) * 0.5);
        assert_eq!(
            image.mips[0],
            vec![255, 0, 0, 255, 128, half_grey, 128, 255]
        );
        assert_eq!(model.mesh.vertices[1].uv, [1.0, 0.0]);
    }

    #[test]
    fn surface_maps_load_as_data_with_their_strengths() {
        // The same 2×1 texture (red, grey) as normal map at half strength,
        // roughness/metal, and occlusion at half strength.
        let gltf = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"TEXCOORD_0":1},"material":0}]}],"materials":[{"pbrMetallicRoughness":{"metallicRoughnessTexture":{"index":0}},"normalTexture":{"index":0,"scale":0.5},"occlusionTexture":{"index":0,"strength":0.5}}],"textures":[{"source":0}],"images":[{"uri":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAAD0lEQVR4nGP4z8DQ0NAAAAkBAoCZxGkyAAAAAElFTkSuQmCC"}],"buffers":[{"byteLength":60,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/"}],"bufferViews":[{"buffer":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":24}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},{"bufferView":1,"componentType":5126,"count":3,"type":"VEC2"}]}"#;
        let path = std::env::temp_dir().join("engine-mapped-triangle.gltf");
        std::fs::write(&path, gltf).unwrap();
        let model = load_gltf(&path).unwrap();
        let maps = &model.maps[0];

        let normal = maps.normal.as_ref().expect("normal map");
        assert!(normal.linear);
        // Red (x = +1, y = -1) bent halfway back toward flat; grey stays.
        assert_eq!(normal.mips[0], vec![191, 64, 0, 255, 128, 128, 128, 255]);

        let metal_rough = maps.metal_rough.as_ref().expect("roughness/metal map");
        assert_eq!(
            metal_rough.mips[0],
            vec![255, 0, 0, 255, 128, 128, 128, 255]
        );

        let occlusion = &maps.occlusion.as_ref().expect("occlusion map").mips[0];
        assert_eq!(occlusion[0], 255);
        assert!((191..=192).contains(&occlusion[4]), "{}", occlusion[4]);
    }

    #[test]
    fn models_without_maps_have_none() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let model = load_gltf(&root.join("models/arch.gltf")).unwrap();
        let maps = &model.maps[0];
        assert!(maps.normal.is_none() && maps.metal_rough.is_none() && maps.occlusion.is_none());
    }

    #[test]
    fn changed_file_reloads_once_it_settles() {
        let root = std::env::temp_dir().join("engine-hot-reload");
        std::fs::create_dir_all(root.join("models")).unwrap();
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let file = root.join("models/arch.gltf");
        std::fs::copy(sandbox.join("models/arch.gltf"), &file).unwrap();
        let touch = |seconds: u64| {
            let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(time)
                .unwrap();
        };
        touch(1_000);
        let mut assets = Assets::new(root.clone());
        let first = assets.model("models/arch.gltf").unwrap();
        assert!(assets.changed_models().is_empty(), "nothing changed");

        touch(2_000);
        assert!(assets.changed_models().is_empty(), "still being written?");
        assert_eq!(assets.changed_models(), vec!["models/arch.gltf"]);
        let second = assets.model("models/arch.gltf").unwrap();
        assert!(!Arc::ptr_eq(&first, &second), "loaded again");
        assert!(assets.changed_models().is_empty());
    }

    #[test]
    fn missing_model_is_none_not_a_panic() {
        let mut assets = Assets::new(PathBuf::from("nowhere"));
        assert!(assets.model("models/missing.gltf").is_none());
        // Cached: asking again doesn't retry or panic.
        assert!(assets.model("models/missing.gltf").is_none());
    }
}

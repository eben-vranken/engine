//! wgpu renderer. Each frame:
//!
//! 1. shadow passes: depth from the sun and from shadowed lights
//! 2. fog: froxel grid lit through the shadow maps (compute)
//! 3. scene pass: objects → HDR color + depth at internal resolution; the
//!    light that isn't direct comes from the lightmaps (static surfaces)
//!    or the probes (moving ones), see `lightmap`
//! 4. post: bloom, tone mapping, grading, grain, nearest-neighbor upscale
//!    → window
//! 5. overlay: egui at native resolution on top
//!
//! See `scale` for how the internal resolution is chosen.

mod bvh;
mod fog;
mod frustum;
pub mod grade;
pub mod lights;
mod live;
pub mod mesh;
mod post;
pub mod quality;
pub mod scale;
mod shadows;
pub mod texture;
pub mod timings;

use crate::assets::{Assets, Resolved};
use crate::camera::Camera;
use crate::lightmap::{self, AmbientCube, Baked, BakedDraw};
use crate::project::Retro;
use crate::scene::{Component, Environment, Geometry, LightSource, PlacedLight, Renderable, Scene};
use bytemuck::{Pod, Zeroable};
use fog::{FogFrame, VolumetricFog};
use frustum::Frustum;
use glam::{Mat4, Vec3, Vec4};
use lights::{GpuLight, MAX_FOG_LIGHTS, MAX_LIGHTS};
use live::{Live, Surfaces};
use mesh::{MeshData, Shape};
use post::Post;
use quality::Quality;
use scale::ScaleLayout;
use shadows::{Caster, Shadows};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;
use texture::Image;
use timings::GpuTimer;
use wgpu::util::DeviceExt;
use winit::window::Window;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Dust motes drawn at `dust: 1`.
const MAX_DUST: u32 = 12_000;
/// Most zones with a reflection cube at once.
const MAX_REFLECTIONS: usize = 16;
/// Mip levels of the reflection cubes: rougher surfaces read blurrier ones.
const REFLECTION_MIPS: u32 = lightmap::REFLECTION_SIZE.ilog2() + 1;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("could not create a surface for the window: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("no suitable GPU adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    #[error("could not open the GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("the surface supports no texture formats on this adapter")]
    NoSurfaceFormat,
}

/// Matches `Frame` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: Mat4,
    sun_view_proj: [Mat4; lights::CASCADES],
    /// The view last frame's live occlusion was traced for.
    occlusion_view_proj: Mat4,
    camera_pos: Vec4,
    sun_dir: Vec4,
    sun_color: Vec4,
    ambient_sky: Vec4,
    ambient_ground: Vec4,
    camera_forward: Vec4,
    params: Vec4,
    shadow: Vec4,
    screen: Vec4,
    /// Dust motes in the camera's zone: amount, size (px), brightness,
    /// speed.
    dust: Vec4,
    /// x: vertex snap grid lines (0 = off), y: 1 = affine textures,
    /// z: live occlusion strength (0 = off).
    retro: Vec4,
}

/// One placed draw's data, read by `instance_index` through the frame's
/// visible list: matches `Instance` in `scene.wgsl` (its start in
/// `shadow.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceData {
    model: Mat4,
    normal_matrix: Mat4,
    color: Vec4,
    highlight: Vec4,
    emissive: Vec4,
    /// x: 1 = lit from its lightmap, 0 = from `ambient`.
    lighting: Vec4,
    /// x: roughness, y: metalness, z: reflection cube index or -1.
    surface: Vec4,
    /// Light arriving from +X, -X, +Y, -Y, +Z, -Z (probes or the sky).
    ambient: [Vec4; 6],
    /// Lightmap UV = the mesh's layout UV * xy + zw (its atlas rectangle).
    lightmap_rect: Vec4,
}

/// One placed draw: an object's primitive, model, or model part.
struct Instance {
    /// Index into `Scene::objects`.
    object: usize,
    /// What the mesh was built from, to notice when it changes.
    geometry: GeometryKey,
    /// Index into `Renderer::meshes`, shared with other placements.
    mesh: usize,
    /// Index into `Renderer::materials` (primitives; models use their
    /// mesh's own).
    material: usize,
    /// Index into `Renderer::lightmaps`, if drawn with a lightmap.
    lightmap: Option<usize>,
    lightmap_rect: Vec4,
    /// Material texture path ("" = none), to notice when it changes.
    texture: String,
    /// The object is hidden (picked up, an open gate): kept, not drawn.
    hidden: bool,
    /// The object moves while the game runs (see `lightmap::moves`).
    moves: bool,
    /// World bounding sphere (center, radius), for culling.
    sphere: (Vec3, f32),
    data: InstanceData,
}

#[derive(Debug, Clone, PartialEq)]
enum GeometryKey {
    Shape(Shape),
    /// The model's path, and which of its parts.
    Model(String, String),
}

impl From<Geometry<'_>> for GeometryKey {
    fn from(geometry: Geometry) -> Self {
        match geometry {
            Geometry::Shape(shape) => GeometryKey::Shape(shape),
            Geometry::Model(path, parts) => {
                GeometryKey::Model(path.to_owned(), format!("{parts:?}"))
            }
        }
    }
}

/// How a draw's surface looks, apart from its texture.
#[derive(Debug, Clone, Copy)]
struct Look {
    /// Primitives tile their texture in meters; models use their own UVs.
    textured: bool,
    lightmapped: bool,
    ambient: AmbientCube,
    /// Roughness and metalness.
    surface: [f32; 2],
    /// Reflection cube index.
    reflection: Option<usize>,
    lightmap_rect: Vec4,
}

fn instance_data(r: &Renderable, highlighted: bool, look: &Look) -> InstanceData {
    let model = r.transform.matrix();
    // Texture repeats per meter of UV: primitives' UVs are in meters.
    let uv_scale = match (look.textured, r.material) {
        (false, _) => 1.0,
        (true, Some(m)) => 1.0 / m.meters.max(0.01),
        (true, None) => 1.0 / texture::GRID_METERS as f32,
    };
    let [roughness, metalness] = match r.material {
        Some(m) => [m.roughness, m.metalness],
        None => look.surface,
    };
    InstanceData {
        model,
        normal_matrix: model.inverse().transpose(),
        color: r.color.extend(1.0),
        highlight: Vec4::new(highlighted as u32 as f32, uv_scale, 0.0, 0.0),
        emissive: r.emissive.extend(0.0),
        lighting: Vec4::new(look.lightmapped as u32 as f32, 0.0, 0.0, 0.0),
        surface: Vec4::new(
            roughness.clamp(0.0, 1.0),
            metalness.clamp(0.0, 1.0),
            look.reflection.map_or(-1.0, |i| i as f32),
            0.0,
        ),
        ambient: look.ambient.map(|c| c.extend(0.0)),
        lightmap_rect: look.lightmap_rect,
    }
}

/// Sorts `items` (key, instance) by key and appends the instances to
/// `visible`, the frame's list: one range per key, drawn as one instanced
/// draw (`instance_index` runs over the range).
fn batch<K: Ord + Copy>(items: &mut [(K, u32)], visible: &mut Vec<u32>) -> Vec<(K, Range<u32>)> {
    items.sort_unstable_by_key(|(key, _)| *key);
    let mut batches: Vec<(K, Range<u32>)> = Vec::new();
    for &(key, instance) in items.iter() {
        let at = visible.len() as u32;
        visible.push(instance);
        match batches.last_mut() {
            Some((k, range)) if *k == key => range.end = at + 1,
            _ => batches.push((key, at..at + 1)),
        }
    }
    batches
}

/// Matches `Reflection` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuReflection {
    center: Vec4,
    min: Vec4,
    max: Vec4,
}

/// The environment's ambient light as a cube: sky above, ground below,
/// halfway at the sides.
fn sky_cube(env: &Environment) -> AmbientCube {
    let side = (env.ambient_sky + env.ambient_ground) * 0.5;
    [side, side, env.ambient_sky, env.ambient_ground, side, side]
}

/// A mesh on the GPU, shared by every placement of the same geometry (and
/// lightmap layout).
struct GpuMesh {
    /// Bounding sphere in the mesh's own space, for culling.
    sphere: (Vec3, f32),
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Lightmap layout UV per vertex (zeros without a lightmap).
    lightmap_uvs: wgpu::Buffer,
    index_count: u32,
    /// The triangles again on the CPU (corner positions, three indices
    /// each), for the live effects' ray tracing.
    positions: Vec<Vec3>,
    corners: Vec<u32>,
    /// Index ranges and their own material (a model's textures); `None`:
    /// the instance's (primitives: the grid or a material texture).
    parts: Vec<(Range<u32>, Option<wgpu::BindGroup>)>,
    /// A primitive: UVs in meters, tiled by the instance's material.
    textured: bool,
    /// The model's own roughness and metalness (primitives: matte).
    surface: [f32; 2],
}

/// Render targets at internal resolution; rebuilt when it changes.
struct Targets {
    /// What the post pass reads.
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// For the live effects; made when one is first on.
    surfaces: Option<Surfaces>,
}

/// A scene light as the renderer keeps it between frames.
struct SceneLight {
    object: usize,
    light: LightSource,
    position: Vec3,
    direction: Vec3,
}

fn scene_lights(scene: &Scene, assets: &mut Assets) -> Vec<SceneLight> {
    to_scene_lights(scene.lights(|path, part| assets.model(path)?.part_center(part)))
}

/// One object's lights (see `Scene::lights_of`).
fn object_lights(scene: &Scene, index: usize, assets: &mut Assets) -> Vec<SceneLight> {
    to_scene_lights(scene.lights_of(index, |path, part| assets.model(path)?.part_center(part)))
}

fn to_scene_lights(lights: Vec<PlacedLight>) -> Vec<SceneLight> {
    lights
        .into_iter()
        .map(|l| SceneLight {
            object: l.object,
            light: l.light.clone(),
            position: l.position,
            direction: l.direction,
        })
        .collect()
}

pub struct Renderer {
    instance: wgpu::Instance,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    adapter_name: String,
    /// Counts full `set_scene` uploads (a new or reloaded scene).
    uploads: u64,
    /// Which light (object) had each shadow slot last frame.
    shadow_owners: Vec<Option<usize>>,
    /// What the surface supports, to tell which mode `Auto*` picked.
    present_modes: Vec<wgpu::PresentMode>,

    /// `None` picks the factor from the window height.
    scale_override: Option<u32>,
    /// Part of the window the scene is drawn into (x, y, width, height in
    /// pixels); `None` = the whole window.
    viewport: Option<[u32; 4]>,
    /// Top-left of the viewport in the window.
    view_origin: [u32; 2],
    layout: ScaleLayout,
    targets: Targets,

    scene_pipeline: wgpu::RenderPipeline,
    /// The scene pipeline that also writes the surfaces the live effects
    /// read.
    surfaces_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    dust_pipeline: wgpu::RenderPipeline,
    /// Per zone: its own fog, if any (for the camera zone's dust).
    zone_fogs: Vec<Option<crate::scene::Fog>>,
    /// Group 3: a zone's lightmap and its sampler.
    lightmap_layout: wgpu::BindGroupLayout,
    lightmap_sampler: wgpu::Sampler,
    /// For draws without a lightmap.
    no_lightmap: wgpu::BindGroup,
    /// One per baked zone (`None` if it has no texels).
    lightmaps: Vec<Option<wgpu::BindGroup>>,
    baked: Option<Arc<Baked>>,
    reflection_texture: wgpu::Texture,
    reflection_view: wgpu::TextureView,
    reflection_sampler: wgpu::Sampler,
    reflection_buffer: wgpu::Buffer,
    /// Baked zone name → reflection cube index.
    reflections: HashMap<String, usize>,
    /// Primitives' materials: the grid first, then textures as used.
    materials: Vec<wgpu::BindGroup>,
    /// Texture path → index into `materials`.
    material_of: HashMap<String, usize>,
    /// Each model's materials (one per image), uploaded once per model.
    model_materials: HashMap<String, Vec<wgpu::BindGroup>>,
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    /// Group 2: base color, the point sampler, then normal,
    /// roughness/metal and occlusion maps.
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Stand-ins for missing maps: a flat normal, and white (full
    /// roughness/metal factor, no occlusion).
    flat_normal: wgpu::Texture,
    white_data: wgpu::Texture,
    /// Every instance's data, read by `instance_index` (storage).
    instance_buffer: wgpu::Buffer,
    /// This frame's instances per draw, in order (storage, rewritten every
    /// frame); grows as needed.
    visible_buffer: wgpu::Buffer,
    instance_bind_group: wgpu::BindGroup,
    /// One per geometry and lightmap layout, shared by their placements.
    meshes: Vec<GpuMesh>,
    /// (geometry, layout) → index into `meshes`, and whether the layout fit.
    mesh_of: HashMap<(String, Option<u32>), (usize, bool)>,
    /// One per placed draw: what it draws and its data.
    instances: Vec<Instance>,
    /// Scene object shown as selected.
    highlighted: Vec<usize>,
    environment: Environment,
    /// Last frame: draws in view, and all draws.
    drawn: [usize; 2],

    lights: Vec<SceneLight>,
    light_buffer: wgpu::Buffer,
    shadows: Shadows,
    fog: VolumetricFog,
    live: Live,
    /// The live effects' triangles need rebuilding: the ones that stay
    /// put, the ones that move.
    stale_triangles: [bool; 2],
    retro: Retro,
    post: Post,
    /// The camera's zone (index into `Scene::zones`), for its grade.
    zone: Option<usize>,
    started: Instant,
    last_frame: Instant,
    /// When the last swapchain image was handed out, and the time between
    /// the last two: the display's rhythm, steadier than the CPU's.
    acquired: Option<Instant>,
    acquire_interval: Option<f32>,
    quality: Quality,
    /// `None` if the GPU can't time its stages.
    timer: Option<GpuTimer>,
}

impl Renderer {
    pub fn new(window: Arc<Window>, quality: Quality) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async(window, quality))
    }

    async fn new_async(window: Arc<Window>, quality: Quality) -> Result<Self, RenderError> {
        let settings = quality.settings();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        log::info!("GPU: {adapter_name}");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("engine"),
                required_features: GpuTimer::features(&adapter),
                ..Default::default()
            })
            .await?;
        let timer = GpuTimer::new(&device, &queue);

        // egui and our post shader both write display-encoded values, so
        // prefer a non-sRGB surface; fall back to converting in the shader.
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or(RenderError::NoSurfaceFormat)?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        log::info!("present modes supported: {:?}", caps.present_modes);

        // Scene pass resources.
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let depth_texture = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1),
                storage(2),
                depth_texture(3, wgpu::TextureViewDimension::D2Array),
                depth_texture(4, wgpu::TextureViewDimension::D2Array),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::CubeArray,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(9),
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let lightmap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lightmap"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_lightmap = lightmap_bind_group(
            &device,
            &queue,
            &lightmap_layout,
            &lightmap_sampler,
            [1, 1],
            &[0],
        );
        // Every instance's data and the frame's list of which to draw, read
        // in the vertex shader with `instance_index` (needs vertex storage
        // buffers: Vulkan, DX12 and Metal have them).
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("instances"),
            entries: &[storage(0), storage(1)],
        });

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lights"),
            size: (size_of::<GpuLight>() * (MAX_LIGHTS + MAX_FOG_LIGHTS)) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadows = Shadows::new(
            &device,
            &object_layout,
            settings.sun_shadow,
            settings.point_shadow,
            settings.shadowed_lights,
        );
        // Linear filtering on a comparison sampler compares four texels
        // and blends the results: soft shadow edges for free.
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let fog = VolumetricFog::new(
            &device,
            &light_buffer,
            &shadows.face_buffer,
            &shadows.sun_view,
            &shadows.point_view,
            &shadow_sampler,
            &settings,
        );
        let fog_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fog"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let reflection_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflections"),
            size: wgpu::Extent3d {
                width: lightmap::REFLECTION_SIZE,
                height: lightmap::REFLECTION_SIZE,
                depth_or_array_layers: (6 * MAX_REFLECTIONS) as u32,
            },
            mip_level_count: REFLECTION_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgb9e5Ufloat,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let reflection_view = reflection_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            ..Default::default()
        });
        let reflection_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reflections"),
            size: (size_of::<GpuReflection>() * MAX_REFLECTIONS) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let reflection_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reflections"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let live = Live::new(&device, &settings);
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shadows.face_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadows.sun_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&shadows.point_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&fog.integrated_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&fog_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&reflection_view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: reflection_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&reflection_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&live.history_view),
                },
            ],
        });

        let grid = texture::upload(&device, &queue, &texture::grid_image());
        // Point sampling up close; blend between mips in the distance so the
        // grid doesn't shimmer.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("point"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let flat_normal = texture::upload(
            &device,
            &queue,
            &Image::data(1, 1, vec![128, 128, 255, 255]),
        );
        let white_data = texture::upload(&device, &queue, &Image::data(1, 1, vec![255; 4]));
        let grid_material = material_bind_group(
            &device,
            &material_layout,
            &sampler,
            [&grid, &flat_normal, &white_data, &white_data],
        );

        let instance_buffer =
            storage_buffer(&device, "instances", size_of::<InstanceData>() as u64);
        let visible_buffer = storage_buffer(&device, "visible instances", 4096);
        let instance_bind_group =
            instance_bind_group(&device, &object_layout, &instance_buffer, &visible_buffer);

        let scene_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/scene.wgsl"));
        let scene_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("scene"),
                bind_group_layouts: &[
                    Some(&frame_layout),
                    Some(&object_layout),
                    Some(&material_layout),
                    Some(&lightmap_layout),
                ],
                immediate_size: 0,
            });
        let scene_pipeline = |label, entry, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&scene_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<mesh::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 4 => Float32x4],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![3 => Float32x2],
                    }),
                ],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some(entry),
                targets,
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        })
        };
        let hdr: Option<wgpu::ColorTargetState> = Some(HDR_FORMAT.into());
        let surfaces_pipeline = scene_pipeline(
            "scene and surfaces",
            "fs_surfaces",
            &[hdr.clone(), hdr.clone(), hdr.clone()],
        );
        let scene_pipeline = scene_pipeline("scene", "fs_main", &[hdr]);

        // The background: sky color seen through the fog, drawn only where
        // nothing else was (at the far plane, depth test LessEqual).
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some("fs_sky"),
                targets: &[Some(HDR_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Dust motes: camera-facing squares, added onto the image where
        // they're in front of the scene (depth tested, not written).
        let dust_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dust"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_dust"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some("fs_dust"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let layout = ScaleLayout::new(
            [config.width, config.height],
            scale::factor_for_lines(config.height, settings.max_lines),
        );
        let targets = create_targets(&device, layout);
        let mut post = Post::new(&device, format);
        post.set_scene_view(&device, &targets.color, layout.internal);

        let shadow_owners = vec![None; shadows.slots()];
        Ok(Self {
            instance,
            window,
            surface,
            device,
            queue,
            config,
            adapter_name,
            scale_override: None,
            viewport: None,
            view_origin: [0, 0],
            layout,
            targets,
            scene_pipeline,
            surfaces_pipeline,
            sky_pipeline,
            dust_pipeline,
            zone_fogs: Vec::new(),
            lightmap_layout,
            lightmap_sampler,
            no_lightmap,
            lightmaps: Vec::new(),
            baked: None,
            reflection_texture,
            reflection_view,
            reflection_sampler,
            reflection_buffer,
            reflections: HashMap::new(),
            materials: vec![grid_material],
            material_of: HashMap::new(),
            model_materials: HashMap::new(),
            frame_buffer,
            frame_bind_group,
            object_layout,
            material_layout,
            sampler,
            flat_normal,
            white_data,
            instance_buffer,
            visible_buffer,
            instance_bind_group,
            meshes: Vec::new(),
            mesh_of: HashMap::new(),
            instances: Vec::new(),
            drawn: [0, 0],
            highlighted: Vec::new(),
            environment: Environment::default(),
            lights: Vec::new(),
            light_buffer,
            shadows,
            shadow_owners,
            uploads: 0,
            present_modes: caps.present_modes.clone(),
            fog,
            live,
            stale_triangles: [true; 2],
            retro: Retro::default(),
            post,
            zone: None,
            started: Instant::now(),
            last_frame: Instant::now(),
            acquired: None,
            acquire_interval: None,
            quality,
            timer,
        })
    }

    /// The scene's light bake (or none); takes effect at the next
    /// `set_scene`.
    pub fn set_lightmaps(&mut self, baked: Option<Arc<Baked>>) {
        self.lightmaps = baked
            .iter()
            .flat_map(|b| &b.zones)
            .map(|zone| {
                (zone.size[0] > 0 && zone.size[1] > 0).then(|| {
                    lightmap_bind_group(
                        &self.device,
                        &self.queue,
                        &self.lightmap_layout,
                        &self.lightmap_sampler,
                        zone.size,
                        &zone.texels,
                    )
                })
            })
            .collect();
        self.upload_reflections(baked.as_deref());
        self.baked = baked;
    }

    /// Puts every baked zone's reflection cube (with its blurrier mips) into
    /// the cube array, and their boxes into the reflection buffer.
    fn upload_reflections(&mut self, baked: Option<&Baked>) {
        self.reflections.clear();
        let mut gpu = Vec::new();
        let zones = baked.iter().flat_map(|b| &b.zones);
        for zone in zones
            .filter(|z| z.reflection.is_some())
            .take(MAX_REFLECTIONS)
        {
            let Some(r) = &zone.reflection else { continue };
            let index = gpu.len();
            let size = lightmap::REFLECTION_SIZE;
            let face = (size * size) as usize;
            for f in 0..6 {
                let mut texels: Vec<Vec3> = r.texels[f * face..(f + 1) * face]
                    .iter()
                    .map(|&t| lightmap::from_rgb9e5(t))
                    .collect();
                let mut side = size;
                for level in 0..REFLECTION_MIPS {
                    let encoded: Vec<u32> = texels.iter().map(|&c| lightmap::rgb9e5(c)).collect();
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.reflection_texture,
                            mip_level: level,
                            origin: wgpu::Origin3d {
                                x: 0,
                                y: 0,
                                z: (index * 6 + f) as u32,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytemuck::cast_slice(&encoded),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(side * 4),
                            rows_per_image: Some(side),
                        },
                        wgpu::Extent3d {
                            width: side,
                            height: side,
                            depth_or_array_layers: 1,
                        },
                    );
                    if side == 1 {
                        break;
                    }
                    // The next mip: each texel the average of four.
                    let half = side / 2;
                    let at = |x: u32, y: u32| texels[(y * side + x) as usize];
                    texels = (0..half * half)
                        .map(|i| {
                            let (x, y) = (i % half * 2, i / half * 2);
                            (at(x, y) + at(x + 1, y) + at(x, y + 1) + at(x + 1, y + 1)) * 0.25
                        })
                        .collect();
                    side = half;
                }
            }
            gpu.push(GpuReflection {
                center: r.center.extend(index as f32),
                min: r.min.extend(0.0),
                max: r.max.extend(0.0),
            });
            self.reflections.insert(zone.name.clone(), index);
        }
        if !gpu.is_empty() {
            self.queue
                .write_buffer(&self.reflection_buffer, 0, bytemuck::cast_slice(&gpu));
        }
    }

    /// The reflection cube for an object's zone, if baked.
    fn reflection_of(&self, scene: &Scene, object: usize) -> Option<usize> {
        self.reflections.get(&scene.objects[object].zone).copied()
    }

    /// Index into `materials` for a primitive's texture: the grid for ""
    /// or one that can't be loaded; uploaded on first use.
    fn material_index(&mut self, path: &str, assets: &mut Assets) -> usize {
        if path.is_empty() {
            return 0;
        }
        if let Some(&index) = self.material_of.get(path) {
            return index;
        }
        let index = match assets.texture(path) {
            Some(image) => {
                let texture = texture::upload(&self.device, &self.queue, &image);
                self.materials.push(material_bind_group(
                    &self.device,
                    &self.material_layout,
                    &self.sampler,
                    [
                        &texture,
                        &self.flat_normal,
                        &self.white_data,
                        &self.white_data,
                    ],
                ));
                self.materials.len() - 1
            }
            None => 0,
        };
        self.material_of.insert(path.to_owned(), index);
        index
    }

    /// A zone's average baked ambient light (from its probes), if baked.
    fn zone_light(&self, zone: &str) -> Option<Vec3> {
        let grid = &self.baked.as_ref()?.zone(zone)?.probes;
        let (sum, n) = grid
            .probes
            .iter()
            .flatten()
            .fold((Vec3::ZERO, 0), |(s, n), cube| {
                (s + cube.iter().sum::<Vec3>() / 6.0, n + 1)
            });
        (n > 0).then(|| sum / n as f32)
    }

    /// Light for an object without a lightmap: from the probes of its
    /// zone's bake around `center`, or else the environment's.
    fn ambient_at(&self, scene: &Scene, object: usize, center: Vec3) -> AmbientCube {
        self.baked
            .as_ref()
            .and_then(|b| b.zone(&scene.objects[object].zone))
            .and_then(|z| z.probes.sample(center))
            .unwrap_or_else(|| sky_cube(&scene.environment))
    }

    /// Uploads every object's mesh and transform, the lights and the
    /// zones' color grades.
    pub fn set_scene(&mut self, scene: &Scene, assets: &mut Assets) {
        crate::perf::time("SET SCENE", || self.upload_scene(scene, assets));
        self.uploads += 1;
    }

    /// How many full `set_scene` uploads there have been: a change means
    /// the scene may have been replaced.
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    fn upload_scene(&mut self, scene: &Scene, assets: &mut Assets) {
        self.environment = scene.environment.clone();
        self.zone_fogs = scene.zones.iter().map(|z| z.fog.clone()).collect();
        self.lights = scene_lights(scene, assets);
        self.shadows.invalidate();
        self.fog.set_regions(
            &self.queue,
            &fog::regions(scene, |zone| self.zone_light(zone)),
        );
        self.fog.reset();
        self.live.reset();
        self.stale_triangles = [true; 2];
        self.post
            .set_grades(&self.device, &self.queue, scene, assets.root());
        self.materials.truncate(1);
        self.material_of.clear();
        self.model_materials.clear();
        self.meshes.clear();
        self.mesh_of.clear();
        self.instances.clear();
        let baked = self.baked.clone();
        let by_key: HashMap<lightmap::DrawKey, &BakedDraw> = baked
            .iter()
            .flat_map(|b| &b.draws)
            .map(|d| (d.key, d))
            .collect();
        // Hidden objects too: showing one again is then `update_object`.
        // (`draw_keys` also has one key per draw of every object.)
        let keys = lightmap::draw_keys(scene);
        for (r, key) in scene.renderables_with_hidden().zip(keys) {
            // Only objects that stay put use their lightmap, and only a
            // layout made for this mesh (and shape of scale).
            let placed = by_key
                .get(&key)
                .copied()
                .filter(|_| !lightmap::moves(&scene.objects[r.object]))
                .filter(|d| matches!(self.lightmaps.get(d.zone as usize), Some(Some(_))));
            let layout = placed.and_then(|d| {
                let layout = baked.as_ref()?.layouts.get(d.layout as usize)?;
                let fits = layout.mesh == lightmap::mesh_key(r.geometry, r.transform.scale);
                fits.then_some((d.layout, layout))
            });
            let (mesh, fits) = self.mesh_for(r.geometry, assets, layout);
            let placed = placed.filter(|_| fits && layout.is_some());
            let texture = material_texture(&r);
            let material = self.material_index(&texture, assets);
            let instance = Instance {
                object: r.object,
                geometry: r.geometry.into(),
                mesh,
                material,
                lightmap: placed.map(|d| d.zone as usize),
                lightmap_rect: placed.map_or(Vec4::ZERO, |d| Vec4::from_array(d.rect)),
                texture,
                hidden: scene.objects[r.object].hidden,
                moves: lightmap::moves(&scene.objects[r.object]),
                sphere: (Vec3::ZERO, 0.0),
                data: InstanceData::zeroed(),
            };
            self.instances.push(instance);
            let slot = self.instances.len() - 1;
            self.refresh_instance(scene, slot, &r);
        }
        self.instance_buffer = storage_buffer(
            &self.device,
            "instances",
            (self.instances.len().max(1) * size_of::<InstanceData>()) as u64,
        );
        self.instance_bind_group = instance_bind_group(
            &self.device,
            &self.object_layout,
            &self.instance_buffer,
            &self.visible_buffer,
        );
        let data: Vec<InstanceData> = self.instances.iter().map(|i| i.data).collect();
        if !data.is_empty() {
            self.queue
                .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&data));
        }
        let mesh_bytes: u64 = self
            .meshes
            .iter()
            .map(|m| m.vertices.size() + m.indices.size() + m.lightmap_uvs.size())
            .sum();
        log::info!(
            "set scene: {} draws, {} meshes, mesh buffers {:.2} MB, object data {:.2} MB",
            self.instances.len(),
            self.meshes.len(),
            mesh_bytes as f64 / 1e6,
            (self.instance_buffer.size() + self.visible_buffer.size()) as f64 / 1e6,
        );
    }

    /// Recomputes an instance's data from its renderable: where it is, its
    /// bounds, how it looks. Doesn't upload it.
    fn refresh_instance(&mut self, scene: &Scene, slot: usize, r: &Renderable) {
        let instance = &self.instances[slot];
        let gpu = &self.meshes[instance.mesh];
        let sphere = frustum::world_sphere(&r.transform.matrix(), gpu.sphere);
        let look = Look {
            textured: gpu.textured,
            lightmapped: instance.lightmap.is_some(),
            ambient: self.ambient_at(scene, r.object, sphere.0),
            surface: gpu.surface,
            reflection: self.reflection_of(scene, r.object),
            lightmap_rect: instance.lightmap_rect,
        };
        let highlighted = self.highlighted.contains(&r.object);
        let data = instance_data(r, highlighted, &look);
        let instance = &mut self.instances[slot];
        instance.sphere = sphere;
        instance.hidden = scene.objects[r.object].hidden;
        instance.data = data;
    }

    /// Re-uploads one object after it was edited, moved, shown or hidden:
    /// its transform, color, visibility and lights, and its mesh if the
    /// geometry changed.
    pub fn update_object(&mut self, scene: &Scene, index: usize, assets: &mut Assets) {
        crate::perf::time("UPDATE OBJECT", || {
            self.reupload_object(scene, index, assets)
        });
    }

    fn reupload_object(&mut self, scene: &Scene, index: usize, assets: &mut Assets) {
        let object = &scene.objects[index];
        // The object's lights (its own and its parts'), as they are now.
        self.lights.retain(|l| l.object != index);
        self.lights.extend(object_lights(scene, index, assets));

        if object
            .components
            .iter()
            .any(|c| matches!(c, Component::ZoneVolume(_) | Component::FogVolume(_)))
        {
            self.fog.set_regions(
                &self.queue,
                &fog::regions(scene, |zone| self.zone_light(zone)),
            );
        }
        let renderables: Vec<_> = scene.renderables_of(index).collect();
        let slots: Vec<usize> = (0..self.instances.len())
            .filter(|&i| self.instances[i].object == index)
            .collect();
        if renderables.len() != slots.len() {
            // Components were added or removed: rebuild everything.
            self.set_scene(scene, assets);
            return;
        }
        let moves = lightmap::moves(object);
        for (&slot, r) in slots.iter().zip(renderables) {
            // Its triangles are somewhere else now, for the live effects.
            if self.instances[slot].moves != moves {
                self.instances[slot].moves = moves;
                self.stale_triangles = [true; 2];
            }
            self.stale_triangles[moves as usize] = true;
            let key = GeometryKey::from(r.geometry);
            if self.instances[slot].geometry != key {
                // Reshaped: its lightmap no longer fits until the next bake.
                let (mesh, _) = self.mesh_for(r.geometry, assets, None);
                let instance = &mut self.instances[slot];
                instance.mesh = mesh;
                instance.geometry = key;
                instance.lightmap = None;
                instance.lightmap_rect = Vec4::ZERO;
                self.shadows.invalidate();
            }
            let texture = material_texture(&r);
            if self.instances[slot].texture != texture {
                // Retextured: same mesh, it keeps its lightmap.
                self.instances[slot].material = self.material_index(&texture, assets);
                self.instances[slot].texture = texture;
            }
            self.refresh_instance(scene, slot, &r);
            self.write_instance(slot);
        }
    }

    /// Highlights the given scene objects (by index) as selected.
    pub fn set_highlight(&mut self, selected: &[usize]) {
        self.highlighted = selected.to_vec();
        for i in 0..self.instances.len() {
            let highlight = selected.contains(&self.instances[i].object) as u32 as f32;
            if self.instances[i].data.highlight.x != highlight {
                self.instances[i].data.highlight.x = highlight;
                self.write_instance(i);
            }
        }
    }

    /// Only the zones' settings changed (fog, grade): updates those without
    /// re-uploading every mesh.
    pub fn set_zones(&mut self, scene: &Scene, assets: &Assets) {
        self.environment = scene.environment.clone();
        self.zone_fogs = scene.zones.iter().map(|z| z.fog.clone()).collect();
        self.post
            .set_grades(&self.device, &self.queue, scene, assets.root());
        let regions = fog::regions(scene, |zone| self.zone_light(zone));
        self.fog.set_regions(&self.queue, &regions);
    }

    /// The game's retro looks (`game.ron`).
    pub fn set_retro(&mut self, retro: Retro) {
        self.retro = retro;
        let levels = if retro.dither {
            retro.dither_levels.clamp(2, 256)
        } else {
            0
        };
        self.post.set_dither(levels);
    }

    /// The zone the camera is in (index into `Scene::zones`), for its
    /// color grade.
    pub fn set_zone(&mut self, zone: Option<usize>) {
        self.zone = zone;
    }

    /// The GPU mesh for a geometry, shared by every placement of it: with a
    /// lightmap layout's split vertices and UVs, or plain. Uploaded on first
    /// use. Also says whether the layout fit (not if the mesh changed since
    /// the bake).
    fn mesh_for(
        &mut self,
        geometry: Geometry,
        assets: &mut Assets,
        layout: Option<(u32, &lightmap::BakedLayout)>,
    ) -> (usize, bool) {
        let key = (
            format!("{:?}", GeometryKey::from(geometry)),
            layout.map(|(i, _)| i),
        );
        if let Some(&found) = self.mesh_of.get(&key) {
            return found;
        }
        let path = match geometry {
            Geometry::Model(path, _) => Some(path.to_owned()),
            Geometry::Shape(_) => None,
        };
        let (mesh, fits) = self.create_mesh(
            assets.resolve(geometry),
            path.as_deref(),
            layout.map(|(_, l)| l),
        );
        self.meshes.push(mesh);
        let found = (self.meshes.len() - 1, fits);
        self.mesh_of.insert(key, found);
        found
    }

    /// A model's materials, one per image: its base color and glTF maps.
    fn model_materials(&self, model: &crate::assets::ModelData) -> Vec<wgpu::BindGroup> {
        let max = self.device.limits().max_texture_dimension_2d;
        let upload = |image: Option<&Image>| {
            let image = image?;
            if image.width.max(image.height) > max {
                log::warn!("texture larger than {max} px; leaving it out");
                return None;
            }
            Some(texture::upload(&self.device, &self.queue, image))
        };
        model
            .images
            .iter()
            .enumerate()
            .map(|(i, image)| {
                let maps = model.maps.get(i);
                let color = upload(Some(image)).unwrap_or_else(|| {
                    texture::upload(&self.device, &self.queue, &Image::solid([1.0; 3]))
                });
                let normal = upload(maps.and_then(|m| m.normal.as_ref()));
                let metal_rough = upload(maps.and_then(|m| m.metal_rough.as_ref()));
                let occlusion = upload(maps.and_then(|m| m.occlusion.as_ref()));
                material_bind_group(
                    &self.device,
                    &self.material_layout,
                    &self.sampler,
                    [
                        &color,
                        normal.as_ref().unwrap_or(&self.flat_normal),
                        metal_rough.as_ref().unwrap_or(&self.white_data),
                        occlusion.as_ref().unwrap_or(&self.white_data),
                    ],
                )
            })
            .collect()
    }

    /// Uploads a mesh, re-made for `layout` if it fits it (then it's drawn
    /// with its lightmap UVs); also says whether it did.
    fn create_mesh(
        &mut self,
        geometry: Resolved,
        path: Option<&str>,
        layout: Option<&lightmap::BakedLayout>,
    ) -> (GpuMesh, bool) {
        let shape_mesh;
        let surface = match &geometry {
            Resolved::Model(model) => model.surface,
            Resolved::Shape(_) => [1.0, 0.0],
        };
        let (data, parts, textured) = match &geometry {
            Resolved::Shape(shape) => {
                shape_mesh = MeshData::from_shape(*shape);
                let all = 0..shape_mesh.indices.len() as u32;
                (&shape_mesh, vec![(all, None)], true)
            }
            Resolved::Model(model) => {
                let cached = path.and_then(|p| self.model_materials.get(p)).cloned();
                let materials = cached.unwrap_or_else(|| {
                    let made = self.model_materials(model);
                    if let Some(p) = path {
                        self.model_materials.insert(p.to_owned(), made.clone());
                    }
                    made
                });
                let parts = model
                    .parts
                    .iter()
                    .map(|part| (part.indices.clone(), materials.get(part.image).cloned()))
                    .collect();
                (&model.mesh, parts, false)
            }
        };
        let remade = layout.and_then(|l| l.apply(data));
        let (data, uvs) = match &remade {
            Some((mesh, uvs)) => (mesh, uvs.clone()),
            None => (data, vec![[0.0; 2]; data.vertices.len()]),
        };
        let buffer = |label, contents: &[u8], usage| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                })
        };
        let mesh = GpuMesh {
            sphere: frustum::bounding_sphere(data.bounds()),
            vertices: buffer(
                "vertices",
                bytemuck::cast_slice(&data.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "indices",
                bytemuck::cast_slice(&data.indices),
                wgpu::BufferUsages::INDEX,
            ),
            lightmap_uvs: buffer(
                "lightmap uvs",
                bytemuck::cast_slice(&uvs),
                wgpu::BufferUsages::VERTEX,
            ),
            index_count: data.indices.len() as u32,
            positions: data.vertices.iter().map(|v| v.position.into()).collect(),
            corners: data.indices.clone(),
            parts,
            textured,
            surface,
        };
        (mesh, remade.is_some())
    }

    fn write_instance(&self, i: usize) {
        self.queue.write_buffer(
            &self.instance_buffer,
            (i * size_of::<InstanceData>()) as u64,
            bytemuck::bytes_of(&self.instances[i].data),
        );
    }

    /// Draws the scene into part of the window (x, y, width, height in
    /// pixels), e.g. between editor panels. `None` = the whole window.
    pub fn set_viewport(&mut self, viewport: Option<[u32; 4]>) {
        if viewport != self.viewport {
            self.viewport = viewport;
            self.update_layout();
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.update_layout();
    }

    /// `None` = automatic (internal height ~540–720 lines).
    pub fn set_render_scale(&mut self, factor: Option<u32>) {
        self.scale_override = factor.map(|f| f.clamp(1, scale::MAX_FACTOR));
        self.update_layout();
    }

    pub fn render_scale_override(&self) -> Option<u32> {
        self.scale_override
    }

    pub fn layout(&self) -> ScaleLayout {
        self.layout
    }

    /// Where the upscaled scene image sits in the window: x, y, width,
    /// height in pixels (inside the viewport, minus any letterbox).
    pub fn image_rect(&self) -> [f32; 4] {
        let [w, h] = self.layout.scaled();
        [
            (self.view_origin[0] + self.layout.origin[0]) as f32,
            (self.view_origin[1] + self.layout.origin[1]) as f32,
            w as f32,
            h as f32,
        ]
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Recent GPU milliseconds per stage (see `timings::STAGES`), if the
    /// GPU can measure them.
    pub fn gpu_times(&self) -> Option<[f32; timings::STAGES.len()]> {
        self.timer.as_ref().map(|t| t.stages)
    }

    /// Last frame: how many draws were in view, out of how many.
    pub fn drawn(&self) -> [usize; 2] {
        self.drawn
    }

    pub fn output_size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// The present mode in use: what wgpu resolves `AutoVsync` (FIFO
    /// relaxed, else FIFO) and `AutoNoVsync` (immediate, else mailbox, else
    /// FIFO) to on this surface.
    pub fn present_mode(&self) -> wgpu::PresentMode {
        use wgpu::PresentMode::*;
        let order: &[wgpu::PresentMode] = match self.config.present_mode {
            AutoVsync => &[FifoRelaxed, Fifo],
            AutoNoVsync => &[Immediate, Mailbox, Fifo],
            mode => return mode,
        };
        order
            .iter()
            .copied()
            .find(|m| self.present_modes.contains(m))
            .unwrap_or(Fifo)
    }

    /// Seconds between the last two swapchain images (a frame late).
    pub fn acquire_interval(&self) -> Option<f32> {
        self.acquire_interval
    }

    pub fn vsync(&self) -> bool {
        self.config.present_mode == wgpu::PresentMode::AutoVsync
    }

    pub fn set_vsync(&mut self, vsync: bool) {
        self.config.present_mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        // Reconfiguring the existing surface back to FIFO leaves NVIDIA's
        // Vulkan driver on Windows presenting to nowhere, so start fresh.
        // Assigning drops the old surface (and swapchain) before configure.
        match self.instance.create_surface(self.window.clone()) {
            Ok(surface) => self.surface = surface,
            Err(err) => log::error!("could not recreate the surface: {err}"),
        }
        self.surface.configure(&self.device, &self.config);
        log::info!("present mode: {:?}", self.present_mode());
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn update_layout(&mut self) {
        let [width, height] = [self.config.width, self.config.height];
        let [x, y, w, h] = self.viewport.unwrap_or([0, 0, width, height]);
        let x = x.min(width - 1);
        let y = y.min(height - 1);
        let size = [w.clamp(1, width - x), h.clamp(1, height - y)];
        self.view_origin = [x, y];
        let max_lines = self.quality.settings().max_lines;
        let factor = self
            .scale_override
            .unwrap_or_else(|| scale::factor_for_lines(size[1], max_lines));
        let layout = ScaleLayout::new(size, factor);
        if layout.internal != self.layout.internal {
            self.targets = create_targets(&self.device, layout);
            self.post
                .set_scene_view(&self.device, &self.targets.color, layout.internal);
        }
        self.layout = layout;
    }

    /// Draws one frame. `overlay` records extra passes (the UI) onto the
    /// window after the scene has been upscaled.
    pub fn render(
        &mut self,
        camera: &Camera,
        overlay: impl FnOnce(&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder, &wgpu::TextureView),
    ) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                // Use it this time, reconfigure for the next frame.
                crate::perf::count("SUBOPTIMAL", 1);
                self.surface.configure(&self.device, &self.config);
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                crate::perf::count("OUTDATED / LOST", 1);
                self.surface.configure(&self.device, &self.config);
                return;
            }
            other => {
                log::debug!("skipping frame: {other:?}");
                return;
            }
        };
        let acquired = Instant::now();
        self.acquire_interval = self.acquired.map(|t| (acquired - t).as_secs_f32());
        self.acquired = Some(acquired);
        let output = frame.texture.create_view(&Default::default());

        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.25);
        self.last_frame = now;
        let time = (now - self.started).as_secs_f32();
        self.post.update(&self.device, self.zone, dt);

        let env = &self.environment;
        let [w, h] = self.layout.internal;
        let view_proj = camera.view_projection(w as f32 / h as f32);
        let frustum = Frustum::new(view_proj);

        let frame_lights = lights::select(
            self.lights
                .iter()
                .map(|l| (l.object, &l.light, l.position, l.direction)),
            camera.position,
            time,
            &mut self.shadow_owners,
        );
        let mut gpu_lights: Vec<GpuLight> = frame_lights.iter().map(|l| l.gpu()).collect();
        let surface_lights = gpu_lights.len();
        // Emissive surfaces glow in the fog: the nearest become fog-only
        // lights after the real ones.
        let mut glowing: Vec<(f32, GpuLight)> = self
            .instances
            .iter()
            .filter(|i| !i.hidden && i.data.emissive.max_element() > 0.0)
            .map(|i| {
                let (center, radius) = i.sphere;
                let light = lights::emissive_light(i.data.emissive.truncate(), center, radius);
                (center.distance(camera.position), light)
            })
            .collect();
        glowing.sort_by(|a, b| a.0.total_cmp(&b.0));
        gpu_lights.extend(glowing.into_iter().take(MAX_FOG_LIGHTS).map(|(_, l)| l));
        if !gpu_lights.is_empty() {
            self.queue
                .write_buffer(&self.light_buffer, 0, bytemuck::cast_slice(&gpu_lights));
        }
        let sun_on = env.sun_color.max_element() > 0.0 && env.shadow_distance > 0.0;
        let radii = lights::cascade_radii(env.shadow_distance);
        let sun_view_proj = radii.map(|radius| {
            lights::sun_matrix(
                camera.position,
                env.sun_direction,
                radius,
                self.shadows.sun_size,
            )
        });
        // Dust follows the camera's zone (its own fog, or the environment's).
        let dust = self
            .zone
            .and_then(|z| self.zone_fogs.get(z)?.as_ref())
            .unwrap_or(&env.fog);
        let dust_count = (dust.dust.clamp(0.0, 1.0) * MAX_DUST as f32) as u32;
        // The live effects, when the scene asks for them: both read the
        // scene pass's surfaces.
        let occlusion = env.live_occlusion > 0.0;
        let reflections = env.screen_reflections;
        if (occlusion || reflections) && self.targets.surfaces.is_none() {
            self.targets.surfaces = Some(Surfaces::new(&self.device, self.layout.internal));
        }
        let surfaces = self
            .targets
            .surfaces
            .as_ref()
            .filter(|_| occlusion || reflections);
        if occlusion && self.stale_triangles != [false; 2] {
            let [still, moving] = self.stale_triangles;
            self.live.set_triangles(
                &self.device,
                &self.queue,
                still.then(|| triangles(&self.instances, &self.meshes, false)),
                moving.then(|| triangles(&self.instances, &self.meshes, true)),
            );
            self.stale_triangles = [false; 2];
        }
        self.live.begin(&self.queue, camera, view_proj, env);
        // Last frame's occlusion, if there was one.
        let traced = self.live.traced().filter(|_| occlusion);
        let uniform = FrameUniform {
            view_proj,
            sun_view_proj,
            occlusion_view_proj: traced.unwrap_or(view_proj),
            camera_pos: camera.position.extend(1.0),
            sun_dir: env
                .sun_direction
                .normalize_or(Vec3::NEG_Y)
                .extend(sun_on as u32 as f32),
            sun_color: env.sun_color.extend(0.0),
            ambient_sky: env.ambient_sky.extend(0.0),
            ambient_ground: env.ambient_ground.extend(0.0),
            camera_forward: camera
                .forward()
                .extend((env.fog_distance > 0.0) as u32 as f32),
            params: Vec4::new(
                1.0 / texture::GRID_METERS as f32,
                surface_lights as f32,
                time,
                self.shadows.sun_size as f32,
            ),
            shadow: Vec4::new(radii[0], radii[1], radii[2], 0.0),
            screen: Vec4::new(
                1.0 / w as f32,
                1.0 / h as f32,
                fog::NEAR,
                env.fog_distance.max(fog::NEAR * 2.0),
            ),
            dust: Vec4::new(
                dust.dust.clamp(0.0, 1.0),
                dust.dust_size.clamp(1.0, 8.0),
                dust.dust_brightness.max(0.0),
                dust.dust_speed,
            ),
            retro: Vec4::new(
                if self.retro.vertex_snap {
                    self.retro.snap_lines.clamp(16, 4096) as f32
                } else {
                    0.0
                },
                self.retro.affine as u32 as f32,
                traced.map_or(0.0, |_| env.live_occlusion.clamp(0.0, 1.0)),
                0.0,
            ),
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        let mark = |encoder: &mut wgpu::CommandEncoder, timer: &Option<GpuTimer>, i| {
            if let Some(t) = timer {
                t.mark(encoder, i);
            }
        };
        mark(&mut encoder, &self.timer, 0);
        let casters: Vec<Caster> = self
            .instances
            .iter()
            .enumerate()
            .filter(|(_, i)| !i.hidden)
            .map(|(n, i)| Caster {
                object: i.object,
                mesh: i.mesh,
                instance: n as u32,
                model: i.data.model,
                sphere: i.sphere,
                index_count: self.meshes[i.mesh].index_count,
            })
            .collect();
        // Every pass's instances go into one list for the frame: shadow
        // passes first, then the scene, one instanced draw per mesh (and
        // material and lightmap) in each.
        let mut visible: Vec<u32> = Vec::new();
        let shadow_plan = self.shadows.plan(
            &self.queue,
            &casters,
            sun_on.then_some(sun_view_proj),
            &frame_lights,
            &mut visible,
        );
        drop(casters);
        let mut items: Vec<((usize, usize, usize), u32)> = self
            .instances
            .iter()
            .enumerate()
            .filter(|(_, i)| !i.hidden && frustum.sees_sphere(i.sphere.0, i.sphere.1))
            .map(|(n, i)| {
                (
                    (i.mesh, i.material, i.lightmap.map_or(0, |z| z + 1)),
                    n as u32,
                )
            })
            .collect();
        let drawn = items.len();
        let batches = batch(&mut items, &mut visible);
        grow_visible(
            &self.device,
            &self.object_layout,
            &self.instance_buffer,
            (&mut self.visible_buffer, &mut self.instance_bind_group),
            visible.len(),
        );
        if !visible.is_empty() {
            self.queue
                .write_buffer(&self.visible_buffer, 0, bytemuck::cast_slice(&visible));
        }
        self.shadows.encode(
            &mut encoder,
            &shadow_plan,
            &self.instance_bind_group,
            &self.meshes,
        );
        mark(&mut encoder, &self.timer, 1);
        if env.fog_distance > 0.0 {
            self.fog.render(
                &self.queue,
                &mut encoder,
                FogFrame {
                    camera,
                    aspect: w as f32 / h as f32,
                    view_proj,
                    env,
                    sun_view_proj,
                    cascades: radii,
                    sun_on,
                    light_count: gpu_lights.len() as u32,
                    time,
                },
            );
        }

        mark(&mut encoder, &self.timer, 2);
        {
            let sky = env.ambient_sky.as_dvec3();
            let attachment = |view, load| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })
            };
            let clear_sky = wgpu::LoadOp::Clear(wgpu::Color {
                r: sky.x,
                g: sky.y,
                b: sky.z,
                a: 1.0,
            });
            let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
            // With reflections on, they are added between this image and
            // the one post reads.
            let color = match surfaces {
                Some(s) if reflections => &s.scene,
                _ => &self.targets.color,
            };
            let mut attachments = vec![attachment(color, clear_sky)];
            if let Some(s) = surfaces {
                attachments.push(attachment(&s.normal, clear));
                attachments.push(attachment(&s.reflectance, clear));
            }
            let depth = |load, store| {
                Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations { load, store }),
                    stencil_ops: None,
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &attachments,
                depth_stencil_attachment: depth(
                    wgpu::LoadOp::Clear(1.0),
                    if surfaces.is_some() {
                        wgpu::StoreOp::Store
                    } else {
                        wgpu::StoreOp::Discard
                    },
                ),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if surfaces.is_some() {
                &self.surfaces_pipeline
            } else {
                &self.scene_pipeline
            });
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            pass.set_bind_group(1, &self.instance_bind_group, &[]);
            let mut calls = 0;
            for ((mesh, material, lightmap), range) in &batches {
                let mesh = &self.meshes[*mesh];
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, mesh.lightmap_uvs.slice(..));
                let lightmap = lightmap
                    .checked_sub(1)
                    .and_then(|z| self.lightmaps.get(z)?.as_ref())
                    .unwrap_or(&self.no_lightmap);
                pass.set_bind_group(3, lightmap, &[]);
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                for (indices, own) in &mesh.parts {
                    let material = own.as_ref().unwrap_or(&self.materials[*material]);
                    pass.set_bind_group(2, material, &[]);
                    pass.draw_indexed(indices.clone(), 0, range.clone());
                    calls += 1;
                }
            }
            self.drawn = [drawn, self.instances.len()];
            crate::perf::count("DRAWN", drawn);
            crate::perf::count("DRAW CALLS", calls);
            if surfaces.is_some() {
                // The sky and dust only have a color: a pass of their own,
                // beside the surfaces.
                drop(pass);
                pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sky and dust"),
                    color_attachments: &[attachment(color, wgpu::LoadOp::Load)],
                    depth_stencil_attachment: depth(wgpu::LoadOp::Load, wgpu::StoreOp::Store),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            pass.draw(0..3, 0..1);
            if dust_count > 0 {
                pass.set_pipeline(&self.dust_pipeline);
                pass.draw(0..6, 0..dust_count);
            }
        }
        if let Some(surfaces) = surfaces {
            if occlusion {
                self.live
                    .occlude(&self.device, &mut encoder, &self.targets.depth, surfaces);
            }
            if reflections {
                self.live.reflect(
                    &self.device,
                    &mut encoder,
                    &self.targets.depth,
                    surfaces,
                    &self.targets.color,
                    &self.reflection_view,
                    &self.reflection_buffer,
                    &self.reflection_sampler,
                );
            }
        }
        self.live.end();

        mark(&mut encoder, &self.timer, 3);
        let x = (self.view_origin[0] + self.layout.origin[0]) as f32;
        let y = (self.view_origin[1] + self.layout.origin[1]) as f32;
        let [sw, sh] = self.layout.scaled().map(|v| v as f32);
        self.post.render(
            &self.queue,
            &mut encoder,
            &output,
            post::Output {
                origin: [x, y],
                factor: self.layout.factor as f32,
                linear: self.config.format.is_srgb(),
            },
            [x, y, sw, sh],
        );

        mark(&mut encoder, &self.timer, 4);
        if let Some(t) = &self.timer {
            t.resolve(&mut encoder);
        }
        // The CPU's share of drawing the scene (culling, shadows, encoding),
        // after the swapchain wait and before the editor's overlay.
        crate::perf::count("RENDER CPU US", acquired.elapsed().as_micros() as usize);

        overlay(&self.device, &self.queue, &mut encoder, &output);

        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        if let Some(t) = &mut self.timer {
            t.collect(&self.device);
        }
    }
}

/// Every shown instance's triangles in world space: the ones of objects
/// that move, or the ones that stay put.
fn triangles(instances: &[Instance], meshes: &[GpuMesh], moving: bool) -> Vec<[Vec3; 3]> {
    let mut triangles = Vec::new();
    for instance in instances.iter().filter(|i| !i.hidden && i.moves == moving) {
        let mesh = &meshes[instance.mesh];
        let corner = |i: u32| {
            instance
                .data
                .model
                .transform_point3(mesh.positions[i as usize])
        };
        triangles.extend(
            mesh.corners
                .chunks_exact(3)
                .map(|c| [corner(c[0]), corner(c[1]), corner(c[2])]),
        );
    }
    triangles
}

/// Group 2 for one material: `[base color, normal, roughness/metal,
/// occlusion]` and the sampler.
fn material_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    textures: [&wgpu::Texture; 4],
) -> wgpu::BindGroup {
    let views = textures.map(|t| t.create_view(&Default::default()));
    let view = |binding, view| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("material"),
        layout,
        entries: &[
            view(0, &views[0]),
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            view(2, &views[1]),
            view(3, &views[2]),
            view(4, &views[3]),
        ],
    })
}

/// A renderable's material texture path, or "" for none (models always
/// use their own textures).
fn material_texture(r: &Renderable) -> String {
    match (r.geometry, r.material) {
        (Geometry::Shape(_), Some(m)) => m.texture.clone(),
        _ => String::new(),
    }
}

/// A zone's lightmap texture (RGB9E5 texels) with its sampler.
fn lightmap_bind_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    size: [u32; 2],
    texels: &[u32],
) -> wgpu::BindGroup {
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lightmap"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgb9e5Ufloat,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 4),
            rows_per_image: Some(size[1]),
        },
        extent,
    );
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("lightmap"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// Makes room for `count` entries in the frame's visible list (a new
/// buffer, and the bind group with it).
fn grow_visible(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    instances: &wgpu::Buffer,
    (visible, group): (&mut wgpu::Buffer, &mut wgpu::BindGroup),
    count: usize,
) {
    let needed = (count.max(1) * 4) as u64;
    if visible.size() >= needed {
        return;
    }
    *visible = storage_buffer(device, "visible instances", needed.next_power_of_two());
    *group = instance_bind_group(device, layout, instances, visible);
}

/// A storage buffer the CPU writes to.
fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Group 1: every instance's data and the frame's visible list.
fn instance_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    instances: &wgpu::Buffer,
    visible: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("instances"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: instances.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: visible.as_entire_binding(),
            },
        ],
    })
}

fn create_targets(device: &wgpu::Device, layout: ScaleLayout) -> Targets {
    let size = wgpu::Extent3d {
        width: layout.internal[0],
        height: layout.internal[1],
        depth_or_array_layers: 1,
    };
    let target = |label, format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let color = target(
        "internal color",
        HDR_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let depth = target(
        "internal depth",
        DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    Targets {
        color,
        depth,
        surfaces: None,
    }
}

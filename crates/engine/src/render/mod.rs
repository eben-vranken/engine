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

mod fog;
mod frustum;
pub mod grade;
pub mod lights;
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
use crate::scene::{Component, Environment, Geometry, LightSource, Renderable, Scene};
use bytemuck::{Pod, Zeroable};
use fog::{FogFrame, VolumetricFog};
use frustum::Frustum;
use glam::{Mat4, Vec3, Vec4};
use lights::{GpuLight, MAX_FOG_LIGHTS, MAX_LIGHTS};
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
    camera_pos: Vec4,
    sun_dir: Vec4,
    sun_color: Vec4,
    ambient_sky: Vec4,
    ambient_ground: Vec4,
    camera_forward: Vec4,
    params: Vec4,
    shadow: Vec4,
    screen: Vec4,
}

/// Matches `Object` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectUniform {
    model: Mat4,
    normal_matrix: Mat4,
    color: Vec4,
    highlight: Vec4,
    emissive: Vec4,
    /// x: 1 = lit from its lightmap, 0 = from `ambient`.
    lighting: Vec4,
    /// Light arriving from +X, -X, +Y, -Y, +Z, -Z (probes or the sky).
    ambient: [Vec4; 6],
}

struct Draw {
    /// Index into `Scene::objects`.
    object: usize,
    /// What the mesh was built from, to notice when it changes.
    geometry: GeometryKey,
    /// Primitives tile the grid texture in meters; models use their own UVs.
    textured: bool,
    /// Index into `Renderer::lightmaps`, if drawn with a lightmap.
    lightmap: Option<usize>,
    uniform: ObjectUniform,
}

#[derive(Debug, Clone, PartialEq)]
enum GeometryKey {
    Shape(Shape),
    Model(String),
}

impl From<Geometry<'_>> for GeometryKey {
    fn from(geometry: Geometry) -> Self {
        match geometry {
            Geometry::Shape(shape) => GeometryKey::Shape(shape),
            Geometry::Model(path) => GeometryKey::Model(path.to_owned()),
        }
    }
}

/// `textured`: primitives tile the grid texture; models use their own UVs.
fn object_uniform(
    r: &Renderable,
    highlighted: bool,
    textured: bool,
    lightmapped: bool,
    ambient: AmbientCube,
) -> ObjectUniform {
    let model = r.transform.matrix();
    ObjectUniform {
        model,
        normal_matrix: model.inverse().transpose(),
        color: r.color.extend(1.0),
        highlight: Vec4::new(highlighted as u32 as f32, textured as u32 as f32, 0.0, 0.0),
        emissive: r.emissive.extend(0.0),
        lighting: Vec4::new(lightmapped as u32 as f32, 0.0, 0.0, 0.0),
        ambient: ambient.map(|c| c.extend(0.0)),
    }
}

/// The environment's ambient light as a cube: sky above, ground below,
/// halfway at the sides.
fn sky_cube(env: &Environment) -> AmbientCube {
    let side = (env.ambient_sky + env.ambient_ground) * 0.5;
    [side, side, env.ambient_sky, env.ambient_ground, side, side]
}

struct GpuMesh {
    /// Bounding sphere in the mesh's own space, for culling.
    sphere: (Vec3, f32),
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Lightmap UV per vertex (zeros without a lightmap).
    lightmap_uvs: wgpu::Buffer,
    index_count: u32,
    /// Index ranges and the texture each is drawn with.
    parts: Vec<(Range<u32>, wgpu::BindGroup)>,
}

/// Render targets at internal resolution; rebuilt when it changes.
struct Targets {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// A scene light as the renderer keeps it between frames.
struct SceneLight {
    object: usize,
    light: LightSource,
    position: Vec3,
}

fn scene_lights(scene: &Scene) -> Vec<SceneLight> {
    scene
        .lights()
        .map(|(object, light, position)| SceneLight {
            object,
            light: light.clone(),
            position,
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
    sky_pipeline: wgpu::RenderPipeline,
    /// Group 3: a zone's lightmap and its sampler.
    lightmap_layout: wgpu::BindGroupLayout,
    lightmap_sampler: wgpu::Sampler,
    /// For draws without a lightmap.
    no_lightmap: wgpu::BindGroup,
    /// One per baked zone (`None` if it has no texels).
    lightmaps: Vec<Option<wgpu::BindGroup>>,
    baked: Option<Arc<Baked>>,
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    /// Group 2: one texture and the point sampler.
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    grid_material: wgpu::BindGroup,
    object_stride: u64,
    object_buffer: wgpu::Buffer,
    object_bind_group: wgpu::BindGroup,
    meshes: Vec<GpuMesh>,
    /// One per mesh in `meshes`: which object it draws, and its data.
    draws: Vec<Draw>,
    /// Scene object shown as selected.
    highlighted: Option<usize>,
    environment: Environment,
    /// Last frame: draws in view, and all draws.
    drawn: [usize; 2],

    lights: Vec<SceneLight>,
    light_buffer: wgpu::Buffer,
    shadows: Shadows,
    fog: VolumetricFog,
    post: Post,
    /// The camera's zone (index into `Scene::zones`), for its grade.
    zone: Option<usize>,
    started: Instant,
    last_frame: Instant,
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

        // Scene pass resources.
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let depth_texture = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
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
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<ObjectUniform>() as u64),
                },
                count: None,
            }],
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
        let grid_material = material_bind_group(&device, &material_layout, &grid, &sampler);

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let object_stride = (size_of::<ObjectUniform>() as u64).next_multiple_of(alignment);
        let (object_buffer, object_bind_group) =
            create_object_buffer(&device, &object_layout, object_stride, 1);

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
        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene"),
            layout: Some(&scene_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<mesh::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
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
                entry_point: Some("fs_main"),
                targets: &[Some(HDR_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

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

        let layout = ScaleLayout::new(
            [config.width, config.height],
            scale::factor_for_lines(config.height, settings.max_lines),
        );
        let targets = create_targets(&device, layout);
        let mut post = Post::new(&device, format);
        post.set_scene_view(&device, &targets.color, layout.internal);

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
            sky_pipeline,
            lightmap_layout,
            lightmap_sampler,
            no_lightmap,
            lightmaps: Vec::new(),
            baked: None,
            frame_buffer,
            frame_bind_group,
            object_layout,
            material_layout,
            sampler,
            grid_material,
            object_stride,
            object_buffer,
            object_bind_group,
            meshes: Vec::new(),
            draws: Vec::new(),
            drawn: [0, 0],
            highlighted: None,
            environment: Environment::default(),
            lights: Vec::new(),
            light_buffer,
            shadows,
            fog,
            post,
            zone: None,
            started: Instant::now(),
            last_frame: Instant::now(),
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
        self.baked = baked;
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
        self.environment = scene.environment.clone();
        self.lights = scene_lights(scene);
        self.shadows.invalidate();
        self.fog.set_regions(
            &self.queue,
            &fog::regions(scene, |zone| self.zone_light(zone)),
        );
        self.fog.reset();
        self.post
            .set_grades(&self.device, &self.queue, scene, assets.root());
        let renderables: Vec<_> = scene.renderables().collect();
        let (buffer, bind_group) = create_object_buffer(
            &self.device,
            &self.object_layout,
            self.object_stride,
            renderables.len().max(1),
        );
        self.object_buffer = buffer;
        self.object_bind_group = bind_group;
        self.meshes.clear();
        self.draws.clear();
        let baked = self.baked.clone();
        let by_key: HashMap<&lightmap::DrawKey, &BakedDraw> = baked
            .iter()
            .flat_map(|b| &b.draws)
            .map(|d| (&d.key, d))
            .collect();
        let keys = lightmap::draw_keys(scene);
        for (r, key) in renderables.into_iter().zip(&keys) {
            // Only objects that stay put use their lightmap.
            let baked_draw = by_key
                .get(key)
                .copied()
                .filter(|_| !lightmap::moves(&scene.objects[r.object]))
                .filter(|d| matches!(self.lightmaps.get(d.zone as usize), Some(Some(_))));
            let (mesh, textured, lightmapped) =
                self.create_mesh(assets.resolve(r.geometry), baked_draw);
            let lightmap = lightmapped
                .then(|| baked_draw.map(|d| d.zone as usize))
                .flatten();
            let center = frustum::world_sphere(&r.transform.matrix(), mesh.sphere).0;
            let ambient = self.ambient_at(scene, r.object, center);
            self.meshes.push(mesh);
            let highlighted = Some(r.object) == self.highlighted;
            self.draws.push(Draw {
                object: r.object,
                geometry: r.geometry.into(),
                textured,
                lightmap,
                uniform: object_uniform(&r, highlighted, textured, lightmap.is_some(), ambient),
            });
        }
        for i in 0..self.draws.len() {
            self.write_draw(i);
        }
    }

    /// Re-uploads one object after it was edited or moved: its transform,
    /// color and lights, and its mesh if the geometry changed.
    pub fn update_object(&mut self, scene: &Scene, index: usize, assets: &mut Assets) {
        let object = &scene.objects[index];
        let lights: Vec<&LightSource> = object
            .components
            .iter()
            .filter_map(|c| match c {
                Component::LightSource(l) => Some(l),
                _ => None,
            })
            .collect();
        if lights.len() != self.lights.iter().filter(|l| l.object == index).count() {
            // A light was added or removed.
            self.lights = scene_lights(scene);
        } else {
            let kept = self.lights.iter_mut().filter(|l| l.object == index);
            for (kept, &light) in kept.zip(&lights) {
                kept.light = light.clone();
                kept.position = object.transform.position;
            }
        }

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
        let renderables: Vec<_> = scene.renderables().filter(|r| r.object == index).collect();
        let slots: Vec<usize> = (0..self.draws.len())
            .filter(|&i| self.draws[i].object == index)
            .collect();
        if renderables.len() != slots.len() {
            // Components were added or removed: rebuild everything.
            self.set_scene(scene, assets);
            return;
        }
        let highlighted = Some(index) == self.highlighted;
        for (&slot, r) in slots.iter().zip(renderables) {
            let key = GeometryKey::from(r.geometry);
            if self.draws[slot].geometry != key {
                // Reshaped: its lightmap no longer fits until the next bake.
                let (mesh, textured, _) = self.create_mesh(assets.resolve(r.geometry), None);
                self.meshes[slot] = mesh;
                self.draws[slot].geometry = key;
                self.draws[slot].textured = textured;
                self.draws[slot].lightmap = None;
                self.shadows.invalidate();
            }
            let textured = self.draws[slot].textured;
            let lightmapped = self.draws[slot].lightmap.is_some();
            let center = frustum::world_sphere(&r.transform.matrix(), self.meshes[slot].sphere).0;
            let ambient = self.ambient_at(scene, index, center);
            self.draws[slot].uniform =
                object_uniform(&r, highlighted, textured, lightmapped, ambient);
            self.write_draw(slot);
        }
    }

    /// Highlights the given scene object (by index) as selected.
    pub fn set_highlight(&mut self, selected: Option<usize>) {
        self.highlighted = selected;
        for i in 0..self.draws.len() {
            let highlight = (Some(self.draws[i].object) == selected) as u32 as f32;
            if self.draws[i].uniform.highlight.x != highlight {
                self.draws[i].uniform.highlight.x = highlight;
                self.write_draw(i);
            }
        }
    }

    /// Only the zones' settings changed (fog, grade): updates those without
    /// re-uploading every mesh.
    pub fn set_zones(&mut self, scene: &Scene, assets: &Assets) {
        self.environment = scene.environment.clone();
        self.post
            .set_grades(&self.device, &self.queue, scene, assets.root());
        let regions = fog::regions(scene, |zone| self.zone_light(zone));
        self.fog.set_regions(&self.queue, &regions);
    }

    /// The zone the camera is in (index into `Scene::zones`), for its
    /// color grade.
    pub fn set_zone(&mut self, zone: Option<usize>) {
        self.zone = zone;
    }

    /// Uploads a mesh; also says whether it gets the grid texture and
    /// whether `lightmap` fit it (then it's drawn with its lightmap UVs).
    fn create_mesh(
        &self,
        geometry: Resolved,
        lightmap: Option<&BakedDraw>,
    ) -> (GpuMesh, bool, bool) {
        let shape_mesh;
        let (data, parts, textured) = match &geometry {
            Resolved::Shape(shape) => {
                shape_mesh = MeshData::from_shape(*shape);
                let all = 0..shape_mesh.indices.len() as u32;
                (&shape_mesh, vec![(all, self.grid_material.clone())], true)
            }
            Resolved::Model(model) => {
                let max = self.device.limits().max_texture_dimension_2d;
                let materials: Vec<wgpu::BindGroup> = model
                    .images
                    .iter()
                    .map(|image| {
                        let fallback;
                        let image = if image.width.max(image.height) > max {
                            log::warn!("texture larger than {max} px; using white");
                            fallback = Image::solid([1.0; 3]);
                            &fallback
                        } else {
                            image
                        };
                        let texture = texture::upload(&self.device, &self.queue, image);
                        material_bind_group(
                            &self.device,
                            &self.material_layout,
                            &texture,
                            &self.sampler,
                        )
                    })
                    .collect();
                let parts = model
                    .parts
                    .iter()
                    .map(|part| (part.indices.clone(), materials[part.image].clone()))
                    .collect();
                (&model.mesh, parts, false)
            }
        };
        let remade = lightmap.and_then(|l| l.apply(data));
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
            parts,
        };
        (mesh, textured, remade.is_some())
    }

    fn write_draw(&self, i: usize) {
        self.queue.write_buffer(
            &self.object_buffer,
            i as u64 * self.object_stride,
            bytemuck::bytes_of(&self.draws[i].uniform),
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
                self.surface.configure(&self.device, &self.config);
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            other => {
                log::debug!("skipping frame: {other:?}");
                return;
            }
        };
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
            self.lights.iter().map(|l| (l.object, &l.light, l.position)),
            camera.position,
            time,
        );
        let mut gpu_lights: Vec<GpuLight> = frame_lights.iter().map(|l| l.gpu()).collect();
        let surface_lights = gpu_lights.len();
        // Emissive surfaces glow in the fog: the nearest become fog-only
        // lights after the real ones.
        let mut glowing: Vec<(f32, GpuLight)> = self
            .meshes
            .iter()
            .zip(&self.draws)
            .filter(|(_, d)| d.uniform.emissive.max_element() > 0.0)
            .map(|(mesh, d)| {
                let (center, radius) = frustum::world_sphere(&d.uniform.model, mesh.sphere);
                let light = lights::emissive_light(d.uniform.emissive.truncate(), center, radius);
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
        let uniform = FrameUniform {
            view_proj,
            sun_view_proj,
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
            .meshes
            .iter()
            .zip(&self.draws)
            .enumerate()
            .map(|(i, (mesh, draw))| Caster {
                object: draw.object,
                model: draw.uniform.model,
                sphere: frustum::world_sphere(&draw.uniform.model, mesh.sphere),
                vertices: &mesh.vertices,
                indices: &mesh.indices,
                index_count: mesh.index_count,
                offset: (i as u64 * self.object_stride) as u32,
            })
            .collect();
        self.shadows.render(
            &self.queue,
            &mut encoder,
            &self.object_bind_group,
            &casters,
            sun_on.then_some(sun_view_proj),
            &frame_lights,
        );
        drop(casters);
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
                },
            );
        }

        mark(&mut encoder, &self.timer, 2);
        {
            let sky = env.ambient_sky.as_dvec3();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: sky.x,
                            g: sky.y,
                            b: sky.z,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.scene_pipeline);
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            let mut drawn = 0;
            for (i, mesh) in self.meshes.iter().enumerate() {
                let (center, radius) =
                    frustum::world_sphere(&self.draws[i].uniform.model, mesh.sphere);
                if !frustum.sees_sphere(center, radius) {
                    continue;
                }
                drawn += 1;
                let offset = (i as u64 * self.object_stride) as u32;
                pass.set_bind_group(1, &self.object_bind_group, &[offset]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, mesh.lightmap_uvs.slice(..));
                let lightmap = self.draws[i]
                    .lightmap
                    .and_then(|z| self.lightmaps.get(z)?.as_ref())
                    .unwrap_or(&self.no_lightmap);
                pass.set_bind_group(3, lightmap, &[]);
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                for (indices, material) in &mesh.parts {
                    pass.set_bind_group(2, material, &[]);
                    pass.draw_indexed(indices.clone(), 0, 0..1);
                }
            }
            self.drawn = [drawn, self.meshes.len()];
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

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

        overlay(&self.device, &self.queue, &mut encoder, &output);

        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        if let Some(t) = &mut self.timer {
            t.collect(&self.device);
        }
    }
}

fn material_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("material"),
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

fn create_object_buffer(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    stride: u64,
    count: usize,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("objects"),
        size: stride * count as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("objects"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size_of::<ObjectUniform>() as u64),
            }),
        }],
    });
    (buffer, bind_group)
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
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    Targets { color, depth }
}

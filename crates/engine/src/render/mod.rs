//! wgpu renderer. Each frame:
//!
//! 1. scene pass: objects → HDR color + depth at internal resolution
//! 2. upscale pass: nearest-neighbor blow-up + tone mapping → window
//! 3. overlay: egui at native resolution on top
//!
//! See `scale` for how the internal resolution is chosen.

pub mod mesh;
pub mod scale;
pub mod texture;

use crate::camera::Camera;
use crate::scene::{Environment, Scene};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec4};
use mesh::MeshData;
use scale::ScaleLayout;
use std::sync::Arc;
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: Mat4,
    camera_pos: Vec4,
    sun_dir: Vec4,
    sun_color: Vec4,
    ambient_sky: Vec4,
    ambient_ground: Vec4,
    fog: Vec4,
    params: Vec4,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectUniform {
    model: Mat4,
    normal_matrix: Mat4,
    color: Vec4,
    highlight: Vec4,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UpscaleParams {
    origin: [f32; 2],
    factor: f32,
    linear_output: u32,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// Render targets at internal resolution; rebuilt when it changes.
struct Targets {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    upscale_bind_group: wgpu::BindGroup,
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
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    object_stride: u64,
    object_buffer: wgpu::Buffer,
    object_bind_group: wgpu::BindGroup,
    meshes: Vec<GpuMesh>,
    /// Per draw: the scene object it belongs to, and its uniform data.
    draws: Vec<(usize, ObjectUniform)>,
    environment: Environment,

    upscale_pipeline: wgpu::RenderPipeline,
    upscale_layout: wgpu::BindGroupLayout,
    upscale_params: wgpu::Buffer,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Result<Self, RenderError> {
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
                ..Default::default()
            })
            .await?;

        // egui and our upscale shader both write display-encoded values, so
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
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
                    resource: wgpu::BindingResource::TextureView(
                        &grid.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let object_stride = (size_of::<ObjectUniform>() as u64).next_multiple_of(alignment);
        let (object_buffer, object_bind_group) =
            create_object_buffer(&device, &object_layout, object_stride, 1);

        let scene_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/scene.wgsl"));
        let scene_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("scene"),
                bind_group_layouts: &[Some(&frame_layout), Some(&object_layout)],
                immediate_size: 0,
            });
        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene"),
            layout: Some(&scene_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<mesh::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                })],
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

        // Upscale pass resources.
        let upscale_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("upscale"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let upscale_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("upscale params"),
            size: size_of::<UpscaleParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let upscale_shader =
            device.create_shader_module(wgpu::include_wgsl!("shaders/upscale.wgsl"));
        let upscale_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("upscale"),
                bind_group_layouts: &[Some(&upscale_layout)],
                immediate_size: 0,
            });
        let upscale_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("upscale"),
            layout: Some(&upscale_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &upscale_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &upscale_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let layout = ScaleLayout::new(
            [config.width, config.height],
            scale::default_factor(config.height),
        );
        let targets = create_targets(&device, &upscale_layout, &upscale_params, layout);

        let renderer = Self {
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
            frame_buffer,
            frame_bind_group,
            object_layout,
            object_stride,
            object_buffer,
            object_bind_group,
            meshes: Vec::new(),
            draws: Vec::new(),
            environment: Environment::default(),
            upscale_pipeline,
            upscale_layout,
            upscale_params,
        };
        renderer.write_upscale_params();
        Ok(renderer)
    }

    /// Uploads every object's mesh and transform. Objects are static for now.
    pub fn set_scene(&mut self, scene: &Scene) {
        self.environment = scene.environment.clone();
        let primitives: Vec<_> = scene.primitives().collect();
        self.meshes = primitives
            .iter()
            .map(|(_, primitive, _)| {
                let data = MeshData::from_shape(primitive.shape);
                GpuMesh {
                    vertices: self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("vertices"),
                            contents: bytemuck::cast_slice(&data.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                    indices: self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("indices"),
                            contents: bytemuck::cast_slice(&data.indices),
                            usage: wgpu::BufferUsages::INDEX,
                        }),
                    index_count: data.indices.len() as u32,
                }
            })
            .collect();

        let (buffer, bind_group) = create_object_buffer(
            &self.device,
            &self.object_layout,
            self.object_stride,
            primitives.len().max(1),
        );
        self.draws = primitives
            .iter()
            .map(|(object, primitive, transform)| {
                let model = transform.matrix();
                let uniform = ObjectUniform {
                    model,
                    normal_matrix: model.inverse().transpose(),
                    color: primitive.color.extend(1.0),
                    highlight: Vec4::ZERO,
                };
                (*object, uniform)
            })
            .collect();
        for (i, (_, uniform)) in self.draws.iter().enumerate() {
            self.queue.write_buffer(
                &buffer,
                i as u64 * self.object_stride,
                bytemuck::bytes_of(uniform),
            );
        }
        self.object_buffer = buffer;
        self.object_bind_group = bind_group;
    }

    /// Highlights the given scene object (by index) as selected.
    pub fn set_highlight(&mut self, selected: Option<usize>) {
        for (i, (object, uniform)) in self.draws.iter_mut().enumerate() {
            let highlight = (Some(*object) == selected) as u32 as f32;
            if uniform.highlight.x != highlight {
                uniform.highlight.x = highlight;
                self.queue.write_buffer(
                    &self.object_buffer,
                    i as u64 * self.object_stride,
                    bytemuck::bytes_of(uniform),
                );
            }
        }
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
        let factor = self
            .scale_override
            .unwrap_or_else(|| scale::default_factor(size[1]));
        let layout = ScaleLayout::new(size, factor);
        if layout.internal != self.layout.internal {
            self.targets = create_targets(
                &self.device,
                &self.upscale_layout,
                &self.upscale_params,
                layout,
            );
        }
        self.layout = layout;
        self.write_upscale_params();
    }

    fn write_upscale_params(&self) {
        let params = UpscaleParams {
            origin: [
                (self.view_origin[0] + self.layout.origin[0]) as f32,
                (self.view_origin[1] + self.layout.origin[1]) as f32,
            ],
            factor: self.layout.factor as f32,
            linear_output: self.config.format.is_srgb() as u32,
        };
        self.queue
            .write_buffer(&self.upscale_params, 0, bytemuck::bytes_of(&params));
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

        let env = &self.environment;
        let [w, h] = self.layout.internal;
        let uniform = FrameUniform {
            view_proj: camera.view_projection(w as f32 / h as f32),
            camera_pos: camera.position.extend(1.0),
            sun_dir: env.sun_direction.normalize().extend(0.0),
            sun_color: env.sun_color.extend(0.0),
            ambient_sky: env.ambient_sky.extend(0.0),
            ambient_ground: env.ambient_ground.extend(0.0),
            fog: env.fog_color.extend(env.fog_density),
            params: Vec4::new(1.0 / texture::GRID_METERS as f32, 0.0, 0.0, 0.0),
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        {
            let fog = env.fog_color.as_dvec3();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: fog.x,
                            g: fog.y,
                            b: fog.z,
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
            for (i, mesh) in self.meshes.iter().enumerate() {
                let offset = (i as u64 * self.object_stride) as u32;
                pass.set_bind_group(1, &self.object_bind_group, &[offset]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("upscale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let x = (self.view_origin[0] + self.layout.origin[0]) as f32;
            let y = (self.view_origin[1] + self.layout.origin[1]) as f32;
            let [sw, sh] = self.layout.scaled().map(|v| v as f32);
            pass.set_viewport(x, y, sw, sh, 0.0, 1.0);
            pass.set_pipeline(&self.upscale_pipeline);
            pass.set_bind_group(0, &self.targets.upscale_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        overlay(&self.device, &self.queue, &mut encoder, &output);

        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
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

fn create_targets(
    device: &wgpu::Device,
    upscale_layout: &wgpu::BindGroupLayout,
    upscale_params: &wgpu::Buffer,
    layout: ScaleLayout,
) -> Targets {
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
    let upscale_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("upscale"),
        layout: upscale_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&color),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: upscale_params.as_entire_binding(),
            },
        ],
    });
    Targets {
        color,
        depth,
        upscale_bind_group,
    }
}

//! Real-time shadow maps: depth images rendered from the lights' point of
//! view. A surface is in shadow where something nearer to the light was
//! drawn into the map.
//!
//! - The sun: three cascades (orthographic maps of growing size) following
//!   the camera.
//! - Point lights: six faces each (see `lights`), re-rendered only when the
//!   light or something within its range moved, so static lights in a
//!   static room cost nothing after the first frame.

use super::frustum::Frustum;
use super::lights::{self, CASCADES, FrameLight, MAX_SHADOWED};
use glam::{Mat4, Vec3};
use std::hash::{DefaultHasher, Hash, Hasher};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Pass matrices: the sun's cascades first, then six per shadow slot.
const PASSES: usize = CASCADES + MAX_SHADOWED * 6;

/// Something that casts shadows, as the frame sees it.
pub struct Caster<'a> {
    pub object: usize,
    pub model: Mat4,
    /// World-space bounding sphere.
    pub sphere: (Vec3, f32),
    pub vertices: &'a wgpu::Buffer,
    pub indices: &'a wgpu::Buffer,
    pub index_count: u32,
    /// Dynamic offset of its object uniform.
    pub offset: u32,
}

pub struct Shadows {
    /// Texels per side of each sun cascade.
    pub sun_size: u32,
    /// All sun cascades, as one array for the scene shader.
    pub sun_view: wgpu::TextureView,
    /// One view per cascade, to render into.
    cascade_views: Vec<wgpu::TextureView>,
    /// All point-light faces, as one array for the scene shader.
    pub point_view: wgpu::TextureView,
    /// One view per point-light face, to render into.
    face_views: Vec<wgpu::TextureView>,
    /// Face matrices for the scene shader (storage buffer).
    pub face_buffer: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    pass_buffer: wgpu::Buffer,
    pass_bind_group: wgpu::BindGroup,
    pass_stride: u64,
    /// Per slot: what its six faces were last rendered for.
    cache: [Option<u64>; MAX_SHADOWED],
}

impl Shadows {
    pub fn new(
        device: &wgpu::Device,
        object_layout: &wgpu::BindGroupLayout,
        sun_size: u32,
        point_size: u32,
    ) -> Self {
        let texture = |label, size, layers| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let sun = texture("sun shadow", sun_size, CASCADES as u32);
        let sun_view = sun.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let cascade_views = (0..CASCADES as u32)
            .map(|layer| {
                sun.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let layers = (MAX_SHADOWED * 6) as u32;
        let point = texture("point shadows", point_size, layers);
        let point_view = point.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let face_views = (0..layers)
            .map(|layer| {
                point.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let face_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow faces"),
            size: (size_of::<Mat4>() * MAX_SHADOWED * 6) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let pass_stride = (size_of::<Mat4>() as u64).next_multiple_of(alignment);
        let pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow passes"),
            size: pass_stride * PASSES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow pass"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Mat4>() as u64),
                },
                count: None,
            }],
        });
        let pass_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow pass"),
            layout: &pass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &pass_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<Mat4>() as u64),
                }),
            }],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/shadow.wgsl"));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&pass_layout), Some(object_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<super::mesh::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
                compilation_options: Default::default(),
            },
            // Both sides: thin planes and open meshes still cast.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        Self {
            sun_size,
            sun_view,
            cascade_views,
            point_view,
            face_views,
            face_buffer,
            pipeline,
            pass_buffer,
            pass_bind_group,
            pass_stride,
            cache: [None; MAX_SHADOWED],
        }
    }

    /// Forget every cached point-light map (the scene was rebuilt).
    pub fn invalidate(&mut self) {
        self.cache = [None; MAX_SHADOWED];
    }

    /// Renders the sun's map (if `sun` is given) and every shadowed light's
    /// faces that are out of date.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        object_bind_group: &wgpu::BindGroup,
        casters: &[Caster],
        sun: Option<[Mat4; CASCADES]>,
        lights: &[FrameLight],
    ) {
        let mut faces = vec![Mat4::ZERO; MAX_SHADOWED * 6];
        let mut todo: Vec<(usize, &FrameLight)> = Vec::new();
        for light in lights {
            let Some(slot) = light.shadow else { continue };
            let matrices = lights::cube_faces(light.position, light.range);
            faces[slot * 6..slot * 6 + 6].copy_from_slice(&matrices);
            let key = cache_key(light, casters);
            if self.cache[slot] != Some(key) {
                self.cache[slot] = Some(key);
                todo.push((slot, light));
            }
        }
        queue.write_buffer(&self.face_buffer, 0, bytemuck::cast_slice(&faces));
        let mut passes = vec![Mat4::ZERO; PASSES];
        passes[..CASCADES].copy_from_slice(&sun.unwrap_or([Mat4::ZERO; CASCADES]));
        passes[CASCADES..].copy_from_slice(&faces);
        for (i, m) in passes.iter().enumerate() {
            queue.write_buffer(
                &self.pass_buffer,
                i as u64 * self.pass_stride,
                bytemuck::bytes_of(m),
            );
        }

        if let Some(sun) = sun {
            for (i, matrix) in sun.into_iter().enumerate() {
                let view = &self.cascade_views[i];
                self.pass(encoder, view, i, matrix, object_bind_group, casters, None);
            }
        }
        for (slot, light) in todo {
            for face in 0..6 {
                let layer = slot * 6 + face;
                let range = Some((light.object, light.position, light.range));
                self.pass(
                    encoder,
                    &self.face_views[layer],
                    CASCADES + layer,
                    faces[layer],
                    object_bind_group,
                    casters,
                    range,
                );
            }
        }
    }

    /// One depth pass. `light`: the light's object (it doesn't shadow
    /// itself), position and range, to skip casters out of reach.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        index: usize,
        view_proj: Mat4,
        object_bind_group: &wgpu::BindGroup,
        casters: &[Caster],
        light: Option<(usize, Vec3, f32)>,
    ) {
        let frustum = Frustum::new(view_proj);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        let offset = (index as u64 * self.pass_stride) as u32;
        pass.set_bind_group(0, &self.pass_bind_group, &[offset]);
        for caster in casters {
            if let Some((object, position, range)) = light
                && (caster.object == object
                    || caster.sphere.0.distance(position) > range + caster.sphere.1)
            {
                continue;
            }
            if !frustum.sees_sphere(caster.sphere.0, caster.sphere.1) {
                continue;
            }
            pass.set_bind_group(1, object_bind_group, &[caster.offset]);
            pass.set_vertex_buffer(0, caster.vertices.slice(..));
            pass.set_index_buffer(caster.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..caster.index_count, 0, 0..1);
        }
    }
}

/// Identifies what a light's shadow map shows: the light, and every
/// caster in its reach with where it is.
fn cache_key(light: &FrameLight, casters: &[Caster]) -> u64 {
    let mut hasher = DefaultHasher::new();
    let bits = |v: Vec3| v.to_array().map(f32::to_bits);
    bits(light.position).hash(&mut hasher);
    light.range.to_bits().hash(&mut hasher);
    light.object.hash(&mut hasher);
    for caster in casters {
        if caster.sphere.0.distance(light.position) <= light.range + caster.sphere.1 {
            caster.object.hash(&mut hasher);
            caster.index_count.hash(&mut hasher);
            caster
                .model
                .to_cols_array()
                .map(f32::to_bits)
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

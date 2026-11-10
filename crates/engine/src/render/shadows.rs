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
use super::lights::{self, CASCADES, FrameLight};
use glam::{Mat4, Vec3};
use std::hash::{DefaultHasher, Hash, Hasher};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// An instance that casts shadows.
pub struct Caster {
    /// Index into `Scene::objects` (a light doesn't shadow its own object).
    pub object: usize,
    /// Index into the renderer's meshes.
    pub mesh: usize,
    /// Index into the renderer's instances.
    pub instance: u32,
    pub model: Mat4,
    pub sphere: (Vec3, f32),
    pub index_count: u32,
}

/// One shadow map to draw this frame.
pub struct PassPlan {
    /// A sun cascade, or a layer of the point-light cube faces.
    target: Target,
    /// Its view-projection's slot in the pass buffer.
    index: usize,
    /// One instanced draw per mesh: the mesh and its range of the frame's
    /// visible list.
    batches: Vec<(usize, std::ops::Range<u32>)>,
}

enum Target {
    Cascade(usize),
    Face(usize),
}

/// This frame's shadow maps to draw.
pub struct ShadowPlan {
    passes: Vec<PassPlan>,
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
    /// Per cube face (slot × 6 + face): what it was last rendered for, so
    /// only faces whose view changed are rendered again. Its length / 6 is
    /// how many point lights can have shadows at once.
    cache: Vec<Option<u64>>,
}

impl Shadows {
    pub fn new(
        device: &wgpu::Device,
        object_layout: &wgpu::BindGroupLayout,
        sun_size: u32,
        point_size: u32,
        slots: usize,
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
        let layers = (slots * 6) as u32;
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
            size: (size_of::<Mat4>() * slots * 6) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let pass_stride = (size_of::<Mat4>() as u64).next_multiple_of(alignment);
        let pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow passes"),
            // The sun's cascades first, then six faces per slot.
            size: pass_stride * (CASCADES + slots * 6) as u64,
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
            cache: vec![None; slots * 6],
        }
    }

    /// How many point lights can have shadows at once.
    pub fn slots(&self) -> usize {
        self.cache.len() / 6
    }

    /// Forget every cached point-light map (the scene was rebuilt).
    pub fn invalidate(&mut self) {
        self.cache.fill(None);
    }

    /// Renders the sun's map (if `sun` is given) and every shadowed light's
    /// faces that are out of date.
    /// Decides what this frame's shadow maps draw: every sun cascade, and
    /// the cube faces whose casters changed. Culled instances go into
    /// `visible`, grouped per mesh; `encode` draws them once that list is
    /// on the GPU.
    pub fn plan(
        &mut self,
        queue: &wgpu::Queue,
        casters: &[Caster],
        sun: Option<[Mat4; CASCADES]>,
        lights: &[FrameLight],
        visible: &mut Vec<u32>,
    ) -> ShadowPlan {
        let mut faces = vec![Mat4::ZERO; self.slots() * 6];
        let mut todo: Vec<(usize, &FrameLight)> = Vec::new();
        for light in lights {
            let Some(slot) = light.shadow else { continue };
            let matrices = lights::cube_faces(light.position, light.range);
            faces[slot * 6..slot * 6 + 6].copy_from_slice(&matrices);
            for (face, matrix) in matrices.iter().enumerate() {
                let layer = slot * 6 + face;
                let key = cache_key(light, *matrix, casters);
                if self.cache[layer] != Some(key) {
                    self.cache[layer] = Some(key);
                    todo.push((layer, light));
                }
            }
        }
        queue.write_buffer(&self.face_buffer, 0, bytemuck::cast_slice(&faces));
        let mut passes = vec![Mat4::ZERO; CASCADES + faces.len()];
        passes[..CASCADES].copy_from_slice(&sun.unwrap_or([Mat4::ZERO; CASCADES]));
        passes[CASCADES..].copy_from_slice(&faces);
        for (i, m) in passes.iter().enumerate() {
            queue.write_buffer(
                &self.pass_buffer,
                i as u64 * self.pass_stride,
                bytemuck::bytes_of(m),
            );
        }

        let mut plan = ShadowPlan { passes: Vec::new() };
        if let Some(sun) = sun {
            for (i, matrix) in sun.into_iter().enumerate() {
                let batches = cull(matrix, casters, None, visible);
                plan.passes.push(PassPlan {
                    target: Target::Cascade(i),
                    index: i,
                    batches,
                });
            }
        }
        if !todo.is_empty() {
            crate::perf::count("CUBE FACES", todo.len());
        }
        for (layer, light) in todo {
            let range = Some((light.object, light.position, light.range));
            let batches = cull(faces[layer], casters, range, visible);
            plan.passes.push(PassPlan {
                target: Target::Face(layer),
                index: CASCADES + layer,
                batches,
            });
        }
        plan
    }

    /// Draws the planned shadow maps (the visible list must be uploaded).
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        plan: &ShadowPlan,
        instance_bind_group: &wgpu::BindGroup,
        meshes: &[super::GpuMesh],
    ) {
        let mut draws = 0;
        for p in &plan.passes {
            let target = match p.target {
                Target::Cascade(i) => &self.cascade_views[i],
                Target::Face(layer) => &self.face_views[layer],
            };
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
            let offset = (p.index as u64 * self.pass_stride) as u32;
            pass.set_bind_group(0, &self.pass_bind_group, &[offset]);
            pass.set_bind_group(1, instance_bind_group, &[]);
            for (mesh, range) in &p.batches {
                let mesh = &meshes[*mesh];
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, range.clone());
                draws += 1;
            }
        }
        crate::perf::count("SHADOW DRAWS", draws);
    }
}

/// The casters one shadow map sees (`light`: a point light's object, which
/// casts no shadow of its own, and its reach), grouped per mesh into
/// `visible`.
fn cull(
    view_proj: Mat4,
    casters: &[Caster],
    light: Option<(usize, Vec3, f32)>,
    visible: &mut Vec<u32>,
) -> Vec<(usize, std::ops::Range<u32>)> {
    let frustum = Frustum::new(view_proj);
    let mut items: Vec<(usize, u32)> = casters
        .iter()
        .filter(|c| {
            light.is_none_or(|(object, position, range)| {
                c.object != object && c.sphere.0.distance(position) <= range + c.sphere.1
            })
        })
        .filter(|c| frustum.sees_sphere(c.sphere.0, c.sphere.1))
        .map(|c| (c.mesh, c.instance))
        .collect();
    super::batch(&mut items, visible)
}

/// Identifies what one cube face of a light's shadow map shows: the light,
/// the face, and every caster that face sees (in reach and in its view),
/// with where it is.
fn cache_key(light: &FrameLight, face: Mat4, casters: &[Caster]) -> u64 {
    let mut hasher = DefaultHasher::new();
    let bits = |v: Vec3| v.to_array().map(f32::to_bits);
    bits(light.position).hash(&mut hasher);
    light.range.to_bits().hash(&mut hasher);
    light.object.hash(&mut hasher);
    face.to_cols_array().map(f32::to_bits).hash(&mut hasher);
    let frustum = Frustum::new(face);
    for caster in casters {
        if caster.object != light.object
            && caster.sphere.0.distance(light.position) <= light.range + caster.sphere.1
            && frustum.sees_sphere(caster.sphere.0, caster.sphere.1)
        {
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

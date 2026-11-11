//! The live effects that read the scene pass's surfaces (depth, normals,
//! reflectance):
//!
//! - **Traced occlusion** (`occlusion.wgsl`): short rays through a BVH of
//!   the scene's triangles, on a coarse fixed grid, accumulated over
//!   frames. The scene pass dims indirect light with last frame's result.
//! - **Screen-space reflections** (`reflect.wgsl`): glossy surfaces reflect
//!   what is on screen, in a pass between the scene and post.
//!
//! Both are off unless the scene's environment turns them on.
//!
//! The triangles are kept as two trees in one pair of buffers: the ones
//! that move first (rebuilt whenever something moves, so they get room to
//! grow), then the ones that stay put (rebuilt only on edits). Lightmapped
//! surfaces trace only the moving tree.

use super::HDR_FORMAT;
use super::bvh::{Bvh, Node, Triangle};
use super::quality::Settings;
use crate::camera::Camera;
use crate::scene::Environment;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

/// Meters an occlusion ray looks for something.
const OCCLUSION_REACH: f32 = 1.5;
/// Meters a reflected ray looks for something.
const REFLECTION_REACH: f32 = 40.0;
const OCCLUSION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Matches `Live` in `occlusion.wgsl` and `reflect.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LiveUniform {
    view_proj: Mat4,
    inv_view_proj: Mat4,
    prev_view_proj: Mat4,
    camera_pos: Vec4,
    ambient_sky: Vec4,
    ambient_ground: Vec4,
    occlusion: Vec4,
    reflection: Vec4,
}

/// The scene pass's extra images at internal resolution, made when a live
/// effect is first on.
pub struct Surfaces {
    /// The scene's color before reflections are added.
    pub scene: wgpu::TextureView,
    /// xyz: world normal, a: roughness.
    pub normal: wgpu::TextureView,
    /// rgb: reflectance, a: reflection cube + 1 (+ 32 if lightmapped).
    pub reflectance: wgpu::TextureView,
}

impl Surfaces {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let target = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HDR_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Self {
            scene: target("scene before reflections"),
            normal: target("surface normals"),
            reflectance: target("surface reflectance"),
        }
    }
}

pub struct Live {
    uniform: wgpu::Buffer,
    occlusion_layout: wgpu::BindGroupLayout,
    occlusion_pipeline: wgpu::ComputePipeline,
    reflect_layout: wgpu::BindGroupLayout,
    reflect_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// Occlusion cells across and down.
    grid: [u32; 2],
    rays: u32,
    steps: u32,
    /// Last frame's occlusion: what the scene pass reads.
    history: wgpu::Texture,
    pub history_view: wgpu::TextureView,
    /// This frame's, copied to `history` when done.
    work: wgpu::Texture,
    work_view: wgpu::TextureView,

    nodes: wgpu::Buffer,
    triangles: wgpu::Buffer,
    still: Bvh,
    moving: Bvh,
    /// Room kept for the moving tree at the start of each buffer.
    node_room: u32,
    triangle_room: u32,

    /// The view the last occlusion pass was traced for, if there was one
    /// last frame.
    traced: Option<Mat4>,
    /// This frame's view, and whether its occlusion pass has run.
    view_proj: Mat4,
    ran: bool,
    frame: u32,
}

impl Live {
    pub fn new(device: &wgpu::Device, settings: &Settings) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let depth = wgpu::TextureSampleType::Depth;
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let sampler_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let fragment = wgpu::ShaderStages::FRAGMENT;

        let occlusion_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("occlusion"),
            entries: &[
                uniform_entry,
                texture(1, depth),
                texture(2, float),
                texture(3, float),
                texture(4, float),
                sampler_entry(5, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: OCCLUSION_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                storage(7, compute),
                storage(8, compute),
            ],
        });
        let occlusion_shader =
            device.create_shader_module(wgpu::include_wgsl!("shaders/occlusion.wgsl"));
        let occlusion_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("occlusion"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("occlusion"),
                    bind_group_layouts: &[Some(&occlusion_layout)],
                    immediate_size: 0,
                }),
            ),
            module: &occlusion_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let reflect_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reflect"),
            entries: &[
                uniform_entry,
                texture(1, float),
                texture(2, depth),
                texture(3, float),
                texture(4, float),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: float,
                        view_dimension: wgpu::TextureViewDimension::CubeArray,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(6, fragment),
                sampler_entry(7, fragment),
            ],
        });
        let reflect_shader =
            device.create_shader_module(wgpu::include_wgsl!("shaders/reflect.wgsl"));
        let reflect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("reflect"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("reflect"),
                    bind_group_layouts: &[Some(&reflect_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &reflect_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &reflect_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(HDR_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let grid = settings.occlusion_grid.map(|n| n.max(1));
        let cells = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: grid[0],
                    height: grid[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OCCLUSION_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let history = cells("occlusion");
        let work = cells("occlusion, this frame");
        let mut live = Self {
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("live"),
                size: size_of::<LiveUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            occlusion_layout,
            occlusion_pipeline,
            reflect_layout,
            reflect_pipeline,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("occlusion history"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            grid,
            rays: settings.occlusion_rays,
            steps: settings.reflection_steps,
            history_view: history.create_view(&Default::default()),
            history,
            work_view: work.create_view(&Default::default()),
            work,
            // Replaced right below.
            nodes: device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            triangles: device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            still: Bvh::default(),
            moving: Bvh::default(),
            node_room: 0,
            triangle_room: 0,
            traced: None,
            view_proj: Mat4::IDENTITY,
            ran: false,
            frame: 0,
        };
        live.upload(device, None, true);
        live
    }

    /// The triangles rays are traced against, in world space: the ones
    /// that stay put and the ones that move; `None` = as they were.
    pub fn set_triangles(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        still: Option<Vec<[Vec3; 3]>>,
        moving: Option<Vec<[Vec3; 3]>>,
    ) {
        let still_changed = still.is_some();
        if let Some(triangles) = still {
            self.still = crate::perf::time("BVH STILL", || Bvh::build(triangles));
        }
        if let Some(triangles) = moving {
            self.moving = Bvh::build(triangles);
        }
        self.upload(device, Some(queue), still_changed);
    }

    /// Puts the trees on the GPU: only the moving one if it still fits its
    /// room and the other didn't change, else both into new buffers.
    fn upload(&mut self, device: &wgpu::Device, queue: Option<&wgpu::Queue>, still_changed: bool) {
        let need = [
            self.moving.nodes.len() as u32,
            self.moving.triangles.len() as u32,
        ];
        let fits = need[0] <= self.node_room && need[1] <= self.triangle_room;
        if let (Some(queue), true, false) = (queue, fits, still_changed) {
            queue.write_buffer(&self.nodes, 0, bytemuck::cast_slice(&self.moving.nodes));
            if !self.moving.triangles.is_empty() {
                queue.write_buffer(
                    &self.triangles,
                    0,
                    bytemuck::cast_slice(&self.moving.triangles),
                );
            }
            return;
        }
        if !fits {
            self.node_room = need[0].next_power_of_two().max(64);
            self.triangle_room = need[1].next_power_of_two().max(64);
        }
        let (nodes, triangles) = layout(
            &self.moving,
            &self.still,
            self.node_room,
            self.triangle_room,
        );
        let buffer = |label, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };
        self.nodes = buffer("bvh nodes", bytemuck::cast_slice(&nodes));
        self.triangles = buffer("bvh triangles", bytemuck::cast_slice(&triangles));
    }

    /// Forget last frame's occlusion (a new scene).
    pub fn reset(&mut self) {
        self.traced = None;
    }

    /// The view last frame's occlusion was traced for, if the scene pass
    /// can use it this frame.
    pub fn traced(&self) -> Option<Mat4> {
        self.traced
    }

    /// Once per frame, before the passes: this frame's view.
    pub fn begin(
        &mut self,
        queue: &wgpu::Queue,
        camera: &Camera,
        view_proj: Mat4,
        env: &Environment,
    ) {
        self.frame = self.frame.wrapping_add(1);
        let uniform = LiveUniform {
            view_proj,
            inv_view_proj: view_proj.inverse(),
            prev_view_proj: self.traced.unwrap_or(view_proj),
            camera_pos: camera.position.extend((self.frame % 65536) as f32),
            ambient_sky: env.ambient_sky.extend(camera.near),
            ambient_ground: env.ambient_ground.extend(camera.far),
            occlusion: Vec4::new(
                self.node_room as f32,
                self.rays.max(1) as f32,
                OCCLUSION_REACH,
                self.traced.is_some() as u32 as f32,
            ),
            reflection: Vec4::new(self.steps.max(1) as f32, REFLECTION_REACH, 0.0, 0.0),
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.view_proj = view_proj;
        self.ran = false;
    }

    /// Traces this frame's occlusion from the scene pass's depth and
    /// surfaces, for the next frame to use.
    pub fn occlude(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        surfaces: &Surfaces,
    ) {
        let view = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("occlusion"),
            layout: &self.occlusion_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                view(1, depth),
                view(2, &surfaces.normal),
                view(3, &surfaces.reflectance),
                view(4, &self.history_view),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                view(6, &self.work_view),
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: self.nodes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: self.triangles.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("occlusion"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.occlusion_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(self.grid[0].div_ceil(8), self.grid[1].div_ceil(8), 1);
        }
        encoder.copy_texture_to_texture(
            self.work.as_image_copy(),
            self.history.as_image_copy(),
            wgpu::Extent3d {
                width: self.grid[0],
                height: self.grid[1],
                depth_or_array_layers: 1,
            },
        );
        self.ran = true;
    }

    /// Once per frame, after the passes: what was traced this frame is
    /// what the next one can use.
    pub fn end(&mut self) {
        self.traced = self.ran.then_some(self.view_proj);
    }

    /// Adds screen-space reflections to the scene's color (`surfaces.scene`),
    /// into `target`.
    #[allow(clippy::too_many_arguments)]
    pub fn reflect(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        surfaces: &Surfaces,
        target: &wgpu::TextureView,
        cubes: &wgpu::TextureView,
        boxes: &wgpu::Buffer,
        cube_sampler: &wgpu::Sampler,
    ) {
        let view = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reflect"),
            layout: &self.reflect_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                view(1, &surfaces.scene),
                view(2, depth),
                view(3, &surfaces.normal),
                view(4, &surfaces.reflectance),
                view(5, cubes),
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: boxes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(cube_sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("reflect"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline(&self.reflect_pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Both trees as they go on the GPU: the moving one at the start of each
/// buffer (with `node_room` / `triangle_room` kept for it), then the still
/// one, its indices moved past that room. The still tree's root is node
/// `node_room`.
fn layout(
    moving: &Bvh,
    still: &Bvh,
    node_room: u32,
    triangle_room: u32,
) -> (Vec<Node>, Vec<Triangle>) {
    let mut nodes = vec![Node::zeroed(); node_room as usize];
    nodes[..moving.nodes.len()].copy_from_slice(&moving.nodes);
    nodes.extend(still.nodes.iter().map(|node| Node {
        first: node.first
            + if node.count > 0 {
                triangle_room
            } else {
                node_room
            },
        ..*node
    }));
    let mut triangles = vec![Triangle::zeroed(); triangle_room as usize];
    triangles[..moving.triangles.len()].copy_from_slice(&moving.triangles);
    triangles.extend_from_slice(&still.triangles);
    (nodes, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straight down from `origin` through the laid-out buffers, starting
    /// at a given root.
    fn trace(nodes: &[Node], triangles: &[Triangle], root: u32, origin: Vec3) -> Option<f32> {
        let mut stack = vec![root];
        let mut nearest = None;
        while let Some(index) = stack.pop() {
            let node = nodes[index as usize];
            if node.count == 0 {
                stack.extend([node.first, node.first + 1]);
                continue;
            }
            let leaf = Bvh {
                nodes: vec![Node { first: 0, ..node }],
                triangles: triangles[node.first as usize..][..node.count as usize].to_vec(),
            };
            if let Some(t) = leaf.trace(origin, Vec3::NEG_Y, 100.0) {
                nearest = Some(nearest.map_or(t, |n: f32| n.min(t)));
            }
        }
        nearest
    }

    fn tiles(count: usize, height: f32) -> Vec<[Vec3; 3]> {
        (0..count)
            .map(|i| {
                let x = i as f32;
                [
                    Vec3::new(x, height, 0.0),
                    Vec3::new(x + 1.0, height, 0.0),
                    Vec3::new(x, height, 1.0),
                ]
            })
            .collect()
    }

    #[test]
    fn both_trees_share_the_buffers() {
        let moving = Bvh::build(tiles(10, 2.0));
        let still = Bvh::build(tiles(40, 0.0));
        let (nodes, triangles) = layout(&moving, &still, 64, 64);
        assert_eq!(nodes.len(), 64 + still.nodes.len());
        assert_eq!(triangles.len(), 64 + 40);
        let above = |x: f32| Vec3::new(x + 0.2, 5.0, 0.2);
        // The moving tree from node 0, the still one from the room's end.
        assert_eq!(
            trace(&nodes, &triangles, 0, above(3.0)).map(f32::round),
            Some(3.0)
        );
        assert_eq!(trace(&nodes, &triangles, 0, above(30.0)), None);
        assert_eq!(
            trace(&nodes, &triangles, 64, above(30.0)).map(f32::round),
            Some(5.0)
        );
    }

    #[test]
    fn empty_trees_lay_out() {
        let (nodes, triangles) = layout(&Bvh::default(), &Bvh::default(), 64, 64);
        assert_eq!((nodes.len(), triangles.len()), (65, 65));
        assert_eq!(trace(&nodes, &triangles, 0, Vec3::Y), None);
        assert_eq!(trace(&nodes, &triangles, 64, Vec3::Y), None);
    }
}

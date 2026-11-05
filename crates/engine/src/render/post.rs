//! Post-processing: bloom, exposure, tone mapping, per-zone color grading
//! and grain, ending in the nearest-neighbor upscale to the window.
//!
//! Bloom runs at internal resolution and below: a chain of half-size
//! images, each a blur of the one above, added back up (see `bloom.wgsl`).
//! Grading is a 3D LUT per zone (see `grade`); moving into a zone with a
//! different grade blends over [`GRADE_BLEND`] seconds.

use super::grade::{self, LUT_SIZE};
use crate::scene::Scene;
use bytemuck::{Pod, Zeroable};
use std::path::Path;

/// Bloom chain length (each level half the previous size).
const BLOOM_LEVELS: u32 = 5;
const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Seconds to blend from one zone's grade to the next.
pub const GRADE_BLEND: f32 = 1.5;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 2],
    factor: f32,
    linear_output: u32,
    bloom: f32,
    grain: f32,
    exposure: f32,
    lut_mix: f32,
    frame: u32,
    /// 1 / bloom levels: the chain adds every level up.
    bloom_scale: f32,
    _pad: [u32; 2],
}

/// Where the post pass draws and how.
#[derive(Debug, Clone, Copy)]
pub struct Output {
    pub origin: [f32; 2],
    pub factor: f32,
    pub linear: bool,
}

struct Bloom {
    /// One view per level, largest first.
    levels: Vec<wgpu::TextureView>,
    /// Level i reads the scene (i = 0) or level i - 1.
    down: Vec<wgpu::BindGroup>,
    /// Level i reads level i + 1.
    up: Vec<wgpu::BindGroup>,
}

pub struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    bloom_layout: wgpu::BindGroupLayout,
    down_first: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    bloom: Option<Bloom>,
    scene_view: Option<wgpu::TextureView>,

    /// One LUT per zone, then the environment's (for zones without one and
    /// outside every zone).
    luts: Vec<wgpu::TextureView>,
    /// Linear exposure multiplier per LUT.
    exposures: Vec<f32>,
    lut_from: usize,
    lut_to: usize,
    lut_mix: f32,
    exposure: f32,
    bloom_strength: f32,
    grain: f32,
    frame: u32,
}

impl Post {
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let texture_entry = |binding, dimension, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let d2 = wgpu::TextureViewDimension::D2;
        let d3 = wgpu::TextureViewDimension::D3;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                texture_entry(0, d2, false),
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
                texture_entry(2, d2, true),
                texture_entry(3, d3, true),
                texture_entry(4, d3, true),
                sampler_entry(5),
            ],
        });
        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom"),
            entries: &[texture_entry(0, d2, true), sampler_entry(1)],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post params"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let post_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/post.wgsl"));
        let pipeline = fullscreen_pipeline(
            device,
            "post",
            &layout,
            &post_shader,
            "fs_main",
            output_format,
            None,
        );
        let bloom_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/bloom.wgsl"));
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let bloom_pipeline = |entry, blend| {
            fullscreen_pipeline(
                device,
                entry,
                &bloom_layout,
                &bloom_shader,
                entry,
                BLOOM_FORMAT,
                blend,
            )
        };

        Self {
            pipeline,
            layout,
            params,
            bind_group: None,
            sampler,
            down_first: bloom_pipeline("fs_down_first", None),
            down: bloom_pipeline("fs_down", None),
            up: bloom_pipeline("fs_up", Some(add)),
            bloom_layout,
            bloom: None,
            scene_view: None,
            luts: Vec::new(),
            exposures: Vec::new(),
            lut_from: 0,
            lut_to: 0,
            lut_mix: 1.0,
            exposure: 1.0,
            bloom_strength: 0.0,
            grain: 0.0,
            frame: 0,
        }
    }

    /// The internal HDR image changed (new size): rebuild the bloom chain.
    pub fn set_scene_view(
        &mut self,
        device: &wgpu::Device,
        scene: &wgpu::TextureView,
        size: [u32; 2],
    ) {
        let levels = BLOOM_LEVELS.min(size[0].min(size[1]).max(2).ilog2()).max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom"),
            size: wgpu::Extent3d {
                width: (size[0] / 2).max(1),
                height: (size[1] / 2).max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BLOOM_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let views: Vec<wgpu::TextureView> = (0..levels)
            .map(|level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let group = |source: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom"),
                layout: &self.bloom_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
        };
        let down = (0..levels as usize)
            .map(|i| group(if i == 0 { scene } else { &views[i - 1] }))
            .collect();
        let up = (0..levels as usize - 1)
            .map(|i| group(&views[i + 1]))
            .collect();
        self.bloom = Some(Bloom {
            levels: views,
            down,
            up,
        });
        self.scene_view = Some(scene.clone());
        self.rebuild_bind_group(device);
    }

    /// Builds one grading LUT per zone (and one for the environment) and
    /// takes the scene's bloom and grain settings.
    pub fn set_grades(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        project: &Path,
    ) {
        let env = &scene.environment;
        let grades = scene
            .zones
            .iter()
            .map(|z| z.grade.as_ref().unwrap_or(&env.grade))
            .chain([&env.grade]);
        self.luts.clear();
        self.exposures.clear();
        for g in grades {
            let lut = grade::load_lut(g, project);
            let data = grade::bake(g, lut.as_ref());
            self.luts.push(upload_lut(device, queue, &data));
            self.exposures.push(2f32.powf(g.exposure));
        }
        let default = self.luts.len() - 1;
        self.lut_from = default;
        self.lut_to = default;
        self.lut_mix = 1.0;
        self.exposure = self.exposures[default];
        self.bloom_strength = env.bloom.clamp(0.0, 1.0);
        self.grain = env.grain.max(0.0);
        self.rebuild_bind_group(device);
    }

    /// The camera's zone this frame (`None` = outside every zone); blends
    /// toward its grade.
    pub fn update(&mut self, device: &wgpu::Device, zone: Option<usize>, dt: f32) {
        if self.luts.is_empty() {
            return;
        }
        let default = self.luts.len() - 1;
        let target = zone.filter(|&z| z < default).unwrap_or(default);
        if target != self.lut_to {
            // Start from wherever the blend has got to.
            self.lut_from = if self.lut_mix < 0.5 {
                self.lut_from
            } else {
                self.lut_to
            };
            self.lut_to = target;
            self.lut_mix = 0.0;
            self.rebuild_bind_group(device);
        }
        let step = dt / GRADE_BLEND;
        self.lut_mix = (self.lut_mix + step).min(1.0);
        let goal = self.exposures[target];
        self.exposure += (goal - self.exposure) * (dt * 3.0 / GRADE_BLEND).min(1.0);
        self.frame = self.frame.wrapping_add(1);
    }

    fn rebuild_bind_group(&mut self, device: &wgpu::Device) {
        let (Some(scene), Some(bloom)) = (&self.scene_view, &self.bloom) else {
            return;
        };
        if self.luts.is_empty() {
            return; // no scene yet
        }
        let view = |v| wgpu::BindingResource::TextureView(v);
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view(&bloom.levels[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: view(&self.luts[self.lut_from]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: view(&self.luts[self.lut_to]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
    }

    /// Bloom passes, then the final pass into `target` within the given
    /// viewport (x, y, width, height in pixels).
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        output: Output,
        viewport: [f32; 4],
    ) {
        let (Some(bloom), Some(bind_group)) = (&self.bloom, &self.bind_group) else {
            return;
        };
        let params = Params {
            origin: output.origin,
            factor: output.factor,
            linear_output: output.linear as u32,
            bloom: self.bloom_strength,
            grain: self.grain,
            exposure: self.exposure,
            lut_mix: smooth(self.lut_mix),
            frame: self.frame,
            bloom_scale: 1.0 / bloom.levels.len() as f32,
            _pad: [0; 2],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));

        if self.bloom_strength > 0.0 {
            for (i, group) in bloom.down.iter().enumerate() {
                let pipeline = if i == 0 { &self.down_first } else { &self.down };
                fullscreen_pass(encoder, &bloom.levels[i], true, pipeline, group);
            }
            for (i, group) in bloom.up.iter().enumerate().rev() {
                fullscreen_pass(encoder, &bloom.levels[i], false, &self.up, group);
            }
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
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
        let [x, y, w, h] = viewport;
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Ease in and out, so grade blends don't start or stop abruptly.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn upload_lut(device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: LUT_SIZE,
        height: LUT_SIZE,
        depth_or_array_layers: LUT_SIZE,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grade lut"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
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
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(LUT_SIZE * 4),
            rows_per_image: Some(LUT_SIZE),
        },
        size,
    );
    texture.create_view(&Default::default())
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    entry: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A fullscreen triangle into `target`; `clear` or add onto what's there.
fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    clear: bool,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("bloom"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

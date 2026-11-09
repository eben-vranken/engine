//! Volumetric fog in a froxel grid: a 3D grid that follows the camera's
//! view (its size set by the quality preset), with depth slices spaced
//! exponentially out to `fog_distance` (thin near the camera, where detail
//! shows).
//!
//! Two compute passes per frame (see the shaders):
//! 1. inject: per cell, fog density and the light it scatters toward the
//!    camera (sun and lights, through their shadow maps, so shafts and
//!    halos fall out naturally), blended with last frame's result;
//! 2. integrate: running sums along each column, so any depth can look up
//!    "the fog between the camera and here" in one texture sample.
//!
//! The scene shader then does `color × transmittance + light` per pixel.

use super::lights::CASCADES;
use crate::camera::Camera;
use crate::scene::{Component, Environment, Fog, Scene};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};

/// Depth of the first slice (meters).
pub const NEAR: f32 = 0.4;
/// Most fog regions (zone volumes plus FogVolumes).
pub const MAX_REGIONS: usize = 64;
/// Weight of this frame's sample against the accumulated history: lower is
/// smoother but slower to follow a flickering light.
const BLEND: f32 = 0.08;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InjectUniform {
    sun_view_proj: [Mat4; CASCADES],
    prev_view_proj: Mat4,
    camera_pos: Vec4,
    camera_forward: Vec4,
    camera_right: Vec4,
    camera_up: Vec4,
    sun_dir: Vec4,
    sun_color: Vec4,
    ambient: Vec4,
    default_fog: Vec4,
    default_shape: Vec4,
    depth: Vec4,
    cascades: Vec4,
    /// Default fog's wisps: strength, size, speed.
    default_wisps: Vec4,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct IntegrateUniform {
    camera_forward: Vec4,
    camera_right: Vec4,
    camera_up: Vec4,
    depth: Vec4,
}

/// Matches `Region` in `fog_inject.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuRegion {
    to_box: Mat4,
    fog: Vec4,
    shape: Vec4,
    /// x: the box's height in meters (FogVolumes); yzw: the ambient light
    /// inside (zones).
    extra: Vec4,
    /// Wisps (zones): strength, size, speed.
    wisps: Vec4,
}

/// The camera's ray basis: forward, and right/up scaled so that
/// `forward + ndc.x * right + ndc.y * up` points through a spot on screen.
pub fn ray_basis(camera: &Camera, aspect: f32) -> [Vec3; 3] {
    let forward = camera.forward();
    let right = camera.right();
    let up = right.cross(forward);
    let tan_y = (camera.fov_y.to_radians() * 0.5).tan();
    [forward, right * tan_y * aspect, up * tan_y]
}

/// The scene's fog regions: every zone volume with its zone's fog (or the
/// environment's), smallest first, then every FogVolume. `zone_ambient`
/// gives a zone's baked ambient light (so fog indoors isn't lit by the
/// open sky); without it, the environment's.
pub fn regions(scene: &Scene, zone_ambient: impl Fn(&str) -> Option<Vec3>) -> Vec<GpuRegion> {
    let env = &scene.environment;
    let sky = (env.ambient_sky + env.ambient_ground) * 0.5;
    let mut zones: Vec<(f32, GpuRegion)> = Vec::new();
    let mut volumes = Vec::new();
    for object in &scene.objects {
        let t = &object.transform;
        for component in &object.components {
            match component {
                Component::ZoneVolume(volume) => {
                    let size = volume.size * t.scale;
                    if size.abs().min_element() <= 0.0 {
                        continue;
                    }
                    let fog = scene
                        .zones
                        .iter()
                        .find(|z| z.name == object.zone)
                        .and_then(|z| z.fog.as_ref())
                        .unwrap_or(&env.fog);
                    let to_box = (t.matrix() * Mat4::from_scale(volume.size)).inverse();
                    let ambient = zone_ambient(&object.zone).unwrap_or(sky);
                    zones.push((
                        size.abs().element_product(),
                        zone_region(to_box, fog, ambient),
                    ));
                }
                Component::FogVolume(volume) => {
                    let size = volume.size * t.scale;
                    if size.abs().min_element() <= 0.0 {
                        continue;
                    }
                    volumes.push(GpuRegion {
                        to_box: (t.matrix() * Mat4::from_scale(volume.size)).inverse(),
                        fog: volume.color.extend(volume.density.max(0.0)),
                        shape: Vec4::new(0.0, volume.falloff, volume.softness.clamp(0.0, 1.0), 1.0),
                        extra: Vec4::new(size.y.abs(), 0.0, 0.0, 0.0),
                        wisps: Vec4::ZERO,
                    });
                }
                _ => {}
            }
        }
    }
    zones.sort_by(|a, b| a.0.total_cmp(&b.0));
    zones
        .into_iter()
        .map(|(_, r)| r)
        .chain(volumes)
        .take(MAX_REGIONS)
        .collect()
}

fn zone_region(to_box: Mat4, fog: &Fog, ambient: Vec3) -> GpuRegion {
    GpuRegion {
        to_box,
        fog: fog.color.extend(fog.density.max(0.0)),
        shape: Vec4::new(fog.height, fog.falloff.max(0.0), 0.0, 0.0),
        extra: Vec4::new(0.0, ambient.x, ambient.y, ambient.z),
        wisps: wisp_params(fog),
    }
}

/// A fog's wisps as strength, size, speed.
fn wisp_params(fog: &Fog) -> Vec4 {
    Vec4::new(
        fog.wisps.clamp(0.0, 1.0),
        fog.wisp_size.max(0.05),
        fog.wisp_speed,
        0.0,
    )
}

/// What the fog needs to know about this frame.
pub struct FogFrame<'a> {
    pub camera: &'a Camera,
    pub aspect: f32,
    pub view_proj: Mat4,
    pub env: &'a Environment,
    pub sun_view_proj: [Mat4; CASCADES],
    pub cascades: [f32; CASCADES],
    pub sun_on: bool,
    /// Lights in the light buffer, including the fog-only ones.
    pub light_count: u32,
    /// Seconds since start, for the drifting wisps.
    pub time: f32,
}

pub struct VolumetricFog {
    /// Cells across, down and deep (from the quality preset). Fixed per
    /// run, so it costs the same at every window size.
    grid: [u32; 3],
    /// Samples per cell per frame.
    samples: u32,
    /// The integrated grid, for the scene shader.
    pub integrated_view: wgpu::TextureView,
    inject_uniform: wgpu::Buffer,
    integrate_uniform: wgpu::Buffer,
    region_buffer: wgpu::Buffer,
    region_count: u32,
    inject: wgpu::ComputePipeline,
    integrate: wgpu::ComputePipeline,
    /// Frame parity i writes scatter texture i and reads the other one.
    inject_groups: [wgpu::BindGroup; 2],
    integrate_groups: [wgpu::BindGroup; 2],
    frame: u32,
    /// Last frame's view-projection; `None` = no usable history.
    prev_view_proj: Option<Mat4>,
}

impl VolumetricFog {
    pub fn new(
        device: &wgpu::Device,
        lights: &wgpu::Buffer,
        shadow_faces: &wgpu::Buffer,
        sun_shadow: &wgpu::TextureView,
        point_shadows: &wgpu::TextureView,
        shadow_sampler: &wgpu::Sampler,
        quality: &super::quality::Settings,
    ) -> Self {
        let (grid, samples) = (quality.fog_grid, quality.fog_samples);
        let volume = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: grid[0],
                        height: grid[1],
                        depth_or_array_layers: grid[2],
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let scatter = [volume("fog scatter 0"), volume("fog scatter 1")];
        let integrated_view = volume("fog integrated");

        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let inject_uniform = uniform("fog inject", size_of::<InjectUniform>() as u64);
        let integrate_uniform = uniform("fog integrate", size_of::<IntegrateUniform>() as u64);
        let region_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fog regions"),
            size: (size_of::<GpuRegion>() * MAX_REGIONS) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fog history"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let compute = wgpu::ShaderStages::COMPUTE;
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty,
            count: None,
        };
        let uniform_ty = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let storage_ty = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let depth_array = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        };
        let volume_ty = |filterable| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        };
        let output_ty = wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: FORMAT,
            view_dimension: wgpu::TextureViewDimension::D3,
        };
        let inject_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fog inject"),
            entries: &[
                entry(0, uniform_ty),
                entry(1, storage_ty),
                entry(2, storage_ty),
                entry(3, depth_array),
                entry(4, depth_array),
                entry(
                    5,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
                entry(6, storage_ty),
                entry(7, volume_ty(true)),
                entry(
                    8,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(9, output_ty),
            ],
        });
        let integrate_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fog integrate"),
            entries: &[
                entry(0, uniform_ty),
                entry(1, volume_ty(false)),
                entry(2, output_ty),
            ],
        });

        let view = wgpu::BindingResource::TextureView;
        let inject_groups = [0, 1].map(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fog inject"),
                layout: &inject_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: inject_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: lights.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: shadow_faces.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: view(sun_shadow),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: view(point_shadows),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(shadow_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: region_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: view(&scatter[1 - i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: view(&scatter[i]),
                    },
                ],
            })
        });
        let integrate_groups = [0, 1].map(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fog integrate"),
                layout: &integrate_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: integrate_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: view(&scatter[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(&integrated_view),
                    },
                ],
            })
        });

        let pipeline = |label, layout: &wgpu::BindGroupLayout, module: wgpu::ShaderModule| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let inject = pipeline(
            "fog inject",
            &inject_layout,
            device.create_shader_module(wgpu::include_wgsl!("shaders/fog_inject.wgsl")),
        );
        let integrate = pipeline(
            "fog integrate",
            &integrate_layout,
            device.create_shader_module(wgpu::include_wgsl!("shaders/fog_integrate.wgsl")),
        );

        Self {
            grid,
            samples: samples.max(1),
            integrated_view,
            inject_uniform,
            integrate_uniform,
            region_buffer,
            region_count: 0,
            inject,
            integrate,
            inject_groups,
            integrate_groups,
            frame: 0,
            prev_view_proj: None,
        }
    }

    /// New fog regions (the scene or a volume changed).
    pub fn set_regions(&mut self, queue: &wgpu::Queue, regions: &[GpuRegion]) {
        self.region_count = regions.len() as u32;
        if !regions.is_empty() {
            queue.write_buffer(&self.region_buffer, 0, bytemuck::cast_slice(regions));
        }
    }

    /// Start over without history (new scene, or the camera jumped).
    pub fn reset(&mut self) {
        self.prev_view_proj = None;
    }

    pub fn render(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, f: FogFrame) {
        let env = f.env;
        let [forward, right, up] = ray_basis(f.camera, f.aspect);
        let far = env.fog_distance.max(NEAR * 2.0);
        let depth = Vec4::new(NEAR, far, 0.0, self.samples as f32);
        let weight = if self.prev_view_proj.is_some() {
            BLEND
        } else {
            1.0
        };
        let ambient = (env.ambient_sky + env.ambient_ground) * 0.5;
        let uniform = InjectUniform {
            sun_view_proj: f.sun_view_proj,
            prev_view_proj: self.prev_view_proj.unwrap_or(f.view_proj),
            // w: seconds, for the drifting wisps.
            camera_pos: f.camera.position.extend(f.time),
            camera_forward: forward.extend(weight),
            camera_right: right.extend(0.0),
            camera_up: up.extend(0.0),
            sun_dir: env
                .sun_direction
                .normalize_or(Vec3::NEG_Y)
                .extend(f.sun_on as u32 as f32),
            sun_color: env.sun_color.extend(env.fog_anisotropy.clamp(-0.95, 0.95)),
            ambient: ambient.extend(self.region_count as f32),
            default_fog: env.fog.color.extend(env.fog.density.max(0.0)),
            default_shape: Vec4::new(
                env.fog.height,
                env.fog.falloff.max(0.0),
                f.light_count as f32,
                self.frame as f32,
            ),
            depth,
            cascades: Vec4::new(f.cascades[0], f.cascades[1], f.cascades[2], 0.0),
            default_wisps: wisp_params(&env.fog),
        };
        queue.write_buffer(&self.inject_uniform, 0, bytemuck::bytes_of(&uniform));
        let integrate = IntegrateUniform {
            camera_forward: forward.extend(0.0),
            camera_right: right.extend(0.0),
            camera_up: up.extend(0.0),
            depth,
        };
        queue.write_buffer(&self.integrate_uniform, 0, bytemuck::bytes_of(&integrate));

        let parity = (self.frame % 2) as usize;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fog"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.inject);
        pass.set_bind_group(0, &self.inject_groups[parity], &[]);
        pass.dispatch_workgroups(
            self.grid[0].div_ceil(4),
            self.grid[1].div_ceil(4),
            self.grid[2].div_ceil(4),
        );
        pass.set_pipeline(&self.integrate);
        pass.set_bind_group(0, &self.integrate_groups[parity], &[]);
        pass.dispatch_workgroups(self.grid[0].div_ceil(8), self.grid[1].div_ceil(8), 1);
        drop(pass);

        self.prev_view_proj = Some(f.view_proj);
        self.frame = self.frame.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{FogVolume, Object, Transform, Zone, ZoneVolume};

    fn object(zone: &str, position: Vec3, component: Component) -> Object {
        Object {
            id: 0,
            hidden: false,
            name: "o".into(),
            zone: zone.into(),
            transform: Transform::at(position),
            components: vec![component],
        }
    }

    #[test]
    fn zones_come_smallest_first_then_volumes() {
        let mut room = Zone::new("room");
        room.fog = Some(Fog {
            density: 0.5,
            ..Default::default()
        });
        let scene = Scene {
            zones: vec![Zone::new("outside"), room],
            objects: vec![
                object(
                    "room",
                    Vec3::ZERO,
                    Component::FogVolume(FogVolume::default()),
                ),
                object(
                    "outside",
                    Vec3::ZERO,
                    Component::ZoneVolume(ZoneVolume {
                        size: Vec3::splat(100.0),
                        ..Default::default()
                    }),
                ),
                object(
                    "room",
                    Vec3::ZERO,
                    Component::ZoneVolume(ZoneVolume::default()),
                ),
            ],
            ..Default::default()
        };
        let regions = regions(&scene, |_| None);
        assert_eq!(regions.len(), 3);
        // The room: its own fog, and first because it's smaller.
        assert_eq!(regions[0].fog.w, 0.5);
        // Outside: the environment's fog.
        assert_eq!(regions[1].fog.w, scene.environment.fog.density);
        // The FogVolume last, marked as a volume.
        assert_eq!(regions[2].shape.w, 1.0);
    }

    #[test]
    fn region_box_maps_its_corners_to_half() {
        let scene = Scene {
            zones: vec![Zone::new("a")],
            objects: vec![object(
                "a",
                Vec3::new(10.0, 0.0, 0.0),
                Component::FogVolume(FogVolume {
                    size: Vec3::new(4.0, 2.0, 6.0),
                    ..Default::default()
                }),
            )],
            ..Default::default()
        };
        let r = regions(&scene, |_| None)[0];
        let corner = r.to_box.transform_point3(Vec3::new(12.0, 1.0, 3.0));
        assert!((corner - Vec3::splat(0.5)).length() < 1e-5);
        assert_eq!(r.extra.x, 2.0);
    }

    #[test]
    fn ray_basis_matches_the_projection() {
        let camera = Camera {
            position: Vec3::new(1.0, 2.0, 3.0),
            yaw: 0.7,
            pitch: -0.3,
            fov_y: 70.0,
            near: 0.05,
            far: 500.0,
        };
        let aspect = 16.0 / 9.0;
        let [f, r, u] = ray_basis(&camera, aspect);
        let view_proj = camera.view_projection(aspect);
        for ndc in [(0.5, -0.25), (-1.0, 1.0), (0.0, 0.0)] {
            let p = camera.position + (f + r * ndc.0 + u * ndc.1) * 7.0;
            let clip = view_proj * p.extend(1.0);
            assert!((clip.x / clip.w - ndc.0).abs() < 1e-4);
            assert!((clip.y / clip.w - ndc.1).abs() < 1e-4);
            // Clip w is the depth along forward.
            assert!((clip.w - 7.0).abs() < 1e-3);
        }
    }
}

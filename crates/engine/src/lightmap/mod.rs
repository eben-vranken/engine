//! Baked lighting: BAKE LIGHTING in the editor path-traces the light that
//! bounces around each zone (see `trace`) into lightmaps, textures that
//! cover every static surface (see `unwrap`). Moving objects get the same
//! light from probes: a grid of points per zone, each storing the light
//! arriving from six directions.
//!
//! Placements of the same mesh share one lightmap layout (UVs over 0–1,
//! made once per mesh), so they can share one GPU mesh and be drawn
//! instanced; each placement gets its own rectangle of its zone's atlas,
//! sized by its scale, and maps the layout into it (`uv * scale + offset`,
//! the usual lightmap scale/offset per renderer). Texels are still traced
//! per placement, in world space.
//!
//! The result is one file next to the scene (`scenes/hall.ron` →
//! `scenes/hall.lightmap`): per zone its atlas (packed RGB9E5 texels) and
//! probe grid, the layouts, then per static draw its layout and rectangle.

mod trace;
pub mod unwrap;

use crate::assets::{Assets, Resolved};
use crate::render::mesh::MeshData;
use crate::render::texture::{self, srgb_to_linear};
use crate::scene::{Component, Geometry, Object, Scene};
use glam::{Mat4, Vec2, Vec3};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use trace::{BakeLight, Rng, World};
use unwrap::Unwrapped;

const MAGIC: &[u8; 8] = b"LIGHTM04";
/// Formats from before shared layouts: their zones (probes, reflections)
/// still load, their draws don't, and the bake shows as outdated.
const OLD_MAGICS: [&[u8; 8]; 2] = [b"LIGHTM02", b"LIGHTM03"];
/// Largest side of one placement's rectangle in its atlas (texels).
const MAX_PLACEMENT: f32 = 2048.0;
/// Texels per side of each face of a zone's reflection cube.
pub const REFLECTION_SIZE: u32 = 64;
/// Bounce rays per reflection texel.
const REFLECTION_SAMPLES: u32 = 12;
/// Meters between probes (before shrinking to fit the cap).
const PROBE_SPACING: f32 = 1.5;
const MAX_PROBES: usize = 4096;
/// Rays per probe direction.
const PROBE_SAMPLES: u32 = 96;
/// How far texel sample points are kept from a triangle's edges (meters).
const EDGE_INSET: f32 = 0.01;

#[derive(Debug, thiserror::Error)]
pub enum LightmapError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}: not a lightmap (or an older format); bake again")]
    Format(PathBuf),
    #[error("nothing static to bake")]
    Empty,
    #[error("the bake crashed")]
    Crashed,
}

/// Which draw a lightmap belongs to, stable across loads, renames and
/// reorders: the object's id and which of its drawable components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrawKey {
    pub id: u32,
    pub part: u32,
}

/// Keys for every renderable, in `Scene::renderables` order.
pub fn draw_keys(scene: &Scene) -> Vec<DrawKey> {
    let mut keys = Vec::new();
    for object in &scene.objects {
        // One per draw: a model with parts of its own makes several.
        let parts: usize = object
            .components
            .iter()
            .map(|c| match c {
                Component::Primitive(_) => 1,
                Component::Model(m) => m.groups().count(),
                _ => 0,
            })
            .sum();
        for part in 0..parts as u32 {
            keys.push(DrawKey {
                id: object.id,
                part,
            });
        }
    }
    keys
}

/// A scale split into its size (the largest axis) and its shape (the scale
/// divided by that, rounded so near-equal shapes count as one).
pub fn split_scale(scale: Vec3) -> (f32, Vec3) {
    let size = scale.abs().max_element();
    if size <= 1e-6 {
        return (0.0, Vec3::ONE);
    }
    let shape = (scale / size * 1000.0).round() / 1000.0;
    (size, shape)
}

/// Which lightmap layout a draw uses: its mesh and the shape of its scale.
/// Placements with the same key share a layout (and a GPU mesh); one that
/// is only bigger or smaller gets more or fewer texels, not a layout of
/// its own.
pub fn mesh_key(geometry: Geometry, scale: Vec3) -> String {
    let (_, shape) = split_scale(scale);
    let mesh = match geometry {
        Geometry::Shape(shape) => format!("{shape:?}"),
        Geometry::Model(path, parts) => format!("{path} {parts:?}"),
    };
    format!("{mesh} @ {:?}", shape.to_array())
}

/// Objects that move while the game runs aren't baked: they get probes.
pub fn moves(object: &Object) -> bool {
    object.is_dynamic()
}

/// Light arriving from six directions (+X, -X, +Y, -Y, +Z, -Z).
pub type AmbientCube = [Vec3; 6];

/// A zone's probes on a regular grid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeGrid {
    pub origin: Vec3,
    pub spacing: f32,
    pub dims: [u32; 3],
    /// `None` = inside geometry, ignored.
    pub probes: Vec<Option<AmbientCube>>,
}

impl ProbeGrid {
    /// Light at a point, blended from the eight probes around it (skipping
    /// ones inside walls). `None` if none of them is usable.
    pub fn sample(&self, p: Vec3) -> Option<AmbientCube> {
        if self.probes.is_empty() {
            return None;
        }
        let max = Vec3::new(
            (self.dims[0] - 1) as f32,
            (self.dims[1] - 1) as f32,
            (self.dims[2] - 1) as f32,
        );
        let g = ((p - self.origin) / self.spacing).clamp(Vec3::ZERO, max);
        let base = g.floor().min((max - 1.0).max(Vec3::ZERO));
        let f = g - base;
        let mut sum = [Vec3::ZERO; 6];
        let mut total = 0.0;
        for corner in 0..8u32 {
            let offset = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
            let index: [u32; 3] =
                std::array::from_fn(|a| (base[a] as u32 + offset[a]).min(self.dims[a] - 1));
            let w: f32 = (0..3)
                .map(|a| if offset[a] == 1 { f[a] } else { 1.0 - f[a] })
                .product();
            let i = index[0] + self.dims[0] * (index[1] + self.dims[1] * index[2]);
            if let Some(Some(cube)) = self.probes.get(i as usize)
                && w > 0.0
            {
                for d in 0..6 {
                    sum[d] += cube[d] * w;
                }
                total += w;
            }
        }
        (total > 1e-4).then(|| sum.map(|s| s / total))
    }
}

/// What glossy surfaces in a zone reflect: a cube of the room seen from
/// the middle of its volumes, and the box it's projected onto (so
/// reflections line up with the walls instead of sitting at infinity).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reflection {
    pub center: Vec3,
    pub min: Vec3,
    pub max: Vec3,
    /// RGB9E5, six faces of `REFLECTION_SIZE`² (+X, -X, +Y, -Y, +Z, -Z), each
    /// row by row.
    pub texels: Vec<u32>,
}

/// The direction through texel (x, y) of a cube face, in the GPU's
/// cube-map convention (+X, -X, +Y, -Y, +Z, -Z; rows top to bottom).
pub fn cube_direction(face: usize, x: u32, y: u32, size: u32) -> Vec3 {
    let u = 2.0 * (x as f32 + 0.5) / size as f32 - 1.0;
    let v = 2.0 * (y as f32 + 0.5) / size as f32 - 1.0;
    match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize()
}

/// One zone's bake.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BakedZone {
    pub name: String,
    pub size: [u32; 2],
    /// RGB9E5 texels, row by row.
    pub texels: Vec<u32>,
    pub probes: ProbeGrid,
    /// `None` for zones without volumes.
    pub reflection: Option<Reflection>,
}

/// One mesh's lightmap layout, shared by its placements: the mesh split
/// where charts meet, with a UV over the layout (0–1) per vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedLayout {
    /// `mesh_key` of what it was made for, to tell whether a draw still
    /// fits it.
    pub mesh: String,
    /// New vertices: the original vertex and its layout UV (0–1).
    pub vertices: Vec<(u32, [f32; 2])>,
    pub indices: Vec<u32>,
}

/// One static draw: its layout and where that sits in its zone's atlas.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedDraw {
    pub key: DrawKey,
    /// Index into `Baked::zones`.
    pub zone: u32,
    /// Index into `Baked::layouts`.
    pub layout: u32,
    /// Atlas UV = layout UV * `[0..2]` + `[2..4]`.
    pub rect: [f32; 4],
}

impl BakedLayout {
    /// The mesh re-made for its lightmap, and the UV per new vertex; `None`
    /// if the mesh no longer matches (edited since the bake).
    pub fn apply(&self, mesh: &MeshData) -> Option<(MeshData, Vec<[f32; 2]>)> {
        if self.indices.len() != mesh.indices.len()
            || self
                .vertices
                .iter()
                .any(|&(v, _)| v as usize >= mesh.vertices.len())
            || self
                .indices
                .iter()
                .any(|&i| i as usize >= self.vertices.len())
        {
            return None;
        }
        let vertices = self
            .vertices
            .iter()
            .map(|&(v, _)| mesh.vertices[v as usize])
            .collect();
        let uvs = self.vertices.iter().map(|&(_, uv)| uv).collect();
        Some((
            MeshData {
                vertices,
                indices: self.indices.clone(),
            },
            uvs,
        ))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Baked {
    pub signature: u64,
    pub zones: Vec<BakedZone>,
    pub layouts: Vec<BakedLayout>,
    pub draws: Vec<BakedDraw>,
}

impl Baked {
    pub fn zone(&self, name: &str) -> Option<&BakedZone> {
        self.zones.iter().find(|z| z.name == name)
    }
}

/// Where a scene's lightmap lives.
pub fn path_for(scene_file: &Path) -> PathBuf {
    scene_file.with_extension("lightmap")
}

/// Fingerprint of everything the bake depends on.
pub fn signature(scene: &Scene) -> u64 {
    let mut hasher = DefaultHasher::new();
    // How layouts are made is part of the bake too.
    LAYOUT_VERSION.hash(&mut hasher);
    let env = &scene.environment;
    format!(
        "{:?}{:?}{:?}{:?}{}{}",
        env.sun_direction,
        env.sun_color,
        env.ambient_sky,
        env.ambient_ground,
        env.lightmap_density,
        env.bake_samples
    )
    .hash(&mut hasher);
    for object in &scene.objects {
        if moves(object) {
            continue;
        }
        for c in &object.components {
            match c {
                Component::Primitive(_)
                | Component::Model(_)
                | Component::ZoneVolume(_)
                | Component::LightSource(_)
                | Component::Material(_) => {
                    // The id, not the name: renaming changes no light.
                    format!("{}{}{:?}{:?}", object.id, object.zone, object.transform, c)
                        .hash(&mut hasher);
                }
                _ => {}
            }
        }
    }
    hasher.finish()
}

/// Bumped when layouts are made differently, so older bakes read as
/// outdated.
const LAYOUT_VERSION: u32 = 1;

/// One mesh's layout in the making: unwrapped once, in its own space with
/// the shape of its scale, at the density its smallest placement needs.
struct Template {
    mesh_key: String,
    mesh: MeshData,
    shape: Vec3,
    /// The smallest placement's size (see `split_scale`).
    smallest: f32,
    unwrapped: Unwrapped,
    /// The layout's size in texels at its density.
    size: [u32; 2],
}

/// A static draw: where its mesh is in the world (traced), its layout,
/// and its rectangle in the zone's atlas.
struct Placement {
    key: DrawKey,
    zone: usize,
    template: usize,
    /// Its size (see `split_scale`).
    size_factor: f32,
    /// World corners and normal of each of the mesh's triangles.
    world: Vec<([Vec3; 3], Vec3)>,
    /// Its rectangle: size and top-left corner in the atlas (texels).
    size: [u32; 2],
    offset: [u32; 2],
}

/// A chart's texels in a placement: the atlas texels whose centers fall in
/// the chart's part of the placement's rectangle.
struct Region {
    origin: [u32; 2],
    size: [u32; 2],
    /// Atlas texels per layout texel, along x and y.
    stretch: Vec2,
}

fn region(placement: &Placement, template: &Template, chart: &unwrap::Chart) -> Region {
    let stretch = Vec2::new(
        placement.size[0] as f32 / template.size[0].max(1) as f32,
        placement.size[1] as f32 / template.size[1].max(1) as f32,
    );
    let offset = Vec2::new(placement.offset[0] as f32, placement.offset[1] as f32);
    let rect = Vec2::new(chart.rect[0] as f32, chart.rect[1] as f32);
    let size = Vec2::new(chart.size[0] as f32, chart.size[1] as f32);
    let lo = (offset + rect * stretch - 0.5).ceil();
    let hi = (offset + (rect + size) * stretch - 0.5).ceil();
    Region {
        origin: [lo.x as u32, lo.y as u32],
        size: [(hi.x - lo.x) as u32, (hi.y - lo.y) as u32],
        stretch,
    }
}

/// Average linear color of an image.
fn average(image: &texture::Image) -> Vec3 {
    let pixels = &image.mips[0];
    let count = (pixels.len() / 4).max(1) as f32;
    let sum: Vec3 = pixels
        .chunks_exact(4)
        .map(|p| {
            Vec3::new(
                srgb_to_linear(p[0]),
                srgb_to_linear(p[1]),
                srgb_to_linear(p[2]),
            )
        })
        .sum();
    sum / count
}

/// How a bake is going.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// What's being baked ("LIGHTMAPS", "PROBES").
    pub stage: &'static str,
    /// 0–1.
    pub fraction: f32,
}

/// A bake running in the background.
pub struct BakeJob {
    progress: Arc<Mutex<Progress>>,
    thread: Option<JoinHandle<Result<PathBuf, LightmapError>>>,
}

impl BakeJob {
    pub fn progress(&self) -> Progress {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// The result once done (the file written); `None` while it runs.
    /// Returns it only once.
    pub fn finished(&mut self) -> Option<Result<PathBuf, LightmapError>> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.thread
                .take()?
                .join()
                .unwrap_or(Err(LightmapError::Crashed)),
        )
    }
}

/// Starts baking `scene` into `file` on a background thread.
pub fn start(scene: &Scene, assets: &mut Assets, file: PathBuf) -> Result<BakeJob, LightmapError> {
    let started = std::time::Instant::now();
    let env = &scene.environment;
    let density = env.lightmap_density.clamp(0.25, 64.0);
    let grid = average(&texture::grid_image());
    let keys = draw_keys(scene);
    let mut placements: Vec<Placement> = Vec::new();
    let mut templates: Vec<Template> = Vec::new();
    let mut template_of: std::collections::HashMap<String, usize> = Default::default();
    let mut triangles: Vec<trace::Triangle> = Vec::new();
    // Each object's world-space bounding radius around its position, for
    // lights sitting inside their own object.
    let mut reach: Vec<f32> = vec![0.0; scene.objects.len()];
    // `keys` has one per draw of every object, hidden ones too.
    for (r, key) in scene.renderables_with_hidden().zip(keys) {
        let object = &scene.objects[r.object];
        if moves(object) || object.hidden {
            continue;
        }
        let model = r.transform.matrix();
        let resolved = assets.resolve(r.geometry);
        if matches!(&resolved, Resolved::Model(m) if m.parts.is_empty()) {
            continue;
        }
        let (mesh, albedo): (MeshData, Vec<Vec3>) = match resolved {
            Resolved::Shape(shape) => {
                let mesh = MeshData::from_shape(shape);
                let n = mesh.indices.len() / 3;
                let texture = r
                    .material
                    .filter(|m| !m.texture.is_empty())
                    .and_then(|m| assets.texture(&m.texture))
                    .map_or(grid, |image| average(&image));
                (mesh, vec![texture * r.color; n])
            }
            Resolved::Model(model_data) => {
                let colors: Vec<Vec3> = model_data.images.iter().map(average).collect();
                let mut albedo = vec![r.color; model_data.mesh.indices.len() / 3];
                for part in &model_data.parts {
                    for t in part.indices.start / 3..part.indices.end / 3 {
                        if let Some(a) = albedo.get_mut(t as usize) {
                            *a = colors.get(part.image).copied().unwrap_or(Vec3::ONE) * r.color;
                        }
                    }
                }
                (model_data.mesh.clone(), albedo)
            }
        };
        for v in &mesh.vertices {
            let d = model
                .transform_point3(Vec3::from(v.position))
                .distance(r.transform.position);
            reach[r.object] = reach[r.object].max(d);
        }
        let (size_factor, shape) = split_scale(r.transform.scale);
        if size_factor == 0.0 {
            continue; // scaled flat to nothing
        }
        let world = world_triangles(&mesh, model);
        for (t, (corners, _)) in world.iter().enumerate() {
            triangles.push((*corners, albedo[t], r.emissive));
        }
        let zone = scene
            .zones
            .iter()
            .position(|z| z.name == object.zone)
            .unwrap_or(0);
        let mesh_key = mesh_key(r.geometry, r.transform.scale);
        let template = *template_of.entry(mesh_key.clone()).or_insert_with(|| {
            templates.push(Template {
                mesh_key,
                mesh,
                shape,
                smallest: size_factor,
                unwrapped: Unwrapped::default(),
                size: [0, 0],
            });
            templates.len() - 1
        });
        let smallest = &mut templates[template].smallest;
        *smallest = smallest.min(size_factor);
        placements.push(Placement {
            key,
            zone,
            template,
            size_factor,
            world,
            size: [0, 0],
            offset: [0, 0],
        });
    }
    if placements.is_empty() {
        return Err(LightmapError::Empty);
    }
    // One unwrap per layout, in the mesh's space with the shape of its
    // scale, at the density its smallest placement needs; each placement
    // gets the layout scaled up by how much bigger it is.
    for t in &mut templates {
        let scale = Mat4::from_scale(t.shape);
        t.unwrapped = unwrap::unwrap(&t.mesh, scale, density * t.smallest);
        t.size = unwrap::pack_layout(&mut t.unwrapped.charts);
        t.mesh = MeshData::default();
    }
    for p in &mut placements {
        let t = &templates[p.template];
        let biggest = t.size[0].max(t.size[1]).max(1) as f32;
        let factor = (p.size_factor / t.smallest)
            .min(MAX_PLACEMENT / biggest)
            .max(1.0);
        // A hair under, so a factor of exactly 1 isn't rounded up.
        p.size = t
            .size
            .map(|s| ((s as f32 * factor) - 1e-3).ceil().max(1.0) as u32);
    }
    let lights = scene
        .lights(|path, part| assets.model(path)?.part_center(part))
        .into_iter()
        .filter(|(i, l, _)| l.baked && !moves(&scene.objects[*i]))
        .map(|(i, l, p)| BakeLight {
            position: p,
            color: l.color * l.intensity,
            range: l.range,
            clearance: reach[i] + 0.01,
        })
        .collect();
    let world = World::new(
        triangles,
        env.sun_direction,
        env.sun_color,
        env.ambient_sky,
        env.ambient_ground,
        lights,
    );
    let probe_grids: Vec<ProbeGrid> = (0..scene.zones.len())
        .map(|z| probe_layout(scene, z))
        .collect();
    let zone_names: Vec<String> = scene.zones.iter().map(|z| z.name.clone()).collect();
    let samples = env.bake_samples.clamp(16, 4096);
    let signature = signature(scene);

    let progress = Arc::new(Mutex::new(Progress::default()));
    let shared = progress.clone();
    let thread = std::thread::Builder::new()
        .name("light bake".into())
        .spawn(move || {
            let baked = bake(
                world,
                templates,
                placements,
                probe_grids,
                zone_names,
                samples,
                signature,
                &shared,
            );
            save(&baked, &file)?;
            let texels: u64 = baked.zones.iter().map(|z| z.texels.len() as u64).sum();
            log::info!(
                "light bake: {:.1} s, atlases {:?}, {:.2} M texels, file {:.2} MB",
                started.elapsed().as_secs_f32(),
                baked.zones.iter().map(|z| z.size).collect::<Vec<_>>(),
                texels as f64 / 1e6,
                std::fs::metadata(&file).map_or(0, |m| m.len()) as f64 / 1e6,
            );
            Ok(file)
        })
        .map_err(|source| LightmapError::Io {
            path: PathBuf::from("light bake thread"),
            source,
        })?;
    Ok(BakeJob {
        progress,
        thread: Some(thread),
    })
}

/// Probe positions for a zone: a grid over the box around its volumes.
fn probe_layout(scene: &Scene, zone: usize) -> ProbeGrid {
    let name = &scene.zones[zone].name;
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for object in scene.objects.iter().filter(|o| &o.zone == name) {
        for c in &object.components {
            if let Component::ZoneVolume(v) = c {
                let m = object.transform.matrix() * Mat4::from_scale(v.size);
                for i in 0..8 {
                    let unit = Vec3::new(
                        (i & 1) as f32 - 0.5,
                        ((i >> 1) & 1) as f32 - 0.5,
                        ((i >> 2) & 1) as f32 - 0.5,
                    );
                    let p = m.transform_point3(unit);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
    }
    if lo.cmpgt(hi).any() {
        return ProbeGrid::default();
    }
    let extent = hi - lo;
    let mut spacing = PROBE_SPACING;
    let dims = loop {
        let d = (extent / spacing).ceil().as_uvec3() + 1;
        if (d.x * d.y * d.z) as usize <= MAX_PROBES {
            break [d.x, d.y, d.z];
        }
        spacing *= 1.25;
    };
    // Centered in the box.
    let used = Vec3::new(
        (dims[0] - 1) as f32,
        (dims[1] - 1) as f32,
        (dims[2] - 1) as f32,
    ) * spacing;
    ProbeGrid {
        origin: lo + (extent - used) * 0.5,
        spacing,
        dims,
        probes: Vec::new(),
    }
}

/// Runs `job(i)` for i in 0..count on every core; returns results in order.
fn parallel<T: Send>(
    count: usize,
    job: impl Fn(usize) -> T + Sync,
    done: impl Fn(usize) + Sync,
) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..count).map(|_| None).collect());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= count {
                        break;
                    }
                    let value = job(i);
                    if let Ok(mut r) = results.lock() {
                        r[i] = Some(value);
                    }
                    done(finished.fetch_add(1, Ordering::Relaxed) + 1);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.expect("every job ran"))
        .collect()
}

/// A chart's texels: light per texel, `None` where it's inside geometry.
/// Traces one chart's texels in one placement (see `region`): each texel
/// center is found on the layout's chart, then on the placement's world
/// triangle there.
fn bake_chart(
    world: &World,
    placement: &Placement,
    template: &Template,
    chart: usize,
    samples: u32,
    seed: u64,
) -> Vec<Option<Vec3>> {
    let mut rng = Rng::new(seed);
    let chart = &template.unwrapped.charts[chart];
    let r = region(placement, template, chart);
    let [w, h] = r.size;
    let tris: Vec<([Vec2; 3], [Vec3; 3], Vec3)> = chart
        .triangles
        .iter()
        .map(|&t| {
            let (layout, _) = template.unwrapped.triangles[t];
            let (p, n) = placement.world[t];
            (layout.map(|c| chart.project(c)), p, n)
        })
        .filter(|(_, _, n)| n.length_squared() > 0.5)
        .collect();
    let offset = Vec2::new(placement.offset[0] as f32, placement.offset[1] as f32);
    let origin = Vec2::new(r.origin[0] as f32, r.origin[1] as f32);
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let center = origin + Vec2::new(x as f32, y as f32) + 0.5;
            let q = chart.plane_at((center - offset) / r.stretch);
            // The surface point for this texel: inside a triangle, or the
            // nearest point on one (for texels along the edges).
            let best = tris
                .iter()
                .map(|(q2, p3, n)| {
                    let (bary, d) = closest_in_triangle(q, q2);
                    let mut p = p3[0] * bary.x + p3[1] * bary.y + p3[2] * bary.z;
                    // Points on an edge sit exactly where this surface meets
                    // the next one (a wall on a floor); rays from there can
                    // slip through. Nudge them a little into the triangle.
                    let center = (p3[0] + p3[1] + p3[2]) / 3.0;
                    let inward = center - p;
                    p += inward.normalize_or_zero() * inward.length().min(EDGE_INSET);
                    (d, p, *n)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            out.push(best.and_then(|(_, p, n)| world.gather(p, n, samples, &mut rng)));
        }
    }
    out
}

/// Barycentric coordinates of the point in a 2D triangle nearest to `q`,
/// and the distance to it (0 inside).
fn closest_in_triangle(q: Vec2, t: &[Vec2; 3]) -> (Vec3, f32) {
    let bary = |p: Vec2| {
        let v0 = t[1] - t[0];
        let v1 = t[2] - t[0];
        let v2 = p - t[0];
        let d = v0.perp_dot(v1);
        if d.abs() < 1e-12 {
            return Vec3::new(1.0, 0.0, 0.0);
        }
        let b1 = v2.perp_dot(v1) / d;
        let b2 = v0.perp_dot(v2) / d;
        Vec3::new(1.0 - b1 - b2, b1, b2)
    };
    let b = bary(q);
    if b.min_element() >= 0.0 {
        return (b, 0.0);
    }
    let mut best = (Vec3::ZERO, f32::MAX);
    for i in 0..3 {
        let (a, c) = (t[i], t[(i + 1) % 3]);
        let edge = c - a;
        let s = ((q - a).dot(edge) / edge.length_squared().max(1e-12)).clamp(0.0, 1.0);
        let p = a + edge * s;
        let d = p.distance(q);
        if d < best.1 {
            let mut w = Vec3::ZERO;
            w[i] = 1.0 - s;
            w[(i + 1) % 3] = s;
            best = (w, d);
        }
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn bake(
    world: World,
    mut templates: Vec<Template>,
    mut placements: Vec<Placement>,
    mut grids: Vec<ProbeGrid>,
    zone_names: Vec<String>,
    samples: u32,
    signature: u64,
    progress: &Mutex<Progress>,
) -> Baked {
    let set = |stage, fraction| {
        if let Ok(mut p) = progress.lock() {
            *p = Progress { stage, fraction };
        }
    };
    // Pack each zone. A mesh placed once shares nothing: its charts go
    // straight into the atlas, and its layout is their bounding box there.
    // A mesh placed more often gets its layout as one rectangle per
    // placement.
    let mut uses = vec![0usize; templates.len()];
    for p in &placements {
        uses[p.template] += 1;
    }
    let mut sizes = vec![[0u32; 2]; zone_names.len()];
    for (zone, size) in sizes.iter_mut().enumerate() {
        let mut items: Vec<(usize, Option<usize>)> = Vec::new();
        for (i, p) in placements
            .iter()
            .enumerate()
            .filter(|(_, p)| p.zone == zone)
        {
            if uses[p.template] == 1 {
                let charts = templates[p.template].unwrapped.charts.len();
                items.extend((0..charts).map(|c| (i, Some(c))));
            } else {
                items.push((i, None));
            }
        }
        let rects: Vec<[u32; 2]> = items
            .iter()
            .map(|&(i, chart)| match chart {
                Some(c) => templates[placements[i].template].unwrapped.charts[c].size,
                None => placements[i].size,
            })
            .collect();
        let (corners, packed) = unwrap::pack_sizes(&rects, true);
        let mut boxes: std::collections::HashMap<usize, ([u32; 2], [u32; 2])> = Default::default();
        for (&(i, chart), (corner, rect)) in items.iter().zip(corners.into_iter().zip(&rects)) {
            match chart {
                None => placements[i].offset = corner,
                Some(c) => {
                    templates[placements[i].template].unwrapped.charts[c].rect = corner;
                    let far = [corner[0] + rect[0], corner[1] + rect[1]];
                    let b = boxes.entry(i).or_insert((corner, far));
                    b.0 = [b.0[0].min(corner[0]), b.0[1].min(corner[1])];
                    b.1 = [b.1[0].max(far[0]), b.1[1].max(far[1])];
                }
            }
        }
        for (i, (lo, hi)) in boxes {
            let p = &mut placements[i];
            p.offset = lo;
            p.size = [hi[0] - lo[0], hi[1] - lo[1]];
            let t = &mut templates[p.template];
            t.size = p.size;
            for chart in &mut t.unwrapped.charts {
                chart.rect = [chart.rect[0] - lo[0], chart.rect[1] - lo[1]];
            }
        }
        *size = packed;
    }
    let area = |s: [u32; 2]| s[0] as u64 * s[1] as u64;
    let charts: u64 = placements
        .iter()
        .map(|p| {
            let t = &templates[p.template];
            let stretch = area(p.size) as f64 / area(t.size).max(1) as f64;
            let own: u64 = t.unwrapped.charts.iter().map(|c| area(c.size)).sum();
            (own as f64 * stretch) as u64
        })
        .sum();
    log::info!(
        "light bake layout: {} layouts for {} draws; texels in charts {:.2} M, in atlases {:.2} M",
        templates.len(),
        placements.len(),
        charts as f64 / 1e6,
        sizes.iter().map(|&s| area(s)).sum::<u64>() as f64 / 1e6,
    );
    // Trace every chart's texels, in every placement.
    let jobs: Vec<(usize, usize)> = placements
        .iter()
        .enumerate()
        .flat_map(|(i, p)| (0..templates[p.template].unwrapped.charts.len()).map(move |c| (i, c)))
        .collect();
    let chart = |i: usize, c: usize| &templates[placements[i].template].unwrapped.charts[c];
    let total_texels: usize = jobs
        .iter()
        .map(|&(i, c)| {
            let r = region(
                &placements[i],
                &templates[placements[i].template],
                chart(i, c),
            );
            (r.size[0] * r.size[1]) as usize
        })
        .sum();
    let texels_done = AtomicUsize::new(0);
    set("LIGHTMAPS", 0.0);
    let results = parallel(
        jobs.len(),
        |j| {
            let (i, c) = jobs[j];
            let p = &placements[i];
            let texels = bake_chart(&world, p, &templates[p.template], c, samples, j as u64 + 1);
            let done = texels_done.fetch_add(texels.len(), Ordering::Relaxed) + texels.len();
            set("LIGHTMAPS", done as f32 / total_texels.max(1) as f32);
            texels
        },
        |_| {},
    );

    // Assemble atlases: fill texels inside geometry from their neighbours,
    // then smooth each chart a little against noise.
    let mut atlases: Vec<Vec<Option<Vec3>>> = sizes
        .iter()
        .map(|s| vec![None; (s[0] * s[1]) as usize])
        .collect();
    for (&(i, c), texels) in jobs.iter().zip(&results) {
        let p = &placements[i];
        let r = region(p, &templates[p.template], chart(i, c));
        let mut local = texels.clone();
        let [w, h] = r.size;
        dilate(&mut local, w, h);
        let smooth = blur(&local, w, h);
        let atlas = &mut atlases[p.zone];
        let [width, height] = sizes[p.zone];
        for y in 0..h {
            for x in 0..w {
                let (ax, ay) = (r.origin[0] + x, r.origin[1] + y);
                if ax < width && ay < height {
                    atlas[(ay * width + ax) as usize] = smooth[(y * w + x) as usize];
                }
            }
        }
    }

    // Reflection cubes: one per zone with volumes, traced from the middle of
    // the box around them.
    let boxes: Vec<Option<(Vec3, Vec3)>> = grids
        .iter()
        .map(|g| {
            (!g.dims.contains(&0) && g.spacing > 0.0).then(|| {
                let extent = Vec3::new(
                    (g.dims[0] - 1) as f32,
                    (g.dims[1] - 1) as f32,
                    (g.dims[2] - 1) as f32,
                ) * g.spacing;
                (g.origin, g.origin + extent)
            })
        })
        .collect();
    let face_texels = (REFLECTION_SIZE * REFLECTION_SIZE) as usize;
    let reflection_jobs: Vec<(usize, usize)> = boxes
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_some())
        .flat_map(|(z, _)| (0..6 * REFLECTION_SIZE as usize).map(move |row| (z, row)))
        .collect();
    set("REFLECTIONS", 0.0);
    let rows = reflection_jobs.len();
    let reflection_rows = parallel(
        rows,
        |j| {
            let (z, row) = reflection_jobs[j];
            let (lo, hi) = boxes[z].expect("filtered");
            let center = (lo + hi) * 0.5;
            let (face, y) = (row / REFLECTION_SIZE as usize, row as u32 % REFLECTION_SIZE);
            let mut rng = Rng::new(0x51ed_270b ^ j as u64);
            (0..REFLECTION_SIZE)
                .map(|x| {
                    let dir = cube_direction(face, x, y, REFLECTION_SIZE);
                    rgb9e5(world.radiance(center, dir, REFLECTION_SAMPLES, &mut rng))
                })
                .collect::<Vec<u32>>()
        },
        |done| set("REFLECTIONS", done as f32 / rows.max(1) as f32),
    );
    let mut reflections: Vec<Option<Reflection>> = boxes
        .iter()
        .map(|b| {
            b.map(|(lo, hi)| Reflection {
                center: (lo + hi) * 0.5,
                min: lo,
                max: hi,
                texels: Vec::with_capacity(face_texels * 6),
            })
        })
        .collect();
    for (&(z, _), row) in reflection_jobs.iter().zip(reflection_rows) {
        if let Some(r) = &mut reflections[z] {
            r.texels.extend(row);
        }
    }

    // Probes.
    let probe_jobs: Vec<(usize, usize)> = grids
        .iter()
        .enumerate()
        .flat_map(|(z, g)| (0..(g.dims[0] * g.dims[1] * g.dims[2]) as usize).map(move |p| (z, p)))
        .collect();
    set("PROBES", 0.0);
    let count = probe_jobs.len();
    let cubes = parallel(
        count,
        |j| {
            let (z, p) = probe_jobs[j];
            let g = &grids[z];
            let (x, y, k) = (
                p as u32 % g.dims[0],
                (p as u32 / g.dims[0]) % g.dims[1],
                p as u32 / (g.dims[0] * g.dims[1]),
            );
            let position = g.origin + Vec3::new(x as f32, y as f32, k as f32) * g.spacing;
            let mut rng = Rng::new(0x9e37_79b9 ^ j as u64);
            let axes = [
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z,
            ];
            let mut cube = [Vec3::ZERO; 6];
            for (d, axis) in axes.iter().enumerate() {
                cube[d] = world.gather(position, *axis, PROBE_SAMPLES, &mut rng)?;
            }
            Some(cube)
        },
        |done| set("PROBES", done as f32 / count.max(1) as f32),
    );
    for (&(z, _), cube) in probe_jobs.iter().zip(cubes) {
        grids[z].probes.push(cube);
    }

    let zones = zone_names
        .into_iter()
        .zip(sizes.iter().copied())
        .zip(atlases)
        .zip(grids)
        .zip(reflections)
        .map(|((((name, size), atlas), probes), reflection)| BakedZone {
            name,
            size,
            texels: atlas
                .into_iter()
                .map(|t| rgb9e5(t.unwrap_or(Vec3::ZERO)))
                .collect(),
            probes,
            reflection,
        })
        .collect();
    let layouts = templates
        .into_iter()
        .map(|t| {
            let size = Vec2::new(t.size[0].max(1) as f32, t.size[1].max(1) as f32);
            BakedLayout {
                vertices: t
                    .unwrapped
                    .vertices
                    .iter()
                    .map(|&(v, chart, q)| {
                        (v, (t.unwrapped.charts[chart].texel_of(q) / size).into())
                    })
                    .collect(),
                indices: t.unwrapped.indices,
                mesh: t.mesh_key,
            }
        })
        .collect();
    let draws = placements
        .into_iter()
        .map(|p| {
            let s = sizes[p.zone];
            let atlas = Vec2::new(s[0].max(1) as f32, s[1].max(1) as f32);
            let scale = Vec2::new(p.size[0] as f32, p.size[1] as f32) / atlas;
            let offset = Vec2::new(p.offset[0] as f32, p.offset[1] as f32) / atlas;
            BakedDraw {
                key: p.key,
                zone: p.zone as u32,
                layout: p.template as u32,
                rect: [scale.x, scale.y, offset.x, offset.y],
            }
        })
        .collect();
    Baked {
        signature,
        zones,
        layouts,
        draws,
    }
}

/// Each of the mesh's triangles placed by `model`: corners and normal.
fn world_triangles(mesh: &MeshData, model: Mat4) -> Vec<([Vec3; 3], Vec3)> {
    mesh.indices
        .chunks_exact(3)
        .map(|t| {
            let p = [t[0], t[1], t[2]]
                .map(|i| model.transform_point3(Vec3::from(mesh.vertices[i as usize].position)));
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            (p, n)
        })
        .collect()
}

/// Texels inside geometry take the average of their usable neighbours,
/// repeatedly, so filtering near walls doesn't pull in black.
fn dilate(texels: &mut [Option<Vec3>], w: u32, h: u32) {
    for _ in 0..(w.max(h)) {
        if texels.iter().all(Option::is_some) || texels.iter().all(Option::is_none) {
            return;
        }
        let before = texels.to_vec();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if before[(y * w as i32 + x) as usize].is_some() {
                    continue;
                }
                let (mut sum, mut n) = (Vec3::ZERO, 0);
                for (dx, dy) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, 1),
                    (-1, 1),
                    (1, -1),
                ] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    if let Some(v) = before[(ny * w as i32 + nx) as usize] {
                        sum += v;
                        n += 1;
                    }
                }
                if n > 0 {
                    texels[(y * w as i32 + x) as usize] = Some(sum / n as f32);
                }
            }
        }
    }
}

/// A 3×3 weighted blur within one chart.
fn blur(texels: &[Option<Vec3>], w: u32, h: u32) -> Vec<Option<Vec3>> {
    let mut out = texels.to_vec();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (mut sum, mut total) = (Vec3::ZERO, 0.0);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    if let Some(v) = texels[(ny * w as i32 + nx) as usize] {
                        let weight = [4.0, 2.0, 1.0][(dx.abs() + dy.abs()) as usize];
                        sum += v * weight;
                        total += weight;
                    }
                }
            }
            if total > 0.0 {
                out[(y * w as i32 + x) as usize] = Some(sum / total);
            }
        }
    }
    out
}

/// Packs a linear HDR color into RGB9E5: three 9-bit mantissas sharing a
/// 5-bit exponent, the GPU's compact format for HDR textures.
pub fn rgb9e5(c: Vec3) -> u32 {
    const MAX: f32 = 65408.0;
    let c = c.clamp(Vec3::ZERO, Vec3::splat(MAX));
    let max = c.max_element();
    if max < 1e-7 {
        return 0;
    }
    let mut exp = (max.log2().floor() as i32).max(-16) + 16;
    let mut denom = 2f32.powi(exp - 24);
    if (max / denom + 0.5).floor() >= 512.0 {
        denom *= 2.0;
        exp += 1;
    }
    let m = |v: f32| ((v / denom + 0.5).floor() as u32).min(511);
    m(c.x) | (m(c.y) << 9) | (m(c.z) << 18) | ((exp as u32) << 27)
}

/// Unpacks an RGB9E5 color.
pub fn from_rgb9e5(v: u32) -> Vec3 {
    let scale = 2f32.powi((v >> 27) as i32 - 24);
    Vec3::new(
        (v & 511) as f32,
        ((v >> 9) & 511) as f32,
        ((v >> 18) & 511) as f32,
    ) * scale
}

// The file: little-endian, see the module docs.

struct Writer(Vec<u8>);

impl Writer {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn vec3(&mut self, v: Vec3) {
        for c in v.to_array() {
            self.f32(c);
        }
    }
    fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.bytes(n)?.to_vec()).ok()
    }
    /// A count that can't be bigger than what's left in the file.
    fn count(&mut self, item: usize) -> Option<usize> {
        let n = self.u32()? as usize;
        (n.saturating_mul(item) <= self.0.len()).then_some(n)
    }
}

fn encode(baked: &Baked) -> Vec<u8> {
    encode_as(baked, MAGIC)
}

/// The file with another format's mark (an older one, for tests): what
/// follows the zones is only read for the current one.
fn encode_as(baked: &Baked, magic: &[u8; 8]) -> Vec<u8> {
    let mut w = Writer(magic.to_vec());
    w.0.extend_from_slice(&baked.signature.to_le_bytes());
    w.u32(baked.zones.len() as u32);
    for z in &baked.zones {
        w.string(&z.name);
        w.u32(z.size[0]);
        w.u32(z.size[1]);
        for &t in &z.texels {
            w.u32(t);
        }
        let g = &z.probes;
        w.vec3(g.origin);
        w.f32(g.spacing);
        for d in g.dims {
            w.u32(d);
        }
        w.u32(g.probes.len() as u32);
        for probe in &g.probes {
            match probe {
                Some(cube) => {
                    w.0.push(1);
                    for c in cube {
                        w.vec3(*c);
                    }
                }
                None => w.0.push(0),
            }
        }
        match &z.reflection {
            Some(r) => {
                w.0.push(1);
                w.vec3(r.center);
                w.vec3(r.min);
                w.vec3(r.max);
                for &t in &r.texels {
                    w.u32(t);
                }
            }
            None => w.0.push(0),
        }
    }
    w.u32(baked.layouts.len() as u32);
    for l in &baked.layouts {
        w.string(&l.mesh);
        w.u32(l.vertices.len() as u32);
        for &(v, uv) in &l.vertices {
            w.u32(v);
            w.f32(uv[0]);
            w.f32(uv[1]);
        }
        w.u32(l.indices.len() as u32);
        for &i in &l.indices {
            w.u32(i);
        }
    }
    w.u32(baked.draws.len() as u32);
    for d in &baked.draws {
        w.u32(d.key.id);
        w.u32(d.key.part);
        w.u32(d.zone);
        w.u32(d.layout);
        for v in d.rect {
            w.f32(v);
        }
    }
    w.0
}

fn decode(bytes: &[u8]) -> Option<Baked> {
    let mut r = Reader(bytes);
    let magic = r.bytes(8)?;
    let current = magic == MAGIC;
    if !current && !OLD_MAGICS.iter().any(|m| magic == *m) {
        return None;
    }
    let signature = r.u64()?;
    let mut zones = Vec::new();
    for _ in 0..r.count(4)? {
        let name = r.string()?;
        let size = [r.u32()?, r.u32()?];
        let n = (size[0] as usize).checked_mul(size[1] as usize)?;
        if n * 4 > r.0.len() {
            return None;
        }
        let texels = (0..n).map(|_| r.u32()).collect::<Option<_>>()?;
        let origin = r.vec3()?;
        let spacing = r.f32()?;
        let dims = [r.u32()?, r.u32()?, r.u32()?];
        let count = r.count(1)?;
        let mut probes = Vec::with_capacity(count);
        for _ in 0..count {
            probes.push(match r.bytes(1)?[0] {
                0 => None,
                _ => Some([
                    r.vec3()?,
                    r.vec3()?,
                    r.vec3()?,
                    r.vec3()?,
                    r.vec3()?,
                    r.vec3()?,
                ]),
            });
        }
        if probes.len() != (dims[0] * dims[1] * dims[2]) as usize && !probes.is_empty() {
            return None;
        }
        let reflection = match r.bytes(1)?[0] {
            0 => None,
            _ => {
                let (center, min, max) = (r.vec3()?, r.vec3()?, r.vec3()?);
                let n = (6 * REFLECTION_SIZE * REFLECTION_SIZE) as usize;
                if n * 4 > r.0.len() {
                    return None;
                }
                let texels = (0..n).map(|_| r.u32()).collect::<Option<_>>()?;
                Some(Reflection {
                    center,
                    min,
                    max,
                    texels,
                })
            }
        };
        zones.push(BakedZone {
            name,
            size,
            texels,
            probes: ProbeGrid {
                origin,
                spacing,
                dims,
                probes,
            },
            reflection,
        });
    }
    if !current {
        // An older format: its zones' probes and reflections still light
        // moving objects and glossy surfaces, but its per-draw UVs don't fit
        // shared meshes. Nothing is lightmapped, and a signature no scene
        // has marks it outdated (BAKE ▸ LIGHTING again).
        return Some(Baked {
            signature: 0,
            zones,
            layouts: Vec::new(),
            draws: Vec::new(),
        });
    }
    let mut layouts = Vec::new();
    for _ in 0..r.count(4)? {
        let mesh = r.string()?;
        let vertices = (0..r.count(12)?)
            .map(|_| Some((r.u32()?, [r.f32()?, r.f32()?])))
            .collect::<Option<_>>()?;
        let indices = (0..r.count(4)?).map(|_| r.u32()).collect::<Option<_>>()?;
        layouts.push(BakedLayout {
            mesh,
            vertices,
            indices,
        });
    }
    let mut draws = Vec::new();
    for _ in 0..r.count(32)? {
        let key = DrawKey {
            id: r.u32()?,
            part: r.u32()?,
        };
        let zone = r.u32()?;
        let layout = r.u32()?;
        if layout as usize >= layouts.len() {
            return None;
        }
        let rect = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        draws.push(BakedDraw {
            key,
            zone,
            layout,
            rect,
        });
    }
    Some(Baked {
        signature,
        zones,
        layouts,
        draws,
    })
}

fn save(baked: &Baked, file: &Path) -> Result<(), LightmapError> {
    let io = |source| LightmapError::Io {
        path: file.to_owned(),
        source,
    };
    let mut f = std::fs::File::create(file).map_err(io)?;
    f.write_all(&encode(baked)).map_err(io)
}

/// The scene's bake, if it has one.
pub fn load(file: &Path) -> Result<Option<Baked>, LightmapError> {
    let mut bytes = Vec::new();
    match std::fs::File::open(file) {
        Ok(mut f) => f
            .read_to_end(&mut bytes)
            .map_err(|source| LightmapError::Io {
                path: file.to_owned(),
                source,
            })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LightmapError::Io {
                path: file.to_owned(),
                source,
            });
        }
    };
    decode(&bytes)
        .map(Some)
        .ok_or_else(|| LightmapError::Format(file.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::ZoneVolume;

    #[test]
    fn rgb9e5_round_trips_closely() {
        for c in [
            Vec3::ZERO,
            Vec3::new(0.01, 0.2, 0.7),
            Vec3::new(3.0, 1.0, 0.0),
            Vec3::splat(120.0),
        ] {
            let back = from_rgb9e5(rgb9e5(c));
            assert!(
                (back - c).abs().max_element() <= c.max_element() / 200.0 + 1e-6,
                "{c} {back}"
            );
        }
    }

    #[test]
    fn cube_faces_point_along_their_axes() {
        let axes = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ];
        for (face, axis) in axes.iter().enumerate() {
            let center = cube_direction(face, 31, 31, 64);
            assert!(center.dot(*axis) > 0.99, "{face} {center}");
        }
        // +Z face: x grows to the right (+X), rows grow downward (-Y).
        assert!(
            cube_direction(4, 63, 32, 64).x > 0.9 * cube_direction(4, 63, 32, 64).length() - 0.3
        );
        assert!(cube_direction(4, 32, 63, 64).y < 0.0);
    }

    /// Renaming or reordering objects keeps each one's key: bakes stay on
    /// their object.
    #[test]
    fn keys_follow_the_object_through_renames_and_reorders() {
        let mut scene = crate::scene::test_scene(vec![
            (Shape::Box { size: Vec3::ONE }, Vec3::ZERO),
            (Shape::Box { size: Vec3::ONE }, Vec3::X),
        ]);
        let before = draw_keys(&scene);
        scene.objects.reverse();
        scene.objects[0].name = "renamed".into();
        assert_eq!(draw_keys(&scene), [before[1], before[0]]);
    }

    /// Only bigger or smaller: the same layout. Stretched another way: a
    /// layout of its own.
    #[test]
    fn layouts_are_shared_by_mesh_and_shape_of_scale() {
        let cube = Geometry::Shape(Shape::Box { size: Vec3::ONE });
        let key = |scale| mesh_key(cube, scale);
        assert_eq!(key(Vec3::ONE), key(Vec3::splat(3.0)));
        assert_eq!(key(Vec3::new(1.0, 2.0, 1.0)), key(Vec3::new(2.0, 4.0, 2.0)));
        assert_ne!(key(Vec3::ONE), key(Vec3::new(1.0, 2.0, 1.0)));
        let wedge = Geometry::Shape(Shape::Wedge { size: Vec3::ONE });
        assert_ne!(key(Vec3::ONE), mesh_key(wedge, Vec3::ONE));
        assert_eq!(
            split_scale(Vec3::new(2.0, 1.0, 0.5)),
            (2.0, Vec3::new(1.0, 0.5, 0.25))
        );
    }

    /// A file from before shared layouts still loads: its zones (probes,
    /// reflections) but no draws, and it reads as outdated.
    #[test]
    fn older_files_keep_their_zones_but_not_their_draws() {
        let baked = Baked {
            signature: 7,
            zones: vec![BakedZone {
                name: "hall".into(),
                size: [1, 1],
                texels: vec![rgb9e5(Vec3::ONE)],
                ..Default::default()
            }],
            layouts: vec![BakedLayout {
                mesh: "box".into(),
                vertices: vec![(0, [0.5, 0.5])],
                indices: vec![0, 0, 0],
            }],
            draws: vec![BakedDraw {
                key: DrawKey { id: 3, part: 0 },
                zone: 0,
                layout: 0,
                rect: [0.5, 0.5, 0.0, 0.5],
            }],
        };
        assert_eq!(decode(&encode(&baked)).expect("loads"), baked);
        for old in OLD_MAGICS {
            let loaded = decode(&encode_as(&baked, old)).expect("loads");
            assert_eq!(loaded.zones, baked.zones);
            assert!(loaded.draws.is_empty() && loaded.layouts.is_empty());
            assert_eq!(loaded.signature, 0);
        }
        assert!(decode(b"LIGHTM01garbage").is_none());
    }

    #[test]
    fn probes_blend_and_skip_invalid_ones() {
        let grid = ProbeGrid {
            origin: Vec3::ZERO,
            spacing: 1.0,
            dims: [2, 1, 1],
            probes: vec![Some([Vec3::ONE; 6]), None],
        };
        // Halfway, but the second probe is inside a wall: all from the first.
        assert_eq!(grid.sample(Vec3::new(0.5, 0.0, 0.0)).unwrap()[0], Vec3::ONE);
        let empty = ProbeGrid {
            probes: vec![None, None],
            ..grid
        };
        assert!(empty.sample(Vec3::ZERO).is_none());
    }

    #[test]
    fn a_layout_rebuilds_the_mesh() {
        let mesh = MeshData::from_shape(Shape::Box { size: Vec3::ONE });
        let u = unwrap::unwrap(&mesh, Mat4::IDENTITY, 4.0);
        let layout = BakedLayout {
            mesh: "box".into(),
            vertices: u
                .vertices
                .iter()
                .map(|&(v, _, _)| (v, [0.0, 0.0]))
                .collect(),
            indices: u.indices.clone(),
        };
        let (rebuilt, uvs) = layout.apply(&mesh).unwrap();
        assert_eq!(rebuilt.indices.len(), mesh.indices.len());
        assert_eq!(uvs.len(), rebuilt.vertices.len());
        // A mesh with a different triangle count is not a match.
        let other = MeshData::from_shape(Shape::Wedge { size: Vec3::ONE });
        assert!(layout.apply(&other).is_none());
    }

    /// A small closed room with a hole in the roof: bakes end to end, the
    /// file round-trips, and the floor under the hole is brighter than in
    /// the corner.
    #[test]
    fn a_room_bakes_and_round_trips() {
        let mut scene = crate::scene::test_scene(vec![
            (
                Shape::Box {
                    size: Vec3::new(6.0, 0.2, 6.0),
                },
                Vec3::new(0.0, -0.1, 0.0),
            ),
            (
                Shape::Box {
                    size: Vec3::new(2.0, 0.2, 6.0),
                },
                Vec3::new(-2.0, 3.1, 0.0),
            ),
            (
                Shape::Box {
                    size: Vec3::new(2.0, 0.2, 6.0),
                },
                Vec3::new(2.0, 3.1, 0.0),
            ),
        ]);
        scene.environment.bake_samples = 64;
        scene.environment.lightmap_density = 2.0;
        scene.objects.push(Object {
            id: 0,
            hidden: false,
            name: "volume".into(),
            zone: "test".into(),
            transform: crate::scene::Transform::at(Vec3::new(0.0, 1.5, 0.0)),
            components: vec![Component::ZoneVolume(ZoneVolume {
                size: Vec3::new(6.0, 3.0, 6.0),
                ..Default::default()
            })],
        });
        let dir = std::env::temp_dir().join(format!("lightmap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("room.lightmap");
        let mut assets = Assets::new(dir.clone());
        let mut job = start(&scene, &mut assets, file.clone()).unwrap();
        let written = loop {
            if let Some(result) = job.finished() {
                break result.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let baked = load(&written).unwrap().unwrap();
        assert_eq!(baked.signature, signature(&scene));
        assert_eq!(baked.draws.len(), 3);
        // The two roof halves are the same box: one layout, two rectangles
        // of the same size side by side in the atlas.
        assert_eq!(baked.layouts.len(), 2);
        let (a, b) = (&baked.draws[1], &baked.draws[2]);
        assert_eq!(a.layout, b.layout);
        assert_eq!(a.rect[..2], b.rect[..2]);
        assert_ne!(a.rect[2..], b.rect[2..]);
        for d in &baked.draws {
            let [w, h, x, y] = d.rect;
            assert!(x >= 0.0 && y >= 0.0 && x + w <= 1.0 + 1e-6 && y + h <= 1.0 + 1e-6);
        }
        let zone = &baked.zones[0];
        assert!(zone.size[0] > 0 && zone.texels.iter().any(|&t| t != 0));
        assert!(zone.probes.probes.iter().any(Option::is_some));
        let reflection = zone
            .reflection
            .as_ref()
            .expect("a zone with a volume reflects");
        assert_eq!(
            reflection.texels.len(),
            (6 * REFLECTION_SIZE * REFLECTION_SIZE) as usize
        );
        std::fs::remove_dir_all(dir).ok();
    }
}

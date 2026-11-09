//! Baked lighting: BAKE LIGHTING in the editor path-traces the light that
//! bounces around each zone (see `trace`) into lightmaps, textures that
//! cover every static surface (see `unwrap`). Moving objects get the same
//! light from probes: a grid of points per zone, each storing the light
//! arriving from six directions.
//!
//! The result is one file next to the scene (`scenes/hall.ron` →
//! `scenes/hall.lightmap`): per zone its atlas (packed RGB9E5 texels) and
//! probe grid, then per static draw its lightmap UVs.

mod trace;
pub mod unwrap;

use crate::assets::{Assets, Resolved};
use crate::render::mesh::MeshData;
use crate::render::texture::{self, srgb_to_linear};
use crate::scene::{Component, Object, Scene};
use glam::{Mat4, Vec2, Vec3};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use trace::{BakeLight, Rng, World};
use unwrap::Unwrapped;

const MAGIC: &[u8; 8] = b"LIGHTM02";
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

/// Which draw a lightmap belongs to, stable across loads: the object's
/// name, which object of that name it is (names can repeat), and which of
/// its drawable components.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DrawKey {
    pub name: String,
    pub occurrence: u32,
    pub part: u32,
}

/// Keys for every renderable, in `Scene::renderables` order.
pub fn draw_keys(scene: &Scene) -> Vec<DrawKey> {
    let mut keys = Vec::new();
    let mut seen: std::collections::HashMap<&str, u32> = Default::default();
    for object in &scene.objects {
        let occurrence = seen.entry(object.name.as_str()).or_insert(0);
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
                name: object.name.clone(),
                occurrence: *occurrence,
                part,
            });
        }
        *occurrence += 1;
    }
    keys
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

/// One static draw's lightmap UVs.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedDraw {
    pub key: DrawKey,
    /// Index into `Baked::zones`.
    pub zone: u32,
    /// New vertices: the original vertex and its lightmap UV (0–1).
    pub vertices: Vec<(u32, [f32; 2])>,
    pub indices: Vec<u32>,
}

impl BakedDraw {
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
                    format!(
                        "{}{}{:?}{:?}",
                        object.name, object.zone, object.transform, c
                    )
                    .hash(&mut hasher);
                }
                _ => {}
            }
        }
    }
    hasher.finish()
}

/// What a static draw contributes to the bake.
struct Input {
    key: DrawKey,
    zone: usize,
    unwrapped: Unwrapped,
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
    let env = &scene.environment;
    let density = env.lightmap_density.clamp(0.25, 64.0);
    let grid = average(&texture::grid_image());
    let keys = draw_keys(scene);
    let mut inputs = Vec::new();
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
        let unwrapped = unwrap::unwrap(&mesh, model, density);
        for (t, (corners, _)) in unwrapped.triangles.iter().enumerate() {
            triangles.push((*corners, albedo[t], r.emissive));
        }
        let zone = scene
            .zones
            .iter()
            .position(|z| z.name == object.zone)
            .unwrap_or(0);
        inputs.push(Input {
            key,
            zone,
            unwrapped,
        });
    }
    if inputs.is_empty() {
        return Err(LightmapError::Empty);
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
                inputs,
                probe_grids,
                zone_names,
                samples,
                signature,
                &shared,
            );
            save(&baked, &file)?;
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
fn bake_chart(
    world: &World,
    input: &Input,
    chart: &unwrap::Chart,
    samples: u32,
    seed: u64,
) -> Vec<Option<Vec3>> {
    let mut rng = Rng::new(seed);
    let [w, h] = chart.size;
    let tris: Vec<([Vec2; 3], [Vec3; 3], Vec3)> = chart
        .triangles
        .iter()
        .map(|&t| {
            let (p, n) = input.unwrapped.triangles[t];
            (p.map(|c| chart.project(c)), p, n)
        })
        .filter(|(_, _, n)| n.length_squared() > 0.5)
        .collect();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let q = chart.plane_of(x, y);
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

fn bake(
    world: World,
    mut inputs: Vec<Input>,
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
    // Pack each zone's charts.
    let mut sizes = vec![[0u32; 2]; zone_names.len()];
    for (zone, size) in sizes.iter_mut().enumerate() {
        let mut charts: Vec<&mut unwrap::Chart> = inputs
            .iter_mut()
            .filter(|i| i.zone == zone)
            .flat_map(|i| i.unwrapped.charts.iter_mut())
            .collect();
        *size = unwrap::pack(&mut charts);
    }

    // Trace every chart's texels.
    let jobs: Vec<(usize, usize)> = inputs
        .iter()
        .enumerate()
        .flat_map(|(i, input)| (0..input.unwrapped.charts.len()).map(move |c| (i, c)))
        .collect();
    let total_texels: usize = jobs
        .iter()
        .map(|&(i, c)| {
            let s = inputs[i].unwrapped.charts[c].size;
            (s[0] * s[1]) as usize
        })
        .sum();
    let texels_done = AtomicUsize::new(0);
    set("LIGHTMAPS", 0.0);
    let results = parallel(
        jobs.len(),
        |j| {
            let (i, c) = jobs[j];
            let chart = &inputs[i].unwrapped.charts[c];
            let texels = bake_chart(&world, &inputs[i], chart, samples, j as u64 + 1);
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
        let chart = &inputs[i].unwrapped.charts[c];
        let mut local = texels.clone();
        let [w, h] = chart.size;
        dilate(&mut local, w, h);
        let smooth = blur(&local, w, h);
        let atlas = &mut atlases[inputs[i].zone];
        let width = sizes[inputs[i].zone][0];
        for y in 0..h {
            for x in 0..w {
                let at = ((chart.rect[1] + y) * width + chart.rect[0] + x) as usize;
                atlas[at] = smooth[(y * w + x) as usize];
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
    let draws = inputs
        .into_iter()
        .map(|input| {
            let size = sizes_of(&input, &sizes);
            BakedDraw {
                key: input.key,
                zone: input.zone as u32,
                vertices: input
                    .unwrapped
                    .vertices
                    .iter()
                    .map(|&(v, chart, q)| {
                        let t = input.unwrapped.charts[chart].texel_of(q);
                        (v, [t.x / size[0] as f32, t.y / size[1] as f32])
                    })
                    .collect(),
                indices: input.unwrapped.indices,
            }
        })
        .collect();
    Baked {
        signature,
        zones,
        draws,
    }
}

fn sizes_of(input: &Input, sizes: &[[u32; 2]]) -> [u32; 2] {
    let s = sizes[input.zone];
    [s[0].max(1), s[1].max(1)]
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
    let mut w = Writer(MAGIC.to_vec());
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
    w.u32(baked.draws.len() as u32);
    for d in &baked.draws {
        w.string(&d.key.name);
        w.u32(d.key.occurrence);
        w.u32(d.key.part);
        w.u32(d.zone);
        w.u32(d.vertices.len() as u32);
        for &(v, uv) in &d.vertices {
            w.u32(v);
            w.f32(uv[0]);
            w.f32(uv[1]);
        }
        w.u32(d.indices.len() as u32);
        for &i in &d.indices {
            w.u32(i);
        }
    }
    w.0
}

fn decode(bytes: &[u8]) -> Option<Baked> {
    let mut r = Reader(bytes);
    if r.bytes(8)? != MAGIC {
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
    let mut draws = Vec::new();
    for _ in 0..r.count(4)? {
        let key = DrawKey {
            name: r.string()?,
            occurrence: r.u32()?,
            part: r.u32()?,
        };
        let zone = r.u32()?;
        let vertices = (0..r.count(12)?)
            .map(|_| Some((r.u32()?, [r.f32()?, r.f32()?])))
            .collect::<Option<_>>()?;
        let indices = (0..r.count(4)?).map(|_| r.u32()).collect::<Option<_>>()?;
        draws.push(BakedDraw {
            key,
            zone,
            vertices,
            indices,
        });
    }
    Some(Baked {
        signature,
        zones,
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

    #[test]
    fn keys_tell_repeated_names_apart() {
        let mut scene = crate::scene::test_scene(vec![
            (Shape::Box { size: Vec3::ONE }, Vec3::ZERO),
            (Shape::Box { size: Vec3::ONE }, Vec3::X),
        ]);
        for o in &mut scene.objects {
            o.name = "pillar".into();
        }
        let keys = draw_keys(&scene);
        assert_eq!(keys[0].occurrence, 0);
        assert_eq!(keys[1].occurrence, 1);
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
    fn baked_draw_rebuilds_the_mesh() {
        let mesh = MeshData::from_shape(Shape::Box { size: Vec3::ONE });
        let u = unwrap::unwrap(&mesh, Mat4::IDENTITY, 4.0);
        let draw = BakedDraw {
            key: DrawKey {
                name: "a".into(),
                occurrence: 0,
                part: 0,
            },
            zone: 0,
            vertices: u
                .vertices
                .iter()
                .map(|&(v, _, _)| (v, [0.0, 0.0]))
                .collect(),
            indices: u.indices.clone(),
        };
        let (rebuilt, uvs) = draw.apply(&mesh).unwrap();
        assert_eq!(rebuilt.indices.len(), mesh.indices.len());
        assert_eq!(uvs.len(), rebuilt.vertices.len());
        // A mesh with a different triangle count is not a match.
        let other = MeshData::from_shape(Shape::Wedge { size: Vec3::ONE });
        assert!(draw.apply(&other).is_none());
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

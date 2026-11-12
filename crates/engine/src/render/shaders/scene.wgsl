// Forward pass at internal resolution. Output is linear HDR; tone mapping
// happens later in the post pass.

struct Frame {
    view_proj: mat4x4<f32>,
    // World → each of the sun's shadow cascades.
    sun_view_proj: array<mat4x4<f32>, 3>,
    // World → the screen the live occlusion was traced for (last frame's).
    occlusion_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: direction the sunlight travels. w: 1 = the sun casts shadows.
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
    // xyz: the way the camera looks. w: 1 = volumetric fog on.
    camera_forward: vec4<f32>,
    // x: texture repeats per meter. y: number of lights. z: seconds.
    // w: texels per side of a sun shadow cascade.
    params: vec4<f32>,
    // xyz: meters around the camera each sun cascade covers.
    shadow: vec4<f32>,
    // xy: 1 / internal size in pixels. z, w: fog grid near and far depth.
    screen: vec4<f32>,
    // Dust motes in the camera's zone: amount, size (px), brightness, speed.
    dust: vec4<f32>,
    // x: lines of the grid vertices snap to (0 = off). y: 1 = textures
    // warp (no perspective correction). z: live occlusion strength (0 = off).
    retro: vec4<f32>,
};

// One placed draw. Instanced draws read theirs through `visible`, the
// frame's list of instances per draw.
struct Instance {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    color: vec4<f32>,
    // x: selection highlight, 0 or 1. y: texture repeats per UV unit
    // (primitives' UVs are in meters; models use 1).
    highlight: vec4<f32>,
    // rgb: light the surface gives off.
    emissive: vec4<f32>,
    // x: 1 = indirect light from the lightmap, 0 = from `ambient`.
    lighting: vec4<f32>,
    // x: roughness, y: metalness, z: reflection cube index or -1.
    surface: vec4<f32>,
    // Light arriving from +X, -X, +Y, -Y, +Z, -Z (probes or the sky).
    ambient: array<vec4<f32>, 6>,
    // Lightmap UV = the mesh's layout UV * xy + zw (its place in the atlas).
    lightmap_rect: vec4<f32>,
};

struct Light {
    // xyz: position, w: range.
    position: vec4<f32>,
    // rgb: color × intensity, w: shadow slot or -1.
    color: vec4<f32>,
    // xyz: where a cone light shines.
    direction: vec4<f32>,
    // x, y: cosines of the beam's outer and inner edges (-2, -1: a point
    // light, every direction inside).
    cone: vec4<f32>,
};


// How much of a light goes along `travel` (unit, away from the light): 1
// inside a cone light's beam, fading to 0 at its edge; always 1 for a
// point light. Mirrors `LightSource::cone_weight`.
fn cone(l: Light, travel: vec3<f32>) -> f32 {
    return smoothstep(l.cone.x, l.cone.y, dot(travel, l.direction.xyz));
}

// A zone's reflection cube and the box it's projected onto.
struct Reflection {
    // xyz: where it was captured, w: its index in the cube array.
    center: vec4<f32>,
    min: vec4<f32>,
    max: vec4<f32>,
};

const PI: f32 = 3.14159265;

// Linear version of the UI amber, for the selection highlight.
const HIGHLIGHT: vec3<f32> = vec3<f32>(1.0, 0.34, 0.025);

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
// Six view-projections per shadow slot, one per cube face.
@group(0) @binding(2) var<storage, read> shadow_faces: array<mat4x4<f32>>;
@group(0) @binding(3) var sun_shadow: texture_depth_2d_array;
@group(0) @binding(4) var point_shadows: texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler: sampler_comparison;
// The fog between the camera and each depth: rgb = light, a = how much of
// what's behind shows through.
@group(0) @binding(6) var fog_volume: texture_3d<f32>;
@group(0) @binding(7) var fog_sampler: sampler;
// What glossy surfaces reflect: one cube per baked zone.
@group(0) @binding(8) var reflections: texture_cube_array<f32>;
@group(0) @binding(9) var<storage, read> reflection_boxes: array<Reflection>;
@group(0) @binding(10) var reflection_sampler: sampler;
// How open each point on screen was last frame (r), traced live: see
// `occlusion.wgsl`.
@group(0) @binding(11) var live_occlusion: texture_2d<f32>;
@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> visible: array<u32>;
// The grid for primitives; a model part's base color texture.
@group(2) @binding(0) var albedo_texture: texture_2d<f32>;
@group(2) @binding(1) var albedo_sampler: sampler;
// A model part's other maps (glTF); flat / white when it has none.
// Normals in tangent space, OpenGL convention (green = up).
@group(2) @binding(2) var normal_texture: texture_2d<f32>;
// g: roughness, b: metalness, multiplying the object's values.
@group(2) @binding(3) var metal_rough_texture: texture_2d<f32>;
// r: ambient occlusion (how much indirect light reaches the texel).
@group(2) @binding(4) var occlusion_texture: texture_2d<f32>;
// The zone's baked bounce, sky and emissive light.
@group(3) @binding(0) var lightmap: texture_2d<f32>;
@group(3) @binding(1) var lightmap_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) lightmap_uv: vec2<f32>,
    @location(4) color: vec3<f32>,
    @location(5) @interpolate(flat) instance: u32,
    // The UV without perspective correction, for the retro texture warp.
    @location(6) @interpolate(linear) flat_uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) lightmap_uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    @builtin(instance_index) slot: u32,
) -> VertexOut {
    let index = visible[slot];
    let object = instances[index];
    let world = object.model * vec4<f32>(position, 1.0);
    var out: VertexOut;
    out.clip = frame.view_proj * world;
    if frame.retro.x > 0.0 && out.clip.w > 0.0 {
        // Retro: vertices land on a coarse screen grid, so edges wobble as
        // things move.
        let grid = vec2<f32>(frame.retro.x * frame.screen.y / frame.screen.x, frame.retro.x) * 0.5;
        let snapped = round(out.clip.xy / out.clip.w * grid) / grid;
        out.clip = vec4<f32>(snapped * out.clip.w, out.clip.zw);
    }
    out.world = world.xyz;
    out.normal = (object.normal_matrix * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv;
    out.flat_uv = uv;
    out.lightmap_uv = lightmap_uv * object.lightmap_rect.xy + object.lightmap_rect.zw;
    out.instance = index;
    out.color = color.rgb;
    return out;
}

// World position → shadow map coordinates (uv, depth).
fn shadow_coords(m: mat4x4<f32>, p: vec3<f32>) -> vec3<f32> {
    let clip = m * vec4<f32>(p, 1.0);
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.xy * vec2<f32>(0.5, -0.5) + 0.5, ndc.z);
}

// How lit a point is in one sun cascade: 1 = lit, 0 = shadow. 3×3
// filtered taps (each a hardware 2×2 comparison) soften the edge.
fn cascade_visibility(cascade: u32, world: vec3<f32>, n: vec3<f32>) -> f32 {
    let texel = 2.0 * frame.shadow[cascade] / frame.params.w;
    // Pushing the lookup out along the normal avoids self-shadowing acne.
    let p = world + n * texel * 1.5;
    let c = shadow_coords(frame.sun_view_proj[cascade], p);
    if any(c.xy < vec2<f32>(0.0)) || any(c.xy > vec2<f32>(1.0)) || c.z > 1.0 {
        return 1.0;
    }
    let step = 1.0 / frame.params.w;
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let uv = c.xy + vec2<f32>(f32(x), f32(y)) * step;
            lit += textureSampleCompareLevel(sun_shadow, shadow_sampler, uv, cascade, c.z);
        }
    }
    return lit / 9.0;
}

// 1 = lit by the sun, 0 = in its shadow. Uses the smallest (sharpest)
// cascade that covers the point, blending into the next one over its outer
// 20% so there's no visible seam; fades out past the last one.
fn sun_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    if frame.sun_dir.w == 0.0 {
        return 1.0;
    }
    let distance = length(world - frame.camera_pos.xyz);
    for (var i = 0u; i < 3u; i++) {
        let radius = frame.shadow[i];
        if distance < radius {
            let lit = cascade_visibility(i, world, n);
            let edge = smoothstep(radius * 0.8, radius, distance);
            if edge <= 0.0 {
                return lit;
            }
            var next = 1.0;
            if i < 2u {
                next = cascade_visibility(i + 1u, world, n);
            }
            return mix(lit, next, edge);
        }
    }
    return 1.0;
}

// Which cube face sees direction d: the axis it points along most.
fn cube_face(d: vec3<f32>) -> u32 {
    let a = abs(d);
    if a.x >= a.y && a.x >= a.z {
        return select(1u, 0u, d.x >= 0.0);
    }
    if a.y >= a.z {
        return select(3u, 2u, d.y >= 0.0);
    }
    return select(5u, 4u, d.z >= 0.0);
}

fn point_visibility(light: Light, world: vec3<f32>, n: vec3<f32>, distance: f32) -> f32 {
    if light.color.w < 0.0 {
        return 1.0;
    }
    // A shadow texel covers about 2·distance/size meters.
    let size = f32(textureDimensions(point_shadows).x);
    let texel = 2.0 * distance / size;
    let p = world + n * (texel * 1.5 + 0.005);
    let layer = u32(light.color.w) * 6u + cube_face(p - light.position.xyz);
    let c = shadow_coords(shadow_faces[layer], p);
    let step = 1.0 / size;
    var lit = 0.0;
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i & 1), f32(i >> 1u)) - 0.5;
        lit += textureSampleCompareLevel(point_shadows, shadow_sampler, c.xy + offset * step, layer, c.z);
    }
    return lit * 0.25;
}

// The surface normal with the normal map applied. The tangent frame comes
// from how the position and UV change across the pixel (screen-space
// derivatives), so meshes need no tangents. glTF's V runs down the image,
// so "up" in the map is toward smaller V.
fn mapped_normal(n: vec3<f32>, world: vec3<f32>, uv: vec2<f32>, texel: vec3<f32>) -> vec3<f32> {
    let dp1 = dpdx(world);
    let dp2 = dpdy(world);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = -(dp2perp * duv1.y + dp1perp * duv2.y);
    let len2 = max(dot(t, t), dot(b, b));
    let local = texel * 2.0 - 1.0;
    // No UVs (or a flat-normal map): nothing to bend.
    if len2 < 1e-20 || all(abs(local.xy) < vec2<f32>(0.004)) {
        return n;
    }
    let scale = inverseSqrt(len2);
    return normalize(t * scale * local.x + b * scale * local.y + n * local.z);
}

// How glossy a surface is, for the lighting functions.
struct Surface {
    n: vec3<f32>,
    // Toward the camera.
    v: vec3<f32>,
    roughness: f32,
    // Reflectance looking straight on: 4% for stone and wood, the albedo
    // for metal.
    f0: vec3<f32>,
    // Fully matte surfaces get no highlights or reflections at all.
    glossy: bool,
};

// A highlight from light arriving along l (GGX distribution, Smith
// visibility, Schlick Fresnel), scaled like the diffuse light (× π).
fn highlight(s: Surface, l: vec3<f32>) -> vec3<f32> {
    if !s.glossy {
        return vec3<f32>(0.0);
    }
    let h = normalize(l + s.v);
    let ndh = max(dot(s.n, h), 0.0);
    let ndl = max(dot(s.n, l), 1e-4);
    let ndv = max(dot(s.n, s.v), 1e-4);
    let a = max(s.roughness * s.roughness, 0.002);
    let a2 = a * a;
    let d = a2 / (PI * pow(ndh * ndh * (a2 - 1.0) + 1.0, 2.0));
    let k = a * 0.5;
    let visibility = 0.25 / ((ndl * (1.0 - k) + k) * (ndv * (1.0 - k) + k));
    let fresnel = s.f0 + (1.0 - s.f0) * pow(1.0 - max(dot(s.v, h), 0.0), 5.0);
    return d * visibility * fresnel * PI;
}

struct Lit {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
};

// Light from all point lights reaching a surface.
fn point_lights(world: vec3<f32>, s: Surface) -> Lit {
    var lit = Lit(vec3<f32>(0.0), vec3<f32>(0.0));
    let count = u32(frame.params.y);
    for (var i = 0u; i < count; i++) {
        let light = lights[i];
        let to_light = light.position.xyz - world;
        let distance = length(to_light);
        let range = light.position.w;
        if distance >= range {
            continue;
        }
        let l = to_light / max(distance, 1e-4);
        let facing = dot(s.n, l) * cone(light, -l);
        if facing <= 0.0 {
            continue;
        }
        // Inverse square, smoothly windowed to zero at the range.
        let ratio = distance / range;
        let window = saturate(1.0 - ratio * ratio * ratio * ratio);
        let falloff = window * window / max(distance * distance, 0.01);
        let arriving = light.color.rgb * facing * falloff * point_visibility(light, world, s.n, distance);
        lit.diffuse += arriving;
        lit.specular += arriving * highlight(s, l);
    }
    return lit;
}

// What a glossy surface reflects along r: its zone's cube, projected onto
// the zone's box so walls appear where they are; the sky without a bake.
fn reflected(world: vec3<f32>, r: vec3<f32>, roughness: f32, cube: f32) -> vec3<f32> {
    let index = i32(cube);
    if index < 0 {
        return mix(frame.ambient_ground.rgb, frame.ambient_sky.rgb, r.y * 0.5 + 0.5);
    }
    let probe = reflection_boxes[index];
    var dir = r;
    if all(world >= probe.min.xyz) && all(world <= probe.max.xyz) {
        let safe = select(r, vec3<f32>(1e-5), abs(r) < vec3<f32>(1e-5));
        let far = max((probe.max.xyz - world) / safe, (probe.min.xyz - world) / safe);
        let t = min(min(far.x, far.y), far.z);
        dir = world + r * t - probe.center.xyz;
    }
    let levels = f32(textureNumLevels(reflections) - 1u);
    return textureSampleLevel(reflections, reflection_sampler, dir, i32(probe.center.w), roughness * levels).rgb;
}

// How much of the reflection a surface shows, averaged over its roughness
// (Karis' fit of the split-sum lookup).
fn reflectance(f0: vec3<f32>, roughness: f32, ndv: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * ndv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// How open a point is to its surroundings, 0–1: last frame's traced
// occlusion, looked up where the point was on screen then and softened
// over the cells around it.
fn openness(world: vec3<f32>) -> f32 {
    let strength = frame.retro.z;
    if strength <= 0.0 {
        return 1.0;
    }
    let clip = frame.occlusion_view_proj * vec4<f32>(world, 1.0);
    if clip.w <= 0.0 {
        return 1.0;
    }
    let uv = clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return 1.0;
    }
    let cell = 0.75 / vec2<f32>(textureDimensions(live_occlusion));
    var open = 0.0;
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i & 1), f32(i >> 1u)) * 2.0 - 1.0;
        open += textureSampleLevel(live_occlusion, fog_sampler, uv + offset * cell, 0.0).r;
    }
    return mix(1.0, open * 0.25, strength);
}

// A shaded pixel, and what the live effects need to know about its
// surface.
struct Shaded {
    color: vec3<f32>,
    normal: vec3<f32>,
    roughness: f32,
    // How much of what it reflects reaches the camera.
    reflectance: vec3<f32>,
    // Reflection cube index + 1, plus 32 if lit from its lightmap.
    code: f32,
};

fn shade(in: VertexOut) -> Shaded {
    let object = instances[in.instance];
    let geometric = normalize(in.normal);
    let uv = select(in.uv, in.flat_uv, frame.retro.y > 0.5) * object.highlight.y;
    let albedo = textureSample(albedo_texture, albedo_sampler, uv).rgb * object.color.rgb * in.color;
    let n = mapped_normal(geometric, in.world, uv, textureSample(normal_texture, albedo_sampler, uv).rgb);
    let metal_rough = textureSample(metal_rough_texture, albedo_sampler, uv);
    // Indirect light is dimmed by the texture's occlusion and the live one.
    let occlusion = textureSample(occlusion_texture, albedo_sampler, uv).r * openness(in.world);
    let roughness = object.surface.x * metal_rough.g;
    let metalness = object.surface.y * metal_rough.b;
    let to_camera = normalize(frame.camera_pos.xyz - in.world);
    let s = Surface(n, to_camera, roughness, mix(vec3<f32>(0.04), albedo, metalness), roughness < 0.99);

    var ambient: vec3<f32>;
    if object.lighting.x > 0.5 {
        ambient = textureSampleLevel(lightmap, lightmap_sampler, in.lightmap_uv, 0.0).rgb;
    } else {
        // Blend the six directions by how much the normal faces each.
        let a = object.ambient;
        let n2 = n * n;
        ambient = n2.x * select(a[1], a[0], n.x >= 0.0).rgb
            + n2.y * select(a[3], a[2], n.y >= 0.0).rgb
            + n2.z * select(a[5], a[4], n.z >= 0.0).rgb;
    }

    var diffuse = ambient * occlusion;
    var specular = vec3<f32>(0.0);
    let facing_sun = max(dot(n, -frame.sun_dir.xyz), 0.0);
    if facing_sun > 0.0 {
        let sun = frame.sun_color.rgb * facing_sun * sun_visibility(in.world, n);
        diffuse += sun;
        specular += sun * highlight(s, -frame.sun_dir.xyz);
    }
    let lamps = point_lights(in.world, s);
    diffuse += lamps.diffuse;
    specular += lamps.specular;
    var reflecting = vec3<f32>(0.0);
    if s.glossy {
        let ndv = max(dot(n, to_camera), 1e-4);
        let r = reflect(-to_camera, n);
        reflecting = reflectance(s.f0, roughness, ndv) * occlusion;
        specular += reflected(in.world, r, roughness, object.surface.z) * reflecting;
    }
    var color = albedo * (1.0 - metalness) * diffuse + specular + object.emissive.rgb;

    // Selected: amber tint, stronger toward the silhouette.
    let rim = pow(1.0 - max(dot(geometric, to_camera), 0.0), 2.0);
    let tint = object.highlight.x * (0.3 + 0.6 * rim);
    color = mix(color, HIGHLIGHT * 1.5, tint);

    let f = fog_at(in.clip.xy, in.world);
    var out: Shaded;
    out.color = color * f.a + f.rgb;
    out.normal = n;
    out.roughness = roughness;
    out.reflectance = reflecting * (1.0 - tint) * f.a;
    out.code = object.surface.z + 1.0 + 32.0 * object.lighting.x;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in).color, 1.0);
}

// The same pixel with its surface written beside it, for the live effects
// (screen-space reflections, traced occlusion).
struct Surfaces {
    @location(0) color: vec4<f32>,
    // xyz: world normal, a: roughness.
    @location(1) normal: vec4<f32>,
    // rgb: reflectance, a: `Shaded.code`.
    @location(2) reflectance: vec4<f32>,
};

@fragment
fn fs_surfaces(in: VertexOut) -> Surfaces {
    let shaded = shade(in);
    var out: Surfaces;
    out.color = vec4<f32>(shaded.color, 1.0);
    out.normal = vec4<f32>(shaded.normal, shaded.roughness);
    out.reflectance = vec4<f32>(shaded.reflectance, shaded.code);
    return out;
}

// The fog between the camera and a point: rgb = its light, a = how much of
// what's behind shows through.
fn fog_at(pixel: vec2<f32>, world: vec3<f32>) -> vec4<f32> {
    if frame.camera_forward.w == 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let uv = pixel * frame.screen.xy;
    let depth = max(dot(world - frame.camera_pos.xyz, frame.camera_forward.xyz), frame.screen.z);
    let slices = f32(textureDimensions(fog_volume).z);
    // Slice k holds the fog up to the far end of slice k.
    let t = log(depth / frame.screen.z) / log(frame.screen.w / frame.screen.z);
    let w = t - 0.5 / slices;
    return textureSampleLevel(fog_volume, fog_sampler, vec3<f32>(uv, w), 0.0);
}

// A fullscreen triangle on the far plane.
@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 1.0, 1.0);
}

// The sky: the ambient sky color behind all of the fog.
@fragment
fn fs_sky(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    if frame.camera_forward.w == 0.0 {
        return vec4<f32>(frame.ambient_sky.rgb, 1.0);
    }
    let uv = pixel.xy * frame.screen.xy;
    let f = textureSampleLevel(fog_volume, fog_sampler, vec3<f32>(uv, 1.0), 0.0);
    return vec4<f32>(frame.ambient_sky.rgb * f.a + f.rgb, 1.0);
}

// ---- Dust motes ----
//
// Points spread through a box that follows the camera (positions come
// from a hash of the instance number and drift with time, wrapping around
// the box), each lit like a speck of dust: the sun and lights through their
// shadow maps, scattered strongly forward. Outside light they're black and
// add nothing; in a beam they glitter.

// Meters from the camera to the edge of the dust box.
const DUST_REACH: f32 = 7.0;

fn dust_hash(n: u32) -> u32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn dust_rand(n: u32) -> f32 {
    return f32(dust_hash(n)) / 4294967295.0;
}

// Henyey–Greenstein phase, strongly forward: motes flare when the light is
// behind them.
fn dust_phase(cos_theta: f32) -> f32 {
    let g = 0.7;
    let d = max(1.0 + g * g - 2.0 * g * cos_theta, 1e-3);
    return (1.0 - g * g) / (4.0 * PI * d * sqrt(d));
}

struct DustOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) light: vec3<f32>,
};

@vertex
fn vs_dust(@builtin(vertex_index) vertex: u32, @builtin(instance_index) id: u32) -> DustOut {
    var out: DustOut;
    let t = frame.params.z;
    let seed = id * 4u;
    // A home position in the box, a slow drift and a little sway.
    let home = vec3<f32>(dust_rand(seed), dust_rand(seed + 1u), dust_rand(seed + 2u));
    let heading = normalize(vec3<f32>(dust_rand(seed + 3u) - 0.5, (dust_rand(seed + 7u) - 0.5) * 0.4, dust_rand(seed + 11u) - 0.5) + vec3<f32>(1e-3));
    let size = DUST_REACH * 2.0;
    let sway = vec3<f32>(sin(t * 0.7 + home.x * 40.0), sin(t * 0.5 + home.y * 30.0), cos(t * 0.6 + home.z * 50.0)) * 0.08;
    let p = home * size + heading * frame.dust.w * t + sway;
    // Wrap into the box around the camera.
    let rel = (fract((p - frame.camera_pos.xyz) / size + 0.5) - 0.5) * size;
    let world = frame.camera_pos.xyz + rel;

    // Light reaching the mote, as fog sees it.
    let view = normalize(rel);
    var light = vec3<f32>(0.0);
    if dot(frame.sun_dir.xyz, frame.sun_dir.xyz) > 0.0 {
        light = frame.sun_color.rgb * sun_visibility(world, vec3<f32>(0.0)) * dust_phase(dot(frame.sun_dir.xyz, -view));
    }
    let count = u32(frame.params.y);
    for (var i = 0u; i < count; i++) {
        let l = lights[i];
        let to_mote = world - l.position.xyz;
        let distance = length(to_mote);
        let range = l.position.w;
        if distance >= range {
            continue;
        }
        let ratio = distance / range;
        let window = saturate(1.0 - ratio * ratio * ratio * ratio);
        let falloff = window * window / max(distance * distance, 0.04);
        let travel = to_mote / max(distance, 1e-4);
        let beam = cone(l, travel);
        if beam <= 0.0 {
            continue;
        }
        light += l.color.rgb * beam * falloff * point_visibility(l, world, vec3<f32>(0.0), distance) * dust_phase(dot(travel, -view));
    }
    // Twinkle as the speck turns, fade near the camera and the box edge.
    let twinkle = 0.55 + 0.45 * sin(t * (1.5 + home.y * 3.0) + home.z * 60.0);
    let distance = length(rel);
    let fade = smoothstep(0.25, 0.9, distance) * (1.0 - smoothstep(DUST_REACH * 0.75, DUST_REACH, max(max(abs(rel.x), abs(rel.y)), abs(rel.z))));
    out.light = light * frame.dust.z * twinkle * fade;

    // A square of `size` internal pixels, snapped to whole pixels so it
    // stays crisp.
    let clip = frame.view_proj * vec4<f32>(world, 1.0);
    let corner = vec2<f32>(f32(vertex == 1u || vertex == 2u || vertex == 4u), f32(vertex >= 2u && vertex <= 4u));
    if clip.w <= 0.05 || all(out.light <= vec3<f32>(1e-4)) {
        // Behind the camera or unlit: collapse it.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let pixels = 1.0 / frame.screen.xy;
    let ndc = clip.xy / clip.w;
    let pixel = floor((ndc * vec2<f32>(0.5, -0.5) + 0.5) * pixels) + corner * frame.dust.y;
    let snapped = (pixel / pixels - 0.5) * vec2<f32>(2.0, -2.0);
    out.clip = vec4<f32>(snapped * clip.w, clip.z, clip.w);
    return out;
}

@fragment
fn fs_dust(in: DustOut) -> @location(0) vec4<f32> {
    // Dimmed by the fog in front of it (its own scattering is the fog's).
    let uv = in.clip.xy * frame.screen.xy;
    var through = 1.0;
    if frame.camera_forward.w != 0.0 {
        let depth = max(in.clip.w, frame.screen.z);
        let t = log(depth / frame.screen.z) / log(frame.screen.w / frame.screen.z);
        let slices = f32(textureDimensions(fog_volume).z);
        through = textureSampleLevel(fog_volume, fog_sampler, vec3<f32>(uv, t - 0.5 / slices), 0.0).a;
    }
    return vec4<f32>(in.light * through, 1.0);
}

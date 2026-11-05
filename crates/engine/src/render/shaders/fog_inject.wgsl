// Volumetric fog, step 1: for every froxel (a cell of a grid that follows
// the camera's view, its slices spaced exponentially in depth), how dense
// the fog is and how much light it scatters toward the camera.
//
// Each frame samples a random depth inside each cell and blends the result
// with the previous frame's value for the same spot in the world (temporal
// accumulation), so few samples per frame still give smooth fog.

struct Fog {
    sun_view_proj: array<mat4x4<f32>, 3>,
    // Last frame's view-projection, to find where a cell was a frame ago.
    prev_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // w: weight of this frame's sample (1 = no history).
    camera_forward: vec4<f32>,
    // Scaled so forward + ndc.x * right + ndc.y * up is a ray through a
    // point on screen with a depth of 1 along forward.
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    // w: 1 = the sun casts shadows.
    sun_dir: vec4<f32>,
    // w: anisotropy of the phase function.
    sun_color: vec4<f32>,
    // w: number of regions.
    ambient: vec4<f32>,
    // Fog outside every zone region. rgb: color, a: density.
    default_fog: vec4<f32>,
    // x: height, y: falloff, z: number of lights, w: frame number.
    default_shape: vec4<f32>,
    // x: near, y: far (meters of fog), z: sun shadow texels per side.
    depth: vec4<f32>,
    // xyz: meters each sun cascade covers.
    cascades: vec4<f32>,
};

struct Light {
    // xyz: position, w: range.
    position: vec4<f32>,
    // rgb: color × intensity, w: shadow slot, or -1 for none.
    color: vec4<f32>,
};

// A zone's fog volume, or a FogVolume.
struct Region {
    // World → the region's box, where -0.5..0.5 on each axis is inside.
    to_box: mat4x4<f32>,
    // rgb: color, a: density.
    fog: vec4<f32>,
    // x: height (zones), y: falloff, z: edge softness (volumes),
    // w: 0 = zone, 1 = FogVolume.
    shape: vec4<f32>,
    // x: the box's height in meters (volumes).
    size: vec4<f32>,
};

const PI: f32 = 3.14159265;
const POINT_SHADOW_SIZE: f32 = 512.0;

@group(0) @binding(0) var<uniform> fog: Fog;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read> shadow_faces: array<mat4x4<f32>>;
@group(0) @binding(3) var sun_shadow: texture_depth_2d_array;
@group(0) @binding(4) var point_shadows: texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var<storage, read> regions: array<Region>;
@group(0) @binding(7) var history: texture_3d<f32>;
@group(0) @binding(8) var linear_sampler: sampler;
@group(0) @binding(9) var output: texture_storage_3d<rgba16float, write>;

fn hash(v: vec3<u32>) -> f32 {
    var x = v.x * 1664525u + v.y * 1013904223u + v.z * 2654435769u;
    x ^= x >> 16u;
    x *= 2246822519u;
    x ^= x >> 13u;
    x *= 3266489917u;
    x ^= x >> 16u;
    return f32(x) / 4294967295.0;
}

// Henyey–Greenstein phase function: how much light traveling along one
// direction scatters into another, cos_theta apart.
fn phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4);
    return (1.0 - g2) / (4.0 * PI * denom * sqrt(denom));
}

fn shadow_coords(m: mat4x4<f32>, p: vec3<f32>) -> vec3<f32> {
    let clip = m * vec4<f32>(p, 1.0);
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.xy * vec2<f32>(0.5, -0.5) + 0.5, ndc.z);
}

fn sun_visibility(p: vec3<f32>) -> f32 {
    if fog.sun_dir.w == 0.0 {
        return 1.0;
    }
    let distance = length(p - fog.camera_pos.xyz);
    for (var i = 0u; i < 3u; i++) {
        if distance < fog.cascades[i] {
            let c = shadow_coords(fog.sun_view_proj[i], p);
            if any(c.xy < vec2<f32>(0.0)) || any(c.xy > vec2<f32>(1.0)) || c.z > 1.0 {
                return 1.0;
            }
            return textureSampleCompareLevel(sun_shadow, shadow_sampler, c.xy, i, c.z);
        }
    }
    return 1.0;
}

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

fn point_visibility(light: Light, p: vec3<f32>) -> f32 {
    if light.color.w < 0.0 {
        return 1.0;
    }
    let layer = u32(light.color.w) * 6u + cube_face(p - light.position.xyz);
    let c = shadow_coords(shadow_faces[layer], p);
    return textureSampleCompareLevel(point_shadows, shadow_sampler, c.xy, layer, c.z);
}

// rgb: scattering (density × color), a: density, at a point.
fn medium(p: vec3<f32>) -> vec4<f32> {
    var zone_fog = fog.default_fog;
    var height = fog.default_shape.x;
    var falloff = fog.default_shape.y;
    var found_zone = false;
    var extra = vec4<f32>(0.0);
    let count = u32(fog.ambient.w);
    for (var i = 0u; i < count; i++) {
        let r = regions[i];
        let local = (r.to_box * vec4<f32>(p, 1.0)).xyz;
        let outside = max(max(abs(local.x), abs(local.y)), abs(local.z));
        if outside > 0.5 {
            continue;
        }
        if r.shape.w == 0.0 {
            // Zones come smallest first: the first one containing p wins.
            if !found_zone {
                found_zone = true;
                zone_fog = r.fog;
                height = r.shape.x;
                falloff = r.shape.y;
            }
        } else {
            let soft = max(r.shape.z, 1e-3) * 0.5;
            let edge = saturate((vec3<f32>(0.5) - abs(local)) / soft);
            let fade = edge.x * edge.y * edge.z;
            let above_bottom = (local.y + 0.5) * r.size.x;
            let d = r.fog.a * fade * exp(-r.shape.y * above_bottom);
            extra += vec4<f32>(r.fog.rgb * d, d);
        }
    }
    let d = zone_fog.a * exp(-falloff * max(p.y - height, 0.0));
    return vec4<f32>(zone_fog.rgb * d, d) + extra;
}

@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output);
    if any(id >= dims) {
        return;
    }
    let size = vec3<f32>(dims);
    let frame = u32(fog.default_shape.w);

    // A random depth inside the cell, different every frame.
    let jitter = fract(hash(id) + f32(frame % 64u) * 0.618034);
    let t = (f32(id.z) + jitter) / size.z;
    let near = fog.depth.x;
    let far = fog.depth.y;
    let depth = near * pow(far / near, t);
    let uv = (vec2<f32>(id.xy) + 0.5) / size.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let ray = fog.camera_forward.xyz + ndc.x * fog.camera_right.xyz + ndc.y * fog.camera_up.xyz;
    let p = fog.camera_pos.xyz + ray * depth;
    let view = normalize(ray);

    let m = medium(p);
    var light = vec3<f32>(0.0);
    if m.a > 0.0 {
        let g = fog.sun_color.w;
        // cos θ between where the light travels and the way back to the
        // camera: 1 when looking straight into the light.
        light = fog.sun_color.rgb * sun_visibility(p) * phase(dot(fog.sun_dir.xyz, -view), g);
        let light_count = u32(fog.default_shape.z);
        for (var i = 0u; i < light_count; i++) {
            let l = lights[i];
            let to_point = p - l.position.xyz;
            let distance = length(to_point);
            let range = l.position.w;
            if distance >= range {
                continue;
            }
            let ratio = distance / range;
            let window = saturate(1.0 - ratio * ratio * ratio * ratio);
            let falloff = window * window / max(distance * distance, 0.04);
            let travel = to_point / max(distance, 1e-4);
            light += l.color.rgb * falloff * point_visibility(l, p) * phase(dot(travel, -view), g);
        }
        // Ambient light arrives from everywhere: the phase averages out.
        light += fog.ambient.rgb / (4.0 * PI);
    }
    var current = vec4<f32>(m.rgb * light, m.a);

    // Blend with where this point was last frame.
    var weight = fog.camera_forward.w;
    if weight < 1.0 {
        let prev = fog.prev_view_proj * vec4<f32>(p, 1.0);
        if prev.w > near {
            let prev_ndc = prev.xy / prev.w;
            let prev_uv = vec2<f32>(prev_ndc.x * 0.5 + 0.5, 0.5 - prev_ndc.y * 0.5);
            let prev_t = log(prev.w / near) / log(far / near);
            let coords = vec3<f32>(prev_uv, prev_t);
            if all(coords >= vec3<f32>(0.0)) && all(coords <= vec3<f32>(1.0)) {
                let old = textureSampleLevel(history, linear_sampler, coords, 0.0);
                current = mix(old, current, weight);
            }
        }
    }
    textureStore(output, id, current);
}

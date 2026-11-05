// Forward pass at internal resolution. Output is linear HDR; tone mapping
// happens later in the post pass.

struct Frame {
    view_proj: mat4x4<f32>,
    // World → each of the sun's shadow cascades.
    sun_view_proj: array<mat4x4<f32>, 3>,
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
};

struct Object {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    color: vec4<f32>,
    // x: selection highlight, 0 or 1. y: 1 = primitive (UVs in meters,
    // tiling the grid texture), 0 = model (its own UVs and texture).
    highlight: vec4<f32>,
    // rgb: light the surface gives off.
    emissive: vec4<f32>,
    // x: 1 = indirect light from the lightmap, 0 = from `ambient`.
    lighting: vec4<f32>,
    // Light arriving from +X, -X, +Y, -Y, +Z, -Z (probes or the sky).
    ambient: array<vec4<f32>, 6>,
};

struct Light {
    // xyz: position, w: range.
    position: vec4<f32>,
    // rgb: color × intensity, w: shadow slot or -1.
    color: vec4<f32>,
};

// Linear version of the UI amber, for the selection highlight.
const HIGHLIGHT: vec3<f32> = vec3<f32>(1.0, 0.34, 0.025);
const POINT_SHADOW_SIZE: f32 = 512.0;

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
@group(1) @binding(0) var<uniform> object: Object;
// The grid for primitives; a model part's base color texture.
@group(2) @binding(0) var albedo_texture: texture_2d<f32>;
@group(2) @binding(1) var albedo_sampler: sampler;
// The zone's baked bounce, sky and emissive light.
@group(3) @binding(0) var lightmap: texture_2d<f32>;
@group(3) @binding(1) var lightmap_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) lightmap_uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) lightmap_uv: vec2<f32>,
) -> VertexOut {
    let world = object.model * vec4<f32>(position, 1.0);
    var out: VertexOut;
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    out.normal = (object.normal_matrix * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv;
    out.lightmap_uv = lightmap_uv;
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
    let texel = 2.0 * distance / POINT_SHADOW_SIZE;
    let p = world + n * (texel * 1.5 + 0.005);
    let layer = u32(light.color.w) * 6u + cube_face(p - light.position.xyz);
    let c = shadow_coords(shadow_faces[layer], p);
    let step = 1.0 / POINT_SHADOW_SIZE;
    var lit = 0.0;
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i & 1), f32(i >> 1u)) - 0.5;
        lit += textureSampleCompareLevel(point_shadows, shadow_sampler, c.xy + offset * step, layer, c.z);
    }
    return lit * 0.25;
}

// Light from all point lights reaching a surface with normal n.
fn point_lights(world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var total = vec3<f32>(0.0);
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
        let facing = dot(n, l);
        if facing <= 0.0 {
            continue;
        }
        // Inverse square, smoothly windowed to zero at the range.
        let ratio = distance / range;
        let window = saturate(1.0 - ratio * ratio * ratio * ratio);
        let falloff = window * window / max(distance * distance, 0.01);
        let visibility = point_visibility(light, world, n, distance);
        total += light.color.rgb * facing * falloff * visibility;
    }
    return total;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let uv = in.uv * mix(1.0, frame.params.x, object.highlight.y);
    let albedo = textureSample(albedo_texture, albedo_sampler, uv).rgb * object.color.rgb;

    let facing_sun = max(dot(n, -frame.sun_dir.xyz), 0.0);
    var sun = vec3<f32>(0.0);
    if facing_sun > 0.0 {
        sun = frame.sun_color.rgb * facing_sun * sun_visibility(in.world, n);
    }
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
    let light = sun + ambient + point_lights(in.world, n);
    var color = albedo * light + object.emissive.rgb;

    // Selected: amber tint, stronger toward the silhouette.
    let to_camera = normalize(frame.camera_pos.xyz - in.world);
    let rim = pow(1.0 - max(dot(n, to_camera), 0.0), 2.0);
    color = mix(color, HIGHLIGHT * 1.5, object.highlight.x * (0.3 + 0.6 * rim));

    return vec4<f32>(fog(color, in.clip.xy, in.world), 1.0);
}

// The scene color at a pixel seen through the fog in front of it.
fn fog(color: vec3<f32>, pixel: vec2<f32>, world: vec3<f32>) -> vec3<f32> {
    if frame.camera_forward.w == 0.0 {
        return color;
    }
    let uv = pixel * frame.screen.xy;
    let depth = max(dot(world - frame.camera_pos.xyz, frame.camera_forward.xyz), frame.screen.z);
    let slices = f32(textureDimensions(fog_volume).z);
    // Slice k holds the fog up to the far end of slice k.
    let t = log(depth / frame.screen.z) / log(frame.screen.w / frame.screen.z);
    let w = t - 0.5 / slices;
    let f = textureSampleLevel(fog_volume, fog_sampler, vec3<f32>(uv, w), 0.0);
    return color * f.a + f.rgb;
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

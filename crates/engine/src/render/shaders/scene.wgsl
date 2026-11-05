// Forward pass at internal resolution. Output is linear HDR; tone mapping
// happens later in the post pass.

struct Frame {
    view_proj: mat4x4<f32>,
    // World → the sun's shadow map.
    sun_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: direction the sunlight travels. w: 1 = the sun casts shadows.
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
    // rgb: color, a: density per meter.
    fog: vec4<f32>,
    // x: texture repeats per meter. y: number of lights. z: seconds.
    // w: size of a sun shadow texel in meters.
    params: vec4<f32>,
    // x: meters around the camera with sun shadows.
    shadow: vec4<f32>,
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
@group(0) @binding(3) var sun_shadow: texture_depth_2d;
@group(0) @binding(4) var point_shadows: texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> object: Object;
// The grid for primitives; a model part's base color texture.
@group(2) @binding(0) var albedo_texture: texture_2d<f32>;
@group(2) @binding(1) var albedo_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VertexOut {
    let world = object.model * vec4<f32>(position, 1.0);
    var out: VertexOut;
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    out.normal = (object.normal_matrix * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv;
    return out;
}

// World position → shadow map coordinates (uv, depth).
fn shadow_coords(m: mat4x4<f32>, p: vec3<f32>) -> vec3<f32> {
    let clip = m * vec4<f32>(p, 1.0);
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.xy * vec2<f32>(0.5, -0.5) + 0.5, ndc.z);
}

// 1 = lit by the sun, 0 = in its shadow. 3×3 filtered taps (each one a
// hardware 2×2 comparison) soften the edge.
fn sun_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    if frame.sun_dir.w == 0.0 {
        return 1.0;
    }
    let texel = frame.params.w;
    // Pushing the lookup out along the normal avoids self-shadowing acne.
    let p = world + n * texel * 1.5;
    let c = shadow_coords(frame.sun_view_proj, p);
    if any(c.xy < vec2<f32>(0.0)) || any(c.xy > vec2<f32>(1.0)) || c.z > 1.0 {
        return 1.0;
    }
    let step = 1.0 / f32(textureDimensions(sun_shadow).x);
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let uv = c.xy + vec2<f32>(f32(x), f32(y)) * step;
            lit += textureSampleCompareLevel(sun_shadow, shadow_sampler, uv, c.z);
        }
    }
    // Fade out toward the edge of the shadowed area instead of a seam.
    let distance = length(world - frame.camera_pos.xyz);
    let fade = smoothstep(frame.shadow.x * 0.8, frame.shadow.x, distance);
    return mix(lit / 9.0, 1.0, fade);
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
    let ambient = mix(frame.ambient_ground.rgb, frame.ambient_sky.rgb, n.y * 0.5 + 0.5);
    let light = sun + ambient + point_lights(in.world, n);
    var color = albedo * light + object.emissive.rgb;

    // Selected: amber tint, stronger toward the silhouette.
    let to_camera = normalize(frame.camera_pos.xyz - in.world);
    let rim = pow(1.0 - max(dot(n, to_camera), 0.0), 2.0);
    color = mix(color, HIGHLIGHT * 1.5, object.highlight.x * (0.3 + 0.6 * rim));

    let distance = length(in.world - frame.camera_pos.xyz);
    let fog = 1.0 - exp(-distance * frame.fog.a);
    color = mix(color, frame.fog.rgb, fog);

    return vec4<f32>(color, 1.0);
}

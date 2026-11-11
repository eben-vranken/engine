// Screen-space reflections: glossy surfaces reflect what is on screen, so
// moving things and live lights show up in a polished floor or a puddle,
// which the baked reflection cubes can't hold.
//
// For each glossy pixel the reflected ray is walked across the screen,
// comparing its depth with the depth image; where it goes behind something,
// that pixel's color is the reflection, and it replaces what the scene pass
// took from the zone's cube. A ray that finds nothing (it leaves the
// screen, or passes behind things) keeps the cube.
//
// Reads the scene pass's color, depth and surfaces; writes the image the
// post pass reads.

// Matches `LiveUniform` in `live.rs` (and `Live` in `occlusion.wgsl`).
struct Live {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // rgb: sky color. w: near plane.
    ambient_sky: vec4<f32>,
    // rgb: ground color. w: far plane.
    ambient_ground: vec4<f32>,
    occlusion: vec4<f32>,
    // x: most steps along a reflected ray. y: meters it looks.
    reflection: vec4<f32>,
};

// As in `scene.wgsl`.
struct Reflection {
    center: vec4<f32>,
    min: vec4<f32>,
    max: vec4<f32>,
};

// Rougher than this, reflections fade back to the (blurry) cube; only sharp
// ones can be read off the screen.
const ROUGH_START: f32 = 0.25;
const ROUGH_END: f32 = 0.5;

@group(0) @binding(0) var<uniform> live: Live;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var depth_texture: texture_depth_2d;
// xyz: world normal, a: roughness.
@group(0) @binding(3) var normal_texture: texture_2d<f32>;
// rgb: reflectance, a: reflection cube index + 1 (+ 32 if lightmapped).
@group(0) @binding(4) var reflectance_texture: texture_2d<f32>;
@group(0) @binding(5) var reflections: texture_cube_array<f32>;
@group(0) @binding(6) var<storage, read> reflection_boxes: array<Reflection>;
@group(0) @binding(7) var reflection_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// What the scene pass reflected here: `reflected` in `scene.wgsl`.
fn cube_reflection(world: vec3<f32>, r: vec3<f32>, roughness: f32, index: i32) -> vec3<f32> {
    if index < 0 {
        return mix(live.ambient_ground.rgb, live.ambient_sky.rgb, r.y * 0.5 + 0.5);
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

// Distance from the camera plane of what the depth image holds at a pixel.
fn scene_distance(pixel: vec2<i32>) -> f32 {
    let depth = textureLoad(depth_texture, pixel, 0);
    let near = live.ambient_sky.w;
    let far = live.ambient_ground.w;
    return near * far / (far - depth * (far - near));
}

fn to_uv(clip: vec4<f32>) -> vec2<f32> {
    return clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5;
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let color = textureLoad(scene, pixel, 0).rgb;
    let depth = textureLoad(depth_texture, pixel, 0);
    let surface = textureLoad(normal_texture, pixel, 0);
    let reflecting = textureLoad(reflectance_texture, pixel, 0);
    let sharp = 1.0 - smoothstep(ROUGH_START, ROUGH_END, surface.a);
    if depth >= 1.0 || sharp <= 0.0 || all(reflecting.rgb <= vec3<f32>(0.0)) {
        return vec4<f32>(color, 1.0);
    }

    let size = vec2<f32>(textureDimensions(scene));
    let uv = position.xy / size;
    let far = live.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let world = far.xyz / far.w;
    let n = normalize(surface.xyz);
    let view = normalize(world - live.camera_pos.xyz);
    let r = reflect(view, n);
    // A ray coming back at the camera hits things the screen doesn't show.
    let away = smoothstep(-0.6, -0.2, dot(r, view));
    if away <= 0.0 {
        return vec4<f32>(color, 1.0);
    }

    // The ray's two ends on screen. Along a straight line on screen, 1 /
    // distance changes evenly, so each step's distance comes from that.
    let near = live.ambient_sky.w;
    // Start a little off the surface (further away: depth is less exact).
    let start = world + n * (0.02 + 0.002 * length(world - live.camera_pos.xyz));
    let c0 = live.view_proj * vec4<f32>(start, 1.0);
    var reach = live.reflection.y;
    var c1 = live.view_proj * vec4<f32>(start + r * reach, 1.0);
    if c1.w < near {
        // It would pass behind the camera: stop just in front of it.
        reach *= (c0.w - near) / (c0.w - c1.w) * 0.98;
        c1 = live.view_proj * vec4<f32>(start + r * reach, 1.0);
    }
    let uv0 = to_uv(c0);
    let uv1 = to_uv(c1);
    let k0 = 1.0 / c0.w;
    let k1 = 1.0 / c1.w;
    let pixels = length((uv1 - uv0) * size);
    let steps = clamp(pixels, 1.0, live.reflection.x);

    var found = -1.0;
    var before = 0.0;
    var last = c0.w;
    for (var i = 1.0; i <= steps; i += 1.0) {
        let f = i / steps;
        let at = mix(uv0, uv1, f);
        if any(at < vec2<f32>(0.0)) || any(at >= vec2<f32>(1.0)) {
            break;
        }
        let ray = 1.0 / mix(k0, k1, f);
        let surface_at = scene_distance(vec2<i32>(at * size));
        // Behind a surface now, and in front of it (or just inside) a step
        // ago: it went in here. Further behind, it's passing behind the
        // whole thing, which isn't a hit.
        if ray - surface_at > 0.02 && min(last, ray) - surface_at < 0.15 {
            found = f;
            break;
        }
        before = f;
        last = ray;
    }
    if found < 0.0 {
        return vec4<f32>(color, 1.0);
    }
    // Close in on where it went behind.
    for (var i = 0; i < 5; i++) {
        let f = (before + found) * 0.5;
        let at = mix(uv0, uv1, f);
        let ray = 1.0 / mix(k0, k1, f);
        if ray - scene_distance(vec2<i32>(at * size)) > 0.02 {
            found = f;
        } else {
            before = f;
        }
    }
    let at = mix(uv0, uv1, found);
    let seen = textureLoad(scene, vec2<i32>(at * size), 0).rgb;
    // Fade out toward the screen's edge, where rays start to miss.
    let margin = min(at, 1.0 - at);
    let inside = smoothstep(0.0, 0.08, min(margin.x, margin.y));

    let index = i32(reflecting.a % 32.0) - 1;
    let cube = cube_reflection(world, r, surface.a, index);
    let amount = sharp * away * inside;
    return vec4<f32>(max(color + reflecting.rgb * (seen - cube) * amount, vec3<f32>(0.0)), 1.0);
}

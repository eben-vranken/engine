// Forward pass at internal resolution. Output is linear HDR; tone mapping
// happens later in the upscale pass.

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: direction the sunlight travels.
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
    // rgb: color, a: density per meter.
    fog: vec4<f32>,
    // x: texture repeats per meter.
    params: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    color: vec4<f32>,
    // x: selection highlight, 0 or 1.
    highlight: vec4<f32>,
};

// Linear version of the UI amber, for the selection highlight.
const HIGHLIGHT: vec3<f32> = vec3<f32>(1.0, 0.34, 0.025);

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var grid_texture: texture_2d<f32>;
@group(0) @binding(2) var grid_sampler: sampler;
@group(1) @binding(0) var<uniform> object: Object;

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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let albedo = textureSample(grid_texture, grid_sampler, in.uv * frame.params.x).rgb
        * object.color.rgb;

    let sun = max(dot(n, -frame.sun_dir.xyz), 0.0) * frame.sun_color.rgb;
    let ambient = mix(frame.ambient_ground.rgb, frame.ambient_sky.rgb, n.y * 0.5 + 0.5);
    var color = albedo * (sun + ambient);

    // Selected: amber tint, stronger toward the silhouette.
    let to_camera = normalize(frame.camera_pos.xyz - in.world);
    let rim = pow(1.0 - max(dot(n, to_camera), 0.0), 2.0);
    color = mix(color, HIGHLIGHT * 1.5, object.highlight.x * (0.3 + 0.6 * rim));

    let distance = length(in.world - frame.camera_pos.xyz);
    let fog = 1.0 - exp(-distance * frame.fog.a);
    color = mix(color, frame.fog.rgb, fog);

    return vec4<f32>(color, 1.0);
}

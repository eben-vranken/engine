// Volumetric fog, step 2: walk each column of the froxel grid away from
// the camera, adding up the light scattered toward the camera and how much
// of what's behind still shows through. Each slice then holds the fog
// between the camera and that depth: rgb = light added, a = transmittance.

struct Params {
    camera_forward: vec4<f32>,
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    // x: near, y: far.
    depth: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var scatter: texture_3d<f32>;
@group(0) @binding(2) var output: texture_storage_3d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output);
    if id.x >= dims.x || id.y >= dims.y {
        return;
    }
    let size = vec3<f32>(dims);
    let uv = (vec2<f32>(id.xy) + 0.5) / size.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    // Meters along the ray per meter of depth (more toward the edges).
    let stretch = length(params.camera_forward.xyz + ndc.x * params.camera_right.xyz
        + ndc.y * params.camera_up.xyz);
    let near = params.depth.x;
    let ratio = params.depth.y / near;

    var light = vec3<f32>(0.0);
    var transmittance = 1.0;
    var start = near;
    for (var z = 0u; z < dims.z; z++) {
        let end = near * pow(ratio, f32(z + 1u) / size.z);
        let span = (end - start) * stretch;
        start = end;
        let cell = textureLoad(scatter, vec3<u32>(id.xy, z), 0);
        let density = max(cell.a, 1e-6);
        let through = exp(-density * span);
        // Light scattered inside the cell, dimmed by the fog in front of it
        // and by the cell itself (integrated exactly over its length).
        light += transmittance * cell.rgb * (1.0 - through) / density;
        transmittance *= through;
        textureStore(output, vec3<u32>(id.xy, z), vec4<f32>(light, transmittance));
    }
}

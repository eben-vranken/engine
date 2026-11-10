// Depth-only pass into a shadow map, from a light's point of view.

struct Pass {
    view_proj: mat4x4<f32>,
};

// Only the start of the scene shader's `Instance` is read; the stride
// must match it (320 bytes).
struct Instance {
    model: mat4x4<f32>,
    rest: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<uniform> pass_: Pass;
@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> visible: array<u32>;

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @builtin(instance_index) slot: u32,
) -> @builtin(position) vec4<f32> {
    let model = instances[visible[slot]].model;
    return pass_.view_proj * model * vec4<f32>(position, 1.0);
}

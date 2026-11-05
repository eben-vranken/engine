// Depth-only pass into a shadow map, from a light's point of view.

struct Pass {
    view_proj: mat4x4<f32>,
};

// Only the start of the scene shader's object uniform is needed.
struct Object {
    model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> pass_: Pass;
@group(1) @binding(0) var<uniform> object: Object;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return pass_.view_proj * object.model * vec4<f32>(position, 1.0);
}

// Nearest-neighbor upscale of the internal HDR image to the window, with
// AgX tone mapping. Drawn as one fullscreen triangle into a viewport that
// covers exactly internal size × factor, so every internal pixel becomes a
// factor × factor block.

struct Params {
    // Top-left of the upscaled image in output pixels (letterbox offset).
    origin: vec2<f32>,
    factor: f32,
    // 1 when the surface is sRGB and expects linear values.
    linear_output: u32,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene));
    let texel = clamp(
        vec2<i32>((position.xy - params.origin) / params.factor),
        vec2<i32>(0),
        size - 1,
    );
    var color = agx(textureLoad(scene, texel, 0).rgb);
    if params.linear_output != 0u {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, 1.0);
}

// AgX tone mapping (Troy Sobotka), polynomial fit by Benjamin Wrensch.
// Takes linear Rec.709 HDR, returns display-encoded (sRGB-like) values.
fn agx(linear: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        0.842479062253094, 0.0423282422610123, 0.0423756549057051,
        0.0784335999999992, 0.878468636469772, 0.0784336,
        0.0792237451477643, 0.0791661274605434, 0.879142973793104,
    );
    let outset = mat3x3<f32>(
        1.19687900512017, -0.0528968517574562, -0.0529716355144438,
        -0.0980208811401368, 1.15190312990417, -0.0980434501171241,
        -0.0990297440797205, -0.0989611768448433, 1.15107367264116,
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;

    var v = inset * max(linear, vec3<f32>(1e-10));
    v = clamp(log2(v), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);

    let v2 = v * v;
    let v4 = v2 * v2;
    v = 15.5 * v4 * v2 - 40.14 * v4 * v + 31.96 * v4 - 6.868 * v2 * v
        + 0.4298 * v2 + 0.1191 * v - 0.00232;

    return clamp(outset * v, vec3<f32>(0.0), vec3<f32>(1.0));
}

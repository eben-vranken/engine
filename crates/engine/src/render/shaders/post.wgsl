// Post-processing and upscale to the window, in one fullscreen triangle
// drawn into a viewport that covers exactly internal size × factor:
//
// 1. nearest-neighbor read of the internal HDR image (every internal pixel
//    becomes a factor × factor block)
// 2. bloom mixed in, exposure
// 3. AgX tone mapping
// 4. color grading through a 3D LUT, blending between the previous and
//    the current zone's grade
// 5. optional retro dithering: few color levels per channel, with an
//    ordered pattern per internal pixel hiding the steps
// 6. film grain, one value per internal pixel, new every frame

struct Params {
    // Top-left of the upscaled image in output pixels (letterbox offset).
    origin: vec2<f32>,
    factor: f32,
    // 1 when the surface is sRGB and expects linear values.
    linear_output: u32,
    // How much of the bloom to mix in, 0–1.
    bloom: f32,
    grain: f32,
    // Linear multiplier before tone mapping.
    exposure: f32,
    // 0 = all `lut_from`, 1 = all `lut_to`.
    lut_mix: f32,
    // Changes every frame, for the grain.
    frame: u32,
    // 1 / bloom levels: the chain adds every level up.
    bloom_scale: f32,
    // Retro: color levels per channel, dithered (0 = off).
    dither: u32,
    _pad: u32,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var bloom_texture: texture_2d<f32>;
@group(0) @binding(3) var lut_from: texture_3d<f32>;
@group(0) @binding(4) var lut_to: texture_3d<f32>;
@group(0) @binding(5) var linear_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene));
    let local = (position.xy - params.origin) / params.factor;
    let texel = clamp(vec2<i32>(local), vec2<i32>(0), size - 1);
    var hdr = textureLoad(scene, texel, 0).rgb;

    // Bloom is smooth, so it's sampled per output pixel, not per block.
    let uv = local / vec2<f32>(size);
    let bloom = textureSampleLevel(bloom_texture, linear_sampler, uv, 0.0).rgb;
    hdr = mix(hdr, bloom * params.bloom_scale, params.bloom) * params.exposure;

    var color = agx(hdr);

    // LUT coordinates hit texel centers at 0 and 1.
    let n = f32(textureDimensions(lut_to).x);
    let lut_uv = color * ((n - 1.0) / n) + 0.5 / n;
    let graded_from = textureSampleLevel(lut_from, linear_sampler, lut_uv, 0.0).rgb;
    let graded_to = textureSampleLevel(lut_to, linear_sampler, lut_uv, 0.0).rgb;
    color = mix(graded_from, graded_to, params.lut_mix);

    if params.dither > 1u {
        // A 4×4 Bayer pattern: each pixel rounds up or down at its own
        // threshold, so an area averages to the color in between.
        let p = vec2<u32>(texel);
        let xy = p.x ^ p.y;
        let order = ((xy & 1u) << 3u) | ((p.y & 1u) << 2u) | (xy & 2u) | ((p.y & 2u) >> 1u);
        let threshold = (f32(order) + 0.5) / 16.0;
        let steps = f32(params.dither) - 1.0;
        color = clamp(floor(color * steps + threshold) / steps, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    // Grain: strongest in the mid tones, like film.
    let noise = hash(vec3<u32>(vec2<u32>(texel), params.frame)) - 0.5;
    let weight = 1.0 - abs(dot(color, vec3<f32>(0.333)) * 2.0 - 1.0) * 0.6;
    color = clamp(color + noise * params.grain * weight, vec3<f32>(0.0), vec3<f32>(1.0));

    if params.linear_output != 0u {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, 1.0);
}

// Uniform 0–1 from three integers (PCG-style mixing).
fn hash(v: vec3<u32>) -> f32 {
    var x = v.x * 1664525u + v.y * 1013904223u + v.z * 2654435769u;
    x ^= x >> 16u;
    x *= 2246822519u;
    x ^= x >> 13u;
    x *= 3266489917u;
    x ^= x >> 16u;
    return f32(x) / 4294967295.0;
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

// Bloom: the HDR image is blurred by halving it a few times and adding the
// levels back up again (the "Call of Duty" filter: a 13-tap downsample and
// a 3×3 tent upsample). The result is mixed into the image in the post
// pass, so bright lights and emissive surfaces glow past their edges.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn tap(uv: vec2<f32>, offset: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, linear_sampler, uv + offset * texel, 0.0).rgb;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// 13 taps in overlapping 2×2 boxes. `first`: weight each box by inverse
// brightness (Karis average) so single very bright pixels don't flicker.
fn downsample(position: vec2<f32>, first: bool) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let texel = 1.0 / size;
    // This target is half the source size: its pixel centers map to source
    // texel corners.
    let uv = position * 2.0 * texel;
    let a = tap(uv, vec2(-2.0, -2.0), texel);
    let b = tap(uv, vec2(0.0, -2.0), texel);
    let c = tap(uv, vec2(2.0, -2.0), texel);
    let d = tap(uv, vec2(-2.0, 0.0), texel);
    let e = tap(uv, vec2(0.0, 0.0), texel);
    let f = tap(uv, vec2(2.0, 0.0), texel);
    let g = tap(uv, vec2(-2.0, 2.0), texel);
    let h = tap(uv, vec2(0.0, 2.0), texel);
    let i = tap(uv, vec2(2.0, 2.0), texel);
    let j = tap(uv, vec2(-1.0, -1.0), texel);
    let k = tap(uv, vec2(1.0, -1.0), texel);
    let l = tap(uv, vec2(-1.0, 1.0), texel);
    let m = tap(uv, vec2(1.0, 1.0), texel);
    if first {
        var boxes = array<vec3<f32>, 5>(
            (j + k + l + m) * 0.25,
            (a + b + d + e) * 0.25,
            (b + c + e + f) * 0.25,
            (d + e + g + h) * 0.25,
            (e + f + h + i) * 0.25,
        );
        let weights = array<f32, 5>(0.5, 0.125, 0.125, 0.125, 0.125);
        var sum = vec3<f32>(0.0);
        var total = 0.0;
        for (var n = 0; n < 5; n++) {
            let w = weights[n] / (1.0 + luma(boxes[n]));
            sum += boxes[n] * w;
            total += w;
        }
        return sum / total;
    }
    var out = e * 0.125;
    out += (a + c + g + i) * 0.03125;
    out += (b + d + f + h) * 0.0625;
    out += (j + k + l + m) * 0.125;
    return out;
}

@fragment
fn fs_down_first(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Clamp what goes in: stray NaNs or huge values would smear everywhere.
    return vec4<f32>(min(downsample(position.xy, true), vec3<f32>(1000.0)), 1.0);
}

@fragment
fn fs_down(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample(position.xy, false), 1.0);
}

// Added onto the next larger level (additive blending).
@fragment
fn fs_up(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let source_size = vec2<f32>(textureDimensions(source));
    let texel = 1.0 / source_size;
    // This target is twice the source size.
    let uv = position.xy * 0.5 * texel;
    var sum = tap(uv, vec2(0.0, 0.0), texel) * 4.0;
    sum += (tap(uv, vec2(-1.0, 0.0), texel) + tap(uv, vec2(1.0, 0.0), texel)
        + tap(uv, vec2(0.0, -1.0), texel) + tap(uv, vec2(0.0, 1.0), texel)) * 2.0;
    sum += tap(uv, vec2(-1.0, -1.0), texel) + tap(uv, vec2(1.0, -1.0), texel)
        + tap(uv, vec2(-1.0, 1.0), texel) + tap(uv, vec2(1.0, 1.0), texel);
    return vec4<f32>(sum / 16.0, 1.0);
}

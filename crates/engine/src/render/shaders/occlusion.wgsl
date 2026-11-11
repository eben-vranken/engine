// Live traced occlusion: how open each point on screen is to its
// surroundings, found by tracing short rays through the scene's triangles
// (a BVH built on the CPU, see `bvh.rs`). Indirect light is dimmed by it in
// the scene pass a frame later, so moving things darken the floor they
// stand on and sit in their corners, which the bake can't know.
//
// Cheap because it's coarse and slow to change: a fixed grid of cells (not
// the screen's resolution), a ray or two per cell per frame, blended into
// what the same point had last frame (found by reprojecting it).
//
// Surfaces lit from a lightmap already carry the shade of everything that
// stays put, so they only look for moving things; everything else looks
// for everything.

// Matches `LiveUniform` in `live.rs` (and `Live` in `reflect.wgsl`).
struct Live {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // Last frame's.
    prev_view_proj: mat4x4<f32>,
    // w: frame number.
    camera_pos: vec4<f32>,
    // rgb: sky color. w: near plane.
    ambient_sky: vec4<f32>,
    // rgb: ground color. w: far plane.
    ambient_ground: vec4<f32>,
    // x: root node of the triangles that stay put (the moving ones' is 0).
    // y: rays per cell. z: meters a ray looks. w: 1 = last frame's result
    // can be blended with.
    occlusion: vec4<f32>,
    // x: most steps along a reflected ray. y: meters it looks.
    reflection: vec4<f32>,
};

struct Node {
    min: vec3<f32>,
    // A leaf's first triangle; otherwise its first child.
    first: u32,
    max: vec3<f32>,
    // Triangles in a leaf; 0 = not a leaf.
    count: u32,
};

struct Triangle {
    a: vec3<f32>,
    ab: vec3<f32>,
    ac: vec3<f32>,
};

const PI: f32 = 3.14159265;
// How much of each frame's result goes into the running average.
const BLEND: f32 = 0.1;

@group(0) @binding(0) var<uniform> live: Live;
@group(0) @binding(1) var depth_texture: texture_depth_2d;
// xyz: world normal, a: roughness.
@group(0) @binding(2) var normal_texture: texture_2d<f32>;
// a: 32 or more = lit from a lightmap.
@group(0) @binding(3) var reflectance_texture: texture_2d<f32>;
// Last frame's result. r: openness, g: distance from the camera plane.
@group(0) @binding(4) var history: texture_2d<f32>;
@group(0) @binding(5) var history_sampler: sampler;
@group(0) @binding(6) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var<storage, read> nodes: array<Node>;
@group(0) @binding(8) var<storage, read> triangles: array<Triangle>;

fn hash(v: vec3<u32>) -> u32 {
    var x = v.x * 1664525u + v.y * 1013904223u + v.z * 2654435769u;
    x ^= x >> 16u;
    x *= 2246822519u;
    x ^= x >> 13u;
    x *= 3266489917u;
    x ^= x >> 16u;
    return x;
}

fn next(seed: ptr<function, u32>) -> f32 {
    *seed = *seed * 747796405u + 2891336453u;
    let x = ((*seed >> ((*seed >> 28u) + 4u)) ^ *seed) * 277803737u;
    return f32((x >> 22u) ^ x) / 4294967295.0;
}

// Distance to the nearest triangle under `root` along a ray, or `reach` if
// none is nearer.
fn trace(root: u32, origin: vec3<f32>, direction: vec3<f32>, reach: f32) -> f32 {
    let safe = select(direction, vec3<f32>(1e-8), abs(direction) < vec3<f32>(1e-8));
    let inverse = 1.0 / safe;
    var nearest = reach;
    var stack: array<u32, 32>;
    stack[0] = root;
    var top = 1;
    while top > 0 {
        top -= 1;
        let node = nodes[stack[top]];
        // Where the ray enters and leaves the node's box.
        let t0 = (node.min - origin) * inverse;
        let t1 = (node.max - origin) * inverse;
        let lo = min(t0, t1);
        let hi = max(t0, t1);
        let enter = max(max(lo.x, lo.y), max(lo.z, 0.0));
        let leave = min(min(hi.x, hi.y), min(hi.z, nearest));
        if enter > leave {
            continue;
        }
        if node.count == 0u {
            if top <= 30 {
                stack[top] = node.first;
                stack[top + 1] = node.first + 1u;
                top += 2;
            }
            continue;
        }
        for (var i = node.first; i < node.first + node.count; i++) {
            // Möller–Trumbore, either side of the triangle.
            let triangle = triangles[i];
            let p = cross(direction, triangle.ac);
            let det = dot(triangle.ab, p);
            if abs(det) < 1e-9 {
                continue;
            }
            let s = origin - triangle.a;
            let u = dot(s, p) / det;
            let q = cross(s, triangle.ab);
            let v = dot(direction, q) / det;
            let t = dot(triangle.ac, q) / det;
            if u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0 && t < nearest {
                nearest = t;
            }
        }
    }
    return nearest;
}

// A direction in the hemisphere around n, more often near n (cosine
// weighted, as matte surfaces gather light).
fn around(n: vec3<f32>, u: f32, v: f32) -> vec3<f32> {
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.9);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    let r = sqrt(u);
    let angle = 2.0 * PI * v;
    return normalize(t * (r * cos(angle)) + b * (r * sin(angle)) + n * sqrt(max(1.0 - u, 0.0)));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let grid = textureDimensions(output);
    if any(id.xy >= grid) {
        return;
    }
    // The screen pixel under the middle of this cell.
    let size = textureDimensions(depth_texture);
    let cell_uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(grid);
    let pixel = min(vec2<u32>(cell_uv * vec2<f32>(size)), size - 1u);
    let depth = textureLoad(depth_texture, pixel, 0);
    if depth >= 1.0 {
        // Sky.
        textureStore(output, id.xy, vec4<f32>(1.0, 0.0, 0.0, 0.0));
        return;
    }
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(size);
    let far = live.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let world = far.xyz / far.w;
    let n = normalize(textureLoad(normal_texture, pixel, 0).xyz);
    let lightmapped = textureLoad(reflectance_texture, pixel, 0).a >= 32.0;
    let clip = live.view_proj * vec4<f32>(world, 1.0);

    // What this point had last frame, if it was on screen and it's the same
    // surface (about as far from the camera as what was stored there).
    var old = -1.0;
    let before = live.prev_view_proj * vec4<f32>(world, 1.0);
    if live.occlusion.w > 0.5 && before.w > 0.0 {
        let prev_uv = before.xy / before.w * vec2<f32>(0.5, -0.5) + 0.5;
        if all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0)) {
            let stored = textureSampleLevel(history, history_sampler, prev_uv, 0.0);
            if abs(stored.g - before.w) < 0.05 + 0.1 * before.w {
                old = stored.r;
            }
        }
    }

    // Nothing to blend with: more rays, so the first frame isn't noise.
    var rays = u32(live.occlusion.y);
    if old < 0.0 {
        rays *= 4u;
    }
    let reach = live.occlusion.z;
    // Start a little off the surface (further away: depth is less exact).
    let origin = world + n * (0.01 + 0.002 * clip.w);
    var seed = hash(vec3<u32>(id.xy, u32(live.camera_pos.w)));
    var open = 0.0;
    for (var i = 0u; i < rays; i++) {
        let direction = around(n, next(&seed), next(&seed));
        var t = trace(0u, origin, direction, reach);
        if !lightmapped {
            t = trace(u32(live.occlusion.x), origin, direction, t);
        }
        // Near things block more than far ones.
        open += t / reach;
    }
    open /= f32(rays);
    if old >= 0.0 {
        open = mix(old, open, BLEND);
    }
    textureStore(output, id.xy, vec4<f32>(open, clip.w, 0.0, 0.0));
}

// Affine transform of every vertex, matching glam's `Mat4::transform_point3`
// (homogeneous divide included, so perspective transforms stay correct).

struct Transform {
    matrix: mat4x4<f32>,
    range: vec4<u32>,
};

@group(0) @binding(0) var<uniform> params: Transform;
@group(0) @binding(1) var<storage, read> positions: array<f32>;
@group(0) @binding(2) var<storage, read_write> out_positions: array<f32>;

@compute @workgroup_size(64)
fn transform(@builtin(global_invocation_id) id: vec3<u32>) {
    let vertex = params.range.x + id.x;
    if (vertex >= params.range.y) {
        return;
    }
    let base = vertex * 3u;
    let p = vec3<f32>(positions[base], positions[base + 1u], positions[base + 2u]);
    let homogeneous = params.matrix * vec4<f32>(p, 1.0);
    let moved = homogeneous.xyz / homogeneous.w;
    out_positions[base] = moved.x;
    out_positions[base + 1u] = moved.y;
    out_positions[base + 2u] = moved.z;
}

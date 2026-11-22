// out[i] = position[i] + normal[i] * amount[i]. Used by the rock detail bake:
// the layered displacement is evaluated on the CPU or on the GPU, this pass
// applies it without a second Python-free round trip.

struct Params {
    start: u32,
    end: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> positions: array<f32>;
@group(0) @binding(2) var<storage, read> normals: array<f32>;
@group(0) @binding(3) var<storage, read> amounts: array<f32>;
@group(0) @binding(4) var<storage, read_write> out_positions: array<f32>;

@compute @workgroup_size(64)
fn displace(@builtin(global_invocation_id) id: vec3<u32>) {
    let vertex = params.start + id.x;
    if (vertex >= params.end) {
        return;
    }
    let base = vertex * 3u;
    let p = vec3<f32>(positions[base], positions[base + 1u], positions[base + 2u]);
    let n = vec3<f32>(normals[base], normals[base + 1u], normals[base + 2u]);
    let moved = p + n * amounts[vertex];
    out_positions[base] = moved.x;
    out_positions[base + 1u] = moved.y;
    out_positions[base + 2u] = moved.z;
}

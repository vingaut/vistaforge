// One Taubin/Laplacian pass: out[i] = in[i] + (mean(neighbours) - in[i]) * factor.
// Neighbours are read from the shared CSR built on the CPU, so every vertex
// sums the same values in the same order as the reference implementation.

struct Params {
    range: vec4<u32>,
    value: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> positions_in: array<f32>;
@group(0) @binding(2) var<storage, read> neighbour_offsets: array<u32>;
@group(0) @binding(3) var<storage, read> neighbours: array<u32>;
@group(0) @binding(4) var<storage, read_write> positions_out: array<f32>;

fn sample(index: u32) -> vec3<f32> {
    let base = index * 3u;
    return vec3<f32>(positions_in[base], positions_in[base + 1u], positions_in[base + 2u]);
}

@compute @workgroup_size(64)
fn smooth_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let vertex = params.range.x + id.x;
    if (vertex >= params.range.y) {
        return;
    }
    let factor = params.value.x;
    var p = sample(vertex);
    let begin = neighbour_offsets[vertex];
    let stop = neighbour_offsets[vertex + 1u];
    if (stop > begin) {
        var average = vec3<f32>(0.0);
        for (var i = begin; i < stop; i = i + 1u) {
            average += sample(neighbours[i]);
        }
        average = average / f32(stop - begin);
        p = p + (average - p) * factor;
    }
    let base = vertex * 3u;
    positions_out[base] = p.x;
    positions_out[base + 1u] = p.y;
    positions_out[base + 2u] = p.z;
}

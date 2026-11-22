// Pass 2 of the normal computation: one invocation per vertex sums its incident
// faces in ascending corner order, exactly like the CPU gather.

struct Params {
    start: u32,
    end: u32,
    weighted: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> face_normals: array<f32>;
@group(0) @binding(2) var<storage, read> corner_faces: array<u32>;
@group(0) @binding(3) var<storage, read> corner_weights: array<f32>;
@group(0) @binding(4) var<storage, read> vertex_offsets: array<u32>;
@group(0) @binding(5) var<storage, read> vertex_corners: array<u32>;
@group(0) @binding(6) var<storage, read_write> out_normals: array<f32>;

fn normalize_or_zero(v: vec3<f32>) -> vec3<f32> {
    let len2 = dot(v, v);
    if (len2 > 0.0) {
        let inv = 1.0 / sqrt(len2);
        if (inv < 3.402823e38) {
            return v * inv;
        }
    }
    return vec3<f32>(0.0);
}

@compute @workgroup_size(64)
fn vertex_normals(@builtin(global_invocation_id) id: vec3<u32>) {
    let vertex = params.start + id.x;
    if (vertex >= params.end) {
        return;
    }
    var sum = vec3<f32>(0.0);
    let begin = vertex_offsets[vertex];
    let stop = vertex_offsets[vertex + 1u];
    for (var i = begin; i < stop; i = i + 1u) {
        let corner = vertex_corners[i];
        let face = corner_faces[corner];
        // Area normals use the unweighted cross product of their triangle.
        let weight = select(1.0, corner_weights[corner], params.weighted == 1u);
        let base = face * 3u;
        sum += vec3<f32>(face_normals[base], face_normals[base + 1u], face_normals[base + 2u]) * weight;
    }
    let unit = normalize_or_zero(sum);
    out_normals[vertex * 3u] = unit.x;
    out_normals[vertex * 3u + 1u] = unit.y;
    out_normals[vertex * 3u + 2u] = unit.z;
}

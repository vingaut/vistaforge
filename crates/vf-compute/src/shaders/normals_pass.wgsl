// Pass 1 of the normal computation: one invocation per face writes the Newell
// normal, one invocation per corner writes its angle weight. Both loops run in
// the same order as the CPU reference so the two backends agree.

struct Params {
    start: u32,
    end: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> positions: array<f32>;
@group(0) @binding(2) var<storage, read> face_offsets: array<u32>;
@group(0) @binding(3) var<storage, read> corners: array<u32>;
@group(0) @binding(4) var<storage, read> corner_faces: array<u32>;
@group(0) @binding(5) var<storage, read_write> out_values: array<f32>;

fn pos(index: u32) -> vec3<f32> {
    let base = index * 3u;
    return vec3<f32>(positions[base], positions[base + 1u], positions[base + 2u]);
}

// Mirrors glam's `normalize_or_zero`: reciprocal square root, zero when the
// length underflows to nothing.
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
fn face_normals(@builtin(global_invocation_id) id: vec3<u32>) {
    let face = params.start + id.x;
    if (face >= params.end) {
        return;
    }
    let begin = face_offsets[face];
    let stop = face_offsets[face + 1u];
    let count = stop - begin;
    var normal = vec3<f32>(0.0);
    if (count >= 3u) {
        for (var i = 0u; i < count; i = i + 1u) {
            let a = pos(corners[begin + i]);
            let b = pos(corners[begin + ((i + 1u) % count)]);
            normal.x += (a.y - b.y) * (a.z + b.z);
            normal.y += (a.z - b.z) * (a.x + b.x);
            normal.z += (a.x - b.x) * (a.y + b.y);
        }
    }
    let unit = normalize_or_zero(normal);
    out_values[face * 3u] = unit.x;
    out_values[face * 3u + 1u] = unit.y;
    out_values[face * 3u + 2u] = unit.z;
}

@compute @workgroup_size(64)
fn face_cross(@builtin(global_invocation_id) id: vec3<u32>) {
    let face = params.start + id.x;
    if (face >= params.end) {
        return;
    }
    let begin = face_offsets[face];
    let stop = face_offsets[face + 1u];
    var product = vec3<f32>(0.0);
    if (stop - begin >= 3u) {
        let a = pos(corners[begin]);
        let b = pos(corners[begin + 1u]);
        let c = pos(corners[begin + 2u]);
        product = cross(b - a, c - a);
    }
    out_values[face * 3u] = product.x;
    out_values[face * 3u + 1u] = product.y;
    out_values[face * 3u + 2u] = product.z;
}

@compute @workgroup_size(64)
fn corner_weights(@builtin(global_invocation_id) id: vec3<u32>) {
    let corner = params.start + id.x;
    if (corner >= params.end) {
        return;
    }
    let face = corner_faces[corner];
    let begin = face_offsets[face];
    let count = face_offsets[face + 1u] - begin;
    if (count < 3u) {
        out_values[corner] = 1e-4;
        return;
    }
    let local = corner - begin;
    let previous = pos(corners[begin + ((local + count - 1u) % count)]);
    let current = pos(corners[corner]);
    let next = pos(corners[begin + ((local + 1u) % count)]);
    let incoming = normalize_or_zero(current - previous);
    let outgoing = normalize_or_zero(next - current);
    let angle = acos(clamp(dot(incoming, outgoing), -1.0, 1.0));
    out_values[corner] = max(angle, 1e-4);
}

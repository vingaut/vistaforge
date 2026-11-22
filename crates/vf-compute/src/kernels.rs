//! Geometry kernels with a CPU and a GPU implementation.
//!
//! Every kernel is data-parallel per vertex or per face and reads its adjacency
//! from the shared [`crate::mesh_graph`] structures. The CPU path runs on the
//! shared [`vf_core::compute`] pool in fixed-size chunks, so the thread count
//! chosen in the preferences applies to geometry work and cannot change the
//! result. The GPU path is used only when the preference selects it and the
//! mesh is large enough to amortise the transfer; any failure falls back here.

use glam::{Mat4, Vec3};
use rayon::prelude::*;

use crate::backend::{ComputeBackend, backend_state};
use crate::mesh_graph::{FaceGraph, Neighbours, VertexCorners};

/// Vertices per rayon task. Fixed, so the partitioning never depends on the
/// number of threads.
const CHUNK: usize = 4096;

/// Below this vertex count, uploading a buffer costs more than the CPU kernel.
const MIN_GPU_VERTICES: usize = 2_048;

/// Which engine actually ran a kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendUsed {
    Cpu,
    Gpu,
}

/// A kernel result together with the engine that produced it. The UI shows the
/// engine so a silent fallback is visible.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome<T> {
    pub value: T,
    pub backend: BackendUsed,
}

impl<T> Outcome<T> {
    pub fn into_value(self) -> T {
        self.value
    }
}

/// True when the current preference asks for a GPU backend.
pub fn gpu_selected() -> bool {
    backend_state().backend.uses_gpu()
}

fn wants_gpu(vertices: usize) -> bool {
    vertices >= MIN_GPU_VERTICES && backend_state().backend.uses_gpu()
}

/// Angle-weighted vertex normals, bit-identical to `Mesh::recompute_normals`.
///
/// The reference implementation scatters `face_normal * corner_angle` into the
/// vertex slots while walking faces in ascending order. The parallel version
/// gathers instead: each vertex reads its incident corners, which the
/// [`VertexCorners`] structure stores in ascending order, so the additions
/// happen in exactly the same sequence.
pub fn normals(
    positions: &[Vec3],
    graph: &FaceGraph,
    corners: &VertexCorners,
) -> Outcome<Vec<Vec3>> {
    if wants_gpu(positions.len())
        && let Some(value) = try_gpu(|state| crate::gpu::normals(state, positions, graph, corners))
    {
        return Outcome {
            value,
            backend: BackendUsed::Gpu,
        };
    }
    Outcome {
        value: normals_cpu(positions, graph, corners),
        backend: BackendUsed::Cpu,
    }
}

/// CPU reference for [`normals`].
pub fn normals_cpu(positions: &[Vec3], graph: &FaceGraph, corners: &VertexCorners) -> Vec<Vec3> {
    let face_count = graph.face_count();
    let corner_count = graph.corner_count();
    if positions.is_empty() || face_count == 0 {
        return vec![Vec3::ZERO; positions.len()];
    }
    let mut face_normals = vec![Vec3::ZERO; face_count];
    let mut corner_weights = vec![0.0f32; corner_count];
    let corner_faces = graph.corner_faces();
    vf_core::compute::install(|| {
        face_normals
            .par_iter_mut()
            .enumerate()
            .for_each(|(face, slot)| {
                *slot = graph.newell_normal(positions, face);
            });
        corner_weights
            .par_iter_mut()
            .enumerate()
            .for_each(|(corner, slot)| {
                let face = corner_faces[corner] as usize;
                *slot = corner_angle(positions, graph, face, corner);
            });
    });
    let mut out = vec![Vec3::ZERO; positions.len()];
    vf_core::compute::install(|| {
        out.par_chunks_mut(CHUNK)
            .enumerate()
            .for_each(|(block, chunk)| {
                let base = block * CHUNK;
                for (offset, slot) in chunk.iter_mut().enumerate() {
                    let vertex = base + offset;
                    if vertex >= positions.len() {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    for corner in corners.range(vertex) {
                        let corner = corners.corners()[corner] as usize;
                        let face = corner_faces[corner] as usize;
                        sum += face_normals[face] * corner_weights[corner].max(1e-4);
                    }
                    *slot = sum.normalize_or_zero();
                }
            });
    });
    out
}

/// Interior angle of `face` at `corner`, clamped exactly like the reference.
fn corner_angle(positions: &[Vec3], graph: &FaceGraph, face: usize, corner: usize) -> f32 {
    let corners = graph.face(face);
    let count = corners.len();
    if count < 3 {
        return 0.0;
    }
    let local = corner - graph.face_range(face).start;
    let previous = positions[corners[(local + count - 1) % count] as usize];
    let current = positions[corners[local] as usize];
    let next = positions[corners[(local + 1) % count] as usize];
    (current - previous)
        .normalize_or_zero()
        .dot((next - current).normalize_or_zero())
        .clamp(-1.0, 1.0)
        .acos()
}

/// Area-weighted vertex normals: the unnormalised cross product of every face
/// accumulated into its vertices, matching `RenderMesh::recompute_normals`.
pub fn area_normals(
    positions: &[Vec3],
    graph: &FaceGraph,
    corners: &VertexCorners,
) -> Outcome<Vec<Vec3>> {
    if wants_gpu(positions.len())
        && let Some(value) =
            try_gpu(|state| crate::gpu::area_normals(state, positions, graph, corners))
    {
        return Outcome {
            value,
            backend: BackendUsed::Gpu,
        };
    }
    Outcome {
        value: area_normals_cpu(positions, graph, corners),
        backend: BackendUsed::Cpu,
    }
}

/// CPU reference for [`area_normals`].
pub fn area_normals_cpu(
    positions: &[Vec3],
    graph: &FaceGraph,
    corners: &VertexCorners,
) -> Vec<Vec3> {
    let face_count = graph.face_count();
    if positions.is_empty() || face_count == 0 {
        return vec![Vec3::ZERO; positions.len()];
    }
    let mut face_cross = vec![Vec3::ZERO; face_count];
    let corner_faces = graph.corner_faces();
    vf_core::compute::install(|| {
        face_cross
            .par_iter_mut()
            .enumerate()
            .for_each(|(face, slot)| {
                let indices = graph.face(face);
                if indices.len() < 3 {
                    return;
                }
                let a = positions[indices[0] as usize];
                let b = positions[indices[1] as usize];
                let c = positions[indices[2] as usize];
                *slot = (b - a).cross(c - a);
            });
    });
    let mut out = vec![Vec3::ZERO; positions.len()];
    vf_core::compute::install(|| {
        out.par_chunks_mut(CHUNK)
            .enumerate()
            .for_each(|(block, chunk)| {
                let base = block * CHUNK;
                for (offset, slot) in chunk.iter_mut().enumerate() {
                    let vertex = base + offset;
                    if vertex >= positions.len() {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    for corner in corners.range(vertex) {
                        let corner = corners.corners()[corner] as usize;
                        sum += face_cross[corner_faces[corner] as usize];
                    }
                    *slot = sum.normalize_or_zero();
                }
            });
    });
    out
}

/// Taubin (λ|μ) smoothing, matching `voxel::taubin_smooth`.
pub fn smooth(
    positions: &[Vec3],
    neighbours: &Neighbours,
    iterations: u32,
    lambda: f32,
    mu: f32,
) -> Outcome<Vec<Vec3>> {
    if iterations == 0 || positions.is_empty() {
        return Outcome {
            value: positions.to_vec(),
            backend: BackendUsed::Cpu,
        };
    }
    if wants_gpu(positions.len())
        && let Some(value) = try_gpu(|state| {
            crate::gpu::smooth(state, positions, neighbours, iterations, lambda, mu)
        })
    {
        return Outcome {
            value,
            backend: BackendUsed::Gpu,
        };
    }
    Outcome {
        value: smooth_cpu(positions, neighbours, iterations, lambda, mu),
        backend: BackendUsed::Cpu,
    }
}

/// CPU reference for [`smooth`].
pub fn smooth_cpu(
    positions: &[Vec3],
    neighbours: &Neighbours,
    iterations: u32,
    lambda: f32,
    mu: f32,
) -> Vec<Vec3> {
    let count = positions.len();
    let mut current = positions.to_vec();
    let mut next = vec![Vec3::ZERO; count];
    let offsets = neighbours.offsets();
    let values = neighbours.values();
    for _ in 0..iterations {
        for factor in [lambda, mu] {
            vf_core::compute::install(|| {
                next.par_chunks_mut(CHUNK)
                    .enumerate()
                    .for_each(|(block, chunk)| {
                        let base = block * CHUNK;
                        for (offset, slot) in chunk.iter_mut().enumerate() {
                            let vertex = base + offset;
                            if vertex >= count {
                                continue;
                            }
                            let begin = offsets[vertex] as usize;
                            let end = offsets[vertex + 1] as usize;
                            let p = current[vertex];
                            if end > begin {
                                let mut average = Vec3::ZERO;
                                for &other in &values[begin..end] {
                                    average += current[other as usize];
                                }
                                average /= (end - begin) as f32;
                                *slot = p + (average - p) * factor;
                            } else {
                                *slot = p;
                            }
                        }
                    });
            });
            std::mem::swap(&mut current, &mut next);
        }
    }
    current
}

/// `position + normal * amount`, one entry per vertex.
pub fn displace(positions: &[Vec3], normals: &[Vec3], amounts: &[f32]) -> Outcome<Vec<Vec3>> {
    let count = positions.len().min(normals.len()).min(amounts.len());
    if count == 0 {
        return Outcome {
            value: Vec::new(),
            backend: BackendUsed::Cpu,
        };
    }
    if wants_gpu(count)
        && let Some(value) =
            try_gpu(|state| crate::gpu::displace(state, positions, normals, amounts))
    {
        return Outcome {
            value,
            backend: BackendUsed::Gpu,
        };
    }
    let mut out = vec![Vec3::ZERO; count];
    vf_core::compute::install(|| {
        out.par_chunks_mut(CHUNK)
            .enumerate()
            .for_each(|(block, chunk)| {
                let base = block * CHUNK;
                for (offset, slot) in chunk.iter_mut().enumerate() {
                    let index = base + offset;
                    if index >= count {
                        continue;
                    }
                    *slot = positions[index] + normals[index] * amounts[index];
                }
            });
    });
    Outcome {
        value: out,
        backend: BackendUsed::Cpu,
    }
}

/// Affine transform of every vertex, matching `Mat4::transform_point3`.
pub fn transform(positions: &[Vec3], matrix: Mat4) -> Outcome<Vec<Vec3>> {
    if positions.is_empty() {
        return Outcome {
            value: Vec::new(),
            backend: BackendUsed::Cpu,
        };
    }
    if wants_gpu(positions.len())
        && let Some(value) = try_gpu(|state| crate::gpu::transform(state, positions, matrix))
    {
        return Outcome {
            value,
            backend: BackendUsed::Gpu,
        };
    }
    let mut out = vec![Vec3::ZERO; positions.len()];
    vf_core::compute::install(|| {
        out.par_chunks_mut(CHUNK)
            .enumerate()
            .for_each(|(block, chunk)| {
                let base = block * CHUNK;
                for (offset, slot) in chunk.iter_mut().enumerate() {
                    let index = base + offset;
                    if index >= positions.len() {
                        continue;
                    }
                    *slot = matrix.transform_point3(positions[index]);
                }
            });
    });
    Outcome {
        value: out,
        backend: BackendUsed::Cpu,
    }
}

/// Run a GPU kernel when the backend is compiled in; `None` means "fall back".
#[cfg(feature = "gpu")]
fn try_gpu<F>(job: F) -> Option<Vec<Vec3>>
where
    F: FnOnce(&crate::backend::BackendState) -> Result<Vec<Vec3>, String>,
{
    let state = backend_state();
    if state.backend == ComputeBackend::Cpu {
        return None;
    }
    job(&state).ok()
}

#[cfg(not(feature = "gpu"))]
fn try_gpu<F>(_job: F) -> Option<Vec<Vec3>>
where
    F: FnOnce(&crate::backend::BackendState) -> Result<Vec<Vec3>, String>,
{
    None
}

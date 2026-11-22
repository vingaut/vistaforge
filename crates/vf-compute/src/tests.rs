//! Kernel tests: the parallel implementations must reproduce the serial
//! reference bit for bit, whatever the thread count, and the GPU path must
//! agree with the CPU path within a small tolerance.

use std::sync::{Mutex, MutexGuard};

use glam::{Mat4, Vec3};

use crate::kernels::{area_normals, displace, normals_cpu, smooth_cpu, transform};
use crate::mesh_graph::{FaceGraph, Neighbours, VertexCorners};

/// The shared rayon pool is global, so the thread-count tests serialise.
fn pool_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

/// A wavy grid: enough curvature that normal weighting and smoothing matter.
fn grid(columns: usize, rows: usize, spacing: f32) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let mut positions = Vec::with_capacity((columns + 1) * (rows + 1));
    for y in 0..=rows {
        for x in 0..=columns {
            let fx = x as f32 / columns as f32;
            let fy = y as f32 / rows as f32;
            let z =
                (fx * std::f32::consts::TAU).sin() * 0.4 + (fy * std::f32::consts::PI).cos() * 0.3;
            positions.push(Vec3::new(
                x as f32 * spacing,
                y as f32 * spacing,
                z * spacing * 3.0,
            ));
        }
    }
    let stride = columns + 1;
    let mut faces = Vec::new();
    for y in 0..rows {
        for x in 0..columns {
            let a = (y * stride + x) as u32;
            let b = a + 1;
            let c = a + 1 + stride as u32;
            let d = a + stride as u32;
            // Quads: exercises the n-gon Newell normal, not just triangles.
            faces.push(vec![a, b, c, d]);
        }
    }
    (positions, faces)
}

fn graph_of(faces: &[Vec<u32>]) -> FaceGraph {
    FaceGraph::from_faces(faces.iter().map(Vec::as_slice))
}

/// Serial scatter reference, copied from `Mesh::recompute_normals`.
fn normals_reference(positions: &[Vec3], faces: &[Vec<u32>]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for face in faces {
        let mut unit = Vec3::ZERO;
        for i in 0..face.len() {
            let a = positions[face[i] as usize];
            let b = positions[face[(i + 1) % face.len()] as usize];
            unit.x += (a.y - b.y) * (a.z + b.z);
            unit.y += (a.z - b.z) * (a.x + b.x);
            unit.z += (a.x - b.x) * (a.y + b.y);
        }
        let normal = unit.normalize_or_zero();
        if normal.length_squared() < 1e-12 {
            continue;
        }
        let n = face.len();
        for i in 0..n {
            let prev = positions[face[(i + n - 1) % n] as usize];
            let current = positions[face[i] as usize];
            let next = positions[face[(i + 1) % n] as usize];
            let angle = (current - prev)
                .normalize_or_zero()
                .dot((next - current).normalize_or_zero())
                .clamp(-1.0, 1.0)
                .acos();
            normals[face[i] as usize] += normal * angle.max(1e-4);
        }
    }
    normals.into_iter().map(Vec3::normalize_or_zero).collect()
}

fn serial_smooth(
    positions: &[Vec3],
    neighbours: &Neighbours,
    iterations: u32,
    lambda: f32,
    mu: f32,
) -> Vec<Vec3> {
    let mut current = positions.to_vec();
    for _ in 0..iterations {
        for factor in [lambda, mu] {
            let previous = current.clone();
            for vertex in 0..previous.len() {
                let range = neighbours.range(vertex);
                if range.is_empty() {
                    continue;
                }
                let mut average = Vec3::ZERO;
                for &other in &neighbours.values()[range] {
                    average += previous[other as usize];
                }
                average /= (neighbours.range(vertex).len()) as f32;
                current[vertex] = previous[vertex] + (average - previous[vertex]) * factor;
            }
        }
    }
    current
}

#[test]
fn normals_match_the_serial_reference_exactly() {
    let (positions, faces) = grid(24, 18, 1.0);
    let graph = graph_of(&faces);
    let corners = VertexCorners::build(positions.len(), &graph);
    let expected = normals_reference(&positions, &faces);
    let _guard = pool_lock();
    for threads in [1usize, 2, 7, 0] {
        let _ = vf_core::compute::configure_threads(threads);
        let actual = normals_cpu(&positions, &graph, &corners);
        assert_eq!(actual.len(), expected.len());
        for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                a.to_array(),
                b.to_array(),
                "vertex {index} diverge avec {threads} threads"
            );
        }
    }
    let _ = vf_core::compute::configure_threads(0);
}

#[test]
fn smoothing_matches_the_serial_reference() {
    let (positions, faces) = grid(12, 9, 1.0);
    let graph = graph_of(&faces);
    let neighbours = Neighbours::build(positions.len(), &graph);
    let expected = serial_smooth(&positions, &neighbours, 3, 0.5, -0.53);
    let _guard = pool_lock();
    for threads in [1usize, 4, 0] {
        let _ = vf_core::compute::configure_threads(threads);
        let actual = smooth_cpu(&positions, &neighbours, 3, 0.5, -0.53);
        for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                a.to_array(),
                b.to_array(),
                "vertex {index}, {threads} threads"
            );
        }
    }
    let _ = vf_core::compute::configure_threads(0);
}

#[test]
fn displacement_moves_along_the_normal() {
    let positions = vec![Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)];
    let normals = vec![Vec3::Z, Vec3::X];
    let amounts = vec![2.0f32, -0.5];
    let outcome = displace(&positions, &normals, &amounts);
    assert_eq!(outcome.value[0], Vec3::new(0.0, 0.0, 2.0));
    assert_eq!(outcome.value[1], Vec3::new(0.5, 0.0, 0.0));
}

#[test]
fn transform_matches_glam() {
    let positions = vec![
        Vec3::new(1.0, 2.0, 3.0),
        Vec3::new(-4.0, 0.5, 8.0),
        Vec3::new(0.0, 0.0, 0.0),
    ];
    let matrix = Mat4::from_translation(Vec3::new(10.0, -2.0, 0.5))
        * Mat4::from_scale(Vec3::new(2.0, 0.5, 1.5));
    let outcome = transform(&positions, matrix);
    for (moved, original) in outcome.value.iter().zip(positions.iter()) {
        let expected = matrix.transform_point3(*original);
        assert_eq!(moved.to_array(), expected.to_array());
    }
}

#[test]
fn normals_of_a_flat_quad_point_up() {
    let positions = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    let faces = vec![vec![0u32, 1, 2, 3]];
    let graph = graph_of(&faces);
    let corners = VertexCorners::build(4, &graph);
    let values = normals_cpu(&positions, &graph, &corners);
    for normal in values {
        assert!((normal.z - 1.0).abs() < 1e-6, "{normal:?}");
    }
}

#[test]
fn empty_meshes_are_safe() {
    let graph = FaceGraph::default();
    let corners = VertexCorners::build(0, &graph);
    assert!(normals_cpu(&[], &graph, &corners).is_empty());
    assert!(area_normals(&[], &graph, &corners).value.is_empty());
    assert!(displace(&[], &[], &[]).value.is_empty());
    assert!(transform(&[], Mat4::IDENTITY).value.is_empty());
}

/// Serial reference for the area-weighted variant (`RenderMesh`).
fn area_normals_reference(positions: &[Vec3], faces: &[Vec<u32>]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for face in faces {
        if face.len() < 3 {
            continue;
        }
        let a = positions[face[0] as usize];
        let b = positions[face[1] as usize];
        let c = positions[face[2] as usize];
        let cross = (b - a).cross(c - a);
        for &index in face {
            normals[index as usize] += cross;
        }
    }
    normals.into_iter().map(Vec3::normalize_or_zero).collect()
}

#[test]
fn area_normals_match_the_serial_reference_exactly() {
    let (positions, faces) = grid(16, 12, 1.0);
    let graph = graph_of(&faces);
    let corners = VertexCorners::build(positions.len(), &graph);
    let expected = area_normals_reference(&positions, &faces);
    let _guard = pool_lock();
    for threads in [1usize, 5, 0] {
        let _ = vf_core::compute::configure_threads(threads);
        let actual = crate::kernels::area_normals_cpu(&positions, &graph, &corners);
        for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                a.to_array(),
                b.to_array(),
                "vertex {index}, {threads} threads"
            );
        }
    }
    let _ = vf_core::compute::configure_threads(0);
}

#[cfg(feature = "gpu")]
#[test]
fn gpu_normals_agree_with_the_cpu_within_a_tolerance() {
    use crate::backend::BackendState;

    let (positions, faces) = grid(20, 15, 1.0);
    let graph = graph_of(&faces);
    let corners = VertexCorners::build(positions.len(), &graph);
    let cpu = normals_cpu(&positions, &graph, &corners);
    let cpu_area = crate::kernels::area_normals_cpu(&positions, &graph, &corners);
    let state = BackendState::gpu_all(Vec::new());
    let gpu = match crate::gpu::normals(&state, &positions, &graph, &corners) {
        Ok(values) => values,
        Err(error) => {
            eprintln!("GPU indisponible, test ignoré : {error}");
            return;
        }
    };
    assert_eq!(gpu.len(), cpu.len());
    for (index, (a, b)) in gpu.iter().zip(cpu.iter()).enumerate() {
        assert!(
            (*a - *b).length() < 2e-3,
            "normale {index} : GPU {a:?} CPU {b:?}"
        );
    }
    let gpu_area = crate::gpu::area_normals(&state, &positions, &graph, &corners)
        .expect("normales de surface GPU");
    for (index, (a, b)) in gpu_area.iter().zip(cpu_area.iter()).enumerate() {
        assert!(
            (*a - *b).length() < 2e-3,
            "normale de surface {index} : GPU {a:?} CPU {b:?}"
        );
    }
}

/// The live selection is what the kernels follow: the preferences panel and the
/// CLI both rely on this, and a request the device cannot satisfy must fall
/// back to the CPU with an identical result.
#[test]
fn the_backend_choice_routes_the_kernels() {
    use crate::backend::BackendState;
    use crate::kernels::BackendUsed;

    let _guard = pool_lock();
    let previous = crate::backend_state();
    // Above the GPU threshold, so the GPU branch is really attempted.
    let (positions, faces) = grid(48, 48, 1.0);
    let graph = graph_of(&faces);
    let corners = VertexCorners::build(positions.len(), &graph);

    crate::set_backend(BackendState::cpu());
    let cpu = crate::normals(&positions, &graph, &corners);
    assert_eq!(cpu.backend, BackendUsed::Cpu);
    assert_eq!(cpu.value.len(), positions.len());

    crate::set_backend(BackendState::gpu("adaptateur-qui-n-existe-pas"));
    let fallback = crate::normals(&positions, &graph, &corners);
    assert_eq!(fallback.backend, BackendUsed::Cpu);
    for (index, (a, b)) in fallback.value.iter().zip(cpu.value.iter()).enumerate() {
        assert_eq!(a.to_array(), b.to_array(), "vertex {index}");
    }

    crate::set_backend(previous);
}

//! Polygonal mesh: positions, normals, UVs, vertex colours and n-gon faces.
//!
//! Faces are stored as ranges in a single corner array, so triangles, quads
//! and the n-gons produced by clipping or capping share one representation.
//! The renderer and the GLB writer triangulate on the fly; the OBJ writer and
//! the quad remesher keep the polygons.
//!
//! All edits go through methods that preserve the invariants: every corner is
//! a valid vertex index, `starts` is increasing and ends at `corners.len()`,
//! and the per-vertex attribute arrays have the same length.

use std::collections::HashMap;

use glam::{Mat4, Vec2, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use vf_compute::{FaceGraph, VertexCorners, VertexFaces};

use crate::math::{Aabb, closest_point_triangle};

/// Corners per rayon task for the render-mesh smoothing pass.
const SMOOTH_CHUNK: usize = 4096;

/// A face corner index list starts at `starts[face]` and ends at
/// `starts[face + 1]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    /// Straight (non-premultiplied) vertex colour. Used for rock tint, lava
    /// glow, charring and painted masks. Empty means "no per-vertex colour".
    pub colors: Vec<[u8; 4]>,
    pub corners: Vec<u32>,
    pub starts: Vec<u32>,
    /// Optional material slot per face. Empty means slot 0 everywhere.
    pub face_materials: Vec<u16>,
}

impl Mesh {
    pub fn new() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            colors: Vec::new(),
            corners: Vec::new(),
            starts: vec![0],
            face_materials: Vec::new(),
        }
    }

    pub fn from_triangles(positions: Vec<Vec3>, indices: Vec<[u32; 3]>) -> Self {
        let mut mesh = Self::new();
        mesh.positions = positions;
        mesh.corners = indices.iter().flat_map(|t| t.iter().copied()).collect();
        mesh.starts = (0..=indices.len() as u32).map(|i| i * 3).collect();
        mesh.finish_attributes();
        mesh
    }

    pub fn from_quads(positions: Vec<Vec3>, quads: Vec<[u32; 4]>) -> Self {
        let mut mesh = Self::new();
        mesh.positions = positions;
        mesh.corners = quads.iter().flat_map(|q| q.iter().copied()).collect();
        mesh.starts = (0..=quads.len() as u32).map(|i| i * 4).collect();
        mesh.finish_attributes();
        mesh
    }

    /// Convert the render/export mesh produced by `vf-mesh` (heightfield
    /// builder) into an editable mesh. Quad polygons are kept as quads.
    pub fn from_vista(data: &vf_mesh::MeshData) -> Self {
        let positions: Vec<Vec3> = data
            .positions
            .iter()
            .map(|p| Vec3::from_array(*p))
            .collect();
        let normals: Vec<Vec3> = data.normals.iter().map(|n| Vec3::from_array(*n)).collect();
        let uvs: Vec<Vec2> = data.uvs.iter().map(|uv| Vec2::from_array(*uv)).collect();
        let faces: Vec<Vec<u32>> = if data.polygons.is_empty() {
            data.indices.iter().map(|t| t.to_vec()).collect()
        } else {
            data.polygons.clone()
        };
        let mut mesh = Self::from_faces(positions, &faces);
        if normals.len() == mesh.positions.len() {
            mesh.normals = normals;
        }
        if uvs.len() == mesh.positions.len() {
            mesh.uvs = uvs;
        }
        mesh.recompute_normals_if_needed();
        mesh
    }

    pub fn from_faces(positions: Vec<Vec3>, faces: &[Vec<u32>]) -> Self {
        let mut mesh = Self::new();
        mesh.positions = positions;
        for face in faces {
            if face.len() >= 3 {
                mesh.push_face(face);
            }
        }
        mesh.finish_attributes();
        mesh
    }

    fn finish_attributes(&mut self) {
        let count = self.positions.len();
        if self.normals.len() < count {
            self.normals.resize(count, Vec3::ZERO);
        }
        if self.uvs.len() < count {
            self.uvs.resize(count, Vec2::ZERO);
        }
        if self.colors.len() == count && self.colors.iter().all(|c| c[3] == 0) {
            self.colors.clear();
        }
        self.recompute_normals_if_needed();
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn face_count(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    pub fn triangle_count(&self) -> usize {
        self.faces().map(|f| f.len().saturating_sub(2)).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty() || self.face_count() == 0
    }

    pub fn face(&self, index: usize) -> &[u32] {
        let start = self.starts[index] as usize;
        let end = self.starts[index + 1] as usize;
        &self.corners[start..end]
    }

    pub fn faces(&self) -> impl Iterator<Item = &[u32]> + '_ {
        (0..self.face_count()).map(move |i| self.face(i))
    }

    pub fn push_vertex(&mut self, position: Vec3) -> u32 {
        self.positions.push(position);
        self.normals.push(Vec3::ZERO);
        self.uvs.push(Vec2::ZERO);
        (self.positions.len() - 1) as u32
    }

    pub fn push_face(&mut self, face: &[u32]) -> usize {
        if face.len() < 3 {
            return usize::MAX;
        }
        self.corners.extend_from_slice(face);
        self.starts.push(self.corners.len() as u32);
        self.face_count() - 1
    }

    pub fn set_colors(&mut self, color: [u8; 4]) {
        self.colors = vec![color; self.positions.len()];
    }

    pub fn ensure_colors(&mut self) -> &mut Vec<[u8; 4]> {
        if self.colors.len() != self.positions.len() {
            self.colors = vec![[150, 145, 138, 255]; self.positions.len()];
        }
        &mut self.colors
    }

    /// Triangulated faces (fan). Used by the BVH, the voxeliser and glTF.
    pub fn triangles(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        let mut face = 0usize;
        let mut k = 1usize;
        std::iter::from_fn(move || {
            while face < self.face_count() {
                let corners = self.face(face);
                if k + 1 >= corners.len() {
                    face += 1;
                    k = 1;
                    continue;
                }
                let out = [corners[0], corners[k], corners[k + 1]];
                k += 1;
                return Some(out);
            }
            None
        })
    }

    pub fn triangulated_indices(&self) -> Vec<[u32; 3]> {
        self.triangles().collect()
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(&self.positions)
    }

    pub fn face_normal(&self, face: usize) -> Vec3 {
        let corners = self.face(face);
        // Newell's method: robust for non-planar quads.
        let mut normal = Vec3::ZERO;
        for i in 0..corners.len() {
            let a = self.positions[corners[i] as usize];
            let b = self.positions[corners[(i + 1) % corners.len()] as usize];
            normal.x += (a.y - b.y) * (a.z + b.z);
            normal.y += (a.z - b.z) * (a.x + b.x);
            normal.z += (a.x - b.x) * (a.y + b.y);
        }
        normal.normalize_or_zero()
    }

    pub fn face_area(&self, face: usize) -> f32 {
        let corners = self.face(face);
        let origin = self.positions[corners[0] as usize];
        let mut area = 0.0;
        for i in 1..corners.len() - 1 {
            let b = self.positions[corners[i] as usize];
            let c = self.positions[corners[i + 1] as usize];
            area += (b - origin).cross(c - origin).length() * 0.5;
        }
        area
    }

    /// Longest edge of a triangle of the BVH fan triangulation.
    pub fn triangle_max_edge(&self, triangle: [u32; 3]) -> f32 {
        let a = self.positions[triangle[0] as usize];
        let b = self.positions[triangle[1] as usize];
        let c = self.positions[triangle[2] as usize];
        a.distance(b).max(b.distance(c)).max(c.distance(a))
    }

    /// Average of the three smooth vertex normals of a triangle. This is the
    /// direction the brush should use and the one drawn in the cursor.
    pub fn triangle_smooth_normal(&self, triangle: [u32; 3]) -> Vec3 {
        let mut sum = Vec3::ZERO;
        for &vertex in &triangle {
            sum += self
                .normals
                .get(vertex as usize)
                .copied()
                .unwrap_or(Vec3::ZERO);
        }
        if sum.length_squared() < 1e-12 {
            let a = self.positions[triangle[0] as usize];
            let b = self.positions[triangle[1] as usize];
            let c = self.positions[triangle[2] as usize];
            return (b - a).cross(c - a).normalize_or_zero();
        }
        sum.normalize_or_zero()
    }

    pub fn face_centroid(&self, face: usize) -> Vec3 {
        let corners = self.face(face);
        let mut sum = Vec3::ZERO;
        for &c in corners {
            sum += self.positions[c as usize];
        }
        sum / corners.len() as f32
    }

    pub fn face_material(&self, face: usize) -> u16 {
        self.face_materials.get(face).copied().unwrap_or(0)
    }

    /// Compact adjacency of the current topology, used by every parallel
    /// geometry kernel. Cheap to build (one pass over the corners) compared
    /// with the work the kernels do with it.
    pub fn face_graph(&self) -> FaceGraph {
        FaceGraph::from_faces(self.faces())
    }

    /// Smooth, area-and-angle weighted vertex normals. Degenerate faces are
    /// skipped, so a zero-area triangle from a boolean cannot poison a normal.
    ///
    /// Runs on the compute backend selected in the preferences: the CPU path is
    /// bit-identical to the former serial loop, the GPU path runs the same
    /// gather on the selected adapter(s).
    pub fn recompute_normals(&mut self) {
        if self.positions.is_empty() {
            self.normals.clear();
            return;
        }
        let graph = self.face_graph();
        let corners = VertexCorners::build(self.positions.len(), &graph);
        self.normals = vf_compute::normals(&self.positions, &graph, &corners).value;
    }

    pub fn recompute_normals_if_needed(&mut self) {
        let bad = self.normals.len() != self.positions.len()
            || self
                .normals
                .iter()
                .any(|n| n.length_squared() < 1e-8 || !n.is_finite());
        if bad {
            self.normals.resize(self.positions.len(), Vec3::ZERO);
            self.recompute_normals();
        }
    }

    /// Box-projected UVs chosen per vertex from the dominant normal axis.
    /// Seams are acceptable for a sculpt preview; Blender's Smart UV Project
    /// can rebake them after import.
    pub fn box_project_uvs(&mut self, scale: f32) {
        let scale = if scale.abs() < 1e-6 { 1.0 } else { scale };
        self.uvs = self
            .positions
            .iter()
            .zip(self.normals.iter())
            .map(|(p, n)| {
                let a = n.abs();
                if a.x >= a.y && a.x >= a.z {
                    Vec2::new(p.z, p.y) * scale
                } else if a.y >= a.z {
                    Vec2::new(p.x, p.z) * scale
                } else {
                    Vec2::new(p.x, p.y) * scale
                }
            })
            .collect();
    }

    pub fn transform(&mut self, matrix: Mat4) {
        if !self.positions.is_empty() {
            self.positions = vf_compute::transform(&self.positions, matrix).value;
        }
        if matrix.determinant() < 0.0 {
            self.flip_faces();
        } else {
            let rotation = Mat4::from_mat3(glam::Mat3::from_mat4(matrix));
            vf_core::compute::install(|| {
                self.normals.par_iter_mut().for_each(|n| {
                    *n = (rotation * n.extend(0.0)).truncate().normalize_or_zero();
                });
            });
        }
    }

    pub fn translate(&mut self, offset: Vec3) {
        vf_core::compute::install(|| {
            self.positions.par_iter_mut().for_each(|p| *p += offset);
        });
    }

    pub fn flip_faces(&mut self) {
        for face in 0..self.face_count() {
            let start = self.starts[face] as usize;
            let end = self.starts[face + 1] as usize;
            self.corners[start..end].reverse();
        }
        for n in &mut self.normals {
            *n = -*n;
        }
    }

    pub fn merge(&mut self, other: &Mesh) {
        let previous_vertices = self.positions.len();
        let previous_faces = self.face_count();
        let has_colors = !self.colors.is_empty() || !other.colors.is_empty();
        if has_colors {
            if self.colors.len() != previous_vertices {
                self.colors.resize(previous_vertices, [150, 145, 138, 255]);
            }
            self.colors.extend(
                (0..other.positions.len())
                    .map(|i| other.colors.get(i).copied().unwrap_or([150, 145, 138, 255])),
            );
        }
        let offset = previous_vertices as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        for face in other.faces() {
            let mapped: Vec<u32> = face.iter().map(|i| i + offset).collect();
            self.push_face(&mapped);
        }
        let new_faces = self.face_count();
        self.face_materials.resize(previous_faces, 0);
        for face in 0..other.face_count() {
            let material = other.face_materials.get(face).copied().unwrap_or(0);
            self.face_materials.push(material);
        }
        self.face_materials.resize(new_faces, 0);
    }

    pub fn merged(parts: &[Mesh]) -> Mesh {
        let mut out = Mesh::new();
        for part in parts {
            out.merge(part);
        }
        out
    }

    /// Weld vertices closer than `epsilon` metres. Returns the number of
    /// vertices removed. Attributes come from the first vertex of each group.
    pub fn weld(&mut self, epsilon: f32) -> usize {
        let epsilon = epsilon.max(1e-7);
        let inv = 1.0 / epsilon;
        let mut map: HashMap<(i64, i64, i64), u32> = HashMap::with_capacity(self.positions.len());
        let mut remap = vec![0u32; self.positions.len()];
        let mut positions = Vec::with_capacity(self.positions.len());
        let mut normals = Vec::with_capacity(self.positions.len());
        let mut uvs = Vec::with_capacity(self.positions.len());
        let mut colors = Vec::with_capacity(self.colors.len());
        let has_colors = self.colors.len() == self.positions.len();
        for (index, p) in self.positions.iter().enumerate() {
            let key = (
                (p.x * inv).round() as i64,
                (p.y * inv).round() as i64,
                (p.z * inv).round() as i64,
            );
            let target = *map.entry(key).or_insert_with(|| {
                let id = positions.len() as u32;
                positions.push(*p);
                normals.push(self.normals.get(index).copied().unwrap_or(Vec3::ZERO));
                uvs.push(self.uvs.get(index).copied().unwrap_or(Vec2::ZERO));
                if has_colors {
                    colors.push(self.colors[index]);
                }
                id
            });
            remap[index] = target;
        }
        let removed = self.positions.len() - positions.len();
        if removed == 0 {
            return 0;
        }
        let mut corners = Vec::with_capacity(self.corners.len());
        let mut starts = vec![0u32];
        let mut materials = Vec::new();
        for face in 0..self.face_count() {
            let old = self.face(face);
            let mut mapped = Vec::with_capacity(old.len());
            for &c in old {
                let v = remap[c as usize];
                if mapped.last() != Some(&v) {
                    mapped.push(v);
                }
            }
            if mapped.len() >= 3 && mapped[0] != *mapped.last().unwrap() {
                corners.extend_from_slice(&mapped);
                starts.push(corners.len() as u32);
                materials.push(self.face_material(face));
            }
        }
        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
        self.colors = colors;
        self.corners = corners;
        self.starts = starts;
        self.face_materials = materials;
        removed
    }

    /// Remove faces with repeated corners, zero area or non-finite positions.
    pub fn remove_degenerate_faces(&mut self, min_area: f32) -> usize {
        let before = self.face_count();
        let mut corners = Vec::with_capacity(self.corners.len());
        let mut starts = vec![0u32];
        let mut materials = Vec::new();
        for face in 0..self.face_count() {
            let old = self.face(face);
            let mut unique = true;
            for i in 0..old.len() {
                for j in i + 1..old.len() {
                    if old[i] == old[j] {
                        unique = false;
                    }
                }
            }
            if !unique || self.face_area(face) <= min_area {
                continue;
            }
            if !old.iter().all(|i| {
                let p = self.positions[*i as usize];
                p.is_finite()
            }) {
                continue;
            }
            corners.extend_from_slice(old);
            starts.push(corners.len() as u32);
            materials.push(self.face_material(face));
        }
        self.corners = corners;
        self.starts = starts;
        if !materials.is_empty() {
            self.face_materials = materials;
        }
        before - self.face_count()
    }

    /// Drop vertices that no face references, keeping attributes aligned.
    pub fn compact(&mut self) -> usize {
        let mut used = vec![false; self.positions.len()];
        for &c in &self.corners {
            if let Some(flag) = used.get_mut(c as usize) {
                *flag = true;
            }
        }
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut colors = Vec::new();
        let has_colors = self.colors.len() == self.positions.len();
        for (index, flag) in used.iter().enumerate() {
            if *flag {
                remap[index] = positions.len() as u32;
                positions.push(self.positions[index]);
                normals.push(self.normals.get(index).copied().unwrap_or(Vec3::ZERO));
                uvs.push(self.uvs.get(index).copied().unwrap_or(Vec2::ZERO));
                if has_colors {
                    colors.push(self.colors[index]);
                }
            }
        }
        let removed = self.positions.len() - positions.len();
        for c in &mut self.corners {
            *c = remap[*c as usize];
        }
        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
        self.colors = colors;
        removed
    }

    pub fn is_index_valid(&self) -> bool {
        self.corners
            .iter()
            .all(|i| (*i as usize) < self.positions.len())
            && self.starts.len() >= 2
            && self.starts.first() == Some(&0)
            && self.starts.last().map(|v| *v as usize) == Some(self.corners.len())
            && self
                .starts
                .windows(2)
                .all(|w| w[0] <= w[1] && (w[1] - w[0]) as usize >= 3)
    }

    /// Undirected edges with their incident face count.
    fn edge_counts(&self) -> HashMap<(u32, u32), u32> {
        let mut map: HashMap<(u32, u32), u32> = HashMap::new();
        for face in self.faces() {
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                *map.entry(key).or_default() += 1;
            }
        }
        map
    }

    /// Boundary loops as ordered vertex lists, from directed edges used once.
    pub fn boundary_loops(&self) -> Vec<Vec<u32>> {
        let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
        for face in self.faces() {
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                *directed.entry(key).or_default() += 1;
            }
        }
        let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
        for face in self.faces() {
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                if directed.get(&key).copied().unwrap_or(0) == 1 {
                    next.entry(a).or_default().push(b);
                }
            }
        }
        let mut visited: HashMap<(u32, u32), bool> = HashMap::new();
        let mut loops = Vec::new();
        // Walk the boundary graph in vertex order, not in `HashMap` order: the
        // loops are compared and merged by callers, so their order matters.
        let mut starts: Vec<u32> = next.keys().copied().collect();
        starts.sort_unstable();
        for start in starts {
            let ends = next[&start].clone();
            for end in ends {
                if visited.contains_key(&(start, end)) {
                    continue;
                }
                let mut loop_vertices = vec![start];
                let mut a = start;
                let mut b = end;
                loop {
                    visited.insert((a, b), true);
                    loop_vertices.push(b);
                    if b == start {
                        loop_vertices.pop();
                        break;
                    }
                    let Some(candidates) = next.get(&b) else {
                        break;
                    };
                    let Some(&c) = candidates.iter().find(|&&c| !visited.contains_key(&(b, c)))
                    else {
                        break;
                    };
                    a = b;
                    b = c;
                }
                if loop_vertices.len() >= 3 {
                    loops.push(loop_vertices);
                }
            }
        }
        loops
    }

    /// Total surface area, square metres.
    pub fn surface_area(&self) -> f64 {
        (0..self.face_count())
            .map(|f| f64::from(self.face_area(f)))
            .sum()
    }

    /// Signed volume via the divergence theorem. Positive for outward-facing
    /// closed meshes. Meaningless for open surfaces.
    pub fn volume(&self) -> f64 {
        let mut volume = 0.0;
        for t in self.triangles() {
            let a = self.positions[t[0] as usize];
            let b = self.positions[t[1] as usize];
            let c = self.positions[t[2] as usize];
            volume += f64::from(a.dot(b.cross(c))) / 6.0;
        }
        volume
    }

    /// Close an open terrain sheet into a printable solid: side walls drop to
    /// `floor_z` and a planar cap closes each boundary loop. Returns `true`
    /// when the mesh was already closed.
    pub fn close_slab(&mut self, floor_z: f32) -> bool {
        let loops = self.boundary_loops();
        if loops.is_empty() {
            return true;
        }
        let mut added = Vec::new();
        for loop_vertices in &loops {
            let lower: Vec<u32> = loop_vertices
                .iter()
                .map(|&v| {
                    let p = self.positions[v as usize];
                    self.push_vertex(Vec3::new(p.x, p.y, floor_z))
                })
                .collect();
            for i in 0..loop_vertices.len() {
                let j = (i + 1) % loop_vertices.len();
                let a = loop_vertices[i];
                let b = loop_vertices[j];
                let c = lower[j];
                let d = lower[i];
                self.push_face(&[a, d, c, b]);
            }
            let center = loop_vertices
                .iter()
                .fold(Vec3::ZERO, |acc, &v| acc + self.positions[v as usize])
                / loop_vertices.len() as f32;
            let center_index = self.push_vertex(Vec3::new(center.x, center.y, floor_z));
            for i in 0..lower.len() {
                let j = (i + 1) % lower.len();
                self.push_face(&[lower[i], center_index, lower[j]]);
            }
            added.extend(lower);
        }
        self.recompute_normals();
        if self.volume() < 0.0 {
            self.flip_faces();
        }
        !added.is_empty()
    }

    /// Close every boundary loop with a triangle fan around its centroid.
    /// The cap follows the loop direction, so each boundary edge is used once
    /// in each direction and the result becomes closed (not necessarily
    /// manifold if the hole is pinched).
    pub fn cap_boundary_loops(&mut self) -> usize {
        let loops = self.boundary_loops();
        let mut added = 0;
        for loop_vertices in loops {
            if loop_vertices.len() < 3 {
                continue;
            }
            let center = loop_vertices
                .iter()
                .fold(Vec3::ZERO, |acc, &v| acc + self.positions[v as usize])
                / loop_vertices.len() as f32;
            let center_index = self.push_vertex(center);
            for i in 0..loop_vertices.len() {
                let a = loop_vertices[i];
                let b = loop_vertices[(i + 1) % loop_vertices.len()];
                // The surrounding faces use a -> b, so the cap uses b -> a.
                self.push_face(&[a, center_index, b]);
                added += 1;
            }
        }
        if added > 0 {
            self.recompute_normals();
        }
        added
    }

    /// Close every boundary edge with a triangle fan around the centre of its
    /// connected boundary component.
    ///
    /// Unlike [`Self::cap_boundary_loops`] this does not need the edges to form
    /// a clean oriented loop: it also closes the small non-manifold holes that
    /// surface nets leaves where two sheets meet at a single vertex.
    pub fn cap_boundary_edges(&mut self) -> usize {
        let counts = self.edge_counts();
        let mut directed: Vec<(u32, u32)> = Vec::new();
        for face in self.faces() {
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                if counts.get(&key).copied().unwrap_or(0) == 1 && a != b {
                    directed.push((a, b));
                }
            }
        }
        if directed.is_empty() {
            return 0;
        }
        // Union-find over the vertices touched by boundary edges.
        let max_vertex = directed
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .max()
            .unwrap_or(0) as usize
            + 1;
        let mut parent: Vec<usize> = (0..max_vertex).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for &(a, b) in &directed {
            let ra = find(&mut parent, a as usize);
            let rb = find(&mut parent, b as usize);
            if ra != rb {
                parent[ra] = rb;
            }
        }
        // One centre per component.
        let mut sums: HashMap<usize, (Vec3, f32)> = HashMap::new();
        for &(a, b) in &directed {
            for vertex in [a, b] {
                let root = find(&mut parent, vertex as usize);
                let entry = sums.entry(root).or_insert((Vec3::ZERO, 0.0));
                entry.0 += self.positions[vertex as usize];
                entry.1 += 1.0;
            }
        }
        let mut centers: HashMap<usize, u32> = HashMap::new();
        // `sums` is a map: push the centre vertices in root order so the vertex
        // numbering (and therefore every later index tie-break) is reproducible.
        let mut roots: Vec<usize> = sums.keys().copied().collect();
        roots.sort_unstable();
        for root in roots {
            let (sum, count) = sums[&root];
            let center = sum / count.max(1.0);
            centers.insert(root, self.push_vertex(center));
        }
        let mut added = 0;
        for &(a, b) in &directed {
            let root = find(&mut parent, a as usize);
            let Some(&center) = centers.get(&root) else {
                continue;
            };
            // The surrounding face uses a -> b; the cap uses b -> a.
            self.push_face(&[b, center, a]);
            added += 1;
        }
        if added > 0 {
            self.recompute_normals();
        }
        added
    }

    /// Resolve corners repeated inside one face. A dual-contouring cell that
    /// is crossed by two sheets of the same surface produces a quad such as
    /// `[a, b, a, c]`: dropping it makes a hole, keeping it makes a degenerate
    /// face. Removing the repeated corner turns it into a triangle and keeps
    /// the surface closed.
    pub fn collapse_repeated_corners(&mut self) -> usize {
        let before = self.face_count();
        let mut corners = Vec::with_capacity(self.corners.len());
        let mut starts = vec![0u32];
        let mut materials = Vec::new();
        for face in 0..self.face_count() {
            let old = self.face(face);
            let mut unique: Vec<u32> = Vec::with_capacity(old.len());
            for &c in old {
                if !unique.contains(&c) {
                    unique.push(c);
                }
            }
            if unique.len() >= 3 {
                corners.extend_from_slice(&unique);
                starts.push(corners.len() as u32);
                materials.push(self.face_material(face));
            }
        }
        self.corners = corners;
        self.starts = starts;
        self.face_materials = materials;
        before.saturating_sub(self.face_count())
    }

    /// Turn a surface into a shell of the given thickness (for 3D printing).
    /// The original faces are kept, the inner copy is flipped and the boundary
    /// is stitched. Only meaningful for open surfaces with a boundary.
    pub fn solidify(&mut self, thickness: f32) -> bool {
        let loops = self.boundary_loops();
        if loops.is_empty() {
            return false;
        }
        let original_faces: Vec<Vec<u32>> = self.faces().map(|f| f.to_vec()).collect();
        let original_count = self.positions.len();
        let mut inner = Vec::with_capacity(original_count);
        for i in 0..original_count {
            let p = self.positions[i];
            let n = self.normals.get(i).copied().unwrap_or(Vec3::Z);
            inner.push(self.push_vertex(p - n * thickness));
        }
        for face in &original_faces {
            let mapped: Vec<u32> = face.iter().rev().map(|&v| inner[v as usize]).collect();
            self.push_face(&mapped);
        }
        for loop_vertices in &loops {
            for i in 0..loop_vertices.len() {
                let j = (i + 1) % loop_vertices.len();
                let a = loop_vertices[i];
                let b = loop_vertices[j];
                self.push_face(&[a, b, inner[b as usize], inner[a as usize]]);
            }
        }
        self.recompute_normals();
        if self.volume() < 0.0 {
            self.flip_faces();
        }
        true
    }

    /// Push a vertex interpolated between two existing vertices, carrying UV
    /// and colour linearly. Used by both subdivision paths.
    pub(crate) fn push_midpoint(&mut self, a: u32, b: u32) -> u32 {
        let pa = self.positions[a as usize];
        let pb = self.positions[b as usize];
        let id = self.push_vertex((pa + pb) * 0.5);
        if self.uvs.len() == self.positions.len() {
            let ua = self.uvs[a as usize];
            let ub = self.uvs[b as usize];
            self.uvs[id as usize] = (ua + ub) * 0.5;
        }
        if self.colors.len() == self.positions.len() {
            let ca = self.colors[a as usize];
            let cb = self.colors[b as usize];
            self.colors[id as usize] = [
                ((ca[0] as u16 + cb[0] as u16) / 2) as u8,
                ((ca[1] as u16 + cb[1] as u16) / 2) as u8,
                ((ca[2] as u16 + cb[2] as u16) / 2) as u8,
                ((ca[3] as u16 + cb[3] as u16) / 2) as u8,
            ];
        }
        id
    }

    pub(crate) fn push_centroid(&mut self, corners: &[u32]) -> u32 {
        let mut p = Vec3::ZERO;
        let mut uv = Vec2::ZERO;
        let mut color = [0u32; 4];
        for &c in corners {
            p += self.positions[c as usize];
            uv += self.uvs[c as usize];
            let c_color = self
                .colors
                .get(c as usize)
                .copied()
                .unwrap_or([150, 145, 138, 255]);
            for i in 0..4 {
                color[i] += u32::from(c_color[i]);
            }
        }
        let n = corners.len().max(1) as f32;
        let id = self.push_vertex(p / n);
        if self.uvs.len() == self.positions.len() {
            self.uvs[id as usize] = uv / n;
        }
        if self.colors.len() == self.positions.len() {
            self.colors[id as usize] = [
                (color[0] / corners.len().max(1) as u32).min(255) as u8,
                (color[1] / corners.len().max(1) as u32).min(255) as u8,
                (color[2] / corners.len().max(1) as u32).min(255) as u8,
                (color[3] / corners.len().max(1) as u32).min(255) as u8,
            ];
        }
        id
    }

    /// Linear midpoint subdivision of the whole mesh: each n-gon becomes one
    /// quad per original corner around a face centre. Quads stay quads, and
    /// every shared edge gets exactly one midpoint, so the result is
    /// watertight. This is the cheap densification used before a crack or a
    /// detail pass.
    pub fn subdivide_linear(&mut self, levels: u32) -> usize {
        let mut added = 0;
        for _ in 0..levels.min(4) {
            let face_count = self.face_count();
            let before = self.positions.len();
            let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
            let mut new_faces: Vec<Vec<u32>> = Vec::with_capacity(face_count * 4);
            for face in 0..face_count {
                let corners = self.face(face).to_vec();
                let n = corners.len();
                let mut mids = Vec::with_capacity(n);
                for i in 0..n {
                    let a = corners[i];
                    let b = corners[(i + 1) % n];
                    let key = if a < b { (a, b) } else { (b, a) };
                    let id = match midpoints.get(&key) {
                        Some(id) => *id,
                        None => {
                            let id = self.push_midpoint(a, b);
                            midpoints.insert(key, id);
                            id
                        }
                    };
                    mids.push(id);
                }
                let center = self.push_centroid(&corners);
                for i in 0..n {
                    let prev_mid = mids[(i + n - 1) % n];
                    new_faces.push(vec![corners[i], mids[i], center, prev_mid]);
                }
            }
            self.corners.clear();
            self.starts = vec![0];
            for face in &new_faces {
                self.push_face(face);
            }
            self.face_materials = vec![0; self.face_count()];
            self.recompute_normals();
            added += self.positions.len() - before;
            if self.positions.len() > 8_000_000 {
                break;
            }
        }
        added
    }

    /// Split only the given faces plus the neighbours that share a split edge.
    /// Midpoints are global, so the dense area stays watertight against the
    /// coarse one. The children of the selected faces are selected again for
    /// the next level; the neighbours are refined once, only to absorb the
    /// T-junction.
    pub fn subdivide_region(&mut self, region: &[u32], levels: u32) -> usize {
        let mut selected: std::collections::HashSet<u32> = region.iter().copied().collect();
        let mut touched_total = 0;
        for _ in 0..levels.min(4) {
            let face_count = self.face_count();
            if selected.is_empty() {
                break;
            }
            let mut split_edges: std::collections::HashSet<(u32, u32)> =
                std::collections::HashSet::new();
            for &face in &selected {
                if (face as usize) >= face_count {
                    continue;
                }
                let corners = self.face(face as usize);
                for i in 0..corners.len() {
                    let a = corners[i];
                    let b = corners[(i + 1) % corners.len()];
                    split_edges.insert(if a < b { (a, b) } else { (b, a) });
                }
            }
            if split_edges.is_empty() {
                break;
            }
            let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
            // Sorted, so the new vertex ids are reproducible across runs.
            let mut split_keys: Vec<(u32, u32)> = split_edges.iter().copied().collect();
            split_keys.sort_unstable();
            for (a, b) in split_keys {
                let id = self.push_midpoint(a, b);
                mid.insert((a, b), id);
            }
            let mut parent_of_face: Vec<u32> = Vec::with_capacity(face_count * 2);
            let mut new_faces: Vec<Vec<u32>> = Vec::with_capacity(face_count * 2);
            let mut children_of_selected: std::collections::HashSet<u32> =
                std::collections::HashSet::new();
            for face in 0..face_count {
                let corners = self.face(face).to_vec();
                let n = corners.len();
                let marked: Vec<bool> = (0..n)
                    .map(|i| {
                        let a = corners[i];
                        let b = corners[(i + 1) % n];
                        split_edges.contains(&(if a < b { (a, b) } else { (b, a) }))
                    })
                    .collect();
                let count = marked.iter().filter(|m| **m).count();
                if count == 0 {
                    parent_of_face.push(face as u32);
                    new_faces.push(corners);
                    continue;
                }
                touched_total += 1;
                let is_selected = selected.contains(&(face as u32));
                let start = new_faces.len();
                if count == n {
                    // Fully refined: one quad per corner around the centre.
                    let center = self.push_centroid(&corners);
                    let mut mids = Vec::with_capacity(n);
                    for i in 0..n {
                        let a = corners[i];
                        let b = corners[(i + 1) % n];
                        mids.push(mid[&(if a < b { (a, b) } else { (b, a) })]);
                    }
                    for i in 0..n {
                        let prev = mids[(i + n - 1) % n];
                        new_faces.push(vec![corners[i], mids[i], center, prev]);
                    }
                } else {
                    // Partial refinement: fan around the centre over the
                    // boundary polygon with its midpoints inserted.
                    let center = self.push_centroid(&corners);
                    for i in 0..n {
                        let a = corners[i];
                        let b = corners[(i + 1) % n];
                        new_faces.push(vec![
                            center,
                            a,
                            if marked[i] {
                                mid[&(if a < b { (a, b) } else { (b, a) })]
                            } else {
                                b
                            },
                        ]);
                        if marked[i] {
                            let m = mid[&(if a < b { (a, b) } else { (b, a) })];
                            new_faces.push(vec![center, m, b]);
                        }
                    }
                }
                for index in start..new_faces.len() {
                    parent_of_face.push(face as u32);
                    if is_selected {
                        children_of_selected.insert(index as u32);
                    }
                }
            }
            self.corners.clear();
            self.starts = vec![0];
            for face in &new_faces {
                self.push_face(face);
            }
            self.face_materials = vec![0; self.face_count()];
            self.recompute_normals();
            selected = children_of_selected;
        }
        touched_total
    }

    /// Catmull–Clark subdivision for quad-dominant meshes. Boundary edges keep
    /// their midpoint and boundary vertices use the crease rule, so an open
    /// terrain sheet does not shrink at its rim.
    pub fn catmull_clark(&mut self) -> usize {
        let face_count = self.face_count();
        if face_count == 0 {
            return 0;
        }
        let mut face_points = Vec::with_capacity(face_count);
        let mut face_adjacency: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        let mut vertex_faces: Vec<Vec<usize>> = vec![Vec::new(); self.positions.len()];
        for face in 0..face_count {
            face_points.push(self.face_centroid(face));
            let corners = self.face(face);
            for i in 0..corners.len() {
                let a = corners[i];
                let b = corners[(i + 1) % corners.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                face_adjacency.entry(key).or_default().push(face);
                vertex_faces[a as usize].push(face);
            }
        }
        let mut edge_point = HashMap::new();
        for (&(a, b), faces) in &face_adjacency {
            let pa = self.positions[a as usize];
            let pb = self.positions[b as usize];
            let p = if faces.len() == 2 {
                let fp = (face_points[faces[0]] + face_points[faces[1]]) * 0.5;
                (pa + pb + fp * 2.0) * 0.25
            } else {
                (pa + pb) * 0.5
            };
            edge_point.insert((a, b), p);
        }
        let mut new_positions = Vec::with_capacity(self.positions.len() * 2 + face_count);
        let mut vertex_remap = vec![0u32; self.positions.len()];
        for (index, faces) in vertex_faces.iter().enumerate() {
            let p = self.positions[index];
            let unique: Vec<usize> = {
                let mut v = faces.clone();
                v.sort_unstable();
                v.dedup();
                v
            };
            let boundary_neighbors: Vec<u32> = edge_point
                .iter()
                .filter(|((a, b), _)| *a == index as u32 || *b == index as u32)
                .filter(|(key, _)| {
                    face_adjacency
                        .get(key)
                        .map(|f| f.len() == 1)
                        .unwrap_or(false)
                })
                .map(|((a, b), _)| if *a == index as u32 { *b } else { *a })
                .collect();
            let new_p = if boundary_neighbors.len() >= 2 {
                let n0 = self.positions[boundary_neighbors[0] as usize];
                let n1 = self.positions[boundary_neighbors[1] as usize];
                (n0 + n1 + p * 6.0) / 8.0
            } else if unique.is_empty() {
                p
            } else {
                let n = unique.len() as f32;
                let f = unique
                    .iter()
                    .fold(Vec3::ZERO, |acc, &face| acc + face_points[face])
                    / n;
                let mut r = Vec3::ZERO;
                let mut edges = 0u32;
                for &(a, b) in edge_point.keys() {
                    if a == index as u32 || b == index as u32 {
                        r += (self.positions[a as usize] + self.positions[b as usize]) * 0.5;
                        edges += 1;
                    }
                }
                r /= edges.max(1) as f32;
                (f + r * 2.0 + p * (n - 3.0)) / n
            };
            vertex_remap[index] = new_positions.len() as u32;
            new_positions.push(new_p);
        }
        let mut edge_remap = HashMap::new();
        for (&key, point) in &edge_point {
            edge_remap.insert(key, new_positions.len() as u32);
            new_positions.push(*point);
        }
        let mut face_remap = Vec::with_capacity(face_count);
        for point in &face_points {
            face_remap.push(new_positions.len() as u32);
            new_positions.push(*point);
        }
        let mut new_faces = Vec::new();
        let mut new_materials = Vec::new();
        for (face, &face_point) in face_remap.iter().enumerate().take(face_count) {
            let corners = self.face(face).to_vec();
            let n = corners.len();
            for i in 0..n {
                let a = corners[i];
                let b = corners[(i + 1) % n];
                let c = corners[(i + n - 1) % n];
                let prev_key = if a < c { (a, c) } else { (c, a) };
                let next_key = if a < b { (a, b) } else { (b, a) };
                new_faces.push(vec![
                    vertex_remap[a as usize],
                    edge_remap[&next_key],
                    face_point,
                    edge_remap[&prev_key],
                ]);
                new_materials.push(self.face_material(face));
            }
        }
        let old_colors = self.colors.clone();
        self.positions = new_positions;
        self.normals = vec![Vec3::ZERO; self.positions.len()];
        self.uvs = vec![Vec2::ZERO; self.positions.len()];
        self.colors = if old_colors.is_empty() {
            Vec::new()
        } else {
            vec![[150, 145, 138, 255]; self.positions.len()]
        };
        self.corners.clear();
        self.starts = vec![0];
        for face in &new_faces {
            self.push_face(face);
        }
        self.face_materials = new_materials;
        // Colours follow the smoothed original vertices; edge and face points
        // inherit the neutral rock tone (they are a small fraction of the new
        // vertices and shading dominates at this scale).
        for (old, &new) in vertex_remap.iter().enumerate() {
            if let Some(color) = old_colors.get(old) {
                self.colors[new as usize] = *color;
            }
        }
        self.box_project_uvs(1.0);
        self.recompute_normals();
        face_count
    }

    /// Per-face normals of the triangulation (for flat shading and export).
    pub fn flat_triangles(&self) -> Vec<([Vec3; 3], Vec3)> {
        self.triangles()
            .map(|t| {
                let p = [
                    self.positions[t[0] as usize],
                    self.positions[t[1] as usize],
                    self.positions[t[2] as usize],
                ];
                let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                (p, n)
            })
            .collect()
    }

    /// Closest point on the mesh surface. Used by brushes and by the detail
    /// transfer after a remesh.
    pub fn closest_point(&self, bvh: &crate::bvh::Bvh, p: Vec3) -> (f32, Vec3, Vec3) {
        match bvh.closest_point(self, p) {
            Some(hit) => {
                let (point, _) = closest_point_triangle(
                    p,
                    self.positions[hit.triangle[0] as usize],
                    self.positions[hit.triangle[1] as usize],
                    self.positions[hit.triangle[2] as usize],
                );
                (point.distance(p), point, hit.normal)
            }
            None => (f32::INFINITY, p, Vec3::Z),
        }
    }

    /// Approximate heap footprint, for the document and undo budgets.
    pub fn memory_bytes(&self) -> usize {
        self.positions.len() * std::mem::size_of::<Vec3>()
            + self.normals.len() * std::mem::size_of::<Vec3>()
            + self.uvs.len() * std::mem::size_of::<glam::Vec2>()
            + self.colors.len() * std::mem::size_of::<[u8; 4]>()
            + self.corners.len() * std::mem::size_of::<u32>()
            + self.starts.len() * std::mem::size_of::<u32>()
            + self.face_materials.len() * std::mem::size_of::<u16>()
    }

    /// Statistics used by the UI, the CLI and the validators.
    pub fn stats(&self) -> MeshStats {
        let mut quads = 0;
        let mut tris = 0;
        let mut ngons = 0;
        for face in self.faces() {
            match face.len() {
                3 => tris += 1,
                4 => quads += 1,
                _ => ngons += 1,
            }
        }
        let edges = self.edge_counts();
        let mut boundary_edges = 0;
        let mut non_manifold_edges = 0;
        for count in edges.values() {
            match *count {
                1 => boundary_edges += 1,
                2 => {}
                _ => non_manifold_edges += 1,
            }
        }
        let degenerate = (0..self.face_count())
            .filter(|&f| self.face_area(f) <= 1e-10)
            .count();
        let components = self.connected_components();
        MeshStats {
            vertices: self.positions.len(),
            faces: self.face_count(),
            triangles: self.triangle_count(),
            quads,
            tris,
            ngons,
            boundary_edges,
            non_manifold_edges,
            degenerate_faces: degenerate,
            components,
            // Watertight means "no hole": a closed surface can still touch
            // itself on a sharp CSG seam, which `is_manifold` reports
            // separately.
            watertight: boundary_edges == 0 && self.face_count() > 0,
            area_m2: self.surface_area(),
            volume_m3: self.volume(),
            bounds: self.bounds(),
        }
    }

    /// Number of edge-connected face groups.
    pub fn connected_components(&self) -> usize {
        let face_count = self.face_count();
        if face_count == 0 {
            return 0;
        }
        let mut parent: Vec<u32> = (0..face_count as u32).collect();
        fn find(parent: &mut [u32], mut x: u32) -> u32 {
            while parent[x as usize] != x {
                let grand = parent[parent[x as usize] as usize];
                parent[x as usize] = grand;
                x = grand;
            }
            x
        }
        let mut edge_owner: HashMap<(u32, u32), u32> = HashMap::new();
        for face in 0..face_count {
            let corners = self.face(face);
            for i in 0..corners.len() {
                let a = corners[i];
                let b = corners[(i + 1) % corners.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                if let Some(&other) = edge_owner.get(&key) {
                    let ra = find(&mut parent, face as u32);
                    let rb = find(&mut parent, other);
                    if ra != rb {
                        parent[ra as usize] = rb;
                    }
                } else {
                    edge_owner.insert(key, face as u32);
                }
            }
        }
        let mut roots = std::collections::HashSet::new();
        for face in 0..face_count {
            let root = find(&mut parent, face as u32);
            roots.insert(root);
        }
        roots.len()
    }

    /// Apply a per-vertex displacement along the vertex normal.
    pub fn displace_along_normals(&mut self, displacement: impl Fn(Vec3, Vec3) -> f32) {
        for index in 0..self.positions.len() {
            let p = self.positions[index];
            let n = self.normals.get(index).copied().unwrap_or(Vec3::Z);
            self.positions[index] = p + n * displacement(p, n);
        }
    }

    /// Re-apply the normals of a reference surface onto this mesh: for every
    /// vertex, find the closest triangle of `source`, and use its interpolated
    /// normal. Called after a remesh so shading keeps the original rock
    /// signature even when the topology changed.
    pub fn transfer_normals(&mut self, source: &Mesh, source_bvh: &crate::bvh::Bvh) {
        let mut normals = Vec::with_capacity(self.positions.len());
        for &p in &self.positions {
            match source_bvh.closest_point(source, p) {
                Some(hit) => {
                    let t = hit.barycentric;
                    let (a, b, c) = (
                        source.normals[hit.triangle[0] as usize],
                        source.normals[hit.triangle[1] as usize],
                        source.normals[hit.triangle[2] as usize],
                    );
                    let n = a * (1.0 - t.x - t.y) + b * t.x + c * t.y;
                    normals.push(n.normalize_or_zero());
                }
                None => normals.push(Vec3::ZERO),
            }
        }
        self.normals = normals;
    }
}

/// A compact, render/export-ready triangulation with corner-split normals.
/// Vertices are duplicated when the smoothing angle separates two faces, so
/// hard rock edges are preserved in Blender and in the viewport.
#[derive(Debug, Clone, Default)]
pub struct RenderMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<[u32; 3]>,
    pub lines: Vec<[u32; 2]>,
    pub bounds: Aabb,
}

impl RenderMesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
    pub fn from_mesh(mesh: &Mesh, smoothing_angle_deg: f32) -> Self {
        let face_count = mesh.face_count();
        let cos_threshold = smoothing_angle_deg.clamp(0.0, 180.0).to_radians().cos();
        // Compact adjacency instead of one `Vec` per vertex: on a million-vertex
        // cliff the per-vertex lists were millions of allocations per rebuild.
        let graph = mesh.face_graph();
        let vertex_faces = VertexFaces::build(mesh.vertices_len(), &graph);
        let corner_faces = graph.corner_faces();
        let mut face_normals = vec![Vec3::ZERO; face_count];
        // Per-corner smoothing normal, filled by one parallel pass. Each corner
        // reads its (vertex, face) incident faces in ascending order, which is
        // exactly the order the previous serial loop used.
        let mut corner_normals = vec![Vec3::ZERO; graph.corner_count()];
        vf_core::compute::install(|| {
            face_normals
                .par_iter_mut()
                .enumerate()
                .for_each(|(face, slot)| {
                    *slot = mesh.face_normal(face);
                });
            corner_normals
                .par_chunks_mut(SMOOTH_CHUNK)
                .enumerate()
                .for_each(|(block, chunk)| {
                    let base = block * SMOOTH_CHUNK;
                    for (offset, slot) in chunk.iter_mut().enumerate() {
                        let corner = base + offset;
                        if corner >= corner_faces.len() {
                            continue;
                        }
                        let vertex = graph.corners()[corner] as usize;
                        let face = corner_faces[corner] as usize;
                        let n = face_normals[face];
                        if n.length_squared() < 1e-12 {
                            *slot = Vec3::ZERO;
                            continue;
                        }
                        let mut sum = Vec3::ZERO;
                        for other in vertex_faces.range(vertex) {
                            let no = face_normals[vertex_faces.faces()[other] as usize];
                            if no.dot(n) >= cos_threshold {
                                sum += no;
                            }
                        }
                        *slot = sum.normalize_or_zero();
                    }
                });
        });
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut colors = Vec::new();
        let has_colors = mesh.colors.len() == mesh.positions.len();
        let mut lookup: HashMap<(u32, i32, i32, i32), u32> = HashMap::new();
        let mut indices = Vec::with_capacity(face_count * 3);
        let mut lines: Vec<[u32; 2]> = Vec::new();
        let mut line_seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for face in 0..face_count {
            let range = graph.face_range(face);
            let corners = graph.face(face);
            let mut local = Vec::with_capacity(corners.len());
            for (offset, &vertex) in corners.iter().enumerate() {
                let normal = corner_normals[range.start + offset];
                let key = (
                    vertex,
                    (normal.x * 512.0).round() as i32,
                    (normal.y * 512.0).round() as i32,
                    (normal.z * 512.0).round() as i32,
                );
                let id = *lookup.entry(key).or_insert_with(|| {
                    let index = positions.len() as u32;
                    positions.push(mesh.positions[vertex as usize]);
                    normals.push(normal);
                    uvs.push(mesh.uvs.get(vertex as usize).copied().unwrap_or(Vec2::ZERO));
                    if has_colors {
                        colors.push(mesh.colors[vertex as usize]);
                    }
                    index
                });
                local.push(id);
            }
            // Wireframe uses original vertices so the drawn edges match the
            // sculpted topology even when shading split a vertex.
            for i in 0..corners.len() {
                let a = corners[i];
                let b = corners[(i + 1) % corners.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                if line_seen.insert(key) {
                    lines.push([a, b]);
                }
            }
            for i in 1..local.len() - 1 {
                indices.push([local[0], local[i], local[i + 1]]);
            }
        }
        // Line indices refer to original vertex ids; the renderer projects
        // `mesh.positions` for lines, so remap them to split ids only when
        // they coincide. Keeping original ids is simpler and the renderer has
        // the mesh anyway.
        let bounds = Aabb::from_points(&positions);
        Self {
            positions,
            normals,
            uvs,
            colors,
            indices,
            lines,
            bounds,
        }
    }

    /// Fast path for interactive navigation and strokes: reuse the mesh
    /// normals and vertex ids instead of splitting corners by smoothing angle.
    /// It skips the expensive per-corner normal clustering, at the cost of
    /// smoothing hard edges while the camera moves. A full-quality rebuild is
    /// used as soon as the interaction stops.
    pub fn from_mesh_fast(mesh: &Mesh) -> Self {
        let count = mesh.positions.len();
        let mut normals = vec![Vec3::Z; count];
        let mut uvs = vec![Vec2::ZERO; count];
        vf_core::compute::install(|| {
            normals
                .par_iter_mut()
                .enumerate()
                .for_each(|(index, slot)| {
                    let normal = mesh.normals.get(index).copied().unwrap_or(Vec3::Z);
                    *slot = if normal.length_squared() > 1e-12 {
                        normal
                    } else {
                        Vec3::Z
                    };
                });
            uvs.par_iter_mut().enumerate().for_each(|(index, slot)| {
                *slot = mesh.uvs.get(index).copied().unwrap_or(Vec2::ZERO);
            });
        });
        let colors = if mesh.colors.len() == count {
            mesh.colors.clone()
        } else {
            Vec::new()
        };
        let mut indices = Vec::with_capacity(mesh.face_count() * 2);
        for face in 0..mesh.face_count() {
            let corners = mesh.face(face);
            for i in 1..corners.len().saturating_sub(1) {
                indices.push([corners[0], corners[i], corners[i + 1]]);
            }
        }
        let positions = mesh.positions.clone();
        let bounds = Aabb::from_points(&positions);
        Self {
            positions,
            normals,
            uvs,
            colors,
            indices,
            lines: Vec::new(),
            bounds,
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len()
    }

    pub fn recompute_normals(&mut self) {
        if self.positions.is_empty() {
            self.normals.clear();
            return;
        }
        let graph = FaceGraph::from_faces(self.indices.iter().map(|triangle| &triangle[..]));
        let corners = VertexCorners::build(self.positions.len(), &graph);
        self.normals = vf_compute::area_normals(&self.positions, &graph, &corners).value;
    }
}

impl Mesh {
    pub fn vertices_len(&self) -> usize {
        self.positions.len()
    }
}

/// Structural report shown in the UI and checked by the CLI validators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshStats {
    pub vertices: usize,
    pub faces: usize,
    pub triangles: usize,
    pub quads: usize,
    pub tris: usize,
    pub ngons: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub degenerate_faces: usize,
    pub components: usize,
    pub watertight: bool,
    pub area_m2: f64,
    pub volume_m3: f64,
    pub bounds: Aabb,
}

impl MeshStats {
    pub fn is_manifold(&self) -> bool {
        self.non_manifold_edges == 0 && self.degenerate_faces == 0
    }

    pub fn summary_fr(&self) -> String {
        format!(
            "{} sommets · {} faces ({} tris, {} quads, {} n-gones) · {} composantes · {} bords · {} non-manifold · {:.0} m² · {:.0} m³",
            self.vertices,
            self.faces,
            self.tris,
            self.quads,
            self.ngons,
            self.components,
            self.boundary_edges,
            self.non_manifold_edges,
            self.area_m2,
            self.volume_m3
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        let positions = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        let faces = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ];
        Mesh::from_faces(positions, &faces)
    }

    #[test]
    fn cube_is_closed_and_has_positive_volume() {
        let mut mesh = cube();
        mesh.recompute_normals();
        let stats = mesh.stats();
        assert_eq!(stats.vertices, 8);
        assert_eq!(stats.faces, 6);
        assert_eq!(stats.quads, 6);
        assert_eq!(stats.triangles, 12);
        assert!(stats.watertight, "{stats:?}");
        assert_eq!(stats.components, 1);
        assert!((mesh.volume() - 8.0).abs() < 1e-4, "{}", mesh.volume());
        assert!((mesh.surface_area() - 24.0).abs() < 1e-4);
    }

    #[test]
    fn welding_removes_duplicates() {
        let mut mesh = Mesh::from_triangles(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::ZERO, Vec3::Z, Vec3::Y],
            vec![[0, 1, 2], [3, 4, 5]],
        );
        let removed = mesh.weld(1e-5);
        assert_eq!(removed, 2);
        assert_eq!(mesh.vertex_count(), 4);
        assert!(mesh.is_index_valid());
    }

    #[test]
    fn triangulation_of_quad_is_two_triangles() {
        let mesh = cube();
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.triangulated_indices().len(), 12);
    }

    #[test]
    fn slab_closing_makes_a_watertight_block() {
        let mut plane = Mesh::from_quads(
            vec![
                Vec3::new(0.0, 0.0, 2.0),
                Vec3::new(4.0, 0.0, 2.0),
                Vec3::new(4.0, 4.0, 2.0),
                Vec3::new(0.0, 4.0, 2.0),
            ],
            vec![[0, 1, 2, 3]],
        );
        assert!(!plane.stats().watertight);
        plane.close_slab(0.0);
        let stats = plane.stats();
        assert!(stats.watertight, "{stats:?}");
        assert!((plane.volume() - 32.0).abs() < 1e-3, "{}", plane.volume());
    }

    #[test]
    fn catmull_clark_increases_faces_and_keeps_volume_roughly() {
        let mut mesh = cube();
        let before = mesh.volume();
        let faces = mesh.catmull_clark();
        assert!(faces >= 6);
        assert!(mesh.face_count() > 6);
        assert!(mesh.stats().watertight);
        let after = mesh.volume();
        // One round of Catmull-Clark rounds the cube: the volume shrinks from
        // 8 to the exact 10/3 of the first-level surface, but stays positive
        // and bounded by the original hull.
        assert!(after > 3.0 && after < 3.7, "{before} -> {after}");
        assert!(after < before);
    }

    #[test]
    fn render_mesh_splits_hard_edges() {
        let mut mesh = cube();
        mesh.recompute_normals();
        let smooth = RenderMesh::from_mesh(&mesh, 180.0);
        let hard = RenderMesh::from_mesh(&mesh, 30.0);
        assert!(hard.positions.len() >= smooth.positions.len());
        assert_eq!(smooth.indices.len(), 12);
        assert!(hard.normals.iter().all(|n| n.length() > 0.9));
    }

    #[test]
    fn fast_render_mesh_keeps_triangles() {
        let mut mesh = cube();
        mesh.recompute_normals();
        let fast = RenderMesh::from_mesh_fast(&mesh);
        let full = RenderMesh::from_mesh(&mesh, 30.0);
        assert_eq!(fast.positions.len(), mesh.positions.len());
        assert_eq!(fast.normals.len(), mesh.positions.len());
        assert_eq!(fast.indices.len(), full.indices.len());
        assert!(fast.bounds.contains(Vec3::ZERO));
    }

    #[test]
    fn region_subdivision_is_conforming() {
        let mut mesh = Mesh::from_quads(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(2.0, 1.0, 0.0),
            ],
            vec![[0, 1, 4, 3], [1, 2, 5, 4]],
        );
        let before = mesh.stats();
        mesh.subdivide_region(&[0], 2);
        let after = mesh.stats();
        assert!(after.faces > before.faces);
        assert!(after.boundary_edges > 0);
        assert_eq!(after.components, 1);
        assert!(mesh.is_index_valid());
    }

    #[test]
    fn transfer_normals_keeps_direction() {
        let mut source = cube();
        source.recompute_normals();
        let mut target = source.clone();
        target.normals = vec![Vec3::ZERO; target.positions.len()];
        let bvh = crate::bvh::Bvh::build(&source);
        target.transfer_normals(&source, &bvh);
        assert!(target.normals.iter().all(|n| n.length() > 0.5));
    }
}

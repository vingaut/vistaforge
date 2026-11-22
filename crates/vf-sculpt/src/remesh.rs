//! Adaptive remeshing: quadric-error decimation, curvature-driven refinement,
//! isotropic remeshing (split / collapse / flip / relax) and quad pairing.
//!
//! The reference surface is never lost. Every operation can project the new
//! vertices back onto the original mesh through its BVH, so a 6 M triangle
//! scan can become a 400 k triangle mesh for export without moving the cliff
//! wall by a millimetre. Normals are transferred at the end to keep the
//! original rock signature even where the topology changed completely.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use glam::Vec3;
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::math::Aabb;
use crate::mesh::{Mesh, MeshStats};
use crate::voxel::{VoxelParams, voxel_remesh};

/// Which topology a remesh produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemeshMode {
    /// Uniform-ish triangles at the target edge length.
    Triangles,
    /// Isotropic triangles paired into quads where the surface allows it.
    Quads,
    /// Voxel remesh: watertight, quad-dominant, loses sub-voxel detail.
    Voxel,
    /// Keep the topology, just densify where the surface bends or stretches.
    RefineOnly,
}

impl RemeshMode {
    pub const ALL: [RemeshMode; 4] = [
        RemeshMode::Triangles,
        RemeshMode::Quads,
        RemeshMode::Voxel,
        RemeshMode::RefineOnly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RemeshMode::Triangles => "Triangles",
            RemeshMode::Quads => "Quads",
            RemeshMode::Voxel => "Voxel",
            RemeshMode::RefineOnly => "Raffiner",
        }
    }
}

/// Remesh parameters exposed by the UI, the CLI and the scripts.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemeshSettings {
    pub mode: RemeshMode,
    /// Target edge length, metres. 0 means "derive from the face budget".
    pub target_edge_m: f32,
    /// Densify where the surface bends more than this, in degrees.
    pub curvature_deg: f32,
    pub preserve_creases: bool,
    pub crease_angle_deg: f32,
    /// Project new vertices onto the reference surface.
    pub project_to_source: bool,
    /// Relaxation / flip iterations for the isotropic path.
    pub iterations: u32,
    /// Face budget for decimation or for target-edge derivation. 0 = no cap.
    pub max_faces: usize,
    /// Smooth the voxel result.
    pub smooth: bool,
}

impl Default for RemeshSettings {
    fn default() -> Self {
        Self {
            mode: RemeshMode::Triangles,
            target_edge_m: 0.5,
            curvature_deg: 28.0,
            preserve_creases: true,
            crease_angle_deg: 35.0,
            project_to_source: true,
            iterations: 4,
            max_faces: 1_500_000,
            smooth: true,
        }
    }
}

impl RemeshSettings {
    pub fn triangles(edge_m: f32) -> Self {
        Self {
            mode: RemeshMode::Triangles,
            target_edge_m: edge_m,
            ..Self::default()
        }
    }

    pub fn quads(edge_m: f32) -> Self {
        Self {
            mode: RemeshMode::Quads,
            target_edge_m: edge_m,
            ..Self::default()
        }
    }

    pub fn voxel(edge_m: f32) -> Self {
        Self {
            mode: RemeshMode::Voxel,
            target_edge_m: edge_m,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.target_edge_m < 0.0 {
            return Err("target edge must be positive".into());
        }
        if self.iterations > 20 {
            return Err("iterations must be <= 20".into());
        }
        Ok(())
    }
}

/// What a remesh did, for the status line and the reports.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemeshReport {
    pub mode: RemeshMode,
    pub before: MeshStats,
    pub after: MeshStats,
    pub target_edge_m: f32,
    pub elapsed_ms: u128,
}

/// Run a full remesh, preserving the reference surface when asked.
pub fn remesh(mesh: &Mesh, settings: RemeshSettings) -> Mesh {
    remesh_with_source(mesh, settings, None)
}

/// Same as [`remesh`] but with an explicit reference surface for projection.
/// Passing the pre-deformation mesh keeps sculpted detail when the current
/// mesh has already been decimated once.
pub fn remesh_with_source(
    mesh: &Mesh,
    settings: RemeshSettings,
    source: Option<(&Mesh, &Bvh)>,
) -> Mesh {
    if mesh.is_empty() {
        return mesh.clone();
    }
    let owned_source;
    let (source_mesh, source_bvh) = match source {
        Some(pair) => pair,
        None => {
            owned_source = (mesh.clone(), Bvh::build(mesh));
            (&owned_source.0, &owned_source.1)
        }
    };
    match settings.mode {
        RemeshMode::Voxel => {
            let params = VoxelParams {
                cell_m: settings.target_edge_m.max(0.01),
                ..VoxelParams::default()
            };
            let mut out = voxel_remesh(source_mesh, source_bvh, params, settings.smooth);
            if settings.project_to_source {
                project_onto(&mut out, source_mesh, source_bvh);
            }
            if !out.is_empty() {
                out.transfer_normals(source_mesh, source_bvh);
                out.recompute_normals_if_needed();
            }
            clamp_faces(&mut out, settings.max_faces);
            out
        }
        RemeshMode::RefineOnly => {
            let mut out = source_mesh.clone();
            for _ in 0..settings.iterations.max(1) {
                let split = refine_adaptive(
                    &mut out,
                    settings.target_edge_m.max(0.01),
                    settings.curvature_deg,
                );
                if split == 0 {
                    break;
                }
            }
            clamp_faces(&mut out, settings.max_faces);
            out
        }
        RemeshMode::Triangles | RemeshMode::Quads => {
            if settings.target_edge_m <= 1e-4 {
                let mut out = mesh.clone();
                if settings.max_faces > 0 && out.face_count() > settings.max_faces {
                    out = decimate(&out, settings.max_faces, true, settings.crease_angle_deg);
                }
                return out;
            }
            // A small target edge on a kilometre-wide terrain would ask for
            // billions of faces. Derive a floor from the face budget so the
            // operation always terminates in a predictable mesh.
            let budget_edge = if settings.max_faces > 0 {
                (source_mesh.surface_area() / settings.max_faces as f64).sqrt() as f32
            } else {
                0.0
            };
            let target_edge = settings.target_edge_m.max(budget_edge * 0.85);
            let mut out = isotropic_remesh(
                source_mesh,
                source_bvh,
                target_edge,
                settings.iterations,
                settings.preserve_creases,
                settings.crease_angle_deg,
                settings.max_faces,
            );
            if settings.project_to_source {
                project_onto(&mut out, source_mesh, source_bvh);
            }
            if settings.mode == RemeshMode::Quads {
                out = quadify(&out, 0.6);
            }
            clamp_faces(&mut out, settings.max_faces);
            out.transfer_normals(source_mesh, source_bvh);
            out.recompute_normals_if_needed();
            out
        }
    }
}

fn clamp_faces(mesh: &mut Mesh, max_faces: usize) {
    if max_faces == 0 || mesh.face_count() <= max_faces {
        return;
    }
    *mesh = decimate(mesh, max_faces, true, 35.0);
}

/// Project every vertex onto the reference surface (closest point).
pub fn project_onto(mesh: &mut Mesh, source: &Mesh, source_bvh: &Bvh) {
    // The BVH is read-only here, so the projections are independent and each
    // vertex is written by exactly one task.
    vf_core::compute::install(|| {
        mesh.positions.par_iter_mut().for_each(|p| {
            if let Some(hit) = source_bvh.closest_point(source, *p) {
                *p = hit.position;
            }
        });
    });
    mesh.recompute_normals();
}

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Undirected edges longer than `length`.
fn edges_longer(mesh: &Mesh, length: f32) -> Vec<(u32, u32)> {
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut out = Vec::new();
    for face in mesh.faces() {
        for i in 0..face.len() {
            let a = face[i];
            let b = face[(i + 1) % face.len()];
            let key = edge_key(a, b);
            if seen.insert(key) {
                let pa = mesh.positions[a as usize];
                let pb = mesh.positions[b as usize];
                if pa.distance(pb) > length {
                    out.push(key);
                }
            }
        }
    }
    out
}

/// Angle in degrees between the two faces of an edge, or 0 on a boundary.
fn dihedral_angle_from_faces(face_normals: &[Vec3], faces: &[u32]) -> f32 {
    let mut normals = Vec::with_capacity(2);
    for &index in faces {
        normals.push(face_normals[index as usize]);
        if normals.len() == 2 {
            break;
        }
    }
    if normals.len() < 2 {
        return 0.0;
    }
    normals[0]
        .dot(normals[1])
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// Map every edge to the faces that use it, once. Rebuilding this inside an
/// edge loop turns adaptive refinement into an O(edges × faces) scan.
fn edge_face_map(mesh: &Mesh) -> HashMap<(u32, u32), Vec<u32>> {
    let mut map: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (index, face) in mesh.faces().enumerate() {
        let n = face.len();
        for i in 0..n {
            map.entry(edge_key(face[i], face[(i + 1) % n]))
                .or_default()
                .push(index as u32);
        }
    }
    map
}
/// Split the given edges, with the same conforming red/green strategy used by
/// the region subdivision. Returns the number of new vertices.
pub fn split_edges(mesh: &mut Mesh, edges: &HashSet<(u32, u32)>) -> usize {
    if edges.is_empty() || mesh.is_empty() {
        return 0;
    }
    let face_count = mesh.face_count();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::with_capacity(edges.len());
    // Sorted keys, so the midpoint vertex ids do not depend on the hash order
    // of the `HashSet`: the same remesh must number vertices the same way on
    // every run.
    let mut keys: Vec<(u32, u32)> = edges.iter().copied().collect();
    keys.sort_unstable();
    for (a, b) in keys {
        let id = mesh.push_midpoint(a, b);
        mid.insert((a, b), id);
    }
    let mut new_faces: Vec<Vec<u32>> = Vec::with_capacity(face_count * 2);
    for face in 0..face_count {
        let corners = mesh.face(face).to_vec();
        let n = corners.len();
        let marked: Vec<bool> = (0..n)
            .map(|i| edges.contains(&edge_key(corners[i], corners[(i + 1) % n])))
            .collect();
        let count = marked.iter().filter(|m| **m).count();
        if count == 0 {
            new_faces.push(corners);
            continue;
        }
        if count == n {
            let center = mesh.push_centroid(&corners);
            let mids: Vec<u32> = (0..n)
                .map(|i| mid[&edge_key(corners[i], corners[(i + 1) % n])])
                .collect();
            for i in 0..n {
                let prev = mids[(i + n - 1) % n];
                new_faces.push(vec![corners[i], mids[i], center, prev]);
            }
        } else {
            let center = mesh.push_centroid(&corners);
            for i in 0..n {
                let a = corners[i];
                let b = corners[(i + 1) % n];
                if marked[i] {
                    let m = mid[&edge_key(a, b)];
                    new_faces.push(vec![center, a, m]);
                    new_faces.push(vec![center, m, b]);
                } else {
                    new_faces.push(vec![center, a, b]);
                }
            }
        }
    }
    mesh.corners.clear();
    mesh.starts = vec![0];
    for face in &new_faces {
        mesh.push_face(face);
    }
    mesh.face_materials = vec![0; mesh.face_count()];
    mesh.recompute_normals();
    edges.len()
}

/// Densify edges longer than `max_edge` and edges whose dihedral angle
/// exceeds `curvature_deg`. Iterated by the caller; returns the number of
/// split edges.
pub fn refine_adaptive(mesh: &mut Mesh, max_edge: f32, curvature_deg: f32) -> usize {
    let face_normals: Vec<Vec3> = (0..mesh.face_count())
        .map(|f| mesh.face_normal(f))
        .collect();
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let edge_faces = edge_face_map(mesh);
    for face in mesh.faces() {
        let n = face.len();
        for i in 0..n {
            let a = face[i];
            let b = face[(i + 1) % n];
            let key = edge_key(a, b);
            if !seen.insert(key) {
                continue;
            }
            let length = mesh.positions[a as usize].distance(mesh.positions[b as usize]);
            let angle = edge_faces
                .get(&key)
                .map(|faces| dihedral_angle_from_faces(&face_normals, faces))
                .unwrap_or(0.0);
            if length > max_edge || angle > curvature_deg {
                edges.insert(key);
            }
        }
    }
    split_edges(mesh, &edges)
}

/// Quadric error metric for one vertex: the 10 unique coefficients of the
/// symmetric 4x4 matrix, in the order a00,a01,a02,a03,a11,a12,a13,a22,a23,a33.
#[derive(Debug, Clone, Copy, Default)]
struct Quadric {
    m: [f64; 10],
}

impl Quadric {
    fn from_plane(normal: Vec3, d: f64) -> Self {
        let n = normal.as_dvec3();
        let a = n.x;
        let b = n.y;
        let c = n.z;
        Self {
            m: [
                a * a,
                a * b,
                a * c,
                a * d,
                b * b,
                b * c,
                b * d,
                c * c,
                c * d,
                d * d,
            ],
        }
    }

    fn add(&mut self, other: &Quadric) {
        for (a, b) in self.m.iter_mut().zip(other.m.iter()) {
            *a += *b;
        }
    }

    fn evaluate(&self, p: Vec3) -> f64 {
        let x = p.x as f64;
        let y = p.y as f64;
        let z = p.z as f64;
        let m = &self.m;
        m[0] * x * x
            + 2.0 * m[1] * x * y
            + 2.0 * m[2] * x * z
            + 2.0 * m[3] * x
            + m[4] * y * y
            + 2.0 * m[5] * y * z
            + 2.0 * m[6] * y
            + m[7] * z * z
            + 2.0 * m[8] * z
            + m[9]
    }

    /// Solve the 3x3 system of the optimal position. `None` when the matrix is
    /// singular, in which case the endpoints are compared instead.
    fn optimum(&self) -> Option<Vec3> {
        let m = &self.m;
        let a = [[m[0], m[1], m[2]], [m[1], m[4], m[5]], [m[2], m[5], m[7]]];
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let cof = [
            [
                (a[1][1] * a[2][2] - a[1][2] * a[2][1]) * inv,
                (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * inv,
                (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * inv,
            ],
            [
                (a[1][2] * a[2][0] - a[1][0] * a[2][2]) * inv,
                (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * inv,
                (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * inv,
            ],
            [
                (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * inv,
                (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * inv,
                (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * inv,
            ],
        ];
        let b = [-m[3], -m[6], -m[8]];
        let p = Vec3::new(
            (cof[0][0] * b[0] + cof[0][1] * b[1] + cof[0][2] * b[2]) as f32,
            (cof[1][0] * b[0] + cof[1][1] * b[1] + cof[1][2] * b[2]) as f32,
            (cof[2][0] * b[0] + cof[2][1] * b[1] + cof[2][2] * b[2]) as f32,
        );
        if p.is_finite() { Some(p) } else { None }
    }
}

struct TriangleSoup {
    positions: Vec<Vec3>,
    faces: Vec<[u32; 3]>,
    alive: Vec<bool>,
    vfaces: Vec<Vec<u32>>,
}

impl TriangleSoup {
    fn new(mesh: &Mesh) -> Self {
        let faces = mesh.triangulated_indices();
        let mut soup = Self {
            positions: mesh.positions.clone(),
            alive: vec![true; faces.len()],
            vfaces: vec![Vec::new(); mesh.positions.len()],
            faces,
        };
        for (index, face) in soup.faces.iter().enumerate() {
            for &v in face {
                soup.vfaces[v as usize].push(index as u32);
            }
        }
        soup
    }

    fn live_faces_of(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        self.vfaces[v as usize]
            .iter()
            .copied()
            .filter(move |f| self.alive[*f as usize])
    }

    fn neighbors(&self, v: u32) -> HashSet<u32> {
        let mut set = HashSet::new();
        for face_index in self.live_faces_of(v) {
            for &other in &self.faces[face_index as usize] {
                if other != v {
                    set.insert(other);
                }
            }
        }
        set
    }

    fn face_normal(&self, face_index: u32) -> Vec3 {
        let f = self.faces[face_index as usize];
        let a = self.positions[f[0] as usize];
        let b = self.positions[f[1] as usize];
        let c = self.positions[f[2] as usize];
        (b - a).cross(c - a).normalize_or_zero()
    }

    fn is_boundary_vertex(&self, v: u32) -> bool {
        for n in self.neighbors(v) {
            let shared = self
                .live_faces_of(v)
                .filter(|f| self.faces[*f as usize].contains(&n))
                .count();
            if shared == 1 {
                return true;
            }
        }
        false
    }

    /// True when the collapse keeps the surface manifold: the two one-rings
    /// share exactly the opposite vertices of the incident faces.
    fn can_collapse(&self, u: u32, v: u32, target: Vec3) -> bool {
        let shared: Vec<u32> = self
            .live_faces_of(u)
            .filter(|f| self.faces[*f as usize].contains(&v))
            .collect();
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }
        let nu = self.neighbors(u);
        let nv = self.neighbors(v);
        let common: HashSet<u32> = nu.intersection(&nv).copied().collect();
        let expected: HashSet<u32> = shared
            .iter()
            .flat_map(|f| self.faces[*f as usize])
            .filter(|x| *x != u && *x != v)
            .collect();
        if common != expected {
            return false;
        }
        // No face inversion and no zero-area face.
        for face_index in self.vfaces[u as usize]
            .iter()
            .chain(self.vfaces[v as usize].iter())
            .copied()
            .filter(|f| self.alive[*f as usize])
        {
            let face = self.faces[face_index as usize];
            if face.contains(&u) && face.contains(&v) {
                continue;
            }
            let old_normal = self.face_normal(face_index);
            let mut p = [Vec3::ZERO; 3];
            for (slot, id) in face.iter().enumerate() {
                let position = if *id == v || *id == u {
                    target
                } else {
                    self.positions[*id as usize]
                };
                p[slot] = position;
            }
            let new_normal = (p[1] - p[0]).cross(p[2] - p[0]);
            if new_normal.length_squared() < 1e-14 {
                return false;
            }
            if new_normal.normalize().dot(old_normal) < 0.15 {
                return false;
            }
        }
        // Valence stays reasonable.
        let union: HashSet<u32> = nu.union(&nv).copied().collect();
        if union.len() < 4 {
            return false;
        }
        true
    }

    fn collapse(&mut self, u: u32, v: u32, target: Vec3) {
        let shared: HashSet<u32> = self
            .live_faces_of(u)
            .filter(|f| self.faces[*f as usize].contains(&v))
            .collect();
        let incident: Vec<u32> = self.vfaces[v as usize].clone();
        for face_index in incident {
            if !self.alive[face_index as usize] {
                continue;
            }
            if shared.contains(&face_index) {
                self.alive[face_index as usize] = false;
                continue;
            }
            let face = &mut self.faces[face_index as usize];
            for id in face.iter_mut() {
                if *id == v {
                    *id = u;
                }
            }
            self.vfaces[u as usize].push(face_index);
        }
        self.vfaces[v as usize].clear();
        self.positions[u as usize] = target;
    }

    fn compact(&self, reference: &Mesh) -> Mesh {
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::new();
        for (index, face) in self.faces.iter().enumerate() {
            if !self.alive[index] {
                continue;
            }
            for &v in face {
                if remap[v as usize] == u32::MAX {
                    remap[v as usize] = positions.len() as u32;
                    positions.push(self.positions[v as usize]);
                }
            }
        }
        let faces: Vec<[u32; 3]> = self
            .faces
            .iter()
            .enumerate()
            .filter(|(i, _)| self.alive[*i])
            .map(|(_, f)| {
                [
                    remap[f[0] as usize],
                    remap[f[1] as usize],
                    remap[f[2] as usize],
                ]
            })
            .collect();
        let mut mesh = Mesh::from_triangles(positions, faces);
        mesh.box_project_uvs(1.0);
        mesh.recompute_normals();
        // Copy vertex colours from the nearest reference vertex.
        if reference.colors.len() == reference.positions.len() {
            let bvh = Bvh::build(reference);
            mesh.colors = mesh
                .positions
                .iter()
                .map(|&p| {
                    bvh.closest_point(reference, p)
                        .and_then(|hit| reference.colors.get(hit.triangle[0] as usize).copied())
                        .unwrap_or([150, 145, 138, 255])
                })
                .collect();
        }
        mesh
    }
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    cost: f64,
    u: u32,
    v: u32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}
impl Eq for Candidate {}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.total_cmp(&self.cost)
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Minimum-heap entry for the edge `u-v`.
fn push_candidate(
    heap: &mut BinaryHeap<Candidate>,
    soup: &TriangleSoup,
    quadrics: &[Quadric],
    a: u32,
    b: u32,
) {
    if a == b {
        return;
    }
    let mut q = quadrics[a as usize];
    q.add(&quadrics[b as usize]);
    let optimum = q.optimum();
    let target = match optimum {
        Some(p) => p,
        None => {
            let pa = soup.positions[a as usize];
            let pb = soup.positions[b as usize];
            if q.evaluate(pa) <= q.evaluate(pb) {
                pa
            } else {
                pb
            }
        }
    };
    let cost = q.evaluate(target);
    heap.push(Candidate { cost, u: a, v: b });
}

/// Quadric-error decimation to a face budget. Returns a triangle mesh.
pub fn decimate(
    mesh: &Mesh,
    max_faces: usize,
    preserve_boundary: bool,
    crease_angle_deg: f32,
) -> Mesh {
    let target = max_faces.max(4);
    if mesh.face_count() <= target || mesh.is_empty() {
        return mesh.clone();
    }
    let mut soup = TriangleSoup::new(mesh);
    let mut quadrics = vec![Quadric::default(); soup.positions.len()];
    let crease_cos = crease_angle_deg.clamp(0.0, 180.0).to_radians().cos();
    // Seed each vertex quadric with its incident face planes. Sharp edges get
    // extra weight so a cliff crease survives the collapse.
    for face_index in 0..soup.faces.len() {
        let face = soup.faces[face_index];
        let a = soup.positions[face[0] as usize];
        let b = soup.positions[face[1] as usize];
        let c = soup.positions[face[2] as usize];
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.length_squared() < 0.5 {
            continue;
        }
        let area = (b - a).cross(c - a).length() * 0.5;
        let d = -normal.dot(a);
        let plane = Quadric::from_plane(normal, f64::from(d));
        let mut weighted = Quadric::default();
        // Area weighting: big faces constrain the surface more than slivers.
        let weight = f64::from(area.max(1e-6)).sqrt();
        for (index, value) in plane.m.iter().enumerate() {
            weighted.m[index] = value * weight;
        }
        for &v in &face {
            quadrics[v as usize].add(&weighted);
        }
    }
    // Crease constraint: edges whose dihedral exceeds the threshold add a
    // perpendicular plane through the edge so the ridge stays put.
    let mut seen = HashSet::new();
    for face_index in 0..soup.faces.len() {
        let face = soup.faces[face_index];
        for i in 0..3 {
            let a = face[i];
            let b = face[(i + 1) % 3];
            if !seen.insert(edge_key(a, b)) {
                continue;
            }
            let mut normals = Vec::new();
            for (other_index, other) in soup.faces.iter().enumerate() {
                if other.contains(&a) && other.contains(&b) {
                    let n = soup.face_normal(other_index as u32);
                    normals.push(n);
                }
            }
            if normals.len() == 2 && normals[0].dot(normals[1]) < crease_cos {
                let edge = soup.positions[b as usize] - soup.positions[a as usize];
                let normal = edge.cross(normals[0] + normals[1]).normalize_or_zero();
                if normal.length_squared() > 0.5 {
                    let d = -normal.dot(soup.positions[a as usize]);
                    let plane = Quadric::from_plane(normal, f64::from(d));
                    let mut weighted = Quadric::default();
                    for (index, value) in plane.m.iter().enumerate() {
                        weighted.m[index] = value * 100.0;
                    }
                    quadrics[a as usize].add(&weighted);
                    quadrics[b as usize].add(&weighted);
                }
            }
        }
    }

    let boundary: Vec<bool> = (0..soup.positions.len())
        .map(|v| soup.is_boundary_vertex(v as u32))
        .collect();
    let mut heap: BinaryHeap<Candidate> = BinaryHeap::new();
    for face_index in 0..soup.faces.len() {
        let face = soup.faces[face_index];
        for i in 0..3 {
            let a = face[i];
            let b = face[(i + 1) % 3];
            if a < b {
                push_candidate(&mut heap, &soup, &quadrics, a, b);
            }
        }
    }

    let mut live_faces = soup.alive.iter().filter(|a| **a).count();
    let mut collapse_attempts = 0usize;
    while live_faces > target && collapse_attempts < 20_000_000 {
        let Some(candidate) = heap.pop() else {
            break;
        };
        collapse_attempts += 1;
        let u = candidate.u;
        let v = candidate.v;
        if !soup.vfaces[v as usize]
            .iter()
            .any(|f| soup.alive[*f as usize])
        {
            continue;
        }
        if preserve_boundary && (boundary[u as usize] || boundary[v as usize]) {
            continue;
        }
        let q = {
            let mut q = quadrics[u as usize];
            q.add(&quadrics[v as usize]);
            q
        };
        let target_position = q
            .optimum()
            .unwrap_or_else(|| (soup.positions[u as usize] + soup.positions[v as usize]) * 0.5);
        if !soup.can_collapse(u, v, target_position) {
            continue;
        }
        soup.collapse(u, v, target_position);
        let removed = quadrics[v as usize];
        quadrics[u as usize].add(&removed);
        quadrics[v as usize] = Quadric::default();
        live_faces = soup.alive.iter().filter(|a| **a).count();
        for n in soup.neighbors(u) {
            if n != u {
                push_candidate(&mut heap, &soup, &quadrics, u.min(n), u.max(n));
            }
        }
    }
    let mut out = soup.compact(mesh);
    if out.is_empty() {
        return mesh.clone();
    }
    out.recompute_normals();
    out
}

/// Isotropic remeshing: split long edges, collapse short ones, flip edges to
/// improve valence and relax tangentially, projecting onto the reference
/// surface after each pass.
pub fn isotropic_remesh(
    source: &Mesh,
    source_bvh: &Bvh,
    target_edge: f32,
    iterations: u32,
    preserve_creases: bool,
    crease_angle_deg: f32,
    max_faces: usize,
) -> Mesh {
    let target = target_edge.max(1e-3);
    let mut mesh = source.clone();
    // Start from the source resolution when it is already close.
    let mut soup = TriangleSoup::new(&mesh);
    let high = target * 4.0 / 3.0;
    let low = target * 0.8;
    for _ in 0..iterations.max(1) {
        // Splitting every edge quadruples the count. Stop before crossing the
        // caller's budget instead of allocating gigabytes and decimating back.
        if max_faces > 0 && mesh.face_count().saturating_mul(4) > max_faces {
            break;
        }
        // Split.
        let long_edges: HashSet<(u32, u32)> = edges_longer(&mesh, high).into_iter().collect();
        if !long_edges.is_empty() {
            split_edges(&mut mesh, &long_edges);
            soup = TriangleSoup::new(&mesh);
        }
        // Collapse.
        let crease_cos = crease_angle_deg.to_radians().cos();
        let mut seen = HashSet::new();
        let mut candidates: Vec<(f32, u32, u32)> = {
            let mut out = Vec::new();
            for face_index in 0..soup.faces.len() {
                let face = soup.faces[face_index];
                for i in 0..3 {
                    let a = face[i];
                    let b = face[(i + 1) % 3];
                    let key = edge_key(a, b);
                    if !seen.insert(key) {
                        continue;
                    }
                    let length = soup.positions[a as usize].distance(soup.positions[b as usize]);
                    if length >= low {
                        continue;
                    }
                    let mut crease = false;
                    if preserve_creases {
                        let mut normals = Vec::new();
                        for &other in &soup.vfaces[a as usize] {
                            if soup.faces[other as usize].contains(&b) {
                                normals.push(soup.face_normal(other));
                            }
                        }
                        if normals.len() == 2 && normals[0].dot(normals[1]) < crease_cos {
                            crease = true;
                        }
                    }
                    if !crease {
                        out.push((length, a, b));
                    }
                }
            }
            out
        };
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, a, b) in candidates {
            if !soup.vfaces[b as usize]
                .iter()
                .any(|f| soup.alive[*f as usize])
            {
                continue;
            }
            let midpoint = (soup.positions[a as usize] + soup.positions[b as usize]) * 0.5;
            if soup.can_collapse(a, b, midpoint) {
                soup.collapse(a, b, midpoint);
            }
        }
        mesh = soup.compact(source);
        // Flip toward valence six.
        let mut soup = TriangleSoup::new(&mesh);
        let mut flips = 0;
        let mut seen = HashSet::new();
        for face_index in 0..soup.faces.len() {
            let face = soup.faces[face_index];
            for i in 0..3 {
                let a = face[i];
                let b = face[(i + 1) % 3];
                if !seen.insert(edge_key(a, b)) {
                    continue;
                }
                if try_flip(&mut soup, a, b) {
                    flips += 1;
                }
            }
        }
        if flips > 0 {
            mesh = soup.compact(source);
        }
        // Tangential relaxation + projection.
        let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); mesh.positions.len()];
        for face in mesh.faces() {
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                if !neighbours[a as usize].contains(&b) {
                    neighbours[a as usize].push(b);
                }
                if !neighbours[b as usize].contains(&a) {
                    neighbours[b as usize].push(a);
                }
            }
        }
        let previous = mesh.positions.clone();
        for (index, list) in neighbours.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let normal = mesh.normals[index];
            let mut average = Vec3::ZERO;
            for &n in list {
                average += previous[n as usize];
            }
            average /= list.len() as f32;
            let delta = average - previous[index];
            let tangent = delta - normal * delta.dot(normal);
            mesh.positions[index] = previous[index] + tangent * 0.45;
        }
        for p in &mut mesh.positions {
            if let Some(hit) = source_bvh.closest_point(source, *p) {
                *p = hit.position;
            }
        }
        mesh.recompute_normals();
    }
    // Final snap, so the caller always gets vertices on the reference surface
    // even if the last pass ended with a flip.
    project_onto(&mut mesh, source, source_bvh);
    mesh
}

fn try_flip(soup: &mut TriangleSoup, a: u32, b: u32) -> bool {
    let faces: Vec<u32> = soup
        .live_faces_of(a)
        .filter(|f| soup.faces[*f as usize].contains(&b))
        .collect();
    if faces.len() != 2 {
        return false;
    }
    let f0 = soup.faces[faces[0] as usize];
    let f1 = soup.faces[faces[1] as usize];
    let c = *f0.iter().find(|v| **v != a && **v != b).unwrap_or(&a);
    let d = *f1.iter().find(|v| **v != a && **v != b).unwrap_or(&b);
    if c == d {
        return false;
    }
    if soup.neighbors(c).contains(&d) {
        return false;
    }
    let valence_c = soup.neighbors(c).len() as i32;
    let valence_d = soup.neighbors(d).len() as i32;
    let valence_a = soup.neighbors(a).len() as i32;
    let valence_b = soup.neighbors(b).len() as i32;
    let before = (valence_c - 6).abs()
        + (valence_d - 6).abs()
        + (valence_a - 6).abs()
        + (valence_b - 6).abs();
    let after = (valence_c - 7).abs()
        + (valence_d - 7).abs()
        + (valence_a - 5).abs()
        + (valence_b - 5).abs();
    if after >= before {
        return false;
    }
    // Geometric validity: the new diagonal must not fold the surface.
    let pa = soup.positions[a as usize];
    let pb = soup.positions[b as usize];
    let pc = soup.positions[c as usize];
    let pd = soup.positions[d as usize];
    let old = soup.face_normal(faces[0]).dot(soup.face_normal(faces[1]));
    let n0 = (pd - pc).cross(pa - pc).normalize_or_zero();
    let n1 = (pa - pb).cross(pd - pb).normalize_or_zero();
    if n0.length_squared() < 0.5 || n1.length_squared() < 0.5 {
        return false;
    }
    if n0.dot(n1) < old {
        return false;
    }
    soup.faces[faces[0] as usize] = [c, d, a];
    soup.faces[faces[1] as usize] = [d, c, b];
    // f0/f1 no longer touch a or b; both now touch c and d.
    for vertex in [a, b] {
        soup.vfaces[vertex as usize].retain(|f| *f != faces[0] && *f != faces[1]);
    }
    for vertex in [c, d] {
        soup.vfaces[vertex as usize].push(faces[0]);
        soup.vfaces[vertex as usize].push(faces[1]);
    }
    true
}

/// Pair adjacent triangles into quads when the two faces are nearly coplanar
/// and the resulting quad is reasonably square.
pub fn quadify(mesh: &Mesh, angle_threshold: f32) -> Mesh {
    let triangles = mesh.triangulated_indices();
    if triangles.is_empty() {
        return mesh.clone();
    }
    let face_normals: Vec<Vec3> = triangles
        .iter()
        .map(|t| {
            let a = mesh.positions[t[0] as usize];
            let b = mesh.positions[t[1] as usize];
            let c = mesh.positions[t[2] as usize];
            (b - a).cross(c - a).normalize_or_zero()
        })
        .collect();
    let cos_threshold = angle_threshold.clamp(0.0, 1.0);
    let mut edge_faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (index, tri) in triangles.iter().enumerate() {
        for i in 0..3 {
            edge_faces
                .entry(edge_key(tri[i], tri[(i + 1) % 3]))
                .or_default()
                .push(index as u32);
        }
    }
    let mut candidates: Vec<(f32, u32, u32)> = Vec::new();
    for (&(a, b), faces) in &edge_faces {
        if faces.len() != 2 {
            continue;
        }
        let f0 = faces[0];
        let f1 = faces[1];
        let align = face_normals[f0 as usize].dot(face_normals[f1 as usize]);
        if align < cos_threshold {
            continue;
        }
        let c = *triangles[f0 as usize]
            .iter()
            .find(|v| **v != a && **v != b)
            .unwrap_or(&a);
        let d = *triangles[f1 as usize]
            .iter()
            .find(|v| **v != a && **v != b)
            .unwrap_or(&b);
        if c == d {
            continue;
        }
        // Prefer short shared edges and square quads.
        let shared = mesh.positions[a as usize].distance(mesh.positions[b as usize]);
        let opposite = mesh.positions[c as usize].distance(mesh.positions[d as usize]);
        let score = align - (shared - opposite).abs() * 0.1;
        candidates.push((score, f0, f1));
    }
    // The candidates come from a `HashMap`, whose iteration order changes with
    // the process seed. Sorting by score alone would leave ties in that random
    // order, and the greedy pairing below would then produce a different mesh
    // on every run. Break ties on the face indices so the result is stable.
    rank_pairs(&mut candidates);
    let mut used = vec![false; triangles.len()];
    let mut faces: Vec<Vec<u32>> = Vec::with_capacity(triangles.len() / 2 + 2);
    for (_, f0, f1) in candidates {
        if used[f0 as usize] || used[f1 as usize] {
            continue;
        }
        let t0 = triangles[f0 as usize];
        let t1 = triangles[f1 as usize];
        let shared: Vec<u32> = t0.iter().copied().filter(|v| t1.contains(v)).collect();
        if shared.len() != 2 {
            continue;
        }
        let a = shared[0];
        let b = shared[1];
        let c = *t0.iter().find(|v| **v != a && **v != b).unwrap_or(&a);
        let d = *t1.iter().find(|v| **v != a && **v != b).unwrap_or(&b);
        // Order the quad around its perimeter: c -> a -> d -> b (or the
        // reverse) depending on the winding of the first triangle.
        let ordered = if t0[1] == a || t0[2] == a {
            vec![c, a, d, b]
        } else {
            vec![c, b, d, a]
        };
        used[f0 as usize] = true;
        used[f1 as usize] = true;
        faces.push(ordered);
    }
    for (index, tri) in triangles.iter().enumerate() {
        if !used[index] {
            faces.push(tri.to_vec());
        }
    }
    let mut out = Mesh::from_faces(mesh.positions.clone(), &faces);
    out.uvs = mesh.uvs.clone();
    out.colors = mesh.colors.clone();
    out.face_materials = vec![0; out.face_count()];
    out.box_project_uvs(1.0);
    out.recompute_normals();
    out
}

/// Order the quad-pairing candidates: best score first, and for equal scores the
/// lowest face indices first.
///
/// The candidates are collected by walking a `HashMap` of edges, so without the
/// index tie-break the greedy pairing that follows would change with the
/// process hash seed and the same mesh would remesh differently on every run.
fn rank_pairs(candidates: &mut [(f32, u32, u32)]) {
    candidates.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
}

/// Signed distance from a point to the mesh, for reports and validation.
pub fn surface_distance(mesh: &Mesh, bvh: &Bvh, point: Vec3) -> f32 {
    bvh.closest_point(mesh, point)
        .map(|hit| hit.distance_sq.sqrt())
        .unwrap_or(f32::INFINITY)
}

/// Bounding box of the vertices that moved further than `epsilon`.
pub fn changed_bounds(before: &[Vec3], after: &[Vec3], epsilon: f32) -> Aabb {
    let mut bounds = Aabb::EMPTY;
    for (a, b) in before.iter().zip(after.iter()) {
        if a.distance(*b) > epsilon {
            bounds = bounds.with_point(*a).with_point(*b);
        }
    }
    bounds
}

/// Clamp an angle to the useful range for a remesh UI slider.
pub fn clamp_curvature(degrees: f32) -> f32 {
    degrees.clamp(1.0, 89.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    fn sphere(radius: f32, segments: usize) -> Mesh {
        let mut positions = Vec::new();
        let mut faces = Vec::new();
        for j in 0..=segments {
            let v = j as f32 / segments as f32;
            let phi = v * std::f32::consts::PI;
            for i in 0..=segments {
                let u = i as f32 / segments as f32;
                let theta = u * std::f32::consts::TAU;
                positions.push(Vec3::new(
                    radius * phi.sin() * theta.cos(),
                    radius * phi.sin() * theta.sin(),
                    radius * phi.cos(),
                ));
            }
        }
        let stride = (segments + 1) as u32;
        for j in 0..segments as u32 {
            for i in 0..segments as u32 {
                let a = j * stride + i;
                let b = a + 1;
                let c = a + stride;
                let d = c + 1;
                faces.push(vec![a, b, d]);
                faces.push(vec![a, d, c]);
            }
        }
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.weld(1e-4);
        mesh.remove_degenerate_faces(1e-9);
        mesh.recompute_normals();
        mesh
    }

    /// Flat `columns × rows` grid, two triangles per cell.
    fn flat_grid(columns: u32, rows: u32) -> Mesh {
        let stride = columns + 1;
        let mut positions = Vec::new();
        for y in 0..=rows {
            for x in 0..=columns {
                positions.push(Vec3::new(x as f32, y as f32, 0.0));
            }
        }
        let mut faces = Vec::new();
        for y in 0..rows {
            for x in 0..columns {
                let a = y * stride + x;
                let b = a + 1;
                let c = a + stride;
                let d = c + 1;
                faces.push(vec![a, b, d]);
                faces.push(vec![a, d, c]);
            }
        }
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.recompute_normals();
        mesh
    }

    #[test]
    fn decimate_reduces_faces_and_keeps_shape() {
        let mesh = sphere(2.0, 48);
        let before = mesh.face_count();
        let out = decimate(&mesh, before / 4, true, 35.0);
        assert!(out.face_count() < before);
        assert!(out.face_count() > before / 8, "{}", out.face_count());
        let stats = out.stats();
        assert!(stats.boundary_edges == 0);
        assert!(stats.vertices > 3);
        for p in &out.positions {
            let radius = p.length();
            assert!((radius - 2.0).abs() < 0.3, "{radius}");
        }
    }

    #[test]
    fn adaptive_refinement_splits_curvature_and_long_edges() {
        let mesh = sphere(1.0, 12);
        let before = mesh.face_count();
        let mut refined = mesh.clone();
        let split = refine_adaptive(&mut refined, 0.2, 20.0);
        assert!(split > 0);
        assert!(refined.face_count() > before);
        assert_eq!(refined.stats().boundary_edges, 0);
    }

    #[test]
    fn isotropic_remesh_is_uniform_and_projects() {
        let source = sphere(1.0, 16);
        let bvh = Bvh::build(&source);
        let out = isotropic_remesh(&source, &bvh, 0.18, 2, true, 35.0, 0);
        assert!(out.face_count() > 100);
        let stats = out.stats();
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        let mut lengths = Vec::new();
        let mut seen = HashSet::new();
        for face in out.faces() {
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                if seen.insert(edge_key(a, b)) {
                    lengths.push(out.positions[a as usize].distance(out.positions[b as usize]));
                }
            }
        }
        let mean = lengths.iter().sum::<f32>() / lengths.len() as f32;
        assert!((mean - 0.18).abs() < 0.06, "mean edge {mean}");
        for p in &out.positions {
            let distance = bvh
                .closest_point(&source, *p)
                .map(|hit| hit.distance_sq.sqrt())
                .unwrap_or(f32::INFINITY);
            assert!(
                distance < 0.002,
                "vertex {distance} from the source surface"
            );
        }
    }

    #[test]
    fn quadify_pairs_coplanar_triangles() {
        let mesh = sphere(1.0, 16);
        let quads = quadify(&mesh, 0.2);
        let stats = quads.stats();
        assert!(stats.quads > 0);
        assert!(stats.quads * 2 >= stats.tris, "{stats:?}");
    }

    /// The candidates come from a `HashMap`, so equal scores must not be left
    /// in iteration order: the greedy pass would pick different pairs and the
    /// face count would change from one process to the next.
    #[test]
    fn quad_candidates_are_ranked_independently_of_their_input_order() {
        let mut forward = vec![
            (0.9f32, 4u32, 9u32),
            (0.9, 2, 7),
            (1.0, 5, 6),
            (0.9, 2, 3),
            (0.5, 1, 8),
        ];
        let mut backward: Vec<(f32, u32, u32)> = forward.iter().rev().copied().collect();
        let mut rotated = vec![forward[2], forward[4], forward[0], forward[3], forward[1]];
        rank_pairs(&mut forward);
        rank_pairs(&mut backward);
        rank_pairs(&mut rotated);
        assert_eq!(forward, backward);
        assert_eq!(forward, rotated);
        // Best score first, then ascending face indices for the tie.
        assert_eq!(forward[0], (1.0, 5, 6));
        assert_eq!(forward[1], (0.9, 2, 3));
        assert_eq!(forward[2], (0.9, 2, 7));
        assert_eq!(forward[3], (0.9, 4, 9));
        assert_eq!(forward[4], (0.5, 1, 8));
    }

    /// On a flat regular grid every triangle can pair with a neighbour: a
    /// pair-order-dependent greedy pass leaves triangles behind, so the quad
    /// count is a direct witness of the deterministic ranking.
    #[test]
    fn a_flat_grid_quadifies_completely_and_repeatedly() {
        let mesh = flat_grid(12, 9);
        let first = quadify(&mesh, 0.2);
        let second = quadify(&mesh, 0.2);
        assert_eq!(first.face_count(), second.face_count());
        assert_eq!(
            first.stats().quads,
            first.face_count(),
            "{:?}",
            first.stats()
        );
        assert_eq!(first.face_count(), 12 * 9);
    }

    #[test]
    fn remesh_dispatch_covers_every_mode() {
        let mesh = sphere(1.0, 12);
        for mode in RemeshMode::ALL {
            let settings = RemeshSettings {
                mode,
                target_edge_m: 0.25,
                iterations: 1,
                max_faces: 200_000,
                ..RemeshSettings::default()
            };
            let out = remesh(&mesh, settings);
            assert!(!out.is_empty(), "{mode:?}");
            assert!(out.positions.iter().all(|p| p.is_finite()));
            assert!(out.face_count() > 0, "{mode:?}");
        }
    }
}

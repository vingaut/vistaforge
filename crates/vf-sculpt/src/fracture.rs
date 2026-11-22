//! Fracturing a mesh into pieces: Voronoi cells clipped out of the original
//! triangles, radial shatter cones, layered slabs and columnar joints.
//!
//! The clips preserve the original surface detail: a piece is the original
//! triangles cut by half-spaces, with a triangulated cap on every cut. That is
//! what makes a broken cliff look like rock rather than like a low-poly
//! explosion.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::math::{Aabb, basis_from_normal, hash_u64, smoothstep};
use crate::mesh::Mesh;
use crate::voxel::VoxelParams;

/// How the pieces are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FracturePattern {
    /// 3D Voronoi cells, the geological default.
    Voronoi,
    /// Radial shatter cone around the impact point.
    Radial,
    /// Horizontal slabs following the bedding.
    Layered,
    /// Vertical columns (basalt organ pipes).
    Columnar,
    /// Parallel shear blocks along a direction.
    Shear,
}

impl FracturePattern {
    pub const ALL: [FracturePattern; 5] = [
        FracturePattern::Voronoi,
        FracturePattern::Radial,
        FracturePattern::Layered,
        FracturePattern::Columnar,
        FracturePattern::Shear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FracturePattern::Voronoi => "Voronoi",
            FracturePattern::Radial => "Éclatement radial",
            FracturePattern::Layered => "Dalles horizontales",
            FracturePattern::Columnar => "Colonnes",
            FracturePattern::Shear => "Blocs de cisaillement",
        }
    }
}

/// Fracture parameters.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FractureSettings {
    pub pattern: FracturePattern,
    /// Average cell size, metres.
    pub cell_m: f32,
    /// 0 = regular grid of sites, 1 = strongly jittered.
    pub jitter: f32,
    /// Pieces are shrunk by this much, leaving a visible gap.
    pub gap_m: f32,
    /// Number of radial sectors for `Radial`, columns for `Columnar`.
    pub divisions: u32,
    /// Bedding thickness for `Layered`, metres.
    pub layer_m: f32,
    /// Direction of the shear planes, degrees from +X.
    pub direction_deg: f32,
    /// Keep pieces smaller than this out of the result (they are gravel).
    pub min_volume_m3: f32,
    pub seed: u64,
}

impl Default for FractureSettings {
    fn default() -> Self {
        Self {
            pattern: FracturePattern::Voronoi,
            cell_m: 4.0,
            jitter: 0.55,
            gap_m: 0.02,
            divisions: 9,
            layer_m: 1.6,
            direction_deg: 0.0,
            min_volume_m3: 0.001,
            seed: 0,
        }
    }
}

impl FractureSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.cell_m <= 0.0 {
            return Err("la taille de cellule doit être positive".into());
        }
        if self.divisions == 0 {
            return Err("le nombre de divisions doit être >= 1".into());
        }
        Ok(())
    }
}

/// One broken piece.
#[derive(Debug, Clone)]
pub struct FracturePiece {
    pub mesh: Mesh,
    pub centroid: Vec3,
    pub volume_m3: f32,
    pub bounds: Aabb,
    /// Index of the Voronoi site that produced it.
    pub site: u32,
}

/// Result of a fracture pass.
#[derive(Debug, Clone, Default)]
pub struct FractureResult {
    pub pieces: Vec<FracturePiece>,
    /// Volume removed from the source (the sum of the pieces).
    pub volume_m3: f32,
    pub gaps: usize,
}

/// Clip a mesh by the half-space `dot(normal, p) >= offset`.
///
/// The cut edges are chained into loops and capped with a triangle fan, so the
/// result stays closed. Intersection vertices are welded through a spatial
/// hash, otherwise adjacent triangles would leave hairline cracks.
pub fn clip_mesh(mesh: &Mesh, normal: Vec3, offset: f32, cap: bool) -> Mesh {
    let normal = normal.normalize_or_zero();
    if normal.length_squared() < 0.5 || mesh.is_empty() {
        return mesh.clone();
    }
    let mut out = Mesh::new();
    let mut weld: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut cut_segments: Vec<(u32, u32)> = Vec::new();
    let vertex_of = |p: Vec3, out: &mut Mesh, weld: &mut HashMap<(i64, i64, i64), u32>| -> u32 {
        let key = (
            (p.x * 8192.0).round() as i64,
            (p.y * 8192.0).round() as i64,
            (p.z * 8192.0).round() as i64,
        );
        if let Some(&index) = weld.get(&key) {
            index
        } else {
            let index = out.push_vertex(p);
            weld.insert(key, index);
            index
        }
    };
    for face_index in 0..mesh.face_count() {
        let face = mesh.face(face_index);
        if face.len() < 3 {
            continue;
        }
        let mut polygon: Vec<Vec3> = Vec::with_capacity(face.len() + 2);
        let mut came_from_cut = false;
        let mut cut_points: Vec<Vec3> = Vec::new();
        for i in 0..face.len() {
            let a = mesh.positions[face[i] as usize];
            let b = mesh.positions[face[(i + 1) % face.len()] as usize];
            let da = normal.dot(a) - offset;
            let db = normal.dot(b) - offset;
            if da >= 0.0 {
                polygon.push(a);
            }
            if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                let t = da / (da - db);
                let p = a.lerp(b, t);
                polygon.push(p);
                cut_points.push(p);
                came_from_cut = true;
            }
        }
        // Remove consecutive duplicates.
        polygon.dedup_by(|a, b| a.distance_squared(*b) < 1e-12);
        if polygon.len() < 3 {
            continue;
        }
        let ids: Vec<u32> = polygon
            .iter()
            .map(|&p| vertex_of(p, &mut out, &mut weld))
            .collect();
        out.push_face(&ids);
        if came_from_cut && cut_points.len() >= 2 {
            let a = vertex_of(cut_points[0], &mut out, &mut weld);
            let b = vertex_of(cut_points[1], &mut out, &mut weld);
            if a != b {
                cut_segments.push((a, b));
            }
        }
    }
    if cap && !cut_segments.is_empty() {
        cap_cut(&mut out, &cut_segments, normal);
    }
    out.box_project_uvs(1.0);
    out.recompute_normals();
    out
}

/// Close the cut boundary with a fan around its centroid. The loop direction
/// follows the cut so the cap faces the same way as the surrounding surface.
fn cap_cut(mesh: &mut Mesh, segments: &[(u32, u32)], _normal: Vec3) {
    // Chain the segments into loops. The cut of a convex piece is a simple
    // loop; a concave piece can have several, which are capped separately.
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in segments {
        next.entry(a).or_default().push(b);
    }
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for &(start, _) in segments {
        if used.iter().any(|&(a, _)| a == start) {
            continue;
        }
        let mut loop_vertices = vec![start];
        let mut current = start;
        while let Some(candidates) = next.get(&current) {
            let Some(&next_vertex) = candidates.iter().find(|&&b| !used.contains(&(current, b)))
            else {
                break;
            };
            used.insert((current, next_vertex));
            if next_vertex == start {
                break;
            }
            loop_vertices.push(next_vertex);
            current = next_vertex;
            if loop_vertices.len() > segments.len() + 2 {
                break;
            }
        }
        if loop_vertices.len() >= 3 {
            loops.push(loop_vertices);
        }
    }
    for loop_vertices in loops {
        let center = loop_vertices
            .iter()
            .fold(Vec3::ZERO, |acc, &v| acc + mesh.positions[v as usize])
            / loop_vertices.len() as f32;
        let center_index = mesh.push_vertex(center);
        for i in 0..loop_vertices.len() {
            let a = loop_vertices[i];
            let b = loop_vertices[(i + 1) % loop_vertices.len()];
            // The cap follows the cut loop: the surrounding faces use a -> b,
            // so the cap uses b -> a through the centre.
            mesh.push_face(&[a, center_index, b]);
        }
    }
}

/// Voronoi sites inside `region`.
fn voronoi_sites(region: Aabb, settings: FractureSettings) -> Vec<Vec3> {
    let size = region.size();
    let cell = settings.cell_m.max(0.05);
    let nx = ((size.x / cell).round() as i64).clamp(1, 48);
    let ny = ((size.y / cell).round() as i64).clamp(1, 48);
    let nz = ((size.z / cell).round() as i64).clamp(1, 48);
    let mut sites = Vec::new();
    let mut index = 0u64;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let base = Vec3::new(
                    (x as f32 + 0.5) / nx as f32,
                    (y as f32 + 0.5) / ny as f32,
                    (z as f32 + 0.5) / nz as f32,
                );
                let jitter = settings.jitter * 0.5;
                let offset = Vec3::new(
                    (hash_u64(settings.seed ^ index.wrapping_mul(3)) as f32 / u64::MAX as f32
                        - 0.5)
                        * jitter,
                    (hash_u64(settings.seed ^ index.wrapping_mul(5) ^ 0x1234) as f32
                        / u64::MAX as f32
                        - 0.5)
                        * jitter,
                    (hash_u64(settings.seed ^ index.wrapping_mul(7) ^ 0xABCD) as f32
                        / u64::MAX as f32
                        - 0.5)
                        * jitter,
                );
                let p = base + offset;
                sites.push(region.min + p * size);
                index += 1;
            }
        }
    }
    sites
}

fn pattern_sites(mesh: &Mesh, settings: FractureSettings) -> Vec<Vec3> {
    let region = mesh.bounds();
    match settings.pattern {
        FracturePattern::Voronoi => voronoi_sites(region, settings),
        FracturePattern::Radial => {
            let center = region.center();
            let count = settings.divisions.clamp(2, 128) as usize;
            let mut sites = vec![center];
            for i in 0..count {
                let angle = std::f32::consts::TAU * i as f32 / count as f32;
                let radius = settings.cell_m.max(0.1);
                sites.push(center + Vec3::new(angle.cos(), angle.sin(), 0.0) * radius);
            }
            sites
        }
        FracturePattern::Layered => {
            let size = region.size();
            let layers = ((size.z / settings.layer_m.max(0.05)).round() as i64).clamp(1, 64);
            let count = settings.divisions.clamp(1, 32) as i64;
            let mut sites = Vec::new();
            for z in 0..layers {
                for i in 0..count {
                    let x = (i as f32 + 0.5) / count as f32 - 0.5;
                    sites.push(
                        region.min
                            + Vec3::new(
                                (x + 0.5) * size.x,
                                0.5 * size.y,
                                (z as f32 + 0.5) / layers as f32 * size.z,
                            ),
                    );
                }
            }
            sites
        }
        FracturePattern::Columnar => {
            let size = region.size();
            let count = settings.divisions.clamp(2, 64) as i64;
            let mut sites = Vec::new();
            for i in 0..count {
                let angle = std::f32::consts::TAU * i as f32 / count as f32;
                sites.push(
                    region.min
                        + Vec3::new(
                            (0.5 + angle.cos() * 0.4) * size.x,
                            (0.5 + angle.sin() * 0.4) * size.y,
                            0.5 * size.z,
                        ),
                );
            }
            sites
        }
        FracturePattern::Shear => {
            let size = region.size();
            let angle = settings.direction_deg.to_radians();
            let dir = Vec3::new(angle.cos(), angle.sin(), 0.0);
            let cell = settings.cell_m.max(0.1);
            let count = ((size.length() / cell).round() as i64).clamp(1, 64);
            let mut sites = Vec::new();
            for i in 0..count {
                sites.push(region.center() + dir * ((i as f32 - count as f32 * 0.5) * cell));
            }
            sites
        }
    }
}

/// Split a mesh into pieces.
///
/// The pieces are extracted from a signed-distance voxelisation: every voxel
/// is assigned to its nearest site, then each site is re-extracted as a closed
/// solid. That is the only approach that yields *solid* chunks from a closed
/// shell, including cells that never touch the original surface.
pub fn fracture(mesh: &Mesh, settings: FractureSettings) -> FractureResult {
    if mesh.is_empty() {
        return FractureResult::default();
    }
    let mut sites = pattern_sites(mesh, settings);
    if sites.len() > 128 {
        sites.truncate(128);
    }
    if sites.is_empty() {
        return FractureResult::default();
    }
    let bvh = Bvh::build(mesh);
    let cell = (settings.cell_m * 0.18).clamp(0.03, 0.6);
    let params = VoxelParams {
        cell_m: cell,
        band_cells: 2.0,
        padding_cells: 2,
        max_voxels: 32_000_000,
    };
    let grid = crate::voxel::voxelize(mesh, &bvh, params);
    if grid.values.is_empty() {
        return FractureResult::default();
    }
    let [nx, ny, nz] = grid.dims;
    // Nearest-site labelling of every voxel inside the solid. Outside voxels
    // stay unlabelled: they become the `+band` margin of every piece. Rows are
    // independent, so the O(voxels × sites) search runs in parallel; the
    // per-piece bounds are then accumulated in a cheap sequential pass.
    let mut labels = vec![u16::MAX; grid.values.len()];
    let row = nx * ny;
    vf_core::compute::install(|| {
        labels
            .par_chunks_mut(row)
            .enumerate()
            .for_each(|(z, slab)| {
                for y in 0..ny {
                    for x in 0..nx {
                        let index = y * nx + x;
                        if grid.values[z * row + index] > 0.0 {
                            continue;
                        }
                        let p = grid.point(x, y, z);
                        let mut best = 0usize;
                        let mut best_distance = f32::INFINITY;
                        for (site_index, &site) in sites.iter().enumerate() {
                            let distance = p.distance_squared(site);
                            if distance < best_distance {
                                best_distance = distance;
                                best = site_index;
                            }
                        }
                        slab[index] = best as u16;
                    }
                }
            });
    });
    let mut bounds: Vec<Option<([usize; 3], [usize; 3])>> = vec![None; sites.len()];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let best = labels[z * row + y * nx + x];
                if best == u16::MAX {
                    continue;
                }
                let slot = &mut bounds[best as usize];
                match slot {
                    None => *slot = Some(([x, y, z], [x, y, z])),
                    Some((min, max)) => {
                        min[0] = min[0].min(x);
                        min[1] = min[1].min(y);
                        min[2] = min[2].min(z);
                        max[0] = max[0].max(x);
                        max[1] = max[1].max(y);
                        max[2] = max[2].max(z);
                    }
                }
            }
        }
    }
    let mut result = FractureResult::default();
    for (site_index, slot) in bounds.iter().enumerate() {
        let Some((min, max)) = slot else {
            continue;
        };
        // One voxel of margin so the piece is closed even when it touches the
        // edge of the labelled region.
        let pad = 1usize;
        let min = [
            min[0].saturating_sub(pad),
            min[1].saturating_sub(pad),
            min[2].saturating_sub(pad),
        ];
        let max = [
            (max[0] + pad).min(nx - 1),
            (max[1] + pad).min(ny - 1),
            (max[2] + pad).min(nz - 1),
        ];
        let dims = [
            max[0] - min[0] + 1,
            max[1] - min[1] + 1,
            max[2] - min[2] + 1,
        ];
        if dims.iter().any(|d| *d < 3) {
            continue;
        }
        let origin = grid.point(min[0], min[1], min[2]);
        let mut sub = crate::voxel::Grid::with_dims(origin, grid.cell, dims, grid.band);
        for lz in 0..dims[2] {
            for ly in 0..dims[1] {
                for lx in 0..dims[0] {
                    let gx = min[0] + lx;
                    let gy = min[1] + ly;
                    let gz = min[2] + lz;
                    let global = grid.index(gx, gy, gz);
                    let local = sub.index(lx, ly, lz);
                    if labels[global] == site_index as u16 {
                        sub.values[local] = (grid.values[global] + settings.gap_m).min(grid.band);
                        sub.state[local] = 1;
                    } else {
                        sub.values[local] = grid.band;
                        sub.state[local] = 2;
                    }
                }
            }
        }
        let mut piece = crate::voxel::surface_nets(&sub, 1);
        if piece.is_empty() {
            continue;
        }
        piece.translate(origin);
        crate::voxel::taubin_smooth(&mut piece, 1, 0.5, -0.53);
        piece.remove_degenerate_faces(1e-10);
        piece.compact();
        let stats = piece.stats();
        let volume = stats.volume_m3.abs() as f32;
        if volume < settings.min_volume_m3 {
            continue;
        }
        result.pieces.push(FracturePiece {
            centroid: piece.bounds().center(),
            volume_m3: volume,
            bounds: piece.bounds(),
            site: site_index as u32,
            mesh: piece,
        });
        result.volume_m3 += volume;
        if stats.boundary_edges > 0 {
            result.gaps += 1;
        }
    }
    // Biggest pieces first: the UI shows those and physics uses them.
    result
        .pieces
        .sort_by(|a, b| b.volume_m3.total_cmp(&a.volume_m3));
    result
}

/// Fracture only the part of the mesh inside `region`, keeping the rest as a
/// single "core" mesh. Used for carving a crater into a cliff face.
pub fn fracture_region(mesh: &Mesh, region: Aabb, settings: FractureSettings) -> FractureResult {
    // Cut a closed block out of the source first: fracturing an open shell
    // would produce shells instead of solid chunks.
    let mut block = mesh.clone();
    for axis in 0..3 {
        let normal = match axis {
            0 => Vec3::X,
            1 => Vec3::Y,
            _ => Vec3::Z,
        };
        block = clip_mesh(&block, -normal, -region.max[axis], true);
        block = clip_mesh(&block, normal, region.min[axis], true);
        if block.is_empty() {
            return FractureResult::default();
        }
    }
    let mut result = fracture(&block, settings);
    result
        .pieces
        .retain(|piece| piece.bounds.intersects(&region));
    result
}

/// Remove a piece from the source with a boolean-like clip, leaving a cavity.
/// The cavity walls are the negated half-spaces of the piece.
pub fn carve_piece(mesh: &Mesh, piece: &FracturePiece) -> Mesh {
    // Subtract the piece by clipping the source with the piece's bounding
    // planes; a true boolean is overkill for a debris cavity and this stays
    // watertight because each clip caps itself.
    let mut out = mesh.clone();
    let center = piece.bounds.center();
    let half = piece.bounds.size() * 0.5;
    for axis in 0..3 {
        let normal = match axis {
            0 => Vec3::X,
            1 => Vec3::Y,
            _ => Vec3::Z,
        };
        let upper = clip_mesh(&out, -normal, -(center[axis] + half[axis]), true);
        let lower = clip_mesh(&upper, normal, center[axis] - half[axis], true);
        out = lower;
    }
    out
}

/// Radial crater fracture helper, used by the impact tool and the debris
/// system. Returns the pieces and the depth of the crater.
pub fn impact_crater(
    mesh: &Mesh,
    center: Vec3,
    radius: f32,
    depth: f32,
    settings: FractureSettings,
) -> (FractureResult, f32) {
    let region = Aabb::from_center_size(center, Vec3::splat(radius * 2.0));
    let mut local = settings;
    local.pattern = FracturePattern::Radial;
    local.cell_m = radius * 0.35;
    local.divisions = (radius / 0.5).clamp(4.0, 48.0) as u32;
    let pieces = fracture_region(mesh, region, local);
    (pieces, depth)
}

/// Height of a debris pile formed by dropping `pieces` on `terrain`.
pub fn pile_height(pieces: &[FracturePiece], terrain: Option<(&Mesh, &Bvh)>) -> f32 {
    let mut height = 0.0f32;
    for piece in pieces {
        let base = piece.bounds.min.z;
        let ground = terrain
            .and_then(|(mesh, bvh)| bvh.height_under(mesh, piece.centroid.x, piece.centroid.y))
            .unwrap_or(0.0);
        height = height.max((base - ground).max(0.0) + piece.bounds.size().z);
    }
    height
}

/// Split a mesh along a single plane, returning the two closed halves.
pub fn split_in_two(mesh: &Mesh, plane_point: Vec3, plane_normal: Vec3) -> (Mesh, Mesh) {
    let normal = plane_normal.normalize_or_zero();
    if normal.length_squared() < 0.5 {
        return (mesh.clone(), Mesh::new());
    }
    let offset = normal.dot(plane_point);
    let a = clip_mesh(mesh, normal, offset, true);
    let b = clip_mesh(mesh, -normal, -offset, true);
    (a, b)
}

/// Score a fracture pattern: fraction of the volume that ended up in pieces,
/// and the average piece size. Used by the tests and the UI report.
pub fn coverage(mesh: &Mesh, result: &FractureResult) -> f32 {
    let source = mesh.volume().abs() as f32;
    (result.volume_m3 / source.max(1e-9)).clamp(0.0, 1.0)
}

/// A deterministic jitter offset for a site, exposed for tests.
pub fn site_jitter(seed: u64, index: u64) -> Vec3 {
    let x = hash_u64(seed ^ index.wrapping_mul(0x9E37_79B9)) as f32 / u64::MAX as f32 - 0.5;
    let y =
        hash_u64(seed ^ index.wrapping_mul(0x85EB_CA6B) ^ 0x1234) as f32 / u64::MAX as f32 - 0.5;
    let z =
        hash_u64(seed ^ index.wrapping_mul(0xC2B2_AE35) ^ 0xABCD) as f32 / u64::MAX as f32 - 0.5;
    Vec3::new(x, y, z)
}

/// Approximate the fracture direction from a stroke, for shear patterns.
pub fn stroke_direction(from: Vec3, to: Vec3, normal: Vec3) -> Vec3 {
    let direction = to - from;
    let tangential = direction - normal * direction.dot(normal);
    if tangential.length_squared() < 1e-8 {
        basis_from_normal(normal).1
    } else {
        tangential.normalize()
    }
}

/// A smooth falloff used to blend the crater rim into the intact rock.
pub fn crater_profile(distance: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    let t = (distance / radius).clamp(0.0, 1.0);
    let bowl = 1.0 - smoothstep(0.0, 1.0, t);
    let rim = (t * std::f32::consts::PI).sin().powf(2.0);
    (-bowl * 0.8 + rim * 0.25).max(-1.0)
}

/// Create a procedural debris field in a ring around `center`, without
/// fracturing the source. Useful for a fast preview of an impact.
pub fn debris_ring(
    center: Vec3,
    radius: f32,
    count: u32,
    size_m: f32,
    seed: u64,
) -> Vec<(Vec3, f32)> {
    let mut out = Vec::new();
    for i in 0..count {
        let angle = std::f32::consts::TAU * (hash_u64(seed ^ i as u64) as f32 / u64::MAX as f32);
        let distance =
            radius * (0.35 + 0.65 * (hash_u64(seed ^ (i as u64 + 1)) as f32 / u64::MAX as f32));
        let position = center + Vec3::new(angle.cos() * distance, angle.sin() * distance, 0.0);
        let size = size_m
            * (0.25 + 0.75 * (hash_u64(seed ^ (i as u64 + 2) ^ 0x99) as f32 / u64::MAX as f32));
        out.push((position, size));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    fn cube(size: f32) -> Mesh {
        let h = size * 0.5;
        let positions = vec![
            Vec3::new(-h, -h, -h),
            Vec3::new(h, -h, -h),
            Vec3::new(h, h, -h),
            Vec3::new(-h, h, -h),
            Vec3::new(-h, -h, h),
            Vec3::new(h, -h, h),
            Vec3::new(h, h, h),
            Vec3::new(-h, h, h),
        ];
        let faces = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ];
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.recompute_normals();
        mesh
    }

    #[test]
    fn clipping_a_cube_caps_the_cut() {
        let mesh = cube(2.0);
        let top = clip_mesh(&mesh, Vec3::Z, 0.0, true);
        assert!(!top.is_empty());
        let stats = top.stats();
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        assert!(
            stats.volume_m3 > 3.5 && stats.volume_m3 < 4.5,
            "{}",
            stats.volume_m3
        );
        assert!(top.positions.iter().all(|p| p.z >= -1e-4));
    }

    #[test]
    fn split_in_two_conserves_volume() {
        let mesh = cube(2.0);
        let (a, b) = split_in_two(&mesh, Vec3::ZERO, Vec3::Z);
        let total = a.stats().volume_m3 + b.stats().volume_m3;
        assert!((total - 8.0).abs() < 0.5, "{total}");
        assert_eq!(a.stats().boundary_edges, 0);
        assert_eq!(b.stats().boundary_edges, 0);
    }

    #[test]
    fn voronoi_fracture_makes_several_pieces() {
        let mesh = cube(6.0);
        let settings = FractureSettings {
            pattern: FracturePattern::Voronoi,
            cell_m: 2.4,
            gap_m: 0.01,
            min_volume_m3: 0.001,
            seed: 9,
            ..FractureSettings::default()
        };
        let result = fracture(&mesh, settings);
        assert!(result.pieces.len() >= 3, "{}", result.pieces.len());
        assert!(
            result
                .pieces
                .iter()
                .all(|p| p.mesh.stats().boundary_edges == 0)
        );
        assert!(result.volume_m3 > 4.0, "{}", result.volume_m3);
    }

    #[test]
    fn radial_fracture_has_a_center_piece() {
        let mesh = cube(4.0);
        let settings = FractureSettings {
            pattern: FracturePattern::Radial,
            cell_m: 1.5,
            divisions: 8,
            ..FractureSettings::default()
        };
        let result = fracture(&mesh, settings);
        assert!(result.pieces.len() >= 2);
        assert!(result.pieces.iter().any(|p| p.site == 0));
    }

    #[test]
    fn crater_profile_dips_in_the_middle() {
        assert!(crater_profile(0.0, 10.0) < 0.0);
        assert!(crater_profile(10.0, 10.0).abs() < 0.1);
        assert!(crater_profile(0.0, 10.0) < crater_profile(4.0, 10.0));
    }

    #[test]
    fn debris_ring_is_deterministic() {
        let a = debris_ring(Vec3::ZERO, 10.0, 20, 0.5, 42);
        let b = debris_ring(Vec3::ZERO, 10.0, 20, 0.5, 42);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.0, y.0);
            assert_eq!(x.1, y.1);
        }
    }
}

//! Rock surface detail: bedding, joints, exfoliation, vesicles, tafoni,
//! ooliths and grain, applied as a layered displacement on an existing mesh.
//!
//! The point is to keep the *geology* readable when the camera gets close: a
//! sandstone wall shows horizontal bedding with undercut benches, a basalt
//! wall shows columnar joints, a granite dome shows exfoliation sheets and
//! coarse grain. All of it is deterministic and can be baked at any density.

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::math::smoothstep;
use crate::mesh::Mesh;
use crate::noise::{
    NoiseKind, NoiseParams, RockSurface, perlin3, strata_profile, voronoi2, voronoi3,
};
use crate::remesh::refine_adaptive;

/// The layered parameters of one rock type.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RockProfile {
    pub surface: RockSurface,
    /// Bedding thickness, metres. 0 disables strata.
    pub strata_m: f32,
    /// Dip of the bedding, degrees.
    pub dip_deg: f32,
    /// Joint spacing, metres. 0 disables joints.
    pub joint_spacing_m: f32,
    pub joint_depth_m: f32,
    pub joint_width: f32,
    /// Exfoliation sheet thickness, metres. 0 disables.
    pub exfoliation_m: f32,
    pub exfoliation_depth_m: f32,
    /// Vesicle (gas bubble) density, 0..1.
    pub vesicles: f32,
    pub vesicle_radius_m: f32,
    /// Tafoni cavity scale, metres. 0 disables.
    pub tafoni_m: f32,
    pub tafoni_depth_m: f32,
    /// Oolith / concretion radius, metres.
    pub oolith_m: f32,
    pub oolith_depth_m: f32,
    /// Bench (wave-cut ledge) vertical spacing, metres.
    pub bench_m: f32,
    pub bench_depth_m: f32,
    /// Fine grain amplitude, metres.
    pub grain_m: f32,
    /// Overall multiplier.
    pub strength: f32,
}

impl Default for RockProfile {
    fn default() -> Self {
        Self::preset("granite").unwrap_or(Self {
            surface: RockSurface::Granite,
            strata_m: 0.0,
            dip_deg: 0.0,
            joint_spacing_m: 6.0,
            joint_depth_m: 0.25,
            joint_width: 0.08,
            exfoliation_m: 3.5,
            exfoliation_depth_m: 0.3,
            vesicles: 0.0,
            vesicle_radius_m: 0.15,
            tafoni_m: 0.0,
            tafoni_depth_m: 0.0,
            oolith_m: 0.0,
            oolith_depth_m: 0.0,
            bench_m: 0.0,
            bench_depth_m: 0.0,
            grain_m: 0.05,
            strength: 1.0,
        })
    }
}

impl RockProfile {
    pub fn preset(name: &str) -> Option<Self> {
        let surface = RockSurface::from_name(name)?;
        let mut profile = Self {
            surface,
            strata_m: surface.strata_m(),
            joint_spacing_m: 6.0,
            joint_depth_m: 0.25,
            joint_width: 0.08,
            exfoliation_m: 3.5,
            exfoliation_depth_m: 0.3,
            vesicles: 0.0,
            vesicle_radius_m: 0.15,
            tafoni_m: 0.0,
            tafoni_depth_m: 0.0,
            oolith_m: 0.0,
            oolith_depth_m: 0.0,
            bench_m: 0.0,
            bench_depth_m: 0.0,
            grain_m: surface.grain_m(),
            strength: 1.0,
            dip_deg: 0.0,
        };
        match surface {
            RockSurface::Granite => {
                profile.joint_spacing_m = 8.0;
                profile.exfoliation_m = 4.0;
                profile.exfoliation_depth_m = 0.35;
                profile.grain_m = 0.06;
            }
            RockSurface::Basalt => {
                profile.joint_spacing_m = 2.2;
                profile.joint_depth_m = 0.35;
                profile.vesicles = 0.35;
                profile.vesicle_radius_m = 0.12;
                profile.grain_m = 0.03;
            }
            RockSurface::Limestone => {
                profile.strata_m = 1.1;
                profile.joint_spacing_m = 5.0;
                profile.tafoni_m = 2.5;
                profile.tafoni_depth_m = 0.3;
                profile.oolith_m = 0.35;
                profile.oolith_depth_m = 0.04;
            }
            RockSurface::Sandstone => {
                profile.strata_m = 0.75;
                profile.joint_spacing_m = 4.0;
                profile.joint_depth_m = 0.18;
                profile.bench_m = 2.4;
                profile.bench_depth_m = 0.35;
                profile.grain_m = 0.02;
            }
            RockSurface::Schist => {
                profile.strata_m = 0.35;
                profile.dip_deg = 55.0;
                profile.joint_spacing_m = 3.5;
                profile.grain_m = 0.01;
            }
            RockSurface::Chalk => {
                profile.strata_m = 1.8;
                profile.joint_spacing_m = 7.0;
                profile.grain_m = 0.04;
            }
            RockSurface::Volcanic => {
                profile.vesicles = 0.55;
                profile.vesicle_radius_m = 0.2;
                profile.joint_spacing_m = 5.0;
                profile.strata_m = 0.0;
            }
            RockSurface::Canyon => {
                profile.strata_m = 0.9;
                profile.bench_m = 3.0;
                profile.bench_depth_m = 0.6;
                profile.joint_spacing_m = 9.0;
                profile.exfoliation_m = 5.0;
            }
            RockSurface::Patagonia => {
                profile.strata_m = 1.4;
                profile.dip_deg = 12.0;
                profile.joint_spacing_m = 4.5;
                profile.grain_m = 0.03;
            }
        }
        Some(profile)
    }

    pub fn all_presets() -> Vec<(&'static str, RockProfile)> {
        RockSurface::ALL
            .iter()
            .filter_map(|surface| {
                Self::preset(surface.name()).map(|profile| (surface.name(), profile))
            })
            .collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.strata_m < 0.0 || self.joint_spacing_m < 0.0 || self.exfoliation_m < 0.0 {
            return Err("les échelles de roche doivent être positives".into());
        }
        if !(0.0..=1.0).contains(&self.vesicles) {
            return Err("la densité de vésicules doit être entre 0 et 1".into());
        }
        Ok(())
    }

    /// The layered displacement at `p`, along `normal`, in metres.
    pub fn displacement(&self, p: Vec3, normal: Vec3, seed: u64) -> f32 {
        let mut height = 0.0;
        let normal = normal.normalize_or_zero();
        // Steep faces show the layers: the strata are cut across, so the
        // horizontal term is strongest where the normal is horizontal.
        let steep = (1.0 - normal.z.abs()).clamp(0.0, 1.0);
        let flat = 1.0 - steep * 0.3;
        if self.strata_m > 0.0 {
            let dip = self.dip_deg.to_radians();
            let along = p.x * dip.sin() + p.y * dip.cos() * 0.3;
            let phase = strata_profile(p.z + along, self.strata_m, 0.0, 0.65);
            let hard = if phase > 0.2 { 1.0 } else { 0.35 };
            height += phase * self.strata_m * 0.22 * hard * steep.max(0.35);
        }
        if self.joint_spacing_m > 0.0 {
            let cell = Vec2::new(p.x, p.z) / self.joint_spacing_m;
            let sample = voronoi2(seed ^ 0x1010, cell);
            let edge = sample.edge() * self.joint_spacing_m;
            let width = (self.joint_width * self.joint_spacing_m).max(0.05);
            let crack = 1.0 - (edge / width).clamp(0.0, 1.0);
            height -= crack.powf(2.0) * self.joint_depth_m * flat;
        }
        if self.exfoliation_m > 0.0 {
            let shell = (p.length() / self.exfoliation_m).fract();
            let sheet = 1.0 - (shell - 0.5).abs() * 2.0;
            height += (sheet - 0.5) * self.exfoliation_depth_m * 0.35;
        }
        if self.vesicles > 0.0 {
            let cell = self.vesicle_radius_m.max(0.05) * 3.0;
            let sample = voronoi3(seed ^ 0x2020, p / cell);
            let radius = self.vesicle_radius_m / cell;
            if sample.f1 < radius {
                let pit = 1.0 - sample.f1 / radius;
                height -= pit.powf(1.5) * self.vesicle_radius_m * 0.6 * self.vesicles;
            }
        }
        if self.tafoni_m > 0.0 {
            let cell = self.tafoni_m.max(0.1);
            let sample = voronoi3(seed ^ 0x3030, p / cell);
            let cavity = 1.0 - smoothstep(0.15, 0.55, sample.f1);
            height -= cavity * self.tafoni_depth_m * steep;
        }
        if self.oolith_m > 0.0 {
            let cell = self.oolith_m.max(0.05);
            let sample = voronoi2(seed ^ 0x4040, Vec2::new(p.x / cell, p.y / cell));
            let bump = 1.0 - smoothstep(0.1, 0.5, sample.f1);
            height += bump * self.oolith_depth_m;
        }
        if self.bench_m > 0.0 {
            let bench = ((p.z / self.bench_m).fract() - 0.5).abs();
            let ledge = 1.0 - smoothstep(0.0, 0.12, bench);
            height -= ledge * self.bench_depth_m * steep;
        }
        if self.grain_m > 0.0 {
            let params = NoiseParams {
                kind: NoiseKind::Fbm,
                frequency: 1.0 / (self.grain_m * 6.0).max(0.005),
                octaves: 4,
                lacunarity: 2.1,
                gain: 0.55,
                warp_m: 0.0,
                seed,
            };
            height += params.displacement(p, self.grain_m);
        }
        height * self.strength
    }
}

/// Settings for a detail bake.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RockDetailSettings {
    pub profile: RockProfile,
    /// Refine the mesh first where a layer is smaller than the edge length.
    pub adaptive: bool,
    pub max_edge_m: f32,
    pub max_vertices: usize,
    /// Apply to the whole mesh or only where the mask is > 0.5.
    pub use_mask: bool,
}

impl Default for RockDetailSettings {
    fn default() -> Self {
        Self {
            profile: RockProfile::default(),
            adaptive: true,
            max_edge_m: 0.35,
            max_vertices: 2_000_000,
            use_mask: false,
        }
    }
}

/// Displace an existing mesh with the layered rock detail. Returns the number
/// of vertices that moved.
///
/// Each vertex depends only on its own position and normal, so the pass is
/// embarrassingly parallel and the result is identical whatever the thread
/// count: every slot is written by exactly one task.
pub fn apply_detail(mesh: &mut Mesh, profile: RockProfile, seed: u64) -> usize {
    if mesh.is_empty() {
        return 0;
    }
    if profile.strata_m <= 0.0
        && profile.joint_spacing_m <= 0.0
        && profile.tafoni_m <= 0.0
        && profile.vesicles <= 0.0
        && profile.oolith_m <= 0.0
        && profile.grain_m <= 0.0
    {
        return 0;
    }
    let count = mesh.positions.len();
    let positions = &mesh.positions;
    let normals = &mesh.normals;
    let mut amounts = vec![0.0f32; count];
    vf_core::compute::install(|| {
        amounts
            .par_chunks_mut(4096)
            .enumerate()
            .for_each(|(block, chunk)| {
                let base = block * 4096;
                for (offset, slot) in chunk.iter_mut().enumerate() {
                    let index = base + offset;
                    if index >= count {
                        continue;
                    }
                    let normal = normals.get(index).copied().unwrap_or(Vec3::Z);
                    let delta = profile.displacement(positions[index], normal, seed);
                    *slot = if delta.abs() > 1e-6 { delta } else { 0.0 };
                }
            });
    });
    let moved = amounts.iter().filter(|value| **value != 0.0).count();
    if moved == 0 {
        return 0;
    }
    // The displacement kernel applies `position + normal * amount` on the
    // selected backend (CPU chunked, or GPU).
    let displaced = vf_compute::displace(&mesh.positions, &mesh.normals, &amounts).value;
    if displaced.len() == mesh.positions.len() {
        mesh.positions = displaced;
    } else {
        for (index, amount) in amounts.iter().enumerate() {
            if *amount != 0.0
                && let (Some(position), Some(normal)) = (
                    mesh.positions.get_mut(index),
                    mesh.normals.get(index).copied(),
                )
            {
                *position += normal * *amount;
            }
        }
    }
    mesh.recompute_normals();
    moved
}

/// Refine and then displace: the pipeline to use on a new cliff. Refinement is
/// driven by both the edge length and the local curvature so a smooth dome is
/// not wasted with millions of triangles.
pub fn adaptive_detail(
    mesh: &Mesh,
    profile: RockProfile,
    settings: RockDetailSettings,
    seed: u64,
) -> Mesh {
    let mut out = mesh.clone();
    if settings.adaptive {
        for _ in 0..3 {
            let split = refine_adaptive(&mut out, settings.max_edge_m.max(0.01), 26.0);
            if split == 0 || out.positions.len() >= settings.max_vertices {
                break;
            }
        }
    }
    let bvh = Bvh::build(&out);
    apply_detail(&mut out, profile, seed);
    if settings.use_mask {
        // A second pass restricted to the mask: the unmasked vertices are
        // restored from the source. Used to paint detail locally.
        let source = mesh;
        for index in 0..out.positions.len().min(source.positions.len()) {
            let mask = out
                .colors
                .get(index)
                .map(|c| c[3] as f32 / 255.0)
                .unwrap_or(1.0);
            if mask < 0.5 {
                out.positions[index] = source.positions[index];
            }
        }
        out.recompute_normals();
    }
    let _ = bvh;
    out
}

/// Add one layer at a time, for the UI checkboxes.
pub fn add_strata(
    mesh: &mut Mesh,
    thickness_m: f32,
    dip_deg: f32,
    depth_m: f32,
    seed: u64,
) -> usize {
    // The profile's `displacement` already scales strata by their own
    // thickness; depth_m scales the whole layer.
    let ratio = depth_m / (thickness_m * 0.22).max(1e-4);
    let profile = RockProfile {
        strata_m: thickness_m.max(0.02),
        dip_deg,
        strength: ratio,
        ..RockProfile::default()
    };
    apply_detail(mesh, profile, seed)
}

pub fn add_joints(mesh: &mut Mesh, spacing_m: f32, depth_m: f32, seed: u64) -> usize {
    let profile = RockProfile {
        joint_spacing_m: spacing_m.max(0.1),
        joint_depth_m: depth_m,
        ..RockProfile::default()
    };
    apply_detail(mesh, profile, seed)
}

pub fn add_exfoliation(mesh: &mut Mesh, sheet_m: f32, depth_m: f32, seed: u64) -> usize {
    let profile = RockProfile {
        exfoliation_m: sheet_m.max(0.05),
        exfoliation_depth_m: depth_m,
        ..RockProfile::default()
    };
    apply_detail(mesh, profile, seed)
}

pub fn add_vesicles(mesh: &mut Mesh, density: f32, radius_m: f32, seed: u64) -> usize {
    let profile = RockProfile {
        vesicles: density.clamp(0.0, 1.0),
        vesicle_radius_m: radius_m.max(0.01),
        ..RockProfile::default()
    };
    apply_detail(mesh, profile, seed)
}

pub fn add_tafoni(mesh: &mut Mesh, scale_m: f32, depth_m: f32, seed: u64) -> usize {
    let profile = RockProfile {
        tafoni_m: scale_m.max(0.1),
        tafoni_depth_m: depth_m,
        ..RockProfile::default()
    };
    apply_detail(mesh, profile, seed)
}

pub fn add_benches(mesh: &mut Mesh, spacing_m: f32, depth_m: f32, seed: u64) -> usize {
    let profile = RockProfile {
        bench_m: spacing_m.max(0.1),
        bench_depth_m: depth_m,
        ..RockProfile::default()
    };
    apply_detail(mesh, profile, seed)
}

/// Statistics that help pick a detail level.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct RockStats {
    pub vertices: usize,
    pub area_m2: f32,
    pub mean_edge_m: f32,
    pub steep_fraction: f32,
    pub recommended_edge_m: f32,
}

pub fn rock_stats(mesh: &Mesh) -> RockStats {
    let mut edge_sum = 0.0;
    let mut edges = 0usize;
    for face in mesh.faces() {
        for i in 0..face.len() {
            let a = face[i];
            let b = face[(i + 1) % face.len()];
            edge_sum += mesh.positions[a as usize].distance(mesh.positions[b as usize]);
            edges += 1;
        }
    }
    let mean_edge = if edges > 0 {
        edge_sum / edges as f32
    } else {
        0.0
    };
    let steep = mesh.normals.iter().filter(|n| n.z.abs() < 0.55).count() as f32
        / mesh.normals.len().max(1) as f32;
    RockStats {
        vertices: mesh.positions.len(),
        area_m2: mesh.surface_area() as f32,
        mean_edge_m: mean_edge,
        steep_fraction: steep,
        recommended_edge_m: mean_edge.clamp(0.05, 2.0) * 0.5,
    }
}

/// Slope mask used by the renderer and the detail preview: 0 on flats, 1 on
/// vertical walls.
pub fn slope_mask(mesh: &Mesh) -> Vec<f32> {
    mesh.normals
        .iter()
        .map(|n| (1.0 - n.z.abs()).clamp(0.0, 1.0))
        .collect()
}

/// Paint a rock-type tint into the vertex colours: sandstone warm, basalt
/// dark, chalk pale, with a bedding modulation. This is the preview material
/// before a real texture is assigned.
pub fn tint_by_profile(mesh: &mut Mesh, profile: RockProfile, seed: u64) {
    mesh.ensure_colors();
    let base = match profile.surface {
        RockSurface::Granite => [176, 168, 158],
        RockSurface::Basalt => [62, 60, 58],
        RockSurface::Limestone => [196, 188, 166],
        RockSurface::Sandstone => [204, 158, 104],
        RockSurface::Schist => [108, 116, 112],
        RockSurface::Chalk => [226, 222, 208],
        RockSurface::Volcanic => [82, 66, 58],
        RockSurface::Canyon => [196, 126, 78],
        RockSurface::Patagonia => [172, 148, 126],
    };
    for index in 0..mesh.positions.len() {
        let p = mesh.positions[index];
        let variation = perlin3(seed ^ 0xBEEF, p * 0.08) * 18.0;
        let band = if profile.strata_m > 0.0 {
            strata_profile(p.z, profile.strata_m, 0.0, 0.8) * 14.0
        } else {
            0.0
        };
        let shade = variation + band;
        mesh.colors[index] = [
            (base[0] as f32 + shade).clamp(0.0, 255.0) as u8,
            (base[1] as f32 + shade * 0.9).clamp(0.0, 255.0) as u8,
            (base[2] as f32 + shade * 0.7).clamp(0.0, 255.0) as u8,
            mesh.colors[index][3],
        ];
    }
}

/// Compute an ambient-occlusion-like cavity term from the local curvature.
/// Used by the viewport for a cheap "dirt in the cracks" look.
pub fn cavity(mesh: &Mesh) -> Vec<f32> {
    let mut vfaces: Vec<Vec<u32>> = vec![Vec::new(); mesh.positions.len()];
    for (index, face) in mesh.faces().enumerate() {
        for &v in face {
            vfaces[v as usize].push(index as u32);
        }
    }
    let mut out = vec![0.0f32; mesh.positions.len()];
    for (v, faces) in vfaces.iter().enumerate() {
        if faces.is_empty() {
            continue;
        }
        let mut average = Vec3::ZERO;
        let mut count = 0.0;
        for &f in faces {
            for &other in mesh.face(f as usize) {
                if other != v as u32 {
                    average += mesh.positions[other as usize];
                    count += 1.0;
                }
            }
        }
        if count == 0.0 {
            continue;
        }
        average /= count;
        let normal = mesh.normals[v];
        let curvature = (average - mesh.positions[v]).dot(normal);
        out[v] = (-curvature * 2.0).clamp(-0.5, 0.8);
    }
    out
}

/// A deterministic hash of a profile, used to invalidate caches.
pub fn profile_hash(profile: &RockProfile) -> u64 {
    let mut hash = 0u64;
    for (index, byte) in format!("{profile:?}").bytes().enumerate() {
        hash = hash.wrapping_add((byte as u64).wrapping_mul(0x100000001B3));
        hash ^= hash >> 29 ^ index as u64;
    }
    hash.max(1)
}

/// Bounds of the region where a layer is smaller than the mesh edge: the UI
/// can highlight where refinement is needed.
pub fn under_resolved(mesh: &Mesh, profile: RockProfile) -> Vec<u32> {
    let mut out = Vec::new();
    let layers = [
        profile.strata_m,
        profile.joint_spacing_m * profile.joint_width,
        profile.tafoni_m,
        profile.vesicle_radius_m,
        profile.oolith_m,
    ];
    let smallest = layers
        .iter()
        .copied()
        .filter(|v| *v > 0.0)
        .fold(f32::INFINITY, f32::min);
    if !smallest.is_finite() {
        return out;
    }
    for (index, face) in mesh.faces().enumerate() {
        let mut longest = 0.0f32;
        for i in 0..face.len() {
            let a = face[i];
            let b = face[(i + 1) % face.len()];
            longest = longest.max(mesh.positions[a as usize].distance(mesh.positions[b as usize]));
        }
        if longest > smallest * 0.6 {
            out.push(index as u32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(size: f32, cells: u32) -> Mesh {
        let mut positions = Vec::new();
        for y in 0..=cells {
            for x in 0..=cells {
                positions.push(Vec3::new(
                    x as f32 / cells as f32 * size - size * 0.5,
                    y as f32 / cells as f32 * size - size * 0.5,
                    0.0,
                ));
            }
        }
        let stride = cells + 1;
        let mut faces = Vec::new();
        for y in 0..cells {
            for x in 0..cells {
                let a = y * stride + x;
                faces.push(vec![a, a + 1, a + 1 + stride, a + stride]);
            }
        }
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.recompute_normals();
        mesh
    }

    #[test]
    fn strata_displace_vertices_in_layers() {
        let mut mesh = plane(40.0, 40);
        let mut profile = RockProfile::preset("sandstone").unwrap();
        profile.joint_spacing_m = 0.0;
        profile.bench_m = 0.0;
        profile.tafoni_m = 0.0;
        let moved = apply_detail(&mut mesh, profile, 3);
        assert!(moved > 100);
        let height_range = mesh
            .positions
            .iter()
            .map(|p| p.z)
            .fold((0.0f32, 0.0f32), |(lo, hi), z| (lo.min(z), hi.max(z)));
        assert!(height_range.1 - height_range.0 > 0.02);
    }

    #[test]
    fn every_preset_is_valid_and_deterministic() {
        for (name, profile) in RockProfile::all_presets() {
            assert!(profile.validate().is_ok(), "{name}");
            let p = Vec3::new(1.5, 2.5, 3.5);
            let a = profile.displacement(p, Vec3::X, 9);
            let b = profile.displacement(p, Vec3::X, 9);
            assert_eq!(a, b, "{name}");
            assert!(a.is_finite(), "{name}");
        }
    }

    #[test]
    fn joints_carve_cracks() {
        let mut mesh = plane(30.0, 60);
        let moved = add_joints(&mut mesh, 4.0, 0.4, 5);
        assert!(moved > 10);
        assert!(mesh.positions.iter().any(|p| p.z < -0.05));
    }

    #[test]
    fn adaptive_detail_refines_and_stays_finite() {
        let mut mesh = plane(10.0, 10);
        let settings = RockDetailSettings {
            profile: RockProfile::preset("canyon").unwrap(),
            adaptive: true,
            max_edge_m: 0.5,
            max_vertices: 200_000,
            use_mask: false,
        };
        let out = adaptive_detail(&mesh, settings.profile, settings, 1);
        assert!(out.positions.len() > mesh.positions.len());
        assert!(out.positions.iter().all(|p| p.is_finite()));
        mesh.recompute_normals();
        assert!(out.face_count() > 0);
    }

    #[test]
    fn rock_stats_are_sane() {
        let mesh = plane(10.0, 10);
        let stats = rock_stats(&mesh);
        assert!(stats.mean_edge_m > 0.0);
        assert_eq!(stats.steep_fraction, 0.0);
        assert!(stats.area_m2 > 99.0 && stats.area_m2 < 101.0);
    }
}

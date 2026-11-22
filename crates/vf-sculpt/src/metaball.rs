//! Metaballs and smooth fusion. A field of balls and capsules is evaluated on
//! a sparse voxel grid and extracted with surface nets, which gives the
//! molten, blobby junction between two meshes that a boolean cannot produce.
//!
//! `blend_meshes` is the "weld two rocks together" operation: each mesh is
//! voxelised, the fields are combined with a smooth minimum and the result is
//! extracted as one watertight surface. Set `smooth_k` to 0 for a hard union.

use glam::Vec3;

use crate::math::{Aabb, smoothstep};
use crate::mesh::Mesh;
use crate::voxel::{Grid, VoxelParams, from_field, smin, surface_nets, taubin_smooth, voxelize};

/// A single metaball.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetaBall {
    pub center: Vec3,
    pub radius: f32,
    /// 0..1, scales the effective radius and the blending strength.
    pub strength: f32,
}

impl MetaBall {
    pub fn new(center: Vec3, radius: f32) -> Self {
        Self {
            center,
            radius: radius.max(1e-3),
            strength: 1.0,
        }
    }

    pub fn field(&self, p: Vec3) -> f32 {
        let radius = self.radius * (0.55 + 0.45 * self.strength.clamp(0.0, 1.0));
        (p - self.center).length() - radius
    }
}

/// A capsule metaball, for blobs stretched between two points.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetaCapsule {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
    pub strength: f32,
}

impl MetaCapsule {
    pub fn new(a: Vec3, b: Vec3, radius: f32) -> Self {
        Self {
            a,
            b,
            radius: radius.max(1e-3),
            strength: 1.0,
        }
    }

    pub fn field(&self, p: Vec3) -> f32 {
        let ab = self.b - self.a;
        let length_sq = ab.length_squared();
        let t = if length_sq > 1e-12 {
            ((p - self.a).dot(ab) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let closest = self.a + ab * t;
        let radius = self.radius * (0.55 + 0.45 * self.strength.clamp(0.0, 1.0));
        (p - closest).length() - radius
    }
}

/// A fusion field: balls, capsules and an optional smooth radius.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MetaField {
    pub balls: Vec<MetaBall>,
    pub capsules: Vec<MetaCapsule>,
    /// Smooth-union radius, metres. 0 gives hard unions.
    pub smooth_k: f32,
    /// Extra radius added to every primitive, for a molten look.
    pub inflation: f32,
}

impl MetaField {
    pub fn new(smooth_k: f32) -> Self {
        Self {
            smooth_k: smooth_k.max(0.0),
            ..Self::default()
        }
    }

    pub fn add_ball(&mut self, center: Vec3, radius: f32) {
        self.balls.push(MetaBall::new(center, radius));
    }

    pub fn add_capsule(&mut self, a: Vec3, b: Vec3, radius: f32) {
        self.capsules.push(MetaCapsule::new(a, b, radius));
    }

    /// A capsule between two points with a thick middle, the classic "blob
    /// bridge" between two meshes.
    pub fn blob_between(&mut self, a: Vec3, b: Vec3, radius: f32) {
        self.add_capsule(a, b, radius);
        let middle = (a + b) * 0.5;
        self.add_ball(middle, radius * 1.15);
    }

    pub fn is_empty(&self) -> bool {
        self.balls.is_empty() && self.capsules.is_empty()
    }

    pub fn field(&self, p: Vec3) -> f32 {
        let mut distance = f32::INFINITY;
        for ball in &self.balls {
            let d = ball.field(p) - self.inflation;
            distance = if distance.is_infinite() {
                d
            } else {
                smin(distance, d, self.smooth_k)
            };
        }
        for capsule in &self.capsules {
            let d = capsule.field(p) - self.inflation;
            distance = if distance.is_infinite() {
                d
            } else {
                smin(distance, d, self.smooth_k)
            };
        }
        distance
    }

    pub fn bounds(&self) -> Aabb {
        let mut bounds = Aabb::EMPTY;
        for ball in &self.balls {
            let radius = ball.radius * (0.55 + 0.45 * ball.strength.clamp(0.0, 1.0))
                + self.smooth_k
                + self.inflation;
            bounds = bounds
                .with_point(ball.center - Vec3::splat(radius))
                .with_point(ball.center + Vec3::splat(radius));
        }
        for capsule in &self.capsules {
            let radius = capsule.radius * (0.55 + 0.45 * capsule.strength.clamp(0.0, 1.0))
                + self.smooth_k
                + self.inflation;
            bounds = bounds
                .with_point(capsule.a - Vec3::splat(radius))
                .with_point(capsule.b + Vec3::splat(radius));
        }
        bounds
    }

    /// Extract the field as a mesh.
    pub fn to_mesh(&self, cell_m: f32, max_voxels: usize) -> Mesh {
        let bounds = self.bounds();
        if bounds.is_empty() {
            return Mesh::new();
        }
        let grid = from_field(&bounds, cell_m, max_voxels, |p| self.field(p));
        let mut mesh = surface_nets(&grid, 2);
        if !mesh.is_empty() {
            taubin_smooth(&mut mesh, 1, 0.5, -0.53);
            mesh.box_project_uvs(1.0);
            mesh.recompute_normals();
        }
        mesh
    }
}

/// Settings for the fusion operators.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FusionSettings {
    pub cell_m: f32,
    pub smooth_k: f32,
    pub max_voxels: usize,
    pub smooth_iterations: u32,
}

impl Default for FusionSettings {
    fn default() -> Self {
        Self {
            cell_m: 0.25,
            smooth_k: 0.4,
            max_voxels: 32_000_000,
            smooth_iterations: 1,
        }
    }
}

impl FusionSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.cell_m <= 0.0 {
            return Err("la taille de voxel doit être positive".into());
        }
        if self.smooth_k < 0.0 {
            return Err("le rayon de fusion doit être positif".into());
        }
        Ok(())
    }
}

/// Voxelise several meshes on a common grid and blend them with a smooth
/// union. The result is one closed surface, so the seam really disappears.
pub fn blend_meshes(meshes: &[&Mesh], settings: FusionSettings) -> Mesh {
    let meshes: Vec<&Mesh> = meshes.iter().copied().filter(|m| !m.is_empty()).collect();
    if meshes.is_empty() {
        return Mesh::new();
    }
    if meshes.len() == 1 {
        return meshes[0].clone();
    }
    let mut bounds = Aabb::EMPTY;
    for mesh in &meshes {
        bounds = bounds.union(mesh.bounds());
    }
    if bounds.is_empty() {
        return Mesh::new();
    }
    let params = VoxelParams {
        cell_m: settings.cell_m,
        ..VoxelParams::default()
    };
    let cell = params.fitted_cell(&bounds);
    let mut grid = Grid::new(&bounds, cell, 3);
    let mut sources = Vec::with_capacity(meshes.len());
    for mesh in &meshes {
        let bvh = crate::bvh::Bvh::build(mesh);
        sources.push(voxelize(mesh, &bvh, params));
    }
    for z in 0..grid.dims[2] {
        for y in 0..grid.dims[1] {
            for x in 0..grid.dims[0] {
                let index = grid.index(x, y, z);
                let p = grid.point(x, y, z);
                let mut distance = f32::INFINITY;
                for source in &sources {
                    let sampled = source.sample(p);
                    distance = if distance.is_infinite() {
                        sampled
                    } else {
                        smin(distance, sampled, settings.smooth_k)
                    };
                }
                grid.values[index] = distance;
                grid.state[index] = 1;
            }
        }
    }
    let mut out = surface_nets(&grid, 2);
    if !out.is_empty() {
        for _ in 0..settings.smooth_iterations {
            taubin_smooth(&mut out, 1, 0.5, -0.53);
        }
        out.box_project_uvs(1.0);
        out.recompute_normals();
    }
    out
}

/// Add a blob to an existing mesh and weld it on. Used by the clay and lava
/// tools when the gesture is slow enough to fuse.
pub fn fuse_blob(mesh: &Mesh, center: Vec3, radius: f32, settings: FusionSettings) -> Mesh {
    let mut field = MetaField::new(settings.smooth_k);
    field.add_ball(center, radius);
    fuse_field(mesh, &field, settings)
}

/// Fuse a metaball field with an existing mesh, keeping the mesh detail where
/// the field does not reach.
pub fn fuse_field(mesh: &Mesh, field: &MetaField, settings: FusionSettings) -> Mesh {
    if mesh.is_empty() || field.is_empty() {
        return mesh.clone();
    }
    let bounds = mesh.bounds().union(field.bounds());
    if bounds.is_empty() {
        return mesh.clone();
    }
    let params = VoxelParams {
        cell_m: settings.cell_m,
        ..VoxelParams::default()
    };
    let cell = params.fitted_cell(&bounds);
    let bvh = crate::bvh::Bvh::build(mesh);
    let source = voxelize(mesh, &bvh, params);
    let mut grid = Grid::new(&bounds, cell, 3);
    for z in 0..grid.dims[2] {
        for y in 0..grid.dims[1] {
            for x in 0..grid.dims[0] {
                let index = grid.index(x, y, z);
                let p = grid.point(x, y, z);
                let mesh_distance = source.sample(p);
                let blob_distance = field.field(p);
                grid.values[index] = smin(mesh_distance, blob_distance, settings.smooth_k);
                grid.state[index] = 1;
            }
        }
    }
    let mut out = surface_nets(&grid, 2);
    if !out.is_empty() {
        for _ in 0..settings.smooth_iterations {
            taubin_smooth(&mut out, 1, 0.5, -0.53);
        }
        out.box_project_uvs(1.0);
        out.recompute_normals();
    }
    out
}

/// Drop a row of blobs along a path, returning the fused mesh. This is the
/// lava tongue / melted rock operator.
pub fn fuse_path(mesh: &Mesh, path: &[Vec3], radius: f32, settings: FusionSettings) -> Mesh {
    if path.len() < 2 {
        return mesh.clone();
    }
    let mut field = MetaField::new(settings.smooth_k);
    for pair in path.windows(2) {
        field.add_capsule(pair[0], pair[1], radius);
    }
    for &point in path {
        field.add_ball(point, radius * 0.9);
    }
    fuse_field(mesh, &field, settings)
}

/// A "blob bridge" between two meshes: both are voxelised, then a capsule of
/// molten rock is fused across the gap.
pub fn bridge_meshes(a: &Mesh, b: &Mesh, radius: f32, settings: FusionSettings) -> Mesh {
    let direction = b.bounds().center() - a.bounds().center();
    let bridge = MetaCapsule::new(a.bounds().center(), b.bounds().center(), radius);
    let mut field = MetaField::new(settings.smooth_k);
    field.capsules.push(bridge);
    let mut merged = blend_meshes(&[a, b], settings);
    if merged.is_empty() {
        return merged;
    }
    let _ = direction;
    let fused = fuse_field(&merged, &field, settings);
    if fused.is_empty() {
        merged
    } else {
        merged = fused;
        merged
    }
}

/// Compute a vertex "blobbiness" for the paint view: how much a vertex belongs
/// to a fused junction rather than to the original surface.
pub fn fusion_mask(mesh: &Mesh, field: &MetaField) -> Vec<f32> {
    mesh.positions
        .iter()
        .map(|&p| {
            let distance = field.field(p);
            smoothstep(0.0, 0.5, -distance)
        })
        .collect()
}

/// Build a line of metaballs along a stroke, with spacing proportional to the
/// radius. Public so the UI can preview the field before committing.
pub fn stroke_balls(path: &[Vec3], radius: f32, spacing: f32) -> Vec<MetaBall> {
    let mut balls = Vec::new();
    let spacing = spacing.max(radius * 0.25).max(0.01);
    for pair in path.windows(2) {
        let distance = pair[0].distance(pair[1]);
        let steps = (distance / spacing).ceil() as usize;
        for step in 0..=steps {
            let t = step as f32 / steps.max(1) as f32;
            balls.push(MetaBall::new(pair[0].lerp(pair[1], t), radius));
        }
    }
    balls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_ball_is_a_sphere() {
        let mut field = MetaField::new(0.0);
        field.add_ball(Vec3::ZERO, 1.0);
        let mesh = field.to_mesh(0.12, 8_000_000);
        assert!(!mesh.is_empty());
        let stats = mesh.stats();
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        let radius = mesh
            .positions
            .iter()
            .map(|p| p.length())
            .fold(0.0f32, f32::max);
        assert!((radius - 1.0).abs() < 0.15, "{radius}");
        assert!(stats.volume_m3 > 3.0, "{}", stats.volume_m3);
    }

    #[test]
    fn two_balls_fuse_with_a_smooth_neck() {
        let mut field = MetaField::new(0.9);
        field.add_ball(Vec3::new(-1.2, 0.0, 0.0), 1.0);
        field.add_ball(Vec3::new(1.2, 0.0, 0.0), 1.0);
        let mesh = field.to_mesh(0.1, 8_000_000);
        let stats = mesh.stats();
        assert_eq!(stats.components, 1, "{stats:?}");
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        // The neck is thicker than two disjoint spheres would allow.
        let neck = mesh
            .positions
            .iter()
            .filter(|p| p.x.abs() < 0.2)
            .map(|p| p.y.abs().max(p.z.abs()))
            .fold(0.0f32, f32::max);
        assert!(neck > 0.05, "{neck}");
    }

    #[test]
    fn blending_two_cubes_makes_one_component() {
        let cube = |offset: f32| {
            let positions = vec![
                Vec3::new(-1.0 + offset, -1.0, -1.0),
                Vec3::new(1.0 + offset, -1.0, -1.0),
                Vec3::new(1.0 + offset, 1.0, -1.0),
                Vec3::new(-1.0 + offset, 1.0, -1.0),
                Vec3::new(-1.0 + offset, -1.0, 1.0),
                Vec3::new(1.0 + offset, -1.0, 1.0),
                Vec3::new(1.0 + offset, 1.0, 1.0),
                Vec3::new(-1.0 + offset, 1.0, 1.0),
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
        };
        let a = cube(0.0);
        let b = cube(1.2);
        let settings = FusionSettings {
            cell_m: 0.2,
            smooth_k: 0.6,
            ..FusionSettings::default()
        };
        let mesh = blend_meshes(&[&a, &b], settings);
        assert!(!mesh.is_empty());
        assert_eq!(mesh.stats().components, 1);
    }

    #[test]
    fn stroke_balls_follow_the_path() {
        let balls = stroke_balls(&[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)], 0.5, 1.0);
        assert!(balls.len() >= 5);
        assert!(balls.first().unwrap().center.distance(Vec3::ZERO) < 1e-6);
        assert!(
            balls
                .last()
                .unwrap()
                .center
                .distance(Vec3::new(4.0, 0.0, 0.0))
                < 1e-6
        );
    }
}

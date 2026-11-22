//! The sculpt document: a mesh, its BVH, the undo stack, the active brush and
//! the host budget. Everything the UI edits goes through here, so the same
//! code paths serve the window, the CLI and the MCP tools.

use std::time::Instant;

use glam::Vec3;

use crate::bvh::Bvh;
use crate::cliff::{CliffSettings, wall};
use crate::fracture::{FractureResult, FractureSettings, fracture};
use crate::mesh::{Mesh, MeshStats, RenderMesh};
use crate::physics::{DebrisBody, PhysicsSettings, PhysicsWorld};
use crate::remesh::{RemeshMode, RemeshReport, RemeshSettings, remesh_with_source};
use crate::rock::{RockProfile, apply_detail};
use crate::sculpt::{BrushSettings, MeshUndo, SculptStroke, Symmetry, ToolKind};
use crate::terrain::{TerrainMeshSettings, from_heightfield};

/// How much detail the document is allowed to use. The presets scale the
/// voxel grid and the remesh tolerance to the host, no other compromise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum QualityLevel {
    Preview,
    #[default]
    Balanced,
    High,
    Ultra,
    Custom,
}

impl QualityLevel {
    pub const ALL: [QualityLevel; 5] = [
        QualityLevel::Preview,
        QualityLevel::Balanced,
        QualityLevel::High,
        QualityLevel::Ultra,
        QualityLevel::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QualityLevel::Preview => "Aperçu",
            QualityLevel::Balanced => "Équilibré",
            QualityLevel::High => "Élevé",
            QualityLevel::Ultra => "Ultra",
            QualityLevel::Custom => "Personnalisé",
        }
    }

    /// Voxel size used by remesh, boolean and fracture, metres.
    pub fn voxel_m(self) -> f32 {
        match self {
            QualityLevel::Preview => 0.8,
            QualityLevel::Balanced => 0.45,
            QualityLevel::High => 0.28,
            QualityLevel::Ultra => 0.16,
            QualityLevel::Custom => 0.35,
        }
    }

    pub fn max_voxels(self) -> usize {
        match self {
            QualityLevel::Preview => 2_000_000,
            QualityLevel::Balanced => 16_000_000,
            QualityLevel::High => 64_000_000,
            QualityLevel::Ultra => 256_000_000,
            QualityLevel::Custom => 64_000_000,
        }
    }

    /// Face budget for an isotropic remesh. Kept below the voxel budgets so a
    /// click in the UI stays in the seconds range instead of minutes.
    pub fn max_faces(self) -> usize {
        match self {
            QualityLevel::Preview => 120_000,
            QualityLevel::Balanced => 300_000,
            QualityLevel::High => 800_000,
            QualityLevel::Ultra => 2_000_000,
            QualityLevel::Custom => 600_000,
        }
    }

    /// Target edge length after an isotropic remesh, metres.
    pub fn remesh_edge_m(self) -> f32 {
        match self {
            QualityLevel::Preview => 1.4,
            QualityLevel::Balanced => 0.65,
            QualityLevel::High => 0.38,
            QualityLevel::Ultra => 0.2,
            QualityLevel::Custom => 0.5,
        }
    }

    /// Rough voxel memory (values + state + labels), bytes.
    pub fn estimated_voxel_bytes(self) -> u64 {
        self.max_voxels() as u64 * 9
    }

    /// Pick the best preset that fits in `available_mb` of free RAM, keeping
    /// one third of it for the mesh, the BVH and the UI.
    pub fn fit(available_mb: u64) -> Self {
        let budget = available_mb.saturating_sub(512) / 3;
        if budget >= 1800 {
            QualityLevel::Ultra
        } else if budget >= 450 {
            QualityLevel::High
        } else if budget >= 120 {
            QualityLevel::Balanced
        } else {
            QualityLevel::Preview
        }
    }

    pub fn voxel_budget_fits(self, available_mb: u64) -> bool {
        self.estimated_voxel_bytes() / (1024 * 1024) < available_mb / 3
    }
}

/// Memory and topology report shown in the status bar.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct SculptStats {
    pub vertices: usize,
    pub faces: usize,
    pub triangles: usize,
    pub quads: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub components: usize,
    pub watertight: bool,
    pub area_m2: f32,
    pub volume_m3: f32,
    pub memory_bytes: u64,
    pub undo_steps: usize,
    pub debris: usize,
}

impl SculptStats {
    pub fn from_mesh(mesh: &Mesh, undo_steps: usize, debris: usize) -> Self {
        let stats: MeshStats = mesh.stats();
        Self {
            vertices: stats.vertices,
            faces: stats.faces,
            triangles: stats.triangles,
            quads: stats.quads,
            boundary_edges: stats.boundary_edges,
            non_manifold_edges: stats.non_manifold_edges,
            components: stats.components,
            watertight: stats.watertight,
            area_m2: stats.area_m2 as f32,
            volume_m3: stats.volume_m3 as f32,
            memory_bytes: mesh.memory_bytes() as u64,
            undo_steps,
            debris,
        }
    }

    pub fn summary_fr(&self) -> String {
        format!(
            "{} sommets · {} faces · {:.1} m² · {:.1} m³ · {}",
            self.vertices,
            self.faces,
            self.area_m2,
            self.volume_m3,
            if self.watertight {
                "étanche"
            } else {
                "ouvert"
            }
        )
    }
}

/// A sculpt session.
pub struct SculptDocument {
    pub name: String,
    pub mesh: Mesh,
    pub bvh: Bvh,
    pub brush: BrushSettings,
    pub rock: RockProfile,
    pub quality: QualityLevel,
    pub symmetry: Symmetry,
    pub dirty: bool,
    /// Monotonic geometry revision, used by the UI to cache expensive
    /// reports without recomputing them every frame.
    revision: u64,
    /// Separate revision for the expensive topology report: it only changes
    /// when the stroke ends or the topology is replaced, not on every stamp.
    stats_revision: u64,
    undo: Vec<MeshUndo>,
    redo: Vec<MeshUndo>,
    stroke: Option<SculptStroke>,
    pub debris: Vec<DebrisBody>,
    pub last_error: Option<String>,
}

impl SculptDocument {
    pub fn new(name: impl Into<String>, mesh: Mesh) -> Self {
        let bvh = Bvh::build(&mesh);
        Self {
            name: name.into(),
            mesh,
            bvh,
            brush: BrushSettings::default(),
            rock: RockProfile::default(),
            quality: QualityLevel::Balanced,
            symmetry: Symmetry::None,
            dirty: false,
            revision: 0,
            stats_revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            stroke: None,
            debris: Vec::new(),
            last_error: None,
        }
    }

    pub fn empty() -> Self {
        Self::new("Sculpture", Mesh::new())
    }

    pub fn from_heightfield(hf: &vf_core::HeightField, settings: TerrainMeshSettings) -> Self {
        Self::new("Terrain sculpté", from_heightfield(hf, settings))
    }

    pub fn from_wall(settings: CliffSettings) -> Self {
        Self::new("Paroi", wall(settings))
    }

    pub fn rebuild_bvh(&mut self) {
        self.bvh = Bvh::build(&self.mesh);
        self.touch();
        self.touch_stats();
    }

    /// Mark the geometry as changed. Direct edits to `mesh` should call this so
    /// cached statistics and renders are invalidated.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }

    fn touch_stats(&mut self) {
        self.stats_revision = self.stats_revision.wrapping_add(1);
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn stats_revision(&self) -> u64 {
        self.stats_revision
    }

    pub fn stats(&self) -> SculptStats {
        SculptStats::from_mesh(
            &self.mesh,
            self.undo.len() + self.redo.len(),
            self.debris.len(),
        )
    }

    pub fn render_mesh(&self, smoothing_angle_deg: f32) -> RenderMesh {
        RenderMesh::from_mesh(&self.mesh, smoothing_angle_deg)
    }

    /// Cheap render mesh for orbiting and live strokes.
    pub fn render_mesh_fast(&self) -> RenderMesh {
        RenderMesh::from_mesh_fast(&self.mesh)
    }

    pub fn is_empty(&self) -> bool {
        self.mesh.is_empty()
    }

    /// Replace the geometry and record a full undo entry.
    pub fn replace_mesh(&mut self, mesh: Mesh) {
        let before = self.mesh.clone();
        self.mesh = mesh;
        self.rebuild_bvh();
        self.undo.push(MeshUndo::Topology {
            before: Box::new(before),
            after: Box::new(self.mesh.clone()),
        });
        self.redo.clear();
        self.touch();
    }

    /// Begin a sculpt stroke with the active brush.
    pub fn stroke_begin(&mut self, seed: u64) -> bool {
        if self.mesh.is_empty() {
            return false;
        }
        self.brush.symmetry = self.symmetry;
        self.stroke = Some(SculptStroke::begin(&self.mesh, self.brush, seed));
        true
    }

    /// Stamp the current stroke. Returns true when vertices moved.
    pub fn stroke_stamp(&mut self, center: Vec3, normal: Vec3) -> bool {
        let Some(mut stroke) = self.stroke.take() else {
            return false;
        };
        let moved = stroke.stamp(&mut self.mesh, &mut self.bvh, center, normal);
        self.stroke = Some(stroke);
        if moved {
            self.touch();
        }
        moved
    }

    /// Finish the stroke and push one undo entry for the whole gesture.
    pub fn stroke_end(&mut self) -> bool {
        let Some(stroke) = self.stroke.take() else {
            return false;
        };
        if let Some(undo) = stroke.finish(&self.mesh) {
            self.undo.push(undo);
            self.redo.clear();
            self.touch();
            self.touch_stats();
            true
        } else {
            false
        }
    }

    /// One-shot stamp, for the CLI and the MCP tools.
    pub fn apply_stamp(&mut self, center: Vec3, normal: Vec3, seed: u64) -> bool {
        if !self.stroke_begin(seed) {
            return false;
        }
        self.stroke_stamp(center, normal);
        self.stroke_end()
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        entry.undo(&mut self.mesh);
        self.redo.push(entry);
        self.rebuild_bvh();
        self.touch();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        entry.redo(&mut self.mesh);
        self.undo.push(entry);
        self.rebuild_bvh();
        self.touch();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear_undo(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.touch_stats();
    }

    /// Remesh the whole document. The source BVH keeps the detail when the
    /// target edge is smaller than the current triangles.
    pub fn remesh(&mut self, mode: RemeshMode, target_edge_m: f32) -> Option<RemeshReport> {
        if self.mesh.is_empty() {
            return None;
        }
        let settings = RemeshSettings {
            mode,
            target_edge_m: target_edge_m.max(0.01),
            max_faces: self.quality.max_faces(),
            ..RemeshSettings::default()
        };
        let source = self.mesh.clone();
        let source_bvh = Bvh::build(&source);
        let start = Instant::now();
        let new_mesh = remesh_with_source(&source, settings, Some((&source, &source_bvh)));
        let report = RemeshReport {
            mode: settings.mode,
            before: source.stats(),
            after: new_mesh.stats(),
            target_edge_m: settings.target_edge_m,
            elapsed_ms: start.elapsed().as_millis(),
        };
        let before = self.mesh.clone();
        self.mesh = new_mesh;
        self.rebuild_bvh();
        self.undo.push(MeshUndo::Topology {
            before: Box::new(before),
            after: Box::new(self.mesh.clone()),
        });
        self.redo.clear();
        self.touch();
        Some(report)
    }

    /// Voxel remesh at the document quality.
    pub fn voxel_remesh(&mut self) -> bool {
        let before = self.mesh.clone();
        let params = crate::voxel::VoxelParams {
            cell_m: self.quality.voxel_m(),
            max_voxels: self.quality.max_voxels(),
            ..crate::voxel::VoxelParams::default()
        };
        let new_mesh = crate::voxel::voxel_remesh(&self.mesh, &self.bvh, params, true);
        if new_mesh.is_empty() {
            return false;
        }
        self.mesh = new_mesh;
        self.rebuild_bvh();
        self.undo.push(MeshUndo::Topology {
            before: Box::new(before),
            after: Box::new(self.mesh.clone()),
        });
        self.redo.clear();
        self.touch();
        true
    }

    /// Fracture the current mesh into pieces without destroying the document.
    pub fn fracture(&self, settings: FractureSettings) -> FractureResult {
        fracture(&self.mesh, settings)
    }

    /// Apply the active rock detail to the whole mesh.
    pub fn apply_rock_detail(&mut self, seed: u64) -> usize {
        let before = self.mesh.clone();
        let moved = apply_detail(&mut self.mesh, self.rock, seed);
        if moved > 0 {
            self.rebuild_bvh();
            self.undo.push(MeshUndo::Topology {
                before: Box::new(before),
                after: Box::new(self.mesh.clone()),
            });
            self.redo.clear();
            self.touch();
        }
        moved
    }

    pub fn set_quality(&mut self, quality: QualityLevel) {
        self.quality = quality;
    }

    /// Check that the configured quality fits in `available_mb`; if not,
    /// return the preset that does.
    pub fn fit_quality(&self, available_mb: u64) -> QualityLevel {
        if self.quality.voxel_budget_fits(available_mb) {
            self.quality
        } else {
            QualityLevel::fit(available_mb)
        }
    }

    /// Add a generation of debris pieces to the document's debris list.
    pub fn add_debris(&mut self, pieces: &[crate::fracture::FracturePiece]) {
        for piece in pieces {
            self.debris.push(DebrisBody::from_piece(piece));
        }
        self.touch_stats();
    }

    /// Simulate the debris onto the current mesh as ground.
    pub fn settle_debris(
        &mut self,
        settings: PhysicsSettings,
        dt: f32,
    ) -> crate::physics::PhysicsReport {
        let ground = self.mesh.clone();
        let bvh = Bvh::build(&ground);
        let mut world = PhysicsWorld::new(settings);
        for body in self.debris.drain(..) {
            world.add(body);
        }
        let report = world.settle(Some((&ground, &bvh)), dt);
        self.debris = world.bodies;
        report
    }

    pub fn debris_mesh(&self) -> Mesh {
        let mut out = Mesh::new();
        for body in &self.debris {
            let mesh = body.world_mesh();
            out.merge(&mesh);
        }
        out
    }

    pub fn memory_bytes(&self) -> u64 {
        self.mesh.memory_bytes() as u64
            + self.bvh.node_count() as u64 * 32
            + self.bvh.triangle_count() as u64 * 12
            + self
                .debris
                .iter()
                .map(|b| b.mesh.memory_bytes() as u64)
                .sum::<u64>()
    }
}

/// A quick-start scene for the UI: a terrain mesh with a cliff wall and a
/// couple of rock types, so the 3D workspace opens on something to sculpt.
pub fn demo_document(hf: &vf_core::HeightField) -> SculptDocument {
    let mut document = SculptDocument::from_heightfield(
        hf,
        TerrainMeshSettings {
            resolution: 96,
            close_slab: true,
            floor_z: -12.0,
            uv_scale_m: 8.0,
            refine_steep: false,
            max_edge_m: 0.8,
            max_vertices: 400_000,
        },
    );
    document.name = "Démo 3D".into();
    document.brush.tool = ToolKind::Clay;
    document.brush.radius_m = 6.0;
    document.rock = RockProfile::preset("sandstone").unwrap_or_default();
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Aabb;
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
    fn a_stroke_is_one_undo_step() {
        let mut document = SculptDocument::new("test", cube(4.0));
        document.brush.tool = ToolKind::Clay;
        document.brush.radius_m = 3.0;
        document.brush.strength = 1.0;
        assert!(document.stroke_begin(1));
        document.stroke_stamp(Vec3::new(0.0, 0.0, 2.0), Vec3::Z);
        document.stroke_stamp(Vec3::new(0.1, 0.0, 2.0), Vec3::Z);
        assert!(document.stroke_end());
        assert_eq!(document.undo.len(), 1);
        let before = document.mesh.positions.clone();
        assert!(document.undo());
        assert!(document.mesh.positions != before);
        assert!(document.redo());
        assert_eq!(document.mesh.positions, before);
    }

    #[test]
    fn quality_presets_scale_with_the_host() {
        assert_eq!(QualityLevel::fit(64_000), QualityLevel::Ultra);
        assert_eq!(QualityLevel::fit(4096), QualityLevel::High);
        assert_eq!(QualityLevel::fit(1024), QualityLevel::Balanced);
        assert_eq!(QualityLevel::fit(256), QualityLevel::Preview);
        assert!(
            QualityLevel::Ultra.estimated_voxel_bytes()
                > QualityLevel::Preview.estimated_voxel_bytes()
        );
    }

    #[test]
    fn remesh_replaces_and_can_be_undone() {
        let mut document = SculptDocument::new("test", cube(6.0));
        let before = document.mesh.stats().vertices;
        let report = document.remesh(RemeshMode::Quads, 0.7).unwrap();
        assert!(report.after.faces > 100, "{report:?}");
        assert!(document.mesh.stats().vertices > before);
        assert!(document.undo());
        assert_eq!(document.mesh.stats().vertices, before);
    }

    #[test]
    fn stats_report_memory_and_extent() {
        let document = SculptDocument::new("test", cube(2.0));
        let stats = document.stats();
        assert_eq!(stats.vertices, 8);
        assert!(stats.memory_bytes > 0);
        assert!(stats.watertight);
        assert!(stats.summary_fr().contains("sommets"));
    }

    #[test]
    fn a_wall_document_is_valid() {
        let document = SculptDocument::from_wall(CliffSettings {
            segments: 12,
            rows: 8,
            height_m: 8.0,
            length_m: 20.0,
            ..CliffSettings::default()
        });
        assert!(!document.is_empty());
        assert!(document.bvh.bounds().size().z > 7.0);
        let _ = Aabb::from_center_size(Vec3::ZERO, Vec3::ONE);
    }
}

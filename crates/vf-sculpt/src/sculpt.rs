//! Hand sculpting tools that a heightfield could never offer: hammer blows,
//! flat and round chisels, files, clay build-up, undercuts, lava melt, impact
//! craters, fracture networks and mask painting.
//!
//! Every tool is a *stamp*: a small, deterministic deformation of the
//! vertices inside a sphere, followed by a local normal update and a BVH
//! refit. A stroke chains stamps along the cursor path, and an undo record
//! stores only the vertices that actually moved, so a 2 M vertex cliff can be
//! sculpted for hours without exploding memory.
//!
//! Masks live in the alpha channel of the vertex colours: a vertex with alpha
//! 0 ignores every tool except the mask brush itself. That keeps the mesh
//! format unchanged and survives every topology operation that already
//! carries colours.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use rayon::prelude::*;
use vf_compute::{FaceGraph, VertexFaces};

use crate::bvh::Bvh;
use crate::math::{Aabb, Falloff, basis_from_normal, smoothstep};
use crate::mesh::Mesh;
use crate::noise::{RockSurface, perlin3, strata_profile, voronoi2, voronoi3};

/// A sculpting tool. The first group removes material, the second adds it,
/// the third smooths and the last two paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Hammer,
    Chisel,
    Gouge,
    Strata,
    File,
    Clay,
    Crease,
    Pinch,
    Inflate,
    Flatten,
    Undercut,
    Melt,
    Impact,
    Fracture,
    Detail,
    Smooth,
    Paint,
    Mask,
    Material,
}

impl ToolKind {
    pub const ALL: [ToolKind; 19] = [
        ToolKind::Hammer,
        ToolKind::Chisel,
        ToolKind::Gouge,
        ToolKind::Strata,
        ToolKind::File,
        ToolKind::Clay,
        ToolKind::Crease,
        ToolKind::Pinch,
        ToolKind::Inflate,
        ToolKind::Flatten,
        ToolKind::Undercut,
        ToolKind::Melt,
        ToolKind::Impact,
        ToolKind::Fracture,
        ToolKind::Detail,
        ToolKind::Smooth,
        ToolKind::Paint,
        ToolKind::Mask,
        ToolKind::Material,
    ];

    /// French label used by the UI.
    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Hammer => "Marteau",
            ToolKind::Chisel => "Burin plat",
            ToolKind::Gouge => "Gouge",
            ToolKind::Strata => "Burin à strates",
            ToolKind::File => "Lime",
            ToolKind::Clay => "Argile (ajout)",
            ToolKind::Crease => "Sillon / arête",
            ToolKind::Pinch => "Pincement",
            ToolKind::Inflate => "Gonflement",
            ToolKind::Flatten => "Aplatir",
            ToolKind::Undercut => "Surplomb",
            ToolKind::Melt => "Fusion / lave",
            ToolKind::Impact => "Impact",
            ToolKind::Fracture => "Fracture",
            ToolKind::Detail => "Grain de roche",
            ToolKind::Smooth => "Lisser",
            ToolKind::Paint => "Peindre",
            ToolKind::Mask => "Masque",
            ToolKind::Material => "Matériau",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ToolKind::Hammer => "Coups secs, enlève de la matière avec un bord irrégulier",
            ToolKind::Chisel => "Taille plate orientée, arêtes nettes",
            ToolKind::Gouge => "Rainure arrondie, copeau large",
            ToolKind::Strata => "Burin guidé par les couches géologiques",
            ToolKind::File => "Abrase et raye dans le sens du geste",
            ToolKind::Clay => "Ajoute de la matière, empilement progressif",
            ToolKind::Crease => "Creuse un sillon ou lève une arête",
            ToolKind::Pinch => "Rapproche la matière du centre",
            ToolKind::Inflate => "Gonfle la surface le long de la normale",
            ToolKind::Flatten => "Projette la zone sur un plan",
            ToolKind::Undercut => "Tire une lèvre horizontale, crée un surplomb",
            ToolKind::Melt => "Fond et fait couler la roche, fusion douce",
            ToolKind::Impact => "Cratère avec bourrelet et fissures radiales",
            ToolKind::Fracture => "Réseau de fissures paramétrable",
            ToolKind::Detail => "Ajoute le grain d'un type de roche",
            ToolKind::Smooth => "Lissage local",
            ToolKind::Paint => "Peint les couleurs de sommet",
            ToolKind::Mask => "Peint le masque (alpha) qui protège la zone",
            ToolKind::Material => "Assigne un emplacement de matériau",
        }
    }

    pub fn is_additive(self) -> bool {
        matches!(self, ToolKind::Clay | ToolKind::Inflate)
    }

    pub fn is_paint(self) -> bool {
        matches!(self, ToolKind::Paint | ToolKind::Mask | ToolKind::Material)
    }
}

/// Fracture network style. The parameters are shared, the field differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FractureKind {
    /// Irregular polygonal cracks (Voronoi boundaries).
    Crack,
    /// Radial cracks from the hit point, like a shatter cone.
    Shatter,
    /// Parallel shear planes, offset by the stroke direction.
    Shear,
    /// Vertical columns with horizontal breaks (basalt).
    Columnar,
    /// Conchoidal rings around the centre.
    Radial,
}

impl FractureKind {
    pub const ALL: [FractureKind; 5] = [
        FractureKind::Crack,
        FractureKind::Shatter,
        FractureKind::Shear,
        FractureKind::Columnar,
        FractureKind::Radial,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FractureKind::Crack => "Fissures polygonales",
            FractureKind::Shatter => "Éclatement radial",
            FractureKind::Shear => "Cisaillement",
            FractureKind::Columnar => "Orgues basaltiques",
            FractureKind::Radial => "Rides concentriques",
        }
    }
}

/// Mirror / radial repetition of a stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Symmetry {
    None,
    X,
    Y,
    Xy,
    Radial(u32),
}

impl Symmetry {
    pub fn label(self) -> &'static str {
        match self {
            Symmetry::None => "Aucune",
            Symmetry::X => "Miroir X",
            Symmetry::Y => "Miroir Y",
            Symmetry::Xy => "Miroir XY",
            Symmetry::Radial(_) => "Radial",
        }
    }
}

/// Everything the UI exposes for the current tool.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrushSettings {
    pub tool: ToolKind,
    pub radius_m: f32,
    /// 0..1. Carve depth is `strength * radius * 0.35` at the centre.
    pub strength: f32,
    pub falloff: Falloff,
    /// 0 = very soft shoulder, 1 = hard edge.
    pub hardness: f32,
    /// Strength of the noise modulation, 0..1.
    pub grain: f32,
    pub noise_scale_m: f32,
    /// Chisel / blade rotation around the surface normal, degrees.
    pub blade_deg: f32,
    /// Blade width as a fraction of the radius.
    pub blade_width: f32,
    /// Impact tilt, 0 = straight in, 90 = grazing.
    pub tilt_deg: f32,
    /// Bedding thickness for the strata chisel, metres.
    pub layer_m: f32,
    pub layer_count: u32,
    pub fracture: FractureKind,
    pub fracture_count: u32,
    /// Crack depth, metres.
    pub fracture_depth_m: f32,
    /// Crack width as a fraction of the radius.
    pub fracture_width: f32,
    /// Melt / fusion amount, 0..1.
    pub melt: f32,
    pub paint_color: [u8; 4],
    /// Material slot written by the Material tool.
    pub material: u16,
    pub surface: RockSurface,
    pub symmetry: Symmetry,
    pub invert: bool,
    /// Refine the mesh under the brush before displacing it.
    pub dynamic_topology: bool,
    pub max_subdivisions: u32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            tool: ToolKind::Hammer,
            radius_m: 3.0,
            strength: 0.55,
            falloff: Falloff::Smooth,
            hardness: 0.45,
            grain: 0.35,
            noise_scale_m: 1.2,
            blade_deg: 0.0,
            blade_width: 0.35,
            tilt_deg: 15.0,
            layer_m: 0.8,
            layer_count: 4,
            fracture: FractureKind::Crack,
            fracture_count: 7,
            fracture_depth_m: 0.6,
            fracture_width: 0.09,
            melt: 0.5,
            paint_color: [188, 176, 158, 255],
            material: 0,
            surface: RockSurface::Granite,
            symmetry: Symmetry::None,
            invert: false,
            dynamic_topology: true,
            max_subdivisions: 2,
        }
    }
}

impl BrushSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.01..=2000.0).contains(&self.radius_m) {
            return Err("le rayon doit être compris entre 1 cm et 2 km".into());
        }
        if !(0.0..=1.0).contains(&self.strength) {
            return Err("la force doit être comprise entre 0 et 1".into());
        }
        if self.fracture_count == 0 {
            return Err("le nombre de fissures doit être >= 1".into());
        }
        if self.fracture_depth_m < 0.0 {
            return Err("la profondeur de fissure doit être positive".into());
        }
        Ok(())
    }

    /// Effective falloff taking hardness into account.
    pub fn weight(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let shaped = if self.hardness > 0.001 {
            t.powf(1.0 / (1.0 - self.hardness * 0.85).max(0.15))
        } else {
            t
        };
        self.falloff.weight(shaped)
    }

    fn displacement_scale(&self, radius: f32) -> f32 {
        self.strength * radius * 0.35
    }

    /// Radius actually used by a stamp. On a coarse mesh a brush smaller than
    /// one triangle would select no vertex at all (the user sees nothing
    /// happen); expand it to cover at least the contact face. The UI draws
    /// this same radius, so what is highlighted is what is sculpted.
    pub fn effective_radius_m(&self, mesh: &Mesh, triangle: [u32; 3]) -> f32 {
        self.radius_m.max(mesh.triangle_max_edge(triangle) * 1.15)
    }
}

/// What a stroke did.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StrokeReport {
    pub stamps: u32,
    pub moved_vertices: usize,
    pub subdivided: u32,
    pub bounds: Aabb,
}

/// Undo data for one stroke. Position-only edits are compact; a stroke that
/// triggered dynamic topology stores the two full meshes instead.
#[derive(Debug, Clone)]
pub enum MeshUndo {
    Vertices {
        indices: Vec<u32>,
        before_positions: Vec<Vec3>,
        after_positions: Vec<Vec3>,
        before_colors: Option<Vec<[u8; 4]>>,
        after_colors: Option<Vec<[u8; 4]>>,
        before_bounds: Aabb,
        after_bounds: Aabb,
    },
    Topology {
        before: Box<Mesh>,
        after: Box<Mesh>,
    },
}

impl MeshUndo {
    pub fn undo(&self, mesh: &mut Mesh) {
        match self {
            MeshUndo::Vertices {
                indices,
                before_positions,
                before_colors,
                ..
            } => {
                for (slot, &index) in indices.iter().enumerate() {
                    mesh.positions[index as usize] = before_positions[slot];
                }
                if let Some(colors) = before_colors {
                    for (slot, &index) in indices.iter().enumerate() {
                        if index as usize >= mesh.colors.len() {
                            mesh.ensure_colors();
                        }
                        mesh.colors[index as usize] = colors[slot];
                    }
                }
                mesh.recompute_normals();
            }
            MeshUndo::Topology { before, .. } => {
                *mesh = (**before).clone();
            }
        }
    }

    pub fn redo(&self, mesh: &mut Mesh) {
        match self {
            MeshUndo::Vertices {
                indices,
                after_positions,
                after_colors,
                ..
            } => {
                for (slot, &index) in indices.iter().enumerate() {
                    mesh.positions[index as usize] = after_positions[slot];
                }
                if let Some(colors) = after_colors {
                    for (slot, &index) in indices.iter().enumerate() {
                        if index as usize >= mesh.colors.len() {
                            mesh.ensure_colors();
                        }
                        mesh.colors[index as usize] = colors[slot];
                    }
                }
                mesh.recompute_normals();
            }
            MeshUndo::Topology { after, .. } => {
                *mesh = (**after).clone();
            }
        }
    }

    pub fn memory_bytes(&self) -> usize {
        match self {
            MeshUndo::Vertices {
                indices,
                before_positions,
                after_positions,
                before_colors,
                after_colors,
                ..
            } => {
                indices.len() * 4
                    + before_positions.len() * 12
                    + after_positions.len() * 12
                    + before_colors.as_ref().map_or(0, |c| c.len() * 4)
                    + after_colors.as_ref().map_or(0, |c| c.len() * 4)
            }
            MeshUndo::Topology { before, after } => before.memory_bytes() + after.memory_bytes(),
        }
    }
}

/// A vertex edit produced by a stroke: index, new position, optional colour
/// and optional material slot.
type VertexEdit = (u32, Vec3, Option<[u8; 4]>, Option<u16>);

/// A live stroke: accumulates undo data and keeps the topology maps the
/// stamps need. Cheap to create; drop it to abandon the stroke.
pub struct SculptStroke {
    brush: BrushSettings,
    seed: u64,
    last_center: Option<Vec3>,
    stamp_index: u64,
    /// Compact face adjacency: one allocation instead of one `Vec` per vertex,
    /// which on a million-vertex cliff dominated the cost of starting a stroke.
    graph: FaceGraph,
    vfaces: VertexFaces,
    tri_face: Vec<u32>,
    before_positions: HashMap<u32, Vec3>,
    after_positions: HashMap<u32, Vec3>,
    before_colors: HashMap<u32, [u8; 4]>,
    after_colors: HashMap<u32, [u8; 4]>,
    topology_before: Option<Box<Mesh>>,
    report: StrokeReport,
}

impl SculptStroke {
    pub fn begin(mesh: &Mesh, brush: BrushSettings, seed: u64) -> Self {
        let graph = mesh.face_graph();
        Self {
            brush,
            seed,
            last_center: None,
            stamp_index: 0,
            vfaces: VertexFaces::build(mesh.positions.len(), &graph),
            graph,
            tri_face: triangle_face_map(mesh),
            before_positions: HashMap::new(),
            after_positions: HashMap::new(),
            before_colors: HashMap::new(),
            after_colors: HashMap::new(),
            topology_before: None,
            report: StrokeReport::default(),
        }
    }

    pub fn brush(&self) -> BrushSettings {
        self.brush
    }

    pub fn set_brush(&mut self, brush: BrushSettings) {
        self.brush = brush;
    }

    pub fn report(&self) -> &StrokeReport {
        &self.report
    }

    /// Stamp at `center` with the surface normal under the cursor. Returns
    /// `true` when the mesh changed.
    pub fn stamp(&mut self, mesh: &mut Mesh, bvh: &mut Bvh, center: Vec3, normal: Vec3) -> bool {
        if let Some(last) = self.last_center {
            let spacing = (self.brush.radius_m * 0.12).max(0.01);
            if last.distance(center) < spacing {
                return false;
            }
        }
        self.last_center = Some(center);
        self.stamp_index += 1;
        let normal = normal.normalize_or_zero();
        let normal = if normal.length_squared() < 0.5 {
            Vec3::Z
        } else {
            normal
        };
        let mut changed = false;
        for (index, (mirrored_center, mirrored_normal)) in
            symmetry_centers(center, normal, self.brush.symmetry)
                .into_iter()
                .enumerate()
        {
            let seed = self.seed ^ (self.stamp_index.wrapping_mul(0x9E37_79B9)) ^ index as u64;
            changed |= self.apply_one(mesh, bvh, mirrored_center, mirrored_normal, seed);
        }
        if changed {
            self.report.stamps += 1;
            bvh.refit(mesh);
        }
        changed
    }

    fn apply_one(
        &mut self,
        mesh: &mut Mesh,
        bvh: &mut Bvh,
        center: Vec3,
        normal: Vec3,
        seed: u64,
    ) -> bool {
        let radius = bvh
            .closest_point(mesh, center)
            .map(|hit| self.brush.effective_radius_m(mesh, hit.triangle))
            .unwrap_or(self.brush.radius_m);
        // Refine first so a coarse mesh can receive the detail instead of
        // stretching one triangle across the whole brush.
        if self.brush.dynamic_topology {
            self.subdivide_under_brush(mesh, bvh, center, normal, radius);
        }
        let affected = bvh.vertices_in_sphere(mesh, center, radius);
        if affected.is_empty() {
            return false;
        }
        let affected_set: HashSet<u32> = affected.iter().copied().collect();
        let neighbours = if needs_neighbourhood(self.brush.tool) {
            let graph = &self.graph;
            let vfaces = &self.vfaces;
            vf_core::compute::install(|| {
                affected
                    .par_iter()
                    .map(|&v| (v, neighbour_average(mesh, graph, vfaces, v)))
                    .collect::<HashMap<u32, Vec3>>()
            })
        } else {
            HashMap::new()
        };
        let (tangent, bitangent, _) = basis_from_normal(normal);
        let angle = self.brush.blade_deg.to_radians();
        // At 0 the blade lies along +X on a flat surface; positive angles
        // rotate it around the surface normal.
        let blade_dir = (bitangent * angle.cos() + tangent * angle.sin()).normalize_or_zero();
        let side_dir = blade_dir.cross(normal).normalize_or_zero();
        // Horizontal direction for undercuts; falls back to X on a vertical
        // wall facing X.
        let horizontal = Vec3::new(-normal.y, normal.x, 0.0).normalize_or_zero();
        let horizontal = if horizontal.length_squared() < 0.5 {
            Vec3::X
        } else {
            horizontal
        };
        let scale = self.brush.displacement_scale(radius);
        let sign = if self.brush.invert { -1.0 } else { 1.0 };
        let mut moved = 0usize;
        // Each vertex is an independent, pure function of the mesh and the
        // brush, so the evaluation runs on the shared compute pool. `collect`
        // keeps the brush order, and the application below stays sequential so
        // the undo bookkeeping is exact.
        let vfaces = &self.vfaces;
        let targets: Vec<VertexEdit> = vf_core::compute::install(|| {
            affected
                .par_iter()
                .filter_map(|&v| {
                    let index = v as usize;
                    if index >= mesh.positions.len() {
                        return None;
                    }
                    let p = mesh.positions[index];
                    let mask = mask_of(mesh, index);
                    if mask < 0.01 && self.brush.tool != ToolKind::Mask {
                        return None;
                    }
                    let offset = p - center;
                    let distance = offset.length();
                    if distance >= radius {
                        return None;
                    }
                    let mut t = distance / radius;
                    // Blade footprint: a line segment in the tangent plane.
                    if matches!(
                        self.brush.tool,
                        ToolKind::Chisel | ToolKind::Gouge | ToolKind::Strata
                    ) {
                        let along = offset.dot(blade_dir) / radius;
                        let across =
                            offset.dot(side_dir) / (radius * self.brush.blade_width.max(0.08));
                        t = (along * along + across * across).sqrt().min(1.0);
                    }
                    let w = self.brush.weight(t) * mask * sign;
                    if !w.is_finite() {
                        return None;
                    }
                    let grain = perlin3(
                        seed ^ 0xA5A5_5A5A,
                        p * (1.0 / self.brush.noise_scale_m.max(0.02)),
                    );
                    let grain_mix = 1.0 + self.brush.grain * grain * 0.9;
                    let vnormal = mesh
                        .normals
                        .get(index)
                        .copied()
                        .unwrap_or(Vec3::Z)
                        .normalize_or_zero();
                    let vnormal = if vnormal.length_squared() < 0.5 {
                        normal
                    } else {
                        vnormal
                    };
                    let flat_normal = matches!(
                        self.brush.tool,
                        ToolKind::Chisel | ToolKind::Gouge | ToolKind::Strata | ToolKind::Flatten
                    );
                    let direction = if flat_normal { normal } else { vnormal };
                    let average = neighbours.get(&v).copied();
                    let (delta, color, material) = match self.brush.tool {
                        ToolKind::Hammer => {
                            let punch = -scale * w * grain_mix;
                            // A small lip of displaced material around the pit.
                            let rim = if (0.62..1.0).contains(&t) {
                                scale * 0.16 * w * (1.0 - (t - 0.62) / 0.38).max(0.0)
                            } else {
                                0.0
                            };
                            (direction * (punch + rim), None, None)
                        }
                        ToolKind::Chisel => {
                            let cut = -scale * w;
                            let edge = smoothstep(0.82, 1.0, t);
                            (direction * (cut * (1.0 - 0.7 * edge)), None, None)
                        }
                        ToolKind::Gouge => {
                            let profile = (1.0 - t * t).max(0.0).sqrt();
                            (
                                direction * (-scale * w * (0.55 + 0.45 * profile)),
                                None,
                                None,
                            )
                        }
                        ToolKind::Strata => {
                            let band = self.brush.layer_m.max(0.02);
                            let phase = strata_profile(p.z * 0.75 + p.y * 0.25, band, 0.0, 0.7);
                            let layer = ((p.z - center.z) / band).round();
                            let step = -scale * w * (0.35 + 0.65 * phase.abs()) * grain_mix;
                            (
                                direction * (step * (1.0 - 0.25 * layer.abs().min(1.0))),
                                None,
                                None,
                            )
                        }
                        ToolKind::File => {
                            if let Some(avg) = average {
                                let smooth = (avg - p) * (0.65 * self.brush.strength * mask);
                                // Scratches along the stroke direction.
                                let scratch = perlin3(
                                    seed ^ 0x51ED_270B,
                                    p * (1.0 / (self.brush.noise_scale_m * 0.25).max(0.01)),
                                );
                                let along = offset.dot(blade_dir);
                                let scratch =
                                    (scratch * 8.0).sin() * 0.06 * self.brush.strength * mask;
                                let wear = direction * (-scale * 0.08 * w * (1.0 + scratch));
                                (
                                    smooth + wear * (1.0 - along.abs() / radius).max(0.0),
                                    None,
                                    None,
                                )
                            } else {
                                (Vec3::ZERO, None, None)
                            }
                        }
                        ToolKind::Clay => {
                            let build = scale * w * (0.75 + 0.25 * grain_mix);
                            let mut delta = direction * build;
                            if let Some(avg) = average {
                                delta += (avg - p) * 0.18 * self.brush.strength * mask;
                            }
                            (delta, None, None)
                        }
                        ToolKind::Crease => {
                            let toward = center - p;
                            let tangential = toward - direction * toward.dot(direction);
                            let ridge = direction
                                * (scale * 0.22 * w * if self.brush.invert { 1.0 } else { -1.0 });
                            (
                                tangential * (0.35 * self.brush.strength) + ridge,
                                None,
                                None,
                            )
                        }
                        ToolKind::Pinch => {
                            let toward = center - p;
                            let tangential = toward - direction * toward.dot(direction);
                            (tangential * (0.45 * self.brush.strength * w), None, None)
                        }
                        ToolKind::Inflate => (direction * (scale * w * grain_mix), None, None),
                        ToolKind::Flatten => {
                            let height = (p - center).dot(normal);
                            let target = p - normal * height;
                            (
                                (target - p) * (w * self.brush.strength).min(1.0),
                                None,
                                None,
                            )
                        }
                        ToolKind::Undercut => {
                            let reach = scale * w * 1.4;
                            let lip = horizontal * reach;
                            let drop = -normal * (scale * 0.35 * w);
                            (lip + drop, None, None)
                        }
                        ToolKind::Melt => {
                            let mut delta = Vec3::ZERO;
                            if let Some(avg) = average {
                                delta += (avg - p) * (self.brush.melt * 0.75 * w).clamp(0.0, 0.9);
                            }
                            let flow = Vec3::new(0.0, 0.0, -1.0)
                                * (scale * 0.45 * w * (1.0 - normal.z.abs() * 0.7));
                            let ripple = perlin3(
                                seed ^ 0x1234_5678,
                                p * (1.0 / (self.brush.noise_scale_m * 2.0)),
                            );
                            delta += flow * (0.75 + 0.25 * ripple);
                            (delta, None, None)
                        }
                        ToolKind::Impact => {
                            let crater = -scale * 1.6 * w;
                            let rim_t = ((t - 0.55) / 0.45).clamp(0.0, 1.0);
                            let rim = scale * 0.45 * (rim_t * (1.0 - rim_t) * 4.0) * (1.0 - t);
                            let mut delta = direction * (crater + rim);
                            // Radial cracks around the crater.
                            let angle = offset.dot(bitangent).atan2(offset.dot(tangent));
                            let spokes = (angle * self.brush.fracture_count as f32).sin().abs();
                            let crack = (1.0 - spokes).powf(8.0) * (1.0 - t).max(0.0);
                            delta += direction * (-scale * 0.8 * crack * w);
                            (delta, None, None)
                        }
                        ToolKind::Fracture => {
                            let depth =
                                self.fracture_field(p, center, normal, tangent, bitangent, seed);
                            (
                                direction * (-self.brush.fracture_depth_m * depth * w),
                                None,
                                None,
                            )
                        }
                        ToolKind::Detail => {
                            let amplitude = self.brush.strength * self.brush.radius_m * 0.08;
                            let surface = &self.brush.surface;
                            let frequency = 1.0 / surface.grain_m().max(0.02);
                            let n = perlin3(seed ^ 0xF00D, p * frequency)
                                + 0.5 * perlin3(seed ^ 0xBEEF, p * frequency * 2.3);
                            let strata = if surface.strata_m() > 0.0 {
                                0.35 * strata_profile(p.z, surface.strata_m(), 0.0, 0.6)
                            } else {
                                0.0
                            };
                            (direction * (amplitude * w * (n + strata)), None, None)
                        }
                        ToolKind::Smooth => {
                            if let Some(avg) = average {
                                (
                                    (avg - p) * (w * self.brush.strength).clamp(0.0, 1.0),
                                    None,
                                    None,
                                )
                            } else {
                                (Vec3::ZERO, None, None)
                            }
                        }
                        ToolKind::Paint => {
                            let [r, g, b, _] = self.brush.paint_color;
                            let amount = (w.abs() * self.brush.strength).clamp(0.0, 1.0);
                            let current = mesh
                                .colors
                                .get(index)
                                .copied()
                                .unwrap_or([150, 145, 138, 255]);
                            let mix = |from: u8, to: u8| {
                                (from as f32 * (1.0 - amount) + to as f32 * amount).round() as u8
                            };
                            (
                                Vec3::ZERO,
                                Some([
                                    mix(current[0], r),
                                    mix(current[1], g),
                                    mix(current[2], b),
                                    current[3],
                                ]),
                                None,
                            )
                        }
                        ToolKind::Mask => {
                            let current = mesh
                                .colors
                                .get(index)
                                .copied()
                                .unwrap_or([150, 145, 138, 255]);
                            let target = if self.brush.invert { 0u8 } else { 255u8 };
                            let amount = (w.abs() * self.brush.strength).clamp(0.0, 1.0);
                            let alpha = (current[3] as f32 * (1.0 - amount)
                                + target as f32 * amount)
                                .round()
                                .clamp(0.0, 255.0) as u8;
                            (
                                Vec3::ZERO,
                                Some([current[0], current[1], current[2], alpha]),
                                None,
                            )
                        }
                        ToolKind::Material => {
                            let material = self.brush.material;
                            let has_face = !vfaces.range(index).is_empty();
                            (Vec3::ZERO, None, has_face.then_some(material))
                        }
                    };
                    if !delta.is_finite() {
                        return None;
                    }
                    let max_step = radius * 0.6;
                    let delta = if delta.length() > max_step {
                        delta.normalize() * max_step
                    } else {
                        delta
                    };
                    Some((v, p + delta, color, material))
                })
                .collect()
        });
        if targets.is_empty() {
            return false;
        }
        // Material assignment works on faces, not vertices.
        if self.brush.tool == ToolKind::Material {
            let mut faces = HashSet::new();
            for &(v, ..) in &targets {
                let range = self.vfaces.range(v as usize);
                for &f in &self.vfaces.faces()[range] {
                    let centroid = mesh.face_centroid(f as usize);
                    if centroid.distance(center) < radius {
                        faces.insert(f);
                    }
                }
            }
            for f in &faces {
                if (*f as usize) < mesh.face_count() {
                    if mesh.face_materials.len() != mesh.face_count() {
                        mesh.face_materials = vec![0; mesh.face_count()];
                    }
                    mesh.face_materials[*f as usize] = self.brush.material;
                }
            }
            return !faces.is_empty();
        }
        let mut changed = false;
        let mut bounds = Aabb::EMPTY;
        for (v, target, color, _) in targets {
            let index = v as usize;
            let before = mesh.positions[index];
            if before.distance_squared(target) > 1e-10 {
                self.before_positions.entry(v).or_insert(before);
                mesh.positions[index] = target;
                self.after_positions.insert(v, target);
                bounds = bounds.with_point(before).with_point(target);
                moved += 1;
                changed = true;
            }
            if let Some(color) = color {
                mesh.ensure_colors();
                let before = mesh.colors[index];
                if before != color {
                    self.before_colors.entry(v).or_insert(before);
                    mesh.colors[index] = color;
                    self.after_colors.insert(v, color);
                    changed = true;
                }
            }
        }
        if changed {
            update_normals(mesh, &self.graph, &self.vfaces, &affected_set);
            self.report.moved_vertices += moved;
            self.report.bounds = self.report.bounds.union(bounds);
        }
        changed
    }

    fn fracture_field(
        &self,
        p: Vec3,
        center: Vec3,
        _normal: Vec3,
        tangent: Vec3,
        bitangent: Vec3,
        seed: u64,
    ) -> f32 {
        let offset = p - center;
        let radius = self.brush.radius_m;
        let width = (self.brush.fracture_width * radius).max(1e-3);
        let count = self.brush.fracture_count.max(1) as f32;
        match self.brush.fracture {
            FractureKind::Crack => {
                let cell = (radius * 2.0) / count;
                let sample = voronoi3(seed ^ 0xFACE, p / cell);
                let edge = sample.edge() * cell;
                (1.0 - (edge / width).clamp(0.0, 1.0)).powf(2.0)
            }
            FractureKind::Shatter => {
                let angle = offset.dot(bitangent).atan2(offset.dot(tangent));
                let spokes = (angle * count).sin().abs();
                let radial = 1.0 - smoothstep(radius * 0.05, radius * 0.9, offset.length());
                (1.0 - spokes).powf(6.0) * radial
            }
            FractureKind::Shear => {
                let across = offset.dot(bitangent);
                let spacing = (radius * 2.0) / count;
                let phase = (across / spacing).fract() - 0.5;
                let wobble = perlin3(seed ^ 0x5EA2, p * 0.4) * 0.25;
                let distance = (phase + wobble).abs() * spacing;
                (1.0 - (distance / width).clamp(0.0, 1.0)).powf(2.0)
            }
            FractureKind::Columnar => {
                let cell = (radius * 2.0) / count;
                let sample = voronoi2(seed ^ 0xC01, glam::Vec2::new(p.x, p.y) / cell);
                let edge = sample.edge() * cell;
                let vertical = 1.0;
                let joint = (1.0 - (edge / width).clamp(0.0, 1.0)).powf(3.0) * vertical;
                // Horizontal breaks every few metres.
                let band = self.brush.layer_m.max(0.1);
                let horizontal =
                    (1.0 - (strata_profile(p.z, band, 0.0, 0.9).abs() / 0.25).min(1.0)).max(0.0);
                joint.max(horizontal * 0.6)
            }
            FractureKind::Radial => {
                let distance = offset.dot(tangent).atan2(0.0);
                let _ = distance;
                let r = offset.length();
                let rings = (r / (radius / count)).fract() - 0.5;
                let wobble = perlin3(seed ^ 0x7777, p * 0.6) * 0.3;
                (1.0 - ((rings + wobble).abs() * 2.0).clamp(0.0, 1.0)).powf(2.0)
            }
        }
    }

    fn subdivide_under_brush(
        &mut self,
        mesh: &mut Mesh,
        bvh: &mut Bvh,
        center: Vec3,
        _normal: Vec3,
        radius: f32,
    ) {
        let mut levels = 0;
        while levels < self.brush.max_subdivisions {
            let Some(max_edge) =
                max_edge_near(mesh, &self.graph, &self.vfaces, bvh, center, radius)
            else {
                break;
            };
            let target = radius * 0.22;
            if max_edge <= target {
                break;
            }
            let mut triangles = Vec::new();
            bvh.query_aabb(
                Aabb::from_center_size(center, Vec3::splat(radius * 1.15)),
                &mut triangles,
            );
            let mut faces: HashSet<u32> = HashSet::new();
            for tri in triangles {
                if let Some(&face) = self.tri_face.get(tri as usize) {
                    faces.insert(face);
                }
            }
            // The brush normal is unused for the selection; keep it for the
            // footprint of the next stamp.
            if faces.is_empty() {
                break;
            }
            if self.topology_before.is_none() {
                self.topology_before = Some(Box::new(mesh.clone()));
            }
            let faces: Vec<u32> = faces.into_iter().collect();
            let added = mesh.subdivide_region(&faces, 1);
            if added == 0 {
                break;
            }
            self.graph = mesh.face_graph();
            self.vfaces = VertexFaces::build(mesh.positions.len(), &self.graph);
            self.tri_face = triangle_face_map(mesh);
            *bvh = Bvh::build(mesh);
            self.report.subdivided += 1;
            levels += 1;
        }
    }

    /// Finish the stroke and produce its undo record.
    pub fn finish(mut self, mesh: &Mesh) -> Option<MeshUndo> {
        if let Some(before) = self.topology_before.take() {
            return Some(MeshUndo::Topology {
                before,
                after: Box::new(mesh.clone()),
            });
        }
        if self.before_positions.is_empty() && self.before_colors.is_empty() {
            return None;
        }
        let mut indices: Vec<u32> = self
            .before_positions
            .keys()
            .chain(self.before_colors.keys())
            .copied()
            .collect::<HashSet<u32>>()
            .into_iter()
            .collect();
        indices.sort_unstable();
        let before_positions: Vec<Vec3> = indices
            .iter()
            .map(|v| {
                self.before_positions
                    .get(v)
                    .copied()
                    .unwrap_or_else(|| mesh.positions[*v as usize])
            })
            .collect();
        let after_positions: Vec<Vec3> = indices
            .iter()
            .map(|v| {
                self.after_positions
                    .get(v)
                    .copied()
                    .unwrap_or_else(|| mesh.positions[*v as usize])
            })
            .collect();
        let colors_changed = !self.before_colors.is_empty();
        let before_colors = colors_changed.then(|| {
            indices
                .iter()
                .map(|v| {
                    self.before_colors
                        .get(v)
                        .copied()
                        .unwrap_or([150, 145, 138, 255])
                })
                .collect()
        });
        let after_colors = colors_changed.then(|| {
            indices
                .iter()
                .map(|v| {
                    self.after_colors
                        .get(v)
                        .copied()
                        .unwrap_or([150, 145, 138, 255])
                })
                .collect()
        });
        let before_bounds = positions_bounds(&before_positions);
        let after_bounds = positions_bounds(&after_positions);
        Some(MeshUndo::Vertices {
            indices,
            before_positions,
            after_positions,
            before_colors,
            after_colors,
            before_bounds,
            after_bounds,
        })
    }
}

fn positions_bounds(positions: &[Vec3]) -> Aabb {
    let mut bounds = Aabb::EMPTY;
    for &p in positions {
        bounds = bounds.with_point(p);
    }
    bounds
}

fn needs_neighbourhood(tool: ToolKind) -> bool {
    matches!(
        tool,
        ToolKind::File | ToolKind::Clay | ToolKind::Melt | ToolKind::Smooth
    )
}

fn mask_of(mesh: &Mesh, index: usize) -> f32 {
    mesh.colors
        .get(index)
        .map(|c| c[3] as f32 / 255.0)
        .unwrap_or(1.0)
}

fn triangle_face_map(mesh: &Mesh) -> Vec<u32> {
    let mut map = Vec::new();
    for (index, face) in mesh.faces().enumerate() {
        let n = face.len();
        for _ in 1..n - 1 {
            map.push(index as u32);
        }
    }
    map
}

fn neighbour_average(mesh: &Mesh, graph: &FaceGraph, vfaces: &VertexFaces, v: u32) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut count = 0.0;
    let range = vfaces.range(v as usize);
    for &f in &vfaces.faces()[range] {
        for &other in graph.face(f as usize) {
            if other != v
                && let Some(p) = mesh.positions.get(other as usize)
            {
                sum += *p;
                count += 1.0;
            }
        }
    }
    if count > 0.0 { sum / count } else { Vec3::ZERO }
}

/// Recompute the normals around a stroke, in parallel and in a fixed order.
///
/// The former implementation accumulated `face_normal * area` into a `HashMap`
/// while iterating a `HashSet` of dirty faces, so the summation order — and
/// therefore the last bits of every normal — changed from run to run. This
/// version gathers, for each touched vertex, its incident faces in ascending
/// face order: complete, deterministic, and one task per vertex.
fn update_normals(
    mesh: &mut Mesh,
    graph: &FaceGraph,
    vfaces: &VertexFaces,
    affected: &HashSet<u32>,
) {
    let mut touched: Vec<u32> = Vec::new();
    for &v in affected {
        let range = vfaces.range(v as usize);
        for &f in &vfaces.faces()[range] {
            for &corner in graph.face(f as usize) {
                touched.push(corner);
            }
        }
    }
    touched.sort_unstable();
    touched.dedup();
    if touched.is_empty() {
        return;
    }
    let updated: Vec<(u32, Vec3)> = vf_core::compute::install(|| {
        touched
            .par_iter()
            .map(|&v| {
                let mut sum = Vec3::ZERO;
                let range = vfaces.range(v as usize);
                for &f in &vfaces.faces()[range] {
                    let face = f as usize;
                    if face >= mesh.face_count() {
                        continue;
                    }
                    let area = mesh.face_area(face).max(1e-9);
                    sum += mesh.face_normal(face) * area;
                }
                (v, sum)
            })
            .collect()
    });
    for (v, normal) in updated {
        if normal.length_squared() > 1e-16
            && let Some(slot) = mesh.normals.get_mut(v as usize)
        {
            *slot = normal.normalize();
        }
    }
}

fn max_edge_near(
    mesh: &Mesh,
    graph: &FaceGraph,
    vfaces: &VertexFaces,
    bvh: &Bvh,
    center: Vec3,
    radius: f32,
) -> Option<f32> {
    let vertices = bvh.vertices_in_sphere(mesh, center, radius);
    let mut max_edge: f32 = 0.0;
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    for v in vertices {
        let range = vfaces.range(v as usize);
        for &f in &vfaces.faces()[range] {
            let face = graph.face(f as usize);
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                let key = if a < b { (a, b) } else { (b, a) };
                if seen.insert(key) {
                    max_edge = max_edge
                        .max(mesh.positions[a as usize].distance(mesh.positions[b as usize]));
                }
            }
        }
    }
    if max_edge > 0.0 { Some(max_edge) } else { None }
}

/// Mirror / radial copies of a stamp.
pub fn symmetry_centers(center: Vec3, normal: Vec3, symmetry: Symmetry) -> Vec<(Vec3, Vec3)> {
    let mut out = vec![(center, normal)];
    match symmetry {
        Symmetry::None => {}
        Symmetry::X => {
            out.push((
                Vec3::new(-center.x, center.y, center.z),
                Vec3::new(-normal.x, normal.y, normal.z),
            ));
        }
        Symmetry::Y => {
            out.push((
                Vec3::new(center.x, -center.y, center.z),
                Vec3::new(normal.x, -normal.y, normal.z),
            ));
        }
        Symmetry::Xy => {
            out.push((
                Vec3::new(-center.x, center.y, center.z),
                Vec3::new(-normal.x, normal.y, normal.z),
            ));
            out.push((
                Vec3::new(center.x, -center.y, center.z),
                Vec3::new(normal.x, -normal.y, normal.z),
            ));
            out.push((
                Vec3::new(-center.x, -center.y, center.z),
                Vec3::new(-normal.x, -normal.y, normal.z),
            ));
        }
        Symmetry::Radial(count) => {
            let count = count.clamp(2, 64);
            for i in 1..count {
                let angle = std::f32::consts::TAU * i as f32 / count as f32;
                let (sin, cos) = angle.sin_cos();
                out.push((
                    Vec3::new(
                        center.x * cos - center.y * sin,
                        center.x * sin + center.y * cos,
                        center.z,
                    ),
                    Vec3::new(
                        normal.x * cos - normal.y * sin,
                        normal.x * sin + normal.y * cos,
                        normal.z,
                    ),
                ));
            }
        }
    }
    // Drop duplicates on the symmetry axes.
    out.dedup_by(|a, b| a.0.distance_squared(b.0) < 1e-6);
    out
}

/// Apply one isolated stamp, useful for tests and for a single click.
pub fn stamp(
    mesh: &mut Mesh,
    bvh: &mut Bvh,
    brush: BrushSettings,
    center: Vec3,
    normal: Vec3,
    seed: u64,
) {
    let mut stroke = SculptStroke::begin(mesh, brush, seed);
    stroke.stamp(mesh, bvh, center, normal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

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
    fn hammer_moves_vertices_down_and_leaves_a_pit() {
        let mut mesh = plane(20.0, 40);
        let mut bvh = Bvh::build(&mesh);
        let brush = BrushSettings {
            tool: ToolKind::Hammer,
            radius_m: 3.0,
            strength: 0.8,
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let center = Vec3::new(0.0, 0.0, 0.0);
        let mut stroke = SculptStroke::begin(&mesh, brush, 7);
        assert!(stroke.stamp(&mut mesh, &mut bvh, center, Vec3::Z));
        let lowest = mesh.positions.iter().map(|p| p.z).fold(0.0f32, f32::min);
        assert!(lowest < -0.05, "{lowest}");
        let undo = stroke.finish(&mesh).expect("undo");
        undo.undo(&mut mesh);
        assert!(mesh.positions.iter().all(|p| p.z.abs() < 1e-5));
        undo.redo(&mut mesh);
        assert!(mesh.positions.iter().any(|p| p.z < -0.05));
    }

    #[test]
    fn brush_smaller_than_a_face_still_carves() {
        // 5 m cells, 1 m brush: without the contact-face expansion the sphere
        // contains no vertex and the user sees nothing happen.
        let mut mesh = plane(20.0, 4);
        let mut bvh = Bvh::build(&mesh);
        let brush = BrushSettings {
            tool: ToolKind::Hammer,
            radius_m: 1.0,
            strength: 1.0,
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let hit = bvh.closest_point(&mesh, Vec3::ZERO).expect("hit");
        assert!(brush.effective_radius_m(&mesh, hit.triangle) > 5.0);
        let mut stroke = SculptStroke::begin(&mesh, brush, 3);
        assert!(stroke.stamp(&mut mesh, &mut bvh, Vec3::ZERO, Vec3::Z));
        let lowest = mesh.positions.iter().map(|p| p.z).fold(0.0f32, f32::min);
        assert!(lowest < -0.05, "{lowest}");
    }

    #[test]
    fn mask_protects_vertices() {
        let mut mesh = plane(20.0, 20);
        mesh.ensure_colors();
        for color in &mut mesh.colors {
            color[3] = 0;
        }
        let mut bvh = Bvh::build(&mesh);
        let brush = BrushSettings {
            tool: ToolKind::Hammer,
            radius_m: 5.0,
            strength: 1.0,
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let mut stroke = SculptStroke::begin(&mesh, brush, 1);
        assert!(!stroke.stamp(&mut mesh, &mut bvh, Vec3::ZERO, Vec3::Z));
        assert!(mesh.positions.iter().all(|p| p.z.abs() < 1e-6));
    }

    #[test]
    fn fracture_carves_a_network() {
        let mut mesh = plane(20.0, 80);
        let mut bvh = Bvh::build(&mesh);
        let brush = BrushSettings {
            tool: ToolKind::Fracture,
            radius_m: 6.0,
            strength: 1.0,
            fracture_depth_m: 1.2,
            fracture_count: 9,
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let mut stroke = SculptStroke::begin(&mesh, brush, 42);
        assert!(stroke.stamp(&mut mesh, &mut bvh, Vec3::ZERO, Vec3::Z));
        let carved = mesh.positions.iter().filter(|p| p.z < -0.2).count();
        assert!(carved > 10, "{carved}");
        assert!(mesh.positions.iter().all(|p| p.z > -2.0));
    }

    #[test]
    fn chisel_footprint_is_not_radially_symmetric() {
        let mut mesh = plane(20.0, 60);
        let mut bvh = Bvh::build(&mesh);
        let brush = BrushSettings {
            tool: ToolKind::Chisel,
            radius_m: 4.0,
            strength: 1.0,
            blade_deg: 0.0,
            blade_width: 0.25,
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let mut stroke = SculptStroke::begin(&mesh, brush, 3);
        assert!(stroke.stamp(&mut mesh, &mut bvh, Vec3::ZERO, Vec3::Z));
        let along: f32 = mesh
            .positions
            .iter()
            .filter(|p| p.x.abs() > 2.5 && p.y.abs() < 0.3)
            .map(|p| p.z)
            .fold(0.0, f32::min);
        let across: f32 = mesh
            .positions
            .iter()
            .filter(|p| p.y.abs() > 2.5 && p.x.abs() < 0.3)
            .map(|p| p.z)
            .fold(0.0, f32::min);
        assert!(along < across - 0.05, "along {along} across {across}");
    }

    #[test]
    fn symmetry_mirrors_the_centre() {
        let centers = symmetry_centers(Vec3::new(2.0, 3.0, 1.0), Vec3::Z, Symmetry::Xy);
        assert_eq!(centers.len(), 4);
        assert!(centers.iter().any(|(c, _)| c.x < 0.0 && c.y < 0.0));
    }

    #[test]
    fn paint_and_mask_write_colours() {
        let mut mesh = plane(10.0, 10);
        mesh.ensure_colors();
        let mut bvh = Bvh::build(&mesh);
        let paint = BrushSettings {
            tool: ToolKind::Paint,
            radius_m: 2.0,
            strength: 1.0,
            paint_color: [250, 10, 10, 255],
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let mut stroke = SculptStroke::begin(&mesh, paint, 1);
        assert!(stroke.stamp(&mut mesh, &mut bvh, Vec3::ZERO, Vec3::Z));
        assert!(mesh.colors.iter().any(|c| c[0] > 200 && c[1] < 60));
        let mask = BrushSettings {
            tool: ToolKind::Mask,
            radius_m: 2.0,
            strength: 1.0,
            invert: true,
            dynamic_topology: false,
            ..BrushSettings::default()
        };
        let mut stroke = SculptStroke::begin(&mesh, mask, 1);
        assert!(stroke.stamp(&mut mesh, &mut bvh, Vec3::ZERO, Vec3::Z));
        assert!(mesh.colors.iter().any(|c| c[3] < 40));
    }
}

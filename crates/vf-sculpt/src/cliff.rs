//! Cliff-wall generation and wall sculpting.
//!
//! A wall is swept along a base line. At each station the profile moves
//! forward and back with the bedding: soft beds recede, hard beds stick out,
//! which is exactly what makes an overhang. The result is a closed solid, so
//! it can be voxel-remeshed, fractured, exported or printed.

use glam::{Vec2, Vec3};

use crate::bvh::Bvh;
use crate::math::{basis_from_normal, smoothstep};
use crate::mesh::Mesh;
use crate::noise::{perlin3, voronoi2};
use crate::rock::RockProfile;
use crate::sculpt::{BrushSettings, SculptStroke, StrokeReport, ToolKind};

/// The kind of wall, with a tuned set of parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliffStyle {
    /// Layered sea cliff: horizontal beds, big overhangs, rough face.
    SeaCliff,
    /// Slate quarry: thin near-vertical sheets and sharp steps.
    SlateQuarry,
    /// Steep schist wall: fine foliation, jagged edge.
    SchistWall,
    /// Basalt columns: vertical polygonal organ pipes.
    BasaltColumns,
    /// Sandstone canyon: alternating hard/soft beds and alcoves.
    SandstoneCanyon,
}

impl CliffStyle {
    pub const ALL: [CliffStyle; 5] = [
        CliffStyle::SeaCliff,
        CliffStyle::SlateQuarry,
        CliffStyle::SchistWall,
        CliffStyle::BasaltColumns,
        CliffStyle::SandstoneCanyon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CliffStyle::SeaCliff => "Falaise littorale",
            CliffStyle::SlateQuarry => "Carrière d'ardoise",
            CliffStyle::SchistWall => "Paroi de schiste",
            CliffStyle::BasaltColumns => "Colonnes de basalte",
            CliffStyle::SandstoneCanyon => "Canyon de grès",
        }
    }

    pub fn settings(self) -> CliffSettings {
        let mut settings = CliffSettings::default();
        match self {
            CliffStyle::SeaCliff => {
                settings.height_m = 42.0;
                settings.length_m = 160.0;
                settings.strata_m = 2.2;
                settings.strata_depth_m = 0.55;
                settings.overhang_m = 1.6;
                settings.roughness_m = 0.7;
                settings.rock = RockProfile::preset("limestone").unwrap_or_default();
            }
            CliffStyle::SlateQuarry => {
                settings.height_m = 28.0;
                settings.length_m = 90.0;
                settings.strata_m = 0.35;
                settings.strata_depth_m = 0.22;
                settings.overhang_m = 0.4;
                settings.roughness_m = 0.18;
                settings.dip_deg = 72.0;
                settings.rock = RockProfile::preset("schist").unwrap_or_default();
            }
            CliffStyle::SchistWall => {
                settings.height_m = 55.0;
                settings.length_m = 120.0;
                settings.strata_m = 0.5;
                settings.strata_depth_m = 0.3;
                settings.overhang_m = 0.8;
                settings.roughness_m = 0.35;
                settings.dip_deg = 58.0;
                settings.rock = RockProfile::preset("schist").unwrap_or_default();
            }
            CliffStyle::BasaltColumns => {
                settings.height_m = 34.0;
                settings.length_m = 70.0;
                settings.strata_m = 0.0;
                settings.columnar = true;
                settings.column_spacing_m = 2.1;
                settings.column_depth_m = 0.45;
                settings.overhang_m = 0.25;
                settings.roughness_m = 0.2;
                settings.rock = RockProfile::preset("basalt").unwrap_or_default();
            }
            CliffStyle::SandstoneCanyon => {
                settings.height_m = 26.0;
                settings.length_m = 110.0;
                settings.strata_m = 1.4;
                settings.strata_depth_m = 0.85;
                settings.overhang_m = 2.4;
                settings.roughness_m = 0.55;
                settings.rock = RockProfile::preset("canyon").unwrap_or_default();
            }
        }
        settings
    }
}

/// Parameters for a generated wall.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CliffSettings {
    pub length_m: f32,
    pub height_m: f32,
    /// Base altitude, metres.
    pub base_z: f32,
    /// Stations along the wall.
    pub segments: u32,
    /// Rows from bottom to top.
    pub rows: u32,
    /// Horizontal curvature of the plan line, metres of sag.
    pub curve_m: f32,
    /// Bedding thickness, metres. 0 disables the beds.
    pub strata_m: f32,
    /// Depth of the undercut at the base of each bed, metres.
    pub strata_depth_m: f32,
    /// Dip of the beds, degrees (leaning to the right).
    pub dip_deg: f32,
    /// Global forward bulge of the wall, metres.
    pub overhang_m: f32,
    /// Small-scale relief, metres.
    pub roughness_m: f32,
    /// Solid thickness behind the face, metres.
    pub thickness_m: f32,
    /// Vertical polygonal columns.
    pub columnar: bool,
    pub column_spacing_m: f32,
    pub column_depth_m: f32,
    /// Taper of the top (negative = leaning back).
    pub taper: f32,
    pub rock: RockProfile,
    pub seed: u64,
}

impl Default for CliffSettings {
    fn default() -> Self {
        let mut settings = Self {
            length_m: 100.0,
            height_m: 30.0,
            base_z: 0.0,
            segments: 96,
            rows: 48,
            curve_m: 0.0,
            strata_m: 1.8,
            strata_depth_m: 0.5,
            dip_deg: 0.0,
            overhang_m: 1.0,
            roughness_m: 0.4,
            thickness_m: 2.0,
            columnar: false,
            column_spacing_m: 2.0,
            column_depth_m: 0.4,
            taper: 0.0,
            rock: RockProfile::preset("sandstone").unwrap_or_default(),
            seed: 0,
        };
        settings.rock.strength = 0.0;
        settings
    }
}

impl CliffSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.length_m <= 0.0 || self.height_m <= 0.0 {
            return Err("la longueur et la hauteur doivent être positives".into());
        }
        if self.segments < 2 || self.rows < 2 {
            return Err("il faut au moins 2 segments et 2 rangées".into());
        }
        if self.thickness_m <= 0.0 {
            return Err("l'épaisseur doit être positive".into());
        }
        self.rock.validate()
    }
}

/// The forward offset of the face at `(x, z)`.
fn face_offset(settings: &CliffSettings, x: f32, z: f32) -> f32 {
    let t = ((z - settings.base_z) / settings.height_m).clamp(0.0, 1.0);
    // Overall bulge: strongest around two thirds of the height, the classic
    // sea-cliff profile.
    let mut offset = settings.overhang_m * (std::f32::consts::PI * t).sin().max(0.0).powf(1.2);
    offset += settings.taper * t;
    if settings.strata_m > 0.0 {
        let dip = settings.dip_deg.to_radians();
        let band = (z / settings.strata_m) - (x * dip.tan() / settings.strata_m);
        let frac = band.fract().abs();
        // Sawtooth: the base of each bed protrudes, then the face retreats
        // until the next hard ledge. That produces a real undercut.
        let ledge = (1.0 - frac).powf(1.6);
        offset -= settings.strata_depth_m * (1.0 - ledge);
        offset += settings.strata_depth_m * 0.35;
        // An extra step every five beds, like a wave-cut bench.
        let bench = (band / 5.0).floor();
        let bench_phase = ((band / 5.0) - bench).clamp(0.0, 1.0);
        if bench_phase < 0.18 {
            offset -= settings.strata_depth_m * 1.4;
        }
    }
    if settings.columnar {
        let cell = settings.column_spacing_m.max(0.2);
        let sample = voronoi2(settings.seed ^ 0xC01, Vec2::new(x / cell, z / (cell * 2.4)));
        let groove = 1.0 - smoothstep(0.0, 0.22, sample.edge());
        offset -= groove * settings.column_depth_m;
    }
    if settings.roughness_m > 0.0 {
        let n = perlin3(
            settings.seed ^ 0x51,
            Vec3::new(x * 0.21, z * 0.37, settings.seed as f32 * 0.01),
        );
        offset += n * settings.roughness_m;
        let fine = perlin3(settings.seed ^ 0x77, Vec3::new(x * 0.9, z * 1.7, 4.0));
        offset += fine * settings.roughness_m * 0.2;
    }
    offset
}

/// Generate a straight wall of `length_m` centred on the origin, facing -Y.
pub fn wall(settings: CliffSettings) -> Mesh {
    let base: Vec<Vec3> = (0..=settings.segments.max(2))
        .map(|i| {
            let x = (i as f32 / settings.segments.max(2) as f32 - 0.5) * settings.length_m;
            let y = settings.curve_m
                * (std::f32::consts::PI * i as f32 / settings.segments.max(2) as f32).sin();
            Vec3::new(x, y, settings.base_z)
        })
        .collect();
    wall_along_path(&base, settings)
}

/// Generate a wall swept along `base` (the bottom line of the face). The wall
/// rises vertically from the path, with the face facing the path's right-hand
/// normal (`tangent × Z`).
pub fn wall_along_path(base: &[Vec3], settings: CliffSettings) -> Mesh {
    if base.len() < 2 {
        return Mesh::new();
    }
    let stations = base.len();
    let rows = settings.rows.max(2) as usize;
    let mut positions = Vec::with_capacity(stations * rows * 2);
    let mut front = vec![Vec3::ZERO; stations * rows];
    let mut back = vec![Vec3::ZERO; stations * rows];
    let mut side_normals = Vec::with_capacity(stations);
    for i in 0..stations {
        let previous = base[i.saturating_sub(1)];
        let next = base[(i + 1).min(stations - 1)];
        let tangent = (next - previous).normalize_or_zero();
        let tangent = if tangent.length_squared() < 0.5 {
            Vec3::X
        } else {
            tangent
        };
        let side = tangent.cross(Vec3::Z).normalize_or_zero();
        let side = if side.length_squared() < 0.5 {
            Vec3::Y
        } else {
            side
        };
        side_normals.push(side);
        if i == 0 {
            side_normals[0] = side;
        }
    }
    for (i, &point) in base.iter().enumerate() {
        let side = side_normals[i];
        let distance = point.distance(base[0]);
        for row in 0..rows {
            let t = row as f32 / (rows - 1) as f32;
            let z = settings.base_z + t * settings.height_m;
            // The face's x coordinate follows the path; the offset is applied
            // along the side normal.
            let offset = face_offset(&settings, distance, z);
            // The top leans back into the hill; the foot is buried.
            let foot = -0.6 * (1.0 - t).powf(3.0);
            let front_point = point + side * (offset + foot) + Vec3::Z * (t * settings.height_m);
            let back_point = point - side * (settings.thickness_m + foot * 0.3)
                + Vec3::Z * (t * settings.height_m);
            front[i * rows + row] = front_point;
            back[i * rows + row] = back_point;
        }
    }
    for point in &front {
        positions.push(*point);
    }
    for point in &back {
        positions.push(*point);
    }
    let mut mesh = Mesh::new();
    for position in positions {
        mesh.push_vertex(position);
    }
    let at = |station: usize, row: usize, back: bool| -> u32 {
        let base = if back { front.len() } else { 0 };
        (base + station * rows + row) as u32
    };
    // Front sheet faces -side.
    for i in 0..stations - 1 {
        for row in 0..rows - 1 {
            let a = at(i, row, false);
            let b = at(i + 1, row, false);
            let c = at(i + 1, row + 1, false);
            let d = at(i, row + 1, false);
            mesh.push_face(&[a, b, c, d]);
        }
    }
    // Back sheet faces +side: reversed winding.
    for i in 0..stations - 1 {
        for row in 0..rows - 1 {
            let a = at(i, row, true);
            let b = at(i, row + 1, true);
            let c = at(i + 1, row + 1, true);
            let d = at(i + 1, row, true);
            mesh.push_face(&[a, b, c, d]);
        }
    }
    // Bottom and top caps.
    for i in 0..stations - 1 {
        let a = at(i, 0, false);
        let b = at(i + 1, 0, false);
        let c = at(i + 1, 0, true);
        let d = at(i, 0, true);
        mesh.push_face(&[a, d, c, b]);
        let a = at(i, rows - 1, false);
        let b = at(i + 1, rows - 1, false);
        let c = at(i + 1, rows - 1, true);
        let d = at(i, rows - 1, true);
        mesh.push_face(&[a, b, c, d]);
    }
    // End caps.
    for row in 0..rows - 1 {
        let a = at(0, row, false);
        let b = at(0, row + 1, false);
        let c = at(0, row + 1, true);
        let d = at(0, row, true);
        mesh.push_face(&[a, b, c, d]);
        let a = at(stations - 1, row, false);
        let b = at(stations - 1, row + 1, false);
        let c = at(stations - 1, row + 1, true);
        let d = at(stations - 1, row, true);
        mesh.push_face(&[a, d, c, b]);
    }
    mesh.box_project_uvs(1.0);
    mesh.recompute_normals();
    mesh
}

/// Add a second wall on top of an existing base, for a stepped cliff.
pub fn stack_wall(mesh: &mut Mesh, base_z: f32, settings: CliffSettings) {
    let mut upper = settings;
    upper.base_z = base_z;
    let wall = wall(upper);
    mesh.merge(&wall);
}

/// The wall-sculpting brush: undercut with a wide blade, the tool that cuts a
/// notch under a ledge.
pub fn undercut_brush(radius_m: f32) -> BrushSettings {
    BrushSettings {
        tool: ToolKind::Undercut,
        radius_m: radius_m.max(0.1),
        strength: 0.75,
        tilt_deg: 32.0,
        blade_width: 0.6,
        ..BrushSettings::default()
    }
}

/// The wall-sculpting brush: a chisel run along the bedding to deepen a seam.
pub fn seam_brush(radius_m: f32) -> BrushSettings {
    BrushSettings {
        tool: ToolKind::Chisel,
        radius_m: radius_m.max(0.1),
        strength: 0.6,
        blade_deg: 90.0,
        blade_width: 0.55,
        ..BrushSettings::default()
    }
}

/// Carve a wall along a path. Every sample is projected onto the mesh with the
/// BVH, so the stroke follows the surface instead of floating in front of it.
pub fn carve_wall(mesh: &mut Mesh, path: &[Vec3], brush: BrushSettings, seed: u64) -> StrokeReport {
    let mut bvh = Bvh::build(mesh);
    let mut stroke = SculptStroke::begin(mesh, brush, seed);
    let mut stamps = 0u32;
    for point in path {
        if let Some(hit) = bvh.closest_point(mesh, *point) {
            // Only carve the face the user pointed at, not the back of the
            // wall: skip hits whose normal faces away from the viewer.
            if hit.normal.length_squared() < 0.5 {
                continue;
            }
            if stroke.stamp(mesh, &mut bvh, hit.position, hit.normal) {
                stamps += 1;
            }
        }
    }
    let bounds = mesh.bounds();
    stroke.finish(mesh);
    StrokeReport {
        stamps,
        moved_vertices: 0,
        subdivided: 0,
        bounds,
    }
}

/// Carve a horizontal notch at `z`, the quick way to make an overhang.
pub fn notch(mesh: &mut Mesh, z: f32, depth_m: f32, seed: u64) -> usize {
    let mut bvh = Bvh::build(mesh);
    let bounds = mesh.bounds();
    let mut moved = 0;
    let mut x = bounds.min.x;
    let step = (bounds.size().x / 40.0).max(0.2);
    let brush = undercut_brush(depth_m.max(0.2));
    while x <= bounds.max.x {
        let probe = Vec3::new(x, bounds.center().y - bounds.size().y, z);
        if let Some(hit) = bvh.closest_point(mesh, probe) {
            crate::sculpt::stamp(mesh, &mut bvh, brush, hit.position, hit.normal, seed);
            moved += 1;
        }
        x += step;
    }
    moved
}

/// A wall's plan line as a gentle arc, useful for a bay or a quarry face.
pub fn arc_base(length_m: f32, sag_m: f32, segments: u32, z: f32) -> Vec<Vec3> {
    (0..=segments.max(2))
        .map(|i| {
            let t = i as f32 / segments.max(2) as f32;
            let x = (t - 0.5) * length_m;
            let y = sag_m * (t * std::f32::consts::PI).sin();
            Vec3::new(x, y, z)
        })
        .collect()
}

/// Where the wall has an overhang: returns the height bands where the face at
/// `z` is further forward than the face above it, which is the geometric
/// definition of an overhang. Used by the UI to highlight the effect.
pub fn overhang_bands(settings: &CliffSettings, samples: u32) -> Vec<(f32, f32)> {
    let mut bands = Vec::new();
    let samples = samples.max(2);
    let x = settings.length_m * 0.17;
    let mut previous = face_offset(settings, x, settings.base_z);
    let mut start = None;
    for i in 1..=samples {
        let z = settings.base_z + settings.height_m * i as f32 / samples as f32;
        let offset = face_offset(settings, x, z);
        let overhang = offset > previous + 1e-3;
        match (overhang, start) {
            (true, None) => start = Some(z),
            (false, Some(from)) => {
                bands.push((from, z));
                start = None;
            }
            _ => {}
        }
        previous = offset;
    }
    if let Some(from) = start {
        bands.push((from, settings.base_z + settings.height_m));
    }
    bands
}

/// A cheap "cliff face" normal for the sculpt cursor: the wall's front is
/// roughly -Y, but the local slope matters.
pub fn wall_normal(mesh: &Mesh, bvh: &Bvh, point: Vec3) -> Vec3 {
    bvh.closest_point(mesh, point)
        .map(|hit| hit.normal)
        .unwrap_or_else(|| {
            let (_, bitangent, normal) = basis_from_normal(Vec3::Y);
            let _ = bitangent;
            normal
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_wall_is_closed() {
        let settings = CliffSettings {
            segments: 24,
            rows: 12,
            height_m: 12.0,
            length_m: 30.0,
            ..CliffSettings::default()
        };
        let mesh = wall(settings);
        assert!(!mesh.is_empty());
        if let Some(index) = mesh.positions.iter().position(|p| !p.is_finite()) {
            panic!(
                "NaN vertex {index}: {:?} at {:?}",
                mesh.positions[index], mesh.positions[index]
            );
        }
        let stats = mesh.stats();
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        assert!(stats.volume_m3 > 50.0, "{stats:?}");
    }

    #[test]
    fn the_face_really_overhangs() {
        let settings = CliffStyle::SeaCliff.settings();
        let bands = overhang_bands(&settings, 200);
        assert!(!bands.is_empty(), "{bands:?}");
        let mesh = wall(settings);
        // Some vertices are further forward (-Y) than the base of the wall.
        let min_y = mesh
            .positions
            .iter()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);
        let base_y = mesh
            .positions
            .iter()
            .filter(|p| p.z < settings.base_z + 0.5)
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);
        assert!(min_y < base_y, "{min_y} {base_y}");
    }

    #[test]
    fn basalt_columns_are_vertical_grooves() {
        let settings = CliffStyle::BasaltColumns.settings();
        let mesh = wall(settings);
        assert!(!mesh.is_empty());
        // A columnar wall has much more area than a smooth one of the same
        // size: the grooves add surface.
        let smooth = wall(CliffSettings {
            columnar: false,
            strata_m: 0.0,
            roughness_m: 0.0,
            overhang_m: 0.0,
            ..settings
        });
        let grooved_y = mesh
            .positions
            .iter()
            .map(|p| p.y)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), y| {
                (lo.min(y), hi.max(y))
            });
        let smooth_y = smooth
            .positions
            .iter()
            .map(|p| p.y)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), y| {
                (lo.min(y), hi.max(y))
            });
        assert!(
            grooved_y.1 - grooved_y.0 > smooth_y.1 - smooth_y.0 + 0.1,
            "{grooved_y:?} {smooth_y:?}"
        );
        assert!(mesh.surface_area() >= smooth.surface_area() * 0.99);
    }

    #[test]
    fn carving_a_wall_removes_material() {
        let settings = CliffSettings {
            segments: 20,
            rows: 10,
            height_m: 8.0,
            length_m: 20.0,
            ..CliffSettings::default()
        };
        let mut mesh = wall(settings);
        let before = mesh.positions.clone();
        let path: Vec<Vec3> = (0..12)
            .map(|i| Vec3::new(-4.0 + i as f32 * 0.7, -5.0, 4.0))
            .collect();
        let report = carve_wall(&mut mesh, &path, undercut_brush(1.2), 4);
        assert!(report.stamps > 0);
        assert!(
            mesh.positions
                .iter()
                .zip(before.iter())
                .any(|(a, b)| a.distance(*b) > 1e-4)
        );
    }

    #[test]
    fn all_styles_build() {
        for style in CliffStyle::ALL {
            let settings = style.settings();
            assert!(settings.validate().is_ok(), "{style:?}");
            let mesh = wall(settings);
            assert!(!mesh.is_empty(), "{style:?}");
            assert_eq!(mesh.stats().boundary_edges, 0, "{style:?}");
        }
    }
}

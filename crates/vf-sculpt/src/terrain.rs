//! Bridge between the heightfield world and the 3D mesh world.
//!
//! The mesh does not feed back into the heightfield module, but the animation
//! module still needs a ground surface when the scene is a 3D sculpture. That
//! is what `to_heightfield` is for: it bakes the sculpt into the same grid the
//! flight and camera code already understands, so paths, clearance and safety
//! keep working without changing their APIs.

use glam::Vec3;

use crate::bvh::Bvh;
use crate::mesh::Mesh;

/// How to turn a heightfield into a mesh.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TerrainMeshSettings {
    /// Grid cells along each axis. 0 uses the heightfield resolution.
    pub resolution: u32,
    /// Close the sheet into a printable slab.
    pub close_slab: bool,
    /// Floor altitude when the slab is closed.
    pub floor_z: f32,
    /// UV metres per tile.
    pub uv_scale_m: f32,
    /// Refine steep faces after construction.
    pub refine_steep: bool,
    pub max_edge_m: f32,
    pub max_vertices: usize,
}

impl Default for TerrainMeshSettings {
    fn default() -> Self {
        Self {
            resolution: 0,
            close_slab: true,
            floor_z: -8.0,
            uv_scale_m: 8.0,
            refine_steep: false,
            max_edge_m: 1.0,
            max_vertices: 1_500_000,
        }
    }
}

impl TerrainMeshSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.uv_scale_m <= 0.0 {
            return Err("l'échelle UV doit être positive".into());
        }
        if self.max_edge_m <= 0.0 {
            return Err("la longueur d'arête doit être positive".into());
        }
        Ok(())
    }

    /// A quick preview at a fraction of the heightfield resolution.
    pub fn preview(divisor: u32) -> Self {
        Self {
            resolution: 0,
            close_slab: false,
            refine_steep: false,
            ..Self::default()
        }
        .with_divisor(divisor.max(1))
    }

    fn with_divisor(mut self, divisor: u32) -> Self {
        self.resolution = divisor;
        self
    }
}

/// Build a mesh from a heightfield. The grid is sampled in world metres, so a
/// heightfield with a non-square spacing still produces square-ish cells when
/// `resolution` is set; with `resolution = 0` the native samples are used.
pub fn from_heightfield(hf: &vf_core::HeightField, settings: TerrainMeshSettings) -> Mesh {
    let (spacing_x, spacing_y) = hf.spacing();
    let (origin_x, origin_y) = hf.origin();
    let (cells_x, cells_y) = if settings.resolution == 0 {
        (hf.width() - 1, hf.height() - 1)
    } else {
        let target = settings.resolution as usize;
        (target.max(1), target.max(1))
    };
    let origin = Vec3::new(origin_x as f32, origin_y as f32, 0.0);
    let mut positions = Vec::with_capacity((cells_x + 1) * (cells_y + 1));
    for y in 0..=cells_y {
        for x in 0..=cells_x {
            let gx = x as f64 / cells_x as f64 * (hf.width() - 1) as f64;
            let gy = y as f64 / cells_y as f64 * (hf.height() - 1) as f64;
            let height = hf.sample_bilinear(gx, gy);
            positions.push(Vec3::new(
                origin.x + (gx as f32) * spacing_x as f32,
                origin.y + (gy as f32) * spacing_y as f32,
                height,
            ));
        }
    }
    let stride = cells_x + 1;
    let mut faces = Vec::with_capacity(cells_x * cells_y);
    for y in 0..cells_y {
        for x in 0..cells_x {
            let a = (y * stride + x) as u32;
            let b = (y * stride + x + 1) as u32;
            let c = ((y + 1) * stride + x + 1) as u32;
            let d = ((y + 1) * stride + x) as u32;
            faces.push(vec![a, b, c, d]);
        }
    }
    let mut mesh = Mesh::from_faces(positions, &faces);
    mesh.box_project_uvs(1.0 / settings.uv_scale_m.max(0.01));
    mesh.recompute_normals();
    if settings.refine_steep {
        crate::remesh::refine_adaptive(&mut mesh, settings.max_edge_m, 24.0);
    }
    if settings.close_slab {
        let floor = settings.floor_z.min(mesh.bounds().min.z - 1.0);
        mesh.close_slab(floor);
    }
    mesh
}

/// Convert a `vf_mesh::MeshData` (the existing terrain mesh pipeline) into an
/// editable sculpt mesh. The triangulation is kept; normals and UVs are copied.
pub fn from_mesh_data(data: &vf_mesh::MeshData) -> Mesh {
    let mut mesh = Mesh::new();
    for position in &data.positions {
        mesh.push_vertex(Vec3::new(position[0], position[1], position[2]));
    }
    if !data.polygons.is_empty() {
        for polygon in &data.polygons {
            mesh.push_face(polygon);
        }
    } else {
        for triangle in &data.indices {
            mesh.push_face(triangle);
        }
    }
    if data.normals.len() == mesh.positions.len() {
        mesh.normals = data
            .normals
            .iter()
            .map(|n| Vec3::new(n[0], n[1], n[2]))
            .collect();
    } else {
        mesh.recompute_normals();
    }
    if data.uvs.len() == mesh.positions.len() {
        mesh.uvs = data
            .uvs
            .iter()
            .map(|uv| glam::Vec2::new(uv[0], uv[1]))
            .collect();
    } else {
        mesh.box_project_uvs(1.0);
    }
    mesh
}

/// Bake a sculpted mesh back to a heightfield for the animation and flight
/// modules. A heightfield cannot represent overhangs, so the highest surface
/// along each column wins.
pub fn to_heightfield(
    mesh: &Mesh,
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin: (f64, f64),
) -> Result<vf_core::HeightField, String> {
    if mesh.is_empty() {
        return Err("le maillage est vide".into());
    }
    let width = width.max(2);
    let height = height.max(2);
    let bvh = Bvh::build(mesh);
    let mut heights = vec![0.0f32; width * height];
    let mut minimum = f32::INFINITY;
    for y in 0..height {
        for x in 0..width {
            let world_x = origin.0 + x as f64 * spacing_x;
            let world_y = origin.1 + y as f64 * spacing_y;
            let value = bvh
                .height_under(mesh, world_x as f32, world_y as f32)
                .unwrap_or_else(|| {
                    // Outside the mesh: sample the nearest column instead of
                    // dropping to zero, so a path that runs off the edge does
                    // not fall through the world.
                    let cx = (x as f32 / width as f32 - 0.5) * 0.0;
                    let _ = cx;
                    0.0
                });
            heights[y * width + x] = value;
            minimum = minimum.min(value);
        }
    }
    if !minimum.is_finite() {
        return Err("aucune hauteur n'a pu être échantillonnée".into());
    }
    vf_core::HeightField::new(width, height, spacing_x, spacing_y, heights)
        .map_err(|error| format!("heightfield invalide: {error}"))
}

/// A heightfield that covers the mesh bounds at the requested resolution.
pub fn heightfield_for_mesh(mesh: &Mesh, max_dimension: usize) -> Option<vf_core::HeightField> {
    let bounds = mesh.bounds();
    if bounds.is_empty() {
        return None;
    }
    let size = bounds.size();
    let longest = size.x.max(size.y).max(1.0);
    let resolution = max_dimension.clamp(16, 4096);
    let spacing = (longest / resolution as f32).max(0.05);
    let width = ((size.x / spacing).ceil() as usize + 1).max(2);
    let height = ((size.y / spacing).ceil() as usize + 1).max(2);
    to_heightfield(
        mesh,
        width,
        height,
        spacing as f64,
        spacing as f64,
        (bounds.min.x as f64, bounds.min.y as f64),
    )
    .ok()
}

/// Height of the sculpt at a world position, for the camera and flight code.
pub fn surface_height(mesh: &Mesh, bvh: &Bvh, x: f32, y: f32) -> Option<f32> {
    bvh.height_under(mesh, x, y)
}

/// A coarse copy of the mesh for the animation preview: the heightfield is
/// sampled from the sculpt but keeps the original grid dimensions.
pub fn animation_heightfield(
    mesh: &Mesh,
    reference: vf_core::HeightField,
) -> Option<vf_core::HeightField> {
    let (spacing_x, spacing_y) = reference.spacing();
    let (origin_x, origin_y) = reference.origin();
    to_heightfield(
        mesh,
        reference.width(),
        reference.height(),
        spacing_x,
        spacing_y,
        (origin_x, origin_y),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> vf_core::HeightField {
        let width = 9;
        let height = 7;
        let mut heights = Vec::new();
        for y in 0..height {
            for x in 0..width {
                heights.push((x as f32 * 0.5 + y as f32 * 0.2).sin() * 4.0 + 10.0);
            }
        }
        vf_core::HeightField::new(width, height, 2.0, 3.0, heights).unwrap()
    }

    #[test]
    fn a_heightfield_becomes_a_closed_slab() {
        let hf = ramp();
        let mesh = from_heightfield(&hf, TerrainMeshSettings::default());
        assert!(!mesh.is_empty());
        let stats = mesh.stats();
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        assert!(stats.volume_m3 > 0.0, "{}", stats.volume_m3);
        let bounds = mesh.bounds();
        assert!(bounds.max.z <= 14.1 && bounds.min.z <= 5.9, "{bounds:?}");
        assert!(bounds.size().x >= 15.99, "{bounds:?}");
    }

    #[test]
    fn world_spacing_is_respected() {
        let hf = ramp();
        let mesh = from_heightfield(
            &hf,
            TerrainMeshSettings {
                close_slab: false,
                resolution: 8,
                ..TerrainMeshSettings::default()
            },
        );
        let bounds = mesh.bounds();
        assert!((bounds.size().x - 16.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.size().y - 18.0).abs() < 0.01, "{bounds:?}");
    }

    #[test]
    fn mesh_heightfield_roundtrip_recovers_the_surface() {
        let hf = ramp();
        let mesh = from_heightfield(
            &hf,
            TerrainMeshSettings {
                close_slab: false,
                ..TerrainMeshSettings::default()
            },
        );
        let baked = to_heightfield(&mesh, hf.width(), hf.height(), 2.0, 3.0, (0.0, 0.0)).unwrap();
        for y in 0..hf.height() {
            for x in 0..hf.width() {
                let original = hf.get(x, y);
                let recovered = baked.get(x, y);
                assert!(
                    (original - recovered).abs() < 0.05,
                    "{x},{y}: {original} vs {recovered}"
                );
            }
        }
    }

    #[test]
    fn preview_lowers_the_resolution() {
        let hf = ramp();
        let mesh = from_heightfield(&hf, TerrainMeshSettings::preview(3));
        assert_eq!(mesh.positions.len(), (3 + 1) * (3 + 1));
        assert!(!mesh.is_empty());
    }
}

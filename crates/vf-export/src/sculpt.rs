//! Bridge from the sculpt mesh to glTF/GLB and the other interchange formats.
//!
//! The sculpt mesh is n-gon and may carry per-vertex colours; `GltfScene`
//! expects triangles and one primitive per material. `RenderMesh` already
//! splits hard edges for correct shading, so it supplies the vertex stream and
//! the triangle list is grouped by face material.

use std::collections::BTreeMap;
use std::path::Path;

use vf_sculpt::io as sculpt_io;
use vf_sculpt::mesh::{Mesh, RenderMesh};

use crate::gltf::{GltfScene, Material};

/// Build a glTF scene from a sculpt mesh. Colours are linear RGBA and the
/// material slots become separate primitives.
pub fn scene_from_sculpt(mesh: &Mesh, name: &str) -> GltfScene {
    let render = RenderMesh::from_mesh(mesh, 35.0);
    let mut triangle_materials: Vec<u16> = Vec::with_capacity(render.indices.len());
    for face in 0..mesh.face_count() {
        let triangles = mesh.face(face).len().saturating_sub(2);
        let material = mesh.face_material(face);
        for _ in 0..triangles {
            triangle_materials.push(material);
        }
    }
    if triangle_materials.len() != render.indices.len() {
        triangle_materials.clear();
        triangle_materials.resize(render.indices.len(), 0);
    }
    let mut grouped: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
    for (triangle, material) in render.indices.iter().zip(&triangle_materials) {
        grouped
            .entry(*material)
            .or_default()
            .extend_from_slice(triangle);
    }
    let slot_count = grouped
        .keys()
        .next_back()
        .map(|m| *m as usize + 1)
        .unwrap_or(1);
    let mut materials = Vec::with_capacity(slot_count.max(1));
    for slot in 0..slot_count.max(1) {
        // Average the vertex colour of the slot for a usable base colour.
        let mut sum = [0.0f32; 3];
        let mut count = 0.0f32;
        if let Some(indices) = grouped.get(&(slot as u16)) {
            for &index in indices {
                if let Some(color) = render.colors.get(index as usize) {
                    sum[0] += f32::from(color[0]) / 255.0;
                    sum[1] += f32::from(color[1]) / 255.0;
                    sum[2] += f32::from(color[2]) / 255.0;
                    count += 1.0;
                }
            }
        }
        let base = if count > 0.0 {
            [sum[0] / count, sum[1] / count, sum[2] / count]
        } else {
            [0.72, 0.66, 0.56]
        };
        materials.push(Material {
            name: format!("{name}_slot{slot}"),
            base_color: base,
            roughness: 0.86,
            metallic: 0.0,
            kind: "rock",
        });
    }
    let primitives = grouped
        .into_iter()
        .map(|(material, indices)| (material as usize, indices))
        .collect();
    let colors: Vec<[f32; 4]> = if render.colors.len() == render.positions.len() {
        render
            .colors
            .iter()
            .map(|color| {
                [
                    f32::from(color[0]) / 255.0,
                    f32::from(color[1]) / 255.0,
                    f32::from(color[2]) / 255.0,
                    f32::from(color[3]) / 255.0,
                ]
            })
            .collect()
    } else {
        vec![[0.72, 0.66, 0.56, 1.0]; render.positions.len()]
    };
    let uv0: Vec<[f32; 2]> = if render.uvs.len() == render.positions.len() {
        render.uvs.iter().map(|uv| uv.to_array()).collect()
    } else {
        vec![[0.0, 0.0]; render.positions.len()]
    };
    GltfScene {
        positions: render.positions.iter().map(|p| p.to_array()).collect(),
        normals: render.normals.iter().map(|n| n.to_array()).collect(),
        uv0: uv0.clone(),
        uv1: uv0,
        colors,
        primitives,
        materials,
        camera: None,
        sun: None,
        generator: "VistaForge vf-sculpt".into(),
    }
}

/// Write a GLB file.
pub fn export_sculpt_glb(path: &Path, mesh: &Mesh, name: &str) -> Result<(), String> {
    let scene = scene_from_sculpt(mesh, name);
    let bytes = scene.to_glb()?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

/// Write OBJ, STL or the native `.vfm`, chosen from the extension.
pub fn export_sculpt_mesh(path: &Path, mesh: &Mesh, name: &str) -> Result<&'static str, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("vfm")
        .to_ascii_lowercase();
    match extension.as_str() {
        "glb" | "gltf" => {
            export_sculpt_glb(path, mesh, name)?;
            Ok("glb")
        }
        "obj" | "stl" | "vfm" => {
            let metadata = sculpt_io::MeshMetadata::new(name);
            sculpt_io::save_mesh(path, mesh, &metadata).map_err(|error| error.to_string())
        }
        other => Err(format!("format de maillage inconnu: .{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn quad() -> Mesh {
        let mut mesh = Mesh::from_faces(
            vec![
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(-1.0, 1.0, 0.0),
            ],
            &[vec![0, 1, 2, 3]],
        );
        mesh.ensure_colors();
        mesh.recompute_normals();
        mesh
    }

    #[test]
    fn a_quad_becomes_two_triangles() {
        let scene = scene_from_sculpt(&quad(), "test");
        assert_eq!(scene.vertex_count(), 4);
        assert_eq!(scene.triangle_count(), 2);
        assert!(scene.validate().is_ok());
    }

    #[test]
    fn glb_has_a_valid_header() {
        let path =
            std::env::temp_dir().join(format!("vf-export-sculpt-{}.glb", std::process::id()));
        export_sculpt_glb(&path, &quad(), "test").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"glTF");
        let _ = std::fs::remove_file(path);
    }
}

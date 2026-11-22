//! Mesh files: the native binary `.vfm`, Wavefront OBJ and binary STL.
//!
//! `.vfm` keeps the n-gon topology, the vertex colours (including the sculpt
//! mask in alpha) and the material slots, so a session can be saved and
//! reopened without losing anything. OBJ and STL are the interchange and
//! printing formats.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use glam::{Vec2, Vec3};

use crate::mesh::Mesh;
use crate::{Result, SculptError};

pub const VFM_MAGIC: [u8; 4] = *b"VFM1";
pub const VFM_VERSION: u32 = 1;
/// Safety limit when loading a file from an untrusted source.
pub const MAX_VERTICES: usize = 32_000_000;
pub const MAX_CORNERS: usize = 128_000_000;

/// Optional description stored with a `.vfm`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MeshMetadata {
    pub name: String,
    pub rock: String,
    pub quality: u8,
    pub seed: u64,
    pub note: String,
}

impl MeshMetadata {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

fn write_u32(writer: &mut impl Write, value: u32) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_f32(writer: &mut impl Write, value: f32) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_str(writer: &mut impl Write, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    write_u32(writer, bytes.len().min(u32::MAX as usize) as u32)?;
    writer.write_all(&bytes[..bytes.len().min(u32::MAX as usize)])
}

/// Write the native format.
pub fn write_vfm(path: &Path, mesh: &Mesh, metadata: &MeshMetadata) -> Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&VFM_MAGIC)?;
    write_u32(&mut writer, VFM_VERSION)?;
    write_u32(
        &mut writer,
        mesh.face_materials.len().min(u16::MAX as usize) as u32,
    )?;
    write_str(&mut writer, &metadata.name)?;
    write_str(&mut writer, &metadata.rock)?;
    write_u32(&mut writer, metadata.quality as u32)?;
    writer.write_all(&metadata.seed.to_le_bytes())?;
    write_str(&mut writer, &metadata.note)?;
    let vertices = mesh.positions.len();
    let faces = mesh.face_count();
    let corners = mesh.corners.len();
    if vertices > MAX_VERTICES || corners > MAX_CORNERS {
        return Err(SculptError::Unsupported(
            "le maillage dépasse les limites du format .vfm".into(),
        ));
    }
    writer.write_all(&(vertices as u64).to_le_bytes())?;
    writer.write_all(&(faces as u64).to_le_bytes())?;
    writer.write_all(&(corners as u64).to_le_bytes())?;
    for position in &mesh.positions {
        write_f32(&mut writer, position.x)?;
        write_f32(&mut writer, position.y)?;
        write_f32(&mut writer, position.z)?;
    }
    for normal in &mesh.normals {
        write_f32(&mut writer, normal.x)?;
        write_f32(&mut writer, normal.y)?;
        write_f32(&mut writer, normal.z)?;
    }
    for uv in &mesh.uvs {
        write_f32(&mut writer, uv.x)?;
        write_f32(&mut writer, uv.y)?;
    }
    if mesh.colors.len() == vertices {
        writer.write_all(&[1u8])?;
        for color in &mesh.colors {
            writer.write_all(color)?;
        }
    } else {
        writer.write_all(&[0u8])?;
    }
    for &corner in &mesh.corners {
        write_u32(&mut writer, corner)?;
    }
    for face in 0..=faces {
        let start = mesh.starts.get(face).copied().unwrap_or(corners as u32);
        write_u32(&mut writer, start)?;
    }
    for face in 0..faces {
        writer.write_all(&mesh.face_material(face).to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

/// Read the native format.
pub fn read_vfm(path: &Path) -> Result<(Mesh, MeshMetadata)> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != VFM_MAGIC {
        return Err(SculptError::Unsupported(
            "ce n'est pas un fichier .vfm".into(),
        ));
    }
    let version = read_u32(&mut reader)?;
    if version != VFM_VERSION {
        return Err(SculptError::Unsupported(format!(
            "version .vfm non prise en charge: {version}"
        )));
    }
    let material_count = read_u32(&mut reader)? as usize;
    let name = read_str(&mut reader)?;
    let rock = read_str(&mut reader)?;
    let quality = read_u32(&mut reader)? as u8;
    let seed = read_u64(&mut reader)?;
    let note = read_str(&mut reader)?;
    let vertices = read_u64(&mut reader)? as usize;
    let faces = read_u64(&mut reader)? as usize;
    let corners = read_u64(&mut reader)? as usize;
    if vertices > MAX_VERTICES || corners > MAX_CORNERS || faces > corners {
        return Err(SculptError::InvalidMesh(
            "les dimensions du fichier .vfm sont invalides".into(),
        ));
    }
    let mut mesh = Mesh::new();
    mesh.positions.reserve(vertices);
    for _ in 0..vertices {
        mesh.positions.push(Vec3::new(
            read_f32(&mut reader)?,
            read_f32(&mut reader)?,
            read_f32(&mut reader)?,
        ));
    }
    mesh.normals.reserve(vertices);
    for _ in 0..vertices {
        mesh.normals.push(Vec3::new(
            read_f32(&mut reader)?,
            read_f32(&mut reader)?,
            read_f32(&mut reader)?,
        ));
    }
    mesh.uvs.reserve(vertices);
    for _ in 0..vertices {
        mesh.uvs
            .push(Vec2::new(read_f32(&mut reader)?, read_f32(&mut reader)?));
    }
    let mut color_flag = [0u8; 1];
    reader.read_exact(&mut color_flag)?;
    if color_flag[0] == 1 {
        mesh.colors.reserve(vertices);
        for _ in 0..vertices {
            let mut color = [0u8; 4];
            reader.read_exact(&mut color)?;
            mesh.colors.push(color);
        }
    }
    mesh.corners.reserve(corners);
    for _ in 0..corners {
        mesh.corners.push(read_u32(&mut reader)?);
    }
    mesh.starts.clear();
    for _ in 0..=faces {
        mesh.starts.push(read_u32(&mut reader)?);
    }
    for _ in 0..faces.min(material_count) {
        let mut bytes = [0u8; 2];
        reader.read_exact(&mut bytes)?;
        mesh.face_materials.push(u16::from_le_bytes(bytes));
    }
    mesh.face_materials.resize(faces, 0);
    if mesh.starts.first() != Some(&0)
        || mesh.starts.last().copied().unwrap_or(0) as usize != corners
        || mesh.corners.iter().any(|&c| c as usize >= vertices)
    {
        return Err(SculptError::InvalidMesh(
            "topologie .vfm incohérente".into(),
        ));
    }
    for face in 0..faces {
        if mesh.starts[face + 1] < mesh.starts[face] + 3 {
            return Err(SculptError::InvalidMesh(
                "face .vfm avec moins de trois sommets".into(),
            ));
        }
    }
    mesh.recompute_normals();
    Ok((
        mesh,
        MeshMetadata {
            name,
            rock,
            quality,
            seed,
            note,
        },
    ))
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_f32(reader: &mut impl Read) -> std::io::Result<f32> {
    Ok(f32::from_bits(read_u32(reader)?))
}

fn read_str(reader: &mut impl Read) -> std::io::Result<String> {
    let length = read_u32(reader)? as usize;
    if length > 1_000_000 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "chaîne .vfm trop longue",
        ));
    }
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// Write a Wavefront OBJ, keeping n-gons and per-vertex normals/UVs.
pub fn write_obj(path: &Path, mesh: &Mesh, material: &str) -> Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "# VistaForge sculpt mesh")?;
    writeln!(writer, "o sculpture")?;
    for position in &mesh.positions {
        writeln!(
            writer,
            "v {:.6} {:.6} {:.6}",
            position.x, position.y, position.z
        )?;
    }
    for uv in &mesh.uvs {
        writeln!(writer, "vt {:.6} {:.6}", uv.x, uv.y)?;
    }
    for normal in &mesh.normals {
        writeln!(writer, "vn {:.6} {:.6} {:.6}", normal.x, normal.y, normal.z)?;
    }
    writeln!(writer, "usemtl {material}")?;
    for face in mesh.faces() {
        write!(writer, "f")?;
        for &corner in face {
            let index = corner as usize + 1;
            if mesh.uvs.len() == mesh.positions.len() && mesh.normals.len() == mesh.positions.len()
            {
                write!(writer, " {index}/{index}/{index}")?;
            } else {
                write!(writer, " {index}")?;
            }
        }
        writeln!(writer)?;
    }
    writer.flush()?;
    Ok(())
}

/// Read a Wavefront OBJ. Materials and smoothing groups are ignored; the
/// polygons are preserved as they are written.
pub fn read_obj(path: &Path) -> Result<Mesh> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut faces: Vec<Vec<u32>> = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => {
                let x = parse_f32(parts.next())?;
                let y = parse_f32(parts.next())?;
                let z = parse_f32(parts.next())?;
                positions.push(Vec3::new(x, y, z));
            }
            Some("vn") => {
                let x = parse_f32(parts.next())?;
                let y = parse_f32(parts.next())?;
                let z = parse_f32(parts.next())?;
                normals.push(Vec3::new(x, y, z));
            }
            Some("vt") => {
                let u = parse_f32(parts.next())?;
                let v = parse_f32(parts.next())?;
                uvs.push(Vec2::new(u, v));
            }
            Some("f") => {
                let mut face = Vec::new();
                for token in parts {
                    let first = token.split('/').next().unwrap_or(token);
                    if let Ok(index) = first.parse::<i64>() {
                        let resolved = if index > 0 {
                            index - 1
                        } else {
                            positions.len() as i64 + index
                        };
                        if resolved >= 0 && (resolved as usize) < positions.len() {
                            face.push(resolved as u32);
                        }
                    }
                }
                if face.len() >= 3 {
                    faces.push(face);
                }
            }
            _ => {}
        }
    }
    if positions.is_empty() || faces.is_empty() {
        return Err(SculptError::InvalidMesh(
            "le fichier OBJ ne contient pas de faces".into(),
        ));
    }
    let mut mesh = Mesh::from_faces(positions, &faces);
    if normals.len() == mesh.positions.len() {
        mesh.normals = normals;
    } else {
        mesh.recompute_normals();
    }
    if uvs.len() == mesh.positions.len() {
        mesh.uvs = uvs;
    } else {
        mesh.box_project_uvs(1.0);
    }
    Ok(mesh)
}

fn parse_f32(value: Option<&str>) -> Result<f32> {
    value
        .and_then(|v| v.parse::<f32>().ok())
        .ok_or_else(|| SculptError::Unsupported("nombre OBJ invalide".into()))
}

/// Write a binary STL. The mesh is triangulated; each triangle gets its flat
/// normal.
pub fn write_stl(path: &Path, mesh: &Mesh) -> Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    let mut header = [0u8; 80];
    let text = b"VistaForge sculpt";
    header[..text.len()].copy_from_slice(text);
    writer.write_all(&header)?;
    let triangles: Vec<[u32; 3]> = mesh.triangles().collect();
    write_u32(&mut writer, triangles.len() as u32)?;
    for triangle in triangles {
        let a = mesh.positions[triangle[0] as usize];
        let b = mesh.positions[triangle[1] as usize];
        let c = mesh.positions[triangle[2] as usize];
        let normal = (b - a).cross(c - a).normalize_or_zero();
        for value in [normal.x, normal.y, normal.z] {
            write_f32(&mut writer, value)?;
        }
        for point in [a, b, c] {
            write_f32(&mut writer, point.x)?;
            write_f32(&mut writer, point.y)?;
            write_f32(&mut writer, point.z)?;
        }
        writer.write_all(&[0u8, 0u8])?;
    }
    writer.flush()?;
    Ok(())
}

/// Save according to the extension. Returns the format actually written.
pub fn save_mesh(path: &Path, mesh: &Mesh, metadata: &MeshMetadata) -> Result<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("vfm")
        .to_lowercase()
        .as_str()
    {
        "vfm" => {
            write_vfm(path, mesh, metadata)?;
            Ok("vfm")
        }
        "obj" => {
            write_obj(path, mesh, "sculpt")?;
            Ok("obj")
        }
        "stl" => {
            write_stl(path, mesh)?;
            Ok("stl")
        }
        other => Err(SculptError::Unsupported(format!(
            "format de maillage inconnu: .{other}"
        ))),
    }
}

/// Load according to the extension.
pub fn load_mesh(path: &Path) -> Result<Mesh> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "vfm" => read_vfm(path).map(|(mesh, _)| mesh),
        "obj" => read_obj(path),
        other => Err(SculptError::Unsupported(format!(
            "format de maillage inconnu: .{other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.recompute_normals();
        mesh.ensure_colors();
        mesh.face_materials = (0..6).map(|i| (i % 3) as u16).collect();
        mesh
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vf-sculpt-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn vfm_roundtrip_keeps_everything() {
        let path = temp_path("roundtrip.vfm");
        let mesh = cube();
        let metadata = MeshMetadata {
            name: "cube".into(),
            rock: "granite".into(),
            quality: 2,
            seed: 42,
            note: "test".into(),
        };
        write_vfm(&path, &mesh, &metadata).unwrap();
        let (loaded, loaded_metadata) = read_vfm(&path).unwrap();
        assert_eq!(loaded.positions.len(), mesh.positions.len());
        assert_eq!(loaded.face_count(), mesh.face_count());
        assert_eq!(loaded.corners, mesh.corners);
        assert_eq!(loaded.colors, mesh.colors);
        assert_eq!(loaded.face_materials, mesh.face_materials);
        assert_eq!(loaded_metadata, metadata);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn obj_roundtrip_keeps_quads() {
        let path = temp_path("roundtrip.obj");
        let mesh = cube();
        write_obj(&path, &mesh, "rock").unwrap();
        let loaded = read_obj(&path).unwrap();
        assert_eq!(loaded.positions.len(), mesh.positions.len());
        assert_eq!(loaded.face_count(), mesh.face_count());
        assert_eq!(loaded.stats().quads, 6);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stl_has_the_triangle_count() {
        let path = temp_path("cube.stl");
        write_stl(&path, &cube()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 84);
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap());
        assert_eq!(count, 12);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn save_dispatch_rejects_unknown_extensions() {
        let path = temp_path("cube.xyz");
        let error = save_mesh(&path, &cube(), &MeshMetadata::default()).unwrap_err();
        assert!(error.to_string().contains("inconnu"));
    }
}

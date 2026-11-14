//! Minimal glTF 2.0 binary (GLB) writer.
//!
//! The writer is intentionally self-contained: no glTF crate, no hidden
//! coordinate conversion. VistaForge works in Z-up metres; glTF is Y-up with
//! `(x, y, z) -> (x, z, -y)`. Cameras look down local -Z; directional lights
//! travel along local -Z. Getting these two conventions right is the whole
//! point of the bridge.

use glam::{Mat3, Quat, Vec3};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub kind: &'static str,
}

#[derive(Debug, Clone)]
pub struct CameraNode {
    /// Absolute world position (Z-up).
    pub position: [f64; 3],
    /// Absolute world target (Z-up).
    pub target: [f64; 3],
    /// World up (Z-up).
    pub up: [f64; 3],
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Debug, Clone)]
pub struct SunNode {
    /// Unit vector pointing from the ground toward the sun (Z-up).
    pub direction: [f64; 3],
    pub color: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug, Clone)]
pub struct GltfScene {
    /// Absolute world positions (Z-up); converted by the writer.
    pub positions: Vec<[f32; 3]>,
    /// World normals (Z-up); converted by the writer.
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// Linear vertex colour, RGBA.
    pub colors: Vec<[f32; 4]>,
    /// One primitive per material: (material index, triangle indices).
    pub primitives: Vec<(usize, Vec<u32>)>,
    pub materials: Vec<Material>,
    pub camera: Option<CameraNode>,
    pub sun: Option<SunNode>,
    pub generator: String,
}

impl GltfScene {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.primitives.iter().map(|(_, indices)| indices.len() / 3).sum()
    }

    pub fn validate(&self) -> Result<(), String> {
        let n = self.positions.len();
        if n == 0 {
            return Err("no vertices".into());
        }
        if self.normals.len() != n
            || self.uv0.len() != n
            || self.uv1.len() != n
            || self.colors.len() != n
        {
            return Err("attribute arrays have different lengths".into());
        }
        for (material, indices) in &self.primitives {
            if *material >= self.materials.len() {
                return Err("primitive references an unknown material".into());
            }
            if indices.len() % 3 != 0 {
                return Err("primitive index count is not a multiple of three".into());
            }
            if indices.iter().any(|i| *i as usize >= n) {
                return Err("primitive references a missing vertex".into());
            }
        }
        Ok(())
    }

    /// Serialise to a GLB byte buffer.
    pub fn to_glb(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut builder = Builder::default();

        let positions: Vec<[f32; 3]> = self
            .positions
            .iter()
            .map(|p| [p[0], p[2], -p[1]])
            .collect();
        let normals: Vec<[f32; 3]> = self
            .normals
            .iter()
            .map(|n| [n[0], n[2], -n[1]])
            .collect();

        let (pos_accessor, _) = builder.add_vec3(&positions, true)?;
        let (normal_accessor, _) = builder.add_vec3(&normals, false)?;
        let (uv0_accessor, _) = builder.add_vec2(&self.uv0)?;
        let (uv1_accessor, _) = builder.add_vec2(&self.uv1)?;
        let (color_accessor, _) = builder.add_vec4(&self.colors)?;

        let mut primitives = Vec::with_capacity(self.primitives.len());
        for (material, indices) in &self.primitives {
            if indices.is_empty() {
                continue;
            }
            let accessor = builder.add_u32_indices(indices)?;
            primitives.push(json!({
                "attributes": {
                    "POSITION": pos_accessor,
                    "NORMAL": normal_accessor,
                    "TEXCOORD_0": uv0_accessor,
                    "TEXCOORD_1": uv1_accessor,
                    "COLOR_0": color_accessor,
                },
                "indices": accessor,
                "material": material,
                "mode": 4,
            }));
        }

        let materials: Vec<Value> = self
            .materials
            .iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "pbrMetallicRoughness": {
                        "baseColorFactor": [m.base_color[0], m.base_color[1], m.base_color[2], 1.0],
                        "metallicFactor": m.metallic,
                        "roughnessFactor": m.roughness,
                    },
                    "doubleSided": false,
                    "extras": { "vf_kind": m.kind },
                })
            })
            .collect();

        let mut nodes = vec![json!({
            "name": "VistaForge Terrain",
            "mesh": 0,
        })];
        let mut cameras = Vec::new();
        let mut lights = Vec::new();
        if let Some(camera) = &self.camera {
            let rotation = look_rotation(
                Vec3::new(
                    (camera.target[0] - camera.position[0]) as f32,
                    (camera.target[2] - camera.position[2]) as f32,
                    -(camera.target[1] - camera.position[1]) as f32,
                ),
                Vec3::new(
                    camera.up[0] as f32,
                    camera.up[2] as f32,
                    -camera.up[1] as f32,
                ),
            );
            cameras.push(json!({
                "name": "VistaForge Camera",
                "type": "perspective",
                "perspective": {
                    "yfov": f64::from(camera.fov_y_deg.to_radians()),
                    "znear": camera.near.max(0.01),
                    "zfar": camera.far.max(camera.near + 1.0),
                }
            }));
            nodes.push(json!({
                "name": "VistaForge Camera",
                "camera": 0,
                "translation": map_point(camera.position),
                "rotation": rotation,
            }));
        }
        if let Some(sun) = &self.sun {
            let direction = Vec3::new(
                sun.direction[0] as f32,
                sun.direction[2] as f32,
                -sun.direction[1] as f32,
            )
            .normalize_or_zero();
            // A directional light travels along its local -Z. The light must
            // travel from the sun toward the scene, i.e. -direction.
            let rotation = look_rotation(-direction, Vec3::Y);
            lights.push(json!({
                "name": "VistaForge Sun",
                "type": "directional",
                "color": [sun.color[0], sun.color[1], sun.color[2]],
                "intensity": (sun.intensity * 3.0).max(0.0),
            }));
            nodes.push(json!({
                "name": "VistaForge Sun",
                "rotation": rotation,
                "extensions": { "KHR_lights_punctual": { "light": 0 } },
            }));
        }

        let mut root = json!({
            "asset": { "version": "2.0", "generator": self.generator },
            "scene": 0,
            "scenes": [{ "name": "VistaForge", "nodes": (0..nodes.len()).collect::<Vec<_>>() }],
            "nodes": nodes,
            "meshes": [{ "name": "Terrain", "primitives": primitives }],
            "materials": materials,
            "accessors": builder.accessors,
            "bufferViews": builder.views,
            "buffers": [{ "byteLength": builder.bytes.len() }],
        });
        if !cameras.is_empty() {
            root["cameras"] = Value::Array(cameras);
        }
        if !lights.is_empty() {
            root["extensionsUsed"] = json!(["KHR_lights_punctual"]);
            root["extensions"] = json!({
                "KHR_lights_punctual": { "lights": lights }
            });
        }
        Ok(pack_glb(&root, &builder.bytes))
    }
}

#[derive(Default)]
struct Builder {
    bytes: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn push_view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(data);
        let mut view = json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len(),
        });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.views.push(view);
        self.views.len() - 1
    }

    fn add_accessor(
        &mut self,
        view: usize,
        component_type: u32,
        count: usize,
        kind: &str,
        min: Option<Vec<f32>>,
        max: Option<Vec<f32>>,
    ) -> usize {
        let mut accessor = json!({
            "bufferView": view,
            "componentType": component_type,
            "count": count,
            "type": kind,
        });
        if let Some(min) = min {
            accessor["min"] = json!(min);
        }
        if let Some(max) = max {
            accessor["max"] = json!(max);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    fn add_vec3(&mut self, values: &[[f32; 3]], bounds: bool) -> Result<(usize, usize), String> {
        let mut data = Vec::with_capacity(values.len() * 12);
        for v in values {
            for c in v {
                data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let view = self.push_view(&data, Some(34962));
        let (min, max) = if bounds {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for v in values {
                for i in 0..3 {
                    min[i] = min[i].min(v[i]);
                    max[i] = max[i].max(v[i]);
                }
            }
            (Some(min.to_vec()), Some(max.to_vec()))
        } else {
            (None, None)
        };
        let accessor = self.add_accessor(view, 5126, values.len(), "VEC3", min, max);
        Ok((accessor, view))
    }

    fn add_vec2(&mut self, values: &[[f32; 2]]) -> Result<(usize, usize), String> {
        let mut data = Vec::with_capacity(values.len() * 8);
        for v in values {
            for c in v {
                data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let view = self.push_view(&data, Some(34962));
        let accessor = self.add_accessor(view, 5126, values.len(), "VEC2", None, None);
        Ok((accessor, view))
    }

    fn add_vec4(&mut self, values: &[[f32; 4]]) -> Result<(usize, usize), String> {
        let mut data = Vec::with_capacity(values.len() * 16);
        for v in values {
            for c in v {
                data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let view = self.push_view(&data, Some(34962));
        let accessor = self.add_accessor(view, 5126, values.len(), "VEC4", None, None);
        Ok((accessor, view))
    }

    fn add_u32_indices(&mut self, indices: &[u32]) -> Result<usize, String> {
        let mut data = Vec::with_capacity(indices.len() * 4);
        for index in indices {
            data.extend_from_slice(&index.to_le_bytes());
        }
        let view = self.push_view(&data, Some(34963));
        Ok(self.add_accessor(view, 5125, indices.len(), "SCALAR", None, None))
    }
}

fn map_point(p: [f64; 3]) -> [f64; 3] {
    [p[0], p[2], -p[1]]
}

/// Quaternion so that local -Z points along `forward` and local +Y stays as
/// close as possible to `up`.
fn look_rotation(forward: Vec3, up: Vec3) -> [f32; 4] {
    let forward = forward.normalize_or_zero();
    if forward.length_squared() < 0.5 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let right = forward.cross(up).normalize_or_zero();
    let right = if right.length_squared() < 0.5 {
        forward.cross(Vec3::X).normalize_or_zero()
    } else {
        right
    };
    let up = right.cross(forward).normalize_or_zero();
    let matrix = Mat3::from_cols(right, up, -forward);
    let q = Quat::from_mat3(&matrix);
    [q.x, q.y, q.z, q.w]
}

fn pack_glb(json_doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(json_doc).expect("glTF JSON serialises");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin_bytes = bin.to_vec();
    while !bin_bytes.len().is_multiple_of(4) {
        bin_bytes.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // glTF
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // JSON
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // BIN\0
    out.extend_from_slice(&bin_bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_scene() -> GltfScene {
        GltfScene {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uv0: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            uv1: vec![[0.0, 0.0]; 3],
            colors: vec![[1.0, 1.0, 1.0, 1.0]; 3],
            primitives: vec![(0, vec![0, 1, 2])],
            materials: vec![Material {
                name: "vf_rock".into(),
                base_color: [0.4, 0.35, 0.3],
                roughness: 0.9,
                metallic: 0.0,
                kind: "rock",
            }],
            camera: None,
            sun: None,
            generator: "VistaForge test".into(),
        }
    }

    #[test]
    fn glb_has_valid_header_and_chunks() {
        let glb = triangle_scene().to_glb().unwrap();
        assert_eq!(glb.len() % 4, 0);
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
        let total = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.len());
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(&glb[16..20], b"JSON");
        let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        assert_eq!(json["meshes"][0]["primitives"][0]["mode"], 4);
        assert_eq!(json["accessors"][0]["type"], "VEC3");
    }

    #[test]
    fn coordinate_conversion_is_z_up_to_y_up() {
        let mut scene = triangle_scene();
        scene.positions[0] = [1.0, 2.0, 3.0];
        let glb = scene.to_glb().unwrap();
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let bin_start = 20 + json_len + 8;
        let x = f32::from_le_bytes(glb[bin_start..bin_start + 4].try_into().unwrap());
        let y = f32::from_le_bytes(glb[bin_start + 4..bin_start + 8].try_into().unwrap());
        let z = f32::from_le_bytes(glb[bin_start + 8..bin_start + 12].try_into().unwrap());
        assert_eq!((x, y, z), (1.0, 3.0, -2.0));
        assert_eq!(json["asset"]["version"], "2.0");
    }
}

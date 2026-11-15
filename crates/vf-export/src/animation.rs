//! Camera/target animation interchange without terrain geometry.
//!
//! Three open formats are written:
//!
//! * `.vfcam` — a documented JSON document containing the complete
//!   [`CameraRig`] (paths, target, optics, motion profile, constraints). This is
//!   the lossless VistaForge round-trip.
//! * `.glb` — a minimal glTF 2.0 scene with a perspective camera, a target
//!   empty and two translation/rotation animations. Blender imports it with its
//!   built-in glTF importer; the full rig is also embedded in `extras` so
//!   VistaForge can re-import the exact recipe. No terrain mesh is included.
//! * `.csv` — one row per frame (eye, target, roll, FOV, speed), readable by
//!   spreadsheets, tracking software and custom pipelines.
//!
//! A `.glb` produced by Blender (camera + optional target empty, animated) can
//! be imported back: translations become the eye path, and camera rotations are
//! converted into a target path so the original orientation is preserved.

use std::fs;
use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vf_core::{CameraRig, HeightField, MotionProfile, Path3D, PathKey, Seed, TargetMode};
use vf_flight::rig::RigSimulation;

use crate::{ExportError, Result};

pub const ANIMATION_FORMAT: &str = "vistaforge.camera-animation.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerrainReference {
    pub width: usize,
    pub height: usize,
    pub spacing_x: f64,
    pub spacing_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub min_height: f32,
    pub max_height: f32,
    pub center: [f64; 3],
}

impl TerrainReference {
    pub fn from_heightfield(hf: &HeightField) -> Self {
        let stats = hf.stats();
        let (sx, sy) = hf.spacing();
        let (ox, oy) = hf.origin();
        Self {
            width: hf.width(),
            height: hf.height(),
            spacing_x: sx,
            spacing_y: sy,
            origin_x: ox,
            origin_y: oy,
            min_height: stats.min,
            max_height: stats.max,
            center: [
                ox + (hf.width() - 1) as f64 * sx * 0.5,
                oy + (hf.height() - 1) as f64 * sy * 0.5,
                f64::from((stats.min + stats.max) * 0.5),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraAnimationFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub rig: CameraRig,
    #[serde(default)]
    pub terrain: Option<TerrainReference>,
}

#[derive(Debug, Clone)]
pub struct ImportedAnimation {
    pub rig: CameraRig,
    pub terrain: Option<TerrainReference>,
    pub source: String,
    pub format: &'static str,
}

#[derive(Debug, Clone)]
pub struct AnimationReport {
    pub path: PathBuf,
    pub format: &'static str,
    pub frames: usize,
    pub duration_s: f32,
    pub has_target: bool,
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    time: f32,
    eye: [f64; 3],
    target: [f64; 3],
    up: [f64; 3],
    roll_deg: f32,
    fov_y_deg: f32,
    speed_mps: f32,
}

/// Export the current rig to `.vfcam`, `.glb` or `.csv`. The heightfield is
/// only used for a small terrain reference block; no terrain samples are
/// written to the animation file.
pub fn export_animation(
    path: &Path,
    rig: &CameraRig,
    heightfield: &HeightField,
    seed: Seed,
) -> Result<AnimationReport> {
    rig.validate().map_err(ExportError::Animation)?;
    let frames = sample_frames(rig, heightfield, seed)?;
    if frames.len() < 2 {
        return Err(ExportError::Animation(
            "animation needs at least two frames".into(),
        ));
    }
    let extension = extension(path);
    let format = match extension.as_str() {
        "vfcam" | "json" => {
            write_vfcam(path, rig, heightfield)?;
            "vfcam JSON"
        }
        "glb" => {
            write_glb(path, rig, heightfield, &frames)?;
            "glTF binary (camera + target)"
        }
        "csv" => {
            write_csv(path, &frames)?;
            "CSV frames"
        }
        other => {
            return Err(ExportError::Animation(format!(
                "unsupported animation extension '{other}' (use .vfcam, .glb or .csv)"
            )));
        }
    };
    Ok(AnimationReport {
        path: path.to_path_buf(),
        format,
        frames: frames.len(),
        duration_s: frames.last().map_or(0.0, |frame| frame.time),
        has_target: rig.target.mode != TargetMode::Manual,
    })
}

/// Import a `.vfcam`, `.glb` or `.csv` camera animation. The terrain is left
/// untouched by the caller.
pub fn import_animation(path: &Path) -> Result<ImportedAnimation> {
    let extension = extension(path);
    match extension.as_str() {
        "vfcam" | "json" => read_vfcam(path),
        "glb" => read_glb(path),
        "csv" => read_csv(path),
        other => Err(ExportError::Animation(format!(
            "unsupported animation extension '{other}' (use .vfcam, .glb or .csv)"
        ))),
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn write_vfcam(path: &Path, rig: &CameraRig, hf: &HeightField) -> Result<()> {
    let document = CameraAnimationFile {
        format: ANIMATION_FORMAT.into(),
        version: 1,
        name: rig.name.clone(),
        rig: rig.clone(),
        terrain: Some(TerrainReference::from_heightfield(hf)),
    };
    let bytes = serde_json::to_vec_pretty(&document)?;
    write_atomic(path, &bytes)?;
    Ok(())
}

fn read_vfcam(path: &Path) -> Result<ImportedAnimation> {
    let text = fs::read_to_string(path)?;
    let document: CameraAnimationFile = serde_json::from_str(&text)?;
    if document.format != ANIMATION_FORMAT {
        return Err(ExportError::Animation(format!(
            "unknown animation format '{}' (expected {ANIMATION_FORMAT})",
            document.format
        )));
    }
    let mut rig = document.rig;
    rig.enabled = true;
    rig.validate().map_err(ExportError::Animation)?;
    Ok(ImportedAnimation {
        rig,
        terrain: document.terrain,
        source: path.display().to_string(),
        format: "vfcam JSON",
    })
}

fn sample_frames(rig: &CameraRig, hf: &HeightField, seed: Seed) -> Result<Vec<Frame>> {
    let fps = rig.fps.clamp(1, 120);
    let frames = ((rig.duration_s * fps as f32).round() as usize).clamp(2, 20_000);
    let mut simulation = RigSimulation::new(rig, hf, seed).map_err(ExportError::Animation)?;
    let mut out = Vec::with_capacity(frames + 1);
    for index in 0..=frames {
        let t = (index as f32 / fps as f32).min(rig.duration_s);
        let sample = simulation.sample_at(t);
        out.push(Frame {
            time: t,
            eye: sample.eye.to_array(),
            target: sample.target.to_array(),
            up: sample.camera.up.to_array(),
            roll_deg: sample.roll_deg,
            fov_y_deg: sample.camera.fov_y_deg,
            speed_mps: sample.speed_mps,
        });
    }
    Ok(out)
}

fn write_csv(path: &Path, frames: &[Frame]) -> Result<()> {
    let mut text = String::from(
        "time_s,eye_x,eye_y,eye_z,target_x,target_y,target_z,up_x,up_y,up_z,roll_deg,fov_y_deg,speed_mps\n",
    );
    for frame in frames {
        text.push_str(&format!(
            "{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.5},{:.5},{:.5},{:.3},{:.3},{:.3}\n",
            frame.time,
            frame.eye[0],
            frame.eye[1],
            frame.eye[2],
            frame.target[0],
            frame.target[1],
            frame.target[2],
            frame.up[0],
            frame.up[1],
            frame.up[2],
            frame.roll_deg,
            frame.fov_y_deg,
            frame.speed_mps
        ));
    }
    write_atomic(path, text.as_bytes())?;
    Ok(())
}

fn read_csv(path: &Path) -> Result<ImportedAnimation> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| ExportError::Animation("empty CSV".into()))?;
    let headers: Vec<String> = header_line
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    let column = |name: &str| headers.iter().position(|header| header == name);
    let time_col = column("time_s")
        .or_else(|| column("time"))
        .ok_or_else(|| ExportError::Animation("CSV has no time_s column".into()))?;
    let eye_cols = [
        column("eye_x").ok_or_else(|| ExportError::Animation("CSV has no eye_x".into()))?,
        column("eye_y").ok_or_else(|| ExportError::Animation("CSV has no eye_y".into()))?,
        column("eye_z").ok_or_else(|| ExportError::Animation("CSV has no eye_z".into()))?,
    ];
    let target_cols = match (column("target_x"), column("target_y"), column("target_z")) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    let roll_col = column("roll_deg");
    let fov_col = column("fov_y_deg");
    let mut eye_keys = Vec::new();
    let mut target_keys = Vec::new();
    let mut last_roll = 0.0f32;
    let mut last_fov = 50.0f32;
    for line in lines {
        let fields: Vec<&str> = line.split(',').collect();
        let read = |index: usize| -> Option<f64> {
            fields
                .get(index)
                .and_then(|value| value.trim().parse::<f64>().ok())
        };
        let Some(time) = read(time_col) else {
            continue;
        };
        let Some(x) = read(eye_cols[0]) else {
            continue;
        };
        let Some(y) = read(eye_cols[1]) else {
            continue;
        };
        let Some(z) = read(eye_cols[2]) else {
            continue;
        };
        if !time.is_finite() || time < 0.0 || [x, y, z].iter().any(|v| !v.is_finite()) {
            continue;
        }
        let time = time as f32;
        eye_keys.push(PathKey::new(time, [x, y, z]));
        if let Some(cols) = target_cols
            && let (Some(tx), Some(ty), Some(tz)) = (read(cols[0]), read(cols[1]), read(cols[2]))
            && [tx, ty, tz].iter().all(|v| v.is_finite())
        {
            target_keys.push(PathKey::new(time, [tx, ty, tz]));
        }
        if let Some(roll) = roll_col.and_then(read) {
            last_roll = roll as f32;
        }
        if let Some(fov) = fov_col.and_then(read) {
            last_fov = fov as f32;
        }
    }
    if eye_keys.len() < 2 {
        return Err(ExportError::Animation(
            "CSV must contain at least two valid frames".into(),
        ));
    }
    eye_keys.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    target_keys.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    let duration = eye_keys.last().map_or(8.0, |key| key.time_s.max(0.1));
    let mut rig = CameraRig {
        enabled: true,
        name: "Imported CSV flyover".into(),
        duration_s: duration,
        eye: Path3D {
            keys: eye_keys,
            closed: false,
        },
        ..CameraRig::default()
    };
    rig.body = MotionProfile::preset(vf_core::AircraftPreset::Manual);
    rig.constraints.enabled = false;
    rig.optics.fov_y_deg = last_fov.clamp(1.0, 170.0);
    rig.optics.roll_deg = last_roll;
    if target_keys.len() >= 2 {
        rig.target.mode = TargetMode::Path;
        rig.target.path.keys = target_keys;
    } else {
        rig.target.mode = TargetMode::Manual;
    }
    rig.validate().map_err(ExportError::Animation)?;
    Ok(ImportedAnimation {
        rig,
        terrain: None,
        source: path.display().to_string(),
        format: "CSV frames",
    })
}

fn map_point(point: [f64; 3]) -> [f64; 3] {
    // Z-up metres -> glTF Y-up metres.
    [point[0], point[2], -point[1]]
}

fn map_vector(vector: Vec3) -> Vec3 {
    Vec3::new(vector.x, vector.z, -vector.y)
}

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
    let matrix = glam::Mat3::from_cols(right, up, -forward);
    let q = Quat::from_mat3(&matrix);
    [q.x, q.y, q.z, q.w]
}

#[derive(Default)]
struct Builder {
    bytes: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn push_view(&mut self, data: &[u8]) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(data);
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len(),
        }));
        self.views.len() - 1
    }

    fn add_accessor(
        &mut self,
        view: usize,
        kind: &str,
        count: usize,
        min: Option<Vec<f32>>,
        max: Option<Vec<f32>>,
    ) -> usize {
        let mut accessor = json!({
            "bufferView": view,
            "componentType": 5126,
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

    fn add_f32(&mut self, values: &[f32], kind: &str, bounds: bool) -> usize {
        let mut data = Vec::with_capacity(values.len() * 4);
        for value in values {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let view = self.push_view(&data);
        let components = match kind {
            "SCALAR" => 1,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => 1,
        };
        let (min, max) = if bounds {
            let mut min = vec![f32::INFINITY; components];
            let mut max = vec![f32::NEG_INFINITY; components];
            for (index, value) in values.iter().enumerate() {
                let component = index % components;
                min[component] = min[component].min(*value);
                max[component] = max[component].max(*value);
            }
            (Some(min), Some(max))
        } else {
            (None, None)
        };
        let count = values.len() / components;
        self.add_accessor(view, kind, count, min, max)
    }
}

fn write_glb(path: &Path, rig: &CameraRig, hf: &HeightField, frames: &[Frame]) -> Result<()> {
    let mut builder = Builder::default();
    let times: Vec<f32> = frames.iter().map(|frame| frame.time).collect();
    let mut eye_translations = Vec::with_capacity(frames.len() * 3);
    let mut eye_rotations = Vec::with_capacity(frames.len() * 4);
    let mut target_translations = Vec::with_capacity(frames.len() * 3);
    for frame in frames {
        let position = map_point(frame.eye);
        let target = map_point(frame.target);
        eye_translations.extend_from_slice(&[
            position[0] as f32,
            position[1] as f32,
            position[2] as f32,
        ]);
        target_translations.extend_from_slice(&[
            target[0] as f32,
            target[1] as f32,
            target[2] as f32,
        ]);
        let forward = Vec3::new(
            (frame.target[0] - frame.eye[0]) as f32,
            (frame.target[2] - frame.eye[2]) as f32,
            -(frame.target[1] - frame.eye[1]) as f32,
        );
        let up = map_vector(Vec3::new(
            frame.up[0] as f32,
            frame.up[1] as f32,
            frame.up[2] as f32,
        ));
        eye_rotations.extend_from_slice(&look_rotation(forward, up));
    }
    let time_accessor = builder.add_f32(&times, "SCALAR", true);
    let eye_translation_accessor = builder.add_f32(&eye_translations, "VEC3", false);
    let eye_rotation_accessor = builder.add_f32(&eye_rotations, "VEC4", false);
    let target_translation_accessor = builder.add_f32(&target_translations, "VEC3", false);

    let first_eye = &eye_translations[..3];
    let first_rotation = &eye_rotations[..4];
    let first_target = &target_translations[..3];
    let rig_json = serde_json::to_value(rig)?;
    let terrain_json = serde_json::to_value(TerrainReference::from_heightfield(hf))?;
    let document = json!({
        "asset": { "version": "2.0", "generator": "VistaForge camera animation" },
        "scene": 0,
        "scenes": [{
            "name": "VistaForge Flyover",
            "nodes": [0, 1],
            "extras": {
                "vistaforge_animation_format": ANIMATION_FORMAT,
                "vistaforge_rig": rig_json,
                "vistaforge_terrain": terrain_json,
            }
        }],
        "nodes": [
            {
                "name": "VistaForge Camera",
                "camera": 0,
                "translation": [first_eye[0], first_eye[1], first_eye[2]],
                "rotation": [first_rotation[0], first_rotation[1], first_rotation[2], first_rotation[3]],
                "extras": { "vf_role": "camera" }
            },
            {
                "name": "VistaForge Target",
                "translation": [first_target[0], first_target[1], first_target[2]],
                "extras": { "vf_role": "target" }
            }
        ],
        "cameras": [{
            "type": "perspective",
            "perspective": {
                "yfov": f64::from(rig.optics.fov_y_deg.to_radians()),
                "znear": rig.optics.near.max(0.01),
                "zfar": rig.optics.far.max(rig.optics.near + 1.0),
            }
        }],
        "animations": [{
            "name": "VistaForge Camera and Target",
            "samplers": [
                { "input": time_accessor, "output": eye_translation_accessor, "interpolation": "LINEAR" },
                { "input": time_accessor, "output": eye_rotation_accessor, "interpolation": "LINEAR" },
                { "input": time_accessor, "output": target_translation_accessor, "interpolation": "LINEAR" }
            ],
            "channels": [
                { "sampler": 0, "target": { "node": 0, "path": "translation" } },
                { "sampler": 1, "target": { "node": 0, "path": "rotation" } },
                { "sampler": 2, "target": { "node": 1, "path": "translation" } }
            ]
        }],
        "accessors": builder.accessors,
        "bufferViews": builder.views,
        "buffers": [{ "byteLength": builder.bytes.len() }]
    });
    let glb = pack_glb(&document, &builder.bytes)?;
    write_atomic(path, &glb)?;
    Ok(())
}

fn pack_glb(document: &Value, binary: &[u8]) -> Result<Vec<u8>> {
    let mut json_bytes = serde_json::to_vec(document)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut binary = binary.to_vec();
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + binary.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&binary);
    Ok(out)
}

struct Glb {
    document: Value,
    binary: Vec<u8>,
}

fn parse_glb(bytes: &[u8]) -> Result<Glb> {
    if bytes.len() < 20 {
        return Err(ExportError::Animation("GLB is truncated".into()));
    }
    if &bytes[0..4] != b"glTF" {
        return Err(ExportError::Animation("not a GLB file".into()));
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != 2 {
        return Err(ExportError::Animation(format!(
            "unsupported GLB version {version}"
        )));
    }
    let mut offset = 12;
    let mut document = None;
    let mut binary = Vec::new();
    while offset + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let start = offset + 8;
        if start + length > bytes.len() {
            return Err(ExportError::Animation("GLB chunk is truncated".into()));
        }
        let chunk = &bytes[start..start + length];
        if kind == b"JSON" {
            document = Some(serde_json::from_slice::<Value>(chunk)?);
        } else if kind == b"BIN\0" {
            binary = chunk.to_vec();
        }
        offset = start + length;
        while offset % 4 != 0 && offset < bytes.len() {
            offset += 1;
        }
    }
    let document =
        document.ok_or_else(|| ExportError::Animation("GLB has no JSON chunk".into()))?;
    Ok(Glb { document, binary })
}

fn accessor_values(document: &Value, binary: &[u8], index: usize) -> Result<Vec<f32>> {
    let accessor = document
        .get("accessors")
        .and_then(|accessors| accessors.get(index))
        .ok_or_else(|| ExportError::Animation(format!("missing accessor {index}")))?;
    if accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        != 5126
    {
        return Err(ExportError::Animation(
            "only float accessors are supported".into(),
        ));
    }
    let count = accessor.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
    let kind = accessor
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("SCALAR");
    let components = match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => {
            return Err(ExportError::Animation(format!(
                "unsupported accessor type {kind}"
            )));
        }
    };
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| ExportError::Animation("accessor has no bufferView".into()))?
        as usize;
    let view = document
        .get("bufferViews")
        .and_then(|views| views.get(view_index))
        .ok_or_else(|| ExportError::Animation("missing bufferView".into()))?;
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let accessor_offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .map_or(components * 4, |stride| stride as usize);
    let start = view_offset + accessor_offset;
    let mut out = Vec::with_capacity(count * components);
    for element in 0..count {
        let base = start + element * stride;
        if base + components * 4 > binary.len() {
            return Err(ExportError::Animation(
                "accessor points outside the GLB buffer".into(),
            ));
        }
        for component in 0..components {
            let offset = base + component * 4;
            out.push(f32::from_le_bytes(
                binary[offset..offset + 4].try_into().unwrap(),
            ));
        }
    }
    Ok(out)
}

fn node_index_with_role(document: &Value, role: &str) -> Option<usize> {
    document
        .get("nodes")
        .and_then(Value::as_array)?
        .iter()
        .enumerate()
        .find(|(_, node)| {
            node.get("extras")
                .and_then(|extras| extras.get("vf_role"))
                .and_then(Value::as_str)
                .is_some_and(|value| value == role)
                || node
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| {
                        name.to_ascii_lowercase()
                            .contains(&role.to_ascii_lowercase())
                    })
        })
        .map(|(index, _)| index)
}

type GltfTrack = (Vec<f32>, Vec<f32>);
type GltfChannels = (Option<GltfTrack>, Option<GltfTrack>);

fn animation_channels(document: &Value, binary: &[u8], node: usize) -> GltfChannels {
    let mut translation = None;
    let mut rotation = None;
    let Some(animation) = document
        .get("animations")
        .and_then(Value::as_array)
        .and_then(|animations| animations.first())
    else {
        return (None, None);
    };
    let Some(channels) = animation.get("channels").and_then(Value::as_array) else {
        return (None, None);
    };
    let Some(samplers) = animation.get("samplers").and_then(Value::as_array) else {
        return (None, None);
    };
    for channel in channels {
        let Some(target) = channel.get("target") else {
            continue;
        };
        if target.get("node").and_then(Value::as_u64) != Some(node as u64) {
            continue;
        }
        let path = target.get("path").and_then(Value::as_str).unwrap_or("");
        let Some(sampler_index) = channel.get("sampler").and_then(Value::as_u64) else {
            continue;
        };
        let Some(sampler) = samplers.get(sampler_index as usize) else {
            continue;
        };
        let Some(input) = sampler.get("input").and_then(Value::as_u64) else {
            continue;
        };
        let Some(output) = sampler.get("output").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(times) = accessor_values(document, binary, input as usize) else {
            continue;
        };
        let Ok(values) = accessor_values(document, binary, output as usize) else {
            continue;
        };
        if path == "translation" {
            translation = Some((times, values));
        } else if path == "rotation" {
            rotation = Some((times, values));
        }
    }
    (translation, rotation)
}

fn read_glb(path: &Path) -> Result<ImportedAnimation> {
    let bytes = fs::read(path)?;
    let glb = parse_glb(&bytes)?;
    let scene_extras = glb
        .document
        .get("scenes")
        .and_then(|scenes| scenes.get(0))
        .and_then(|scene| scene.get("extras"));
    if let Some(rig_value) = scene_extras.and_then(|extras| extras.get("vistaforge_rig")) {
        let mut rig: CameraRig = serde_json::from_value(rig_value.clone())?;
        rig.enabled = true;
        rig.validate().map_err(ExportError::Animation)?;
        let terrain = scene_extras
            .and_then(|extras| extras.get("vistaforge_terrain"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        return Ok(ImportedAnimation {
            rig,
            terrain,
            source: path.display().to_string(),
            format: "glTF binary (VistaForge extras)",
        });
    }

    // Generic Blender/glTF camera animation.
    let camera_node = glb
        .document
        .get("nodes")
        .and_then(Value::as_array)
        .and_then(|nodes| nodes.iter().position(|node| node.get("camera").is_some()))
        .ok_or_else(|| ExportError::Animation("GLB has no camera node".into()))?;
    let target_node = node_index_with_role(&glb.document, "target");
    let (times, eye_values) = animation_channels(&glb.document, &glb.binary, camera_node)
        .0
        .ok_or_else(|| {
            ExportError::Animation(
                "GLB camera has no translation animation (export the camera with keyframes)".into(),
            )
        })?;
    let rotations = animation_channels(&glb.document, &glb.binary, camera_node).1;
    let target_values =
        target_node.and_then(|node| animation_channels(&glb.document, &glb.binary, node).0);
    let frames = times.len().min(eye_values.len() / 3);
    if frames < 2 {
        return Err(ExportError::Animation(
            "GLB camera animation has fewer than two frames".into(),
        ));
    }
    let mut eye_keys = Vec::with_capacity(frames);
    let mut target_keys = Vec::with_capacity(frames);
    let fov = glb
        .document
        .get("cameras")
        .and_then(|cameras| cameras.get(0))
        .and_then(|camera| camera.get("perspective"))
        .and_then(|perspective| perspective.get("yfov"))
        .and_then(Value::as_f64)
        .map_or(50.0f32, |yfov| (yfov.to_degrees()) as f32);
    for index in 0..frames {
        let time = times[index].max(0.0);
        let eye_gltf = [
            eye_values[index * 3],
            eye_values[index * 3 + 1],
            eye_values[index * 3 + 2],
        ];
        // glTF Y-up -> Z-up.
        let eye = [
            f64::from(eye_gltf[0]),
            f64::from(-eye_gltf[2]),
            f64::from(eye_gltf[1]),
        ];
        eye_keys.push(PathKey::new(time, eye));
        let target = if let Some((target_times, target_values)) = &target_values {
            sample_vec3(target_times, target_values, times[index]).map_or(eye, |p| {
                [f64::from(p[0]), f64::from(-p[2]), f64::from(p[1])]
            })
        } else if let Some((rotation_times, rotation_values)) = &rotations {
            sample_rotation_forward(rotation_times, rotation_values, times[index]).map_or(
                eye,
                |forward| {
                    [
                        eye[0] + forward[0] * 500.0,
                        eye[1] + forward[1] * 500.0,
                        eye[2] + forward[2] * 500.0,
                    ]
                },
            )
        } else {
            eye
        };
        target_keys.push(PathKey::new(time, target));
    }
    // Avoid a degenerate target path (Blender exports without a target empty).
    if target_keys
        .iter()
        .all(|key| key.position == target_keys[0].position)
    {
        target_keys.clear();
    }
    let duration = eye_keys.last().map_or(8.0, |key| key.time_s.max(0.1));
    let mut rig = CameraRig {
        enabled: true,
        name: "Imported Blender camera".into(),
        duration_s: duration,
        eye: Path3D {
            keys: eye_keys,
            closed: false,
        },
        ..CameraRig::default()
    };
    rig.body = MotionProfile::preset(vf_core::AircraftPreset::Manual);
    rig.constraints.enabled = false;
    rig.optics.fov_y_deg = fov.clamp(1.0, 170.0);
    if target_keys.len() >= 2 {
        rig.target.mode = TargetMode::Path;
        rig.target.path.keys = target_keys;
    } else {
        rig.target.mode = TargetMode::Manual;
    }
    rig.validate().map_err(ExportError::Animation)?;
    Ok(ImportedAnimation {
        rig,
        terrain: None,
        source: path.display().to_string(),
        format: "glTF binary (camera tracks)",
    })
}

fn sample_vec3(times: &[f32], values: &[f32], time: f32) -> Option<[f32; 3]> {
    let frames = times.len().min(values.len() / 3);
    if frames == 0 {
        return None;
    }
    let index = times[..frames]
        .iter()
        .position(|sample| *sample >= time)
        .unwrap_or(frames - 1);
    let i = index.saturating_sub(1);
    let j = index;
    let span = (times[j] - times[i]).max(1e-6);
    let f = ((time - times[i]) / span).clamp(0.0, 1.0);
    let a = [values[i * 3], values[i * 3 + 1], values[i * 3 + 2]];
    let b = [values[j * 3], values[j * 3 + 1], values[j * 3 + 2]];
    Some([
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ])
}

fn sample_rotation_forward(times: &[f32], rotations: &[f32], time: f32) -> Option<[f64; 3]> {
    let frames = times.len().min(rotations.len() / 4);
    if frames == 0 {
        return None;
    }
    let index = times[..frames]
        .iter()
        .position(|sample| *sample >= time)
        .unwrap_or(frames - 1);
    let i = index.saturating_sub(1);
    let j = index;
    let span = (times[j] - times[i]).max(1e-6);
    let f = ((time - times[i]) / span).clamp(0.0, 1.0);
    let qa = Quat::from_xyzw(
        rotations[i * 4],
        rotations[i * 4 + 1],
        rotations[i * 4 + 2],
        rotations[i * 4 + 3],
    );
    let qb = Quat::from_xyzw(
        rotations[j * 4],
        rotations[j * 4 + 1],
        rotations[j * 4 + 2],
        rotations[j * 4 + 3],
    );
    let q = qa.slerp(qb, f).normalize();
    // glTF cameras look down local -Z. Convert the result to Z-up.
    let forward = q * Vec3::new(0.0, 0.0, -1.0);
    let forward = Vec3::new(forward.x, -forward.z, forward.y).normalize_or_zero();
    Some([
        f64::from(forward.x),
        f64::from(forward.y),
        f64::from(forward.z),
    ])
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("anim")
    ));
    fs::write(&tmp, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{HeightField, PathKey};

    fn field() -> HeightField {
        let heights = (0..129 * 129)
            .map(|index| {
                let x = (index % 129) as f32;
                let y = (index / 129) as f32;
                100.0 + x * 2.0 + (y * 0.2).sin() * 20.0
            })
            .collect();
        HeightField::new(129, 129, 10.0, 10.0, heights).unwrap()
    }

    fn rig() -> CameraRig {
        let mut rig = CameraRig::preset("drone").unwrap();
        rig.eye.keys = vec![
            PathKey::new(0.0, [200.0, 200.0, 250.0]),
            PathKey::new(3.0, [500.0, 400.0, 300.0]),
            PathKey::new(6.0, [800.0, 700.0, 250.0]),
        ];
        rig.duration_s = 6.0;
        rig.fps = 24;
        rig.target.mode = TargetMode::Path;
        rig.target.path.keys = vec![
            PathKey::new(0.0, [500.0, 500.0, 100.0]),
            PathKey::new(3.0, [600.0, 600.0, 100.0]),
            PathKey::new(6.0, [700.0, 700.0, 100.0]),
        ];
        rig
    }

    fn temp_path(name: &str, extension: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("vf-anim-{name}-{nanos}.{extension}"))
    }

    #[test]
    fn vfcam_round_trip_is_lossless() {
        let hf = field();
        let rig = rig();
        let path = temp_path("roundtrip", "vfcam");
        let report = export_animation(&path, &rig, &hf, Seed::new(2)).unwrap();
        assert!(report.frames > 100);
        let imported = import_animation(&path).unwrap();
        assert_eq!(imported.rig.eye.keys.len(), 3);
        assert_eq!(imported.rig.duration_s, 6.0);
        assert!(imported.terrain.is_some());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn glb_contains_camera_animation_and_extras() {
        let hf = field();
        let rig = rig();
        let path = temp_path("camera", "glb");
        export_animation(&path, &rig, &hf, Seed::new(3)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let glb = parse_glb(&bytes).unwrap();
        assert!(glb.document["cameras"].is_array());
        assert!(glb.document["animations"][0]["channels"].is_array());
        assert_eq!(
            glb.document["scenes"][0]["extras"]["vistaforge_animation_format"],
            ANIMATION_FORMAT
        );
        let imported = import_animation(&path).unwrap();
        assert_eq!(imported.rig.eye.keys.len(), 3);
        assert!((imported.rig.duration_s - 6.0).abs() < 1e-4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn generic_blender_glb_without_extras_imports_camera_tracks() {
        let hf = field();
        let rig = rig();
        let path = temp_path("generic", "glb");
        export_animation(&path, &rig, &hf, Seed::new(5)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut glb = parse_glb(&bytes).unwrap();
        // Simulate a GLB produced by Blender: no VistaForge extras.
        if let Some(scenes) = glb.document.get_mut("scenes").and_then(Value::as_array_mut)
            && let Some(scene) = scenes.first_mut()
            && let Some(extras) = scene.get_mut("extras").and_then(Value::as_object_mut)
        {
            extras.remove("vistaforge_rig");
            extras.remove("vistaforge_animation_format");
            extras.remove("vistaforge_terrain");
        }
        let rewritten = pack_glb(&glb.document, &glb.binary).unwrap();
        std::fs::write(&path, rewritten).unwrap();
        let imported = import_animation(&path).unwrap();
        assert!(imported.rig.eye.keys.len() > 100);
        assert!(
            imported.rig.target.mode == TargetMode::Path
                || imported.rig.target.mode == TargetMode::Manual
        );
        assert!((imported.rig.eye.keys[0].position[0] - 200.0).abs() < 10.0);
        assert!((imported.rig.eye.keys[0].position[2] - 250.0).abs() < 10.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn camera_only_glb_rebuilds_the_aim_from_rotations() {
        let hf = field();
        let rig = rig();
        let path = temp_path("camera-only", "glb");
        export_animation(&path, &rig, &hf, Seed::new(6)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut glb = parse_glb(&bytes).unwrap();
        // Remove VistaForge extras and the target animation channel, as a
        // Blender export that only animated the camera would.
        if let Some(scene) = glb
            .document
            .get_mut("scenes")
            .and_then(Value::as_array_mut)
            .and_then(|scenes| scenes.first_mut())
            && let Some(extras) = scene.get_mut("extras").and_then(Value::as_object_mut)
        {
            extras.clear();
        }
        if let Some(channels) = glb
            .document
            .get_mut("animations")
            .and_then(Value::as_array_mut)
            .and_then(|animations| animations.first_mut())
            .and_then(|animation| animation.get_mut("channels"))
            .and_then(Value::as_array_mut)
        {
            channels.retain(|channel| {
                channel
                    .get("target")
                    .and_then(|target| target.get("node"))
                    .and_then(Value::as_u64)
                    != Some(1)
            });
        }
        let rewritten = pack_glb(&glb.document, &glb.binary).unwrap();
        std::fs::write(&path, rewritten).unwrap();
        let imported = import_animation(&path).unwrap();
        assert_eq!(imported.rig.target.mode, TargetMode::Path);
        let first = imported.rig.target.path.keys.first().unwrap();
        let eye = imported.rig.eye.keys.first().unwrap();
        let distance = ((first.position[0] - eye.position[0]).powi(2)
            + (first.position[1] - eye.position[1]).powi(2)
            + (first.position[2] - eye.position[2]).powi(2))
        .sqrt();
        assert!((distance - 500.0).abs() < 5.0, "aim distance {distance}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn csv_export_import_preserves_path() {
        let hf = field();
        let rig = rig();
        let path = temp_path("frames", "csv");
        export_animation(&path, &rig, &hf, Seed::new(4)).unwrap();
        let imported = import_animation(&path).unwrap();
        assert!(imported.rig.eye.keys.len() > 100);
        assert_eq!(imported.rig.target.mode, TargetMode::Path);
        assert!((imported.rig.eye.keys[0].position[0] - 200.0).abs() < 5.0);
        let _ = std::fs::remove_file(path);
    }
}

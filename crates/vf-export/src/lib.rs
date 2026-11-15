//! Export a VistaForge scene to glTF/GLB and to a Blender-ready directory.
//!
//! The exporter writes the *actual* render mesh (continuous detail included),
//! not the stored DEM: what you see in the rasteriser is what Blender imports.
//! Vertex colours carry the material masks, material slots separate the
//! families a texture artist wants to work on, and a JSON sidecar carries the
//! non-mesh scene state (sun, camera, palette, height map, render settings).

pub mod animation;
pub mod gltf;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vf_core::{HeightField, Palette, Project};
use vf_detail::DetailSampler;
use vf_mesh::MeshData;

pub use animation::{
    ANIMATION_FORMAT, AnimationReport, CameraAnimationFile, ImportedAnimation, TerrainReference,
    export_animation, import_animation,
};
pub use gltf::{CameraNode, GltfScene, Material, SunNode};

/// Export parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOptions {
    /// Mesh density: 1 = stored grid, 2..4 = subdivided.
    pub subdivision: u32,
    /// Vertex budget for the export mesh.
    pub max_vertices: usize,
    /// Include a sea plane.
    pub water: bool,
    /// Include the camera node.
    pub camera: bool,
    /// Include the directional sun.
    pub sun: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            subdivision: 2,
            max_vertices: 4_000_000,
            water: true,
            camera: true,
            sun: true,
        }
    }
}

/// What was written.
#[derive(Debug, Clone)]
pub struct ExportReport {
    pub glb: PathBuf,
    pub sidecar: PathBuf,
    pub heightmap: PathBuf,
    pub albedo: PathBuf,
    pub normal: PathBuf,
    pub masks: PathBuf,
    pub vertices: usize,
    pub triangles: usize,
    pub materials: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("glTF error: {0}")]
    Gltf(String),
    #[error("project has no terrain")]
    NoTerrain,
    #[error("surface pipeline error: {0}")]
    Pipeline(String),
    #[error("animation error: {0}")]
    Animation(String),
}

pub type Result<T> = std::result::Result<T, ExportError>;

const MATERIAL_ROCK: usize = 0;
const MATERIAL_STRATA_A: usize = 1;
const MATERIAL_SCREE: usize = 2;
const MATERIAL_SNOW: usize = 3;
const MATERIAL_STRATA_B: usize = 4;
const MATERIAL_VEGETATION: usize = 5;
const MATERIAL_WATER: usize = 6;
const MATERIAL_COUNT: usize = 7;

/// Export a project and its current heightfield into `out_dir`.
pub fn export_project(
    project: &Project,
    hf: &HeightField,
    out_dir: &Path,
    options: &ExportOptions,
) -> Result<ExportReport> {
    if hf.width() < 2 || hf.height() < 2 {
        return Err(ExportError::NoTerrain);
    }
    fs::create_dir_all(out_dir)?;

    // Export the same surface the renderer sees: modifiers and the cliff pass
    // are part of the scene, not a renderer-only trick.
    let export_hf = prepare_surface(project, hf)?;
    let sampler = DetailSampler::new(&export_hf, &project.detail, project.seed);
    let mesh = vf_mesh::build(
        &sampler,
        options.subdivision.clamp(1, 8),
        options.max_vertices.max(1024),
        None,
    );
    let built = build_scene(project, &export_hf, &mesh, options)?;
    let scene = &built.scene;
    let glb = scene.to_glb().map_err(ExportError::Gltf)?;
    let glb_path = out_dir.join("scene.glb");
    fs::write(&glb_path, glb)?;

    let heightmap_path = out_dir.join("heightmap_16.png");
    write_heightmap(&export_hf, &heightmap_path)?;
    let textures = write_texture_maps(
        project,
        &export_hf,
        &mesh,
        &built.albedo,
        &built.masks,
        out_dir,
    )?;
    let sidecar_path = out_dir.join("scene.json");
    let sidecar = build_sidecar(
        project,
        &export_hf,
        scene,
        &options.clone(),
        &heightmap_path,
    )
    .map_err(ExportError::Json)?;
    fs::write(&sidecar_path, serde_json::to_vec_pretty(&sidecar)?)?;

    let readme = format!(
        "VistaForge Blender export\n\
         =========================\n\n\
         scene.glb   mesh, vertex colours, material slots, camera, sun\n\
         scene.json  palette, sun, camera, units, height map, render settings\n\
         albedo.png  baked albedo (sRGB, UV0)\n\
         normal.png  baked height-derived normal map (UV0)\n\
         masks.png   R=snow, G=vegetation, B=scree (non-color)\n\
         heightmap_16.png  reference height field (16-bit)\n\n\
         Import with Blender 4.x/5.x:\n\n\
         blender --background --python bridge/blender/vistaforge_import.py -- \\\n\
           {} {} --render render.png\n\n\
         Or open Blender and run the script from the Scripting workspace.\n",
        glb_path.display(),
        sidecar_path.display()
    );
    fs::write(out_dir.join("README.txt"), readme)?;

    Ok(ExportReport {
        glb: glb_path,
        sidecar: sidecar_path,
        heightmap: heightmap_path,
        albedo: textures.albedo,
        normal: textures.normal,
        masks: textures.masks,
        vertices: scene.vertex_count(),
        triangles: scene.triangle_count(),
        materials: scene.materials.len(),
    })
}

fn prepare_surface(project: &Project, hf: &HeightField) -> Result<HeightField> {
    let modulated = if project.modifiers.active().next().is_some() {
        vf_modifiers::apply(hf, &project.modifiers, project.seed)
            .map_err(|e| ExportError::Pipeline(format!("modifier: {e}")))?
    } else {
        hf.clone()
    };
    if project.render.cliffs {
        vf_procgen::cliff::sharpen(
            &modulated,
            project.seed,
            project.render.cliff_warp,
            project.render.cliff_warp_scale_m,
        )
        .map_err(|e| ExportError::Pipeline(format!("cliff shaping: {e}")))
    } else {
        Ok(modulated)
    }
}

struct BuiltScene {
    scene: GltfScene,
    albedo: Vec<[f32; 3]>,
    masks: Vec<Masks>,
}

fn build_scene(
    project: &Project,
    hf: &HeightField,
    mesh: &MeshData,
    options: &ExportOptions,
) -> Result<BuiltScene> {
    let stats = hf.stats();
    let relief = (stats.max - stats.min).max(1e-6);
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let ex = (hf.width() - 1) as f64 * sx;
    let ey = (hf.height() - 1) as f64 * sy;

    let mut positions = mesh.positions.clone();
    let mut normals = mesh.normals.clone();
    let mut uv0 = mesh.uvs.clone();
    let mut colors = Vec::with_capacity(mesh.vertex_count());
    let mut albedo = Vec::with_capacity(mesh.vertex_count());
    let mut vertex_masks = Vec::with_capacity(mesh.vertex_count());
    for (index, position) in mesh.positions.iter().enumerate() {
        let normal = mesh.normals[index];
        let masks = masks_at(
            project,
            f64::from(position[0]),
            f64::from(position[1]),
            position[2],
            normal,
            stats.min,
            relief,
        );
        let color = blend_albedo(&project.palette, masks, normal);
        // glTF multiplies COLOR_0 by the material baseColorFactor. Export the
        // per-vertex colour as a *relative* multiplier around 1.0 so the
        // product recovers the rasteriser albedo instead of darkening it.
        let material = material_for(masks, project);
        let base = material_base_color(material, &project.palette);
        let multiplier = [
            (color[0] / base[0].max(0.02)).clamp(0.35, 1.8),
            (color[1] / base[1].max(0.02)).clamp(0.35, 1.8),
            (color[2] / base[2].max(0.02)).clamp(0.35, 1.8),
        ];
        colors.push([multiplier[0], multiplier[1], multiplier[2], 1.0]);
        albedo.push(color);
        vertex_masks.push(masks);
    }

    let mut uv1 = box_atlas_uvs(&mesh.positions, &mesh.normals);

    let mut primitives: Vec<Vec<u32>> = vec![Vec::new(); MATERIAL_COUNT];
    for [a, b, c] in &mesh.indices {
        let ma = vertex_masks[*a as usize];
        let mb = vertex_masks[*b as usize];
        let mc = vertex_masks[*c as usize];
        let masks = Masks {
            snow: (ma.snow + mb.snow + mc.snow) / 3.0,
            vegetation: (ma.vegetation + mb.vegetation + mc.vegetation) / 3.0,
            scree: (ma.scree + mb.scree + mc.scree) / 3.0,
            strata: (ma.strata + mb.strata + mc.strata) / 3.0,
        };
        let material = material_for(masks, project);
        primitives[material].extend_from_slice(&[*a, *b, *c]);
    }

    // Water: four corners of a large sea plane at the project sea level.
    if options.water && project.water.enabled {
        let sea = project.water.sea_level;
        let half_x = (ex * 3.0).max(1000.0);
        let half_y = (ey * 3.0).max(1000.0);
        let cx = ox + ex * 0.5;
        let cy = oy + ey * 0.5;
        let base = mesh.vertex_count() as u32;
        let corners = [
            (cx - half_x, cy - half_y),
            (cx + half_x, cy - half_y),
            (cx - half_x, cy + half_y),
            (cx + half_x, cy + half_y),
        ];
        for (index, (x, y)) in corners.into_iter().enumerate() {
            positions.push([x as f32, y as f32, sea]);
            normals.push([0.0, 0.0, 1.0]);
            uv0.push(if index % 2 == 0 {
                [0.0, 0.0]
            } else {
                [1.0, 0.0]
            });
            uv1.push([0.0, 0.0]);
            colors.push([1.0, 1.0, 1.0, 1.0]);
            albedo.push(project.palette.water_deep.to_array());
        }
        primitives[MATERIAL_WATER].extend_from_slice(&[
            base,
            base + 1,
            base + 2,
            base + 1,
            base + 3,
            base + 2,
        ]);
    }

    let materials = build_materials(&project.palette);

    let camera = if options.camera {
        Some(CameraNode {
            position: project.camera.position.to_array(),
            target: project.camera.target.to_array(),
            up: project.camera.up.to_array(),
            fov_y_deg: project.camera.fov_y_deg,
            near: project.camera.near,
            far: project.camera.far,
        })
    } else {
        None
    };
    let sun_dir = project.sun.direction();
    let sun = if options.sun {
        Some(SunNode {
            direction: sun_dir.to_array(),
            color: project.sun.color().to_array(),
            intensity: project.sun.intensity,
        })
    } else {
        None
    };

    let primitives = primitives
        .into_iter()
        .enumerate()
        .filter(|(_, indices)| !indices.is_empty())
        .collect();

    Ok(BuiltScene {
        scene: GltfScene {
            positions,
            normals,
            uv0,
            uv1,
            colors,
            primitives,
            materials,
            camera,
            sun,
            generator: format!("VistaForge {}", vf_core::SCHEMA_VERSION),
        },
        albedo,
        masks: vertex_masks,
    })
}

/// Per-vertex material masks, shared by colour and material assignment.
#[derive(Debug, Clone, Copy, Default)]
struct Masks {
    snow: f32,
    vegetation: f32,
    scree: f32,
    strata: f32,
}

fn masks_at(
    project: &Project,
    x: f64,
    y: f64,
    z: f32,
    normal: [f32; 3],
    min: f32,
    relief: f32,
) -> Masks {
    let rel = ((z - min) / relief).clamp(0.0, 1.0);
    let slope = (1.0 - normal[2]).clamp(0.0, 1.0);
    let style = &project.style;
    let mats = &project.materials;
    let snow_line = ((style.snow_line_m - min) / relief).clamp(0.0, 1.0);
    let tree_line = ((style.tree_line_m - min) / relief).clamp(0.0, 1.0);
    let snow = smoothstep(snow_line - 0.10, snow_line + 0.04, rel)
        * (1.0 - smoothstep(mats.snow_slope + 0.08, mats.snow_slope + 0.25, slope))
        * mats.snow_coverage.clamp(0.0, 1.0);
    let vegetation = (1.0 - smoothstep(tree_line - 0.04, tree_line + 0.10, rel))
        * (1.0 - smoothstep(0.18, 0.42, slope))
        * mats.vegetation.clamp(0.0, 1.0);
    let scree = smoothstep(mats.scree_slope - 0.10, mats.scree_slope + 0.15, slope)
        * (1.0 - smoothstep(0.68, 0.90, slope))
        * mats.scree.clamp(0.0, 1.0);
    let strata = if project.geology.enabled && project.geology.strata.enabled {
        let sample = vf_geology::strata_sample(
            &project.geology.strata,
            x,
            y,
            z,
            project.seed.derive("strata").0,
        );
        sample.band
    } else {
        0.5
    };
    Masks {
        snow,
        vegetation,
        scree,
        strata,
    }
}

fn blend_albedo(palette: &Palette, masks: Masks, normal: [f32; 3]) -> [f32; 3] {
    let slope = (1.0 - normal[2]).clamp(0.0, 1.0);
    let rock = palette.rock.lerp(palette.cliff, 0.55 * slope);
    let rock = rock.lerp(palette.sand, 0.22 * (masks.strata - 0.5).abs() * 2.0);
    let vegetation = palette.grass.lerp(palette.forest, 0.45);
    let color = rock
        .lerp(palette.sand, masks.scree * 0.55)
        .lerp(vegetation, masks.vegetation * 0.65)
        .lerp(palette.snow, masks.snow);
    color.to_array()
}

fn material_for(masks: Masks, project: &Project) -> usize {
    // Keep rock as the default: the rasteriser tints rock with vegetation and
    // scree, it does not replace it. Material slots are for texture painting,
    // so only the dominant families get their own slot.
    if masks.snow > 0.58 {
        return MATERIAL_SNOW;
    }
    if masks.vegetation > 0.82 {
        return MATERIAL_VEGETATION;
    }
    if masks.scree > 0.72 {
        return MATERIAL_SCREE;
    }
    if project.geology.enabled && project.geology.strata.enabled {
        if masks.strata > 0.62 {
            return MATERIAL_STRATA_B;
        }
        if masks.strata < 0.38 {
            return MATERIAL_STRATA_A;
        }
    }
    MATERIAL_ROCK
}

/// Per-vertex box projection packed into six atlas cells (3×2). Planar UV0
/// stays the bake UV; UV1 gives a non-stretched starting point for cliffs.
fn box_atlas_uvs(positions: &[[f32; 3]], normals: &[[f32; 3]]) -> Vec<[f32; 2]> {
    let mut axes = vec![0u8; positions.len()];
    let mut bounds = [[
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ]; 6];
    for (index, normal) in normals.iter().enumerate() {
        let abs = [normal[0].abs(), normal[1].abs(), normal[2].abs()];
        let axis = if abs[0] >= abs[1] && abs[0] >= abs[2] {
            if normal[0] >= 0.0 { 0 } else { 1 }
        } else if abs[1] >= abs[2] {
            if normal[1] >= 0.0 { 2 } else { 3 }
        } else if normal[2] >= 0.0 {
            4
        } else {
            5
        };
        axes[index] = axis;
        let (a, b) = project_axis(positions[index], axis);
        let cell = &mut bounds[axis as usize];
        cell[0] = cell[0].min(a);
        cell[1] = cell[1].max(a);
        cell[2] = cell[2].min(b);
        cell[3] = cell[3].max(b);
    }
    let margin = 0.025f32;
    let cell_w = 1.0 / 3.0;
    let cell_h = 1.0 / 2.0;
    let mut uvs = Vec::with_capacity(positions.len());
    for (index, position) in positions.iter().enumerate() {
        let axis = axes[index];
        let (a, b) = project_axis(*position, axis);
        let bounds = bounds[axis as usize];
        let span_a = (bounds[1] - bounds[0]).max(1e-6);
        let span_b = (bounds[3] - bounds[2]).max(1e-6);
        let col = axis % 3;
        let row = axis / 3;
        let u = col as f32 * cell_w
            + margin
            + ((a - bounds[0]) / span_a).clamp(0.0, 1.0) * (cell_w - 2.0 * margin);
        let v = row as f32 * cell_h
            + margin
            + ((b - bounds[2]) / span_b).clamp(0.0, 1.0) * (cell_h - 2.0 * margin);
        uvs.push([u, v]);
    }
    uvs
}

fn project_axis(position: [f32; 3], axis: u8) -> (f32, f32) {
    match axis {
        0 | 1 => (position[1], position[2]),
        2 | 3 => (position[0], position[2]),
        _ => (position[0], position[1]),
    }
}

fn material_base_color(material: usize, palette: &Palette) -> [f32; 3] {
    match material {
        MATERIAL_STRATA_A => palette.cliff.to_array(),
        MATERIAL_SCREE => palette.sand.to_array(),
        MATERIAL_SNOW => palette.snow.to_array(),
        MATERIAL_STRATA_B => palette.rock.scale(0.72).lerp(palette.cliff, 0.5).to_array(),
        MATERIAL_VEGETATION => palette.grass.lerp(palette.forest, 0.45).to_array(),
        MATERIAL_WATER => palette.water_deep.to_array(),
        _ => palette.rock.to_array(),
    }
}

fn build_materials(palette: &Palette) -> Vec<Material> {
    vec![
        Material {
            name: "vf_rock".into(),
            base_color: palette.rock.to_array(),
            roughness: 0.92,
            metallic: 0.0,
            kind: "rock",
        },
        Material {
            name: "vf_strata_a".into(),
            base_color: palette.cliff.to_array(),
            roughness: 0.90,
            metallic: 0.0,
            kind: "strata",
        },
        Material {
            name: "vf_scree".into(),
            base_color: palette.sand.to_array(),
            roughness: 1.0,
            metallic: 0.0,
            kind: "scree",
        },
        Material {
            name: "vf_snow".into(),
            base_color: palette.snow.to_array(),
            roughness: 0.62,
            metallic: 0.0,
            kind: "snow",
        },
        Material {
            name: "vf_strata_b".into(),
            base_color: palette.rock.scale(0.72).lerp(palette.cliff, 0.5).to_array(),
            roughness: 0.88,
            metallic: 0.0,
            kind: "strata",
        },
        Material {
            name: "vf_vegetation".into(),
            base_color: palette.grass.lerp(palette.forest, 0.45).to_array(),
            roughness: 0.95,
            metallic: 0.0,
            kind: "vegetation",
        },
        Material {
            name: "vf_water".into(),
            base_color: palette.water_deep.to_array(),
            roughness: 0.08,
            metallic: 0.0,
            kind: "water",
        },
    ]
}

struct TexturePaths {
    albedo: PathBuf,
    normal: PathBuf,
    masks: PathBuf,
}

fn write_texture_maps(
    project: &Project,
    hf: &HeightField,
    mesh: &MeshData,
    albedo: &[[f32; 3]],
    masks: &[Masks],
    out_dir: &Path,
) -> Result<TexturePaths> {
    let _ = project;
    let w = mesh.nx.max(2);
    let h = mesh.ny.max(2);
    let factor = ((w.max(h) as f32) / 4096.0).ceil().max(1.0) as usize;
    let tw = (w / factor).max(2);
    let th = (h / factor).max(2);
    let (sx, sy) = hf.spacing();
    let mut albedo_img = image::RgbImage::new(tw as u32, th as u32);
    let mut normal_img = image::RgbImage::new(tw as u32, th as u32);
    let mut mask_img = image::RgbImage::new(tw as u32, th as u32);
    for oy in 0..th {
        // Flip: Blender samples UV (0,0) at the bottom-left of the image.
        let iy = (th - 1 - oy) * (h - 1) / (th.max(2) - 1).max(1);
        for ox in 0..tw {
            let ix = ox * (w - 1) / (tw.max(2) - 1).max(1);
            let index = iy.min(mesh.ny - 1) * mesh.nx + ix.min(mesh.nx - 1);
            let color = albedo.get(index).copied().unwrap_or([0.5, 0.5, 0.5]);
            let rgb = vf_core::Rgb::new(color[0], color[1], color[2]).to_srgb_u8();
            albedo_img.put_pixel(ox as u32, oy as u32, image::Rgb(rgb));

            // Height-derived normal in the top-down UV frame. Cliff UVs
            // stretch, so this is a starting map, not a tangent-perfect bake.
            let x0 = ix.saturating_sub(1).min(mesh.nx - 1);
            let x1 = (ix + 1).min(mesh.nx - 1);
            let y0 = iy.saturating_sub(1).min(mesh.ny - 1);
            let y1 = (iy + 1).min(mesh.ny - 1);
            let zx = mesh.positions[iy * mesh.nx + x1][2] - mesh.positions[iy * mesh.nx + x0][2];
            let zy = mesh.positions[y1 * mesh.nx + ix][2] - mesh.positions[y0 * mesh.nx + ix][2];
            let dx = zx / ((x1 - x0).max(1) as f32 * sx as f32);
            let dy = zy / ((y1 - y0).max(1) as f32 * sy as f32);
            let len = (dx * dx + dy * dy + 1.0).sqrt();
            let n = [
                (-dx / len) * 0.5 + 0.5,
                (-dy / len) * 0.5 + 0.5,
                (1.0 / len) * 0.5 + 0.5,
            ];
            normal_img.put_pixel(
                ox as u32,
                oy as u32,
                image::Rgb([
                    (n[0] * 255.0 + 0.5) as u8,
                    (n[1] * 255.0 + 0.5) as u8,
                    (n[2] * 255.0 + 0.5) as u8,
                ]),
            );

            let mask = masks.get(index).copied().unwrap_or_default();
            mask_img.put_pixel(
                ox as u32,
                oy as u32,
                image::Rgb([
                    (mask.snow.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                    (mask.vegetation.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                    (mask.scree.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                ]),
            );
        }
    }
    let albedo_path = out_dir.join("albedo.png");
    let normal_path = out_dir.join("normal.png");
    let mask_path = out_dir.join("masks.png");
    albedo_img.save(&albedo_path)?;
    normal_img.save(&normal_path)?;
    mask_img.save(&mask_path)?;
    Ok(TexturePaths {
        albedo: albedo_path,
        normal: normal_path,
        masks: mask_path,
    })
}

fn write_heightmap(hf: &HeightField, path: &Path) -> Result<()> {
    let stats = hf.stats();
    let range = (stats.max - stats.min).max(1e-6);
    let (w, h) = (hf.width() as u32, hf.height() as u32);
    let mut image = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::new(w, h);
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            let value = ((hf.get(x, y) - stats.min) / range).clamp(0.0, 1.0);
            let pixel = (value * 65535.0 + 0.5) as u16;
            // Row 0 is south in the heightfield; keep that convention and
            // document it in the sidecar.
            image.put_pixel(x as u32, y as u32, image::Luma([pixel]));
        }
    }
    image.save(path)?;
    Ok(())
}

fn build_sidecar(
    project: &Project,
    hf: &HeightField,
    scene: &GltfScene,
    options: &ExportOptions,
    heightmap: &Path,
) -> serde_json::Result<Value> {
    let stats = hf.stats();
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let palette = &project.palette;
    let sun_dir = project.sun.direction();
    let (dof_enabled, dof_focus, dof_fstop) = if project.rig.enabled {
        let focus = if project.rig.optics.focus_on_target {
            (project.camera.target - project.camera.position).length() as f32
        } else {
            project.rig.optics.focus_distance_m
        };
        (
            project.rig.optics.dof_enabled,
            focus.max(0.1),
            project.rig.optics.fstop,
        )
    } else {
        (false, 100.0, 8.0)
    };
    let materials: Vec<Value> = scene
        .materials
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "kind": m.kind,
                "base_color": m.base_color,
                "roughness": m.roughness,
            })
        })
        .collect();
    Ok(json!({
        "schema": "vistaforge.blender/1",
        "generator": format!("VistaForge {}", vf_core::SCHEMA_VERSION),
        "project": {
            "name": project.meta.name,
            "author": project.meta.author,
            "seed": project.seed.0,
            "units": project.meta.units,
            "crs": project.meta.crs,
        },
        "terrain": {
            "width": hf.width(),
            "height": hf.height(),
            "spacing_x": sx,
            "spacing_y": sy,
            "origin": [ox, oy],
            "min_m": stats.min,
            "max_m": stats.max,
            "relief_m": stats.relief(),
            "heightmap": heightmap.file_name().and_then(|n| n.to_str()).unwrap_or("heightmap_16.png"),
            "heightmap_row_zero": "south",
            "heightmap_range": [stats.min, stats.max],
        },
        "mesh": {
            "vertices": scene.vertex_count(),
            "triangles": scene.triangle_count(),
            "subdivision": options.subdivision,
            "uv0": "planar top-down",
            "uv1": "box atlas, six projections (stitch seams in Blender if needed)",
        },
        "textures": {
            "albedo": "albedo.png",
            "normal": "normal.png",
            "masks": "masks.png",
            "heightmap": "heightmap_16.png",
            "mask_channels": "R=snow, G=vegetation, B=scree",
            "uv_origin": "UV (0,0) is south-west; PNG rows are flipped for Blender",
        },
        "camera": {
            "position": project.camera.position.to_array(),
            "target": project.camera.target.to_array(),
            "up": project.camera.up.to_array(),
            "fov_y_deg": project.camera.fov_y_deg,
            "near": project.camera.near,
            "far": project.camera.far,
            "dof": { "enabled": dof_enabled, "focus_m": dof_focus, "fstop": dof_fstop },
        },
        "sun": {
            "direction": sun_dir.to_array(),
            "color": project.sun.color().to_array(),
            "intensity": project.sun.intensity,
            "color_k": project.sun.color_k,
            "angular_radius_deg": project.sun.angular_radius_deg,
        },
        "palette": {
            "name": palette.name,
            "sky_zenith": palette.sky_zenith.to_array(),
            "sky_horizon": palette.sky_horizon.to_array(),
            "fog": palette.fog.to_array(),
            "sun": palette.sun.to_array(),
            "water_shallow": palette.water_shallow.to_array(),
            "water_deep": palette.water_deep.to_array(),
            "snow": palette.snow.to_array(),
        },
        "render": {
            "exposure": project.render.exposure,
            "quality": format!("{:?}", project.render.quality),
        },
        "detail": {
            "enabled": project.detail.enabled,
            "profile": project.detail.profile,
            "scale_m": project.detail.scale_m,
            "amplitude_m": project.detail.amplitude_m,
            "heterogeneity": project.detail.heterogeneity,
        },
        "geology": {
            "preset": project.geology.preset,
            "strata_enabled": project.geology.strata.enabled,
        },
        "materials": materials,
    }))
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{ClassicParams, DetailSettings, Seed};

    fn test_project() -> (Project, HeightField) {
        let mut project = Project::new("Export test", Seed::new(7), 65);
        project.source = vf_core::TerrainSource::Classic {
            params: ClassicParams::default().with_size_seed(65, 7),
        };
        project.detail = DetailSettings::preset("granite").unwrap();
        let hf =
            vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(65, 7)).unwrap();
        project.sync_terrain(&hf);
        (project, hf)
    }

    #[test]
    fn exports_glb_sidecar_and_heightmap() {
        let (project, hf) = test_project();
        let dir = std::env::temp_dir().join(format!("vf-export-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let report = export_project(&project, &hf, &dir, &ExportOptions::default()).unwrap();
        assert!(report.glb.exists());
        assert!(report.sidecar.exists());
        assert!(report.heightmap.exists());
        assert!(report.triangles > 0);
        let glb = fs::read(&report.glb).unwrap();
        assert_eq!(&glb[0..4], b"glTF");
        let sidecar: Value = serde_json::from_slice(&fs::read(&report.sidecar).unwrap()).unwrap();
        assert_eq!(sidecar["schema"], "vistaforge.blender/1");
        assert!(sidecar["terrain"]["relief_m"].as_f64().unwrap() > 0.0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_is_deterministic() {
        let (project, hf) = test_project();
        let a = build_scene(
            &project,
            &hf,
            &vf_mesh::build(
                &DetailSampler::new(&hf, &project.detail, project.seed),
                1,
                1_000_000,
                None,
            ),
            &ExportOptions::default(),
        )
        .unwrap()
        .scene;
        let b = build_scene(
            &project,
            &hf,
            &vf_mesh::build(
                &DetailSampler::new(&hf, &project.detail, project.seed),
                1,
                1_000_000,
                None,
            ),
            &ExportOptions::default(),
        )
        .unwrap()
        .scene;
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.colors, b.colors);
        assert_eq!(a.primitives, b.primitives);
    }
}

//! Deterministic CPU landscape renderer.
//!
//! The renderer rasterises the heightfield as a triangle mesh with a z-buffer,
//! adds a procedural sky with a 2.5D cloud layer, a water plane with Fresnel
//! reflection, horizon-based ambient occlusion and a sun shadow map, then
//! tone-maps the linear HDR buffer to 8-bit sRGB.
//!
//! It is intentionally dependency-light and deterministic: no GPU, no clock,
//! no random sampling. The same project renders the same image everywhere.

mod raster;
mod shade;
mod shadow;

use std::time::Instant;

use glam::{Mat4, Vec3, Vec4};
use rayon::prelude::*;
use vf_core::{HeightField, Project, RenderSettings, Tonemap};
use vf_io::RgbaImage;
use vf_procgen::{Noise, value2};

pub(crate) use shade::ShadeCtx;

const TILE: usize = 64;

/// Upper bound for the render mesh. Beyond this the builder coarsens the grid
/// instead of failing; the detail field still carries the surface story.
const MAX_MESH_VERTICES: usize = 6_000_000;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid render options: {0}")]
    Invalid(String),
    #[error("image error: {0}")]
    Image(#[from] vf_io::IoError),
}

pub type Result<T> = std::result::Result<T, RenderError>;

/// Render parameters. Usually derived from [`RenderSettings`].
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    /// Supersampling factor (1..=3).
    pub ssaa: u8,
    pub shadows: bool,
    pub shadow_resolution: u32,
    pub ao: bool,
    pub cliffs: bool,
    pub clouds: bool,
    pub water: bool,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub tonemap: Tonemap,
    /// Mesh density: 1 = stored grid, 2..4 = subdivided.
    pub subdivision: u8,
    /// Evaluate the continuous rock detail on the mesh.
    pub detail_geometry: bool,
    /// Screen-adaptive block LOD; false builds a uniform grid.
    pub adaptive: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::from(&RenderSettings::default())
    }
}

impl From<&RenderSettings> for RenderOptions {
    fn from(settings: &RenderSettings) -> Self {
        Self {
            width: settings.width,
            height: settings.height,
            ssaa: settings.ssaa,
            shadows: settings.shadows,
            shadow_resolution: settings.shadow_resolution,
            ao: settings.ao,
            cliffs: settings.cliffs,
            clouds: settings.clouds,
            water: settings.water,
            exposure: settings.exposure,
            contrast: settings.contrast,
            saturation: settings.saturation,
            gamma: settings.gamma,
            tonemap: settings.tonemap,
            subdivision: settings.subdivision,
            detail_geometry: true,
            adaptive: settings.adaptive,
        }
    }
}

impl RenderOptions {
    /// Fast interactive preview settings.
    pub fn preview(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            ssaa: 1,
            shadows: false,
            shadow_resolution: 1024,
            ao: false,
            cliffs: false,
            clouds: true,
            water: true,
            exposure: 1.0,
            contrast: 0.02,
            saturation: 1.05,
            gamma: 2.2,
            tonemap: Tonemap::Aces,
            subdivision: 1,
            detail_geometry: true,
            adaptive: true,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 || self.width > 16_384 || self.height > 16_384 {
            return Err(RenderError::Invalid(
                "width and height must be in 1..=16384".into(),
            ));
        }
        if !(1..=3).contains(&self.ssaa) {
            return Err(RenderError::Invalid("ssaa must be 1, 2 or 3".into()));
        }
        if !(1..=4).contains(&self.subdivision) {
            return Err(RenderError::Invalid("subdivision must be in 1..=4".into()));
        }
        if self.shadows && !(256..=8192).contains(&self.shadow_resolution) {
            return Err(RenderError::Invalid(
                "shadow_resolution must be in 256..=8192".into(),
            ));
        }
        let pixels = u64::from(self.width) * u64::from(self.height) * u64::from(self.ssaa).pow(2);
        if pixels > 150_000_000 {
            return Err(RenderError::Invalid(
                "render exceeds 150 megapixels; reduce size or ssaa".into(),
            ));
        }
        if !self.exposure.is_finite() || self.exposure <= 0.0 {
            return Err(RenderError::Invalid("exposure must be positive".into()));
        }
        if !self.gamma.is_finite() || self.gamma <= 0.0 {
            return Err(RenderError::Invalid("gamma must be positive".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RenderStats {
    pub width: u32,
    pub height: u32,
    pub ssaa: u8,
    pub triangles: usize,
    pub shadow_resolution: u32,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub image: RgbaImage,
    pub stats: RenderStats,
}

impl RenderOutput {
    pub fn save_png(&self, path: &std::path::Path) -> Result<()> {
        self.image.save_png(path)?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Vertex {
    pub pos: Vec3,
    pub normal: Vec3,
    pub ao: f32,
    pub clip: Vec4,
}

#[derive(Clone, Copy)]
pub(crate) struct Tri {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub kind: u8,
}

pub(crate) const KIND_TERRAIN: u8 = 0;
pub(crate) const KIND_WATER: u8 = 1;
pub(crate) const KIND_SKIRT: u8 = 2;
pub(crate) const KIND_GROUND: u8 = 3;

pub(crate) struct SceneData {
    pub center: [f64; 3],
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<Tri>,
    pub camera_pos: Vec3,
    pub camera_forward: Vec3,
    pub camera_right: Vec3,
    pub camera_up: Vec3,
    pub tan_half_fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub shadow: Option<shadow::ShadowMap>,
    pub sun_dir: Vec3,
    pub sun_color: vf_core::Rgb,
    pub noise: Noise,
}

/// Render using the project's own render settings.
pub fn render(project: &Project, hf: &HeightField) -> Result<RenderOutput> {
    let options = RenderOptions::from(&project.render);
    render_with_options(project, hf, &options)
}

/// Render with explicit options (CLI overrides, previews, UI).
pub fn render_with_options(
    project: &Project,
    hf: &HeightField,
    options: &RenderOptions,
) -> Result<RenderOutput> {
    let start = Instant::now();
    options.validate()?;
    project.camera.validate().map_err(RenderError::Invalid)?;
    if hf.width() < 2 || hf.height() < 2 {
        return Err(RenderError::Invalid("heightfield is empty".into()));
    }

    let render_w = options.width as usize * options.ssaa as usize;
    let render_h = options.height as usize * options.ssaa as usize;
    // The editable modifier stack runs first, then the cliff pass. The cliff
    // pass shapes the heightfield itself: walls then belong to the terrain
    // mesh (continuous, shadow-casting, material-shaded) instead of being
    // overlays that can detach from the ground.
    let modulated = if project.modifiers.active().next().is_some() {
        Some(
            vf_modifiers::apply(hf, &project.modifiers, project.seed)
                .map_err(|e| RenderError::Invalid(format!("modifier: {e}")))?,
        )
    } else {
        None
    };
    let base = modulated.as_ref().unwrap_or(hf);
    let shaped = if options.cliffs {
        Some(
            vf_procgen::cliff::sharpen(
                base,
                project.seed,
                project.render.cliff_warp,
                project.render.cliff_warp_scale_m,
            )
            .map_err(|e| RenderError::Invalid(format!("cliff shaping: {e}")))?,
        )
    } else {
        None
    };
    let hf = shaped.as_ref().unwrap_or(base);
    let scene = build_scene(project, hf, options, render_w, render_h)?;
    let ctx = ShadeCtx::new(project, hf, &scene, options, scene.shadow.as_ref());

    let jobs = raster::build_jobs(&scene, render_w, render_h);
    let tiles: Vec<raster::TileOut> = jobs
        .into_par_iter()
        .map(|job| raster::render_tile(job, &scene, &ctx, render_w, render_h))
        .collect();

    let mut linear = vec![[0.0f32; 3]; render_w * render_h];
    for tile in tiles {
        raster::blit_tile(&mut linear, render_w, &tile);
    }

    let image = postprocess(&linear, render_w, options);
    Ok(RenderOutput {
        image,
        stats: RenderStats {
            width: options.width,
            height: options.height,
            ssaa: options.ssaa,
            triangles: scene.triangles.len(),
            shadow_resolution: scene.shadow.as_ref().map_or(0, |s| s.size() as u32),
            elapsed_ms: start.elapsed().as_millis(),
        },
    })
}

/// Bilinear sample of a per-sample grid value at an absolute world position.
fn sample_grid_value(hf: &HeightField, values: &[f32], x: f64, y: f64) -> f32 {
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let gx = ((x - ox) / sx).clamp(0.0, (hf.width() - 1) as f64);
    let gy = ((y - oy) / sy).clamp(0.0, (hf.height() - 1) as f64);
    let x0 = gx.floor() as usize;
    let y0 = gy.floor() as usize;
    let x1 = (x0 + 1).min(hf.width() - 1);
    let y1 = (y0 + 1).min(hf.height() - 1);
    let tx = (gx - x0 as f64) as f32;
    let ty = (gy - y0 as f64) as f32;
    let w = hf.width();
    let v00 = values[y0 * w + x0];
    let v10 = values[y0 * w + x1];
    let v01 = values[y1 * w + x0];
    let v11 = values[y1 * w + x1];
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

fn extent_for_plane(bounds: &vf_core::Bounds) -> f32 {
    let size = bounds.size();
    (size[0].max(size[1]) as f32).max(100.0)
}

fn build_scene(
    project: &Project,
    hf: &HeightField,
    options: &RenderOptions,
    render_w: usize,
    render_h: usize,
) -> Result<SceneData> {
    let bounds = hf.bounds();
    let center_f64 = bounds.center();
    let center = Vec3::new(
        center_f64[0] as f32,
        center_f64[1] as f32,
        center_f64[2] as f32,
    );
    let aspect = render_w as f32 / render_h.max(1) as f32;

    let eye = Vec3::new(
        (project.camera.position.x - center_f64[0]) as f32,
        (project.camera.position.y - center_f64[1]) as f32,
        (project.camera.position.z - center_f64[2]) as f32,
    );
    let target = Vec3::new(
        (project.camera.target.x - center_f64[0]) as f32,
        (project.camera.target.y - center_f64[1]) as f32,
        (project.camera.target.z - center_f64[2]) as f32,
    );
    let up = Vec3::new(
        project.camera.up.x as f32,
        project.camera.up.y as f32,
        project.camera.up.z as f32,
    );
    let forward = (target - eye).normalize_or_zero();
    let right = forward.cross(up).normalize_or_zero();
    let camera_up = right.cross(forward).normalize_or_zero();
    if forward.length_squared() < 0.5 {
        return Err(RenderError::Invalid("degenerate camera".into()));
    }

    let near = project.camera.near.max(0.01);
    let far = project.camera.far.max(near + 1.0);
    let view = Mat4::look_at_rh(eye, target, up);
    let proj = Mat4::perspective_rh(
        (f64::from(project.camera.fov_y_deg).to_radians()) as f32,
        aspect,
        near,
        far,
    );
    let vp = proj * view;

    let (sx, sy) = hf.spacing();
    let ao_values = if options.ao {
        // Two scales: the coarse sweep catches the canyon, the fine sweep the
        // 20–50 m gullies and the foot of the cliffs. Multiplying them keeps
        // the open sky at 1.0 while stacking the occlusion of a recess.
        let max_distance = (vf_scene::terrain_extent(hf) * 0.12) as f32;
        let coarse = vf_terrain::horizon_ao(hf, max_distance);
        let fine_range = ((sx.max(sy) as f32) * 6.0).max(18.0);
        let fine = vf_terrain::horizon_ao_fine(hf, fine_range);
        coarse
            .iter()
            .zip(fine.iter())
            .map(|(c, f)| c * (0.50 + 0.50 * f))
            .collect()
    } else {
        vec![1.0; hf.sample_count()]
    };
    // Mesh from the continuous detail surface. Subdivision is a render
    // setting; the stored heightfield is never resampled, so brushes and
    // erosion keep working on the cheap 2.5D grid.
    let detail_settings = if options.detail_geometry {
        project.detail.clone()
    } else {
        vf_core::DetailSettings::default()
    };
    let sampler = vf_detail::DetailSampler::new(hf, &detail_settings, project.seed);
    let material_texture = vf_material::RockTexture::load(&project.materials.texture_path);
    let material_displacement = project.materials.enabled
        && (project.materials.displacement_m > 0.0
            || project.materials.texture_displacement_m > 0.0);
    let material_sampler = material_displacement.then(|| {
        vf_material::MaterialSampler::new(
            &sampler,
            &project.materials,
            project.seed,
            material_texture.as_ref(),
        )
    });
    let mesh_view = vf_mesh::View {
        camera: [
            project.camera.position.x,
            project.camera.position.y,
            project.camera.position.z,
        ],
        fov_y_rad: (f64::from(project.camera.fov_y_deg).to_radians()) as f32,
        pixel_height: render_h as u32,
    };
    // Adaptive LOD pays for its block/skirt bookkeeping on large grids; on a
    // small tile the uniform mesh is both simpler and faster.
    let use_adaptive = options.adaptive && hf.sample_count() >= 2_000_000;
    let base_subdivision = u32::from(options.subdivision.max(1));
    // Displacement lives below the base cell size. When it is active, raise
    // the maximum subdivision and tighten the tolerance so the adaptive pass
    // resolves the displaced surface instead of shearing it into spikes.
    let subdivision = if material_displacement {
        base_subdivision.max(2)
    } else {
        base_subdivision
    };
    let adaptive_tolerance = if material_displacement {
        let amplitude = project
            .materials
            .displacement_m
            .max(project.materials.texture_displacement_m);
        detail_settings
            .adaptive_tolerance_m
            .min((amplitude * 0.75).max(0.05))
    } else {
        detail_settings.adaptive_tolerance_m
    };
    let mut mesh = if detail_settings.adaptive {
        // Shape-adaptive path: spend vertices where the surface bends, keep
        // large quads on flats. The material sampler is the second pass: the
        // builder sees the displaced surface, not the bare heightfield.
        let surface: &dyn vf_mesh::SurfaceSampler = match &material_sampler {
            Some(sampler) => sampler,
            None => &sampler,
        };
        vf_mesh::build_shape_adaptive(surface, subdivision, adaptive_tolerance, MAX_MESH_VERTICES)
    } else if use_adaptive {
        vf_mesh::build_adaptive(&sampler, subdivision, MAX_MESH_VERTICES, mesh_view)
    } else {
        vf_mesh::build(&sampler, subdivision, MAX_MESH_VERTICES, Some(mesh_view))
    };
    if !detail_settings.adaptive && material_displacement {
        // Uniform grid: apply the material displacement after meshing.
        vf_material::displace(
            &mut mesh,
            &project.materials,
            project.seed,
            material_texture.as_ref(),
        );
    }

    let mut vertices = Vec::with_capacity(mesh.vertex_count() + 4);
    for (index, position) in mesh.positions.iter().enumerate() {
        let pos = Vec3::new(
            position[0] - center_f64[0] as f32,
            position[1] - center_f64[1] as f32,
            position[2] - center_f64[2] as f32,
        );
        let normal = mesh.normals[index];
        vertices.push(Vertex {
            pos,
            normal: Vec3::new(normal[0], normal[1], normal[2]),
            ao: sample_grid_value(
                hf,
                &ao_values,
                f64::from(position[0]),
                f64::from(position[1]),
            ),
            clip: vp * pos.extend(1.0),
        });
    }

    let mut triangles = Vec::with_capacity(mesh.triangle_count() + 2);
    // When a close-up patch is active, remove the coarse triangles under it.
    // The patch has centimetre-scale relief that goes below the coarse
    // interpolated plane; without a hole the two surfaces z-fight and produce
    // a hatch pattern. The hole is slightly smaller than the patch so the
    // faded patch border overlaps solid coarse mesh.
    let focus_hole = if detail_settings.enabled && detail_settings.focus_enabled {
        let half = (detail_settings.focus_size_m * 0.5 - detail_settings.focus_size_m * 0.12)
            .max(4.0) as f64;
        Some((
            detail_settings.focus_center[0],
            detail_settings.focus_center[1],
            half,
        ))
    } else {
        None
    };
    for [a, b, c] in &mesh.indices {
        if let Some((cx, cy, half)) = focus_hole {
            let pa = mesh.positions[*a as usize];
            let pb = mesh.positions[*b as usize];
            let pc = mesh.positions[*c as usize];
            let x = (f64::from(pa[0]) + f64::from(pb[0]) + f64::from(pc[0])) / 3.0;
            let y = (f64::from(pa[1]) + f64::from(pb[1]) + f64::from(pc[1])) / 3.0;
            if (x - cx).abs() < half && (y - cy).abs() < half {
                continue;
            }
        }
        triangles.push(Tri {
            a: *a,
            b: *b,
            c: *c,
            kind: KIND_TERRAIN,
        });
    }

    // Optional high-density patch for a close-up. It is built on top of the
    // coarse mesh and fades to the base surface at its border.
    if detail_settings.enabled && detail_settings.focus_enabled {
        let (ox, oy) = hf.origin();
        let x1 = ox + (hf.width() - 1) as f64 * sx;
        let y1 = oy + (hf.height() - 1) as f64 * sy;
        let center = [
            detail_settings.focus_center[0].clamp(ox, x1),
            detail_settings.focus_center[1].clamp(oy, y1),
        ];
        let mut patch = vf_mesh::build_focus_patch(
            &sampler,
            center,
            detail_settings.focus_size_m,
            detail_settings.focus_spacing_m,
            2_500_000,
            Some(mesh_view),
        );
        if material_displacement {
            vf_material::displace(
                &mut patch,
                &project.materials,
                project.seed,
                material_texture.as_ref(),
            );
        }
        let base_index = vertices.len() as u32;
        for (index, position) in patch.positions.iter().enumerate() {
            let pos = Vec3::new(
                position[0] - center_f64[0] as f32,
                position[1] - center_f64[1] as f32,
                position[2] - center_f64[2] as f32,
            );
            let normal = patch.normals[index];
            vertices.push(Vertex {
                pos,
                normal: Vec3::new(normal[0], normal[1], normal[2]),
                ao: sample_grid_value(
                    hf,
                    &ao_values,
                    f64::from(position[0]),
                    f64::from(position[1]),
                ),
                clip: vp * pos.extend(1.0),
            });
        }
        for [a, b, c] in &patch.indices {
            triangles.push(Tri {
                a: base_index + *a,
                b: base_index + *b,
                c: base_index + *c,
                kind: KIND_TERRAIN,
            });
        }
    }

    // Vertical skirt around the DEM patch: turns the heightfield sheet into a
    // solid terrain block and hides the edge-on backfaces.
    let relief = (bounds.max[2] - bounds.min[2]) as f32;
    let base_abs = bounds.min[2] - f64::from(relief) * 0.02 - 10.0;
    let base_abs = if project.water.enabled {
        base_abs.min(f64::from(project.water.sea_level) - 20.0)
    } else {
        base_abs
    };
    let base_rel = (base_abs - center_f64[2]) as f32;
    // Outer skirt: build it from the heightfield boundary, not from the mesh
    // topology, so it works for the regular mesh and for the adaptive LOD.
    let (w, h) = (hf.width(), hf.height());
    let (ox, oy) = hf.origin();
    let cell = sx.max(sy) as f32;
    let mut ring: Vec<(usize, usize)> = Vec::with_capacity(2 * (w + h));
    for x in 0..w {
        ring.push((x, 0));
    }
    for y in 1..h {
        ring.push((w - 1, y));
    }
    for x in (0..w - 1).rev() {
        ring.push((x, h - 1));
    }
    for y in (1..h - 1).rev() {
        ring.push((0, y));
    }
    let ring_pos: Vec<Vec3> = ring
        .iter()
        .map(|(x, y)| {
            let wx = ox + *x as f64 * sx;
            let wy = oy + *y as f64 * sy;
            let z = sampler.height(wx, wy, cell);
            Vec3::new(
                (wx - center_f64[0]) as f32,
                (wy - center_f64[1]) as f32,
                (f64::from(z) - center_f64[2]) as f32,
            )
        })
        .collect();
    let mut skirt_tops = Vec::with_capacity(ring.len());
    let mut skirt_bottoms = Vec::with_capacity(ring.len());
    for (i, &(x, y)) in ring.iter().enumerate() {
        let prev = ring_pos[(i + ring.len() - 1) % ring.len()];
        let next = ring_pos[(i + 1) % ring.len()];
        let tangent = (next - prev).normalize_or_zero();
        let outward = tangent.cross(Vec3::Z).normalize_or_zero();
        let top_pos = ring_pos[i];
        let top_idx = vertices.len() as u32;
        vertices.push(Vertex {
            pos: top_pos,
            normal: outward,
            ao: sample_grid_value(hf, &ao_values, ox + x as f64 * sx, oy + y as f64 * sy),
            clip: vp * top_pos.extend(1.0),
        });
        let thickness = (relief * 0.02 + 10.0).max(8.0);
        let mut bottom_z = top_pos.z - thickness;
        if project.water.enabled {
            bottom_z = bottom_z.min(project.water.sea_level - 12.0 - center_f64[2] as f32);
        }
        let bottom_z = bottom_z.max(base_rel.min(top_pos.z));
        let bottom_pos = Vec3::new(top_pos.x, top_pos.y, bottom_z);
        let bottom_idx = vertices.len() as u32;
        vertices.push(Vertex {
            pos: bottom_pos,
            normal: outward,
            ao: 0.45,
            clip: vp * bottom_pos.extend(1.0),
        });
        skirt_tops.push(top_idx);
        skirt_bottoms.push(bottom_idx);
    }
    for i in 0..ring.len() {
        let j = (i + 1) % ring.len();
        triangles.push(Tri {
            a: skirt_tops[i],
            b: skirt_tops[j],
            c: skirt_bottoms[i],
            kind: KIND_SKIRT,
        });
        triangles.push(Tri {
            a: skirt_tops[j],
            b: skirt_bottoms[j],
            c: skirt_bottoms[i],
            kind: KIND_SKIRT,
        });
    }

    // A large, softly lit ground plane catches the terrain edges and fades
    // into the horizon haze instead of showing the sky under the DEM patch.
    {
        let ground_z = if project.water.enabled {
            (f64::from(project.water.sea_level) - 25.0 - center_f64[2]) as f32
        } else {
            base_rel
        };
        // Keep the backdrop close enough that haze does not turn it into a milky
        // band: 8 terrain widths still fills the horizon of a wide lens.
        let half = extent_for_plane(&bounds) * 8.0;
        let base = vertices.len() as u32;
        for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let pos = Vec3::new(dx * half, dy * half, ground_z);
            vertices.push(Vertex {
                pos,
                normal: Vec3::Z,
                ao: 1.0,
                clip: vp * pos.extend(1.0),
            });
        }
        triangles.push(Tri {
            a: base,
            b: base + 1,
            c: base + 2,
            kind: KIND_GROUND,
        });
        triangles.push(Tri {
            a: base + 1,
            b: base + 3,
            c: base + 2,
            kind: KIND_GROUND,
        });
    }

    let sea_rel = project.water.sea_level - center_f64[2] as f32;
    if options.water && project.water.enabled && f64::from(project.water.sea_level) < bounds.max[2]
    {
        let half_x = (bounds.size()[0] as f32) * 20.0;
        let half_y = (bounds.size()[1] as f32) * 20.0;
        let base = vertices.len() as u32;
        for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let pos = Vec3::new(dx * half_x, dy * half_y, sea_rel);
            vertices.push(Vertex {
                pos,
                normal: Vec3::Z,
                ao: 1.0,
                clip: vp * pos.extend(1.0),
            });
        }
        triangles.push(Tri {
            a: base,
            b: base + 1,
            c: base + 2,
            kind: KIND_WATER,
        });
        triangles.push(Tri {
            a: base + 1,
            b: base + 3,
            c: base + 2,
            kind: KIND_WATER,
        });
    }

    let sun_dir = project.sun.direction();
    let sun_dir = Vec3::new(sun_dir.x as f32, sun_dir.y as f32, sun_dir.z as f32).normalize();
    let shadow = if options.shadows && sun_dir.z > 0.01 {
        Some(shadow::build(
            &vertices,
            &triangles,
            sun_dir,
            center,
            vf_scene::terrain_extent(hf) as f32,
            options.shadow_resolution as usize,
        ))
    } else {
        None
    };

    Ok(SceneData {
        center: center_f64,
        vertices,
        triangles,
        camera_pos: eye,
        camera_forward: forward,
        camera_right: right,
        camera_up,
        tan_half_fov_y: (f64::from(project.camera.fov_y_deg).to_radians() * 0.5).tan() as f32,
        aspect,
        near,
        shadow,
        sun_dir,
        sun_color: project.sun.color(),
        noise: Noise::new(project.seed.derive("render")),
    })
}

fn postprocess(linear: &[[f32; 3]], width: usize, options: &RenderOptions) -> RgbaImage {
    let out_w = options.width as usize;
    let out_h = options.height as usize;
    let ssaa = options.ssaa as usize;
    let inv = 1.0 / (ssaa * ssaa) as f32;
    let mut pixels = vec![0u8; out_w * out_h * 4];
    const BAYER: [[f32; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];

    for oy in 0..out_h {
        for ox in 0..out_w {
            let mut acc = [0.0f32; 3];
            for sy in 0..ssaa {
                for sx in 0..ssaa {
                    let c = linear[(oy * ssaa + sy) * width + ox * ssaa + sx];
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                }
            }
            let exposure = options.exposure;
            let mut rgb = [
                acc[0] * inv * exposure,
                acc[1] * inv * exposure,
                acc[2] * inv * exposure,
            ];
            for channel in &mut rgb {
                *channel = tone_map(*channel, options.tonemap);
                *channel = (*channel - 0.5) * (1.0 + options.contrast) + 0.5;
            }
            let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            for channel in &mut rgb {
                *channel = (luma + (*channel - luma) * options.saturation).clamp(0.0, 1.0);
                *channel = channel.powf(1.0 / options.gamma);
                *channel = (*channel + (BAYER[oy & 3][ox & 3] - 7.5) / 255.0 / options.gamma)
                    .clamp(0.0, 1.0);
            }
            let i = (oy * out_w + ox) * 4;
            pixels[i] = (rgb[0] * 255.0 + 0.5) as u8;
            pixels[i + 1] = (rgb[1] * 255.0 + 0.5) as u8;
            pixels[i + 2] = (rgb[2] * 255.0 + 0.5) as u8;
            pixels[i + 3] = 255;
        }
    }
    RgbaImage::new(options.width, options.height, pixels)
        .expect("postprocess buffer has the right size")
}

pub(crate) fn tone_map(x: f32, mode: Tonemap) -> f32 {
    let x = x.max(0.0);
    match mode {
        Tonemap::None => x.min(1.0),
        Tonemap::Reinhard => x / (1.0 + x),
        Tonemap::Aces => {
            let a = 2.51;
            let b = 0.03;
            let c = 2.43;
            let d = 0.59;
            let e = 0.14;
            ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
        }
    }
}

/// Small deterministic cloud texture helper used by previews and the UI.
pub fn cloud_density_at(project: &Project, x: f32, y: f32) -> f32 {
    let clouds = &project.clouds;
    if !clouds.enabled {
        return 0.0;
    }
    let seed = vf_core::Seed(clouds.seed).derive("cloud-layer");
    let mut n = 0.0;
    let mut amp = 0.55;
    let mut freq = 1.0 / 1400.0;
    for _ in 0..5 {
        n += amp
            * value2(
                x * freq + clouds.wind[0] * freq,
                y * freq + clouds.wind[1] * freq,
                seed.0,
            );
        amp *= 0.5;
        freq *= 2.0;
    }
    let coverage = clouds.coverage.clamp(0.0, 1.0);
    let softness = clouds.softness.clamp(0.01, 1.0);
    let threshold = 1.0 - coverage;
    smoothstep(threshold - softness * 0.5, threshold + softness * 0.5, n)
}

pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn ray_dir_for_pixel(scene: &SceneData, x: usize, y: usize, w: usize, h: usize) -> Vec3 {
    let ndc_x = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - (y as f32 + 0.5) / h as f32 * 2.0;
    let tan_h = scene.tan_half_fov_y * scene.aspect;
    (scene.camera_forward
        + scene.camera_right * (ndc_x * tan_h)
        + scene.camera_up * (ndc_y * scene.tan_half_fov_y))
        .normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{ClassicParams, Seed};

    fn test_project() -> (Project, HeightField) {
        let mut project = Project::new("Render test", Seed::new(3), 33);
        project.source = vf_core::TerrainSource::Classic {
            params: ClassicParams::default().with_size_seed(33, 3),
        };
        project.clouds.enabled = false;
        project.water.enabled = false;
        project.render.ao = false;
        project.render.shadows = false;
        let hf =
            vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(33, 3)).unwrap();
        project.sync_terrain(&hf);
        (project, hf)
    }

    #[test]
    fn rock_presets_change_the_render() {
        let (mut project, hf) = test_project();
        project.camera = vf_scene::auto_frame(&hf, 4.0 / 3.0);
        project.detail.enabled = false;
        project.materials.enabled = true;
        let options = RenderOptions {
            width: 96,
            height: 72,
            ssaa: 1,
            shadows: false,
            shadow_resolution: 256,
            ao: false,
            cliffs: false,
            clouds: false,
            water: false,
            exposure: 1.0,
            contrast: 0.0,
            saturation: 1.0,
            gamma: 2.2,
            tonemap: Tonemap::Aces,
            subdivision: 1,
            detail_geometry: false,
            adaptive: false,
        };
        let mean = |pixels: &[u8]| -> f64 {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0)
                .sum::<f64>()
                / (pixels.len() / 4) as f64
        };
        project.materials.rock_preset = "granite".into();
        let granite = render_with_options(&project, &hf, &options)
            .unwrap()
            .image
            .pixels;
        project.materials.rock_preset = "basalt".into();
        let basalt = render_with_options(&project, &hf, &options)
            .unwrap()
            .image
            .pixels;
        project.materials.rock_preset = "sandstone".into();
        let sandstone = render_with_options(&project, &hf, &options)
            .unwrap()
            .image
            .pixels;
        let g = mean(&granite);
        let b = mean(&basalt);
        let s = mean(&sandstone);
        assert!((g - b).abs() > 0.5, "granite {g} vs basalt {b}");
        assert!((g - s).abs() > 0.5, "granite {g} vs sandstone {s}");
    }

    #[test]
    fn renders_a_small_deterministic_image() {
        let (mut project, hf) = test_project();
        project.camera = vf_scene::auto_frame(&hf, 4.0 / 3.0);
        let options = RenderOptions {
            width: 64,
            height: 48,
            ssaa: 1,
            shadows: false,
            shadow_resolution: 256,
            ao: false,
            cliffs: false,
            clouds: false,
            water: false,
            exposure: 1.0,
            contrast: 0.0,
            saturation: 1.0,
            gamma: 2.2,
            tonemap: Tonemap::Aces,
            subdivision: 1,
            detail_geometry: false,
            adaptive: false,
        };
        let a = render_with_options(&project, &hf, &options).unwrap();
        let b = render_with_options(&project, &hf, &options).unwrap();
        assert_eq!(a.image.pixels, b.image.pixels);
        assert_eq!((a.image.width, a.image.height), (64, 48));
        let first = a.image.pixel(0, 0);
        assert!(
            a.image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| (p[0], p[1], p[2]) != (first[0], first[1], first[2])),
            "image is entirely uniform"
        );
    }

    #[test]
    fn modifier_stack_is_applied_before_rendering() {
        let (mut project, hf) = test_project();
        project.camera = vf_scene::auto_frame(&hf, 4.0 / 3.0);
        project.render.cliffs = false;
        let options = RenderOptions {
            width: 48,
            height: 36,
            ssaa: 1,
            shadows: false,
            shadow_resolution: 256,
            ao: false,
            cliffs: false,
            clouds: false,
            water: false,
            exposure: 1.0,
            contrast: 0.0,
            saturation: 1.0,
            gamma: 2.2,
            tonemap: Tonemap::Aces,
            subdivision: 1,
            detail_geometry: false,
            adaptive: false,
        };
        let before = render_with_options(&project, &hf, &options).unwrap();
        let mut modifier = vf_core::Modifier::new(vf_core::ModifierKind::Displace);
        modifier.set("amount_m", 250.0);
        project.modifiers.items.push(modifier);
        let after = render_with_options(&project, &hf, &options).unwrap();
        assert_ne!(before.image.pixels, after.image.pixels);
    }

    #[test]
    fn water_covers_low_areas() {
        let (mut project, hf) = test_project();
        project.camera = vf_scene::auto_frame(&hf, 1.0);
        project.water.enabled = true;
        project.water.sea_level = hf.stats().min + 40.0;
        let options = RenderOptions::preview(48, 48);
        let out = render_with_options(&project, &hf, &options).unwrap();
        assert!(
            out.image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[2] > p[0] + 10),
            "no water pixels found"
        );
    }
}

#[cfg(test)]
mod cloud_tests {
    use super::*;
    use vf_core::{Project, Seed};

    #[test]
    fn cloud_layer_has_visible_coverage() {
        let project = Project::new("clouds", Seed::new(2), 258);
        let mut non_zero = 0;
        let mut total = 0;
        let mut max = 0.0f32;
        for y in 0..100 {
            for x in 0..100 {
                let d = cloud_density_at(&project, x as f32 * 120.0, y as f32 * 120.0);
                max = max.max(d);
                if d > 0.05 {
                    non_zero += 1;
                }
                total += 1;
            }
        }
        println!("cloud coverage: {non_zero}/{total} max={max}");
        assert!(non_zero > total / 10, "cloud layer is empty");
    }
}

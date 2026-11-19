//! Procedural rock presets, texture sampling and material displacement.
//!
//! The material panel is not a post-process: it feeds the CPU shader (albedo
//! and bump) and the mesh builder (displacement). Keeping the preset table in
//! one place means the viewport, the final render and the Blender bake all
//! describe the same rock.

use glam::{Vec2, Vec3};
use vf_core::{Materials, Project, Rgb, Seed};
use vf_mesh::MeshData;
use vf_procgen::Noise;

/// A loaded custom texture. Kept linear and small enough for per-pixel
/// sampling in the CPU renderer.
pub struct RockTexture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 3]>,
}

impl RockTexture {
    pub fn load(path: &str) -> Option<Self> {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        let image = image::load_from_memory(&bytes).ok()?.to_rgb8();
        let (width, height) = image.dimensions();
        if width == 0 || height == 0 {
            return None;
        }
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for pixel in image.pixels() {
            let color = Rgb::from_srgb_u8(pixel[0], pixel[1], pixel[2]);
            pixels.push([color.r, color.g, color.b]);
        }
        Some(Self {
            width: width as usize,
            height: height as usize,
            pixels,
        })
    }

    /// Bilinear sample with wrap. `u`/`v` are in tile units.
    pub fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        if self.width == 0 || self.height == 0 {
            return [0.5; 3];
        }
        let x = u.rem_euclid(1.0) * self.width as f32 - 0.5;
        let y = v.rem_euclid(1.0) * self.height as f32 - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let tx = x - x0;
        let ty = y - y0;
        let xi = |value: f32| (value as i64).rem_euclid(self.width as i64) as usize;
        let yi = |value: f32| (value as i64).rem_euclid(self.height as i64) as usize;
        let x0 = xi(x0);
        let x1 = xi(x0 as f32 + 1.0);
        let y0 = yi(y0);
        let y1 = yi(y0 as f32 + 1.0);
        let a = self.pixels[y0 * self.width + x0];
        let b = self.pixels[y0 * self.width + x1];
        let c = self.pixels[y1 * self.width + x0];
        let d = self.pixels[y1 * self.width + x1];
        let mut out = [0.0f32; 3];
        for index in 0..3 {
            let top = a[index] + (b[index] - a[index]) * tx;
            let bottom = c[index] + (d[index] - c[index]) * tx;
            out[index] = top + (bottom - top) * ty;
        }
        out
    }

    pub fn luminance(&self, u: f32, v: f32) -> f32 {
        let color = self.sample(u, v);
        (color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722).clamp(0.0, 1.0)
    }
}

/// Physical signature of a rock family. Values are multipliers in roughly
/// `0..2`; the palette still supplies the local geological colour.
#[derive(Debug, Clone, Copy)]
pub struct RockStyle {
    pub tint: [f32; 3],
    /// Bedding band thickness, metres (0 = no bedding).
    pub bedding_m: f32,
    pub bedding_strength: f32,
    /// Grain wavelength, metres.
    pub grain_m: f32,
    pub grain_strength: f32,
    /// Crack spacing, metres.
    pub crack_m: f32,
    pub crack_strength: f32,
    /// Lichen / moss patches, 0..1.
    pub lichen: f32,
    /// Speckle contrast, 0..1.
    pub speckle: f32,
}

impl RockStyle {
    pub fn preset(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "granite" => Self {
                tint: [1.06, 0.98, 0.94],
                bedding_m: 0.0,
                bedding_strength: 0.0,
                grain_m: 1.6,
                grain_strength: 0.30,
                crack_m: 42.0,
                crack_strength: 0.45,
                lichen: 0.10,
                speckle: 0.55,
            },
            "basalt" => Self {
                tint: [0.58, 0.60, 0.66],
                bedding_m: 0.0,
                bedding_strength: 0.0,
                grain_m: 0.9,
                grain_strength: 0.38,
                crack_m: 18.0,
                crack_strength: 0.70,
                lichen: 0.05,
                speckle: 0.25,
            },
            "limestone" => Self {
                tint: [1.12, 1.06, 0.94],
                bedding_m: 6.5,
                bedding_strength: 0.55,
                grain_m: 2.8,
                grain_strength: 0.18,
                crack_m: 26.0,
                crack_strength: 0.38,
                lichen: 0.18,
                speckle: 0.20,
            },
            "sandstone" => Self {
                tint: [1.38, 0.88, 0.62],
                bedding_m: 3.8,
                bedding_strength: 0.80,
                grain_m: 1.2,
                grain_strength: 0.22,
                crack_m: 55.0,
                crack_strength: 0.30,
                lichen: 0.04,
                speckle: 0.18,
            },
            "schist" => Self {
                tint: [0.72, 0.80, 0.88],
                bedding_m: 2.6,
                bedding_strength: 0.65,
                grain_m: 0.8,
                grain_strength: 0.35,
                crack_m: 12.0,
                crack_strength: 0.55,
                lichen: 0.22,
                speckle: 0.40,
            },
            "chalk" => Self {
                tint: [1.20, 1.18, 1.10],
                bedding_m: 9.0,
                bedding_strength: 0.35,
                grain_m: 3.5,
                grain_strength: 0.12,
                crack_m: 34.0,
                crack_strength: 0.28,
                lichen: 0.06,
                speckle: 0.10,
            },
            "volcanic" => Self {
                tint: [1.05, 0.58, 0.45],
                bedding_m: 0.0,
                bedding_strength: 0.0,
                grain_m: 1.4,
                grain_strength: 0.45,
                crack_m: 14.0,
                crack_strength: 0.65,
                lichen: 0.03,
                speckle: 0.60,
            },
            _ => Self {
                tint: [1.0; 3],
                bedding_m: 0.0,
                bedding_strength: 0.0,
                grain_m: 2.0,
                grain_strength: 0.18,
                crack_m: 30.0,
                crack_strength: 0.30,
                lichen: 0.10,
                speckle: 0.20,
            },
        }
    }

    /// Colour of a point on the rock, in linear RGB. `warp` is the
    /// low-frequency terrain variation (0..1), `slope` is 0 (flat)..1
    /// (vertical).
    pub fn shade(&self, base: Rgb, pos: Vec3, slope: f32, noise: &Noise, seed_offset: f32) -> Rgb {
        let p = Vec2::new(pos.x + seed_offset, pos.y - seed_offset);
        // Tint each channel separately. Basalt is genuinely darker and
        // sandstone genuinely warmer; restoring the mean luminance made every
        // preset look identical on screen.
        let mut color = Rgb::new(
            base.r * self.tint[0],
            base.g * self.tint[1],
            base.b * self.tint[2],
        );

        let grain = noise.fbm2(
            p.x / self.grain_m.max(0.05),
            p.y / self.grain_m.max(0.05),
            4,
            2.0,
            0.55,
        );
        color = color.scale(1.0 + grain * self.grain_strength * 2.0);
        if self.bedding_m > 0.01 {
            let warp = noise.fbm2(
                p.x / (self.bedding_m * 9.0),
                p.y / (self.bedding_m * 9.0),
                3,
                2.0,
                0.5,
            ) * 0.75;
            let phase = (pos.z / self.bedding_m + warp).sin() * 0.5 + 0.5;
            let band = 0.75 + 0.45 * phase * self.bedding_strength;
            color = color.scale(band);
        }
        if self.crack_strength > 0.0 {
            let ridged = noise.ridged2(
                p.x / self.crack_m.max(1.0),
                p.y / self.crack_m.max(1.0),
                4,
                2.0,
                0.5,
            );
            let seam = (1.0 - smoothstep(0.78, 0.94, ridged))
                * self.crack_strength
                * smoothstep(0.18, 0.65, slope);
            color = color.scale(1.0 - seam * 0.7);
        }
        if self.lichen > 0.0 {
            let patch = noise.fbm01(p.x / 6.5 + 17.0, p.y / 6.5 - 4.0, 3, 2.0, 0.5);
            let lichen = smoothstep(0.62, 0.82, patch) * self.lichen * (1.0 - slope * 0.7);
            color = color.lerp(Rgb::new(0.20, 0.28, 0.13), lichen);
        }
        if self.speckle > 0.0 {
            let speck = noise.noise2(p.x / 0.45, p.y / 0.45) * 0.5 + 0.5;
            color = color.scale(1.0 + (speck - 0.5) * self.speckle * 0.35);
        }
        color
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Apply material displacement to a built mesh. Two layers:
/// * a procedural fractal shared by all presets (cheap, always coherent);
/// * the custom texture luminance, sampled in world XY at `texture_scale_m`.
///
/// The mesh normals are recomputed because the displaced surface is the one
/// the light must see.
pub fn displace(
    mesh: &mut MeshData,
    materials: &Materials,
    project_seed: Seed,
    texture: Option<&RockTexture>,
) {
    let procedural = materials.displacement_m.max(0.0);
    let textured = materials.texture_displacement_m.max(0.0);
    if (procedural <= 1e-4 && textured <= 1e-4) || mesh.positions.is_empty() {
        return;
    }
    let noise = Noise::new(project_seed.derive("material-displacement"));
    let scale = materials.bump_scale_m.max(0.05);
    let texture_scale = materials.texture_scale_m.max(0.1);
    for (index, position) in mesh.positions.iter_mut().enumerate() {
        let normal = Vec3::from_array(mesh.normals[index]);
        let mut offset = 0.0;
        if procedural > 0.0 {
            offset +=
                noise.fbm2(position[0] / scale, position[1] / scale, 4, 2.0, 0.5) * procedural;
        }
        if textured > 0.0
            && let Some(texture) = texture
        {
            let u = position[0] / texture_scale;
            let v = position[1] / texture_scale;
            offset += (texture.luminance(u, v) - 0.5) * 2.0 * textured;
        }
        if offset.abs() > 1e-6 {
            let p = normal * offset;
            position[0] += p.x;
            position[1] += p.y;
            position[2] += p.z;
        }
    }
    mesh.recompute_normals();
}

/// Displacement helper for the Blender export, which builds its own mesh and
/// therefore cannot use the renderer's `MeshData` internals.
pub fn displacement_scale(project: &Project) -> (f32, f32, f32) {
    (
        project.materials.displacement_m,
        project.materials.texture_displacement_m,
        project.materials.texture_scale_m,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::Seed;
    use vf_detail::DetailSampler;
    use vf_mesh::build;

    #[test]
    fn presets_produce_distinct_rock_colours() {
        let noise = Noise::new(Seed::new(11));
        let base = Rgb::new(0.5, 0.45, 0.4);
        let pos = Vec3::new(123.0, 456.0, 700.0);
        let granite = RockStyle::preset("granite").shade(base, pos, 0.4, &noise, 7.0);
        let basalt = RockStyle::preset("basalt").shade(base, pos, 0.4, &noise, 7.0);
        let sandstone = RockStyle::preset("sandstone").shade(base, pos, 0.4, &noise, 7.0);
        let distance = |a: Rgb, b: Rgb| (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs();
        assert!(distance(granite, basalt) > 0.05);
        assert!(distance(granite, sandstone) > 0.05);
        assert!(distance(basalt, sandstone) > 0.05);
    }

    #[test]
    fn custom_texture_displacement_moves_vertices_and_keeps_uvs() {
        let width = 17usize;
        let heights = vec![0.0f32; width * width];
        let hf = vf_core::HeightField::new(width, width, 10.0, 10.0, heights).unwrap();
        let detail = vf_core::DetailSettings::default();
        let sampler = DetailSampler::new(&hf, &detail, Seed::new(5));
        let mut mesh = build(&sampler, 2, 100_000, None);
        let before = mesh.positions.clone();
        let before_uvs = mesh.uvs.clone();
        let texture = RockTexture {
            width: 4,
            height: 4,
            pixels: (0..16)
                .map(|index| if index % 2 == 0 { [1.0; 3] } else { [0.0; 3] })
                .collect(),
        };
        let materials = Materials {
            texture_displacement_m: 6.0,
            texture_scale_m: 50.0,
            ..Materials::default()
        };
        displace(&mut mesh, &materials, Seed::new(5), Some(&texture));
        assert!(
            mesh.positions
                .iter()
                .zip(&before)
                .any(|(a, b)| (a[2] - b[2]).abs() > 0.01)
        );
        assert_eq!(mesh.uvs, before_uvs, "UVs must stay stable for texturing");
        assert_eq!(mesh.positions.len(), before.len());
    }
}

//! Modern fBm/ridged terrain with domain warp, continent masks, terraces and
//! optional craters.

use crate::DEFAULT_WORLD_EXTENT_M;
use crate::noise::Noise;
use rayon::prelude::*;
use vf_core::{HeightField, ModernParams, Result, Seed};

/// Generate a modern fractal terrain.
pub fn generate(params: &ModernParams) -> Result<HeightField> {
    params.validate().map_err(vf_core::Error::Invalid)?;
    let size = params.size as usize;
    let inv = 1.0 / (size - 1).max(1) as f32;
    let noise = Noise::new(Seed::new(params.seed));
    // Cap detail so small grids do not alias into one-pixel spikes.
    let octaves = if size < 512 {
        params.octaves.min(7)
    } else {
        params.octaves
    };
    let coast_noise = Noise::new(Seed::new(params.seed).derive("coast"));
    let mut heights = vec![0.0f32; size * size];

    let (min, max) = vf_core::compute::install(|| {
        heights
            .par_chunks_mut(size)
            .enumerate()
            .map(|(y, row)| {
                let v = y as f32 * inv;
                let mut local_min = f32::INFINITY;
                let mut local_max = f32::NEG_INFINITY;
                for (x, out) in row.iter_mut().enumerate() {
                    let u = x as f32 * inv;
                    let cx = (u - 0.5) * 2.0;
                    let cy = (v - 0.5) * 2.0;

                    // Two low-frequency warp fields break the lattice look.
                    let wx = noise.fbm2(
                        u * params.warp_frequency * 1.7 + 11.3,
                        v * params.warp_frequency * 1.7 - 4.2,
                        4,
                        2.0,
                        0.5,
                    );
                    let wy = noise.fbm2(
                        u * params.warp_frequency * 1.7 - 7.1,
                        v * params.warp_frequency * 1.7 + 3.7,
                        4,
                        2.0,
                        0.5,
                    );
                    let qx = u * params.frequency + wx * params.warp_strength;
                    let qy = v * params.frequency + wy * params.warp_strength;

                    let base = noise.fbm01(qx, qy, octaves, params.lacunarity, params.gain);
                    let ridge = noise.ridged2(
                        qx * 1.3 + 5.2,
                        qy * 1.3 - 2.4,
                        octaves,
                        params.lacunarity,
                        params.gain,
                    );
                    let mut h =
                        base * (1.0 - params.ridge_strength) + ridge * params.ridge_strength;

                    // Continent/island mask with a noisy coastline.
                    if params.continent > 0.0 {
                        let warp =
                            coast_noise.fbm2(u * 3.1 + 9.0, v * 3.1 - 2.0, 3, 2.0, 0.5) * 0.12;
                        let d = ((cx * cx + cy * cy).sqrt() + warp).max(0.0);
                        let mask = 1.0 - smoothstep(0.42, 1.0, d);
                        h *= 1.0 - params.continent + params.continent * mask;
                    }

                    *out = h;
                    local_min = local_min.min(h);
                    local_max = local_max.max(h);
                }
                (local_min, local_max)
            })
            .reduce(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |a, b| (a.0.min(b.0), a.1.max(b.1)),
            )
    });

    // Normalise before adding craters and terraces, which are expressed in
    // normalised units.
    let range = (max - min).max(1e-9);
    vf_core::compute::install(|| {
        heights.par_iter_mut().for_each(|h| {
            *h = (*h - min) / range;
        });
    });

    if params.crater > 0.0 {
        add_craters(&mut heights, size, params.crater, params.seed);
    }
    if params.border_falloff > 0.0 {
        apply_border_falloff(&mut heights, size, params.border_falloff);
    }
    if params.terrace_steps > 1 {
        let steps = params.terrace_steps as f32;
        vf_core::compute::install(|| {
            heights.par_iter_mut().for_each(|h| {
                let q = (*h * steps).round() / steps;
                *h += (q - *h) * 0.62;
            });
        });
    }

    // Renormalise after craters/terraces, then scale to metres.
    let (min, max) = vf_core::compute::install(|| {
        heights.par_iter().map(|&h| (h, h)).reduce(
            || (f32::INFINITY, f32::NEG_INFINITY),
            |a, b| (a.0.min(b.0), a.1.max(b.1)),
        )
    });
    let range = (max - min).max(1e-9);
    let scale = params.vertical_scale.max(0.0);
    vf_core::compute::install(|| {
        heights.par_iter_mut().for_each(|h| {
            *h = (*h - min) / range * scale;
        });
    });

    let spacing = DEFAULT_WORLD_EXTENT_M / (size - 1) as f64;
    let mut hf = HeightField::new(size, size, spacing, spacing, heights)?;
    hf.set_origin(0.0, 0.0);
    Ok(hf)
}

fn apply_border_falloff(heights: &mut [f32], size: usize, strength: f32) {
    let start = 1.0 - strength.clamp(0.0, 1.0) * 0.55;
    vf_core::compute::install(|| {
        heights
            .par_chunks_mut(size)
            .enumerate()
            .for_each(|(y, row)| {
                let v = y as f32 / (size - 1).max(1) as f32 - 0.5;
                for (x, h) in row.iter_mut().enumerate() {
                    let u = x as f32 / (size - 1).max(1) as f32 - 0.5;
                    // Radial taper: the square grid corners sink below the sea and
                    // the visible land reads as a rounded island rather than a cut
                    // block.
                    let radius = (u * u + v * v).sqrt() * 2.0;
                    let taper = 1.0 - smoothstep(start, 1.0, radius);
                    *h *= taper;
                }
            });
    });
}

fn add_craters(heights: &mut [f32], size: usize, amount: f32, seed: u64) {
    let count = (amount * 90.0).round() as usize;
    let mut rng = Seed::new(seed).derive("craters").rng();
    for _ in 0..count {
        let cx = rng.next_f32();
        let cy = rng.next_f32();
        let radius = 0.012 + rng.next_f32() * 0.055;
        let depth = 0.05 + rng.next_f32() * 0.22;
        let x0 = ((cx - radius * 1.6) * (size - 1) as f32).floor().max(0.0) as usize;
        let x1 = ((cx + radius * 1.6) * (size - 1) as f32)
            .ceil()
            .min((size - 1) as f32) as usize;
        let y0 = ((cy - radius * 1.6) * (size - 1) as f32).floor().max(0.0) as usize;
        let y1 = ((cy + radius * 1.6) * (size - 1) as f32)
            .ceil()
            .min((size - 1) as f32) as usize;
        for y in y0..=y1 {
            let v = y as f32 / (size - 1) as f32;
            for x in x0..=x1 {
                let u = x as f32 / (size - 1) as f32;
                let dx = u - cx;
                let dy = v - cy;
                let d = (dx * dx + dy * dy).sqrt() / radius;
                if d > 1.7 {
                    continue;
                }
                let i = y * size + x;
                if d < 1.0 {
                    heights[i] -= depth * (1.0 - d * d);
                }
                let rim = (-((d - 1.06) / 0.13).powi(2)).exp();
                heights[i] += depth * 0.35 * rim;
            }
        }
    }
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::terrain_params::modern_preset;

    #[test]
    fn modern_is_deterministic_and_sized() {
        let params = modern_preset("alpine").unwrap().with_size_seed(258, 99);
        let a = generate(&params).unwrap();
        let b = generate(&params).unwrap();
        assert_eq!((a.width(), a.height()), (258, 258));
        assert_eq!(a.heights(), b.heights());
        assert_eq!(a.stats().min, 0.0);
        assert!((a.stats().max - params.vertical_scale).abs() < 1e-3);
    }

    #[test]
    fn presets_differ() {
        let alpine = generate(&modern_preset("alpine").unwrap().with_size_seed(129, 1)).unwrap();
        let lunar = generate(&modern_preset("lunar").unwrap().with_size_seed(129, 1)).unwrap();
        assert_ne!(alpine.heights(), lunar.heights());
    }

    #[test]
    fn craters_change_lunar_terrain() {
        let p = modern_preset("lunar").unwrap().with_size_seed(129, 4);
        let with = generate(&p).unwrap();
        let without = generate(&ModernParams { crater: 0.0, ..p }).unwrap();
        assert_ne!(with.heights(), without.heights());
    }
}

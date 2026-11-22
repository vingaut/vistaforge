//! Natural erosion operators: hydraulic, thermal, alluvium, snow and dunes.

use std::collections::BinaryHeap;

use rayon::prelude::*;
use vf_core::{ErosionParams, HeightField, Modifier, Result, Seed};

use crate::{noise_for, rebuild, slope_at, smoothstep};

fn hydraulic_params(modifier: &Modifier) -> ErosionParams {
    ErosionParams {
        hydraulic: true,
        droplets: modifier.integer("droplets").clamp(1000, 500_000) as u32,
        erosion_rate: modifier.value("erosion_rate").clamp(0.0, 1.0),
        deposition_rate: modifier.value("deposition_rate").clamp(0.0, 1.0),
        max_lifetime: modifier.integer("lifetime").clamp(5, 200) as u32,
        inertia: 0.05,
        capacity: 1.2,
        evaporation: 0.015,
        gravity: 4.0,
        thermal: false,
        talus_angle_deg: 35.0,
        thermal_iterations: 1,
    }
}

pub fn hydraulic(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let params = hydraulic_params(modifier);
    vf_procgen::erosion::erode(hf, &params, seed.derive("hydraulic"))
}

pub fn thermal(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let params = ErosionParams {
        hydraulic: false,
        droplets: 0,
        erosion_rate: 0.3,
        deposition_rate: 0.2,
        max_lifetime: 64,
        inertia: 0.05,
        capacity: 1.2,
        evaporation: 0.015,
        gravity: 4.0,
        thermal: true,
        talus_angle_deg: modifier.value("talus_angle_deg").clamp(1.0, 89.0),
        thermal_iterations: modifier.integer("iterations").clamp(1, 64) as u32,
    };
    vf_procgen::erosion::erode(hf, &params, Seed::new(0))
}

pub fn alluvium(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let droplets = modifier.integer("droplets").clamp(1000, 500_000) as u32;
    let deposition = modifier.value("deposition").clamp(0.0, 1.0);
    let smoothing = modifier.value("smoothing").clamp(0.0, 1.0);

    // Pass 1: water entrains material with a strong deposition bias.
    let first = ErosionParams {
        hydraulic: true,
        droplets,
        erosion_rate: 0.22,
        deposition_rate: (0.3 + 0.7 * deposition).clamp(0.0, 1.0),
        max_lifetime: 48,
        inertia: 0.05,
        capacity: 1.6,
        evaporation: 0.02,
        gravity: 4.0,
        thermal: false,
        talus_angle_deg: 35.0,
        thermal_iterations: 1,
    };
    let mut out = vf_procgen::erosion::erode(hf, &first, seed.derive("alluvium-1"))?;

    // Pass 2: fans settle, so low ground is smoothed more than cliffs.
    if smoothing > 0.0 {
        let (sx, sy) = out.spacing();
        let cell = (sx.max(sy) as f32).max(0.001);
        let radius = ((1.0 + smoothing * 4.0) * cell).round().max(1.0) as usize;
        let smoothed = vf_terrain::smooth(&out, radius, 1);
        let mut heights = out.heights().to_vec();
        let width = out.width();
        vf_core::compute::install(|| {
            heights
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, value) in row.iter_mut().enumerate() {
                        if !out.is_valid(x, y) {
                            continue;
                        }
                        let slope = slope_at(&out, x, y);
                        let weight = (1.0 - smoothstep(0.20, 0.70, slope)) * smoothing * 0.55;
                        let z = *value;
                        *value = z + (smoothed.heights()[y * width + x] - z) * weight;
                    }
                });
        });
        out = rebuild(&out, heights)?;
    }

    // Pass 3: a light drizzle distributes the remaining sediment.
    let second = ErosionParams {
        hydraulic: true,
        droplets: (droplets / 3).max(1000),
        erosion_rate: 0.08,
        deposition_rate: 0.8,
        max_lifetime: 32,
        inertia: 0.05,
        capacity: 1.0,
        evaporation: 0.025,
        gravity: 4.0,
        thermal: false,
        talus_angle_deg: 35.0,
        thermal_iterations: 1,
    };
    vf_procgen::erosion::erode(&out, &second, seed.derive("alluvium-3"))
}

pub fn snow(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let amount = modifier.value("amount_m");
    let coverage = modifier.value("coverage").clamp(0.0, 1.0);
    let scale = modifier.value("scale_m").max(1.0);
    if amount == 0.0 || coverage == 0.0 {
        return Ok(hf.clone());
    }
    let noise = noise_for(seed, "snow");
    let curvature = vf_terrain::curvature(hf);
    // A local mean gives the depth of the hollow, including flat-bottomed
    // basins where the curvature itself is zero.
    let smooth_radius = 4usize;
    let local_mean = vf_terrain::smooth(hf, smooth_radius, 1);
    let (ox, oy, _, _) = crate::world_extent(hf);
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    let width = hf.width();
    vf_core::compute::install(|| {
        heights
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, value) in row.iter_mut().enumerate() {
                    if !hf.is_valid(x, y) {
                        continue;
                    }
                    let wx = (ox + x as f64 * sx) as f32;
                    let wy = (oy + y as f64 * sy) as f32;
                    let i = y * width + x;
                    let z = hf.get(x, y);
                    let basin = (local_mean.heights()[i] - z).max(0.0);
                    // Basin depth fills flat-bottomed cirques; positive
                    // curvature catches bowls whose local mean equals their
                    // centre.
                    let hollow = (smoothstep(0.15, 2.5, basin)
                        + 0.65 * smoothstep(0.0, 0.012, curvature[i]))
                    .min(1.0);
                    let patch = 0.55 + 0.45 * noise.fbm01(wx / scale, wy / scale, 4, 2.0, 0.5);
                    *value += amount * coverage * hollow * patch;
                }
            });
    });
    let mut out = rebuild(hf, heights)?;
    // Glaciation (negative amount) also broadens the valleys: a U-shaped
    // profile is smoother than the V-shaped river valley it replaces.
    if amount < 0.0 && coverage > 0.0 {
        let smoothed = vf_terrain::smooth(&out, 2, 1);
        let blend = (coverage * 0.25).clamp(0.0, 0.35);
        let mut mixed = out.heights().to_vec();
        let width = out.width();
        vf_core::compute::install(|| {
            mixed
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, value) in row.iter_mut().enumerate() {
                        let slope = slope_at(&out, x, y);
                        let weight = blend * (1.0 - smoothstep(0.4, 0.9, slope));
                        let z = *value;
                        *value = z + (smoothed.heights()[y * width + x] - z) * weight;
                    }
                });
        });
        out = rebuild(&out, mixed)?;
    }
    Ok(out)
}

/// Geometric talus fill (scree aprons).
///
/// HighMap's `fill_talus` raises every cell that lies below the surface
/// obtained by projecting a cone of repose from the surrounding high ground.
/// The difference with thermal erosion is that this is a *fill*: it builds
/// the debris apron at the foot of a cliff instead of only relaxing the rock.
/// A priority queue propagates the envelope from the highest cells down; a
/// `depth_m` cap keeps a 300 m canyon from being completely filled with talus.
pub fn talus(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let talus = modifier
        .value("talus_deg")
        .clamp(1.0, 89.0)
        .to_radians()
        .tan();
    let depth = modifier.value("depth_m").max(0.0);
    let noise = modifier.value("noise").clamp(0.0, 0.5);
    if depth == 0.0 || !talus.is_finite() || talus <= 0.0 {
        return Ok(hf.clone());
    }
    let (sx, sy) = hf.spacing();
    let cell = sx.max(sy) as f32;
    let width = hf.width();
    let height = hf.height();
    let original = hf.heights().to_vec();
    let mut heights = original.clone();
    let mut heap: BinaryHeap<(u32, usize)> = BinaryHeap::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            if hf.is_valid(x, y) {
                let i = y * width + x;
                heap.push((sortable(heights[i]), i));
            }
        }
    }
    while let Some((key, index)) = heap.pop() {
        if sortable(heights[index]) != key {
            continue; // stale entry
        }
        let z = heights[index];
        let x = index % width;
        let y = index / width;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as isize + dx as isize;
                let ny = y as isize + dy as isize;
                if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                    continue;
                }
                let (nx, ny) = (nx as usize, ny as usize);
                if !hf.is_valid(nx, ny) {
                    continue;
                }
                let n = ny * width + nx;
                let limit = original[n] + depth;
                if heights[n] >= limit {
                    continue;
                }
                let dist = cell
                    * if dx != 0 && dy != 0 {
                        std::f32::consts::SQRT_2
                    } else {
                        1.0
                    };
                let jitter = 1.0 + noise * (2.0 * hash01(n as u64 ^ seed.0) - 1.0);
                let candidate = (z - dist * talus * jitter).min(limit);
                if candidate > heights[n] + 0.01 {
                    heights[n] = candidate;
                    heap.push((sortable(candidate), n));
                }
            }
        }
    }
    rebuild(hf, heights)
}

/// Order-preserving f32 -> u32 mapping for the max-heap.
fn sortable(value: f32) -> u32 {
    let bits = value.to_bits();
    if bits & 0x8000_0000 != 0 {
        !bits
    } else {
        bits ^ 0x8000_0000
    }
}

fn hash01(mut x: u64) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x & 0x00ff_ffff) as f32 / 16_777_216.0
}

pub fn dunes(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let amplitude = modifier.value("amplitude_m");
    let wavelength = modifier.value("wavelength_m").max(1.0);
    let angle = modifier.value("angle_deg").to_radians();
    let smoothing = modifier.value("smoothing").clamp(0.0, 1.0);
    let stoss = modifier.value("stoss").clamp(0.1, 0.95);
    let lee = modifier.value("lee").clamp(0.2, 1.0).max(stoss + 0.03);
    if amplitude == 0.0 {
        return Ok(hf.clone());
    }
    let noise = noise_for(seed, "dunes");
    let base = if smoothing > 0.0 {
        let smoothed = vf_terrain::smooth(hf, 2, 1);
        let mut mixed = hf.heights().to_vec();
        let width = hf.width();
        vf_core::compute::install(|| {
            mixed
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, value) in row.iter_mut().enumerate() {
                        let i = y * width + x;
                        *value = hf.heights()[i]
                            + (smoothed.heights()[i] - hf.heights()[i]) * smoothing * 0.6;
                    }
                });
        });
        rebuild(hf, mixed)?
    } else {
        hf.clone()
    };
    let (ox, oy, _, _) = crate::world_extent(&base);
    let (sx, sy) = base.spacing();
    let mut heights = base.heights().to_vec();
    let (wind_x, wind_y) = (angle.cos(), angle.sin());
    let tau = std::f32::consts::TAU / wavelength;
    let width = base.width();
    vf_core::compute::install(|| {
        heights
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, value) in row.iter_mut().enumerate() {
                    if !base.is_valid(x, y) {
                        continue;
                    }
                    let wx = (ox + x as f64 * sx) as f32;
                    let wy = (oy + y as f64 * sy) as f32;
                    let slope = slope_at(&base, x, y);
                    // Dunes only form on sand-flat ground; cliffs keep their
                    // rock.
                    let mask = 1.0 - smoothstep(0.15, 0.50, slope);
                    if mask <= 0.0 {
                        continue;
                    }
                    let along = wx * wind_x + wy * wind_y;
                    let jitter = noise.fbm01(wx * 0.002, wy * 0.002, 3, 2.0, 0.5);
                    let phase = along * tau + jitter * 2.5;
                    // Asymmetric dune profile from the HighMap `wave_dune`
                    // function: a gentle stoss slope, a sharp crest, then a
                    // steep lee face.
                    let xp = (phase / std::f32::consts::TAU).rem_euclid(1.0);
                    let shaped = if xp < stoss {
                        let r = xp / stoss.max(1e-3);
                        r * r * (3.0 - 2.0 * r)
                    } else if xp < lee {
                        let r = (xp - lee) / (stoss - lee);
                        r * r * (2.0 - r)
                    } else {
                        0.0
                    };
                    *value += amplitude * (shaped - 0.45) * mask;
                }
            });
    });
    rebuild(&base, heights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::ModifierKind;

    fn rough(size: usize) -> HeightField {
        let mut heights = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                heights[y * size + x] =
                    (x as f32 * 13.0).sin() * (y as f32 * 7.0).cos() * 30.0 + x as f32 * 3.0;
            }
        }
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn hydraulic_changes_the_surface_deterministically() {
        let hf = rough(65);
        let mut modifier = Modifier::new(ModifierKind::Erosion);
        modifier.set("droplets", 5000.0);
        let a = hydraulic(&hf, &modifier, Seed::new(1)).unwrap();
        let b = hydraulic(&hf, &modifier, Seed::new(1)).unwrap();
        assert_eq!(a.heights(), b.heights());
        assert_ne!(a.heights(), hf.heights());
    }

    #[test]
    fn thermal_relaxes_slopes() {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 0..65 {
                heights[y * 65 + x] = if x < 32 { 300.0 } else { 0.0 };
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let modifier = Modifier::new(ModifierKind::Thermal);
        let out = thermal(&hf, &modifier).unwrap();
        let max_step = |field: &HeightField| {
            let mut max = 0.0f32;
            for x in 0..field.width() - 1 {
                max = max.max((field.get(x + 1, 30) - field.get(x, 30)).abs());
            }
            max
        };
        assert!(max_step(&out) < max_step(&hf));
    }

    #[test]
    fn alluvium_runs_and_changes_the_surface() {
        let hf = rough(65);
        let mut modifier = Modifier::new(ModifierKind::Alluvium);
        modifier.set("droplets", 3000.0);
        let out = alluvium(&hf, &modifier, Seed::new(2)).unwrap();
        assert_ne!(out.heights(), hf.heights());
        assert!(out.heights().iter().all(|v| v.is_finite()));
    }

    #[test]
    fn snow_fills_hollows() {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 0..65 {
                let dx = (x as f32 - 32.0) / 22.0;
                let dy = (y as f32 - 32.0) / 22.0;
                let d = (dx * dx + dy * dy).sqrt();
                if d < 1.0 {
                    heights[y * 65 + x] = -40.0 * (1.0 - d * d);
                }
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let mut modifier = Modifier::new(ModifierKind::Snow);
        modifier.set("amount_m", 50.0);
        let out = snow(&hf, &modifier, Seed::new(3)).unwrap();
        assert!(out.get(32, 32) > hf.get(32, 32));
        assert_eq!(out.get(0, 0), 0.0);
    }

    #[test]
    fn talus_builds_an_apron_at_the_cliff_foot() {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 0..65 {
                heights[y * 65 + x] = if x < 20 { 300.0 } else { 0.0 };
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let mut modifier = Modifier::new(ModifierKind::Talus);
        modifier.set("talus_deg", 40.0);
        modifier.set("depth_m", 80.0);
        let out = talus(&hf, &modifier, Seed::new(2)).unwrap();
        assert!(out.get(22, 32) > 0.0, "no apron at the foot");
        assert_eq!(out.get(60, 32), 0.0, "far ground must stay untouched");
        assert!(out.get(22, 32) <= 80.0 + 1e-3);
    }

    #[test]
    fn dunes_ripple_flat_ground() {
        let hf = HeightField::filled(129, 129, 10.0, 100.0).unwrap();
        let mut modifier = Modifier::new(ModifierKind::Dunes);
        modifier.set("amplitude_m", 10.0);
        modifier.set("wavelength_m", 200.0);
        let out = dunes(&hf, &modifier, Seed::new(4)).unwrap();
        assert!(out.stats().std_dev > 1.0);
        assert!(out.stats().mean > 99.0 && out.stats().mean < 101.0);
    }
}

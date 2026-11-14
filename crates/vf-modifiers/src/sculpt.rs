//! Geological sculpting and combination operators.

use vf_core::{HeightField, Modifier, Result, Seed};
use vf_procgen::Noise;

use crate::{noise_for, normalised, rebuild, sharpen_fraction, slope_at, smoothstep, soft_clamp};

pub fn terrace(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let step = modifier.value("step_m");
    let strength = modifier.value("strength").clamp(0.0, 1.0);
    let sharpness = modifier.value("sharpness").clamp(0.0, 1.0);
    let gamma = modifier.value("gamma").clamp(0.1, 2.0);
    let variation = modifier.value("variation").clamp(0.0, 1.0);
    if step <= 0.0 || strength == 0.0 {
        return Ok(hf.clone());
    }
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let z = hf.get(x, y);
            let t = z / step;
            let base = t.floor();
            let frac = t - base;
            // Each bed gets its own gamma, like the HighMap strata kernel:
            // a stack of identical steps reads as a staircase, a stack of
            // subtly different profiles reads as rock.
            let jitter = 2.0 * hash01(base as i64 as u64 ^ seed.0) - 1.0;
            let gamma_r = (gamma * (1.0 + variation * jitter)).clamp(0.05, 10.0);
            let smooth = gamma_profile(frac, gamma_r);
            // `sharpness` blends the geological profile toward a hard step.
            let hard = sharpen_fraction(frac, sharpness);
            let shaped = (base + smooth * (1.0 - sharpness) + hard * sharpness) * step;
            heights[y * hf.width() + x] = z + (shaped - z) * strength;
        }
    }
    rebuild(hf, heights)
}

/// Piecewise gamma terrace profile from the HighMap strata kernel.
fn gamma_profile(u: f32, gamma: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    if (gamma - 1.0).abs() < 0.01 {
        return u;
    }
    let a = (1.0 / gamma).powf(1.0 / (gamma - 1.0));
    let b = gamma.powf(-gamma / (gamma - 1.0));
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || a >= 1.0 || !(0.0..=1.0).contains(&b) {
        return u;
    }
    if u < a {
        u * b / a
    } else {
        b + (1.0 - b) * (u - a) / (1.0 - a)
    }
}

/// Cheap deterministic hash for per-bed variation.
fn hash01(mut x: u64) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x & 0x00ff_ffff) as f32 / 16_777_216.0
}

pub fn stratify(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let thickness = modifier.value("thickness_m").max(0.01);
    let depth = modifier.value("depth_m");
    let dip = modifier.value("dip_deg").to_radians();
    let azimuth = modifier.value("azimuth_deg").to_radians();
    let steep_only = modifier.value("steep_only").clamp(0.0, 1.0);
    if depth == 0.0 {
        return Ok(hf.clone());
    }
    let noise = noise_for(seed, "stratify");
    let (ox, oy, _, _) = crate::world_extent(hf);
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let slope = slope_at(hf, x, y);
            let weight = 1.0 - steep_only + steep_only * smoothstep(0.25, 0.65, slope);
            if weight <= 0.0 {
                continue;
            }
            let wx = ox + x as f64 * sx;
            let wy = oy + y as f64 * sy;
            let run = (wx * f64::from(azimuth.cos()) + wy * f64::from(azimuth.sin())) as f32;
            let z = hf.get(x, y);
            // Tilted stratigraphic column: phase advances upward but also
            // sideways along the dip azimuth.
            let phase = (z - run * dip.tan()) / thickness;
            let frac = phase.rem_euclid(1.0);
            let band = smoothstep(0.32, 0.68, frac) - 0.5;
            let jitter =
                0.75 + 0.25 * noise.fbm01(wx as f32 * 0.015, wy as f32 * 0.015, 2, 2.0, 0.5);
            heights[y * hf.width() + x] += depth * weight * band * 2.0 * jitter;
        }
    }
    rebuild(hf, heights)
}

pub fn aperture(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let amount = modifier.value("amount").clamp(-1.0, 1.0);
    if amount == 0.0 {
        return Ok(hf.clone());
    }
    let (sx, sy) = hf.spacing();
    let cell = (sx.max(sy) as f32).max(0.001);
    let radius = (modifier.value("radius_m") / cell).round().clamp(1.0, 24.0) as usize;
    let dilate = amount > 0.0;
    let width = hf.width();
    let height = hf.height();
    let extreme = |a: f32, b: f32| if dilate { a.max(b) } else { a.min(b) };
    let empty = if dilate {
        f32::NEG_INFINITY
    } else {
        f32::INFINITY
    };

    // Separable max/min filter: horizontal then vertical. A square window is
    // enough for a dilation/contraction of landforms.
    let mut tmp = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) {
                tmp[y * width + x] = hf.get(x, y);
                continue;
            }
            let mut value = empty;
            for k in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                if hf.is_valid(k, y) {
                    value = extreme(value, hf.get(k, y));
                }
            }
            tmp[y * width + x] = value;
        }
    }
    let mut heights = hf.heights().to_vec();
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) {
                continue;
            }
            let mut value = empty;
            for k in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                if hf.is_valid(x, k) {
                    value = extreme(value, tmp[k * width + x]);
                }
            }
            if !value.is_finite() {
                value = hf.get(x, y);
            }
            let z = hf.get(x, y);
            heights[y * width + x] = z + (value - z) * amount.abs();
        }
    }
    rebuild(hf, heights)
}

pub fn clamp(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let mut min = modifier.value("min_m");
    let mut max = modifier.value("max_m");
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    let feather = modifier.value("feather_m").max(0.0);
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let i = y * hf.width() + x;
            heights[i] = soft_clamp(heights[i], min, max, feather);
        }
    }
    rebuild(hf, heights)
}

pub fn blur(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let (sx, sy) = hf.spacing();
    let cell = (sx.max(sy) as f32).max(0.001);
    let radius = (modifier.value("radius_m") / cell).round().max(1.0) as usize;
    let iterations = modifier.integer("iterations").clamp(1, 8) as usize;
    Ok(vf_terrain::smooth(hf, radius, iterations))
}

pub fn sharpen(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let amount = modifier.value("amount").max(0.0);
    if amount == 0.0 {
        return Ok(hf.clone());
    }
    let (sx, sy) = hf.spacing();
    let cell = (sx.max(sy) as f32).max(0.001);
    let radius = (modifier.value("radius_m") / cell).round().max(1.0) as usize;
    let blurred = vf_terrain::smooth(hf, radius, 1);
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let i = y * hf.width() + x;
            let detail = heights[i] - blurred.heights()[i];
            heights[i] += detail * amount;
        }
    }
    rebuild(hf, heights)
}

pub fn crater(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let count = modifier.integer("count").clamp(1, 32) as usize;
    let radius_m = f64::from(modifier.value("radius_m").max(1.0));
    let depth = modifier.value("depth_m").max(0.0);
    let rim = modifier.value("rim").clamp(0.0, 0.6);
    let (ox, oy, ex, ey) = crate::world_extent(hf);
    let (sx, sy) = hf.spacing();
    let mut rng = seed.derive("crater").rng();
    let mut heights = hf.heights().to_vec();
    for _ in 0..count {
        let radius = radius_m * (0.7 + 0.6 * rng.next_f64());
        let margin = radius * 1.35;
        if margin * 2.0 >= ex.min(ey) {
            continue;
        }
        let cx = ox + margin + rng.next_f64() * (ex - margin * 2.0);
        let cy = oy + margin + rng.next_f64() * (ey - margin * 2.0);
        let x0 = (((cx - margin - ox) / sx).floor()).max(0.0) as usize;
        let x1 = (((cx + margin - ox) / sx).ceil()).min((hf.width() - 1) as f64) as usize;
        let y0 = (((cy - margin - oy) / sy).floor()).max(0.0) as usize;
        let y1 = (((cy + margin - oy) / sy).ceil()).min((hf.height() - 1) as f64) as usize;
        for y in y0..=y1 {
            for x in x0..=x1 {
                if !hf.is_valid(x, y) {
                    continue;
                }
                let wx = ox + x as f64 * sx;
                let wy = oy + y as f64 * sy;
                let d = ((wx - cx).powi(2) + (wy - cy).powi(2)).sqrt() / radius;
                let mut delta = 0.0f32;
                if d < 1.0 {
                    let bowl = (1.0 - d * d).max(0.0).powf(1.5);
                    delta -= (bowl as f32) * depth;
                }
                // Raised lip just outside the cavity.
                let lip = (-((d - 1.02) / 0.10).powi(2)).exp();
                delta += (lip as f32) * depth * rim;
                heights[y * hf.width() + x] += delta;
            }
        }
    }
    rebuild(hf, heights)
}

/// Poisson-based cliff recast.
///
/// Port of the HighMap `recast_cliff` idea: instead of snapping heights to a
/// ladder (which always leaves terraces), compute the gradient field, amplify
/// it where the slope exceeds the talus angle, and solve a Poisson equation
/// whose solution has that amplified gradient while keeping the border fixed.
/// The result is a smooth, integrable surface with genuinely steep walls.
///
/// The red-black SOR solve is the expensive part; `iterations` trades quality
/// for time. The original algorithm uses the same approach on the GPU.
pub fn recast_cliff(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let talus = modifier
        .value("talus_deg")
        .clamp(1.0, 89.0)
        .to_radians()
        .tan();
    let amplitude = modifier.value("amplitude").clamp(0.0, 2.0);
    let gain = modifier.value("gain").clamp(1.0, 8.0);
    let iterations = modifier.integer("iterations").clamp(20, 2000) as usize;
    if amplitude == 0.0 || (gain - 1.0).abs() < 1e-6 {
        return Ok(hf.clone());
    }
    let width = hf.width();
    let height = hf.height();
    if width < 4 || height < 4 {
        return Ok(hf.clone());
    }
    let (sx, sy) = hf.spacing();
    let cell = sx.max(sy) as f32;
    let (ox, oy) = hf.origin();

    // Slope mask from a smoothed copy: a sharp cliff edge must not be split
    // into per-cell decisions.
    let radius_m = modifier.value("radius_m").max(0.0);
    let radius = (radius_m / cell).round() as usize;
    let base = if radius > 0 {
        vf_terrain::smooth(hf, radius, 1)
    } else {
        hf.clone()
    };
    let interior = |x: usize, y: usize| x > 0 && y > 0 && x + 1 < width && y + 1 < height;
    let mut dn = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) || !interior(x, y) {
                continue;
            }
            let (gx, gy) = base.gradient_at(x, y);
            let slope = (gx * gx + gy * gy).sqrt();
            let excess = ((slope - talus) / 0.25).clamp(0.0, 1.0);
            dn[y * width + x] = excess;
        }
    }
    if radius > 0 {
        // Smooth the mask itself through a temporary heightfield.
        let (sx, sy) = hf.spacing();
        if let Ok(mask_field) = HeightField::new(width, height, sx, sy, dn.clone()) {
            let smoothed = vf_terrain::smooth(&mask_field, radius.min(16), 1);
            dn.copy_from_slice(smoothed.heights());
        }
    }

    // Amplified gradient field and its divergence (Poisson right-hand side).
    let mut gx = vec![0.0f32; width * height];
    let mut gy = vec![0.0f32; width * height];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if !hf.is_valid(x, y) {
                continue;
            }
            let i = y * width + x;
            let dx = (hf.get(x + 1, y) - hf.get(x - 1, y)) / (2.0 * cell);
            let dy = (hf.get(x, y + 1) - hf.get(x, y - 1)) / (2.0 * cell);
            let scale = 1.0 + amplitude * (gain - 1.0) * dn[i];
            gx[i] = dx * scale;
            gy[i] = dy * scale;
        }
    }
    let inv_2h = 1.0 / (2.0 * cell);
    let mut rho = vec![0.0f32; width * height];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if !hf.is_valid(x, y) {
                continue;
            }
            let i = y * width + x;
            rho[i] = (gx[i + 1] - gx[i - 1]) * inv_2h + (gy[i + width] - gy[i - width]) * inv_2h;
        }
    }

    // Red-black SOR on the interior. Borders and masked cells are Dirichlet.
    let mut u = hf.heights().to_vec();
    let nx = width as f32;
    let ny = height as f32;
    let rho_jac = 0.5 * ((std::f32::consts::PI / nx).cos() + (std::f32::consts::PI / ny).cos());
    let omega = 2.0 / (1.0 + (1.0 - rho_jac * rho_jac).max(0.0).sqrt());
    let h2 = cell * cell;
    for _ in 0..iterations {
        for color in 0..2usize {
            for y in 1..height - 1 {
                for x in 1..width - 1 {
                    if (x + y) % 2 != color || !hf.is_valid(x, y) {
                        continue;
                    }
                    let i = y * width + x;
                    let jacobi =
                        (u[i - 1] + u[i + 1] + u[i - width] + u[i + width] - rho[i] * h2) * 0.25;
                    u[i] += omega * (jacobi - u[i]);
                }
            }
        }
    }

    // The SOR solve leaves tiny high-frequency residuals that the rock
    // normal detail happily amplifies into moire. Smooth only the *added
    // deformation*: the original terrain detail and the large-scale cliff
    // shape are untouched.
    if let Ok(deformation) = HeightField::new(
        width,
        height,
        hf.spacing().0,
        hf.spacing().1,
        u.iter()
            .zip(hf.heights())
            .map(|(solved, original)| solved - original)
            .collect(),
    ) {
        let smoothed = vf_terrain::smooth(&deformation, 1, 1);
        // Apply the smoothed deformation to the original heights.
        for y in 0..height {
            for x in 0..width {
                if hf.is_valid(x, y) {
                    let i = y * width + x;
                    u[i] = hf.get(x, y) + smoothed.heights()[i];
                }
            }
        }
    }

    // Preserve the input amplitude, as the reference implementation does.
    let stats = hf.stats();
    let mut new_min = f32::INFINITY;
    let mut new_max = f32::NEG_INFINITY;
    for y in 0..height {
        for x in 0..width {
            if hf.is_valid(x, y) {
                let value = u[y * width + x];
                new_min = new_min.min(value);
                new_max = new_max.max(value);
            }
        }
    }
    let new_relief = (new_max - new_min).max(1e-6);
    let relief = stats.relief().max(1e-6);
    for y in 0..height {
        for x in 0..width {
            if hf.is_valid(x, y) {
                let i = y * width + x;
                u[i] = stats.min + (u[i] - new_min) / new_relief * relief;
            }
        }
    }
    let _ = (ox, oy);
    rebuild(hf, u)
}

pub fn combine(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let operation = modifier.integer("operation").clamp(0, 4);
    let amplitude = modifier.value("amplitude_m");
    let scale = modifier.value("scale_m").max(1.0);
    let weight = modifier.value("weight").clamp(0.0, 1.0);
    let ridged = modifier.value("ridged").clamp(0.0, 1.0);
    if amplitude == 0.0 || weight == 0.0 {
        return Ok(hf.clone());
    }
    let noise = Noise::new(seed.derive("combine"));
    let (ox, oy, _, _) = crate::world_extent(hf);
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = (ox + x as f64 * sx) as f32;
            let wy = (oy + y as f64 * sy) as f32;
            let fbm = noise.fbm01(wx / scale, wy / scale, 5, 2.0, 0.5);
            let ridge = noise.ridged2(wx / scale + 4.1, wy / scale - 2.7, 4, 2.0, 0.5);
            let n = fbm * (1.0 - ridged) + ridge * ridged;
            let secondary = (n - 0.5) * 2.0 * amplitude;
            let i = y * hf.width() + x;
            heights[i] += match operation {
                1 => -secondary * weight,
                2 => secondary.max(0.0) * weight,
                3 => secondary.min(0.0) * weight,
                4 => secondary * weight * 0.5,
                _ => secondary * weight,
            };
        }
    }
    rebuild(hf, heights)
}

pub fn bias_gain(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let bias = modifier.value("bias").clamp(-1.0, 1.0);
    let gain = modifier.value("gain").clamp(-1.0, 1.0);
    let pivot = modifier.value("pivot").clamp(0.001, 0.999);
    if bias == 0.0 && gain == 0.0 {
        return Ok(hf.clone());
    }
    let (values, min, relief) = normalised(hf);
    let exponent = 2f32.powf(-bias * 2.0);
    let contrast = 2f32.powf(gain * 2.0);
    let mut heights = hf.heights().to_vec();
    for (i, &t) in values.iter().enumerate() {
        let mut v = t.powf(exponent);
        v = pivot + (v - pivot) * contrast;
        heights[i] = min + v.clamp(0.0, 1.0) * relief;
    }
    rebuild(hf, heights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::ModifierKind;

    fn ramp(size: usize, drop: f32) -> HeightField {
        let mut heights = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                heights[y * size + x] = x as f32 * drop;
            }
        }
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn terrace_steepens_a_ramp() {
        let hf = ramp(65, 20.0);
        let modifier = Modifier::new(ModifierKind::Terrace);
        let out = terrace(&hf, &modifier, Seed::new(1)).unwrap();
        let max_step = |field: &HeightField| {
            let mut max = 0.0f32;
            for x in 0..field.width() - 1 {
                max = max.max((field.get(x + 1, 0) - field.get(x, 0)).abs());
            }
            max
        };
        assert!(max_step(&out) > max_step(&hf));
    }

    #[test]
    fn gamma_profile_is_monotonic_and_bounded() {
        for gamma in [0.2f32, 0.5, 1.0, 1.5, 2.0] {
            let mut previous = -1.0;
            for i in 0..=100 {
                let u = i as f32 / 100.0;
                let v = gamma_profile(u, gamma);
                assert!((0.0..=1.0).contains(&v), "gamma {gamma} gives {v}");
                assert!(v >= previous - 1e-5, "gamma {gamma} is not monotonic");
                previous = v;
            }
        }
    }

    #[test]
    fn terrace_variation_breaks_the_uniform_ladder() {
        let hf = ramp(65, 20.0);
        let mut modifier = Modifier::new(ModifierKind::Terrace);
        modifier.set("strength", 1.0);
        modifier.set("sharpness", 0.0);
        modifier.set("variation", 0.9);
        let out = terrace(&hf, &modifier, Seed::new(3)).unwrap();
        // The same fractional position in two different beds must not always
        // produce the same offset.
        let mut offsets = std::collections::HashSet::new();
        for x in 1..64 {
            let off = (out.get(x, 10) - hf.get(x, 10)).round() as i32;
            offsets.insert(off);
        }
        assert!(offsets.len() > 1, "terrace variation had no effect");
    }

    #[test]
    fn blur_reduces_and_sharpen_restores_roughness() {
        let hf = ramp(65, 20.0);
        let mut blurred = hf.clone();
        // Add a checkerboard so blur has something to remove.
        for y in 0..65 {
            for x in 0..65 {
                if (x + y) % 2 == 0 {
                    blurred.set(x, y, blurred.get(x, y) + 50.0);
                }
            }
        }
        let mut modifier = Modifier::new(ModifierKind::Blur);
        modifier.set("radius_m", 30.0);
        modifier.set("iterations", 2.0);
        let smooth = blur(&blurred, &modifier).unwrap();
        assert!(smooth.stats().std_dev < blurred.stats().std_dev);
        let mut sharpen = Modifier::new(ModifierKind::Sharpen);
        sharpen.set("radius_m", 30.0);
        sharpen.set("amount", 1.0);
        let sharp = super::sharpen(&smooth, &sharpen).unwrap();
        assert!(sharp.stats().std_dev > smooth.stats().std_dev);
    }

    #[test]
    fn clamp_bounds_altitudes() {
        let hf = ramp(33, 20.0);
        let mut modifier = Modifier::new(ModifierKind::Clamp);
        modifier.set("min_m", 100.0);
        modifier.set("max_m", 300.0);
        let out = clamp(&hf, &modifier).unwrap();
        assert!(out.stats().min >= 100.0 - 1e-3);
        assert!(out.stats().max <= 300.0 + 1e-3);
    }

    #[test]
    fn aperture_dilation_raises_the_low_ground() {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 25..40 {
            for x in 25..40 {
                heights[y * 65 + x] = 100.0;
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let mut modifier = Modifier::new(ModifierKind::Aperture);
        modifier.set("radius_m", 50.0);
        modifier.set("amount", 1.0);
        let out = aperture(&hf, &modifier).unwrap();
        assert!(out.get(22, 32) > 0.0, "dilation did not spread the hill");
    }

    #[test]
    fn crater_digs_a_bowl_with_a_rim() {
        let hf = HeightField::filled(129, 129, 10.0, 100.0).unwrap();
        let mut modifier = Modifier::new(ModifierKind::Crater);
        modifier.set("count", 1.0);
        modifier.set("radius_m", 200.0);
        modifier.set("depth_m", 100.0);
        modifier.set("rim", 0.3);
        let out = crater(&hf, &modifier, Seed::new(4)).unwrap();
        assert!(out.stats().min < 20.0);
        assert!(out.stats().max > 100.0);
    }

    #[test]
    fn bias_gain_spreads_the_histogram() {
        let hf = ramp(33, 10.0);
        let mut modifier = Modifier::new(ModifierKind::BiasGain);
        modifier.set("gain", 0.8);
        let out = bias_gain(&hf, &modifier).unwrap();
        let spread = |field: &HeightField| {
            let stats = field.stats();
            stats.std_dev / stats.relief().max(1.0)
        };
        assert!(spread(&out) > spread(&hf));
    }

    #[test]
    fn recast_cliff_steepens_a_rounded_escarpment() {
        // A smooth 400 m escarpment: the recast must concentrate the drop
        // into a steeper face while keeping the plateau and floor.
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 0..65 {
                let t = ((x as f32 - 20.0) / 20.0).clamp(0.0, 1.0);
                heights[y * 65 + x] = 400.0 * (1.0 - t * t * (3.0 - 2.0 * t));
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let mut modifier = Modifier::new(ModifierKind::RecastCliff);
        modifier.set("talus_deg", 30.0);
        modifier.set("radius_m", 0.0);
        modifier.set("iterations", 300.0);
        let out = super::recast_cliff(&hf, &modifier).unwrap();
        assert!(out.heights().iter().all(|v| v.is_finite()));
        let max_step = |field: &HeightField| {
            let mut max = 0.0f32;
            for x in 1..field.width() - 2 {
                max = max.max((field.get(x + 1, 20) - field.get(x, 20)).abs());
            }
            max
        };
        assert!(
            max_step(&out) > max_step(&hf) * 1.05,
            "{} -> {}",
            max_step(&hf),
            max_step(&out)
        );
        let (in_stats, out_stats) = (hf.stats(), out.stats());
        assert!((in_stats.min - out_stats.min).abs() < 1.0);
        assert!((in_stats.max - out_stats.max).abs() < 1.0);
    }

    #[test]
    fn combine_changes_the_surface() {
        let hf = HeightField::filled(65, 65, 10.0, 100.0).unwrap();
        let modifier = Modifier::new(ModifierKind::Combine);
        let out = combine(&hf, &modifier, Seed::new(2)).unwrap();
        assert!(out.stats().std_dev > 1.0);
    }
}

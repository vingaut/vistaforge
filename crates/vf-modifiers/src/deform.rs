//! Deformation operators: displace, warp, transform, swirl and fold.

use vf_core::{HeightField, Modifier, Result, Seed};

use crate::{noise_for, rebuild, rotate, sample_world, slope_at, world_extent};

pub fn displace(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let amount = modifier.value("amount_m");
    if amount == 0.0 {
        return Ok(hf.clone());
    }
    let scale = modifier.value("scale_m").max(1.0);
    let angle = modifier.value("angle_deg").to_radians();
    let ridged = modifier.value("ridged").clamp(0.0, 1.0);
    let noise = noise_for(seed, "displace");
    let (ox, oy, _, _) = world_extent(hf);
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = (ox + x as f64 * sx) as f32;
            let wy = (oy + y as f64 * sy) as f32;
            let (u, v) = rotate(wx, wy, angle);
            // Stretch across the direction: elongated ridges instead of blobs.
            let (a, b) = (u / scale, v / (scale * 0.45));
            let fbm = noise.fbm01(a, b, 5, 2.0, 0.5);
            let ridge = noise.ridged2(a + 13.7, b - 8.1, 3, 2.0, 0.5);
            let n = fbm * (1.0 - ridged) + ridge * ridged;
            let i = y * hf.width() + x;
            heights[i] += (n - 0.5) * 2.0 * amount;
        }
    }
    rebuild(hf, heights)
}

pub fn warp(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let amount = modifier.value("amount_m");
    if amount == 0.0 {
        return Ok(hf.clone());
    }
    let scale = modifier.value("scale_m").max(1.0);
    let angle = modifier.value("angle_deg").to_radians();
    let slope_bias = modifier.value("slope_bias").clamp(0.0, 1.0);
    let noise = noise_for(seed, "warp");
    let (ox, oy, _, _) = world_extent(hf);
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = ox + x as f64 * sx;
            let wy = oy + y as f64 * sy;
            let slope = slope_at(hf, x, y);
            // Directional warp: the noise is stretched along `angle`, and the
            // displacement follows the local slope when bias is high.
            let bias = 1.0 - slope_bias + slope_bias * crate::smoothstep(0.25, 1.20, slope);
            let (u, v) = rotate(wx as f32, wy as f32, angle);
            let n1 = noise.fbm2(u / scale, v / (scale * 0.5), 4, 2.0, 0.5);
            let n2 = noise.fbm2(u / scale + 31.7, v / (scale * 0.5) - 17.3, 4, 2.0, 0.5);
            let (dx, dy) = rotate(n1, n2 * 0.6, angle);
            let px = wx + f64::from(dx * amount * bias);
            let py = wy + f64::from(dy * amount * bias);
            heights[y * hf.width() + x] = sample_world(hf, px, py, false);
        }
    }
    rebuild(hf, heights)
}

pub fn transform(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let offset_x = f64::from(modifier.value("offset_x_m"));
    let offset_y = f64::from(modifier.value("offset_y_m"));
    let rotation = modifier.value("rotation_deg").to_radians();
    let scale = f64::from(modifier.value("scale").max(0.1));
    let repeat = modifier.value("repeat") >= 0.5;
    if offset_x == 0.0 && offset_y == 0.0 && rotation == 0.0 && (scale - 1.0).abs() < 1e-6 {
        return Ok(hf.clone());
    }
    let (ox, oy, ex, ey) = world_extent(hf);
    let cx = ox + ex * 0.5;
    let cy = oy + ey * 0.5;
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = ox + x as f64 * sx;
            let wy = oy + y as f64 * sy;
            // Inverse transform around the terrain centre.
            let dx = wx - cx - offset_x;
            let dy = wy - cy - offset_y;
            let (rx, ry) = rotate(dx as f32, dy as f32, -rotation);
            let qx = cx + f64::from(rx) / scale;
            let qy = cy + f64::from(ry) / scale;
            heights[y * hf.width() + x] = sample_world(hf, qx, qy, repeat);
        }
    }
    rebuild(hf, heights)
}

pub fn swirl(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let radius = f64::from(modifier.value("radius_m").max(1.0));
    let angle = modifier.value("angle_deg").to_radians();
    let falloff = modifier.value("falloff").clamp(0.0, 1.0);
    if angle == 0.0 {
        return Ok(hf.clone());
    }
    let (ox, oy, ex, ey) = world_extent(hf);
    let cx = ox + ex * f64::from(modifier.value("center_x").clamp(0.0, 1.0));
    let cy = oy + ey * f64::from(modifier.value("center_y").clamp(0.0, 1.0));
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = ox + x as f64 * sx;
            let wy = oy + y as f64 * sy;
            let dx = wx - cx;
            let dy = wy - cy;
            let r = (dx * dx + dy * dy).sqrt();
            if r >= radius {
                heights[y * hf.width() + x] = hf.get(x, y);
                continue;
            }
            let t = (1.0 - r / radius) as f32;
            let twist = angle * t.powf(1.0 + 1.5 * falloff);
            let (rx, ry) = rotate(dx as f32, dy as f32, -twist);
            heights[y * hf.width() + x] =
                sample_world(hf, cx + f64::from(rx), cy + f64::from(ry), false);
        }
    }
    rebuild(hf, heights)
}

pub fn fold(hf: &HeightField, modifier: &Modifier) -> Result<HeightField> {
    let strength = modifier.value("strength").clamp(0.0, 1.0);
    if strength == 0.0 {
        return Ok(hf.clone());
    }
    let angle = modifier.value("angle_deg").to_radians();
    let period = f64::from(modifier.value("period_m").max(1.0));
    let amplitude = modifier.value("amplitude_m");
    let (ox, oy, ex, ey) = world_extent(hf);
    let cx = ox + ex * 0.5;
    let cy = oy + ey * 0.5;
    let (dir_x, dir_y) = (angle.cos(), angle.sin());
    let (sx, sy) = hf.spacing();
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = ox + x as f64 * sx;
            let wy = oy + y as f64 * sy;
            let dx = wx - cx;
            let dy = wy - cy;
            let u = dx * f64::from(dir_x) + dy * f64::from(dir_y);
            let v = -dx * f64::from(dir_y) + dy * f64::from(dir_x);
            let phase = (u / period).rem_euclid(2.0);
            let triangle = 1.0 - (phase - 1.0).abs();
            let u_fold = period * triangle;
            let u_mix = u + (u_fold - u) * f64::from(strength);
            let qx = cx + u_mix * f64::from(dir_x) - v * f64::from(dir_y);
            let qy = cy + u_mix * f64::from(dir_y) + v * f64::from(dir_x);
            let ridge = (1.0 - (phase - 1.0).abs()) as f32;
            let accent = amplitude * (ridge * ridge - 0.35) * strength;
            heights[y * hf.width() + x] = sample_world(hf, qx, qy, false) + accent;
        }
    }
    rebuild(hf, heights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::ModifierKind;

    fn ramp(size: usize) -> HeightField {
        let mut heights = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                heights[y * size + x] = x as f32 * 10.0;
            }
        }
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn displace_moves_a_flat_field() {
        let hf = HeightField::filled(65, 65, 10.0, 100.0).unwrap();
        let modifier = Modifier::new(ModifierKind::Displace);
        let out = displace(&hf, &modifier, Seed::new(1)).unwrap();
        assert!(out.stats().std_dev > 1.0);
        assert!(
            (out.stats().mean - 100.0).abs() < 25.0,
            "displacement should stay centred: {}",
            out.stats().mean
        );
    }

    #[test]
    fn transform_offset_moves_the_ramp() {
        let hf = ramp(65);
        let mut modifier = Modifier::new(ModifierKind::Transform);
        modifier.set("offset_x_m", 100.0);
        let out = transform(&hf, &modifier).unwrap();
        // The terrain moved 100 m east: the sample at x=20 is now the source
        // at x=10.
        assert!((out.get(20, 20) - 100.0).abs() < 1e-3);
        assert!((out.get(0, 20) - hf.get(0, 20)).abs() < 1e-3);
    }

    #[test]
    fn swirl_is_identity_outside_its_radius() {
        let hf = ramp(65);
        let mut modifier = Modifier::new(ModifierKind::Swirl);
        modifier.set("radius_m", 100.0);
        modifier.set("center_x", 0.5);
        modifier.set("center_y", 0.5);
        let out = swirl(&hf, &modifier).unwrap();
        assert_eq!(out.get(0, 0), hf.get(0, 0));
        assert_ne!(out.get(32, 25), hf.get(32, 25));
    }

    #[test]
    fn fold_stays_finite() {
        let hf = ramp(65);
        let modifier = Modifier::new(ModifierKind::Fold);
        let out = fold(&hf, &modifier).unwrap();
        assert!(out.heights().iter().all(|v| v.is_finite()));
    }
}

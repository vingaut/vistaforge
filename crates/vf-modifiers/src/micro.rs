//! Micro-detail operators: grooves, ridges, crater fields, exfoliation,
//! vesicles and undercut ledges.
//!
//! These are heightfield operators, not the continuous render-time detail
//! field of `vf-detail`. They are used when the artist wants to bake the
//! relief into the DEM (so it survives export to a plain heightfield, feeds
//! erosion, or participates in the stored project). Every distance is in
//! metres and every feature is widened to at least two cells: a groove
//! narrower than the grid is noise, not a groove.

use vf_core::{HeightField, Modifier, Result, Seed};

use crate::{noise_for, rebuild, rotate, slope_at, smoothstep, world_extent};

const TAU: f32 = std::f32::consts::TAU;

fn cell_size(hf: &HeightField) -> f32 {
    let (sx, sy) = hf.spacing();
    (sx.max(sy) as f32).max(0.001)
}

fn slope_weight(slope: f32, slope_only: f32) -> f32 {
    let slope_only = slope_only.clamp(0.0, 1.0);
    1.0 - slope_only + slope_only * smoothstep(0.18, 0.70, slope)
}

fn world_cell(hf: &HeightField, x: usize, y: usize) -> (f32, f32) {
    let (ox, oy, _, _) = world_extent(hf);
    let (sx, sy) = hf.spacing();
    (
        (ox + x as f64 * sx) as f32,
        (oy + y as f64 * sy) as f32,
    )
}

/// Directional solution flutes / runnels.
pub fn grooves(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let depth = modifier.value("depth_m");
    if depth == 0.0 {
        return Ok(hf.clone());
    }
    let cell = cell_size(hf);
    let spacing = modifier.value("spacing_m").max(cell * 2.0);
    let angle = modifier.value("angle_deg").to_radians();
    let sharpness = modifier.value("sharpness").clamp(0.0, 1.0);
    let wander = modifier.value("wander").clamp(0.0, 1.0);
    let slope_only = modifier.value("slope_only");
    let noise = noise_for(seed, "grooves");
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let slope = slope_at(hf, x, y);
            let weight = slope_weight(slope, slope_only);
            if weight <= 0.0 {
                continue;
            }
            let (wx, wy) = world_cell(hf, x, y);
            let (u, v) = rotate(wx, wy, angle);
            let wander_m = noise.fbm2(
                u / (spacing * 6.0),
                v / (spacing * 6.0),
                3,
                2.0,
                0.5,
            ) * wander
                * spacing
                * 0.8;
            let phase = (u + wander_m) / spacing * TAU;
            let channel = (0.5 + 0.5 * phase.cos()).powf(0.6 + sharpness * 3.0);
            let i = y * hf.width() + x;
            heights[i] -= depth * channel * weight;
        }
    }
    rebuild(hf, heights)
}

/// Sharp crests and arêtes.
pub fn ridges(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let amplitude = modifier.value("amplitude_m");
    if amplitude == 0.0 {
        return Ok(hf.clone());
    }
    let cell = cell_size(hf);
    let scale = modifier.value("scale_m").max(cell * 2.0);
    let sharpness = modifier.value("sharpness").clamp(0.0, 1.0);
    let anisotropy = modifier.value("anisotropy").max(1.0);
    let angle = modifier.value("angle_deg").to_radians();
    let slope_only = modifier.value("slope_only");
    let noise = noise_for(seed, "ridges");
    let exponent = 1.0 / (0.35 + sharpness);
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let slope = slope_at(hf, x, y);
            let weight = slope_weight(slope, slope_only);
            if weight <= 0.0 {
                continue;
            }
            let (wx, wy) = world_cell(hf, x, y);
            let (u, v) = rotate(wx, wy, angle);
            let n = noise.ridged2(u / scale, v / (scale * anisotropy), 6, 2.0, 0.5);
            let crest = (n * 2.0 - 1.0).clamp(-1.0, 1.0);
            let crest = crest.signum() * crest.abs().powf(exponent);
            heights[y * hf.width() + x] += amplitude * crest * weight;
        }
    }
    rebuild(hf, heights)
}

/// A deterministic field of impact / solution craters with rims and ejecta.
pub fn crater_field(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let depth = modifier.value("depth_m");
    let density = modifier.value("density").clamp(0.0, 1.0);
    if depth == 0.0 || density <= 0.0 {
        return Ok(hf.clone());
    }
    let cell = cell_size(hf);
    let spacing = modifier.value("spacing_m").max(cell * 3.0);
    let radius = modifier.value("radius_m").max(cell * 2.0);
    let rim = modifier.value("rim").clamp(0.0, 1.0);
    let ejecta = modifier.value("ejecta").max(0.0);
    let noise = noise_for(seed, "crater-field");
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let (wx, wy) = world_cell(hf, x, y);
            let gx = (wx / spacing).floor() as i32;
            let gy = (wy / spacing).floor() as i32;
            let mut delta = 0.0f32;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let cx = gx + dx;
                    let cy = gy + dy;
                    if hash01(cx, cy, seed.0) > density {
                        continue;
                    }
                    let px = (cx as f32 + 0.12 + 0.76 * hash01(cx + 17, cy - 9, seed.0))
                        * spacing;
                    let py = (cy as f32 + 0.12 + 0.76 * hash01(cx - 5, cy + 23, seed.0))
                        * spacing;
                    let r = radius * (0.35 + 1.3 * hash01(cx + 3, cy + 7, seed.0));
                    let d = ((wx - px).powi(2) + (wy - py).powi(2)).sqrt() / r.max(0.2);
                    if d < 1.0 {
                        delta -= depth * (1.0 - d * d).max(0.0).powi(2);
                    }
                    if rim > 0.0 {
                        delta += depth * rim * (-(d - 1.0).powi(2) / 0.02).exp();
                    }
                    if ejecta > 0.0 && d < 1.0 + ejecta {
                        let t = ((d - 1.0) / ejecta.max(0.01)).clamp(0.0, 1.0);
                        let grain = noise.noise2(wx / (r * 0.35), wy / (r * 0.35));
                        delta += depth * ejecta * 0.12 * (1.0 - t).powi(2) * grain;
                    }
                }
            }
            heights[y * hf.width() + x] += delta;
        }
    }
    rebuild(hf, heights)
}

/// Exfoliation sheets: stacked, warped plates peeling off massive rock.
pub fn exfoliate(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let amplitude = modifier.value("amplitude_m");
    if amplitude == 0.0 {
        return Ok(hf.clone());
    }
    let cell = cell_size(hf);
    let scale = modifier.value("scale_m").max(cell * 1.5);
    let sharpness = modifier.value("sharpness").clamp(0.0, 1.0);
    let layers = modifier.integer("layers").clamp(1, 8) as f32;
    let slope_only = modifier.value("slope_only");
    let noise = noise_for(seed, "exfoliate");
    let steps = layers * 2.0 + 1.0;
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let slope = slope_at(hf, x, y);
            let weight = slope_weight(slope, slope_only);
            if weight <= 0.0 {
                continue;
            }
            let (wx, wy) = world_cell(hf, x, y);
            let f = noise.fbm2(wx / scale, wy / scale, 5, 2.0, 0.55);
            let q = (f * steps).round() / steps;
            let shell = (q - f) * 2.0;
            let edge = 1.0 - smoothstep(0.0, 0.10, (f - q).abs());
            let sharp = 0.35 + 0.65 * sharpness;
            heights[y * hf.width() + x] +=
                amplitude * shell * (0.55 + 0.45 * edge) * sharp * weight;
        }
    }
    rebuild(hf, heights)
}

/// Vesicles / pits in volcanic rock.
pub fn vesicles(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let depth = modifier.value("depth_m");
    let density = modifier.value("density").clamp(0.0, 1.0);
    if depth == 0.0 || density <= 0.0 {
        return Ok(hf.clone());
    }
    let cell = cell_size(hf);
    let spacing = modifier.value("spacing_m").max(cell * 1.5);
    let threshold = 1.0 - density * 0.75;
    let noise = noise_for(seed, "vesicles");
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let (wx, wy) = world_cell(hf, x, y);
            let n = 1.0 - (noise.noise2(wx / spacing, wy / spacing)).abs();
            let pit = smoothstep(threshold, 1.0, n);
            heights[y * hf.width() + x] -= depth * pit * pit;
        }
    }
    rebuild(hf, heights)
}

/// Carve a recessed ledge / alcove under a resistant bed. This is a
/// heightfield approximation of an overhang: the true overhanging lip is a
/// mesh layer (see the roadmap), but the recess already reads as a cornice at
/// medium distance and avoids one-cell needles.
pub fn undercut(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    let depth = modifier.value("depth_m");
    if depth == 0.0 {
        return Ok(hf.clone());
    }
    let band = modifier.value("band_m").max(2.0);
    let height = modifier.value("height_m").clamp(1.0, 50.0);
    let spacing = modifier.value("spacing_m").max(5.0);
    let angle = modifier.value("angle_deg").to_radians();
    let sharpness = modifier.value("sharpness").clamp(0.0, 1.0);
    let noise = noise_for(seed, "undercut");
    let half = (height * 0.5 / band).clamp(0.015, 0.45);
    let exponent = 0.5 + sharpness * 3.0;
    let mut heights = hf.heights().to_vec();
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            if !hf.is_valid(x, y) {
                continue;
            }
            let slope = slope_at(hf, x, y);
            let wall = smoothstep(0.30, 0.85, slope);
            if wall <= 0.0 {
                continue;
            }
            let z = hf.get(x, y);
            let phase = (z / band).rem_euclid(1.0);
            let notch = 1.0 - smoothstep(half * 0.55, half, (phase - 0.5).abs());
            if notch <= 0.0 {
                continue;
            }
            let (wx, wy) = world_cell(hf, x, y);
            let (u, v) = rotate(wx, wy, angle);
            let wander = noise.fbm2(
                u / (spacing * 5.0),
                v / (spacing * 5.0),
                3,
                2.0,
                0.5,
            ) * spacing
                * 0.35;
            let buttress =
                (0.5 + 0.5 * ((u + wander) / spacing * TAU).cos()).powf(exponent);
            let patch = 0.55 + 0.45 * noise.fbm01(wx / (spacing * 3.0), wy / (spacing * 3.0), 3, 2.0, 0.5);
            heights[y * hf.width() + x] -= depth * notch * wall * buttress * patch;
        }
    }
    rebuild(hf, heights)
}

/// Tiny deterministic hash used by the crater field.
fn hash01(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = seed;
    h ^= u64::from(x as u32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= u64::from(y as u32).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::ModifierKind;

    fn ramp() -> HeightField {
        // A slope with a flat plateau at the top: the slope-only operators
        // must leave the plateau untouched.
        let size = 65;
        let mut heights = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                let t = (x as f32 / 40.0).clamp(0.0, 1.0);
                heights[y * size + x] = 400.0 * (1.0 - t);
            }
        }
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn grooves_and_ridges_change_the_surface_deterministically() {
        for (kind, label) in [
            (ModifierKind::Grooves, "grooves"),
            (ModifierKind::Ridges, "ridges"),
        ] {
            let hf = ramp();
            let mut modifier = Modifier::new(kind);
            modifier.set("depth_m", 5.0);
            modifier.set("amplitude_m", 4.0);
            let a = super::super::apply_one(&hf, &modifier, Seed::new(3)).unwrap();
            let b = super::super::apply_one(&hf, &modifier, Seed::new(3)).unwrap();
            assert_eq!(a.heights(), b.heights(), "{label} not deterministic");
            assert_ne!(a.heights(), hf.heights(), "{label} did nothing");
        }
    }

    #[test]
    fn craters_create_a_depression_and_a_rim() {
        let hf = HeightField::filled(129, 129, 5.0, 500.0).unwrap();
        let mut modifier = Modifier::new(ModifierKind::CraterField);
        modifier.set("density", 1.0);
        modifier.set("spacing_m", 300.0);
        modifier.set("radius_m", 40.0);
        modifier.set("depth_m", 10.0);
        modifier.set("rim", 0.5);
        let out = super::super::apply_one(&hf, &modifier, Seed::new(1)).unwrap();
        assert!(out.stats().min < hf.stats().min - 1.0);
        assert!(out.stats().max > hf.stats().max + 0.1);
    }

    #[test]
    fn exfoliate_and_vesicles_are_resolution_aware() {
        for kind in [ModifierKind::Exfoliate, ModifierKind::Vesicles] {
            let hf = HeightField::filled(33, 33, 50.0, 100.0).unwrap();
            let mut modifier = Modifier::new(kind);
            modifier.set("scale_m", 0.1);
            modifier.set("spacing_m", 0.1);
            let out = super::super::apply_one(&hf, &modifier, Seed::new(2)).unwrap();
            assert_eq!(out.width(), 33);
            assert!(out.heights().iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn undercut_lowers_steep_faces_only() {
        let hf = ramp();
        let mut modifier = Modifier::new(ModifierKind::Undercut);
        modifier.set("depth_m", 3.0);
        modifier.set("band_m", 40.0);
        modifier.set("height_m", 10.0);
        let out = super::super::apply_one(&hf, &modifier, Seed::new(1)).unwrap();
        // The flat top (x >= 40) must stay at 0.
        for y in 0..65 {
            for x in 45..65 {
                assert!((out.get(x, y) - hf.get(x, y)).abs() < 1e-4);
            }
        }
        assert!(out.stats().min <= hf.stats().min);
    }
}

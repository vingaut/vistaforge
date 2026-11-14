//! Deterministic terrain modifiers.
//!
//! The base pipeline (`vf_geology::build`) produces the terrain recipe; this
//! crate applies the editable modifier stack on top of it. Every operator is
//! resolution-aware (distances are metres, converted to cells through the
//! heightfield spacing), deterministic (all randomness comes from the project
//! seed) and preserves the mask, origin and nodata value of its input.
//!
//! The operators are grouped as in the reference documentation:
//!
//! * deformation: [`ModifierKind::Displace`], [`ModifierKind::Warp`],
//!   [`ModifierKind::Transform`], [`ModifierKind::Swirl`];
//! * geological sculpting: [`ModifierKind::Terrace`], [`ModifierKind::Fold`],
//!   [`ModifierKind::Stratify`], [`ModifierKind::Grooves`],
//!   [`ModifierKind::Ridges`], [`ModifierKind::CraterField`],
//!   [`ModifierKind::Exfoliate`], [`ModifierKind::Vesicles`],
//!   [`ModifierKind::Undercut`], [`ModifierKind::Aperture`],
//!   [`ModifierKind::Clamp`], [`ModifierKind::Blur`],
//!   [`ModifierKind::Sharpen`], [`ModifierKind::Crater`];
//! * erosion: [`ModifierKind::Erosion`], [`ModifierKind::Thermal`],
//!   [`ModifierKind::Alluvium`], [`ModifierKind::Snow`],
//!   [`ModifierKind::Dunes`];
//! * combination: [`ModifierKind::Combine`], [`ModifierKind::BiasGain`].

mod deform;
mod erode;
mod micro;
mod sculpt;

use vf_core::{HeightField, Modifier, ModifierKind, ModifierStack, Result, Seed};
use vf_procgen::Noise;

/// Apply an entire stack, in order. Disabled entries are skipped, so artists
/// can preview a variant without losing the settings.
pub fn apply(hf: &HeightField, stack: &ModifierStack, seed: Seed) -> Result<HeightField> {
    stack.validate().map_err(vf_core::Error::Invalid)?;
    if stack.active().next().is_none() {
        return Ok(hf.clone());
    }
    let mut current = hf.clone();
    for (index, modifier) in stack.items.iter().enumerate() {
        if !modifier.enabled {
            continue;
        }
        let modifier_seed = seed.derive(&format!("modifier-{index}-{}", modifier.kind.name()));
        current = apply_one(&current, modifier, modifier_seed)?;
    }
    Ok(current)
}

/// Apply one modifier.
pub fn apply_one(hf: &HeightField, modifier: &Modifier, seed: Seed) -> Result<HeightField> {
    modifier.validate().map_err(vf_core::Error::Invalid)?;
    let mut out = match modifier.kind {
        ModifierKind::Displace => deform::displace(hf, modifier, seed)?,
        ModifierKind::Warp => deform::warp(hf, modifier, seed)?,
        ModifierKind::Transform => deform::transform(hf, modifier)?,
        ModifierKind::Swirl => deform::swirl(hf, modifier)?,
        ModifierKind::Terrace => sculpt::terrace(hf, modifier, seed)?,
        ModifierKind::Fold => deform::fold(hf, modifier)?,
        ModifierKind::Stratify => sculpt::stratify(hf, modifier, seed)?,
        ModifierKind::Grooves => micro::grooves(hf, modifier, seed)?,
        ModifierKind::Ridges => micro::ridges(hf, modifier, seed)?,
        ModifierKind::CraterField => micro::crater_field(hf, modifier, seed)?,
        ModifierKind::Exfoliate => micro::exfoliate(hf, modifier, seed)?,
        ModifierKind::Vesicles => micro::vesicles(hf, modifier, seed)?,
        ModifierKind::Undercut => micro::undercut(hf, modifier, seed)?,
        ModifierKind::Aperture => sculpt::aperture(hf, modifier)?,
        ModifierKind::Clamp => sculpt::clamp(hf, modifier)?,
        ModifierKind::Blur => sculpt::blur(hf, modifier)?,
        ModifierKind::Sharpen => sculpt::sharpen(hf, modifier)?,
        ModifierKind::Crater => sculpt::crater(hf, modifier, seed)?,
        ModifierKind::RecastCliff => sculpt::recast_cliff(hf, modifier)?,
        ModifierKind::Erosion => erode::hydraulic(hf, modifier, seed)?,
        ModifierKind::Thermal => erode::thermal(hf, modifier)?,
        ModifierKind::Alluvium => erode::alluvium(hf, modifier, seed)?,
        ModifierKind::Snow => erode::snow(hf, modifier, seed)?,
        ModifierKind::Dunes => erode::dunes(hf, modifier, seed)?,
        ModifierKind::Talus => erode::talus(hf, modifier, seed)?,
        ModifierKind::Combine => sculpt::combine(hf, modifier, seed)?,
        ModifierKind::BiasGain => sculpt::bias_gain(hf, modifier)?,
    };
    // Masked samples must survive every operator, including the resampling
    // and erosion ones that do not know about the mask.
    if let Some(mask) = hf.mask() {
        let heights = out.heights_mut();
        for (i, &valid) in mask.iter().enumerate() {
            if !valid {
                heights[i] = hf.heights()[i];
            }
        }
    }
    Ok(out)
}

/// Default Poisson cliff recast used by the renderer's `cliffs` option.
///
/// The parameters are tuned for a 5–10 m DEM: a 38° talus threshold, a strong
/// amplification and a 200-iteration red-black solve. The renderer can trade
/// time for quality by changing `iterations`.
pub fn recast_cliff(hf: &HeightField, iterations: u32) -> Result<HeightField> {
    let mut modifier = Modifier::new(ModifierKind::RecastCliff);
    modifier.set("talus_deg", 38.0);
    modifier.set("amplitude", 1.5);
    modifier.set("gain", 5.0);
    modifier.set("radius_m", 60.0);
    modifier.set("iterations", iterations as f32);
    sculpt::recast_cliff(hf, &modifier)
}

/// Rebuild a heightfield with new samples, keeping all metadata.
pub(crate) fn rebuild(hf: &HeightField, heights: Vec<f32>) -> Result<HeightField> {
    let (sx, sy) = hf.spacing();
    let mut out = HeightField::new(hf.width(), hf.height(), sx, sy, heights)?;
    let (ox, oy) = hf.origin();
    out.set_origin(ox, oy);
    out.set_nodata(hf.nodata());
    if let Some(mask) = hf.mask() {
        out.set_mask(Some(mask.to_vec()))?;
    }
    Ok(out)
}

/// World-space bounds of the grid centres, in metres.
pub(crate) fn world_extent(hf: &HeightField) -> (f64, f64, f64, f64) {
    let (sx, sy) = hf.spacing();
    let (ox, oy) = hf.origin();
    let ex = (hf.width() - 1) as f64 * sx;
    let ey = (hf.height() - 1) as f64 * sy;
    (ox, oy, ex, ey)
}

/// Bilinear sample at a world position, with edge clamping.
pub(crate) fn sample_world(hf: &HeightField, x: f64, y: f64, repeat: bool) -> f32 {
    let (sx, sy) = hf.spacing();
    let (ox, oy) = hf.origin();
    let (_, _, ex, ey) = world_extent(hf);
    let mut x = x - ox;
    let mut y = y - oy;
    if repeat && ex > 0.0 && ey > 0.0 {
        x = x.rem_euclid(ex);
        y = y.rem_euclid(ey);
    }
    let gx = (x / sx).clamp(0.0, (hf.width() - 1) as f64);
    let gy = (y / sy).clamp(0.0, (hf.height() - 1) as f64);
    hf.sample_bilinear(gx, gy)
}

/// Hermite step, same convention as the renderer.
#[inline]
pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rotate a pair of world offsets.
#[inline]
pub(crate) fn rotate(x: f32, y: f32, radians: f32) -> (f32, f32) {
    let (s, c) = radians.sin_cos();
    (c * x - s * y, s * x + c * y)
}

/// Fractional position inside the ladder, pushed toward 0 or 1 by `sharpness`.
#[inline]
pub(crate) fn sharpen_fraction(frac: f32, sharpness: f32) -> f32 {
    let band = 0.5 - 0.48 * sharpness.clamp(0.0, 1.0);
    let width = 1.0 - 2.0 * band;
    if width < 1e-6 {
        return if frac >= 0.5 { 1.0 } else { 0.0 };
    }
    let t = ((frac - band) / width).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth union-ish clamp used by `Clamp` when a feather is requested.
pub(crate) fn soft_clamp(value: f32, min: f32, max: f32, feather: f32) -> f32 {
    if feather <= 0.0 {
        return value.clamp(min, max);
    }
    if value < min {
        // Softplus on the underside: approaches `min` asymptotically.
        min - feather * ((-(value - min) / feather).exp()).ln_1p()
    } else if value > max {
        max + feather * ((-(value - max) / feather).exp()).ln_1p()
    } else {
        value
    }
}

/// Noise handle seeded for a specific operator.
pub(crate) fn noise_for(seed: Seed, label: &str) -> Noise {
    Noise::new(seed.derive(label))
}

/// Slope magnitude (rise/run) at a grid cell.
#[inline]
pub(crate) fn slope_at(hf: &HeightField, x: usize, y: usize) -> f32 {
    let (gx, gy) = hf.gradient_at(x, y);
    (gx * gx + gy * gy).sqrt()
}

/// Normalise a field to `[0, 1]`, returning the min and relief used.
pub(crate) fn normalised(hf: &HeightField) -> (Vec<f32>, f32, f32) {
    let stats = hf.stats();
    let relief = (stats.max - stats.min).max(1e-6);
    let values = hf
        .heights()
        .iter()
        .map(|v| ((v - stats.min) / relief).clamp(0.0, 1.0))
        .collect();
    (values, stats.min, relief)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::ModifierKind;

    pub(crate) fn test_hf(size: usize) -> HeightField {
        let mut heights = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                let u = x as f32 / (size - 1) as f32;
                let v = y as f32 / (size - 1) as f32;
                heights[y * size + x] =
                    200.0 + 400.0 * (u * 3.0).sin() * (v * 2.0).cos() + 100.0 * u;
            }
        }
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn every_modifier_runs_and_keeps_metadata() {
        let hf = test_hf(65);
        for kind in ModifierKind::ALL {
            let modifier = Modifier::new(kind);
            let out = apply_one(&hf, &modifier, Seed::new(7)).unwrap();
            assert_eq!((out.width(), out.height()), (65, 65), "{}", kind.name());
            assert_eq!(out.spacing(), hf.spacing(), "{}", kind.name());
            assert_eq!(out.origin(), hf.origin(), "{}", kind.name());
            assert!(
                out.heights().iter().all(|v| v.is_finite()),
                "{}",
                kind.name()
            );
        }
    }

    #[test]
    fn stack_is_deterministic_and_order_sensitive() {
        let hf = test_hf(65);
        let mut stack = ModifierStack::default();
        stack.push(ModifierKind::Displace);
        stack.push(ModifierKind::Terrace);
        let a = apply(&hf, &stack, Seed::new(3)).unwrap();
        let b = apply(&hf, &stack, Seed::new(3)).unwrap();
        assert_eq!(a.heights(), b.heights());
        stack.items[0].enabled = false;
        let c = apply(&hf, &stack, Seed::new(3)).unwrap();
        assert_ne!(a.heights(), c.heights());
    }

    #[test]
    fn mask_is_preserved_and_masked_cells_untouched() {
        let hf = test_hf(65);
        let mut hf = hf;
        let mut mask = vec![true; 65 * 65];
        for y in 20..30 {
            for x in 20..30 {
                mask[y * 65 + x] = false;
            }
        }
        hf.set_mask(Some(mask)).unwrap();
        for kind in ModifierKind::ALL {
            let modifier = Modifier::new(kind);
            let out = apply_one(&hf, &modifier, Seed::new(9)).unwrap();
            let mask_out = out.mask().expect("mask must be preserved");
            assert!(
                !mask_out[25 * 65 + 25],
                "{} touched a masked cell",
                kind.name()
            );
        }
    }
}

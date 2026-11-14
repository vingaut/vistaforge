//! Local cliff shaping.
//!
//! A heightfield cannot hold a true overhang, but it can hold a near-vertical
//! wall if the whole drop happens between two neighbouring samples. The raw
//! terrain does not do that: erosion spreads each 250 m wall over thirty
//! cells, which reads as a smooth 40° slope.
//!
//! This pass snaps altitudes to a stratigraphic ladder on steep ground, so the
//! surface becomes alternating treads and one-cell risers. The ladder is
//! applied with a **smoothed weight field** (slope × local relief): a per-cell
//! weight changes abruptly at a rim and leaves one-cell spikes behind, which is
//! exactly the picket fence the first versions produced. Smoothing the weight
//! over a few cells makes the transition continuous.
//!
//! It is a heightfield pass, not an overlay: walls cast and receive shadows,
//! are lit by the normal rock material and take part in AO and water sampling.
//! For a smoother, solver-based wall see the `recast_cliff` modifier, which
//! implements the Hesiod/HighMap Poisson recast but is slower.

use vf_core::{HeightField, Result, Seed};

use crate::Noise;

/// The altitude ladder is 18 cells high, clamped to a geologically readable
/// range. At 5 m/cell this gives ~90 m beds: three risers for a 300 m canyon.
const LADDER_CELLS: f32 = 18.0;
const LADDER_MIN_M: f32 = 80.0;
const LADDER_MAX_M: f32 = 250.0;

/// Return a copy of `hf` with steep faces shaped into cliffs.
///
/// `warp` (0..1) undulates the stratigraphic ladder with low-frequency noise
/// so the risers do not form perfectly horizontal planes across the whole
/// massif. `warp_scale_m` is the wavelength of that undulation. The ladder is
/// evaluated in warped altitude and the warp is removed afterwards, so the
/// terrain itself is not shifted, only the bed boundaries.
pub fn sharpen(
    hf: &HeightField,
    seed: Seed,
    warp: f32,
    warp_scale_m: f32,
) -> Result<HeightField> {
    let (sx, sy) = hf.spacing();
    let cell = sx.max(sy) as f32;
    let mut out = hf.clone();
    if cell <= 0.0 || hf.width() < 3 || hf.height() < 3 {
        return Ok(out);
    }
    let stats = hf.stats();
    let step = (cell * LADDER_CELLS).clamp(LADDER_MIN_M, LADDER_MAX_M);
    if !step.is_finite() || step <= 0.0 {
        return Ok(out);
    }
    let width = hf.width();
    let height = hf.height();
    let count = width * height;
    let noise = Noise::new(seed.derive("cliff-ladder"));
    // The warp is applied *after* choosing the rung, as a smooth altitude
    // offset. Moving the rung boundary instead changes which rung a cell
    // snaps to; on a diagonal contour that leaves isolated one-cell needles.
    let warp_amount = warp.clamp(0.0, 1.0) * step * 0.12;
    let warp_scale = warp_scale_m.clamp(80.0, 4000.0);
    let warp_at = |x: f32, y: f32| -> f32 {
        if warp_amount <= 0.0 {
            return 0.0;
        }
        let broad = noise.fbm2(x / warp_scale, y / warp_scale, 4, 2.0, 0.5);
        let fine = noise.fbm2(
            x / (warp_scale * 0.23) + 13.1,
            y / (warp_scale * 0.23) - 7.7,
            3,
            2.0,
            0.5,
        );
        ((broad * 0.85 + fine * 0.18) * warp_amount).clamp(-0.16 * step, 0.16 * step)
    };

    // Local relief mask and weight field.
    let local_mean = vf_terrain::smooth(hf, 24, 1);
    let mut weight = vec![0.0f32; count];
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) {
                continue;
            }
            let (gx, gy) = hf.gradient_at(x, y);
            let slope = (gx * gx + gy * gy).sqrt();
            let feature = (hf.get(x, y) - local_mean.get(x, y)).abs();
            weight[y * width + x] =
                smoothstep(1.05, 1.50, slope) * smoothstep(12.0, 40.0, feature);
        }
    }
    // Smooth the weight so neighbouring cells do not make opposite decisions.
    let weight = HeightField::new(width, height, sx, sy, weight)
        .map(|field| vf_terrain::smooth(&field, 2, 1))
        .expect("weight field has the heightfield dimensions");

    // Local envelope: a snap can push one cell above the rim or below the
    // floor; clamping to the original 7×7 range removes isolated needles.
    let (local_min, local_max) = local_envelope(hf, 3);

    let heights = out.heights_mut();
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) {
                continue;
            }
            let i = y * width + x;
            let w = weight.heights()[i].clamp(0.0, 1.0);
            if w <= 0.01 {
                continue;
            }
            let z = hf.get(x, y);
            let offset = warp_at(x as f32 * cell, y as f32 * cell);
            let t = (z - stats.min) / step;
            let base = t.floor();
            let frac = t - base;
            let snapped = smoothstep(0.42, 0.58, frac);
            let ladder = stats.min + (base + snapped) * step + offset;
            // Only ever lower a cell. Raising cells on a rim creates isolated
            // one-cell pickets when the weight varies; lowering them turns the
            // face into a cliff without adding anything above the skyline.
            let shaped = z - (z - ladder).max(0.0) * w;
            heights[i] = shaped.clamp(local_min[i], local_max[i]);
        }
    }

    Ok(out)
}

/// Separable min/max over a square window, in metres. Masked samples do not
/// contribute. Returns `(min, max)` per sample.
fn local_envelope(hf: &HeightField, radius: usize) -> (Vec<f32>, Vec<f32>) {
    let width = hf.width();
    let height = hf.height();
    let count = width * height;
    let mut h_min = vec![f32::INFINITY; count];
    let mut h_max = vec![f32::NEG_INFINITY; count];
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) {
                continue;
            }
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for k in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                if hf.is_valid(k, y) {
                    lo = lo.min(hf.get(k, y));
                    hi = hi.max(hf.get(k, y));
                }
            }
            h_min[y * width + x] = lo;
            h_max[y * width + x] = hi;
        }
    }
    let mut v_min = vec![f32::INFINITY; count];
    let mut v_max = vec![f32::NEG_INFINITY; count];
    for y in 0..height {
        for x in 0..width {
            if !hf.is_valid(x, y) {
                continue;
            }
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for k in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                let src = k * width + x;
                lo = lo.min(h_min[src]);
                hi = hi.max(h_max[src]);
            }
            v_min[y * width + x] = lo;
            v_max[y * width + x] = hi;
        }
    }
    (v_min, v_max)
}

#[inline]
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

    fn rounded_escarpment() -> HeightField {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 0..65 {
                let t = ((x as f32 - 20.0) / 20.0).clamp(0.0, 1.0);
                heights[y * 65 + x] = 400.0 * (1.0 - t * t * (3.0 - 2.0 * t));
            }
        }
        HeightField::new(65, 65, 10.0, 10.0, heights).unwrap()
    }

    fn max_step(hf: &HeightField) -> f32 {
        let mut max = 0.0f32;
        for x in 1..hf.width() - 2 {
            max = max.max((hf.get(x + 1, 20) - hf.get(x, 20)).abs());
        }
        max
    }

    #[test]
    fn escarpment_becomes_steeper() {
        let hf = rounded_escarpment();
        let shaped = sharpen(&hf, Seed::new(1), 0.0, 450.0).unwrap();
        assert!(
            max_step(&shaped) > max_step(&hf) * 1.05,
            "{} -> {}",
            max_step(&hf),
            max_step(&shaped)
        );
        assert!((shaped.stats().max - hf.stats().max).abs() < 100.0);
    }

    #[test]
    fn flat_ground_is_untouched() {
        let hf = HeightField::filled(33, 33, 10.0, 100.0).unwrap();
        let shaped = sharpen(&hf, Seed::new(1), 0.0, 450.0).unwrap();
        assert_eq!(shaped.heights(), hf.heights());
    }

    #[test]
    fn shaping_is_deterministic() {
        let hf = rounded_escarpment();
        let a = sharpen(&hf, Seed::new(1), 0.0, 450.0).unwrap();
        let b = sharpen(&hf, Seed::new(1), 0.0, 450.0).unwrap();
        assert_eq!(a.heights(), b.heights());
    }
}

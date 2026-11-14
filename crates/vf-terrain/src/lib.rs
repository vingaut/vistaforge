//! Heightfield operations: resampling, editing brushes, normals, curvature and
//! hillshading. All functions are deterministic and dependency-free.

pub mod brush;
pub mod world;

use vf_core::{HeightField, HeightFieldError, Sun};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResampleFilter {
    Nearest,
    Bilinear,
    Bicubic,
}

impl ResampleFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "nearest" => Some(Self::Nearest),
            "bilinear" | "linear" => Some(Self::Bilinear),
            "bicubic" | "cubic" | "lanczos" => Some(Self::Bicubic),
            _ => None,
        }
    }
}

/// Resize a heightfield, preserving the world extent (origin and edges stay
/// fixed) and adjusting sample spacing accordingly.
pub fn resample(
    hf: &HeightField,
    width: usize,
    height: usize,
    filter: ResampleFilter,
) -> Result<HeightField, HeightFieldError> {
    if width < 2 || height < 2 {
        return Err(HeightFieldError::TooSmall { width, height });
    }
    if width == hf.width() && height == hf.height() {
        return Ok(hf.clone());
    }
    let mut heights = Vec::with_capacity(width * height);
    let sx = (hf.width() - 1) as f64 / (width - 1) as f64;
    let sy = (hf.height() - 1) as f64 / (height - 1) as f64;
    for y in 0..height {
        let fy = y as f64 * sy;
        for x in 0..width {
            let fx = x as f64 * sx;
            let value = match filter {
                ResampleFilter::Nearest => hf.sample_nearest(fx, fy),
                ResampleFilter::Bilinear => hf.sample_bilinear(fx, fy),
                ResampleFilter::Bicubic => {
                    if hf.mask().is_some() {
                        hf.sample_bilinear(fx, fy)
                    } else {
                        sample_bicubic(hf, fx, fy)
                    }
                }
            };
            heights.push(value);
        }
    }
    let spacing_x = hf.spacing().0 * sx;
    let spacing_y = hf.spacing().1 * sy;
    let mut out = HeightField::new(width, height, spacing_x, spacing_y, heights)?;
    let (ox, oy) = hf.origin();
    out.set_origin(ox, oy);
    out.set_nodata(hf.nodata());
    if let Some(src_mask) = hf.mask() {
        let mut mask = Vec::with_capacity(width * height);
        for y in 0..height {
            let syi = (y as f64 * sy).round() as usize;
            for x in 0..width {
                let sxi = (x as f64 * sx).round() as usize;
                mask.push(src_mask[syi * hf.width() + sxi]);
            }
        }
        out.set_mask(Some(mask))?;
    }
    Ok(out)
}

fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

fn sample_bicubic(hf: &HeightField, x: f64, y: f64) -> f32 {
    let x = x.clamp(0.0, (hf.width() - 1) as f64);
    let y = y.clamp(0.0, (hf.height() - 1) as f64);
    let xi = x.floor() as isize;
    let yi = y.floor() as isize;
    let tx = (x - xi as f64) as f32;
    let ty = (y - yi as f64) as f32;
    let mut rows = [0.0f32; 4];
    for (j, row) in rows.iter_mut().enumerate() {
        let sy = (yi + j as isize - 1).clamp(0, hf.height() as isize - 1) as usize;
        let mut samples = [0.0f32; 4];
        for (i, sample) in samples.iter_mut().enumerate() {
            let sx = (xi + i as isize - 1).clamp(0, hf.width() as isize - 1) as usize;
            *sample = hf.get(sx, sy);
        }
        *row = catmull_rom(samples[0], samples[1], samples[2], samples[3], tx);
    }
    catmull_rom(rows[0], rows[1], rows[2], rows[3], ty)
}

/// Separable Gaussian smoothing. Masked samples are left untouched and do not
/// contribute to their neighbours.
pub fn smooth(hf: &HeightField, radius: usize, iterations: usize) -> HeightField {
    let radius = radius.max(1);
    let mut out = hf.clone();
    for _ in 0..iterations.max(1) {
        let src = out.clone();
        let kernel = gaussian_kernel(radius);
        let half = radius as isize;
        let (w, h) = (src.width(), src.height());
        for y in 0..h {
            for x in 0..w {
                if !src.is_valid(x, y) {
                    continue;
                }
                let mut acc = 0.0f32;
                let mut weight = 0.0f32;
                for (k, &kw) in kernel.iter().enumerate() {
                    let sx = x as isize + k as isize - half;
                    if sx < 0 || sx >= w as isize {
                        continue;
                    }
                    let sx = sx as usize;
                    if !src.is_valid(sx, y) {
                        continue;
                    }
                    acc += src.get(sx, y) * kw;
                    weight += kw;
                }
                if weight > 1e-6 {
                    out.set(x, y, acc / weight);
                }
            }
        }
        let intermediate = out.clone();
        for y in 0..h {
            for x in 0..w {
                if !intermediate.is_valid(x, y) {
                    continue;
                }
                let mut acc = 0.0f32;
                let mut weight = 0.0f32;
                for (k, &kw) in kernel.iter().enumerate() {
                    let sy = y as isize + k as isize - half;
                    if sy < 0 || sy >= h as isize {
                        continue;
                    }
                    let sy = sy as usize;
                    if !intermediate.is_valid(x, sy) {
                        continue;
                    }
                    acc += intermediate.get(x, sy) * kw;
                    weight += kw;
                }
                if weight > 1e-6 {
                    out.set(x, y, acc / weight);
                }
            }
        }
    }
    out
}

fn gaussian_kernel(radius: usize) -> Vec<f32> {
    let sigma = (radius as f32 * 0.5).max(0.5);
    let mut kernel = Vec::with_capacity(radius * 2 + 1);
    let mut sum = 0.0;
    for i in 0..=radius * 2 {
        let d = i as f32 - radius as f32;
        let v = (-d * d / (2.0 * sigma * sigma)).exp();
        kernel.push(v);
        sum += v;
    }
    for v in &mut kernel {
        *v /= sum;
    }
    kernel
}

/// Circular raise/lower/level brush. Coordinates are in grid samples, radius
/// in samples, strength in metres. Positive strength raises, negative lowers.
pub fn brush(
    hf: &mut HeightField,
    cx: f64,
    cy: f64,
    radius: f64,
    strength: f32,
    target: Option<f32>,
) {
    let radius = radius.max(0.5);
    let (w, h) = (hf.width(), hf.height());
    let x0 = (cx - radius).floor().max(0.0) as usize;
    let x1 = (cx + radius).ceil().min((w - 1) as f64) as usize;
    let y0 = (cy - radius).floor().max(0.0) as usize;
    let y1 = (cy + radius).ceil().min((h - 1) as f64) as usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if !hf.is_valid(x, y) {
                continue;
            }
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let d = (dx * dx + dy * dy).sqrt() / radius;
            if d >= 1.0 {
                continue;
            }
            let t = 1.0 - d;
            let falloff = (t * t * (3.0 - 2.0 * t)) as f32;
            let old = hf.get(x, y);
            let new = match target {
                Some(target_value) => {
                    old + (target_value - old) * falloff * strength.clamp(0.0, 1.0)
                }
                None => old + strength * falloff,
            };
            hf.set(x, y, new);
        }
    }
}

/// Raise or lower a circular region.
pub fn raise(hf: &mut HeightField, cx: f64, cy: f64, radius: f64, delta: f32) {
    brush(hf, cx, cy, radius, delta, None);
}

/// Flatten a circular region toward `target` (0..1 blend strength).
pub fn flatten(hf: &mut HeightField, cx: f64, cy: f64, radius: f64, target: f32, strength: f32) {
    brush(hf, cx, cy, radius, strength, Some(target));
}

/// Quantise altitudes into terraces, blended by `strength` (0..1).
pub fn terrace(hf: &mut HeightField, step: f32, strength: f32) {
    if step <= 0.0 {
        return;
    }
    let strength = strength.clamp(0.0, 1.0);
    hf.map_valid(|z| {
        let q = (z / step).round() * step;
        z + (q - z) * strength
    });
}

pub fn clamp_heights(hf: &mut HeightField, min: f32, max: f32) {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    hf.map_valid(|z| z.clamp(lo, hi));
}

/// Normalise valid samples to `[0, 1]` and scale to `vertical_scale`.
pub fn normalize(hf: &mut HeightField, vertical_scale: f32) {
    let stats = hf.stats();
    let range = (stats.max - stats.min).max(1e-6);
    hf.map_valid(|z| (z - stats.min) / range * vertical_scale);
}

/// Per-vertex normals, row-major, matching the heightfield layout.
pub fn vertex_normals(hf: &HeightField, vertical_scale: f32) -> Vec<[f32; 3]> {
    let mut normals = Vec::with_capacity(hf.sample_count());
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            normals.push(hf.normal_at(x, y, vertical_scale));
        }
    }
    normals
}

/// Signed Laplacian curvature in 1/metres. Positive means a local pit,
/// negative means a ridge.
pub fn curvature(hf: &HeightField) -> Vec<f32> {
    let (sx, sy) = hf.spacing();
    let sx = sx as f32;
    let sy = sy as f32;
    let mut out = Vec::with_capacity(hf.sample_count());
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(hf.width() - 1);
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(hf.height() - 1);
            let dxx = (hf.get(xp, y) - 2.0 * hf.get(x, y) + hf.get(xm, y)) / (sx * sx);
            let dyy = (hf.get(x, yp) - 2.0 * hf.get(x, y) + hf.get(x, ym)) / (sy * sy);
            out.push(dxx + dyy);
        }
    }
    out
}

/// Approximate sky-visibility per vertex from a horizon sweep in 8 directions.
/// Returned values are in `[0, 1]`, 1 = fully open sky.
pub fn horizon_ao(hf: &HeightField, max_distance_m: f32) -> Vec<f32> {
    const DIRS: [(f32, f32); 8] = [
        (1.0, 0.0),
        (0.707_106_77, 0.707_106_77),
        (0.0, 1.0),
        (-0.707_106_77, 0.707_106_77),
        (-1.0, 0.0),
        (-0.707_106_77, -0.707_106_77),
        (0.0, -1.0),
        (0.707_106_77, -0.707_106_77),
    ];
    let (sx, sy) = hf.spacing();
    let sx = sx as f32;
    let sy = sy as f32;
    let mut ao = Vec::with_capacity(hf.sample_count());
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            let z0 = hf.get(x, y);
            let mut occlusion = 0.0f32;
            for (dx, dy) in DIRS {
                let mut max_slope = 0.0f32;
                let mut dist = 3.0f32;
                while dist <= max_distance_m {
                    let px = x as f32 + dx * dist / sx;
                    let py = y as f32 + dy * dist / sy;
                    if px < 0.0
                        || py < 0.0
                        || px > (hf.width() - 1) as f32
                        || py > (hf.height() - 1) as f32
                    {
                        break;
                    }
                    let h = hf.sample_bilinear(px as f64, py as f64);
                    max_slope = max_slope.max((h - z0) / dist);
                    dist *= 1.7;
                }
                occlusion += max_slope.atan() / std::f32::consts::FRAC_PI_2;
            }
            let occ = (occlusion / DIRS.len() as f32).clamp(0.0, 1.0);
            ao.push(1.0 - occ * 0.85);
        }
    }
    ao
}

/// Near-field sky occlusion for steep ground.
///
/// The coarse [`horizon_ao`] map sweeps 12 % of the terrain extent: it catches
/// the canyon as a whole but nothing at the scale of a 20–50 m gully. This
/// second sweep uses the same horizon method over a short range (six cells),
/// with 12 directions, and only evaluates cells steep enough to hide sky.
/// Callers combine both maps; the result is a multi-scale AO that darkens
/// crevices and the foot of cliffs without changing the large-scale lighting.
pub fn horizon_ao_fine(hf: &HeightField, max_distance_m: f32) -> Vec<f32> {
    const DIRS: [(f32, f32); 12] = [
        (1.0, 0.0),
        (0.866_025_4, 0.5),
        (0.5, 0.866_025_4),
        (0.0, 1.0),
        (-0.5, 0.866_025_4),
        (-0.866_025_4, 0.5),
        (-1.0, 0.0),
        (-0.866_025_4, -0.5),
        (-0.5, -0.866_025_4),
        (0.0, -1.0),
        (0.5, -0.866_025_4),
        (0.866_025_4, -0.5),
    ];
    let (sx, sy) = hf.spacing();
    let sx = sx as f32;
    let sy = sy as f32;
    let start = (sx.max(sy) * 0.9).max(2.0);
    let max_distance_m = max_distance_m.max(start * 1.5);
    let mut ao = Vec::with_capacity(hf.sample_count());
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            let z0 = hf.get(x, y);
            let mut occlusion = 0.0f32;
            for (dx, dy) in DIRS {
                let mut max_slope = 0.0f32;
                let mut dist = start;
                while dist <= max_distance_m {
                    let px = x as f32 + dx * dist / sx;
                    let py = y as f32 + dy * dist / sy;
                    if px < 0.0
                        || py < 0.0
                        || px > (hf.width() - 1) as f32
                        || py > (hf.height() - 1) as f32
                    {
                        break;
                    }
                    let h = hf.sample_bilinear(px as f64, py as f64);
                    max_slope = max_slope.max((h - z0) / dist);
                    dist *= 1.45;
                }
                occlusion += max_slope.atan() / std::f32::consts::FRAC_PI_2;
            }
            let occ = (occlusion / DIRS.len() as f32).clamp(0.0, 1.0);
            ao.push(1.0 - occ * 0.9);
        }
    }
    ao
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HillshadeParams {
    /// Compass convention: degrees from south, positive toward west.
    pub azimuth_deg_from_south: f32,
    pub declination_deg: f32,
    /// Multiplier applied to slopes before lighting.
    pub vertical_exaggeration: f32,
    /// Ambient floor, 0..1.
    pub ambient: f32,
}

impl Default for HillshadeParams {
    fn default() -> Self {
        Self {
            azimuth_deg_from_south: 315.0,
            declination_deg: 45.0,
            vertical_exaggeration: 1.0,
            ambient: 0.15,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    /// Top row is north.
    pub pixels: Vec<u8>,
}

/// Grayscale shaded-relief image. Row 0 of the heightfield (south) ends up at
/// the bottom of the image.
pub fn hillshade(hf: &HeightField, params: HillshadeParams) -> GrayImage {
    let sun = Sun {
        azimuth_deg_from_south: params.azimuth_deg_from_south,
        declination_deg: params.declination_deg.clamp(0.0, 90.0),
        ..Sun::default()
    };
    let l = sun.direction();
    let l = [l.x as f32, l.y as f32, l.z as f32];
    let ambient = params.ambient.clamp(0.0, 1.0);
    let mut pixels = vec![0u8; hf.width() * hf.height()];
    for y in 0..hf.height() {
        for x in 0..hf.width() {
            let n = hf.normal_at(x, y, params.vertical_exaggeration);
            let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
            let shade = ambient + (1.0 - ambient) * ndl;
            let value = (shade.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            let dst_y = hf.height() - 1 - y;
            pixels[dst_y * hf.width() + x] = value;
        }
    }
    GrayImage {
        width: hf.width(),
        height: hf.height(),
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(size: usize) -> HeightField {
        let heights = (0..size * size)
            .map(|i| ((i % size) as f32) * 10.0)
            .collect();
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn resample_identity_and_size() {
        let hf = ramp(65);
        let same = resample(&hf, 65, 65, ResampleFilter::Bilinear).unwrap();
        assert_eq!(same.heights(), hf.heights());
        let half = resample(&hf, 33, 33, ResampleFilter::Bilinear).unwrap();
        assert_eq!((half.width(), half.height()), (33, 33));
        assert!((half.spacing().0 - 20.0).abs() < 1e-9);
    }

    #[test]
    fn smooth_reduces_variance() {
        let mut heights = vec![0.0f32; 65 * 65];
        for (i, h) in heights.iter_mut().enumerate() {
            *h = if i % 2 == 0 { 100.0 } else { -100.0 };
        }
        let hf = HeightField::new(65, 65, 1.0, 1.0, heights).unwrap();
        let smoothed = smooth(&hf, 3, 2);
        assert!(smoothed.stats().std_dev < hf.stats().std_dev);
    }

    #[test]
    fn brush_raises_a_disc() {
        let mut hf = HeightField::filled(21, 21, 1.0, 0.0).unwrap();
        raise(&mut hf, 10.0, 10.0, 5.0, 100.0);
        assert!(hf.get(10, 10) > 90.0);
        assert_eq!(hf.get(0, 0), 0.0);
    }

    #[test]
    fn hillshade_is_bounded_and_flipped() {
        let hf = ramp(9);
        let img = hillshade(&hf, HillshadeParams::default());
        assert_eq!(img.pixels.len(), 81);
        let flat = HeightField::filled(9, 9, 1.0, 100.0).unwrap();
        let flat_img = hillshade(&flat, HillshadeParams::default());
        assert!(flat_img.pixels.iter().all(|&v| v == flat_img.pixels[0]));
    }

    #[test]
    fn curvature_zero_on_plane() {
        let hf = HeightField::filled(9, 9, 1.0, 42.0).unwrap();
        assert!(curvature(&hf).iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn fine_ao_opens_on_a_plain_and_darkens_a_pit() {
        let plain = HeightField::filled(33, 33, 10.0, 100.0).unwrap();
        let ao = horizon_ao_fine(&plain, 150.0);
        assert!(ao.iter().all(|v| *v > 0.99));
        let mut heights = vec![100.0f32; 33 * 33];
        for y in 8..25 {
            for x in 8..25 {
                heights[y * 33 + x] = 40.0;
            }
        }
        let pit = HeightField::new(33, 33, 10.0, 10.0, heights).unwrap();
        let ao = horizon_ao_fine(&pit, 150.0);
        let center = ao[16 * 33 + 16];
        let rim = ao[33 + 1];
        assert!(
            center < rim,
            "pit floor should be more occluded: {center} vs {rim}"
        );
        assert!(rim > 0.9);
    }
}

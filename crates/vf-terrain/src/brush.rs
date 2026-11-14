//! Configurable heightfield brushes for the manual sculpting workspace.
//!
//! The brush engine is deliberately independent of `egui`: the same stamp can
//! be driven by a mouse, a tablet, a CLI test or an MCP tool. Distances are in
//! metres, not pixels, so a brush keeps its physical size when the terrain is
//! resampled. Anti-aliasing is done by supersampling the noise modulation and
//! by a footprint-scaled edge, never by blurring the whole stamp.

use vf_core::{HeightField, Seed};

/// Footprint of one stamp in the heightfield.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushShape {
    Circle,
    Square,
    Diamond,
    Ring,
    Star,
    Polygon,
}

/// Radial falloff from the centre to the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Falloff {
    Constant,
    Linear,
    Smooth,
    Gaussian,
    Plateau,
}

/// What the stamp does to the height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushMode {
    Raise,
    Lower,
    Smooth,
    Flatten,
    Terrace,
    Ridge,
    Groove,
    Crater,
    Noise,
    Warp,
    Clone,
}

/// Mirroring around the terrain centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symmetry {
    None,
    X,
    Y,
    Xy,
    Radial(u32),
}

impl BrushMode {
    pub const ALL: [Self; 11] = [
        Self::Raise,
        Self::Lower,
        Self::Smooth,
        Self::Flatten,
        Self::Terrace,
        Self::Ridge,
        Self::Groove,
        Self::Crater,
        Self::Noise,
        Self::Warp,
        Self::Clone,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Raise => "raise",
            Self::Lower => "lower",
            Self::Smooth => "smooth",
            Self::Flatten => "flatten",
            Self::Terrace => "terrace",
            Self::Ridge => "ridge",
            Self::Groove => "groove",
            Self::Crater => "crater",
            Self::Noise => "noise",
            Self::Warp => "warp",
            Self::Clone => "clone",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let key = value.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL.into_iter().find(|mode| mode.name() == key)
    }
}

impl BrushShape {
    pub fn name(self) -> &'static str {
        match self {
            Self::Circle => "circle",
            Self::Square => "square",
            Self::Diamond => "diamond",
            Self::Ring => "ring",
            Self::Star => "star",
            Self::Polygon => "polygon",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let key = value.trim().to_ascii_lowercase();
        [
            Self::Circle,
            Self::Square,
            Self::Diamond,
            Self::Ring,
            Self::Star,
            Self::Polygon,
        ]
        .into_iter()
        .find(|shape| shape.name() == key)
    }
}

impl Falloff {
    pub fn name(self) -> &'static str {
        match self {
            Self::Constant => "constant",
            Self::Linear => "linear",
            Self::Smooth => "smooth",
            Self::Gaussian => "gaussian",
            Self::Plateau => "plateau",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let key = value.trim().to_ascii_lowercase();
        [
            Self::Constant,
            Self::Linear,
            Self::Smooth,
            Self::Gaussian,
            Self::Plateau,
        ]
        .into_iter()
        .find(|falloff| falloff.name() == key)
    }
}

/// One brush configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    pub shape: BrushShape,
    pub falloff: Falloff,
    pub mode: BrushMode,
    /// Radius in metres.
    pub radius_m: f32,
    /// Ellipse aspect (1 = round).
    pub aspect: f32,
    /// Rotation of the ellipse / polygon, degrees.
    pub angle_deg: f32,
    /// Stamp strength. Metres for Raise/Lower, blend 0..1 for the others.
    pub strength: f32,
    /// 0 = soft edge, 1 = hard edge.
    pub hardness: f32,
    /// Multi-scale perturbation of the stamp, 0..1.
    pub roughness: f32,
    /// Wavelength of the roughness, metres.
    pub noise_scale_m: f32,
    /// Supersampling of the roughness, 1..8.
    pub aa_samples: u32,
    pub symmetry: Symmetry,
    /// Terrace step, metres.
    pub step_m: f32,
    /// Clone source offset, metres.
    pub clone_offset_m: [f32; 2],
    /// Number of sides for the polygon shape.
    pub sides: u32,
    /// Invert raise/lower or ridge/groove.
    pub invert: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            shape: BrushShape::Circle,
            falloff: Falloff::Smooth,
            mode: BrushMode::Raise,
            radius_m: 80.0,
            aspect: 1.0,
            angle_deg: 0.0,
            strength: 8.0,
            hardness: 0.35,
            roughness: 0.25,
            noise_scale_m: 12.0,
            aa_samples: 4,
            symmetry: Symmetry::None,
            step_m: 25.0,
            clone_offset_m: [0.0, 0.0],
            sides: 6,
            invert: false,
        }
    }
}

impl Brush {
    pub fn validate(&self) -> Result<(), String> {
        if !self.radius_m.is_finite() || !(0.05..=20_000.0).contains(&self.radius_m) {
            return Err("brush radius must be in 0.05..=20000 metres".into());
        }
        if !self.aspect.is_finite() || !(0.05..=20.0).contains(&self.aspect) {
            return Err("brush aspect must be in 0.05..=20".into());
        }
        if !self.strength.is_finite() || self.strength.abs() > 10_000.0 {
            return Err("brush strength is out of range".into());
        }
        if !(1..=8).contains(&self.aa_samples) {
            return Err("brush aa_samples must be in 1..=8".into());
        }
        if !self.noise_scale_m.is_finite() || self.noise_scale_m <= 0.0 {
            return Err("brush noise scale must be positive".into());
        }
        Ok(())
    }
}

/// A rectangular grid region touched by a stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrushRegion {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl BrushRegion {
    pub fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub fn width(self) -> usize {
        self.x1 - self.x0
    }

    pub fn height(self) -> usize {
        self.y1 - self.y0
    }
}

/// Reversible edit for one stroke.
#[derive(Debug, Clone)]
pub struct BrushUndo {
    pub label: String,
    pub region: BrushRegion,
    pub before: Vec<f32>,
    pub after: Vec<f32>,
}

impl BrushUndo {
    pub fn undo(&self, hf: &mut HeightField) {
        write_region(hf, self.region, &self.before);
    }

    pub fn redo(&self, hf: &mut HeightField) {
        write_region(hf, self.region, &self.after);
    }

    pub fn bytes(&self) -> usize {
        (self.before.len() + self.after.len()) * 4
    }
}

/// A stroke in progress. It keeps one snapshot of the pre-stroke heights; the
/// undo entry is reduced to the touched rectangle when the stroke ends.
#[derive(Debug)]
pub struct Stroke {
    snapshot: Vec<f32>,
    region: Option<BrushRegion>,
    last: Option<(f64, f64)>,
    seed: Seed,
    stamp: u64,
    label: String,
}

impl Stroke {
    pub fn begin(hf: &HeightField, seed: Seed, label: impl Into<String>) -> Self {
        Self {
            snapshot: hf.heights().to_vec(),
            region: None,
            last: None,
            seed,
            stamp: 0,
            label: label.into(),
        }
    }

    /// Apply stamps from the previous point to `(x, y)` (world metres).
    pub fn stamp_to(&mut self, hf: &mut HeightField, brush: &Brush, x: f64, y: f64) -> bool {
        let spacing = (brush.radius_m as f64 * 0.18).max(0.25);
        let mut applied: Option<BrushRegion> = None;
        if let Some((lx, ly)) = self.last {
            let dx = x - lx;
            let dy = y - ly;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance < spacing * 0.20 {
                self.last = Some((x, y));
                return false;
            }
            let count = (distance / spacing).ceil() as usize;
            if count > 1 {
                for step in 1..count {
                    let t = step as f64 / count as f64;
                    if let Some(region) =
                        apply_stamp(hf, brush, lx + dx * t, ly + dy * t, self.seed, self.stamp)
                    {
                        applied = Some(match applied {
                            Some(existing) => existing.union(region),
                            None => region,
                        });
                    }
                    self.stamp += 1;
                }
            }
        }
        if let Some(region) = apply_stamp(hf, brush, x, y, self.seed, self.stamp) {
            applied = Some(match applied {
                Some(existing) => existing.union(region),
                None => region,
            });
            self.stamp += 1;
        }
        if let Some(region) = applied {
            self.region = Some(match self.region {
                Some(existing) => existing.union(region),
                None => region,
            });
        }
        self.last = Some((x, y));
        applied.is_some()
    }

    pub fn region(&self) -> Option<BrushRegion> {
        self.region
    }

    pub fn finish(mut self, hf: &HeightField) -> Option<BrushUndo> {
        // The region is the union of all applied stamps; `apply_stamp` updates
        // it through `record_region` below.
        let region = self.region.take()?;
        let mut before = Vec::with_capacity(region.width() * region.height());
        for y in region.y0..region.y1 {
            let start = y * hf.width() + region.x0;
            before.extend_from_slice(&self.snapshot[start..start + region.width()]);
        }
        let mut after = Vec::with_capacity(before.len());
        for y in region.y0..region.y1 {
            let start = y * hf.width() + region.x0;
            after.extend_from_slice(&hf.heights()[start..start + region.width()]);
        }
        Some(BrushUndo {
            label: self.label,
            region,
            before,
            after,
        })
    }
}

/// Apply one stamp and return the touched rectangle.
pub fn apply_stamp(
    hf: &mut HeightField,
    brush: &Brush,
    cx: f64,
    cy: f64,
    seed: Seed,
    stamp_index: u64,
) -> Option<BrushRegion> {
    if brush.validate().is_err() {
        return None;
    }
    let centers = symmetry_centers(hf, brush.symmetry, cx, cy);
    let mut region: Option<BrushRegion> = None;
    for (sx, sy) in centers {
        if let Some(r) = apply_one(hf, brush, sx, sy, seed, stamp_index) {
            region = Some(match region {
                Some(existing) => existing.union(r),
                None => r,
            });
        }
    }
    region
}

fn apply_one(
    hf: &mut HeightField,
    brush: &Brush,
    cx: f64,
    cy: f64,
    seed: Seed,
    stamp_index: u64,
) -> Option<BrushRegion> {
    let (sx, sy) = hf.spacing();
    let (ox, oy) = hf.origin();
    let radius = f64::from(brush.radius_m.max(0.05));
    let aspect = f64::from(brush.aspect.max(0.05));
    let reach = radius * aspect.max(1.0) * 1.15;
    let gx0 = ((cx - reach - ox) / sx).floor() as isize;
    let gy0 = ((cy - reach - oy) / sy).floor() as isize;
    let gx1 = ((cx + reach - ox) / sx).ceil() as isize;
    let gy1 = ((cy + reach - oy) / sy).ceil() as isize;
    let x0 = gx0.clamp(0, hf.width() as isize - 1) as usize;
    let y0 = gy0.clamp(0, hf.height() as isize - 1) as usize;
    let x1 = (gx1.clamp(0, hf.width() as isize - 1) as usize + 1).min(hf.width());
    let y1 = (gy1.clamp(0, hf.height() as isize - 1) as usize + 1).min(hf.height());
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let cell = (sx.max(sy) as f32).max(0.001);
    let angle = brush.angle_deg.to_radians();
    let (sin, cos) = (angle.sin() as f64, angle.cos() as f64);
    let seed_value = seed.derive(&format!("brush-{stamp_index}")).0;
    let target = target_height(hf, cx, cy);
    let mut touched = false;

    for y in y0..y1 {
        for x in x0..x1 {
            if !hf.is_valid(x, y) {
                continue;
            }
            let wx = ox + x as f64 * sx;
            let wy = oy + y as f64 * sy;
            let (u, v) = rotate(wx - cx, wy - cy, cos, sin);
            let normalised = shape_distance(brush, u, v, radius, aspect);
            if normalised >= 1.0 {
                continue;
            }
            let aa = (cell as f64 / radius).clamp(0.002, 0.25) * (1.0 + (8 - brush.aa_samples) as f64 / 16.0);
            let edge = 1.0 - smoothstep((1.0 - aa) as f32, 1.0, normalised as f32);
            let radial = falloff(brush.falloff, normalised as f32, brush.hardness);
            let coverage = edge * radial;
            if coverage <= 1e-5 {
                continue;
            }
            let modulated = modulate(brush, wx as f32, wy as f32, seed_value, coverage);
            let old = hf.get(x, y);
            let new = match brush.mode {
                BrushMode::Raise => old + signed(brush) * modulated * coverage,
                BrushMode::Lower => old - signed(brush) * modulated * coverage,
                BrushMode::Smooth => {
                    let average = neighbourhood_average(hf, x, y);
                    old + (average - old) * brush.strength.clamp(0.0, 1.0) * coverage
                }
                BrushMode::Flatten => {
                    old + (target - old) * brush.strength.clamp(0.0, 1.0) * coverage
                }
                BrushMode::Terrace => {
                    let step = brush.step_m.max(0.01);
                    let quantised = (old / step).round() * step;
                    old + (quantised - old) * brush.strength.clamp(0.0, 1.0) * coverage
                }
                BrushMode::Ridge => {
                    let n = value_noise(
                        wx as f32 / brush.noise_scale_m.max(0.01),
                        wy as f32 / brush.noise_scale_m.max(0.01),
                        seed_value,
                    );
                    let crest = (n * 2.0 - 1.0).abs().powf(0.6) * brush.strength;
                    old + signed(brush) * crest * coverage
                }
                BrushMode::Groove => {
                    let n = value_noise(
                        wx as f32 / brush.noise_scale_m.max(0.01),
                        wy as f32 / brush.noise_scale_m.max(0.01),
                        seed_value,
                    );
                    let channel = (1.0 - (n * 2.0 - 1.0).abs()).powf(0.7) * brush.strength;
                    old - signed(brush) * channel * coverage
                }
                BrushMode::Crater => {
                    let t = normalised as f32;
                    let bowl = (1.0 - t * t).max(0.0).powi(2);
                    let rim = (-(t - 0.92).powi(2) / 0.008).exp() * 0.25;
                    old - signed(brush) * brush.strength * (bowl - rim) * coverage
                }
                BrushMode::Noise => {
                    let n = value_noise(
                        wx as f32 / brush.noise_scale_m.max(0.01),
                        wy as f32 / brush.noise_scale_m.max(0.01),
                        seed_value,
                    );
                    old + signed(brush) * (n - 0.5) * 2.0 * brush.strength * coverage
                }
                BrushMode::Warp => {
                    let dx = (value_noise(
                        wx as f32 / brush.noise_scale_m.max(0.01) + 17.0,
                        wy as f32 / brush.noise_scale_m.max(0.01),
                        seed_value,
                    ) - 0.5) * 2.0 * brush.strength;
                    let dy = (value_noise(
                        wx as f32 / brush.noise_scale_m.max(0.01),
                        wy as f32 / brush.noise_scale_m.max(0.01) - 9.0,
                        seed_value,
                    ) - 0.5) * 2.0 * brush.strength;
                    let px = wx + f64::from(dx * coverage);
                    let py = wy + f64::from(dy * coverage);
                    sample_bilinear(hf, px, py)
                }
                BrushMode::Clone => {
                    let px = wx + f64::from(brush.clone_offset_m[0]);
                    let py = wy + f64::from(brush.clone_offset_m[1]);
                    let source = sample_bilinear(hf, px, py);
                    old + (source - old) * brush.strength.clamp(0.0, 1.0) * coverage
                }
            };
            if new.is_finite() {
                hf.set(x, y, new);
                touched = true;
            }
        }
    }
    if touched {
        Some(BrushRegion { x0, y0, x1, y1 })
    } else {
        None
    }
}

fn target_height(hf: &HeightField, cx: f64, cy: f64) -> f32 {
    sample_bilinear(hf, cx, cy)
}

fn signed(brush: &Brush) -> f32 {
    if brush.invert {
        -brush.strength.abs()
    } else {
        brush.strength.abs()
    }
}

fn modulate(brush: &Brush, x: f32, y: f32, seed: u64, coverage: f32) -> f32 {
    if brush.roughness <= 0.0 || brush.aa_samples <= 1 {
        return 1.0;
    }
    let samples = brush.aa_samples.min(8);
    let scale = brush.noise_scale_m.max(0.01);
    let mut sum = 0.0;
    for i in 0..samples {
        let angle = (i as f32) * 2.399_963;
        let offset = (i as f32 + 0.5) / samples as f32;
        let dx = angle.cos() * offset * 0.8;
        let dy = angle.sin() * offset * 0.8;
        let n = value_noise((x / scale) + dx, (y / scale) + dy, seed);
        sum += 0.65 + 0.7 * n;
    }
    let noise = sum / samples as f32;
    1.0 + (noise - 1.0) * brush.roughness * coverage
}

fn shape_distance(brush: &Brush, u: f64, v: f64, radius: f64, aspect: f64) -> f64 {
    let v = v / aspect;
    match brush.shape {
        BrushShape::Circle => (u * u + v * v).sqrt() / radius,
        BrushShape::Square => (u.abs().max(v.abs())) / radius,
        BrushShape::Diamond => (u.abs() + v.abs()) / radius,
        BrushShape::Ring => {
            let r = (u * u + v * v).sqrt();
            ((r - radius * 0.62).abs() / (radius * 0.38)).min(1.0)
        }
        BrushShape::Star => {
            let r = (u * u + v * v).sqrt();
            let theta = v.atan2(u);
            let boundary = radius * (0.68 + 0.32 * (5.0 * theta).cos());
            r / boundary.max(1e-6)
        }
        BrushShape::Polygon => {
            let theta = v.atan2(u);
            let sides = brush.sides.clamp(3, 24) as f64;
            let sector = (theta * sides / std::f64::consts::TAU).fract().abs();
            let boundary = radius * (std::f64::consts::PI / sides).cos()
                / ((sector * std::f64::consts::TAU / sides).cos().abs().max(0.2));
            (u * u + v * v).sqrt() / boundary.max(1e-6)
        }
    }
}

fn falloff(falloff: Falloff, t: f32, hardness: f32) -> f32 {
    let hard = hardness.clamp(0.0, 1.0);
    let plateau = 0.15 + 0.70 * hard;
    let t = ((t - plateau) / (1.0 - plateau).max(0.01)).clamp(0.0, 1.0);
    match falloff {
        Falloff::Constant => 1.0,
        Falloff::Linear => 1.0 - t,
        Falloff::Smooth => 1.0 - t * t * (3.0 - 2.0 * t),
        Falloff::Gaussian => (-3.0 * t * t).exp(),
        Falloff::Plateau => 1.0 - smoothstep(0.0, 1.0, t),
    }
}

fn symmetry_centers(hf: &HeightField, symmetry: Symmetry, x: f64, y: f64) -> Vec<(f64, f64)> {
    let bounds = hf.bounds();
    let cx = (bounds.min[0] + bounds.max[0]) * 0.5;
    let cy = (bounds.min[1] + bounds.max[1]) * 0.5;
    let mut out = vec![(x, y)];
    match symmetry {
        Symmetry::None => {}
        Symmetry::X => out.push((2.0 * cx - x, y)),
        Symmetry::Y => out.push((x, 2.0 * cy - y)),
        Symmetry::Xy => {
            out.push((2.0 * cx - x, y));
            out.push((x, 2.0 * cy - y));
            out.push((2.0 * cx - x, 2.0 * cy - y));
        }
        Symmetry::Radial(count) => {
            let count = count.clamp(2, 24);
            let dx = x - cx;
            let dy = y - cy;
            for i in 1..count {
                let angle = std::f64::consts::TAU * i as f64 / f64::from(count);
                let (s, c) = angle.sin_cos();
                out.push((cx + c * dx - s * dy, cy + s * dx + c * dy));
            }
        }
    }
    out
}

fn neighbourhood_average(hf: &HeightField, x: usize, y: usize) -> f32 {
    let mut sum = 0.0;
    let mut count = 0.0;
    for ky in y.saturating_sub(1)..=(y + 1).min(hf.height() - 1) {
        for kx in x.saturating_sub(1)..=(x + 1).min(hf.width() - 1) {
            if hf.is_valid(kx, ky) {
                sum += hf.get(kx, ky);
                count += 1.0;
            }
        }
    }
    if count > 0.0 { sum / count } else { hf.get(x, y) }
}

fn sample_bilinear(hf: &HeightField, x: f64, y: f64) -> f32 {
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let gx = (x - ox) / sx;
    let gy = (y - oy) / sy;
    hf.sample_bilinear(gx, gy)
}

fn rotate(x: f64, y: f64, cos: f64, sin: f64) -> (f64, f64) {
    (cos * x + sin * y, -sin * x + cos * y)
}


fn write_region(hf: &mut HeightField, region: BrushRegion, values: &[f32]) {
    for (row, y) in (region.y0..region.y1).enumerate() {
        let start = y * hf.width() + region.x0;
        let src = row * region.width();
        hf.heights_mut()[start..start + region.width()]
            .copy_from_slice(&values[src..src + region.width()]);
    }
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deterministic value noise in `[0, 1]` for brush roughness.
fn value_noise(x: f32, y: f32, seed: u64) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let ix = xi as i32;
    let iy = yi as i32;
    let v00 = hash01(ix, iy, seed);
    let v10 = hash01(ix + 1, iy, seed);
    let v01 = hash01(ix, iy + 1, seed);
    let v11 = hash01(ix + 1, iy + 1, seed);
    let u = fade(xf);
    let v = fade(yf);
    let a = v00 + (v10 - v00) * u;
    let b = v01 + (v11 - v01) * u;
    a + (b - a) * v
}

fn hash01(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = seed;
    h ^= u64::from(x as u32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= u64::from(y as u32).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::Seed;

    fn flat() -> HeightField {
        HeightField::filled(65, 65, 10.0, 100.0).unwrap()
    }

    #[test]
    fn raise_and_lower_are_local() {
        let mut hf = flat();
        let brush = Brush {
            radius_m: 60.0,
            strength: 5.0,
            ..Brush::default()
        };
        let region = apply_stamp(&mut hf, &brush, 300.0, 300.0, Seed::new(1), 0).unwrap();
        assert!(region.width() > 1);
        assert!((hf.get(32, 32) - 105.0).abs() < 2.0);
        assert!((hf.get(2, 2) - 100.0).abs() < 1e-4);
        let mut lower = brush.clone();
        lower.mode = BrushMode::Lower;
        apply_stamp(&mut hf, &lower, 300.0, 300.0, Seed::new(1), 1).unwrap();
        assert!((hf.get(32, 32) - 100.0).abs() < 3.0);
    }

    #[test]
    fn undo_and_redo_restore_the_region() {
        let mut hf = flat();
        let brush = Brush::default();
        let mut stroke = Stroke::begin(&hf, Seed::new(4), "test");
        stroke.stamp_to(&mut hf, &brush, 300.0, 300.0);
        stroke.stamp_to(&mut hf, &brush, 350.0, 300.0);
        let undo = stroke.finish(&hf).unwrap();
        let edited = hf.heights().to_vec();
        undo.undo(&mut hf);
        assert_eq!(hf.heights(), flat().heights());
        undo.redo(&mut hf);
        assert_eq!(hf.heights(), edited);
    }

    #[test]
    fn symmetry_x_mirrors_the_stamp() {
        let mut hf = flat();
        let brush = Brush {
            symmetry: Symmetry::X,
            radius_m: 40.0,
            roughness: 0.0,
            ..Brush::default()
        };
        apply_stamp(&mut hf, &brush, 100.0, 300.0, Seed::new(1), 0).unwrap();
        let left = hf.get(10, 30);
        let right = hf.get(54, 30);
        assert!((left - right).abs() < 1e-4);
    }

    #[test]
    fn square_shape_has_a_square_footprint() {
        let mut hf = flat();
        let brush = Brush {
            shape: BrushShape::Square,
            radius_m: 50.0,
            strength: 5.0,
            ..Brush::default()
        };
        apply_stamp(&mut hf, &brush, 300.0, 300.0, Seed::new(1), 0).unwrap();
        // Corner of the square is inside; a circle of the same radius would
        // not reach it.
        assert!(hf.get(27, 27) > 100.0);
    }

    #[test]
    fn flatten_moves_towards_the_target() {
        let mut hf = flat();
        hf.set(32, 32, 120.0);
        let brush = Brush {
            mode: BrushMode::Flatten,
            radius_m: 60.0,
            strength: 0.8,
            ..Brush::default()
        };
        apply_stamp(&mut hf, &brush, 300.0, 300.0, Seed::new(1), 0).unwrap();
        assert!(hf.get(32, 32) < 120.0);
    }

    #[test]
    fn stroke_interpolation_covers_the_segment() {
        let mut hf = flat();
        let brush = Brush {
            radius_m: 30.0,
            strength: 3.0,
            ..Brush::default()
        };
        let mut stroke = Stroke::begin(&hf, Seed::new(7), "line");
        stroke.stamp_to(&mut hf, &brush, 100.0, 300.0);
        stroke.stamp_to(&mut hf, &brush, 500.0, 300.0);
        // Midpoint must have been stamped by the interpolated path.
        assert!(hf.get(30, 30) > 101.0);
        assert!(stroke.finish(&hf).is_some());
    }

    #[test]
    fn brush_is_deterministic() {
        let mut a = flat();
        let mut b = flat();
        let brush = Brush {
            roughness: 0.8,
            aa_samples: 6,
            ..Brush::default()
        };
        apply_stamp(&mut a, &brush, 300.0, 300.0, Seed::new(9), 3).unwrap();
        apply_stamp(&mut b, &brush, 300.0, 300.0, Seed::new(9), 3).unwrap();
        assert_eq!(a.heights(), b.heights());
    }
}

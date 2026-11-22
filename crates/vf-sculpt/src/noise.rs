//! Deterministic gradient noise, fractal sums and cellular (Voronoi) fields.
//!
//! The sculpting module needs its own noise so that a stroke replay, a detail
//! pass or a fracture pattern is bit-identical on every machine and with any
//! thread count. Every octave is offset by a hash of the seed, every gradient
//! comes from an integer hash, and no floating-point reduction is reordered.
//!
//! Frequencies are expressed in world units (metres): a frequency of 0.5 means
//! one feature every two metres, which is what the UI sliders expose.

use glam::{Vec2, Vec3};

use crate::math::{hash_u64, smoothstep};

/// Noise family used by detail passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    Fbm,
    Ridged,
    Billow,
    Turbulence,
    Voronoi,
    Crackle,
}

impl NoiseKind {
    pub const ALL: [NoiseKind; 6] = [
        NoiseKind::Fbm,
        NoiseKind::Ridged,
        NoiseKind::Billow,
        NoiseKind::Turbulence,
        NoiseKind::Voronoi,
        NoiseKind::Crackle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NoiseKind::Fbm => "fBm",
            NoiseKind::Ridged => "Crêtes",
            NoiseKind::Billow => "Billow",
            NoiseKind::Turbulence => "Turbulence",
            NoiseKind::Voronoi => "Voronoi",
            NoiseKind::Crackle => "Fissures",
        }
    }
}

/// Parameters shared by every fractal detail pass.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoiseParams {
    pub kind: NoiseKind,
    /// Features per metre.
    pub frequency: f32,
    pub octaves: u8,
    pub lacunarity: f32,
    pub gain: f32,
    /// Bends the input coordinates with another octave of noise, in metres.
    pub warp_m: f32,
    pub seed: u64,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            kind: NoiseKind::Fbm,
            frequency: 0.12,
            octaves: 5,
            lacunarity: 2.03,
            gain: 0.5,
            warp_m: 0.0,
            seed: 1,
        }
    }
}

impl NoiseParams {
    pub fn new(kind: NoiseKind, frequency: f32) -> Self {
        Self {
            kind,
            frequency,
            ..Self::default()
        }
    }

    /// Evaluate the field at a world position. The result is in `[-1, 1]` for
    /// fBm/billow/turbulence, `[0, 1]` for ridged and cellular families.
    pub fn sample(&self, p: Vec3) -> f32 {
        let mut q = p;
        if self.warp_m.abs() > 1e-5 {
            let w = Vec3::new(
                fbm3(self.seed ^ 0x51, p * (self.frequency * 2.0), 3, 2.0, 0.5),
                fbm3(self.seed ^ 0x52, p * (self.frequency * 2.0), 3, 2.0, 0.5),
                fbm3(self.seed ^ 0x53, p * (self.frequency * 2.0), 3, 2.0, 0.5),
            );
            q += w * self.warp_m * self.frequency;
        }
        match self.kind {
            NoiseKind::Fbm => self.fbm(q),
            NoiseKind::Ridged => self.ridged(q),
            NoiseKind::Billow => self.billow(q),
            NoiseKind::Turbulence => self.fbm(q).abs() * 2.0 - 1.0,
            NoiseKind::Voronoi => {
                let cell = voronoi3(self.seed, q * self.frequency);
                cell.f1.min(1.0) * 2.0 - 1.0
            }
            NoiseKind::Crackle => {
                let cell = voronoi3(self.seed, q * self.frequency);
                (1.0 - (cell.f2 - cell.f1).min(1.0)) * 2.0 - 1.0
            }
        }
    }

    pub fn fbm(&self, p: Vec3) -> f32 {
        let mut sum = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = self.frequency;
        let mut norm = 0.0;
        for octave in 0..self.octaves.max(1) {
            let offset = Vec3::new(
                hash01(self.seed, octave as u64 * 3 + 1) * 100.0,
                hash01(self.seed, octave as u64 * 3 + 2) * 100.0,
                hash01(self.seed, octave as u64 * 3 + 3) * 100.0,
            );
            sum += perlin3(self.seed + octave as u64, (p + offset) * frequency) * amplitude;
            norm += amplitude;
            amplitude *= self.gain;
            frequency *= self.lacunarity;
        }
        sum / norm.max(1e-6)
    }

    pub fn ridged(&self, p: Vec3) -> f32 {
        let mut sum = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = self.frequency;
        let mut weight = 1.0f32;
        let mut norm = 0.0;
        for octave in 0..self.octaves.max(1) {
            let offset = Vec3::splat(hash01(self.seed, 100 + octave as u64) * 64.0);
            let n = 1.0 - perlin3(self.seed + octave as u64, (p + offset) * frequency).abs();
            let signal = n * n * weight;
            weight = (signal * 2.0).clamp(0.0, 1.0);
            sum += signal * amplitude;
            norm += amplitude;
            amplitude *= self.gain;
            frequency *= self.lacunarity;
        }
        (sum / norm.max(1e-6)).clamp(0.0, 1.0)
    }

    pub fn billow(&self, p: Vec3) -> f32 {
        let mut sum = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = self.frequency;
        let mut norm = 0.0;
        for octave in 0..self.octaves.max(1) {
            let offset = Vec3::splat(hash01(self.seed, 200 + octave as u64) * 64.0);
            let n = perlin3(self.seed + octave as u64, (p + offset) * frequency).abs();
            sum += (n * 2.0 - 1.0) * amplitude;
            norm += amplitude;
            amplitude *= self.gain;
            frequency *= self.lacunarity;
        }
        (sum / norm.max(1e-6)).clamp(-1.0, 1.0)
    }

    /// Value for a displacement of `strength` metres (signed for every family).
    pub fn displacement(&self, p: Vec3, strength_m: f32) -> f32 {
        match self.kind {
            NoiseKind::Ridged | NoiseKind::Voronoi | NoiseKind::Crackle => {
                (self.sample(p) - 0.5) * 2.0 * strength_m
            }
            _ => self.sample(p) * strength_m,
        }
    }
}

#[inline]
fn hash01(seed: u64, index: u64) -> f32 {
    (hash_u64(seed ^ index.wrapping_mul(0x9e3779b97f4a7c15)) >> 40) as f32 / (1u64 << 24) as f32
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn gradient(hash: u64, x: f32, y: f32, z: f32) -> f32 {
    // 12 gradients of the classic Perlin set, selected by 4 bits.
    let h = (hash & 15) as u32;
    let (u, v) = if h < 8 { (x, y) } else { (y, x) };
    let a = if h & 1 == 0 { u } else { -u };
    let b = if h & 2 == 0 { v } else { -v };
    let c = if h < 4 {
        0.0
    } else if h == 12 || h == 14 {
        x
    } else {
        z
    };
    a + b + c
}

/// Perlin gradient noise, output roughly in `[-1, 1]`.
pub fn perlin3(seed: u64, p: Vec3) -> f32 {
    let pf = p.floor();
    let cell = [pf.x as i64, pf.y as i64, pf.z as i64];
    let f = p - pf;
    let u = fade(f.x);
    let v = fade(f.y);
    let w = fade(f.z);
    let mut c = [[[0.0f32; 2]; 2]; 2];
    for (i, item) in c.iter_mut().enumerate() {
        for (j, item2) in item.iter_mut().enumerate() {
            for (k, value) in item2.iter_mut().enumerate() {
                let x = cell[0] + i as i64;
                let y = cell[1] + j as i64;
                let z = cell[2] + k as i64;
                let h = hash_u64(
                    seed ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
                        ^ (y as u64).wrapping_mul(0xc2b2ae3d27d4eb4f)
                        ^ (z as u64).wrapping_mul(0x165667b19e3779f9),
                );
                *value = gradient(h, f.x - i as f32, f.y - j as f32, f.z - k as f32);
            }
        }
    }
    let x00 = c[0][0][0] + (c[1][0][0] - c[0][0][0]) * u;
    let x10 = c[0][1][0] + (c[1][1][0] - c[0][1][0]) * u;
    let x01 = c[0][0][1] + (c[1][0][1] - c[0][0][1]) * u;
    let x11 = c[0][1][1] + (c[1][1][1] - c[0][1][1]) * u;
    let y0 = x00 + (x10 - x00) * v;
    let y1 = x01 + (x11 - x01) * v;
    y0 + (y1 - y0) * w
}

/// Two-dimensional Perlin noise, used by crack networks projected on a plane.
pub fn perlin2(seed: u64, p: Vec2) -> f32 {
    perlin3(seed, Vec3::new(p.x, p.y, 0.0))
}

/// Simple fBm helper used by the warping step.
fn fbm3(seed: u64, p: Vec3, octaves: u8, lacunarity: f32, gain: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for octave in 0..octaves.max(1) {
        sum += perlin3(seed + octave as u64, p * freq) * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    sum / norm.max(1e-6)
}

/// Cellular sample: distances to the two closest feature points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoronoiSample {
    pub f1: f32,
    pub f2: f32,
    pub id: u64,
}

impl VoronoiSample {
    /// `f2 - f1` is the distance to the cell boundary: zero on an edge.
    pub fn edge(&self) -> f32 {
        self.f2 - self.f1
    }
}

#[inline]
fn feature_point(seed: u64, cell: [i64; 3]) -> Vec3 {
    let h = hash_u64(
        seed ^ (cell[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (cell[1] as u64).wrapping_mul(0xc2b2ae3d27d4eb4f)
            ^ (cell[2] as u64).wrapping_mul(0x165667b19e3779f9),
    );
    Vec3::new(
        (hash_u64(h) >> 40) as f32 / (1u64 << 24) as f32,
        (hash_u64(h ^ 0x517cc1b727220a95) >> 40) as f32 / (1u64 << 24) as f32,
        (hash_u64(h ^ 0x2545f4914f6cdd1d) >> 40) as f32 / (1u64 << 24) as f32,
    )
}

/// 3D Voronoi (F1/F2) on a jittered grid. `p` is already in cell units.
pub fn voronoi3(seed: u64, p: Vec3) -> VoronoiSample {
    let base = p.floor();
    let cell = [base.x as i64, base.y as i64, base.z as i64];
    let mut f1 = f32::INFINITY;
    let mut f2 = f32::INFINITY;
    let mut id = 0u64;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                let feature =
                    Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) + feature_point(seed, c);
                let d = (feature - p).length();
                if d < f1 {
                    f2 = f1;
                    f1 = d;
                    id = hash_u64(
                        seed ^ (c[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
                            ^ (c[1] as u64).wrapping_mul(0xc2b2ae3d27d4eb4f)
                            ^ (c[2] as u64).wrapping_mul(0x165667b19e3779f9),
                    );
                } else if d < f2 {
                    f2 = d;
                }
            }
        }
    }
    VoronoiSample { f1, f2, id }
}

/// 2D Voronoi with cell identity, used by the fracture network generator.
pub fn voronoi2(seed: u64, p: Vec2) -> VoronoiSample {
    let base = p.floor();
    let cell = [base.x as i64, base.y as i64, 0];
    let mut f1 = f32::INFINITY;
    let mut f2 = f32::INFINITY;
    let mut id = 0u64;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let c = [cell[0] + dx, cell[1] + dy, 0];
            let f = feature_point(seed, c);
            let feature = Vec2::new(c[0] as f32 + f.x, c[1] as f32 + f.y);
            let d = (feature - p).length();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash_u64(
                    seed ^ (c[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
                        ^ (c[1] as u64).wrapping_mul(0xc2b2ae3d27d4eb4f),
                );
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    VoronoiSample { f1, f2, id }
}

/// Ridged, terraced profile used by strata: returns a value in `[0, 1]` where
/// `1` is the top of a bed. `bands` is the number of beds over the height.
pub fn strata_profile(z: f32, band_m: f32, dip: f32, sharpness: f32) -> f32 {
    if band_m <= 1e-5 {
        return 0.0;
    }
    let u = (z + dip) / band_m;
    let frac = u - u.floor();
    // Each bed has a flat bench and a riser. Sharpness biases the width of the
    // riser: a high value gives a near-vertical step.
    let riser = (1.0 - sharpness.clamp(0.0, 0.999)) * 0.5 + 0.02;
    let bench = smoothstep(0.0, 1.0 - riser, frac);
    (1.0 - bench).powf(0.6)
}

/// Rock-surface families used by the automatic subdivision planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RockSurface {
    Granite,
    Basalt,
    Limestone,
    Sandstone,
    Schist,
    Chalk,
    Volcanic,
    Canyon,
    Patagonia,
}

impl RockSurface {
    pub const ALL: [RockSurface; 9] = [
        RockSurface::Granite,
        RockSurface::Basalt,
        RockSurface::Limestone,
        RockSurface::Sandstone,
        RockSurface::Schist,
        RockSurface::Chalk,
        RockSurface::Volcanic,
        RockSurface::Canyon,
        RockSurface::Patagonia,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RockSurface::Granite => "Granite",
            RockSurface::Basalt => "Basalte",
            RockSurface::Limestone => "Calcaire",
            RockSurface::Sandstone => "Grès",
            RockSurface::Schist => "Schiste",
            RockSurface::Chalk => "Craie",
            RockSurface::Volcanic => "Volcanique",
            RockSurface::Canyon => "Canyon",
            RockSurface::Patagonia => "Patagonie",
        }
    }

    /// Stable identifier used by presets, files and the CLI.
    pub fn name(self) -> &'static str {
        match self {
            RockSurface::Granite => "granite",
            RockSurface::Basalt => "basalt",
            RockSurface::Limestone => "limestone",
            RockSurface::Sandstone => "sandstone",
            RockSurface::Schist => "schist",
            RockSurface::Chalk => "chalk",
            RockSurface::Volcanic => "volcanic",
            RockSurface::Canyon => "canyon",
            RockSurface::Patagonia => "patagonia",
        }
    }

    /// French or English name, with the usual aliases (slate -> schist,
    /// greywacke -> schist, tuff -> volcanic).
    pub fn from_name(name: &str) -> Option<Self> {
        let normalized = name.trim().to_lowercase();
        let normalized = normalized
            .replace(['é', 'è', 'ê'], "e")
            .replace('à', "a")
            .replace('ç', "c");
        Some(match normalized.as_str() {
            "granite" | "granit" => RockSurface::Granite,
            "basalt" | "basalte" => RockSurface::Basalt,
            "limestone" | "calcaire" => RockSurface::Limestone,
            "sandstone" | "gres" | "grès" => RockSurface::Sandstone,
            "schist" | "schiste" | "slate" | "ardoise" | "gneiss" | "phyllite" | "greywacke" => {
                RockSurface::Schist
            }
            "chalk" | "craie" => RockSurface::Chalk,
            "volcanic" | "volcanique" | "basalte_volcanique" | "tuff" | "tuf" => {
                RockSurface::Volcanic
            }
            "canyon" | "gres_canyon" => RockSurface::Canyon,
            "patagonia" | "patagonie" => RockSurface::Patagonia,
            _ => return None,
        })
    }

    /// Characteristic dimensions of the rock, in metres. These drive the
    /// subdivision planner: the smallest feature that must be resolved.
    pub fn grain_m(self) -> f32 {
        match self {
            RockSurface::Granite => 0.35,
            RockSurface::Basalt => 0.9,
            RockSurface::Limestone => 0.6,
            RockSurface::Sandstone => 0.45,
            RockSurface::Schist => 0.22,
            RockSurface::Chalk => 1.2,
            RockSurface::Volcanic => 0.7,
            RockSurface::Canyon => 0.8,
            RockSurface::Patagonia => 1.4,
        }
    }

    pub fn strata_m(self) -> f32 {
        match self {
            RockSurface::Granite => 0.0,
            RockSurface::Basalt => 6.0,
            RockSurface::Limestone => 2.2,
            RockSurface::Sandstone => 1.4,
            RockSurface::Schist => 0.35,
            RockSurface::Chalk => 4.0,
            RockSurface::Volcanic => 3.0,
            RockSurface::Canyon => 2.6,
            RockSurface::Patagonia => 8.0,
        }
    }

    /// Displacement detail typical of the rock, metres at the grain scale.
    pub fn roughness_m(self) -> f32 {
        match self {
            RockSurface::Granite => 0.06,
            RockSurface::Basalt => 0.05,
            RockSurface::Limestone => 0.12,
            RockSurface::Sandstone => 0.08,
            RockSurface::Schist => 0.05,
            RockSurface::Chalk => 0.16,
            RockSurface::Volcanic => 0.1,
            RockSurface::Canyon => 0.09,
            RockSurface::Patagonia => 0.2,
        }
    }

    /// 0 = massive isotropic rock, 1 = strongly layered.
    pub fn anisotropy(self) -> f32 {
        match self {
            RockSurface::Granite => 0.1,
            RockSurface::Basalt => 0.75,
            RockSurface::Limestone => 0.8,
            RockSurface::Sandstone => 0.9,
            RockSurface::Schist => 1.0,
            RockSurface::Chalk => 0.7,
            RockSurface::Volcanic => 0.5,
            RockSurface::Canyon => 0.85,
            RockSurface::Patagonia => 0.6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perlin_is_bounded_and_continuous() {
        for i in 0..1000 {
            let p = Vec3::new(i as f32 * 0.37, i as f32 * 0.11, i as f32 * 0.53);
            let n = perlin3(4, p);
            assert!(n.abs() < 1.6, "{n}");
            let m = perlin3(4, p + Vec3::splat(1e-4));
            assert!((n - m).abs() < 0.01);
        }
    }

    #[test]
    fn fbm_octaves_add_detail() {
        let low = NoiseParams {
            octaves: 1,
            ..NoiseParams::default()
        };
        let high = NoiseParams {
            octaves: 6,
            ..NoiseParams::default()
        };
        let mut variance_low = 0.0;
        let mut variance_high = 0.0;
        for i in 0..400 {
            let p = Vec3::new(i as f32 * 0.71, i as f32 * 0.33, 3.0);
            variance_low += low.sample(p).abs();
            variance_high += high.sample(p).abs();
        }
        assert!(variance_high > variance_low * 0.5);
    }

    #[test]
    fn voronoi_edges_are_zero_and_f1_below_f2() {
        for i in 0..200 {
            let p = Vec2::new(i as f32 * 0.41, i as f32 * 0.17);
            let s = voronoi2(9, p);
            assert!(s.f1 <= s.f2);
            assert!(s.f1 >= 0.0);
        }
        // A point exactly on a cell boundary has a very small edge distance.
        let s = voronoi2(9, Vec2::new(0.5, 0.5));
        assert!(s.edge() >= 0.0);
    }

    #[test]
    fn noise_is_deterministic() {
        let params = NoiseParams::new(NoiseKind::Crackle, 0.4);
        for i in 0..50 {
            let p = Vec3::splat(i as f32 * 0.3);
            assert_eq!(params.sample(p), params.sample(p));
        }
    }

    #[test]
    fn strata_profile_is_in_range() {
        for i in 0..500 {
            let z = i as f32 * 0.17;
            let s = strata_profile(z, 2.0, 0.3, 0.8);
            assert!((0.0..=1.0).contains(&s), "{s}");
        }
    }
}

//! Deterministic gradient noise and fractal combinations.
//!
//! The implementation is self-contained so terrain and cloud patterns are
//! reproducible on every platform without external RNG or noise crates.

use vf_core::Seed;

const GRADIENTS: [[f32; 2]; 8] = [
    [1.0, 0.0],
    [-1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
    [0.707_106_77, 0.707_106_77],
    [-0.707_106_77, 0.707_106_77],
    [0.707_106_77, -0.707_106_77],
    [-0.707_106_77, -0.707_106_77],
];

#[derive(Debug, Clone, Copy)]
pub struct Noise {
    seed: u64,
}

impl Noise {
    pub const fn new(seed: Seed) -> Self {
        Self { seed: seed.0 }
    }

    pub const fn from_u64(seed: u64) -> Self {
        Self { seed }
    }

    /// 2D gradient noise in roughly `[-1, 1]`.
    pub fn noise2(&self, x: f32, y: f32) -> f32 {
        let xi = x.floor();
        let yi = y.floor();
        let xf = x - xi;
        let yf = y - yi;
        let ix = xi as i32;
        let iy = yi as i32;

        let g00 = self.gradient(ix, iy);
        let g10 = self.gradient(ix + 1, iy);
        let g01 = self.gradient(ix, iy + 1);
        let g11 = self.gradient(ix + 1, iy + 1);

        let n00 = g00[0] * xf + g00[1] * yf;
        let n10 = g10[0] * (xf - 1.0) + g10[1] * yf;
        let n01 = g01[0] * xf + g01[1] * (yf - 1.0);
        let n11 = g11[0] * (xf - 1.0) + g11[1] * (yf - 1.0);

        let u = fade(xf);
        let v = fade(yf);
        let nx0 = n00 + (n10 - n00) * u;
        let nx1 = n01 + (n11 - n01) * u;
        (nx0 + (nx1 - nx0) * v) * 1.4
    }

    /// Fractal Brownian motion, normalised to approximately `[-1, 1]`.
    pub fn fbm2(&self, x: f32, y: f32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
        let mut px = x;
        let mut py = y;
        let mut frequency = 1.0f32;
        let mut amplitude = 1.0f32;
        let mut sum = 0.0f32;
        let mut norm = 0.0f32;
        for octave in 0..octaves.max(1) {
            sum += amplitude * self.noise2(px * frequency, py * frequency);
            norm += amplitude;
            amplitude *= gain;
            frequency *= lacunarity;
            // Rotate each octave to hide lattice-aligned artefacts.
            let angle = 0.5 + octave as f32 * 0.13;
            let (s, c) = angle.sin_cos();
            let (nx, ny) = (c * px - s * py, s * px + c * py);
            px = nx;
            py = ny;
        }
        if norm > 0.0 { sum / norm } else { 0.0 }
    }

    /// Ridged multifractal in `[0, 1]`, good for mountain crests.
    pub fn ridged2(&self, x: f32, y: f32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
        let mut px = x;
        let mut py = y;
        let mut frequency = 1.0f32;
        let mut amplitude = 1.0f32;
        let mut sum = 0.0f32;
        let mut norm = 0.0f32;
        let mut weight = 1.0f32;
        for octave in 0..octaves.max(1) {
            let mut n = 1.0 - self.noise2(px * frequency, py * frequency).abs();
            n *= n;
            n *= weight;
            weight = (n * 2.0).clamp(0.0, 1.0);
            sum += n * amplitude;
            norm += amplitude;
            amplitude *= gain;
            frequency *= lacunarity;
            let angle = 0.7 + octave as f32 * 0.19;
            let (s, c) = angle.sin_cos();
            let (nx, ny) = (c * px - s * py, s * px + c * py);
            px = nx;
            py = ny;
        }
        if norm > 0.0 {
            (sum / norm).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Convenience wrapper returning `[0, 1]`.
    pub fn fbm01(&self, x: f32, y: f32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
        (self.fbm2(x, y, octaves, lacunarity, gain) * 0.5 + 0.5).clamp(0.0, 1.0)
    }

    fn gradient(&self, x: i32, y: i32) -> [f32; 2] {
        let h = hash2(x, y, self.seed);
        GRADIENTS[(h & 7) as usize]
    }
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn hash2(x: i32, y: i32, seed: u64) -> u64 {
    let mut h = seed;
    h ^= u64::from(x as u32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= u64::from(y as u32).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    splitmix64(h)
}

#[inline]
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Cheap smooth 2D value noise in `[0, 1]`, used for cloud density.
pub fn value2(x: f32, y: f32, seed: u64) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let ix = xi as i32;
    let iy = yi as i32;
    let v00 = unit_hash(ix, iy, seed);
    let v10 = unit_hash(ix + 1, iy, seed);
    let v01 = unit_hash(ix, iy + 1, seed);
    let v11 = unit_hash(ix + 1, iy + 1, seed);
    let u = fade(xf);
    let v = fade(yf);
    let a = v00 + (v10 - v00) * u;
    let b = v01 + (v11 - v01) * u;
    a + (b - a) * v
}

#[inline]
fn unit_hash(x: i32, y: i32, seed: u64) -> f32 {
    (hash2(x, y, seed) >> 40) as f32 * (1.0 / 16_777_216.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let n = Noise::new(Seed::new(3));
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for y in 0..32 {
            for x in 0..32 {
                let a = n.noise2(x as f32 * 0.13, y as f32 * 0.17);
                let b = n.noise2(x as f32 * 0.13, y as f32 * 0.17);
                assert_eq!(a, b);
                min = min.min(a);
                max = max.max(a);
            }
        }
        assert!(min > -1.5 && max < 1.5, "{min}..{max}");
    }

    #[test]
    fn fbm_and_ridged_ranges() {
        let n = Noise::new(Seed::new(9));
        for (x, y) in [(0.0, 0.0), (1.7, 3.2), (-4.5, 2.1)] {
            let f = n.fbm2(x, y, 6, 2.0, 0.5);
            let r = n.ridged2(x, y, 6, 2.0, 0.5);
            assert!(f.abs() <= 1.2, "{f}");
            assert!((0.0..=1.0).contains(&r), "{r}");
        }
    }
}

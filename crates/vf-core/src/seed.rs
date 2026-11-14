use std::time::{SystemTime, UNIX_EPOCH};

/// A deterministic seed with named sub-streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Seed(pub u64);

impl Seed {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Derive a stable sub-seed from a label (FNV-1a 64 on the label,
    /// then splitmix64 finalisation).
    pub fn derive(self, label: &str) -> Self {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        for b in label.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        Self(splitmix64(self.0 ^ h))
    }

    pub fn rng(self) -> Rng {
        Rng::new(self)
    }

    /// A non-deterministic seed for "new random scene" actions. The value is
    /// always reported to the caller so the scene can be reproduced.
    pub fn from_time() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self(splitmix64(nanos ^ 0xA076_1D64_78BD_642F))
    }
}

impl Default for Seed {
    fn default() -> Self {
        Self(1)
    }
}

impl std::fmt::Display for Seed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// SplitMix64 generator. Small, fast, deterministic and dependency-free.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub const fn new(seed: Seed) -> Self {
        Self { state: seed.0 }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        splitmix64(self.state)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform float in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        // 24 significant bits, exactly representable.
        ((self.next_u64() >> 40) as f32) * (1.0 / 16_777_216.0)
    }

    /// Uniform float in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) * (1.0 / 9_007_199_254_740_992.0)
    }

    pub fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_f32()
    }

    pub fn range_f64(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.next_f64()
    }

    /// Uniform integer in `[0, n)`. Returns 0 for `n == 0`.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform float in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// Approximately normal (mean 0, stddev 1) via the sum of four uniforms.
    pub fn gaussian(&mut self) -> f32 {
        (self.next_f32() + self.next_f32() + self.next_f32() + self.next_f32() - 2.0) * 1.732_050_8
    }
}

#[inline]
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_stable_and_distinct() {
        let s = Seed::new(42);
        assert_eq!(s.derive("erosion"), s.derive("erosion"));
        assert_ne!(s.derive("erosion"), s.derive("trees"));
        assert_ne!(s.derive("erosion"), s);
    }

    #[test]
    fn rng_is_reproducible_and_bounded() {
        let mut a = Seed::new(7).rng();
        let mut b = Seed::new(7).rng();
        for _ in 0..10_000 {
            let x = a.next_f32();
            assert_eq!(x, b.next_f32());
            assert!((0.0..1.0).contains(&x));
        }
    }
}

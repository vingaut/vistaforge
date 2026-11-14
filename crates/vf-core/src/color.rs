use serde::{Deserialize, Serialize};

/// Linear RGB colour in `[0, 1]` (scene-referred).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        )
    }

    pub fn from_hex(hex: u32) -> Self {
        Self::from_u8(
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        )
    }

    /// sRGB-encoded components to linear light.
    pub fn from_srgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(
            srgb_to_linear(f32::from(r) / 255.0),
            srgb_to_linear(f32::from(g) / 255.0),
            srgb_to_linear(f32::from(b) / 255.0),
        )
    }

    pub fn to_srgb_u8(self) -> [u8; 3] {
        [
            (linear_to_srgb(self.r.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (linear_to_srgb(self.g.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (linear_to_srgb(self.b.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
        ]
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self::new(
            self.r + (other.r - self.r) * t,
            self.g + (other.g - self.g) * t,
            self.b + (other.b - self.b) * t,
        )
    }

    pub fn scale(self, k: f32) -> Self {
        Self::new(self.r * k, self.g * k, self.b * k)
    }

    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    pub fn saturate(self, factor: f32) -> Self {
        let l = self.luminance();
        Self::new(
            l + (self.r - l) * factor,
            l + (self.g - l) * factor,
            l + (self.b - l) * factor,
        )
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }
}

impl std::ops::Add for Rgb {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.r + other.r, self.g + other.g, self.b + other.b)
    }
}

impl std::ops::Mul for Rgb {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self::new(self.r * other.r, self.g * other.g, self.b * other.b)
    }
}

impl Default for Rgb {
    fn default() -> Self {
        Self::BLACK
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for v in [0u8, 1, 64, 128, 200, 255] {
            let c = Rgb::from_srgb_u8(v, v, v);
            assert_eq!(c.to_srgb_u8(), [v, v, v]);
        }
    }

    #[test]
    fn interpolation_endpoints() {
        let a = Rgb::from_hex(0x112233);
        let b = Rgb::from_hex(0xaabbcc);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
    }
}

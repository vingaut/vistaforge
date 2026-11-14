//! PNG and raw image containers shared with the renderer and UI.

use std::path::Path;

use crate::{IoError, Result};

/// 8-bit RGBA image, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(IoError::Invalid(format!(
                "RGBA buffer has {} bytes, expected {expected} for {width}x{height}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn from_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<Self> {
        let expected = width as usize * height as usize * 3;
        if rgb.len() != expected {
            return Err(IoError::Invalid(format!(
                "RGB buffer has {} bytes, expected {expected}",
                rgb.len()
            )));
        }
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for chunk in rgb.as_chunks::<3>().0 {
            pixels.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
        }
        Self::new(width, height, pixels)
    }

    pub fn save_png(&self, path: &Path) -> Result<()> {
        let buffer = image::RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or_else(|| IoError::Invalid("invalid RGBA dimensions".into()))?;
        buffer.save(path)?;
        Ok(())
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub fn average_luminance(&self) -> f64 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        let sum: u64 = self
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                u64::from(
                    (u32::from(p[0]) * 2126 + u32::from(p[1]) * 7152 + u32::from(p[2]) * 722)
                        / 10_000,
                )
            })
            .sum();
        sum as f64 / (self.pixels.len() / 4) as f64
    }
}

pub fn save_png_rgba(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    RgbaImage::new(width, height, pixels.to_vec())?.save_png(path)
}

pub fn save_png_rgb(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    RgbaImage::from_rgb(width, height, pixels)?.save_png(path)
}

pub fn save_png_gray(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    let expected = width as usize * height as usize;
    if pixels.len() != expected {
        return Err(IoError::Invalid(format!(
            "grayscale buffer has {} bytes, expected {expected}",
            pixels.len()
        )));
    }
    let buffer = image::GrayImage::from_raw(width, height, pixels.to_vec())
        .ok_or_else(|| IoError::Invalid("invalid grayscale dimensions".into()))?;
    buffer.save(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_validation() {
        assert!(RgbaImage::new(2, 2, vec![0; 16]).is_ok());
        assert!(RgbaImage::new(2, 2, vec![0; 15]).is_err());
    }

    #[test]
    fn rgb_conversion_sets_alpha() {
        let img = RgbaImage::from_rgb(1, 1, &[1, 2, 3]).unwrap();
        assert_eq!(img.pixel(0, 0), [1, 2, 3, 255]);
    }
}

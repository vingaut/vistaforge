//! Heightmap image import/export (PNG, JPEG, TIFF) and world-file metadata.
//!
//! PNG and TIFF are the lossless interchange formats for heightmaps; JPEG is
//! accepted for convenience but is lossy. A sidecar JSON keeps the absolute
//! elevation range and georeferencing so a PNG export can be re-imported
//! without guessing. The sidecar and `.pgw`/`.tfw` world file are optional:
//! any grayscale image can be imported with an explicit height range.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use vf_core::HeightField;

use crate::geotiff::{GeoReference, read_geotiff};
use crate::{IoError, Result};

pub const HEIGHTMAP_FORMAT: &str = "vistaforge.heightmap.v1";

/// Sidecar describing a heightmap image.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeightmapMeta {
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub min_height: f32,
    pub max_height: f32,
    pub spacing_x: f64,
    pub spacing_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub nodata: Option<f32>,
    pub bit_depth: u8,
}

impl Default for HeightmapMeta {
    fn default() -> Self {
        Self {
            format: HEIGHTMAP_FORMAT.into(),
            width: 0,
            height: 0,
            min_height: 0.0,
            max_height: 1000.0,
            spacing_x: 1.0,
            spacing_y: 1.0,
            origin_x: 0.0,
            origin_y: 0.0,
            nodata: None,
            bit_depth: 16,
        }
    }
}

/// A decoded heightmap, north row first (image order).
#[derive(Debug, Clone)]
pub struct HeightmapImport {
    pub width: usize,
    pub height: usize,
    /// Normalised 0..1 when `normalized`, otherwise metres.
    pub values: Vec<f32>,
    pub normalized: bool,
    pub bit_depth: u8,
    pub meta: Option<HeightmapMeta>,
    pub geo: Option<GeoReference>,
    pub source: String,
}

impl HeightmapImport {
    /// Range suggested by the sidecar, or the observed range for direct
    /// (floating-point/GeoTIFF) values.
    pub fn suggested_range(&self) -> (f32, f32) {
        if let Some(meta) = &self.meta {
            return (meta.min_height, meta.max_height);
        }
        if self.normalized {
            return (0.0, 1000.0);
        }
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for value in &self.values {
            if value.is_finite() {
                min = min.min(*value);
                max = max.max(*value);
            }
        }
        if min.is_finite() && max.is_finite() {
            (min, max)
        } else {
            (0.0, 1000.0)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HeightmapOptions {
    pub min_height: f32,
    pub max_height: f32,
    /// 0 or negative means "use the file georeferencing, else 1 m".
    pub spacing_m: f64,
    pub origin: [f64; 2],
    /// Image row 0 is north; VistaForge row 0 is south. Flip by default.
    pub flip_y: bool,
}

impl Default for HeightmapOptions {
    fn default() -> Self {
        Self {
            min_height: 0.0,
            max_height: 1000.0,
            spacing_m: 0.0,
            origin: [0.0, 0.0],
            flip_y: true,
        }
    }
}

impl HeightmapImport {
    pub fn into_heightfield(&self, options: HeightmapOptions) -> Result<HeightField> {
        if self.width < 2 || self.height < 2 {
            return Err(IoError::Invalid(format!(
                "heightmap is too small: {}x{}",
                self.width, self.height
            )));
        }
        if self.width > 16_384 || self.height > 16_384 {
            return Err(IoError::Invalid(format!(
                "heightmap is too large: {}x{} (max 16384x16384)",
                self.width, self.height
            )));
        }
        let (spacing_x, spacing_y) = if options.spacing_m > 0.0 {
            (options.spacing_m, options.spacing_m)
        } else if let Some(geo) = self.geo {
            (geo.spacing_x.abs().max(1e-6), geo.spacing_y.abs().max(1e-6))
        } else if let Some(meta) = &self.meta {
            (
                meta.spacing_x.abs().max(1e-6),
                meta.spacing_y.abs().max(1e-6),
            )
        } else {
            (1.0, 1.0)
        };
        let (origin_x, origin_y) = if let Some(geo) = self.geo {
            (geo.origin_x, geo.origin_y)
        } else if let Some(meta) = &self.meta {
            (meta.origin_x, meta.origin_y)
        } else {
            (options.origin[0], options.origin[1])
        };
        // If the caller did not provide a usable range, fall back to the
        // sidecar (or the observed values) instead of collapsing the terrain.
        let (suggested_min, suggested_max) = self.suggested_range();
        let usable = options.min_height.is_finite()
            && options.max_height.is_finite()
            && (options.max_height - options.min_height).abs() > 1e-6;
        let min = if usable {
            options.min_height
        } else {
            suggested_min
        };
        let max = if usable {
            options.max_height
        } else {
            suggested_max
        };
        let range = (max - min).abs().max(1e-6);
        let mut heights = vec![0.0f32; self.width * self.height];
        let mut mask = if self.meta.as_ref().is_some_and(|meta| meta.nodata.is_some()) {
            Some(vec![true; self.width * self.height])
        } else {
            None
        };
        let mut valid_sum = 0.0f64;
        let mut valid_count = 0usize;
        for y in 0..self.height {
            let src_row = if options.flip_y {
                self.height - 1 - y
            } else {
                y
            };
            for x in 0..self.width {
                let raw = self.values[src_row * self.width + x];
                if !raw.is_finite() {
                    return Err(IoError::Invalid(format!(
                        "heightmap contains a non-finite sample at row {src_row}, column {x}"
                    )));
                }
                let index = y * self.width + x;
                let nodata = self
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.nodata)
                    .or(self.geo.and_then(|geo| geo.nodata));
                if let Some(nodata) = nodata
                    && (raw - nodata).abs() <= f32::EPSILON * nodata.abs().max(1.0)
                {
                    if let Some(mask) = &mut mask {
                        mask[index] = false;
                    }
                    continue;
                }
                let value = if self.normalized {
                    min + raw.clamp(0.0, 1.0) * range
                } else {
                    raw
                };
                heights[index] = value;
                valid_sum += f64::from(value);
                valid_count += 1;
            }
        }
        if let Some(mask) = &mask {
            let fill = if valid_count > 0 {
                (valid_sum / valid_count as f64) as f32
            } else {
                min
            };
            for (index, valid) in mask.iter().enumerate() {
                if !valid {
                    heights[index] = fill;
                }
            }
        }
        let mut hf = HeightField::new(self.width, self.height, spacing_x, spacing_y, heights)?;
        hf.set_origin(origin_x, origin_y);
        hf.set_nodata(
            self.meta
                .as_ref()
                .and_then(|meta| meta.nodata)
                .or(self.geo.and_then(|geo| geo.nodata)),
        );
        if let Some(mask) = mask {
            hf.set_mask(Some(mask))?;
        }
        Ok(hf)
    }
}

/// Read a PNG/JPEG/TIFF heightmap. GeoTIFF is decoded with its georeferencing;
/// other images use the sidecar (if present) for the elevation range.
pub fn read_heightmap(path: &Path) -> Result<HeightmapImport> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "tif" | "tiff") {
        // First try the full GeoTIFF decoder (float bands, geo tags). If the
        // file is a plain TIFF, fall back to the image crate.
        if let Ok(raster) = read_geotiff(path) {
            let (min, max) = min_max(&raster.values);
            return Ok(HeightmapImport {
                width: raster.width,
                height: raster.height,
                values: raster.values,
                normalized: false,
                bit_depth: 32,
                meta: Some(HeightmapMeta {
                    format: HEIGHTMAP_FORMAT.into(),
                    width: raster.width as u32,
                    height: raster.height as u32,
                    min_height: min,
                    max_height: max,
                    spacing_x: raster.geo.spacing_x,
                    spacing_y: raster.geo.spacing_y,
                    origin_x: raster.geo.origin_x,
                    origin_y: raster.geo.origin_y,
                    nodata: raster.geo.nodata,
                    bit_depth: 32,
                }),
                geo: Some(raster.geo),
                source: raster.source,
            });
        }
    }
    let decoded = image::open(path)?;
    let (width, height) = (decoded.width() as usize, decoded.height() as usize);
    let (values, normalized, bit_depth) = image_values(&decoded)?;
    let meta = read_sidecar(path);
    Ok(HeightmapImport {
        width,
        height,
        values,
        normalized,
        bit_depth,
        meta,
        geo: None,
        source: path.display().to_string(),
    })
}

fn image_values(decoded: &DynamicImage) -> Result<(Vec<f32>, bool, u8)> {
    Ok(match decoded {
        DynamicImage::ImageLuma8(image) => (
            image
                .pixels()
                .map(|pixel| f32::from(pixel[0]) / 255.0)
                .collect(),
            true,
            8,
        ),
        DynamicImage::ImageLumaA8(image) => (
            image
                .pixels()
                .map(|pixel| f32::from(pixel[0]) / 255.0)
                .collect(),
            true,
            8,
        ),
        DynamicImage::ImageLuma16(image) => (
            image
                .pixels()
                .map(|pixel| f32::from(pixel[0]) / 65535.0)
                .collect(),
            true,
            16,
        ),
        DynamicImage::ImageLumaA16(image) => (
            image
                .pixels()
                .map(|pixel| f32::from(pixel[0]) / 65535.0)
                .collect(),
            true,
            16,
        ),
        DynamicImage::ImageRgb8(image) => (
            image
                .pixels()
                .map(|pixel| luminance8(pixel[0], pixel[1], pixel[2]))
                .collect(),
            true,
            8,
        ),
        DynamicImage::ImageRgba8(image) => (
            image
                .pixels()
                .map(|pixel| luminance8(pixel[0], pixel[1], pixel[2]))
                .collect(),
            true,
            8,
        ),
        DynamicImage::ImageRgb16(image) => (
            image
                .pixels()
                .map(|pixel| luminance16(pixel[0], pixel[1], pixel[2]))
                .collect(),
            true,
            16,
        ),
        DynamicImage::ImageRgba16(image) => (
            image
                .pixels()
                .map(|pixel| luminance16(pixel[0], pixel[1], pixel[2]))
                .collect(),
            true,
            16,
        ),
        DynamicImage::ImageRgb32F(image) => {
            (image.pixels().map(|pixel| pixel[0]).collect(), false, 32)
        }
        DynamicImage::ImageRgba32F(image) => {
            (image.pixels().map(|pixel| pixel[0]).collect(), false, 32)
        }
        other => {
            let luma = other.to_luma16();
            (
                luma.pixels()
                    .map(|pixel| f32::from(pixel[0]) / 65535.0)
                    .collect(),
                true,
                16,
            )
        }
    })
}

fn luminance8(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

fn luminance16(r: u16, g: u16, b: u16) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 65535.0
}

fn min_max(values: &[f32]) -> (f32, f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for value in values {
        if value.is_finite() {
            min = min.min(*value);
            max = max.max(*value);
        }
    }
    if min.is_finite() && max.is_finite() {
        (min, max)
    } else {
        (0.0, 1.0)
    }
}

fn sidecar_path(path: &Path) -> PathBuf {
    path.with_extension("json")
}

fn alternative_sidecar_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".json");
    PathBuf::from(name)
}

pub fn read_sidecar(path: &Path) -> Option<HeightmapMeta> {
    let candidates = [sidecar_path(path), alternative_sidecar_path(path)];
    for candidate in candidates {
        if let Ok(text) = std::fs::read_to_string(&candidate)
            && let Ok(meta) = serde_json::from_str::<HeightmapMeta>(&text)
            && meta.format == HEIGHTMAP_FORMAT
        {
            return Some(meta);
        }
    }
    None
}

/// Write a 16-bit PNG heightmap plus its JSON sidecar and `.pgw` world file.
/// The elevation range is normalised, so the sidecar is what makes the export
/// re-importable at the correct scale.
pub fn write_heightmap_png(path: &Path, hf: &HeightField) -> Result<()> {
    let (min, max) = min_max(hf.heights());
    let range = (max - min).max(1e-6);
    let width = hf.width() as u32;
    let height = hf.height() as u32;
    let mut samples = Vec::with_capacity(hf.sample_count());
    for y in (0..hf.height()).rev() {
        for x in 0..hf.width() {
            let normalized = if hf.is_valid(x, y) {
                ((hf.get(x, y) - min) / range).clamp(0.0, 1.0)
            } else {
                0.0
            };
            samples.push((normalized * 65535.0 + 0.5) as u16);
        }
    }
    let buffer = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(width, height, samples)
        .ok_or_else(|| IoError::Invalid("invalid heightmap dimensions".into()))?;
    DynamicImage::ImageLuma16(buffer).save_with_format(path, ImageFormat::Png)?;
    let meta = HeightmapMeta {
        format: HEIGHTMAP_FORMAT.into(),
        width,
        height,
        min_height: min,
        max_height: max,
        spacing_x: hf.spacing().0,
        spacing_y: hf.spacing().1,
        origin_x: hf.origin().0,
        origin_y: hf.origin().1,
        nodata: hf.nodata(),
        bit_depth: 16,
    };
    std::fs::write(sidecar_path(path), serde_json::to_vec_pretty(&meta)?)?;
    write_world_file(path, &meta)?;
    Ok(())
}

/// Write an 8-bit JPEG preview. Height range is stored in the sidecar, but the
/// JPEG quantisation makes it unsuitable for precise DEM round-trips.
pub fn write_heightmap_jpeg(path: &Path, hf: &HeightField) -> Result<()> {
    let (min, max) = min_max(hf.heights());
    let range = (max - min).max(1e-6);
    let width = hf.width() as u32;
    let height = hf.height() as u32;
    let mut samples = Vec::with_capacity(hf.sample_count());
    for y in (0..hf.height()).rev() {
        for x in 0..hf.width() {
            let normalized = if hf.is_valid(x, y) {
                ((hf.get(x, y) - min) / range).clamp(0.0, 1.0)
            } else {
                0.0
            };
            samples.push((normalized * 255.0 + 0.5) as u8);
        }
    }
    let buffer = image::GrayImage::from_raw(width, height, samples)
        .ok_or_else(|| IoError::Invalid("invalid heightmap dimensions".into()))?;
    DynamicImage::ImageLuma8(buffer).save_with_format(path, ImageFormat::Jpeg)?;
    let meta = HeightmapMeta {
        format: HEIGHTMAP_FORMAT.into(),
        width,
        height,
        min_height: min,
        max_height: max,
        spacing_x: hf.spacing().0,
        spacing_y: hf.spacing().1,
        origin_x: hf.origin().0,
        origin_y: hf.origin().1,
        nodata: hf.nodata(),
        bit_depth: 8,
    };
    std::fs::write(sidecar_path(path), serde_json::to_vec_pretty(&meta)?)?;
    write_world_file(path, &meta)?;
    Ok(())
}

fn write_world_file(path: &Path, meta: &HeightmapMeta) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let world_extension = match extension.as_str() {
        "png" => "pgw",
        "jpg" | "jpeg" => "jgw",
        "tif" | "tiff" => "tfw",
        _ => return Ok(()),
    };
    let world_path = path.with_extension(world_extension);
    let top_y = meta.origin_y + (f64::from(meta.height) - 1.0) * meta.spacing_y;
    let mut file = BufWriter::new(File::create(world_path)?);
    writeln!(file, "{}", meta.spacing_x)?;
    writeln!(file, "0")?;
    writeln!(file, "0")?;
    writeln!(file, "{}", -meta.spacing_y)?;
    writeln!(file, "{}", meta.origin_x - meta.spacing_x * 0.5)?;
    writeln!(file, "{}", top_y + meta.spacing_y * 0.5)?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_field() -> HeightField {
        let mut heights = Vec::new();
        for y in 0..6 {
            for x in 0..5 {
                heights.push(120.0 + x as f32 * 30.0 + y as f32 * 7.0);
            }
        }
        let mut hf = HeightField::new(5, 6, 25.0, 25.0, heights).unwrap();
        hf.set_origin(500.0, 700.0);
        hf
    }

    fn temp_path(name: &str, extension: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("vf-{name}-{nanos}.{extension}"))
    }

    #[test]
    fn png_round_trip_uses_sidecar_range() {
        let hf = sample_field();
        let path = temp_path("heightmap", "png");
        write_heightmap_png(&path, &hf).unwrap();
        let import = read_heightmap(&path).unwrap();
        assert_eq!((import.width, import.height), (5, 6));
        assert!(import.normalized);
        assert_eq!(import.bit_depth, 16);
        let (min, max) = import.suggested_range();
        let back = import
            .into_heightfield(HeightmapOptions {
                min_height: min,
                max_height: max,
                ..HeightmapOptions::default()
            })
            .unwrap();
        for y in 0..hf.height() {
            for x in 0..hf.width() {
                assert!(
                    (back.get(x, y) - hf.get(x, y)).abs() < 6.0,
                    "({x},{y}): {} vs {}",
                    back.get(x, y),
                    hf.get(x, y)
                );
            }
        }
        let (ox, oy) = back.origin();
        assert!((ox - 500.0).abs() < 1e-6);
        assert!((oy - 700.0).abs() < 1e-6);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
        let _ = std::fs::remove_file(path.with_extension("pgw"));
    }

    #[test]
    fn jpeg_is_accepted_but_lossy() {
        let hf = sample_field();
        let path = temp_path("heightmap", "jpg");
        write_heightmap_jpeg(&path, &hf).unwrap();
        let import = read_heightmap(&path).unwrap();
        assert_eq!((import.width, import.height), (5, 6));
        let (min, max) = import.suggested_range();
        let back = import
            .into_heightfield(HeightmapOptions {
                min_height: min,
                max_height: max,
                ..HeightmapOptions::default()
            })
            .unwrap();
        assert!((back.get(0, 0) - hf.get(0, 0)).abs() < 25.0);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
        let _ = std::fs::remove_file(path.with_extension("jgw"));
    }
}

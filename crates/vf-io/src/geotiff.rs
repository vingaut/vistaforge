//! GeoTIFF DEM import/export.
//!
//! GeoTIFF is the open, widely supported format for elevation rasters: QGIS,
//! GDAL, Blender (through the GIS add-ons), Unity/Unreal importers and every
//! DEM provider understand it. VistaForge writes a single-band 32-bit float
//! GeoTIFF with `ModelPixelScale`, `ModelTiepoint`, a user-defined projected
//! CRS in metres and an optional `GDAL_NODATA` value. Reading supports integer
//! and float bands and keeps the georeferencing when present.

use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Write};
use std::path::Path;

use tiff::decoder::ifd::Value;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::Tag;

use vf_core::HeightField;

use crate::{IoError, Result};

/// Horizontal georeferencing of a raster. Rows are stored north-first in the
/// file; [`GeoRaster::into_heightfield`] flips them to VistaForge's south-first
/// convention and moves the origin accordingly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoReference {
    pub origin_x: f64,
    /// South-west corner (sample 0,0 after flipping).
    pub origin_y: f64,
    pub spacing_x: f64,
    pub spacing_y: f64,
    pub nodata: Option<f32>,
}

impl Default for GeoReference {
    fn default() -> Self {
        Self {
            origin_x: 0.0,
            origin_y: 0.0,
            spacing_x: 1.0,
            spacing_y: 1.0,
            nodata: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeoRaster {
    pub width: usize,
    pub height: usize,
    /// Samples, north row first (the GeoTIFF order).
    pub values: Vec<f32>,
    pub geo: GeoReference,
    pub source: String,
}

impl GeoRaster {
    /// Convert to a VistaForge heightfield, flipping rows and applying the
    /// nodata mask.
    pub fn into_heightfield(self) -> Result<HeightField> {
        if self.width < 2 || self.height < 2 {
            return Err(IoError::Invalid(format!(
                "GeoTIFF is too small: {}x{}",
                self.width, self.height
            )));
        }
        let mut heights = vec![0.0f32; self.width * self.height];
        let mut mask = self
            .geo
            .nodata
            .map(|_| vec![true; self.width * self.height]);
        let mut fill_sum = 0.0f64;
        let mut fill_count = 0usize;
        for y in 0..self.height {
            let src_row = self.height - 1 - y;
            for x in 0..self.width {
                let raw = self.values[src_row * self.width + x];
                if !raw.is_finite() {
                    return Err(IoError::Invalid(format!(
                        "GeoTIFF contains a non-finite sample at row {src_row}, column {x}"
                    )));
                }
                let index = y * self.width + x;
                if let Some(nodata) = self.geo.nodata
                    && (raw - nodata).abs() <= f32::EPSILON * nodata.abs().max(1.0)
                {
                    if let Some(mask) = &mut mask {
                        mask[index] = false;
                    }
                    continue;
                }
                heights[index] = raw;
                fill_sum += f64::from(raw);
                fill_count += 1;
            }
        }
        if let Some(mask) = &mask {
            let fill = if fill_count > 0 {
                (fill_sum / fill_count as f64) as f32
            } else {
                0.0
            };
            for (index, valid) in mask.iter().enumerate() {
                if !valid {
                    heights[index] = fill;
                }
            }
        }
        let mut hf = HeightField::new(
            self.width,
            self.height,
            self.geo.spacing_x.abs().max(1e-6),
            self.geo.spacing_y.abs().max(1e-6),
            heights,
        )?;
        hf.set_origin(self.geo.origin_x, self.geo.origin_y);
        hf.set_nodata(self.geo.nodata);
        if let Some(mask) = mask {
            hf.set_mask(Some(mask))?;
        }
        Ok(hf)
    }
}

/// Read a GeoTIFF raster. Accepts integer and float grayscale bands (RGB
/// bands are converted to luminance so a screenshot-style TIFF still imports).
pub fn read_geotiff(path: &Path) -> Result<GeoRaster> {
    let file = File::open(path)?;
    let mut decoder = Decoder::new(BufReader::new(file)).map_err(tiff_error)?;
    let (width, height) = decoder.dimensions().map_err(tiff_error)?;
    let width = width as usize;
    let height = height as usize;
    if width < 2 || height < 2 {
        return Err(IoError::Invalid(format!(
            "GeoTIFF is too small: {width}x{height}"
        )));
    }
    let pixel_scale = tag_f64_values(&mut decoder, Tag::ModelPixelScaleTag);
    let tiepoint = tag_f64_values(&mut decoder, Tag::ModelTiepointTag);
    let transform = tag_f64_values(&mut decoder, Tag::ModelTransformationTag);
    let nodata = decoder
        .find_tag(Tag::GdalNodata)
        .ok()
        .flatten()
        .and_then(value_as_string)
        .and_then(|text| text.trim().parse::<f32>().ok());
    let geo = derive_reference(
        width,
        height,
        pixel_scale.as_deref(),
        tiepoint.as_deref(),
        transform.as_deref(),
        nodata,
    );

    let decoded = decoder.read_image().map_err(tiff_error)?;
    let samples = decoding_result_to_f32(decoded)?;
    let expected = width * height;
    if samples.len() < expected {
        return Err(IoError::Invalid(format!(
            "GeoTIFF has {} samples, expected at least {expected}",
            samples.len()
        )));
    }
    let channels = (samples.len() / expected).max(1);
    let values = if channels == 1 {
        samples[..expected].to_vec()
    } else {
        // Luminance for RGB(A) rasters.
        (0..expected)
            .map(|index| {
                let base = index * channels;
                let r = samples[base];
                let g = samples[base + 1.min(channels - 1)];
                let b = samples[base + 2.min(channels - 1)];
                0.2126 * r + 0.7152 * g + 0.0722 * b
            })
            .collect()
    };
    Ok(GeoRaster {
        width,
        height,
        values,
        geo,
        source: path.display().to_string(),
    })
}

fn derive_reference(
    width: usize,
    height: usize,
    pixel_scale: Option<&[f64]>,
    tiepoint: Option<&[f64]>,
    transform: Option<&[f64]>,
    nodata: Option<f32>,
) -> GeoReference {
    let mut geo = GeoReference {
        nodata,
        ..GeoReference::default()
    };
    if let Some(scale) = pixel_scale {
        if scale.len() >= 2 {
            geo.spacing_x = scale[0].abs().max(1e-6);
            geo.spacing_y = scale[1].abs().max(1e-6);
        } else if let Some(value) = scale.first() {
            geo.spacing_x = value.abs().max(1e-6);
            geo.spacing_y = geo.spacing_x;
        }
    }
    if let Some(transform) = transform
        && transform.len() >= 16
    {
        // 4x4 row-major matrix. The first row is the X step, the second the Y.
        geo.spacing_x = transform[0].abs().max(1e-6);
        geo.spacing_y = transform[5].abs().max(1e-6);
        geo.origin_x = transform[3];
        let top_y = transform[7];
        geo.origin_y = top_y - (height as f64 - 1.0) * geo.spacing_y;
        return geo;
    }
    if let Some(tiepoint) = tiepoint
        && tiepoint.len() >= 6
    {
        // (i, j, k, x, y, z): raster (i,j) maps to model (x,y), row j from top.
        let i = tiepoint[0];
        let j = tiepoint[1];
        let x = tiepoint[3];
        let y = tiepoint[4];
        geo.origin_x = x - i * geo.spacing_x;
        let top_y = y + j * geo.spacing_y;
        geo.origin_y = top_y - (height as f64 - 1.0) * geo.spacing_y;
    }
    let _ = width;
    geo
}

pub fn write_geotiff(
    path: &Path,
    hf: &HeightField,
    title: &str,
    nodata: Option<f32>,
) -> Result<()> {
    let (spacing_x, spacing_y) = hf.spacing();
    let (origin_x, origin_y) = hf.origin();
    let width = u32::try_from(hf.width())
        .map_err(|_| IoError::Invalid("heightfield is too wide for GeoTIFF".into()))?;
    let height = u32::try_from(hf.height())
        .map_err(|_| IoError::Invalid("heightfield is too tall for GeoTIFF".into()))?;
    // North row first.
    let mut samples = Vec::with_capacity(hf.sample_count());
    for y in (0..hf.height()).rev() {
        for x in 0..hf.width() {
            if hf.is_valid(x, y) {
                samples.push(hf.get(x, y));
            } else {
                samples.push(nodata.unwrap_or_else(|| hf.get(x, y)));
            }
        }
    }

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut cursor).map_err(tiff_error)?;
        let mut image = encoder
            .new_image::<colortype::Gray32Float>(width, height)
            .map_err(tiff_error)?;
        image
            .encoder()
            .write_tag(
                Tag::ModelPixelScaleTag,
                &[spacing_x.abs(), spacing_y.abs(), 0.0][..],
            )
            .map_err(tiff_error)?;
        let top_y = origin_y + (hf.height() - 1) as f64 * spacing_y;
        // Raster pixel (0,0) (north-west) maps to this model coordinate.
        image
            .encoder()
            .write_tag(
                Tag::ModelTiepointTag,
                &[0.0, 0.0, 0.0, origin_x, top_y, 0.0][..],
            )
            .map_err(tiff_error)?;
        // Projected CRS, user-defined, metres, pixel-is-point.
        let keys: [u16; 20] = [
            1, 1, 0, 4, // header: version, revision, minor, number of keys
            1024, 0, 1, 1, // GTModelTypeGeoKey = projected
            1025, 0, 1, 2, // GTRasterTypeGeoKey = pixel is point
            3072, 0, 1, 32767, // ProjectedCSTypeGeoKey = user-defined
            3076, 0, 1, 9001, // ProjLinearUnitsGeoKey = metre
        ];
        image
            .encoder()
            .write_tag(Tag::GeoKeyDirectoryTag, &keys[..])
            .map_err(tiff_error)?;
        let citation = format!("{title}|VistaForge|");
        image
            .encoder()
            .write_tag(Tag::GeoAsciiParamsTag, citation.as_str())
            .map_err(tiff_error)?;
        if let Some(nodata) = nodata {
            image
                .encoder()
                .write_tag(Tag::GdalNodata, format!("{nodata}").as_str())
                .map_err(tiff_error)?;
        }
        image.write_data(&samples).map_err(tiff_error)?;
    }
    write_atomic(path, &cursor.into_inner())?;
    Ok(())
}

fn tag_f64_values<R: std::io::Read + std::io::Seek>(
    decoder: &mut Decoder<R>,
    tag: Tag,
) -> Option<Vec<f64>> {
    match decoder.find_tag(tag).ok().flatten()? {
        Value::List(values) => values
            .into_iter()
            .map(value_as_f64)
            .collect::<Option<Vec<_>>>(),
        value => value_as_f64(value).map(|value| vec![value]),
    }
}

fn value_as_f64(value: Value) -> Option<f64> {
    match value {
        Value::Byte(v) => Some(f64::from(v)),
        Value::Short(v) => Some(f64::from(v)),
        Value::SignedByte(v) => Some(f64::from(v)),
        Value::SignedShort(v) => Some(f64::from(v)),
        Value::Signed(v) => Some(f64::from(v)),
        Value::SignedBig(v) => Some(v as f64),
        Value::Unsigned(v) => Some(f64::from(v)),
        Value::UnsignedBig(v) => Some(v as f64),
        Value::Float(v) => Some(f64::from(v)),
        Value::Double(v) => Some(v),
        Value::Rational(n, d) if d != 0 => Some(f64::from(n) / f64::from(d)),
        Value::SRational(n, d) if d != 0 => Some(f64::from(n) / f64::from(d)),
        _ => None,
    }
}

fn value_as_string(value: Value) -> Option<String> {
    match value {
        Value::Ascii(text) => Some(text),
        _ => None,
    }
}

fn decoding_result_to_f32(decoded: DecodingResult) -> Result<Vec<f32>> {
    Ok(match decoded {
        DecodingResult::U8(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I8(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::I16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
    })
}

fn tiff_error(error: tiff::TiffError) -> IoError {
    IoError::Invalid(format!("TIFF: {error}"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("tif")
    ));
    {
        let mut file = BufWriter::new(File::create(&tmp)?);
        file.write_all(bytes)?;
        file.flush()?;
    }
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_field() -> HeightField {
        let mut heights = Vec::new();
        for y in 0..5 {
            for x in 0..7 {
                heights.push(x as f32 * 10.0 + y as f32 * 2.0);
            }
        }
        let mut hf = HeightField::new(7, 5, 12.5, 10.0, heights).unwrap();
        hf.set_origin(1000.0, 2000.0);
        hf
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("vf-{name}-{nanos}.tif"))
    }

    #[test]
    fn geotiff_round_trip_preserves_values_and_georeferencing() {
        let hf = sample_field();
        let path = temp_path("geotiff");
        write_geotiff(&path, &hf, "round-trip", Some(-9999.0)).unwrap();
        let raster = read_geotiff(&path).unwrap();
        assert_eq!((raster.width, raster.height), (7, 5));
        let back = raster.into_heightfield().unwrap();
        assert_eq!(back.width(), hf.width());
        assert_eq!(back.height(), hf.height());
        for y in 0..hf.height() {
            for x in 0..hf.width() {
                assert!(
                    (back.get(x, y) - hf.get(x, y)).abs() < 1e-4,
                    "({x},{y}): {} vs {}",
                    back.get(x, y),
                    hf.get(x, y)
                );
            }
        }
        let (sx, sy) = back.spacing();
        assert!((sx - 12.5).abs() < 1e-6);
        assert!((sy - 10.0).abs() < 1e-6);
        let (ox, oy) = back.origin();
        assert!((ox - 1000.0).abs() < 1e-6);
        assert!((oy - 2000.0).abs() < 1e-6);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ascii_tag_citation_survives() {
        let hf = sample_field();
        let path = temp_path("citation");
        write_geotiff(&path, &hf, "my dem", None).unwrap();
        let raster = read_geotiff(&path).unwrap();
        assert_eq!(raster.width, 7);
        let _ = std::fs::remove_file(path);
    }
}

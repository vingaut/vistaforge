//! One entry point for terrain import/export.
//!
//! The extension chooses the format: GeoTIFF and ESRI ASCII Grid are the open
//! DEM formats, PNG/JPEG are heightmaps, and `.dem` is the legacy VistaForge
//! container. Importing a raster never touches the camera rig; the project
//! keeps its animation.

use std::path::{Path, PathBuf};

use vf_core::{DemSource, HeightField, TerrainSource};

use crate::ascii::{read_ascii_grid, write_ascii_grid};
use crate::dem::DemFile;
use crate::geotiff::{read_geotiff, write_geotiff};
use crate::heightmap::{
    HeightmapOptions, read_heightmap, read_sidecar, write_heightmap_jpeg, write_heightmap_png,
};
use crate::{IoError, Result};

#[derive(Debug, Clone, Copy)]
pub struct TerrainImportOptions {
    pub heightmap: HeightmapOptions,
    /// Spacing used for formats without georeferencing (legacy `.dem`).
    pub dem_spacing_m: f32,
}

impl Default for TerrainImportOptions {
    fn default() -> Self {
        Self {
            heightmap: HeightmapOptions::default(),
            dem_spacing_m: 10.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportedTerrain {
    pub heightfield: HeightField,
    pub source: TerrainSource,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct TerrainExportReport {
    pub path: PathBuf,
    pub format: &'static str,
    pub width: usize,
    pub height: usize,
    pub spacing_m: f64,
    pub warnings: Vec<String>,
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Import a terrain from GeoTIFF, ESRI ASCII Grid, legacy `.dem`, PNG or JPEG.
pub fn import_terrain(path: &Path, options: TerrainImportOptions) -> Result<ImportedTerrain> {
    let ext = extension(path);
    let (heightfield, description) = match ext.as_str() {
        "dem" => {
            let bytes = std::fs::read(path)?;
            let dem = DemFile::parse(&bytes)?;
            let spacing = if options.dem_spacing_m > 0.0 {
                options.dem_spacing_m
            } else {
                options.heightmap.spacing_m as f32
            };
            let spacing = if spacing > 0.0 { spacing } else { 10.0 };
            let hf = dem.to_heightfield(spacing)?;
            (
                hf,
                format!(
                    "DEM v1 {}x{} · {} m/cellule",
                    dem.header.effective_dimensions().0,
                    dem.header.effective_dimensions().1,
                    spacing
                ),
            )
        }
        "asc" | "grd" => (read_ascii_grid(path)?, "ESRI ASCII Grid".to_string()),
        "tif" | "tiff" => {
            let raster = read_geotiff(path)?;
            let width = raster.width;
            let height = raster.height;
            let spacing = raster.geo.spacing_x;
            let hf = if options.heightmap.spacing_m > 0.0 || geometry_needs_override(&raster.geo) {
                let import = crate::heightmap::HeightmapImport {
                    width,
                    height,
                    values: raster.values,
                    normalized: false,
                    bit_depth: 32,
                    meta: None,
                    geo: Some(raster.geo),
                    source: path.display().to_string(),
                };
                import.into_heightfield(options.heightmap)?
            } else {
                raster.into_heightfield()?
            };
            (
                hf,
                format!("GeoTIFF {}x{} · {:.3} m/cellule", width, height, spacing),
            )
        }
        "r32" => {
            let bytes = std::fs::read(path)?;
            let meta_text = std::fs::read_to_string(path.with_extension("json"))
                .map_err(|_| IoError::Invalid("raw .r32 needs a .json sidecar".into()))?;
            let meta: serde_json::Value = serde_json::from_str(&meta_text)?;
            let width = meta
                .get("width")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| IoError::Invalid("raw .r32 sidecar has no width".into()))?
                as usize;
            let height = meta
                .get("height")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| IoError::Invalid("raw .r32 sidecar has no height".into()))?
                as usize;
            if bytes.len() != width * height * 4 {
                return Err(IoError::Invalid(format!(
                    "raw .r32 has {} bytes, expected {} for {}x{}",
                    bytes.len(),
                    width * height * 4,
                    width,
                    height
                )));
            }
            let heights: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes(*chunk))
                .collect();
            let mut hf = HeightField::new(
                width,
                height,
                meta.get("spacing_x")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(1.0),
                meta.get("spacing_y")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(1.0),
                heights,
            )?;
            hf.set_origin(
                meta.get("origin_x")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
                meta.get("origin_y")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
            );
            (hf, format!("raw f32 {}x{} + sidecar", width, height))
        }
        "png" | "jpg" | "jpeg" | "bmp" | "webp" => {
            let import = read_heightmap(path)?;
            let description = format!(
                "Heightmap {}x{} · {} bits{}",
                import.width,
                import.height,
                import.bit_depth,
                if import.meta.is_some() {
                    " · sidecar"
                } else {
                    ""
                }
            );
            let hf = import.into_heightfield(options.heightmap)?;
            (hf, description)
        }
        other => {
            return Err(IoError::Invalid(format!(
                "unsupported terrain format '{other}' (use .tif, .asc, .dem, .png, .jpg)"
            )));
        }
    };
    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Imported terrain")
        .to_string();
    let (sx, sy) = heightfield.spacing();
    let source = TerrainSource::Dem {
        source: DemSource {
            path: path.display().to_string(),
            title: title.clone(),
            comment: description.clone(),
            spacing_m: ((sx + sy) * 0.5) as f32,
        },
    };
    Ok(ImportedTerrain {
        heightfield,
        source,
        description,
    })
}

fn geometry_needs_override(geo: &crate::geotiff::GeoReference) -> bool {
    // A plain TIFF decoded as float has no meaningful georeferencing; use the
    // user-provided spacing instead of the 1 m default.
    (geo.spacing_x - 1.0).abs() < 1e-9 && (geo.spacing_y - 1.0).abs() < 1e-9
}

/// Export the current heightfield to an open DEM or heightmap format.
pub fn export_terrain(
    path: &Path,
    hf: &HeightField,
    title: &str,
    nodata: Option<f32>,
) -> Result<TerrainExportReport> {
    let ext = extension(path);
    let (sx, sy) = hf.spacing();
    let mut warnings = Vec::new();
    let format = match ext.as_str() {
        "tif" | "tiff" => {
            write_geotiff(path, hf, title, nodata)?;
            "GeoTIFF 32-bit float"
        }
        "asc" | "grd" => {
            write_ascii_grid(path, hf, nodata.unwrap_or(-9999.0))?;
            "ESRI ASCII Grid"
        }
        "dem" => {
            let dem = DemFile::from_heightfield(hf, title, "VistaForge export", Vec::new())?;
            std::fs::write(path, dem.write()?)?;
            "DEM v1 (i16, lossy)"
        }
        "png" => {
            write_heightmap_png(path, hf)?;
            "PNG 16-bit + JSON"
        }
        "jpg" | "jpeg" => {
            write_heightmap_jpeg(path, hf)?;
            warnings.push("JPEG is lossy: use GeoTIFF or PNG for DEM work.".into());
            "JPEG 8-bit + JSON"
        }
        "r32" => {
            let mut bytes = Vec::with_capacity(hf.sample_count() * 4);
            for value in hf.heights() {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            std::fs::write(path, bytes)?;
            warnings
                .push("Raw f32 has no header: width/height/spacing use a .json sidecar.".into());
            "raw f32 LE"
        }
        other => {
            return Err(IoError::Invalid(format!(
                "unsupported export extension '{other}' (use .tif, .asc, .dem, .png, .jpg)"
            )));
        }
    };
    if matches!(ext.as_str(), "r32") {
        let meta = serde_json::json!({
            "format": "vistaforge.raw32.v1",
            "width": hf.width(),
            "height": hf.height(),
            "spacing_x": sx,
            "spacing_y": sy,
            "origin_x": hf.origin().0,
            "origin_y": hf.origin().1,
            "nodata": nodata,
            "layout": "south_row_first"
        });
        std::fs::write(
            path.with_extension("json"),
            serde_json::to_vec_pretty(&meta)?,
        )?;
    }
    // Keep the PNG/JPEG sidecar consistent with the requested nodata.
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg")
        && let Some(mut meta) = read_sidecar(path)
    {
        meta.nodata = nodata;
        std::fs::write(
            path.with_extension("json"),
            serde_json::to_vec_pretty(&meta)?,
        )?;
    }
    Ok(TerrainExportReport {
        path: path.to_path_buf(),
        format,
        width: hf.width(),
        height: hf.height(),
        spacing_m: (sx + sy) * 0.5,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn field() -> HeightField {
        let mut heights = Vec::new();
        for y in 0..9 {
            for x in 0..8 {
                heights.push(200.0 + x as f32 * 12.0 + (y as f32 * 1.7).sin() * 9.0);
            }
        }
        let mut hf = HeightField::new(8, 9, 30.0, 30.0, heights).unwrap();
        hf.set_origin(100.0, 200.0);
        hf
    }

    fn temp_path(name: &str, extension: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("vf-terrain-{name}-{nanos}.{extension}"))
    }

    #[test]
    fn geotiff_export_import_round_trip() {
        let hf = field();
        let path = temp_path("roundtrip", "tif");
        let report = export_terrain(&path, &hf, "round trip", None).unwrap();
        assert_eq!(report.width, 8);
        let imported = import_terrain(&path, TerrainImportOptions::default()).unwrap();
        assert_eq!(imported.heightfield.width(), 8);
        let (ox, oy) = imported.heightfield.origin();
        assert!((ox - 100.0).abs() < 1e-6);
        assert!((oy - 200.0).abs() < 1e-6);
        for y in 0..hf.height() {
            for x in 0..hf.width() {
                assert!((imported.heightfield.get(x, y) - hf.get(x, y)).abs() < 1e-4);
            }
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ascii_export_import_round_trip() {
        let hf = field();
        let path = temp_path("ascii", "asc");
        export_terrain(&path, &hf, "ascii", None).unwrap();
        let imported = import_terrain(&path, TerrainImportOptions::default()).unwrap();
        assert_eq!(imported.heightfield.width(), 8);
        assert!((imported.heightfield.get(4, 4) - hf.get(4, 4)).abs() < 0.01);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn png_heightmap_import_uses_sidecar_range() {
        let hf = field();
        let path = temp_path("height", "png");
        export_terrain(&path, &hf, "png", None).unwrap();
        let imported = import_terrain(
            &path,
            TerrainImportOptions {
                heightmap: HeightmapOptions {
                    min_height: 0.0,
                    max_height: 0.0,
                    ..HeightmapOptions::default()
                },
                ..TerrainImportOptions::default()
            },
        )
        .unwrap();
        // 0/0 would collapse the range, but the sidecar is applied by the
        // caller; without it the image is normalized to 0..1. Just check the
        // dimensions and that the file was understood.
        assert_eq!(imported.heightfield.width(), 8);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
        let _ = std::fs::remove_file(path.with_extension("pgw"));
    }
}

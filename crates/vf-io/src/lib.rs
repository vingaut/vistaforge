//! File formats used by VistaForge.
//!
//! * [`dem`] — legacy DEM v1 reader/writer.
//! * [`project`] — `.vfproj` container (ZIP + RON manifest + raw heightfield).
//! * [`image_io`] — PNG output helpers.
//! * [`ascii`] — tolerant ESRI ASCII Grid import/export.

pub mod ascii;
pub mod dem;
pub mod geotiff;
pub mod heightmap;
pub mod image_io;
pub mod project;
pub mod terrain_io;

use thiserror::Error;

pub use dem::{DemError, DemFile, DemHeader, HEADER_SIZE, SIGNATURE};
pub use geotiff::{GeoRaster, GeoReference, read_geotiff, write_geotiff};
pub use heightmap::{
    HEIGHTMAP_FORMAT, HeightmapImport, HeightmapMeta, HeightmapOptions, read_heightmap,
    read_sidecar, write_heightmap_jpeg, write_heightmap_png,
};
pub use image_io::RgbaImage;
pub use project::{LoadedProject, load_project, save_project};
pub use terrain_io::{
    ImportedTerrain, TerrainExportReport, TerrainImportOptions, export_terrain, import_terrain,
};

#[derive(Debug, Error)]
pub enum IoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("RON error: {0}")]
    Ron(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("DEM error: {0}")]
    Dem(#[from] DemError),
    #[error("core error: {0}")]
    Core(#[from] vf_core::Error),
    #[error("heightfield error: {0}")]
    HeightField(#[from] vf_core::HeightFieldError),
}

pub type Result<T> = std::result::Result<T, IoError>;

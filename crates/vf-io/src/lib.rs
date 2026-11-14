//! File formats used by VistaForge.
//!
//! * [`dem`] — legacy DEM v1 reader/writer.
//! * [`project`] — `.vfproj` container (ZIP + RON manifest + raw heightfield).
//! * [`image_io`] — PNG output helpers.
//! * [`ascii`] — tolerant ESRI ASCII Grid import/export.

pub mod ascii;
pub mod dem;
pub mod image_io;
pub mod project;

use thiserror::Error;

pub use dem::{DemError, DemFile, DemHeader, HEADER_SIZE, SIGNATURE};
pub use image_io::RgbaImage;
pub use project::{LoadedProject, load_project, save_project};

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
    #[error("DEM error: {0}")]
    Dem(#[from] DemError),
    #[error("core error: {0}")]
    Core(#[from] vf_core::Error),
    #[error("heightfield error: {0}")]
    HeightField(#[from] vf_core::HeightFieldError),
}

pub type Result<T> = std::result::Result<T, IoError>;

//! Deterministic procedural terrain generation for VistaForge.
//!
//! * [`classic`] implements the classic midpoint-displacement style.
//! * [`modern`] combines fBm, ridged multifractal, domain warp, continent
//!   masks, terraces and craters.
//! * [`erosion`] applies hydraulic droplets and thermal talus relaxation.
//! * [`noise`] exposes the deterministic noise used by the renderer too.

pub mod classic;
pub mod cliff;
pub mod erosion;
pub mod modern;
pub mod noise;

pub use noise::{Noise, value2};

/// Generated fractal landscapes span roughly 10 km across.
pub const DEFAULT_WORLD_EXTENT_M: f64 = 10_000.0;

use vf_core::{ErosionParams, HeightField, Result, Seed, TerrainSource};

/// Generate the terrain described by a project source recipe.
pub fn generate(
    source: &TerrainSource,
    erosion: Option<&ErosionParams>,
    seed: Seed,
) -> Result<HeightField> {
    let mut hf = match source {
        TerrainSource::Classic { params } => classic::generate(params)?,
        TerrainSource::Modern { params } => modern::generate(params)?,
        // Neutral base: a constant altitude, no noise at all. The user adds
        // relief on purpose (generation preset, brush, modifier, erosion).
        TerrainSource::Flat { size, height_m } => {
            let size = (*size).clamp(2, 16_384) as usize;
            let spacing = DEFAULT_WORLD_EXTENT_M / (size - 1) as f64;
            HeightField::new(size, size, spacing, spacing, vec![*height_m; size * size])?
        }
        TerrainSource::Dem { .. } => {
            return Err(vf_core::Error::Unsupported(
                "DEM sources must be imported through vf-io".into(),
            ));
        }
    };
    if let Some(params) = erosion {
        hf = erosion::erode(&hf, params, seed.derive("erosion"))?;
    }
    Ok(hf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{ClassicParams, ErosionParams};

    #[test]
    fn generate_dispatches_and_erodes() {
        let source = TerrainSource::Classic {
            params: ClassicParams::default().with_size_seed(129, 8),
        };
        let params = ErosionParams {
            droplets: 500,
            thermal_iterations: 2,
            ..ErosionParams::default()
        };
        let plain = generate(&source, None, Seed::new(8)).unwrap();
        let eroded = generate(&source, Some(&params), Seed::new(8)).unwrap();
        assert_ne!(plain.heights(), eroded.heights());
        assert_eq!(eroded.width(), 129);
    }

    /// The neutral source is what a new project starts from: a constant height,
    /// no noise, no erosion effect.
    #[test]
    fn a_flat_source_is_a_constant_heightfield() {
        let source = TerrainSource::flat(65, 12.5);
        let plain = generate(&source, None, Seed::new(3)).unwrap();
        assert_eq!((plain.width(), plain.height()), (65, 65));
        assert!(plain.heights().iter().all(|height| *height == 12.5));
        let eroded = generate(&source, Some(&ErosionParams::default()), Seed::new(3)).unwrap();
        assert_eq!(
            eroded.heights(),
            plain.heights(),
            "eroding a flat surface must not invent relief"
        );
    }
}

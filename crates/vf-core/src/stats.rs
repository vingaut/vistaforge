use serde::{Deserialize, Serialize};

/// Aggregated statistics for a heightfield, in metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TerrainStats {
    pub width: usize,
    pub height: usize,
    pub valid_cells: usize,
    pub nodata_cells: usize,
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    pub std_dev: f32,
}

impl TerrainStats {
    pub fn relief(&self) -> f32 {
        self.max - self.min
    }
}

//! World composition: single terrain, tiled instances or a seamless tile set.
//!
//! A tiled world is a *composition* of the stored tile, not a second copy of
//! the data. `vf-terrain::world` builds the composed heightfield; the renderer
//! and the exporter then see one ordinary grid, which keeps the pipeline
//! simple and makes every existing tool (erosion, brushes, export) work.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldMode {
    #[default]
    Single,
    Tiled,
    Seamless,
}

impl WorldMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Tiled => "tiled",
            Self::Seamless => "seamless",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "single" => Some(Self::Single),
            "tiled" | "tile" => Some(Self::Tiled),
            "seamless" => Some(Self::Seamless),
            _ => None,
        }
    }
}

/// Per-instance variation. Height offsets are disabled on seamless worlds
/// because they would break the edge match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TileVariation {
    /// Use flips (and rotations on square tiles) per instance.
    pub flip_and_rotate: bool,
    /// Small vertical offset per instance, metres (tiled mode only).
    pub height_offset_m: f32,
    /// Scale the tile around its centre, 1 = unchanged (tiled mode only).
    pub scale: f32,
}

impl Default for TileVariation {
    fn default() -> Self {
        Self {
            flip_and_rotate: true,
            height_offset_m: 0.0,
            scale: 1.0,
        }
    }
}

/// World size and tiling recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldSettings {
    pub mode: WorldMode,
    pub tiles_x: u32,
    pub tiles_y: u32,
    /// Edge correction width for the seamless mode, in samples.
    pub blend_cells: u32,
    /// Smooth world-scale height variation, metres. It is sampled in world
    /// coordinates, so it is continuous across tile boundaries and breaks the
    /// "same mountain repeated nine times" look.
    pub macro_variation_m: f32,
    pub macro_scale_m: f32,
    pub variation: TileVariation,
}

impl Default for WorldSettings {
    fn default() -> Self {
        Self {
            mode: WorldMode::Single,
            tiles_x: 2,
            tiles_y: 2,
            blend_cells: 48,
            macro_variation_m: 0.0,
            macro_scale_m: 2400.0,
            variation: TileVariation::default(),
        }
    }
}

impl WorldSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.tiles_x) || !(1..=16).contains(&self.tiles_y) {
            return Err("world tiles must be in 1..=16 on each axis".into());
        }
        if !(2..=4096).contains(&self.blend_cells) {
            return Err("world blend_cells must be in 2..=4096".into());
        }
        if !self.macro_variation_m.is_finite() || !(0.0..=2000.0).contains(&self.macro_variation_m)
        {
            return Err("world macro variation must be in 0..=2000 metres".into());
        }
        if !self.macro_scale_m.is_finite() || !(50.0..=100_000.0).contains(&self.macro_scale_m) {
            return Err("world macro scale must be in 50..=100000 metres".into());
        }
        if !self.variation.height_offset_m.is_finite()
            || self.variation.height_offset_m.abs() > 5000.0
        {
            return Err("world height offset is out of range".into());
        }
        if !self.variation.scale.is_finite() || !(0.1..=10.0).contains(&self.variation.scale) {
            return Err("world tile scale must be in 0.1..=10".into());
        }
        Ok(())
    }

    pub fn output_size(&self, width: usize, height: usize) -> (usize, usize) {
        let tw = (width - 1) * self.tiles_x as usize + 1;
        let th = (height - 1) * self.tiles_y as usize + 1;
        (tw, th)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        WorldSettings::default().validate().unwrap();
        assert_eq!(WorldMode::parse("seamless"), Some(WorldMode::Seamless));
        assert_eq!(WorldMode::parse("nope"), None);
    }

    #[test]
    fn output_size_shares_edges() {
        let settings = WorldSettings {
            tiles_x: 3,
            tiles_y: 2,
            ..WorldSettings::default()
        };
        assert_eq!(settings.output_size(101, 51), (301, 101));
    }
}

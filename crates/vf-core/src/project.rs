use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::detail::DetailSettings;
use crate::flight::FlightSettings;
use crate::geology::GeologySettings;
use crate::heightfield::HeightField;
use crate::ids::ProjectId;
use crate::modifiers::ModifierStack;
use crate::rig::CameraRig;
use crate::scene::Materials;
use crate::scene::{
    Atmosphere, Camera, CloudSettings, Palette, RenderSettings, Sun, TerrainStyle, WaterSettings,
};
use crate::seed::Seed;
use crate::terrain_params::{ClassicParams, DemSource, ErosionParams, ModernParams, TerrainSource};
use crate::world::WorldSettings;

pub const SCHEMA_VERSION: u32 = 6;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectMeta {
    pub name: String,
    pub author: String,
    pub description: String,
    pub created_unix: u64,
    pub modified_unix: u64,
    pub app_version: String,
    pub crs: Option<String>,
    /// Always "meters" in V1.
    pub units: String,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        let now = now_unix();
        Self {
            name: "Untitled landscape".into(),
            author: String::new(),
            description: String::new(),
            created_unix: now,
            modified_unix: now,
            app_version: APP_VERSION.into(),
            crs: None,
            units: "meters".into(),
        }
    }
}

/// Size and georeferencing of the heightfield stored alongside the manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainMeta {
    pub width: usize,
    pub height: usize,
    pub spacing_x: f64,
    pub spacing_y: f64,
    pub origin: [f64; 2],
    pub nodata: Option<f32>,
    pub has_mask: bool,
    pub generator_version: u32,
}

impl Default for TerrainMeta {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            spacing_x: 30.0,
            spacing_y: 30.0,
            origin: [0.0, 0.0],
            nodata: None,
            has_mask: false,
            generator_version: 1,
        }
    }
}

impl TerrainMeta {
    pub fn from_heightfield(hf: &HeightField) -> Self {
        let (spacing_x, spacing_y) = hf.spacing();
        let (origin_x, origin_y) = hf.origin();
        Self {
            width: hf.width(),
            height: hf.height(),
            spacing_x,
            spacing_y,
            origin: [origin_x, origin_y],
            nodata: hf.nodata(),
            has_mask: hf.mask().is_some(),
            generator_version: 1,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.width < 2 || self.height < 2 {
            return Err("project has no terrain (generate or import one first)".into());
        }
        if self.width > 16_384 || self.height > 16_384 {
            return Err("terrain dimensions exceed 16384".into());
        }
        if !self.spacing_x.is_finite()
            || !self.spacing_y.is_finite()
            || self.spacing_x <= 0.0
            || self.spacing_y <= 0.0
        {
            return Err("terrain spacing must be finite and positive".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LegacyMeta {
    pub dem_title: String,
    pub dem_comment: String,
    pub palette_bgr: bool,
    pub session_flags: u32,
}

/// Everything needed to reproduce, edit and render a landscape.
/// The heightfield itself is stored as a binary asset in `.vfproj`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub schema_version: u32,
    pub id: ProjectId,
    pub meta: ProjectMeta,
    pub seed: Seed,
    pub source: TerrainSource,
    pub terrain: TerrainMeta,
    pub camera: Camera,
    pub sun: Sun,
    pub palette: Palette,
    pub style: TerrainStyle,
    pub geology: GeologySettings,
    /// Continuous rock detail evaluated above the stored heightfield.
    pub detail: DetailSettings,
    /// Live modifier stack applied at render time, after the base pipeline.
    pub modifiers: ModifierStack,
    pub materials: Materials,
    pub flight: FlightSettings,
    /// Authored camera rig (LightWave-style eye/target/optics).
    pub rig: CameraRig,
    /// World composition: single grid, tiled instances or seamless tiles.
    pub world: WorldSettings,
    pub erosion: ErosionParams,
    pub erosion_enabled: bool,
    pub atmosphere: Atmosphere,
    pub water: WaterSettings,
    pub clouds: CloudSettings,
    pub render: RenderSettings,
    pub legacy: Option<LegacyMeta>,
}

impl Default for Project {
    fn default() -> Self {
        Self::new("Untitled landscape", Seed::new(1), 1026)
    }
}

impl Project {
    pub fn new(name: impl Into<String>, seed: Seed, size: u32) -> Self {
        let now = now_unix();
        let meta = ProjectMeta {
            name: name.into(),
            ..ProjectMeta::default()
        };
        let source = TerrainSource::Classic {
            params: ClassicParams::default().with_size_seed(size, seed.0),
        };
        Self {
            schema_version: SCHEMA_VERSION,
            id: ProjectId::generate(seed.0, now),
            meta,
            seed,
            source,
            terrain: TerrainMeta::default(),
            camera: Camera::default(),
            sun: Sun::default(),
            palette: Palette::alpine(),
            style: TerrainStyle::default(),
            geology: GeologySettings::default(),
            detail: DetailSettings::default(),
            modifiers: ModifierStack::default(),
            materials: Materials::default(),
            flight: FlightSettings::default(),
            rig: CameraRig::default(),
            world: WorldSettings::default(),
            erosion: ErosionParams::default(),
            erosion_enabled: false,
            atmosphere: Atmosphere::default(),
            water: WaterSettings::default(),
            clouds: CloudSettings::default(),
            render: RenderSettings::default(),
            legacy: None,
        }
    }

    pub fn with_classic(mut self, params: ClassicParams) -> Self {
        self.seed = Seed::new(params.seed);
        self.source = TerrainSource::Classic { params };
        self
    }

    pub fn with_modern(mut self, params: ModernParams) -> Self {
        self.seed = Seed::new(params.seed);
        if let Some(palette) = Palette::preset(&params.preset) {
            self.palette = palette;
        }
        self.source = TerrainSource::Modern { params };
        self
    }

    pub fn with_dem(mut self, source: DemSource) -> Self {
        self.source = TerrainSource::Dem { source };
        self
    }

    /// Update metadata and automatic altitude thresholds after a terrain
    /// change. Explicit user thresholds are re-derived; callers that want to
    /// preserve custom values should restore them afterwards.
    pub fn sync_terrain(&mut self, hf: &HeightField) {
        self.terrain = TerrainMeta::from_heightfield(hf);
        self.auto_style(hf);
        self.mark_modified();
    }

    pub fn auto_style(&mut self, hf: &HeightField) {
        let stats = hf.stats();
        let relief = (stats.max - stats.min).max(1.0);
        let arid = matches!(
            self.palette.name.as_str(),
            "desert" | "canyon" | "lunar" | "volcanic" | "dusk"
        );
        self.style.snow_line_m = if arid {
            stats.max + 500.0
        } else if self.palette.name == "patagonia" {
            // Patagonian towers are mostly bare rock with snow patches.
            stats.min + relief * 0.92
        } else {
            stats.min + relief * 0.80
        };
        self.style.tree_line_m = stats.min + relief * 0.52;
        if arid {
            // A flat sea plane across a desert reads as a bug, not a choice.
            self.water.enabled = false;
            // Dust, not fog: keep the distant plain readable.
            self.atmosphere.haze_density = self.atmosphere.haze_density.min(0.000_004);
        }
        self.style.beach_height_m = stats.min + relief * 0.012;
        if self.water.enabled {
            // Keep the sea just above the lowest basin unless the user moved it.
            self.water.sea_level = stats.min + relief * 0.008;
        }
    }

    /// Apply a named lighting/mood preset without regenerating the terrain.
    pub fn apply_scene_preset(&mut self, name: &str) -> Result<(), String> {
        match name.to_ascii_lowercase().as_str() {
            "alpine" => {
                self.palette = Palette::alpine();
                self.sun.azimuth_deg_from_south = 315.0;
                self.sun.declination_deg = 35.0;
                self.clouds.coverage = 0.42;
            }
            "sunset" => {
                self.palette = Palette::alpine();
                self.palette.sun = crate::Rgb::from_srgb_u8(255, 205, 150);
                self.sun.azimuth_deg_from_south = 270.0;
                self.sun.declination_deg = 6.0;
                self.sun.color_k = 2600.0;
                self.clouds.coverage = 0.5;
                self.atmosphere.haze_density = 0.000_12;
            }
            "desert" => {
                self.palette = Palette::desert();
                self.sun.azimuth_deg_from_south = 340.0;
                self.sun.declination_deg = 55.0;
                self.clouds.enabled = false;
                self.water.enabled = false;
            }
            "volcanic" => {
                self.palette = Palette::volcanic();
                self.sun.azimuth_deg_from_south = 300.0;
                self.sun.declination_deg = 12.0;
                self.sun.color_k = 3200.0;
                self.clouds.coverage = 0.6;
                self.atmosphere.haze_density = 0.000_15;
            }
            "lunar" => {
                self.palette = Palette::lunar();
                self.sun.azimuth_deg_from_south = 315.0;
                self.sun.declination_deg = 20.0;
                self.sun.color_k = 6500.0;
                self.clouds.enabled = false;
                self.water.enabled = false;
                self.atmosphere.enabled = false;
            }
            "dusk" => {
                self.palette = Palette::dusk();
                self.sun.azimuth_deg_from_south = 285.0;
                self.sun.declination_deg = 4.0;
                self.sun.intensity = 0.9;
                self.sun.color_k = 2850.0;
                self.atmosphere.stars = 0.85;
                self.atmosphere.haze_density = 0.000_035;
                self.atmosphere.ambient_strength = 0.5;
                self.render.exposure = 1.15;
                self.clouds.coverage = 0.25;
                self.clouds.enabled = false;
            }
            "coastal" => {
                self.palette = Palette::coastal();
                self.sun.azimuth_deg_from_south = 330.0;
                self.sun.declination_deg = 40.0;
                self.clouds.coverage = 0.3;
            }
            other => return Err(format!("unknown scene preset '{other}'")),
        }
        self.mark_modified();
        Ok(())
    }

    pub fn mark_modified(&mut self) {
        self.meta.modified_unix = now_unix();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported project schema {} (expected {})",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        if self.schema_version == 0 {
            return Err("invalid schema version".into());
        }
        self.terrain.validate()?;
        self.source.validate()?;
        self.camera.validate()?;
        self.sun.validate()?;
        self.palette.validate()?;
        self.style.validate()?;
        self.geology.validate()?;
        self.modifiers.validate()?;
        self.materials.validate()?;
        self.flight.validate()?;
        self.erosion.validate()?;
        self.atmosphere.validate()?;
        self.water.validate()?;
        self.clouds.validate()?;
        self.render.validate()?;
        Ok(())
    }

    /// Description shown by `project info` and MCP.
    pub fn summary(&self) -> String {
        format!(
            "{} [{}] {}x{} seed={} source={}",
            self.meta.name,
            self.id,
            self.terrain.width,
            self.terrain.height,
            self.seed,
            self.source.kind()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_defaults_are_consistent() {
        let p = Project::new("Test", Seed::new(7), 258);
        assert_eq!(p.schema_version, SCHEMA_VERSION);
        assert_eq!(p.seed, Seed::new(7));
        assert_eq!(p.source.kind(), "classic");
        p.validate().unwrap_err(); // no terrain yet
    }

    #[test]
    fn scene_presets_apply() {
        let mut p = Project::new("Test", Seed::new(7), 258);
        p.apply_scene_preset("sunset").unwrap();
        assert_eq!(p.sun.color_k, 2600.0);
        assert!(p.apply_scene_preset("nope").is_err());
    }
}

use serde::{Deserialize, Serialize};

/// Parameters of the legacy-inspired midpoint-displacement generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassicParams {
    pub seed: u64,
    /// Output grid size (both dimensions), typically 258, 514, 1026 or 2050.
    pub size: u32,
    /// Classic fractal dimension: 0..200, default 100. Higher is rougher.
    pub fractal_dimension: f32,
    /// 1, 2, 4 or 8. Larger starts the subdivision at a finer step.
    pub divisor: u8,
    /// Island mode forces the four borders to the same altitude.
    pub island: bool,
    /// Global amplitude multiplier.
    pub roughness: f32,
    /// Peak-to-peak altitude in metres before erosion.
    pub vertical_scale: f32,
}

impl Default for ClassicParams {
    fn default() -> Self {
        Self {
            seed: 1,
            size: 1026,
            fractal_dimension: 100.0,
            divisor: 1,
            island: false,
            roughness: 1.0,
            vertical_scale: 1400.0,
        }
    }
}

impl ClassicParams {
    pub fn with_size_seed(mut self, size: u32, seed: u64) -> Self {
        self.size = size;
        self.seed = seed;
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(2..=8192).contains(&self.size) {
            return Err(format!("size must be in 2..=8192, got {}", self.size));
        }
        if !(0.0..=200.0).contains(&self.fractal_dimension) {
            return Err(format!(
                "fractal_dimension must be in 0..=200, got {}",
                self.fractal_dimension
            ));
        }
        if !matches!(self.divisor, 1 | 2 | 4 | 8) {
            return Err(format!(
                "divisor must be 1, 2, 4 or 8, got {}",
                self.divisor
            ));
        }
        if !self.roughness.is_finite() || self.roughness <= 0.0 {
            return Err("roughness must be finite and positive".into());
        }
        if !self.vertical_scale.is_finite() || self.vertical_scale < 0.0 {
            return Err("vertical_scale must be finite and non-negative".into());
        }
        Ok(())
    }
}

/// Parameters of the modern fBm/ridged generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModernParams {
    pub seed: u64,
    pub size: u32,
    /// Preset name: alpine, desert, volcanic, coastal, canyon, lunar, island, custom.
    pub preset: String,
    pub octaves: u32,
    pub frequency: f32,
    pub lacunarity: f32,
    pub gain: f32,
    /// Blend of the ridged multifractal component, 0..1.
    pub ridge_strength: f32,
    pub warp_strength: f32,
    pub warp_frequency: f32,
    /// Radial continent mask strength, 0..1.
    pub continent: f32,
    /// Peak-to-peak altitude in metres before erosion.
    pub vertical_scale: f32,
    /// 0 disables terracing, otherwise the number of steps.
    pub terrace_steps: u32,
    /// 0..1 crater field strength.
    pub crater: f32,
    /// 0..1 edge falloff width. Terrain always returns to its base altitude at
    /// the grid border; larger values make the taper start further inward.
    pub border_falloff: f32,
}

impl Default for ModernParams {
    fn default() -> Self {
        Self {
            seed: 1,
            size: 1026,
            preset: "alpine".into(),
            octaves: 10,
            frequency: 1.6,
            lacunarity: 2.0,
            gain: 0.5,
            ridge_strength: 0.28,
            warp_strength: 0.8,
            warp_frequency: 1.4,
            continent: 0.15,
            vertical_scale: 1700.0,
            terrace_steps: 0,
            crater: 0.0,
            border_falloff: 0.55,
        }
    }
}

impl ModernParams {
    pub fn with_size_seed(mut self, size: u32, seed: u64) -> Self {
        self.size = size;
        self.seed = seed;
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(2..=8192).contains(&self.size) {
            return Err(format!("size must be in 2..=8192, got {}", self.size));
        }
        if !(1..=24).contains(&self.octaves) {
            return Err(format!("octaves must be in 1..=24, got {}", self.octaves));
        }
        if !self.frequency.is_finite() || self.frequency <= 0.0 {
            return Err("frequency must be finite and positive".into());
        }
        if !self.lacunarity.is_finite() || self.lacunarity <= 1.0 {
            return Err("lacunarity must be greater than 1".into());
        }
        if !(0.0..=1.0).contains(&self.gain) {
            return Err("gain must be in 0..=1".into());
        }
        if !(0.0..=1.0).contains(&self.ridge_strength) {
            return Err("ridge_strength must be in 0..=1".into());
        }
        if !(0.0..=1.0).contains(&self.border_falloff) {
            return Err("border_falloff must be in 0..=1".into());
        }
        for (name, value) in [
            ("warp_strength", self.warp_strength),
            ("warp_frequency", self.warp_frequency),
            ("continent", self.continent),
            ("vertical_scale", self.vertical_scale),
            ("crater", self.crater),
            ("border_falloff", self.border_falloff),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("{name} must be finite and non-negative"));
            }
        }
        Ok(())
    }
}

/// Build a modern preset by name. Unknown names yield `None`.
pub fn modern_preset(name: &str) -> Option<ModernParams> {
    let mut p = ModernParams {
        preset: name.to_ascii_lowercase(),
        ..ModernParams::default()
    };
    match name.to_ascii_lowercase().as_str() {
        "alpine" => {}
        "desert" => {
            p.octaves = 8;
            p.frequency = 1.8;
            p.gain = 0.48;
            p.ridge_strength = 0.25;
            p.warp_strength = 1.6;
            p.continent = 0.10;
            p.vertical_scale = 900.0;
            p.terrace_steps = 6;
            p.border_falloff = 0.0;
        }
        "volcanic" => {
            p.octaves = 9;
            p.frequency = 2.0;
            p.gain = 0.52;
            p.ridge_strength = 0.75;
            p.warp_strength = 1.8;
            p.continent = 0.20;
            p.vertical_scale = 2400.0;
            p.crater = 0.8;
            p.border_falloff = 0.35;
        }
        "coastal" => {
            p.octaves = 9;
            p.frequency = 2.6;
            p.gain = 0.5;
            p.ridge_strength = 0.35;
            p.warp_strength = 0.8;
            p.continent = 0.60;
            p.vertical_scale = 800.0;
            p.border_falloff = 0.8;
        }
        "canyon" => {
            p.octaves = 9;
            p.frequency = 1.6;
            p.gain = 0.5;
            p.ridge_strength = 0.5;
            p.warp_strength = 1.3;
            p.continent = 0.05;
            p.vertical_scale = 1300.0;
            p.terrace_steps = 10;
            p.border_falloff = 0.0;
        }
        "lunar" => {
            p.octaves = 8;
            p.frequency = 3.0;
            p.gain = 0.55;
            p.ridge_strength = 0.15;
            p.warp_strength = 0.4;
            p.continent = 0.0;
            p.vertical_scale = 1600.0;
            p.crater = 1.0;
            p.border_falloff = 0.15;
        }
        "island" => {
            p.octaves = 10;
            p.frequency = 2.4;
            p.gain = 0.5;
            p.ridge_strength = 0.4;
            p.warp_strength = 0.9;
            p.continent = 0.9;
            p.vertical_scale = 1200.0;
            p.border_falloff = 0.9;
        }
        "custom" => {}
        _ => return None,
    }
    Some(p)
}

pub const MODERN_PRESETS: &[&str] = &[
    "alpine", "desert", "volcanic", "coastal", "canyon", "lunar", "island",
];

/// Hydraulic and thermal erosion settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErosionParams {
    pub hydraulic: bool,
    pub droplets: u32,
    pub erosion_rate: f32,
    pub deposition_rate: f32,
    pub max_lifetime: u32,
    pub inertia: f32,
    pub capacity: f32,
    pub evaporation: f32,
    pub gravity: f32,
    pub thermal: bool,
    pub talus_angle_deg: f32,
    pub thermal_iterations: u32,
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            hydraulic: true,
            droplets: 50_000,
            erosion_rate: 0.06,
            deposition_rate: 0.12,
            max_lifetime: 64,
            inertia: 0.05,
            capacity: 1.2,
            evaporation: 0.015,
            gravity: 4.0,
            thermal: true,
            talus_angle_deg: 38.0,
            thermal_iterations: 12,
        }
    }
}

impl ErosionParams {
    pub fn scaled_for(size: u32) -> Self {
        Self {
            // ~0.05 droplets per cell, clamped for quick previews and large DEMs.
            droplets: ((u64::from(size) * u64::from(size)) / 40).clamp(4_000, 300_000) as u32,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.droplets > 10_000_000 {
            return Err("droplets is unreasonably large (max 10,000,000)".into());
        }
        if self.max_lifetime == 0 || self.max_lifetime > 4096 {
            return Err("max_lifetime must be in 1..=4096".into());
        }
        if !(0.0..=1.0).contains(&self.inertia) {
            return Err("inertia must be in 0..=1".into());
        }
        if !(0.0..=1.0).contains(&self.evaporation) {
            return Err("evaporation must be in 0..=1".into());
        }
        if !self.talus_angle_deg.is_finite() || !(0.0..90.0).contains(&self.talus_angle_deg) {
            return Err("talus_angle_deg must be in 0..90".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DemSource {
    /// Original path, for provenance only. Terrain data is stored in the project.
    pub path: String,
    pub title: String,
    pub comment: String,
    pub spacing_m: f32,
}

/// How the heightfield in a project was produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerrainSource {
    Classic {
        params: ClassicParams,
    },
    Modern {
        params: ModernParams,
    },
    /// Neutral starting surface: a constant altitude over the whole extent.
    ///
    /// A new project uses this so the heightmap starts with **no** procedural
    /// noise, no geology and no erosion: every feature is then added on purpose
    /// with the heightmap tools. `size` follows the same convention as
    /// [`ClassicParams::size`].
    Flat {
        size: u32,
        #[serde(default)]
        height_m: f32,
    },
    Dem {
        source: DemSource,
    },
}

impl TerrainSource {
    /// Neutral flat surface of `size` samples per side at altitude `height_m`.
    pub fn flat(size: u32, height_m: f32) -> Self {
        Self::Flat {
            size: size.clamp(2, 8192),
            height_m,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Classic { .. } => "classic",
            Self::Modern { .. } => "modern",
            Self::Flat { .. } => "flat",
            Self::Dem { .. } => "dem",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Classic { params } => params.validate(),
            Self::Modern { params } => params.validate(),
            Self::Flat { size, height_m } => {
                if !(2..=16_384).contains(size) {
                    return Err("flat terrain size must be in 2..=16384".into());
                }
                if !height_m.is_finite() {
                    return Err("flat terrain altitude must be finite".into());
                }
                Ok(())
            }
            Self::Dem { source } => {
                if !source.spacing_m.is_finite() || source.spacing_m <= 0.0 {
                    Err("DEM spacing must be finite and positive".into())
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid() {
        for name in MODERN_PRESETS {
            let p = modern_preset(name).expect("known preset");
            assert_eq!(p.preset, *name);
            p.validate().unwrap();
        }
        assert!(modern_preset("nope").is_none());
    }

    #[test]
    fn classic_validation() {
        let mut p = ClassicParams::default();
        p.validate().unwrap();
        p.divisor = 3;
        assert!(p.validate().is_err());
    }
}

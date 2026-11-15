//! Continuous rock-detail model.
//!
//! The heightfield stored in the project is the *base* surface. The high
//! frequency relief that makes a granite face read as granite at two metres is
//! not baked into that grid: it is evaluated continuously at render/export
//! time by [`vf_detail::DetailSampler`]. This keeps the detail independent of
//! the DEM resolution and lets the adaptive mesh decide where to spend
//! triangles.
//!
//! This module only holds the serialisable recipe. The evaluation lives in
//! `vf-detail`; the presets are deliberately expressed as physical signatures
//! (bed thickness, flute spacing, crater density) rather than arbitrary noise
//! parameters.

use serde::{Deserialize, Serialize};

/// Directional flutes / runnels cut down a face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grooves {
    pub enabled: bool,
    /// Mean spacing between channels, metres.
    pub spacing_m: f32,
    /// Channel depth, metres.
    pub depth_m: f32,
    /// Compass direction of the channels, degrees (0 = north, 90 = east).
    pub angle_deg: f32,
    /// 0 = rounded trough, 1 = sharp knife-cut.
    pub sharpness: f32,
    /// How much the channels wander, 0..1.
    pub wander: f32,
}

impl Default for Grooves {
    fn default() -> Self {
        Self {
            enabled: false,
            spacing_m: 18.0,
            depth_m: 0.8,
            angle_deg: 90.0,
            sharpness: 0.55,
            wander: 0.25,
        }
    }
}

/// Sharp crests and arêtes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ridges {
    pub enabled: bool,
    /// Crest height, metres.
    pub amplitude_m: f32,
    /// Base wavelength, metres.
    pub scale_m: f32,
    /// 0 = rounded, 1 = razor edge.
    pub sharpness: f32,
    /// Anisotropy: 1 = isotropic, >1 stretches crests along the direction.
    pub anisotropy: f32,
    /// Direction of stretching, degrees.
    pub angle_deg: f32,
}

impl Default for Ridges {
    fn default() -> Self {
        Self {
            enabled: true,
            amplitude_m: 0.4,
            scale_m: 22.0,
            sharpness: 0.6,
            anisotropy: 1.0,
            angle_deg: 0.0,
        }
    }
}

/// Impact / solution crater field with rims and ejecta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CraterField {
    pub enabled: bool,
    /// Fraction of cells that receive a crater, 0..1.
    pub density: f32,
    /// Crater cell size (mean distance between craters), metres.
    pub spacing_m: f32,
    /// Mean crater radius, metres.
    pub radius_m: f32,
    /// Mean crater depth, metres.
    pub depth_m: f32,
    /// Rim height as a fraction of depth, 0..1.
    pub rim: f32,
    /// Ejecta blanket extent, in crater radii.
    pub ejecta: f32,
}

impl Default for CraterField {
    fn default() -> Self {
        Self {
            enabled: false,
            density: 0.35,
            spacing_m: 90.0,
            radius_m: 14.0,
            depth_m: 4.0,
            rim: 0.35,
            ejecta: 0.45,
        }
    }
}

/// Exfoliation sheets: curved plates peeling off a massive rock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exfoliation {
    pub enabled: bool,
    /// Plate relief, metres.
    pub amplitude_m: f32,
    /// Plate size, metres.
    pub scale_m: f32,
    /// 0 = thick rounded shells, 1 = thin sharp flakes.
    pub sharpness: f32,
    /// Number of stacked sheets per scale.
    pub layers: u32,
}

impl Default for Exfoliation {
    fn default() -> Self {
        Self {
            enabled: false,
            amplitude_m: 0.6,
            scale_m: 9.0,
            sharpness: 0.65,
            layers: 3,
        }
    }
}

/// Vesicles / pits in volcanic rock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vesicles {
    pub enabled: bool,
    /// Pit depth, metres.
    pub depth_m: f32,
    /// Pit spacing, metres.
    pub spacing_m: f32,
    /// Fraction of the surface covered, 0..1.
    pub density: f32,
}

impl Default for Vesicles {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_m: 0.35,
            spacing_m: 1.6,
            density: 0.35,
        }
    }
}

/// Resolution-independent rock detail recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetailSettings {
    pub enabled: bool,
    /// Last applied profile, informational.
    pub profile: String,
    /// Master strength, 0..2.
    pub strength: f32,
    /// Base wavelength of the fractal, metres.
    pub scale_m: f32,
    /// Peak-to-peak amplitude of the fractal, metres.
    pub amplitude_m: f32,
    /// Fractal dimension, 2.0 (smooth) .. 3.0 (space-filling).
    pub fractal_dimension: f32,
    pub octaves: u32,
    pub lacunarity: f32,
    pub gain: f32,
    /// Domain warp, 0..1.
    pub warp: f32,
    /// 0 = detail everywhere, 1 = detail concentrated on steep faces.
    pub slope_bias: f32,
    /// Large-scale patchiness: breaks the repetition of the detail layers and
    /// varies their amplitude/scale over hundreds of metres, 0..1.
    pub heterogeneity: f32,
    /// Attenuate octaves below the pixel footprint instead of aliasing.
    pub band_limit: bool,
    pub grooves: Grooves,
    pub ridges: Ridges,
    pub craters: CraterField,
    pub exfoliation: Exfoliation,
    pub vesicles: Vesicles,
    /// Optional high-density patch around a chosen point. This is how a
    /// close-up of one rock gets centimetre-scale triangles without paying
    /// for a dense mesh over the whole 10 km terrain.
    pub focus_enabled: bool,
    /// Focus centre in world metres.
    pub focus_center: [f64; 2],
    /// Patch size, metres.
    pub focus_size_m: f32,
    /// Target vertex spacing inside the patch, metres.
    pub focus_spacing_m: f32,
    /// Extra seed mixed with the project seed.
    pub seed: u64,
}

impl Default for DetailSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: "generic".into(),
            strength: 1.0,
            scale_m: 45.0,
            amplitude_m: 1.2,
            fractal_dimension: 2.25,
            octaves: 7,
            lacunarity: 2.0,
            gain: 0.5,
            warp: 0.35,
            slope_bias: 0.35,
            heterogeneity: 0.6,
            band_limit: true,
            grooves: Grooves::default(),
            ridges: Ridges::default(),
            craters: CraterField::default(),
            exfoliation: Exfoliation::default(),
            vesicles: Vesicles::default(),
            focus_enabled: false,
            focus_center: [0.0, 0.0],
            focus_size_m: 240.0,
            focus_spacing_m: 0.35,
            seed: 0,
        }
    }
}

/// All rock detail profiles exposed by the CLI/UI/MCP.
pub const DETAIL_PRESETS: &[&str] = &[
    "generic",
    "granite",
    "basalt",
    "limestone",
    "sandstone",
    "schist",
    "chalk",
    "volcanic",
    "canyon",
    "patagonia",
];

impl DetailSettings {
    /// Build a profile. Returns `None` for an unknown name.
    pub fn preset(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase().replace('-', "_");
        let mut d = Self {
            enabled: true,
            profile: key.clone(),
            seed: 0,
            ..Self::default()
        };
        match key.as_str() {
            "generic" | "none" => {
                if key == "none" {
                    d.enabled = false;
                }
            }
            "granite" => {
                // Coarse exfoliation shells, rounded joints, sparse vesicles.
                d.scale_m = 32.0;
                d.amplitude_m = 1.6;
                d.fractal_dimension = 2.22;
                d.octaves = 7;
                d.gain = 0.48;
                d.warp = 0.30;
                d.slope_bias = 0.25;
                d.ridges = Ridges {
                    enabled: true,
                    amplitude_m: 0.55,
                    scale_m: 17.0,
                    sharpness: 0.55,
                    anisotropy: 1.4,
                    angle_deg: 35.0,
                };
                d.exfoliation = Exfoliation {
                    enabled: true,
                    amplitude_m: 0.75,
                    scale_m: 8.5,
                    sharpness: 0.6,
                    layers: 3,
                };
                d.vesicles = Vesicles {
                    enabled: true,
                    depth_m: 0.18,
                    spacing_m: 2.2,
                    density: 0.12,
                };
            }
            "basalt" => {
                // Columnar rhythm, vesicles, small craterlets.
                d.scale_m = 24.0;
                d.amplitude_m = 1.1;
                d.fractal_dimension = 2.42;
                d.octaves = 8;
                d.gain = 0.55;
                d.warp = 0.18;
                d.slope_bias = 0.45;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 13.0,
                    depth_m: 0.9,
                    angle_deg: 90.0,
                    sharpness: 0.7,
                    wander: 0.08,
                };
                d.vesicles = Vesicles {
                    enabled: true,
                    depth_m: 0.40,
                    spacing_m: 1.1,
                    density: 0.42,
                };
                d.craters = CraterField {
                    enabled: true,
                    density: 0.16,
                    spacing_m: 38.0,
                    radius_m: 4.5,
                    depth_m: 1.2,
                    rim: 0.30,
                    ejecta: 0.25,
                };
            }
            "limestone" => {
                // Bedding planes, solution flutes, dolines.
                d.scale_m = 55.0;
                d.amplitude_m = 2.1;
                d.fractal_dimension = 2.18;
                d.octaves = 7;
                d.gain = 0.46;
                d.warp = 0.40;
                d.slope_bias = 0.55;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 22.0,
                    depth_m: 1.6,
                    angle_deg: 90.0,
                    sharpness: 0.45,
                    wander: 0.35,
                };
                d.craters = CraterField {
                    enabled: true,
                    density: 0.22,
                    spacing_m: 130.0,
                    radius_m: 22.0,
                    depth_m: 6.0,
                    rim: 0.15,
                    ejecta: 0.0,
                };
                d.exfoliation = Exfoliation {
                    enabled: true,
                    amplitude_m: 0.35,
                    scale_m: 14.0,
                    sharpness: 0.4,
                    layers: 2,
                };
            }
            "sandstone" => {
                // Cross-bedding, rounded ledges, honeycomb pitting.
                d.scale_m = 42.0;
                d.amplitude_m = 1.5;
                d.fractal_dimension = 2.12;
                d.octaves = 6;
                d.gain = 0.44;
                d.warp = 0.45;
                d.slope_bias = 0.45;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 30.0,
                    depth_m: 0.8,
                    angle_deg: 20.0,
                    sharpness: 0.35,
                    wander: 0.5,
                };
                d.ridges = Ridges {
                    enabled: true,
                    amplitude_m: 0.35,
                    scale_m: 26.0,
                    sharpness: 0.35,
                    anisotropy: 1.6,
                    angle_deg: 20.0,
                };
                d.exfoliation = Exfoliation {
                    enabled: true,
                    amplitude_m: 0.45,
                    scale_m: 12.0,
                    sharpness: 0.45,
                    layers: 2,
                };
            }
            "schist" => {
                // Foliation, sharp flakes, strong anisotropy.
                d.scale_m = 28.0;
                d.amplitude_m = 1.35;
                d.fractal_dimension = 2.5;
                d.octaves = 8;
                d.gain = 0.58;
                d.warp = 0.22;
                d.slope_bias = 0.3;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 12.0,
                    depth_m: 1.0,
                    angle_deg: 35.0,
                    sharpness: 0.8,
                    wander: 0.2,
                };
                d.ridges = Ridges {
                    enabled: true,
                    amplitude_m: 0.6,
                    scale_m: 11.0,
                    sharpness: 0.8,
                    anisotropy: 2.2,
                    angle_deg: 35.0,
                };
            }
            "chalk" => {
                // Soft, rounded, dry valleys and flint bands.
                d.scale_m = 70.0;
                d.amplitude_m = 0.9;
                d.fractal_dimension = 2.05;
                d.octaves = 6;
                d.gain = 0.42;
                d.warp = 0.5;
                d.slope_bias = 0.2;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 36.0,
                    depth_m: 0.8,
                    angle_deg: 90.0,
                    sharpness: 0.3,
                    wander: 0.55,
                };
                d.exfoliation = Exfoliation {
                    enabled: true,
                    amplitude_m: 0.25,
                    scale_m: 18.0,
                    sharpness: 0.3,
                    layers: 2,
                };
            }
            "volcanic" => {
                // Lava ropes, scoria, small craters.
                d.scale_m = 20.0;
                d.amplitude_m = 1.7;
                d.fractal_dimension = 2.46;
                d.octaves = 8;
                d.gain = 0.56;
                d.warp = 0.55;
                d.slope_bias = 0.35;
                d.ridges = Ridges {
                    enabled: true,
                    amplitude_m: 0.7,
                    scale_m: 9.0,
                    sharpness: 0.45,
                    anisotropy: 2.8,
                    angle_deg: 0.0,
                };
                d.craters = CraterField {
                    enabled: true,
                    density: 0.55,
                    spacing_m: 34.0,
                    radius_m: 5.0,
                    depth_m: 1.8,
                    rim: 0.45,
                    ejecta: 0.6,
                };
                d.vesicles = Vesicles {
                    enabled: true,
                    depth_m: 0.3,
                    spacing_m: 1.4,
                    density: 0.3,
                };
            }
            "canyon" => {
                // Horizontal strata, vertical flutes, alcoves.
                d.scale_m = 60.0;
                d.amplitude_m = 1.9;
                d.fractal_dimension = 2.2;
                d.octaves = 7;
                d.gain = 0.48;
                d.warp = 0.3;
                d.slope_bias = 0.65;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 26.0,
                    depth_m: 2.0,
                    angle_deg: 90.0,
                    sharpness: 0.55,
                    wander: 0.18,
                };
                d.craters = CraterField {
                    enabled: true,
                    density: 0.10,
                    spacing_m: 150.0,
                    radius_m: 28.0,
                    depth_m: 4.0,
                    rim: 0.12,
                    ejecta: 0.0,
                };
            }
            "patagonia" => {
                // Vertical granite towers, glacial polish, exfoliation.
                d.scale_m = 34.0;
                d.amplitude_m = 1.5;
                d.fractal_dimension = 2.32;
                d.octaves = 8;
                d.gain = 0.5;
                d.warp = 0.25;
                d.slope_bias = 0.55;
                d.grooves = Grooves {
                    enabled: true,
                    spacing_m: 18.0,
                    depth_m: 1.2,
                    angle_deg: 75.0,
                    sharpness: 0.65,
                    wander: 0.12,
                };
                d.ridges = Ridges {
                    enabled: true,
                    amplitude_m: 0.55,
                    scale_m: 13.0,
                    sharpness: 0.7,
                    anisotropy: 1.8,
                    angle_deg: 75.0,
                };
                d.exfoliation = Exfoliation {
                    enabled: true,
                    amplitude_m: 0.6,
                    scale_m: 10.0,
                    sharpness: 0.65,
                    layers: 3,
                };
            }
            _ => return None,
        }
        Some(d)
    }

    /// Apply a profile by name, preserving the extra seed.
    pub fn apply_preset(&mut self, name: &str) -> bool {
        match Self::preset(name) {
            Some(preset) => {
                let seed = self.seed;
                *self = preset;
                self.seed = seed;
                true
            }
            None => false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.strength.is_finite() || !(0.0..=2.0).contains(&self.strength) {
            return Err("detail strength must be in 0..=2".into());
        }
        for (name, value, lo, hi) in [
            ("scale_m", self.scale_m, 0.25, 5000.0),
            ("amplitude_m", self.amplitude_m, 0.0, 200.0),
            ("fractal_dimension", self.fractal_dimension, 2.0, 3.0),
            ("lacunarity", self.lacunarity, 1.2, 4.0),
            ("gain", self.gain, 0.0, 0.95),
            ("warp", self.warp, 0.0, 1.0),
            ("slope_bias", self.slope_bias, 0.0, 1.0),
            ("heterogeneity", self.heterogeneity, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(lo..=hi).contains(&value) {
                return Err(format!("detail {name} must be in {lo}..={hi}, got {value}"));
            }
        }
        if !(1..=16).contains(&self.octaves) {
            return Err("detail octaves must be in 1..=16".into());
        }
        if !self.grooves.spacing_m.is_finite() || self.grooves.spacing_m <= 0.0 {
            return Err("detail groove spacing must be positive".into());
        }
        if !self.ridges.scale_m.is_finite() || self.ridges.scale_m <= 0.0 {
            return Err("detail ridge scale must be positive".into());
        }
        if !self.craters.spacing_m.is_finite() || self.craters.spacing_m <= 0.0 {
            return Err("detail crater spacing must be positive".into());
        }
        if !self.craters.radius_m.is_finite() || self.craters.radius_m <= 0.0 {
            return Err("detail crater radius must be positive".into());
        }
        if !self.exfoliation.scale_m.is_finite() || self.exfoliation.scale_m <= 0.0 {
            return Err("detail exfoliation scale must be positive".into());
        }
        if !self.vesicles.spacing_m.is_finite() || self.vesicles.spacing_m <= 0.0 {
            return Err("detail vesicle spacing must be positive".into());
        }
        if self.focus_enabled {
            if !self.focus_size_m.is_finite() || !(8.0..=5000.0).contains(&self.focus_size_m) {
                return Err("detail focus size must be in 8..=5000 metres".into());
            }
            if !self.focus_spacing_m.is_finite() || !(0.02..=50.0).contains(&self.focus_spacing_m) {
                return Err("detail focus spacing must be in 0.02..=50 metres".into());
            }
            if !self.focus_center[0].is_finite() || !self.focus_center[1].is_finite() {
                return Err("detail focus centre must be finite".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_distinct() {
        for name in DETAIL_PRESETS {
            let d = DetailSettings::preset(name).expect("known profile");
            d.validate().unwrap();
            assert_eq!(d.profile, *name);
        }
        let granite = DetailSettings::preset("granite").unwrap();
        let chalk = DetailSettings::preset("chalk").unwrap();
        assert!(granite.exfoliation.enabled);
        assert!(granite.amplitude_m > chalk.amplitude_m);
        assert!(DetailSettings::preset("nope").is_none());
        assert!(!DetailSettings::preset("none").unwrap().enabled);
    }

    #[test]
    fn apply_preset_preserves_seed() {
        let mut d = DetailSettings {
            seed: 42,
            ..DetailSettings::default()
        };
        assert!(d.apply_preset("granite"));
        assert_eq!(d.seed, 42);
        assert_eq!(d.profile, "granite");
        assert!(!d.apply_preset("nope"));
    }

    #[test]
    fn default_is_disabled_and_valid() {
        let d = DetailSettings::default();
        assert!(!d.enabled);
        d.validate().unwrap();
    }
}

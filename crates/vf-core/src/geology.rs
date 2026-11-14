//! Geological surface model: sedimentary strata, joint networks, karst
//! dissolution and micro-relief.
//!
//! The model is deliberately described as *rock families* rather than as a
//! generic noise stack: each preset maps a few physically meaningful knobs
//! (bed thickness, dip, hardness contrast, joint spacing) to the look of a
//! known rock type. Geometry is applied to the heightfield; the renderer
//! re-evaluates the same stratigraphy for colour, so bands stay coherent with
//! the relief.

use serde::{Deserialize, Serialize};

/// Stratigraphic layering. A layer stack is a plane with a dip, distorted by
/// low-frequency noise, sampled at a fixed bed thickness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Strata {
    pub enabled: bool,
    /// Mean bed thickness in metres.
    pub thickness_m: f32,
    /// Dip of the bedding plane, degrees.
    pub dip_deg: f32,
    /// Dip direction, compass (0 = south, 90 = west).
    pub dip_azimuth_deg: f32,
    /// Difference in erosion resistance between beds, 0 = uniform rock.
    pub hardness_contrast: f32,
    /// Peak-to-valley relief of the differential erosion, metres.
    pub relief_m: f32,
    /// Layer distortion amplitude, in bed thicknesses.
    pub warp: f32,
    /// Wavelength of the layer distortion, metres.
    pub warp_scale_m: f32,
    /// Colour contrast between beds, 0..1.
    pub color_contrast: f32,
}

impl Default for Strata {
    fn default() -> Self {
        Self {
            enabled: true,
            thickness_m: 30.0,
            dip_deg: 4.0,
            dip_azimuth_deg: 90.0,
            hardness_contrast: 0.6,
            relief_m: 8.0,
            warp: 0.5,
            warp_scale_m: 700.0,
            color_contrast: 0.45,
        }
    }
}

/// Joint / fracture pattern. Joints control both the cliff silhouettes and the
/// columnar look of basalt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JointPattern {
    /// Two near-orthogonal joint sets, typical of granite and limestone.
    #[default]
    Orthogonal,
    /// Hexagonal columnar jointing, typical of basalt flows.
    HexColumns,
    /// Radial fractures around a centre, typical of impact or volcanic necks.
    Radial,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Joints {
    pub enabled: bool,
    pub pattern: JointPattern,
    /// Mean spacing between fractures, metres.
    pub spacing_m: f32,
    /// Groove depth, metres.
    pub depth_m: f32,
    /// Groove width, fraction of the spacing.
    pub width: f32,
    /// Irregularity of the fracture lattice, 0 = regular, 1 = chaotic.
    pub jitter: f32,
    /// Rotation of the joint set, degrees.
    pub angle_deg: f32,
}

impl Default for Joints {
    fn default() -> Self {
        Self {
            enabled: false,
            pattern: JointPattern::Orthogonal,
            spacing_m: 40.0,
            depth_m: 3.0,
            width: 0.12,
            jitter: 0.5,
            angle_deg: 25.0,
        }
    }
}

/// Karst dissolution: dolines (sinkholes) and fluted rock (lapiaz).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dissolution {
    pub enabled: bool,
    /// Number of dolines per square kilometre, 0..1 scaled to the terrain.
    pub density: f32,
    pub radius_m: f32,
    pub depth_m: f32,
    /// Vertical fluting strength on steep faces, 0..1.
    pub fluting: f32,
}

impl Default for Dissolution {
    fn default() -> Self {
        Self {
            enabled: false,
            density: 0.4,
            radius_m: 90.0,
            depth_m: 12.0,
            fluting: 0.4,
        }
    }
}

/// Vertical fluting: narrow channels cut down steep faces, the signature of
/// desert cliffs and limestone gorges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gullies {
    pub enabled: bool,
    /// Mean spacing between channels, metres.
    pub spacing_m: f32,
    /// Channel depth, metres.
    pub depth_m: f32,
    /// Channel width, 0..1 (fraction of the spacing).
    pub sharpness: f32,
}

impl Default for Gullies {
    fn default() -> Self {
        Self {
            enabled: false,
            spacing_m: 55.0,
            depth_m: 9.0,
            sharpness: 0.45,
        }
    }
}

/// Granite spires: a handful of tall towers rising from a ridge, the signature
/// of Patagonian massifs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spires {
    pub enabled: bool,
    pub count: u32,
    /// Tower height above the local surface, metres.
    pub height_m: f32,
    /// Base radius, metres.
    pub radius_m: f32,
    /// 0 = rounded dome, 1 = needle.
    pub sharpness: f32,
}

impl Default for Spires {
    fn default() -> Self {
        Self {
            enabled: false,
            count: 7,
            height_m: 420.0,
            radius_m: 150.0,
            sharpness: 0.6,
        }
    }
}

/// A river canyon cut through a plateau: flat floor, steep stratified walls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Canyon {
    pub enabled: bool,
    /// Depth below the plateau rim, metres.
    pub depth_m: f32,
    /// Half-width of the canyon, metres.
    pub width_m: f32,
    /// Meander amplitude, 0..1.
    pub sinuosity: f32,
    /// Cut a smaller tributary canyon.
    pub tributary: bool,
}

impl Default for Canyon {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_m: 260.0,
            width_m: 1100.0,
            sinuosity: 0.5,
            tributary: true,
        }
    }
}

/// Complete geological surface model for a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeologySettings {
    pub enabled: bool,
    /// Name of the last applied preset (informational).
    pub preset: String,
    /// Let the preset choose a matching colour palette when it is applied.
    pub auto_palette: bool,
    pub strata: Strata,
    pub joints: Joints,
    pub dissolution: Dissolution,
    pub gullies: Gullies,
    pub canyon: Canyon,
    pub spires: Spires,
    /// Micro-relief added on slopes, metres.
    pub detail_m: f32,
    /// Wavelength of the micro-relief, metres.
    pub detail_scale_m: f32,
}

impl Default for GeologySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: "none".into(),
            auto_palette: false,
            strata: Strata::default(),
            joints: Joints::default(),
            dissolution: Dissolution::default(),
            gullies: Gullies::default(),
            canyon: Canyon::default(),
            spires: Spires::default(),
            detail_m: 1.0,
            detail_scale_m: 20.0,
        }
    }
}

impl GeologySettings {
    pub fn preset(name: &str) -> Option<Self> {
        let key = name.to_ascii_lowercase();
        if key == "none" {
            return Some(Self::default());
        }
        let mut g = Self {
            enabled: true,
            preset: key.clone(),
            ..Self::default()
        };
        match key.as_str() {
            "granite" => {
                g.strata.enabled = false;
                g.strata.relief_m = 0.0;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 45.0,
                    depth_m: 3.5,
                    width: 0.14,
                    jitter: 0.55,
                    angle_deg: 25.0,
                };
                g.detail_m = 2.2;
                g.detail_scale_m = 35.0;
                g.gullies = Gullies {
                    enabled: true,
                    spacing_m: 48.0,
                    depth_m: 5.0,
                    sharpness: 0.55,
                };
            }
            "basalt" => {
                g.strata.thickness_m = 45.0;
                g.strata.dip_deg = 1.0;
                g.strata.hardness_contrast = 0.3;
                g.strata.relief_m = 3.0;
                g.strata.color_contrast = 0.25;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::HexColumns,
                    spacing_m: 22.0,
                    depth_m: 6.5,
                    width: 0.24,
                    jitter: 0.08,
                    angle_deg: 0.0,
                };
                g.detail_m = 0.7;
                g.detail_scale_m = 8.0;
            }
            "limestone" => {
                g.strata.thickness_m = 12.0;
                g.strata.dip_deg = 3.0;
                g.strata.hardness_contrast = 0.55;
                g.strata.relief_m = 5.0;
                g.strata.warp = 0.5;
                g.strata.warp_scale_m = 600.0;
                g.strata.color_contrast = 0.35;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 28.0,
                    depth_m: 2.2,
                    width: 0.10,
                    jitter: 0.4,
                    angle_deg: 25.0,
                };
                g.gullies = Gullies {
                    enabled: true,
                    spacing_m: 34.0,
                    depth_m: 6.0,
                    sharpness: 0.5,
                };
                g.dissolution = Dissolution {
                    enabled: true,
                    density: 0.55,
                    radius_m: 90.0,
                    depth_m: 14.0,
                    fluting: 0.5,
                };
                g.detail_m = 0.9;
                g.detail_scale_m = 12.0;
            }
            "sandstone" => {
                g.strata.thickness_m = 26.0;
                g.strata.dip_deg = 6.0;
                g.strata.hardness_contrast = 0.85;
                g.strata.relief_m = 22.0;
                g.strata.warp = 0.7;
                g.strata.warp_scale_m = 900.0;
                g.strata.color_contrast = 0.6;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 60.0,
                    depth_m: 4.0,
                    width: 0.10,
                    jitter: 0.45,
                    angle_deg: 12.0,
                };
                g.detail_m = 5.0;
                g.detail_scale_m = 55.0;
                g.gullies = Gullies {
                    enabled: true,
                    spacing_m: 70.0,
                    depth_m: 22.0,
                    sharpness: 0.42,
                };
            }
            "schist" => {
                g.strata.thickness_m = 6.0;
                g.strata.dip_deg = 60.0;
                g.strata.hardness_contrast = 0.45;
                g.strata.relief_m = 7.0;
                g.strata.warp = 0.9;
                g.strata.warp_scale_m = 400.0;
                g.strata.color_contrast = 0.5;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 18.0,
                    depth_m: 2.0,
                    width: 0.08,
                    jitter: 0.6,
                    angle_deg: 70.0,
                };
                g.detail_m = 4.0;
                g.detail_scale_m = 30.0;
            }
            "chalk" => {
                g.strata.thickness_m = 40.0;
                g.strata.dip_deg = 2.0;
                g.strata.hardness_contrast = 0.25;
                g.strata.relief_m = 2.5;
                g.strata.color_contrast = 0.2;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 35.0,
                    depth_m: 1.5,
                    width: 0.07,
                    jitter: 0.5,
                    angle_deg: 30.0,
                };
                g.detail_m = 0.5;
                g.detail_scale_m = 10.0;
            }
            "canyon" => {
                g.strata.thickness_m = 26.0;
                g.strata.dip_deg = 2.0;
                g.strata.hardness_contrast = 0.85;
                g.strata.relief_m = 22.0;
                g.strata.warp = 0.7;
                g.strata.warp_scale_m = 900.0;
                g.strata.color_contrast = 0.6;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 60.0,
                    depth_m: 4.0,
                    width: 0.10,
                    jitter: 0.45,
                    angle_deg: 12.0,
                };
                g.gullies = Gullies {
                    enabled: true,
                    spacing_m: 62.0,
                    depth_m: 20.0,
                    sharpness: 0.45,
                };
                g.canyon = Canyon {
                    enabled: true,
                    depth_m: 260.0,
                    width_m: 1100.0,
                    sinuosity: 0.55,
                    tributary: true,
                };
                g.detail_m = 4.0;
                g.detail_scale_m = 45.0;
            }
            "patagonia" => {
                g.strata.enabled = false;
                g.strata.relief_m = 0.0;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::Orthogonal,
                    spacing_m: 38.0,
                    depth_m: 5.0,
                    width: 0.12,
                    jitter: 0.5,
                    angle_deg: 20.0,
                };
                g.gullies = Gullies {
                    enabled: true,
                    spacing_m: 40.0,
                    depth_m: 10.0,
                    sharpness: 0.55,
                };
                g.spires = Spires {
                    enabled: true,
                    count: 5,
                    height_m: 620.0,
                    radius_m: 210.0,
                    sharpness: 0.34,
                };
                g.detail_m = 2.5;
                g.detail_scale_m = 38.0;
            }
            "volcanic" => {
                g.strata.thickness_m = 60.0;
                g.strata.hardness_contrast = 0.5;
                g.strata.relief_m = 4.0;
                g.strata.color_contrast = 0.3;
                g.joints = Joints {
                    enabled: true,
                    pattern: JointPattern::HexColumns,
                    spacing_m: 30.0,
                    depth_m: 4.0,
                    width: 0.2,
                    jitter: 0.15,
                    angle_deg: 0.0,
                };
                g.detail_m = 1.5;
                g.detail_scale_m = 14.0;
            }
            _ => return None,
        }
        Some(g)
    }

    /// Palette that reads best with this rock family.
    pub fn suggested_palette(&self) -> &'static str {
        match self.preset.as_str() {
            "sandstone" => "canyon",
            "basalt" | "volcanic" => "volcanic",
            "chalk" => "coastal",
            "canyon" => "canyon",
            "patagonia" => "patagonia",
            _ => "alpine",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value, lo, hi) in [
            ("strata.thickness_m", self.strata.thickness_m, 1.0, 500.0),
            ("strata.dip_deg", self.strata.dip_deg, 0.0, 80.0),
            (
                "strata.hardness_contrast",
                self.strata.hardness_contrast,
                0.0,
                1.0,
            ),
            ("strata.relief_m", self.strata.relief_m, 0.0, 60.0),
            ("strata.warp", self.strata.warp, 0.0, 3.0),
            (
                "strata.warp_scale_m",
                self.strata.warp_scale_m,
                20.0,
                20_000.0,
            ),
            (
                "strata.color_contrast",
                self.strata.color_contrast,
                0.0,
                1.0,
            ),
            ("joints.spacing_m", self.joints.spacing_m, 2.0, 1000.0),
            ("joints.depth_m", self.joints.depth_m, 0.0, 40.0),
            ("joints.width", self.joints.width, 0.01, 0.5),
            ("joints.jitter", self.joints.jitter, 0.0, 1.0),
            ("dissolution.density", self.dissolution.density, 0.0, 1.0),
            (
                "dissolution.radius_m",
                self.dissolution.radius_m,
                5.0,
                800.0,
            ),
            ("dissolution.depth_m", self.dissolution.depth_m, 0.0, 60.0),
            ("dissolution.fluting", self.dissolution.fluting, 0.0, 1.0),
            ("gullies.spacing_m", self.gullies.spacing_m, 5.0, 400.0),
            ("gullies.depth_m", self.gullies.depth_m, 0.0, 40.0),
            ("gullies.sharpness", self.gullies.sharpness, 0.05, 0.95),
            ("canyon.depth_m", self.canyon.depth_m, 0.0, 1200.0),
            ("spires.height_m", self.spires.height_m, 0.0, 2000.0),
            ("spires.radius_m", self.spires.radius_m, 20.0, 1000.0),
            ("spires.sharpness", self.spires.sharpness, 0.0, 1.0),
            ("canyon.width_m", self.canyon.width_m, 50.0, 5000.0),
            ("canyon.sinuosity", self.canyon.sinuosity, 0.0, 1.0),
            ("detail_m", self.detail_m, 0.0, 20.0),
            ("detail_scale_m", self.detail_scale_m, 1.0, 1000.0),
        ] {
            if !value.is_finite() || !(lo..=hi).contains(&value) {
                return Err(format!(
                    "geology {name} must be in {lo}..={hi}, got {value}"
                ));
            }
        }
        if !self.strata.dip_azimuth_deg.is_finite() {
            return Err("geology strata.dip_azimuth_deg must be finite".into());
        }
        if !self.joints.angle_deg.is_finite() {
            return Err("geology joints.angle_deg must be finite".into());
        }
        Ok(())
    }
}

pub const GEOLOGY_PRESETS: &[&str] = &[
    "granite",
    "canyon",
    "patagonia",
    "basalt",
    "limestone",
    "sandstone",
    "schist",
    "chalk",
    "volcanic",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid() {
        for name in GEOLOGY_PRESETS {
            let g = GeologySettings::preset(name).expect("known preset");
            assert_eq!(g.preset, *name);
            assert!(g.enabled);
            g.validate().unwrap();
        }
        assert!(GeologySettings::preset("nope").is_none());
        assert!(GeologySettings::preset("none").unwrap().validate().is_ok());
    }

    #[test]
    fn defaults_validate() {
        GeologySettings::default().validate().unwrap();
    }

    #[test]
    fn sandstone_is_harder_banded_than_chalk() {
        let sand = GeologySettings::preset("sandstone").unwrap();
        let chalk = GeologySettings::preset("chalk").unwrap();
        assert!(sand.strata.hardness_contrast > chalk.strata.hardness_contrast);
        assert!(sand.strata.relief_m > chalk.strata.relief_m);
    }
}

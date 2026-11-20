use glam::{DMat4, DVec3};
use serde::{Deserialize, Serialize};

use crate::Rgb;

/// Look-at camera. World space is Z-up, X east, Y north, in metres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub position: DVec3,
    pub target: DVec3,
    pub up: DVec3,
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
    /// Optional 35 mm-equivalent focal length, informational only.
    pub focal_length_mm: Option<f32>,
    pub sensor_mm: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: DVec3::new(0.0, -2500.0, 1400.0),
            target: DVec3::new(0.0, 0.0, 350.0),
            up: DVec3::Z,
            fov_y_deg: 50.0,
            near: 1.0,
            far: 200_000.0,
            focal_length_mm: None,
            sensor_mm: 36.0,
        }
    }
}

impl Camera {
    pub fn forward(&self) -> DVec3 {
        (self.target - self.position).normalize_or_zero()
    }

    pub fn right(&self) -> DVec3 {
        self.forward().cross(self.up).normalize_or_zero()
    }

    pub fn true_up(&self) -> DVec3 {
        self.right().cross(self.forward()).normalize_or_zero()
    }

    pub fn fov_y_rad(&self) -> f64 {
        f64::from(self.fov_y_deg).to_radians()
    }

    pub fn aspect_fov_x_deg(&self, aspect: f64) -> f64 {
        2.0 * ((self.fov_y_rad() * 0.5).tan() * aspect)
            .atan()
            .to_degrees()
    }

    pub fn view_matrix(&self) -> DMat4 {
        DMat4::look_at_rh(self.position, self.target, self.up)
    }

    pub fn projection_matrix(&self, aspect: f64) -> DMat4 {
        DMat4::perspective_rh(
            self.fov_y_rad(),
            aspect.max(1e-6),
            f64::from(self.near.max(0.001)),
            f64::from(self.far.max(self.near + 1.0)),
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        let values = [
            self.position.x,
            self.position.y,
            self.position.z,
            self.target.x,
            self.target.y,
            self.target.z,
            self.up.x,
            self.up.y,
            self.up.z,
        ];
        if values.iter().any(|v| !v.is_finite()) {
            return Err("camera contains non-finite values".into());
        }
        if (self.target - self.position).length_squared() < 1e-6 {
            return Err("camera target must differ from position".into());
        }
        if !(1.0..=170.0).contains(&self.fov_y_deg) {
            return Err("camera fov must be in 1..=170 degrees".into());
        }
        if self.near <= 0.0 || self.far <= self.near {
            return Err("invalid camera near/far planes".into());
        }
        Ok(())
    }
}

/// Directional sun. Azimuth is measured from south, positive
/// toward west (0 = south, 90 = west, 180 = north, 270 = east).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sun {
    pub azimuth_deg_from_south: f32,
    pub declination_deg: f32,
    pub intensity: f32,
    /// Correlated colour temperature in kelvin.
    pub color_k: f32,
    pub angular_radius_deg: f32,
}

impl Default for Sun {
    fn default() -> Self {
        Self {
            azimuth_deg_from_south: 45.0,
            declination_deg: 30.0,
            intensity: 1.15,
            color_k: 5600.0,
            angular_radius_deg: 0.53,
        }
    }
}

impl Sun {
    /// Unit vector pointing from the ground toward the sun.
    pub fn direction(&self) -> DVec3 {
        let az = f64::from(self.azimuth_deg_from_south).to_radians();
        let dec = f64::from(self.declination_deg).to_radians();
        DVec3::new(-az.sin() * dec.cos(), -az.cos() * dec.cos(), dec.sin()).normalize_or_zero()
    }

    pub fn color(&self) -> Rgb {
        kelvin_to_rgb(self.color_k)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.azimuth_deg_from_south.is_finite() {
            return Err("sun azimuth must be finite".into());
        }
        if !(0.0..=90.0).contains(&self.declination_deg) {
            return Err("sun declination must be in 0..=90 degrees".into());
        }
        if !self.intensity.is_finite() || self.intensity < 0.0 {
            return Err("sun intensity must be finite and non-negative".into());
        }
        if !(1000.0..=40000.0).contains(&self.color_k) {
            return Err("sun colour temperature must be in 1000..=40000 K".into());
        }
        if !(0.0..=10.0).contains(&self.angular_radius_deg) {
            return Err("sun angular radius must be in 0..=10 degrees".into());
        }
        Ok(())
    }
}

/// Approximate blackbody colour (Tanner Helland fit), returned in linear RGB.
pub fn kelvin_to_rgb(kelvin: f32) -> Rgb {
    let t = kelvin.clamp(1000.0, 40_000.0) / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_16 * (t - 60.0).powf(-0.075_514_85)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    let c = Rgb::from_srgb_u8(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    );
    let max = c.r.max(c.g).max(c.b).max(1e-6);
    Rgb::new(c.r / max, c.g / max, c.b / max)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atmosphere {
    pub enabled: bool,
    /// Exponential haze density per metre.
    pub haze_density: f32,
    /// Height falloff, 1/metres.
    pub haze_falloff: f32,
    /// Ambient sky contribution to surfaces, 0..1.
    pub ambient_strength: f32,
    pub sky_intensity: f32,
    /// Star field strength, 0..1. Useful at dusk and night.
    pub stars: f32,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self {
            enabled: true,
            haze_density: 0.000_012,
            haze_falloff: 1.0 / 1200.0,
            ambient_strength: 0.62,
            sky_intensity: 1.0,
            stars: 0.0,
        }
    }
}

impl Atmosphere {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("haze_density", self.haze_density),
            ("haze_falloff", self.haze_falloff),
            ("ambient_strength", self.ambient_strength),
            ("sky_intensity", self.sky_intensity),
            ("stars", self.stars),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("atmosphere {name} must be finite and non-negative"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterSettings {
    pub enabled: bool,
    pub sea_level: f32,
    pub wave_strength: f32,
    pub wave_scale: f32,
    pub opacity: f32,
    pub foam: f32,
}

impl Default for WaterSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            sea_level: 0.0,
            wave_strength: 0.35,
            wave_scale: 0.06,
            opacity: 0.88,
            foam: 0.6,
        }
    }
}

impl WaterSettings {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("sea_level", self.sea_level),
            ("wave_strength", self.wave_strength),
            ("wave_scale", self.wave_scale),
            ("opacity", self.opacity),
            ("foam", self.foam),
        ] {
            if !value.is_finite() {
                return Err(format!("water {name} must be finite"));
            }
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err("water opacity must be in 0..=1".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub enabled: bool,
    /// Sky coverage, 0..1.
    pub coverage: f32,
    pub density: f32,
    pub altitude_m: f32,
    pub thickness_m: f32,
    pub softness: f32,
    /// Wind offset in metres, applied as `wind * time`.
    pub wind: [f32; 2],
    pub shadow_strength: f32,
    pub seed: u64,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            coverage: 0.35,
            density: 0.80,
            altitude_m: 4200.0,
            thickness_m: 700.0,
            softness: 0.45,
            wind: [12.0, 4.0],
            shadow_strength: 0.35,
            seed: 7,
        }
    }
}

impl CloudSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.coverage) || !(0.0..=1.0).contains(&self.softness) {
            return Err("cloud coverage and softness must be in 0..=1".into());
        }
        for (name, value) in [
            ("density", self.density),
            ("altitude_m", self.altitude_m),
            ("thickness_m", self.thickness_m),
            ("shadow_strength", self.shadow_strength),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("cloud {name} must be finite and non-negative"));
            }
        }
        Ok(())
    }
}

/// Surface colours and altitude/slope thresholds for the modern renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Palette {
    pub name: String,
    pub sky_zenith: Rgb,
    pub sky_horizon: Rgb,
    pub sun: Rgb,
    pub fog: Rgb,
    pub cloud_lit: Rgb,
    pub cloud_shadow: Rgb,
    pub water_shallow: Rgb,
    pub water_deep: Rgb,
    pub foam: Rgb,
    pub sand: Rgb,
    pub grass: Rgb,
    pub forest: Rgb,
    pub rock: Rgb,
    pub cliff: Rgb,
    pub snow: Rgb,
}

fn srgb(hex: u32) -> Rgb {
    Rgb::from_srgb_u8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

impl Default for Palette {
    fn default() -> Self {
        Self::alpine()
    }
}

impl Palette {
    pub fn alpine() -> Self {
        Self {
            name: "alpine".into(),
            sky_zenith: srgb(0x0d3b66),
            sky_horizon: srgb(0x8fb4d4),
            sun: srgb(0xfff4da),
            fog: srgb(0xa8c0d4),
            cloud_lit: srgb(0xffffff),
            cloud_shadow: srgb(0x7a879c),
            water_shallow: srgb(0x4f93a8),
            water_deep: srgb(0x0e3448),
            foam: srgb(0xf2f7f7),
            sand: srgb(0xc9b98d),
            grass: srgb(0x6f8a45),
            forest: srgb(0x2f4a28),
            rock: srgb(0x7a756c),
            cliff: srgb(0x57534c),
            snow: srgb(0xe9eff4),
        }
    }

    pub fn desert() -> Self {
        Self {
            name: "desert".into(),
            sky_zenith: srgb(0x3d7eb8),
            sky_horizon: srgb(0xf2dfbf),
            sun: srgb(0xfff0c8),
            fog: srgb(0xe6d3ae),
            cloud_lit: srgb(0xfff8ec),
            cloud_shadow: srgb(0xa08a70),
            water_shallow: srgb(0x4f9fb0),
            water_deep: srgb(0x123f4c),
            foam: srgb(0xeee5ce),
            sand: srgb(0xd9b878),
            grass: srgb(0xa39a58),
            forest: srgb(0x6f6a3c),
            rock: srgb(0xa06a48),
            cliff: srgb(0x7a4632),
            snow: srgb(0xf6f0df),
        }
    }

    pub fn volcanic() -> Self {
        Self {
            name: "volcanic".into(),
            sky_zenith: srgb(0x352d3d),
            sky_horizon: srgb(0xd18b58),
            sun: srgb(0xffd7a0),
            fog: srgb(0xb08a74),
            cloud_lit: srgb(0xe8d4c0),
            cloud_shadow: srgb(0x54464a),
            water_shallow: srgb(0x3c6d76),
            water_deep: srgb(0x14262c),
            foam: srgb(0xcfd8d4),
            sand: srgb(0x6b625a),
            grass: srgb(0x55563c),
            forest: srgb(0x343a28),
            rock: srgb(0x3d3633),
            cliff: srgb(0x272120),
            snow: srgb(0xe6e4e6),
        }
    }

    pub fn coastal() -> Self {
        Self {
            name: "coastal".into(),
            sky_zenith: srgb(0x2f83c4),
            sky_horizon: srgb(0xd8edf5),
            sun: srgb(0xfff3d8),
            fog: srgb(0xc3dce8),
            cloud_lit: srgb(0xffffff),
            cloud_shadow: srgb(0x7f92a6),
            water_shallow: srgb(0x2fa7b8),
            water_deep: srgb(0x0a3d58),
            foam: srgb(0xf7ffff),
            sand: srgb(0xe6d5ab),
            grass: srgb(0x84a35f),
            forest: srgb(0x39613b),
            rock: srgb(0x85796a),
            cliff: srgb(0x62584d),
            snow: srgb(0xf0f4f6),
        }
    }

    pub fn canyon() -> Self {
        Self {
            name: "canyon".into(),
            sky_zenith: srgb(0x3b79b0),
            sky_horizon: srgb(0xead6bb),
            sun: srgb(0xffeccb),
            fog: srgb(0xd9c0a4),
            cloud_lit: srgb(0xfff7ea),
            cloud_shadow: srgb(0x967d68),
            water_shallow: srgb(0x5a9486),
            water_deep: srgb(0x1c3b36),
            foam: srgb(0xe8e0ce),
            sand: srgb(0xbfa17e),
            grass: srgb(0x7f7a4e),
            forest: srgb(0x4e4c30),
            rock: srgb(0x8d6248),
            cliff: srgb(0x5f3f31),
            snow: srgb(0xf2e8da),
        }
    }

    pub fn lunar() -> Self {
        Self {
            name: "lunar".into(),
            sky_zenith: srgb(0x05070c),
            sky_horizon: srgb(0x141820),
            sun: srgb(0xfff8ee),
            fog: srgb(0x101318),
            cloud_lit: srgb(0x222222),
            cloud_shadow: srgb(0x000000),
            water_shallow: srgb(0x222222),
            water_deep: srgb(0x000000),
            foam: srgb(0x222222),
            sand: srgb(0x9a9890),
            grass: srgb(0x8d8b84),
            forest: srgb(0x7d7b75),
            rock: srgb(0x8a8882),
            cliff: srgb(0x65635e),
            snow: srgb(0xd8d8d5),
        }
    }

    pub fn island() -> Self {
        Self {
            name: "island".into(),
            sky_zenith: srgb(0x2a86cc),
            sky_horizon: srgb(0xd9f1f7),
            sun: srgb(0xfff6de),
            fog: srgb(0xc9e6ee),
            cloud_lit: srgb(0xffffff),
            cloud_shadow: srgb(0x7e94a8),
            water_shallow: srgb(0x2fc4c9),
            water_deep: srgb(0x07506e),
            foam: srgb(0xffffff),
            sand: srgb(0xefdcb0),
            grass: srgb(0x8cb35e),
            forest: srgb(0x2f6a3a),
            rock: srgb(0x8b8070),
            cliff: srgb(0x645b4f),
            snow: srgb(0xf4f7f8),
        }
    }

    /// Patagonian granite: grey towers, turquoise water, bright snow.
    pub fn patagonia() -> Self {
        Self {
            name: "patagonia".into(),
            sky_zenith: srgb(0x1d5f9e),
            sky_horizon: srgb(0xcfe4ee),
            sun: srgb(0xfff6e2),
            fog: srgb(0xc2d6df),
            cloud_lit: srgb(0xffffff),
            cloud_shadow: srgb(0x7d8fa0),
            water_shallow: srgb(0x2fc7c0),
            water_deep: srgb(0x0b5f6b),
            foam: srgb(0xffffff),
            sand: srgb(0xd8d2c0),
            grass: srgb(0x7d8a55),
            forest: srgb(0x40522f),
            rock: srgb(0x8c8a86),
            cliff: srgb(0x5f5d5a),
            snow: srgb(0xf2f6f8),
        }
    }

    /// Warm dusk: deep blue zenith, amber horizon, dark rock.
    pub fn dusk() -> Self {
        Self {
            name: "dusk".into(),
            sky_zenith: srgb(0x111c3e),
            sky_horizon: srgb(0xf0a05a),
            sun: srgb(0xffc48a),
            fog: srgb(0xc98a6a),
            cloud_lit: srgb(0xffd2a8),
            cloud_shadow: srgb(0x5a4050),
            water_shallow: srgb(0x3f7f8f),
            water_deep: srgb(0x122436),
            foam: srgb(0xe8d8c8),
            sand: srgb(0xc98a5e),
            grass: srgb(0x8a7a4a),
            forest: srgb(0x4a4430),
            rock: srgb(0x8a5a44),
            cliff: srgb(0x553226),
            snow: srgb(0xe8dcd0),
        }
    }

    pub fn preset(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "alpine" => Some(Self::alpine()),
            "desert" => Some(Self::desert()),
            "volcanic" => Some(Self::volcanic()),
            "coastal" => Some(Self::coastal()),
            "canyon" => Some(Self::canyon()),
            "lunar" => Some(Self::lunar()),
            "island" => Some(Self::island()),
            "dusk" => Some(Self::dusk()),
            "patagonia" => Some(Self::patagonia()),
            _ => None,
        }
    }
}

impl Palette {
    pub fn validate(&self) -> Result<(), String> {
        for (name, c) in [
            ("sky_zenith", self.sky_zenith),
            ("sky_horizon", self.sky_horizon),
            ("sun", self.sun),
            ("fog", self.fog),
            ("cloud_lit", self.cloud_lit),
            ("cloud_shadow", self.cloud_shadow),
            ("water_shallow", self.water_shallow),
            ("water_deep", self.water_deep),
            ("foam", self.foam),
            ("sand", self.sand),
            ("grass", self.grass),
            ("forest", self.forest),
            ("rock", self.rock),
            ("cliff", self.cliff),
            ("snow", self.snow),
        ] {
            if ![c.r, c.g, c.b].iter().all(|v| v.is_finite()) {
                return Err(format!("palette colour {name} is not finite"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainStyle {
    pub snow_line_m: f32,
    pub tree_line_m: f32,
    pub beach_height_m: f32,
    pub rock_slope: f32,
    pub snow_slope: f32,
    pub macro_variation: f32,
}

impl Default for TerrainStyle {
    fn default() -> Self {
        Self {
            snow_line_m: 1700.0,
            tree_line_m: 1300.0,
            beach_height_m: 8.0,
            rock_slope: 0.55,
            snow_slope: 0.62,
            macro_variation: 0.18,
        }
    }
}

impl TerrainStyle {
    pub fn validate(&self) -> Result<(), String> {
        if !self.snow_line_m.is_finite() || !self.tree_line_m.is_finite() {
            return Err("terrain style lines must be finite".into());
        }
        if !(0.0..=1.0).contains(&self.rock_slope) || !(0.0..=1.0).contains(&self.snow_slope) {
            return Err("terrain slope thresholds must be in 0..=1".into());
        }
        if !self.macro_variation.is_finite() || self.macro_variation < 0.0 {
            return Err("macro_variation must be finite and non-negative".into());
        }
        Ok(())
    }
}

/// Procedural rock presets exposed by the material panel and the CLI.
pub const ROCK_PRESETS: &[&str] = &[
    "generic",
    "granite",
    "basalt",
    "limestone",
    "sandstone",
    "schist",
    "chalk",
    "volcanic",
    "chert",
    "andesite",
    "marble",
    "phyllite",
    "slate",
    "gneiss",
    "conglomerate",
    "travertine",
];

/// Surface material mix used by the renderer. Masks are procedural; the
/// palette supplies the base colours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Materials {
    pub enabled: bool,
    /// Weight of the geological rock colour (strata, joints), 0..1.
    pub rock: f32,
    /// Strength of the per-pixel normal perturbation, 0..1.
    pub detail_normal: f32,
    /// Wavelength of the surface detail, metres.
    pub detail_scale_m: f32,
    /// Scree / talus coverage, 0..1.
    pub scree: f32,
    /// Slope at which scree starts accumulating, 0..1 (0 = flat).
    pub scree_slope: f32,
    /// Snow patchiness, 0..1 (0 = uniform band, 1 = isolated patches).
    pub snow_coverage: f32,
    /// Maximum slope that still holds snow, 0..1.
    pub snow_slope: f32,
    /// Vegetation density, 0..1.
    pub vegetation: f32,
    /// Wetness darkening near water and in gullies, 0..1.
    pub wetness: f32,
    /// Named procedural rock (granite, basalt, limestone, sandstone, schist,
    /// chalk, volcanic or generic). The preset drives the procedural grain,
    /// bedding, crack and lichen layers evaluated by the CPU shader.
    pub rock_preset: String,
    /// Strength of the procedural bump normal, 0..2.
    pub bump_strength: f32,
    /// Wavelength of the procedural bump, metres.
    pub bump_scale_m: f32,
    /// Additional geometric displacement from the material layer, metres.
    /// 0 keeps the geometry from the detail menu only.
    pub displacement_m: f32,
    /// Optional custom texture (PNG/JPG/TIFF) projected in world space.
    pub texture_path: String,
    /// World-space size of one texture tile, metres.
    pub texture_scale_m: f32,
    /// Bump strength read from the custom texture luminance, 0..2.
    pub texture_bump: f32,
    /// Displacement strength read from the custom texture luminance, metres.
    pub texture_displacement_m: f32,
    /// Mix between the geological rock colour and the custom texture, 0..1.
    pub texture_mix: f32,
}

impl Default for Materials {
    fn default() -> Self {
        Self {
            enabled: true,
            rock: 0.8,
            detail_normal: 0.0,
            detail_scale_m: 18.0,
            scree: 0.4,
            scree_slope: 0.42,
            snow_coverage: 0.55,
            snow_slope: 0.62,
            vegetation: 0.74,
            wetness: 0.3,
            rock_preset: "generic".into(),
            bump_strength: 0.0,
            bump_scale_m: 7.0,
            displacement_m: 0.0,
            texture_path: String::new(),
            texture_scale_m: 80.0,
            texture_bump: 0.0,
            texture_displacement_m: 0.0,
            texture_mix: 0.8,
        }
    }
}

impl Materials {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value, lo, hi) in [
            ("rock", self.rock, 0.0, 1.0),
            ("detail_normal", self.detail_normal, 0.0, 1.0),
            ("detail_scale_m", self.detail_scale_m, 1.0, 500.0),
            ("scree", self.scree, 0.0, 1.0),
            ("scree_slope", self.scree_slope, 0.0, 1.0),
            ("snow_coverage", self.snow_coverage, 0.0, 1.0),
            ("snow_slope", self.snow_slope, 0.0, 1.0),
            ("vegetation", self.vegetation, 0.0, 1.0),
            ("wetness", self.wetness, 0.0, 1.0),
            ("bump_strength", self.bump_strength, 0.0, 2.0),
            ("bump_scale_m", self.bump_scale_m, 0.05, 500.0),
            ("displacement_m", self.displacement_m, 0.0, 100.0),
            ("texture_scale_m", self.texture_scale_m, 0.1, 10_000.0),
            ("texture_bump", self.texture_bump, 0.0, 2.0),
            (
                "texture_displacement_m",
                self.texture_displacement_m,
                0.0,
                100.0,
            ),
            ("texture_mix", self.texture_mix, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(lo..=hi).contains(&value) {
                return Err(format!(
                    "material {name} must be in {lo}..={hi}, got {value}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tonemap {
    None,
    Reinhard,
    #[default]
    Aces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
    Custom,
}

impl Quality {
    pub fn presets(self) -> (u32, u32, bool, bool, u8, u32) {
        // (width, height, shadows, ao, ssaa, shadow_resolution)
        match self {
            Self::Low => (1280, 720, false, false, 1, 1024),
            Self::Medium => (1920, 1080, true, false, 1, 2048),
            Self::High => (1920, 1080, true, true, 2, 4096),
            Self::Ultra => (3840, 2160, true, true, 2, 4096),
            Self::Custom => (1920, 1080, true, true, 2, 4096),
        }
    }
}

/// Fast / refined / photoreal render recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    Fast,
    #[default]
    Refined,
    Photoreal,
}

impl RenderMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Refined => "refined",
            Self::Photoreal => "photoreal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Rapide",
            Self::Refined => "Soigné",
            Self::Photoreal => "Photorealiste",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" | "rapide" => Some(Self::Fast),
            "refined" | "soigne" | "soigné" => Some(Self::Refined),
            "photoreal" | "photorealiste" | "photoréaliste" => Some(Self::Photoreal),
            _ => None,
        }
    }

    /// (ssaa, shadows, ao, shadow_resolution, subdivision)
    pub fn presets(self) -> (u8, bool, bool, u32, u8) {
        match self {
            Self::Fast => (1, false, false, 1024, 1),
            Self::Refined => (2, true, true, 4096, 2),
            // The CPU renderer has no path tracing; "photoreal" means the
            // highest-quality deterministic raster pass plus Blender export.
            Self::Photoreal => (3, true, true, 8192, 3),
        }
    }

    pub fn apply(self, settings: &mut RenderSettings) {
        let (ssaa, shadows, ao, shadow_resolution, subdivision) = self.presets();
        settings.ssaa = ssaa;
        settings.shadows = shadows;
        settings.ao = ao;
        settings.shadow_resolution = shadow_resolution;
        settings.subdivision = subdivision;
        settings.render_mode = self;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    /// Supersampling factor: 1, 2 or 3.
    pub ssaa: u8,
    pub shadows: bool,
    pub shadow_resolution: u32,
    pub ao: bool,
    /// Shape steep faces into vertical cliffs before meshing.
    pub cliffs: bool,
    /// Undulate the cliff ladder so the risers are not a single horizontal
    /// plane, 0..1.
    pub cliff_warp: f32,
    /// Wavelength of the cliff-ladder undulation, metres.
    pub cliff_warp_scale_m: f32,
    pub clouds: bool,
    pub water: bool,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub tonemap: Tonemap,
    pub quality: Quality,
    /// Fast / refined / photoreal recipe.
    pub render_mode: RenderMode,
    /// Geometric subdivision of the heightfield mesh (1 = base grid).
    pub subdivision: u8,
    /// Screen-adaptive block LOD. Disable for a uniform grid (tests, exact
    /// vertex counts, some exports).
    pub adaptive: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        let (width, height, shadows, ao, ssaa, shadow_resolution) = Quality::High.presets();
        Self {
            width,
            height,
            ssaa,
            shadows,
            shadow_resolution,
            ao,
            cliffs: true,
            cliff_warp: 0.65,
            cliff_warp_scale_m: 520.0,
            clouds: true,
            water: true,
            exposure: 1.0,
            contrast: 0.05,
            saturation: 1.12,
            gamma: 2.2,
            tonemap: Tonemap::Aces,
            quality: Quality::High,
            render_mode: RenderMode::Refined,
            subdivision: 1,
            adaptive: true,
        }
    }
}

impl RenderSettings {
    pub fn apply_quality(&mut self, quality: Quality) {
        let (width, height, shadows, ao, ssaa, shadow_resolution) = quality.presets();
        self.quality = quality;
        self.width = width;
        self.height = height;
        self.shadows = shadows;
        self.ao = ao;
        self.ssaa = ssaa;
        self.shadow_resolution = shadow_resolution;
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > 16_384 || self.height > 16_384 {
            return Err("render dimensions must be in 1..=16384".into());
        }
        if !(1..=3).contains(&self.ssaa) {
            return Err("ssaa must be 1, 2 or 3".into());
        }
        if !(1..=8).contains(&self.subdivision) {
            return Err("render subdivision must be in 1..=8".into());
        }
        if !self.cliff_warp.is_finite() || !(0.0..=1.0).contains(&self.cliff_warp) {
            return Err("cliff_warp must be in 0..=1".into());
        }
        if !self.cliff_warp_scale_m.is_finite()
            || !(80.0..=4000.0).contains(&self.cliff_warp_scale_m)
        {
            return Err("cliff_warp_scale_m must be in 80..=4000 metres".into());
        }
        if !(256..=8192).contains(&self.shadow_resolution) {
            return Err("shadow_resolution must be in 256..=8192".into());
        }
        if !self.exposure.is_finite() || self.exposure <= 0.0 {
            return Err("exposure must be finite and positive".into());
        }
        if !self.gamma.is_finite() || self.gamma <= 0.0 {
            return Err("gamma must be finite and positive".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_direction_matches_compass() {
        let mut s = Sun {
            azimuth_deg_from_south: 0.0,
            declination_deg: 0.0,
            ..Sun::default()
        };
        assert!((s.direction() - DVec3::new(0.0, -1.0, 0.0)).length() < 1e-9);
        s.azimuth_deg_from_south = 90.0;
        assert!((s.direction() - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-9);
        s.azimuth_deg_from_south = 270.0;
        assert!((s.direction() - DVec3::new(1.0, 0.0, 0.0)).length() < 1e-9);
        s.azimuth_deg_from_south = 180.0;
        s.declination_deg = 90.0;
        assert!((s.direction() - DVec3::Z).length() < 1e-9);
    }

    #[test]
    fn default_materials_do_not_add_surface_noise() {
        let m = Materials::default();
        assert_eq!(m.bump_strength, 0.0);
        assert_eq!(m.detail_normal, 0.0);
        assert_eq!(m.displacement_m, 0.0);
        m.validate().unwrap();
    }

    #[test]
    fn palette_presets_exist() {
        for name in [
            "alpine", "desert", "volcanic", "coastal", "canyon", "lunar", "island",
        ] {
            let p = Palette::preset(name).unwrap();
            p.validate().unwrap();
        }
    }

    #[test]
    fn camera_matrices_are_finite() {
        let c = Camera::default();
        let vp = c.projection_matrix(16.0 / 9.0) * c.view_matrix();
        assert!(vp.to_cols_array().iter().all(|v| v.is_finite()));
    }
}

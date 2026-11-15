//! Terrain modifier stack: the editable recipe applied on top of the base
//! heightfield at render time.
//!
//! Every modifier is a small, deterministic operator working on the current
//! heightfield. Parameters live in a name → value map rather than one struct
//! per operator, so the CLI, the UI and the MCP server can describe, list and
//! edit any modifier through the same [`ParamSpec`] table. Adding a parameter
//! never breaks an older project: missing keys fall back to the declared
//! default when the stack is normalised.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One operator family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKind {
    #[default]
    Displace,
    Warp,
    Transform,
    Swirl,
    Terrace,
    Fold,
    Stratify,
    Grooves,
    Ridges,
    CraterField,
    Exfoliate,
    Vesicles,
    Undercut,
    Aperture,
    Clamp,
    Blur,
    Sharpen,
    Crater,
    RecastCliff,
    Erosion,
    Thermal,
    Alluvium,
    Snow,
    Dunes,
    Talus,
    Combine,
    BiasGain,
}

impl ModifierKind {
    pub const ALL: [Self; 27] = [
        Self::Displace,
        Self::Warp,
        Self::Transform,
        Self::Swirl,
        Self::Terrace,
        Self::Fold,
        Self::Stratify,
        Self::Grooves,
        Self::Ridges,
        Self::CraterField,
        Self::Exfoliate,
        Self::Vesicles,
        Self::Undercut,
        Self::Aperture,
        Self::Clamp,
        Self::Blur,
        Self::Sharpen,
        Self::Crater,
        Self::RecastCliff,
        Self::Erosion,
        Self::Thermal,
        Self::Alluvium,
        Self::Snow,
        Self::Dunes,
        Self::Talus,
        Self::Combine,
        Self::BiasGain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Displace => "displace",
            Self::Warp => "warp",
            Self::Transform => "transform",
            Self::Swirl => "swirl",
            Self::Terrace => "terrace",
            Self::Fold => "fold",
            Self::Stratify => "stratify",
            Self::Grooves => "grooves",
            Self::Ridges => "ridges",
            Self::CraterField => "crater_field",
            Self::Exfoliate => "exfoliate",
            Self::Vesicles => "vesicles",
            Self::Undercut => "undercut",
            Self::Aperture => "aperture",
            Self::Clamp => "clamp",
            Self::Blur => "blur",
            Self::Sharpen => "sharpen",
            Self::Crater => "crater",
            Self::RecastCliff => "recast_cliff",
            Self::Erosion => "erosion",
            Self::Thermal => "thermal",
            Self::Alluvium => "alluvium",
            Self::Snow => "snow",
            Self::Dunes => "dunes",
            Self::Talus => "talus",
            Self::Combine => "combine",
            Self::BiasGain => "bias_gain",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Displace => "Displace",
            Self::Warp => "Warp",
            Self::Transform => "Transform",
            Self::Swirl => "Swirl",
            Self::Terrace => "Terrace",
            Self::Fold => "Fold",
            Self::Stratify => "Stratify",
            Self::Grooves => "Sillons",
            Self::Ridges => "Arêtes",
            Self::CraterField => "Champ de cratères",
            Self::Exfoliate => "Exfoliation",
            Self::Vesicles => "Vésicules",
            Self::Undercut => "Surplomb / corniche",
            Self::Aperture => "Aperture",
            Self::Clamp => "Clamp / Clip",
            Self::Blur => "Blur / Smooth",
            Self::Sharpen => "Sharpen",
            Self::Crater => "Crater",
            Self::RecastCliff => "Recast cliff",
            Self::Erosion => "Erosion",
            Self::Thermal => "Thermal / Talus",
            Self::Alluvium => "Alluvium",
            Self::Snow => "Snow / Glaciation",
            Self::Dunes => "Dunes / Sand",
            Self::Talus => "Talus / Éboulis",
            Self::Combine => "Combine",
            Self::BiasGain => "BiasGain / Height",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::Displace | Self::Warp | Self::Transform | Self::Swirl => "Déformation",
            Self::Terrace
            | Self::Fold
            | Self::Stratify
            | Self::Aperture
            | Self::Clamp
            | Self::Blur
            | Self::Sharpen
            | Self::Crater
            | Self::RecastCliff => "Sculpture",
            Self::Erosion
            | Self::Thermal
            | Self::Alluvium
            | Self::Snow
            | Self::Dunes
            | Self::Talus => "Érosion",
            Self::Grooves
            | Self::Ridges
            | Self::CraterField
            | Self::Exfoliate
            | Self::Vesicles
            | Self::Undercut => "Détail de roche",
            Self::Combine | Self::BiasGain => "Combinaison",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL.into_iter().find(|kind| kind.name() == key)
    }

    pub fn specs(self) -> &'static [ParamSpec] {
        match self {
            Self::Displace => DISPLACE,
            Self::Warp => WARP,
            Self::Transform => TRANSFORM,
            Self::Swirl => SWIRL,
            Self::Terrace => TERRACE,
            Self::Fold => FOLD,
            Self::Stratify => STRATIFY,
            Self::Grooves => GROOVES,
            Self::Ridges => RIDGES,
            Self::CraterField => CRATER_FIELD,
            Self::Exfoliate => EXFOLIATE,
            Self::Vesicles => VESICLES,
            Self::Undercut => UNDERCUT,
            Self::Aperture => APERTURE,
            Self::Clamp => CLAMP,
            Self::Blur => BLUR,
            Self::Sharpen => SHARPEN,
            Self::Crater => CRATER,
            Self::RecastCliff => RECAST_CLIFF,
            Self::Erosion => EROSION,
            Self::Thermal => THERMAL,
            Self::Alluvium => ALLUVIUM,
            Self::Snow => SNOW,
            Self::Dunes => DUNES,
            Self::Talus => TALUS,
            Self::Combine => COMBINE,
            Self::BiasGain => BIAS_GAIN,
        }
    }

    /// Default values for a freshly added modifier.
    pub fn defaults(self) -> BTreeMap<String, f32> {
        self.specs()
            .iter()
            .map(|spec| (spec.key.to_string(), spec.default))
            .collect()
    }
}

/// Description of one editable parameter, used by the UI, the CLI and MCP.
#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub integer: bool,
    pub logarithmic: bool,
    /// Non-empty for enumerated parameters: numeric value + display label.
    pub choices: &'static [(f32, &'static str)],
}

const fn p(
    key: &'static str,
    label: &'static str,
    unit: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamSpec {
    ParamSpec {
        key,
        label,
        unit,
        min,
        max,
        default,
        integer: false,
        logarithmic: false,
        choices: &[],
    }
}

const fn pi(key: &'static str, label: &'static str, min: f32, max: f32, default: f32) -> ParamSpec {
    ParamSpec {
        key,
        label,
        unit: "",
        min,
        max,
        default,
        integer: true,
        logarithmic: false,
        choices: &[],
    }
}

const fn plog(
    key: &'static str,
    label: &'static str,
    unit: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamSpec {
    ParamSpec {
        key,
        label,
        unit,
        min,
        max,
        default,
        integer: false,
        logarithmic: true,
        choices: &[],
    }
}

const fn pc(
    key: &'static str,
    label: &'static str,
    default: f32,
    choices: &'static [(f32, &'static str)],
) -> ParamSpec {
    let max = (choices.len() - 1) as f32;
    ParamSpec {
        key,
        label,
        unit: "",
        min: 0.0,
        max,
        default,
        integer: true,
        logarithmic: false,
        choices,
    }
}

// 1. Déformation et déplacement ------------------------------------------------

const DISPLACE: &[ParamSpec] = &[
    p("amount_m", "Amplitude", "m", -600.0, 600.0, 40.0),
    plog("scale_m", "Échelle", "m", 20.0, 20_000.0, 700.0),
    p("angle_deg", "Étirement", "°", 0.0, 360.0, 0.0),
    p("ridged", "Crêtes", "", 0.0, 1.0, 0.0),
];

const WARP: &[ParamSpec] = &[
    p("amount_m", "Déplacement", "m", 0.0, 2000.0, 120.0),
    plog("scale_m", "Échelle", "m", 20.0, 20_000.0, 900.0),
    p("angle_deg", "Direction", "°", 0.0, 360.0, 0.0),
    p("slope_bias", "Pilotage par pente", "", 0.0, 1.0, 0.35),
];

const TRANSFORM: &[ParamSpec] = &[
    p("offset_x_m", "Translation X", "m", -20_000.0, 20_000.0, 0.0),
    p("offset_y_m", "Translation Y", "m", -20_000.0, 20_000.0, 0.0),
    p("rotation_deg", "Rotation", "°", -180.0, 180.0, 0.0),
    p("scale", "Échelle", "×", 0.1, 4.0, 1.0),
    pc("repeat", "Répétition", 0.0, &[(0.0, "non"), (1.0, "oui")]),
];

const SWIRL: &[ParamSpec] = &[
    p("radius_m", "Rayon", "m", 10.0, 20_000.0, 1200.0),
    p("angle_deg", "Torsion", "°", -360.0, 360.0, 45.0),
    p("center_x", "Centre X", "", 0.0, 1.0, 0.5),
    p("center_y", "Centre Y", "", 0.0, 1.0, 0.5),
    p("falloff", "Atténuation", "", 0.0, 1.0, 0.5),
];

// 2. Sculpture géologique ------------------------------------------------------

const TERRACE: &[ParamSpec] = &[
    p("step_m", "Marche", "m", 2.0, 1000.0, 80.0),
    p("strength", "Force", "", 0.0, 1.0, 0.8),
    p("sharpness", "Netteté", "", 0.0, 1.0, 0.4),
    p("gamma", "Profil gamma", "", 0.1, 2.0, 0.5),
    p("variation", "Variation des bancs", "", 0.0, 1.0, 0.3),
];

const FOLD: &[ParamSpec] = &[
    p("angle_deg", "Axe", "°", 0.0, 360.0, 0.0),
    plog("period_m", "Période", "m", 50.0, 20_000.0, 1600.0),
    p("strength", "Compression", "", 0.0, 1.0, 0.5),
    p("amplitude_m", "Relief des plis", "m", 0.0, 1000.0, 120.0),
];

const STRATIFY: &[ParamSpec] = &[
    p("thickness_m", "Épaisseur des bancs", "m", 4.0, 500.0, 45.0),
    p("depth_m", "Relief", "m", 0.0, 200.0, 12.0),
    p("dip_deg", "Pendage", "°", -60.0, 60.0, 4.0),
    p("azimuth_deg", "Azimut", "°", 0.0, 360.0, 0.0),
    p("steep_only", "Faces seulement", "", 0.0, 1.0, 0.5),
];

const GROOVES: &[ParamSpec] = &[
    p("spacing_m", "Espacement", "m", 2.0, 400.0, 18.0),
    p("depth_m", "Profondeur", "m", 0.0, 30.0, 1.0),
    p("angle_deg", "Orientation", "°", 0.0, 360.0, 90.0),
    p("sharpness", "Netteté", "", 0.0, 1.0, 0.6),
    p("wander", "Divagation", "", 0.0, 1.0, 0.3),
    p("slope_only", "Faces seulement", "", 0.0, 1.0, 0.8),
];

const RIDGES: &[ParamSpec] = &[
    p("scale_m", "Échelle", "m", 2.0, 500.0, 20.0),
    p("amplitude_m", "Hauteur", "m", 0.0, 30.0, 1.0),
    p("sharpness", "Netteté", "", 0.0, 1.0, 0.6),
    p("anisotropy", "Anisotropie", "×", 1.0, 6.0, 1.5),
    p("angle_deg", "Orientation", "°", 0.0, 360.0, 0.0),
    p("slope_only", "Faces seulement", "", 0.0, 1.0, 0.3),
];

const CRATER_FIELD: &[ParamSpec] = &[
    p("density", "Densité", "", 0.0, 1.0, 0.35),
    p("spacing_m", "Espacement", "m", 10.0, 2000.0, 90.0),
    p("radius_m", "Rayon", "m", 1.0, 500.0, 14.0),
    p("depth_m", "Profondeur", "m", 0.0, 100.0, 4.0),
    p("rim", "Bourrelet", "", 0.0, 1.0, 0.35),
    p("ejecta", "Éjecta", "×", 0.0, 2.0, 0.4),
];

const EXFOLIATE: &[ParamSpec] = &[
    p("scale_m", "Taille des plaques", "m", 1.0, 200.0, 9.0),
    p("amplitude_m", "Relief", "m", 0.0, 20.0, 0.7),
    p("sharpness", "Netteté", "", 0.0, 1.0, 0.6),
    pi("layers", "Feuillets", 1.0, 8.0, 3.0),
    p("slope_only", "Faces seulement", "", 0.0, 1.0, 0.3),
];

const VESICLES: &[ParamSpec] = &[
    p("spacing_m", "Espacement", "m", 0.5, 30.0, 1.6),
    p("depth_m", "Profondeur", "m", 0.0, 10.0, 0.35),
    p("density", "Densité", "", 0.0, 1.0, 0.35),
];

const UNDERCUT: &[ParamSpec] = &[
    p("depth_m", "Creux", "m", 0.0, 20.0, 2.0),
    p("band_m", "Épaisseur des bancs", "m", 2.0, 200.0, 25.0),
    p("height_m", "Hauteur de la niche", "m", 1.0, 50.0, 5.0),
    p(
        "spacing_m",
        "Espacement des contreforts",
        "m",
        5.0,
        300.0,
        40.0,
    ),
    p("angle_deg", "Orientation", "°", 0.0, 360.0, 90.0),
    p("sharpness", "Netteté", "", 0.0, 1.0, 0.55),
];

const APERTURE: &[ParamSpec] = &[
    p("radius_m", "Rayon", "m", 5.0, 1000.0, 60.0),
    p("amount", "Dilater / contracter", "", -1.0, 1.0, 0.5),
];

const CLAMP: &[ParamSpec] = &[
    p("min_m", "Altitude min", "m", -10_000.0, 12_000.0, -2000.0),
    p("max_m", "Altitude max", "m", -10_000.0, 12_000.0, 6000.0),
    p("feather_m", "Adoucissement", "m", 0.0, 500.0, 0.0),
];

const BLUR: &[ParamSpec] = &[
    plog("radius_m", "Rayon", "m", 1.0, 5000.0, 40.0),
    pi("iterations", "Itérations", 1.0, 8.0, 1.0),
];

const SHARPEN: &[ParamSpec] = &[
    p("radius_m", "Rayon", "m", 1.0, 2000.0, 25.0),
    p("amount", "Force", "", 0.0, 4.0, 0.8),
];

const CRATER: &[ParamSpec] = &[
    pi("count", "Nombre", 1.0, 32.0, 3.0),
    p("radius_m", "Rayon", "m", 20.0, 3000.0, 220.0),
    p("depth_m", "Profondeur", "m", 5.0, 500.0, 90.0),
    p("rim", "Bourrelet", "", 0.0, 0.6, 0.18),
];

const RECAST_CLIFF: &[ParamSpec] = &[
    p("talus_deg", "Angle de talus", "°", 15.0, 80.0, 45.0),
    p("amplitude", "Amplification", "", 0.0, 2.0, 1.0),
    p("gain", "Gain", "", 1.0, 8.0, 3.0),
    p("radius_m", "Lissage du masque", "m", 0.0, 500.0, 60.0),
    pi("iterations", "Itérations Poisson", 20.0, 2000.0, 300.0),
];

// 3. Érosion naturelle ---------------------------------------------------------

const EROSION: &[ParamSpec] = &[
    pi("droplets", "Gouttes", 1000.0, 500_000.0, 30_000.0),
    pi("lifetime", "Durée de vie", 5.0, 200.0, 40.0),
    p("erosion_rate", "Érosion", "", 0.0, 1.0, 0.3),
    p("deposition_rate", "Dépôt", "", 0.0, 1.0, 0.25),
];

const THERMAL: &[ParamSpec] = &[
    p("talus_angle_deg", "Angle de talus", "°", 10.0, 70.0, 36.0),
    pi("iterations", "Itérations", 1.0, 64.0, 8.0),
];

const ALLUVIUM: &[ParamSpec] = &[
    pi("droplets", "Gouttes", 1000.0, 500_000.0, 40_000.0),
    p("deposition", "Dépôt", "", 0.0, 1.0, 0.7),
    p("smoothing", "Lissage", "", 0.0, 1.0, 0.5),
];

const SNOW: &[ParamSpec] = &[
    p("amount_m", "Accumulation", "m", -200.0, 200.0, 30.0),
    p("coverage", "Couverture", "", 0.0, 1.0, 0.8),
    plog("scale_m", "Échelle", "m", 20.0, 5000.0, 300.0),
];

const DUNES: &[ParamSpec] = &[
    p("amplitude_m", "Amplitude", "m", 0.0, 100.0, 8.0),
    p("wavelength_m", "Longueur d'onde", "m", 10.0, 2000.0, 140.0),
    p("angle_deg", "Vent", "°", 0.0, 360.0, 0.0),
    p("smoothing", "Lissage préalable", "", 0.0, 1.0, 0.4),
    p("stoss", "Versant au vent", "", 0.1, 0.95, 0.70),
    p("lee", "Versant sous le vent", "", 0.2, 1.0, 0.90),
];

const TALUS: &[ParamSpec] = &[
    p("talus_deg", "Angle de repos", "°", 15.0, 70.0, 38.0),
    p("depth_m", "Épaisseur max", "m", 0.0, 600.0, 100.0),
    p("noise", "Irrégularité", "", 0.0, 0.5, 0.15),
];

// 4. Combinaison géométrique ---------------------------------------------------

const COMBINE: &[ParamSpec] = &[
    pc(
        "operation",
        "Opération",
        0.0,
        &[
            (0.0, "add"),
            (1.0, "subtract"),
            (2.0, "max"),
            (3.0, "min"),
            (4.0, "embed"),
        ],
    ),
    p("amplitude_m", "Amplitude", "m", 0.0, 3000.0, 250.0),
    plog("scale_m", "Échelle", "m", 20.0, 20_000.0, 1200.0),
    p("weight", "Mélange", "", 0.0, 1.0, 0.5),
    p("ridged", "Crêtes", "", 0.0, 1.0, 0.5),
];

const BIAS_GAIN: &[ParamSpec] = &[
    p("bias", "Biais", "", -1.0, 1.0, 0.0),
    p("gain", "Gain", "", -1.0, 1.0, 0.0),
    p("pivot", "Pivot", "", 0.0, 1.0, 0.5),
];

/// One entry in the stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modifier {
    pub kind: ModifierKind,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

fn enabled_default() -> bool {
    true
}

impl Modifier {
    pub fn new(kind: ModifierKind) -> Self {
        Self {
            kind,
            enabled: true,
            params: kind.defaults(),
        }
    }

    pub fn specs(&self) -> &'static [ParamSpec] {
        self.kind.specs()
    }

    /// Read a parameter, falling back to the declared default.
    pub fn value(&self, key: &str) -> f32 {
        self.params.get(key).copied().unwrap_or_else(|| {
            self.specs()
                .iter()
                .find(|spec| spec.key == key)
                .map_or(0.0, |spec| spec.default)
        })
    }

    pub fn integer(&self, key: &str) -> i64 {
        self.value(key).round() as i64
    }

    /// Set a parameter, clamped to its declared range. Integer parameters are
    /// rounded. Returns false when the key does not belong to this kind.
    pub fn set(&mut self, key: &str, value: f32) -> bool {
        let Some(spec) = self.specs().iter().find(|spec| spec.key == key) else {
            return false;
        };
        let mut value = value.clamp(spec.min, spec.max);
        if !value.is_finite() {
            value = spec.default;
        }
        if spec.integer {
            value = value.round();
        }
        self.params.insert(spec.key.to_string(), value);
        true
    }

    /// Fill missing parameters, clamp everything and round integer knobs.
    pub fn normalize(&mut self) {
        let mut clean = BTreeMap::new();
        for spec in self.specs() {
            let value = self
                .params
                .get(spec.key)
                .copied()
                .filter(|value| value.is_finite())
                .unwrap_or(spec.default)
                .clamp(spec.min, spec.max);
            let value = if spec.integer { value.round() } else { value };
            clean.insert(spec.key.to_string(), value);
        }
        self.params = clean;
    }

    pub fn validate(&self) -> Result<(), String> {
        for spec in self.specs() {
            let value = self.params.get(spec.key).copied().unwrap_or(spec.default);
            if !value.is_finite() {
                return Err(format!("{}: {} is not finite", self.kind.name(), spec.key));
            }
            if !(spec.min..=spec.max).contains(&value) {
                return Err(format!(
                    "{}: {} = {value} outside {}..={}",
                    self.kind.name(),
                    spec.key,
                    spec.min,
                    spec.max
                ));
            }
        }
        if self.kind == ModifierKind::Clamp && self.value("min_m") > self.value("max_m") {
            return Err("clamp: min_m must not exceed max_m".into());
        }
        Ok(())
    }

    /// Human-readable one-line description, e.g. `Displace (40 m, 700 m)`.
    pub fn summary(&self) -> String {
        let values: Vec<String> = self
            .specs()
            .iter()
            .take(2)
            .map(|spec| {
                if !spec.choices.is_empty() {
                    let choice = spec
                        .choices
                        .iter()
                        .find(|(value, _)| (*value - self.value(spec.key)).abs() < 0.5)
                        .map_or("?", |(_, label)| *label);
                    format!("{}: {choice}", spec.label)
                } else if spec.unit.is_empty() {
                    format!("{} {:.2}", spec.label, self.value(spec.key))
                } else {
                    format!("{} {:.0} {}", spec.label, self.value(spec.key), spec.unit)
                }
            })
            .collect();
        format!("{} ({})", self.kind.label(), values.join(", "))
    }
}

/// Ordered list of modifiers, applied after the base terrain pipeline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModifierStack {
    #[serde(default)]
    pub items: Vec<Modifier>,
}

impl ModifierStack {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn active(&self) -> impl Iterator<Item = &Modifier> {
        self.items.iter().filter(|item| item.enabled)
    }

    pub fn push(&mut self, kind: ModifierKind) -> usize {
        self.items.push(Modifier::new(kind));
        self.items.len() - 1
    }

    pub fn remove(&mut self, index: usize) -> Option<Modifier> {
        (index < self.items.len()).then(|| self.items.remove(index))
    }

    pub fn move_up(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.items.len() {
            return false;
        }
        self.items.swap(index, index - 1);
        true
    }

    pub fn move_down(&mut self, index: usize) -> bool {
        if index + 1 >= self.items.len() {
            return false;
        }
        self.items.swap(index, index + 1);
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn normalize(&mut self) {
        for item in &mut self.items {
            item.normalize();
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.items.len() > 128 {
            return Err("modifier stack is limited to 128 entries".into());
        }
        for (index, item) in self.items.iter().enumerate() {
            item.validate()
                .map_err(|error| format!("modifier {index}: {error}"))?;
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        if self.items.is_empty() {
            return "empty".into();
        }
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                format!(
                    "{index}. {}{}",
                    if item.enabled { "" } else { "[off] " },
                    item.summary()
                )
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_round_trip_and_defaults() {
        for kind in ModifierKind::ALL {
            let modifier = Modifier::new(kind);
            assert_eq!(modifier.kind, kind);
            assert!(
                !modifier.specs().is_empty(),
                "{} has no params",
                kind.name()
            );
            for spec in modifier.specs() {
                let value = modifier.value(spec.key);
                assert!((spec.min..=spec.max).contains(&value));
            }
            modifier.validate().unwrap();
            let text = ron::to_string(&modifier).unwrap();
            let back: Modifier = ron::from_str(&text).unwrap();
            assert_eq!(back, modifier);
        }
    }

    #[test]
    fn set_clamps_and_rounds() {
        let mut modifier = Modifier::new(ModifierKind::Blur);
        assert!(modifier.set("iterations", 2.7));
        assert_eq!(modifier.value("iterations"), 3.0);
        assert!(modifier.set("radius_m", 1.0e9));
        assert_eq!(modifier.value("radius_m"), 5000.0);
        assert!(!modifier.set("nope", 1.0));
    }

    #[test]
    fn normalise_fills_missing_keys() {
        let mut modifier = Modifier::new(ModifierKind::Crater);
        modifier.params.clear();
        modifier.normalize();
        assert_eq!(modifier.value("count"), 3.0);
        assert_eq!(modifier.value("depth_m"), 90.0);
    }

    #[test]
    fn stack_reorders_and_validates() {
        let mut stack = ModifierStack::default();
        stack.push(ModifierKind::Displace);
        stack.push(ModifierKind::Crater);
        assert!(stack.move_up(1));
        assert_eq!(stack.items[0].kind, ModifierKind::Crater);
        assert!(stack.move_down(0));
        assert_eq!(stack.items[0].kind, ModifierKind::Displace);
        stack.validate().unwrap();
        assert_eq!(stack.items.len(), 2);
    }

    #[test]
    fn clamp_rejects_inverted_bounds() {
        let mut modifier = Modifier::new(ModifierKind::Clamp);
        modifier.set("min_m", 2000.0);
        modifier.set("max_m", 1000.0);
        assert!(modifier.validate().is_err());
    }
}

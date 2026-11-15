//! LightWave-style camera rig: an editable point-of-view path, a pursuit
//! target, an optical camera and an aircraft motion profile.
//!
//! The rig is deliberately separate from `FlightSettings` (the older
//! pattern-based flight module). A rig is authored: keys carry a time and
//! Bezier handles, the target can be fixed or follow the point of view, the
//! camera can lag on a spring, and the body constrains how fast each axis can
//! accelerate. `vf-flight::rig` turns this recipe into a camera sample.

use serde::{Deserialize, Serialize};

/// One authored key on a 3D path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathKey {
    /// Key time, seconds.
    pub time_s: f32,
    /// Absolute world position, metres (Z-up).
    pub position: [f64; 3],
    /// Incoming handle, relative to `position`.
    pub in_handle: [f64; 3],
    /// Outgoing handle, relative to `position`.
    pub out_handle: [f64; 3],
    /// 0 = linear segment, 1 = fully eased.
    pub ease: f32,
}

impl Default for PathKey {
    fn default() -> Self {
        Self {
            time_s: 0.0,
            position: [0.0, 0.0, 500.0],
            in_handle: [-300.0, 0.0, 0.0],
            out_handle: [300.0, 0.0, 0.0],
            ease: 0.5,
        }
    }
}

impl PathKey {
    pub fn new(time_s: f32, position: [f64; 3]) -> Self {
        // Zero handles mean "automatic": `vf_flight::rig::sample_path` derives
        // a smooth Catmull-Rom tangent from the neighbours. The old ±300 m
        // default made every key traced by the user overshoot wildly.
        Self {
            time_s,
            position,
            in_handle: [0.0; 3],
            out_handle: [0.0; 3],
            // 0 = constant parameter speed. A per-key smoothstep makes the
            // camera stop and surge at every key, which the body can only
            // follow by lagging behind the curve. Users can still raise
            // `Adoucissement` on a key when they want an ease in/out.
            ease: 0.0,
        }
    }

    pub fn with_handles(mut self, in_handle: [f64; 3], out_handle: [f64; 3]) -> Self {
        self.in_handle = in_handle;
        self.out_handle = out_handle;
        self
    }
}

/// Cubic Bezier path through timed keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Path3D {
    pub keys: Vec<PathKey>,
    pub closed: bool,
}

impl Default for Path3D {
    fn default() -> Self {
        Self {
            keys: vec![
                PathKey::new(0.0, [-1500.0, -1500.0, 650.0]),
                PathKey::new(4.0, [0.0, -1900.0, 900.0]),
                PathKey::new(8.0, [1500.0, -1500.0, 650.0]),
            ],
            closed: false,
        }
    }
}

impl Path3D {
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.len() < 2 {
            return Err("a camera path needs at least two keys".into());
        }
        let mut previous = f32::NEG_INFINITY;
        for key in &self.keys {
            if !key.time_s.is_finite() {
                return Err("path key time must be finite".into());
            }
            if key.time_s < previous {
                return Err("path key times must be non-decreasing".into());
            }
            previous = key.time_s;
            if key.position.iter().any(|v| !v.is_finite())
                || key.in_handle.iter().any(|v| !v.is_finite())
                || key.out_handle.iter().any(|v| !v.is_finite())
            {
                return Err("path key contains non-finite values".into());
            }
        }
        if self.keys.last().is_some_and(|key| key.time_s <= 0.0) {
            return Err("the last path key must have a positive time".into());
        }
        Ok(())
    }

    pub fn duration_s(&self) -> f32 {
        self.keys.last().map_or(0.0, |key| key.time_s.max(0.0))
    }

    pub fn push(&mut self, key: PathKey) {
        self.keys.push(key);
        self.keys.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    }
}

/// How the camera chooses its aim point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TargetMode {
    /// A fixed world point.
    #[default]
    Fixed,
    /// Look ahead of the eye along its velocity vector.
    EyeAhead,
    /// A second authored path.
    Path,
    /// Follow the eye with a fixed world-space offset (rigid link).
    Offset,
    /// Look at where the eye was `lead_s` seconds ago (trailing target).
    Trail,
    /// Aim along the camera's own axis (manual orientation).
    Manual,
}

/// Pursuit target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetRig {
    pub mode: TargetMode,
    pub fixed: [f64; 3],
    pub path: Path3D,
    /// Look-ahead for [`TargetMode::EyeAhead`], delay for
    /// [`TargetMode::Trail`], seconds.
    pub lead_s: f32,
    /// Offset added to the aim point, metres.
    pub offset: [f64; 3],
    /// Smoothing of the aim point, 0 = snap, 1 = heavy.
    pub smoothing: f32,
}

impl Default for TargetRig {
    fn default() -> Self {
        let mut path = Path3D::default();
        for key in &mut path.keys {
            key.position[2] *= 0.5;
        }
        Self {
            mode: TargetMode::EyeAhead,
            fixed: [0.0, 0.0, 500.0],
            path,
            lead_s: 0.35,
            offset: [0.0, 0.0, 0.0],
            smoothing: 0.35,
        }
    }
}

/// How the optical camera is attached to the point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CameraLink {
    /// Camera exactly at the eye.
    #[default]
    Eye,
    /// Camera follows the eye with a spring: the classic pursuit lag.
    Spring,
}

/// Optical and focus settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpticalRig {
    pub link: CameraLink,
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
    /// Spring stiffness for [`CameraLink::Spring`].
    pub spring_stiffness: f32,
    pub spring_damping: f32,
    /// World-space offset from the eye.
    pub offset: [f64; 3],
    /// Explicit roll added to the automatic bank, degrees.
    pub roll_deg: f32,
    pub dof_enabled: bool,
    /// Focus distance; 0 means focus on the aim point.
    pub focus_distance_m: f32,
    pub fstop: f32,
    pub focus_on_target: bool,
}

impl Default for OpticalRig {
    fn default() -> Self {
        Self {
            link: CameraLink::Eye,
            fov_y_deg: 50.0,
            near: 1.0,
            far: 200_000.0,
            spring_stiffness: 6.0,
            spring_damping: 0.8,
            offset: [0.0, 0.0, 0.0],
            roll_deg: 0.0,
            dof_enabled: false,
            focus_distance_m: 100.0,
            fstop: 8.0,
            focus_on_target: true,
        }
    }
}

/// Aircraft behaviour presets. They set acceleration limits per axis and a
/// handful of response constants; the path itself stays authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AircraftPreset {
    #[default]
    DroneFpv,
    Bird,
    Helicopter,
    Airship,
    FighterJet,
    Manual,
}

impl AircraftPreset {
    pub const ALL: [Self; 6] = [
        Self::DroneFpv,
        Self::Bird,
        Self::Helicopter,
        Self::Airship,
        Self::FighterJet,
        Self::Manual,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::DroneFpv => "drone",
            Self::Bird => "bird",
            Self::Helicopter => "helicopter",
            Self::Airship => "airship",
            Self::FighterJet => "fighter",
            Self::Manual => "manual",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DroneFpv => "Drone FPV",
            Self::Bird => "Oiseau",
            Self::Helicopter => "Hélicoptère",
            Self::Airship => "Ballon dirigeable",
            Self::FighterJet => "Avion de chasse",
            Self::Manual => "Manuel",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let key = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|preset| preset.name() == key)
    }
}

/// Per-axis motion limits and response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionProfile {
    pub preset: AircraftPreset,
    pub max_speed_mps: f32,
    pub accel_forward_mps2: f32,
    pub accel_lateral_mps2: f32,
    pub accel_vertical_mps2: f32,
    pub jerk_mps3: f32,
    pub drag: f32,
    pub gravity: f32,
    pub bank_gain: f32,
    pub max_bank_deg: f32,
    pub roll_response: f32,
    pub pitch_response: f32,
    pub look_lag: f32,
    pub shake_deg: f32,
    pub turbulence: f32,
    pub manual_orientation: bool,
}

impl Default for MotionProfile {
    fn default() -> Self {
        Self::preset(AircraftPreset::DroneFpv)
    }
}

impl MotionProfile {
    pub fn preset(preset: AircraftPreset) -> Self {
        let mut profile = Self {
            preset,
            max_speed_mps: 30.0,
            accel_forward_mps2: 25.0,
            accel_lateral_mps2: 30.0,
            accel_vertical_mps2: 20.0,
            jerk_mps3: 60.0,
            drag: 0.10,
            gravity: 0.0,
            bank_gain: 1.0,
            max_bank_deg: 75.0,
            roll_response: 10.0,
            pitch_response: 8.0,
            look_lag: 0.08,
            shake_deg: 0.10,
            turbulence: 0.35,
            manual_orientation: false,
        };
        match preset {
            AircraftPreset::DroneFpv => {}
            AircraftPreset::Bird => {
                profile.max_speed_mps = 18.0;
                profile.accel_forward_mps2 = 8.0;
                profile.accel_lateral_mps2 = 12.0;
                profile.accel_vertical_mps2 = 10.0;
                profile.jerk_mps3 = 28.0;
                profile.drag = 0.16;
                profile.gravity = 9.81;
                profile.bank_gain = 0.9;
                profile.max_bank_deg = 55.0;
                profile.roll_response = 6.0;
                profile.shake_deg = 0.16;
                profile.turbulence = 0.5;
            }
            AircraftPreset::Helicopter => {
                profile.max_speed_mps = 22.0;
                profile.accel_forward_mps2 = 4.0;
                profile.accel_lateral_mps2 = 6.0;
                profile.accel_vertical_mps2 = 5.0;
                profile.jerk_mps3 = 12.0;
                profile.drag = 0.16;
                profile.bank_gain = 0.55;
                profile.max_bank_deg = 25.0;
                profile.roll_response = 4.0;
                profile.look_lag = 0.16;
                profile.shake_deg = 0.05;
                profile.turbulence = 0.25;
            }
            AircraftPreset::Airship => {
                profile.max_speed_mps = 12.0;
                profile.accel_forward_mps2 = 1.5;
                profile.accel_lateral_mps2 = 1.5;
                profile.accel_vertical_mps2 = 1.0;
                profile.jerk_mps3 = 3.0;
                profile.drag = 0.08;
                profile.bank_gain = 0.25;
                profile.max_bank_deg = 8.0;
                profile.roll_response = 2.0;
                profile.look_lag = 0.4;
                profile.shake_deg = 0.02;
                profile.turbulence = 0.15;
            }
            AircraftPreset::FighterJet => {
                profile.max_speed_mps = 90.0;
                profile.accel_forward_mps2 = 40.0;
                profile.accel_lateral_mps2 = 60.0;
                profile.accel_vertical_mps2 = 30.0;
                profile.jerk_mps3 = 140.0;
                profile.drag = 0.04;
                profile.bank_gain = 1.1;
                profile.max_bank_deg = 85.0;
                profile.roll_response = 16.0;
                profile.look_lag = 0.02;
                profile.shake_deg = 0.04;
                profile.turbulence = 0.2;
            }
            AircraftPreset::Manual => {
                profile.max_speed_mps = 200.0;
                profile.accel_forward_mps2 = 200.0;
                profile.accel_lateral_mps2 = 200.0;
                profile.accel_vertical_mps2 = 200.0;
                profile.jerk_mps3 = 500.0;
                profile.drag = 0.0;
                profile.bank_gain = 0.0;
                profile.max_bank_deg = 0.0;
                profile.roll_response = 1.0;
                profile.look_lag = 0.0;
                profile.shake_deg = 0.0;
                profile.turbulence = 0.0;
                profile.manual_orientation = true;
            }
        }
        profile
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value, lo, hi) in [
            ("max_speed_mps", self.max_speed_mps, 0.5, 500.0),
            ("accel_forward_mps2", self.accel_forward_mps2, 0.05, 500.0),
            ("accel_lateral_mps2", self.accel_lateral_mps2, 0.05, 500.0),
            ("accel_vertical_mps2", self.accel_vertical_mps2, 0.05, 500.0),
            ("jerk_mps3", self.jerk_mps3, 0.1, 2000.0),
            ("drag", self.drag, 0.0, 1.0),
            ("gravity", self.gravity, 0.0, 30.0),
            ("bank_gain", self.bank_gain, 0.0, 2.0),
            ("max_bank_deg", self.max_bank_deg, 0.0, 89.0),
            ("roll_response", self.roll_response, 0.1, 40.0),
            ("pitch_response", self.pitch_response, 0.1, 40.0),
            ("look_lag", self.look_lag, 0.0, 2.0),
            ("shake_deg", self.shake_deg, 0.0, 5.0),
            ("turbulence", self.turbulence, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(lo..=hi).contains(&value) {
                return Err(format!("rig {name} must be in {lo}..={hi}, got {value}"));
            }
        }
        Ok(())
    }
}

/// Automatic ground / wall constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Constraints {
    pub enabled: bool,
    /// Use a fixed reference plane in addition to the terrain.
    pub reference_enabled: bool,
    pub reference_z_m: f32,
    /// Target clearance above the reference surface, metres.
    pub ground_clearance_m: f32,
    /// Minimum distance to a steep wall, metres.
    pub wall_distance_m: f32,
    pub wall_avoidance: bool,
    pub apply_eye: bool,
    pub apply_camera: bool,
    pub apply_target: bool,
    /// 0 = hard correction, 1 = very soft.
    pub softness: f32,
    /// How far ahead the avoidance looks, seconds.
    pub look_ahead_s: f32,
}

impl Default for Constraints {
    fn default() -> Self {
        Self {
            enabled: true,
            reference_enabled: false,
            reference_z_m: 0.0,
            ground_clearance_m: 25.0,
            wall_distance_m: 40.0,
            wall_avoidance: true,
            apply_eye: true,
            apply_camera: true,
            apply_target: false,
            softness: 0.35,
            look_ahead_s: 0.6,
        }
    }
}

impl Constraints {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value, lo, hi) in [
            ("ground_clearance_m", self.ground_clearance_m, 0.0, 5000.0),
            ("wall_distance_m", self.wall_distance_m, 0.0, 5000.0),
            ("softness", self.softness, 0.0, 1.0),
            ("look_ahead_s", self.look_ahead_s, 0.0, 5.0),
            ("reference_z_m", self.reference_z_m, -10_000.0, 20_000.0),
        ] {
            if !value.is_finite() || !(lo..=hi).contains(&value) {
                return Err(format!("rig {name} must be in {lo}..={hi}, got {value}"));
            }
        }
        Ok(())
    }
}

/// Complete camera rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraRig {
    pub enabled: bool,
    pub name: String,
    pub duration_s: f32,
    /// Follow the authored curve exactly at the key times, instead of letting
    /// the motion profile's spring lag behind it. Profiles still drive roll,
    /// shake and look smoothing; this flag makes key/time/distance authoritative.
    #[serde(default = "default_exact_path")]
    pub exact_path: bool,
    pub fps: u32,
    pub eye: Path3D,
    pub target: TargetRig,
    pub optics: OpticalRig,
    pub body: MotionProfile,
    pub constraints: Constraints,
    pub seed: u64,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "Camera rig".into(),
            duration_s: 8.0,
            exact_path: true,
            fps: 24,
            eye: Path3D::default(),
            target: TargetRig::default(),
            optics: OpticalRig::default(),
            body: MotionProfile::default(),
            constraints: Constraints::default(),
            seed: 7,
        }
    }
}

fn default_exact_path() -> bool {
    true
}

impl CameraRig {
    pub fn preset(name: &str) -> Option<Self> {
        let preset = AircraftPreset::parse(name)?;
        let mut rig = Self {
            enabled: true,
            name: format!("Rig {}", preset.label()),
            ..Self::default()
        };
        rig.body = MotionProfile::preset(preset);
        match preset {
            AircraftPreset::DroneFpv => {
                rig.optics.fov_y_deg = 78.0;
                rig.constraints.ground_clearance_m = 18.0;
                rig.optics.fstop = 2.8;
            }
            AircraftPreset::Bird => {
                rig.optics.fov_y_deg = 42.0;
                rig.constraints.ground_clearance_m = 35.0;
                rig.optics.fstop = 5.6;
            }
            AircraftPreset::Helicopter => {
                rig.optics.fov_y_deg = 35.0;
                rig.constraints.ground_clearance_m = 70.0;
                rig.optics.fstop = 8.0;
            }
            AircraftPreset::Airship => {
                rig.optics.fov_y_deg = 30.0;
                rig.constraints.ground_clearance_m = 160.0;
                rig.optics.fstop = 11.0;
            }
            AircraftPreset::FighterJet => {
                rig.optics.fov_y_deg = 62.0;
                rig.constraints.ground_clearance_m = 30.0;
                rig.optics.fstop = 4.0;
            }
            AircraftPreset::Manual => {
                rig.optics.fov_y_deg = 50.0;
                rig.constraints.enabled = false;
            }
        }
        Some(rig)
    }

    /// Scale every authored key time by `factor`, keeping positions and
    /// handles untouched. This is how a duration change slows down or speeds
    /// up the whole flight instead of letting the camera reach the last key
    /// early and wait there.
    pub fn scale_path_times(&mut self, factor: f32) -> bool {
        if !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        let mut changed = false;
        for key in &mut self.eye.keys {
            let time = key.time_s * factor;
            if time.is_finite() && (time - key.time_s).abs() > 1e-6 {
                key.time_s = time;
                changed = true;
            }
        }
        for key in &mut self.target.path.keys {
            let time = key.time_s * factor;
            if time.is_finite() && (time - key.time_s).abs() > 1e-6 {
                key.time_s = time;
                changed = true;
            }
        }
        changed
    }

    /// Set the clip duration and stretch the authored paths to fill it.
    ///
    /// The factor is computed from the **end of the authored path**, not from
    /// the previous duration: this repairs projects whose duration was changed
    /// without retiming the keys (the camera used to reach the end at the old
    /// key time and then stand still). Positions are never modified.
    pub fn stretch_duration(&mut self, new_duration: f32) -> bool {
        if !new_duration.is_finite() {
            return false;
        }
        let new_duration = new_duration.clamp(0.1, 600.0);
        let path_end = self
            .eye
            .duration_s()
            .max(if self.target.mode == TargetMode::Path {
                self.target.path.duration_s()
            } else {
                0.0
            });
        let mut changed = false;
        if path_end > 1e-6 && (new_duration - path_end).abs() > 1e-5 {
            changed |= self.scale_path_times(new_duration / path_end);
        }
        if (self.duration_s - new_duration).abs() > 1e-6 {
            self.duration_s = new_duration;
            changed = true;
        }
        changed
    }

    pub fn frame_count(&self) -> usize {
        ((self.duration_s * self.fps as f32).round() as usize).clamp(1, 200_000)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.duration_s.is_finite() || !(0.1..=600.0).contains(&self.duration_s) {
            return Err("rig duration must be in 0.1..=600 seconds".into());
        }
        if !(1..=120).contains(&self.fps) {
            return Err("rig fps must be in 1..=120".into());
        }
        self.eye.validate()?;
        if self.target.mode == TargetMode::Path {
            self.target.path.validate()?;
        }
        if !(1.0..=170.0).contains(&self.optics.fov_y_deg) {
            return Err("rig fov must be in 1..=170 degrees".into());
        }
        if self.optics.near <= 0.0 || self.optics.far <= self.optics.near {
            return Err("invalid rig near/far planes".into());
        }
        if !self.optics.fstop.is_finite() || self.optics.fstop <= 0.0 {
            return Err("rig fstop must be positive".into());
        }
        self.body.validate()?;
        self.constraints.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretching_duration_fills_the_clip_without_moving_positions() {
        let mut rig = CameraRig::preset("drone").unwrap();
        rig.duration_s = 50.0;
        rig.eye.keys = vec![
            PathKey::new(0.0, [0.0, 0.0, 500.0]),
            PathKey::new(25.0, [1000.0, 0.0, 500.0]),
            PathKey::new(50.0, [2000.0, 0.0, 500.0]),
        ];
        let positions: Vec<[f64; 3]> = rig.eye.keys.iter().map(|key| key.position).collect();
        assert!(rig.stretch_duration(20.0));
        assert_eq!(rig.duration_s, 20.0);
        assert!((rig.eye.keys[1].time_s - 10.0).abs() < 1e-4);
        assert!((rig.eye.keys[2].time_s - 20.0).abs() < 1e-4);
        assert_eq!(
            positions,
            rig.eye
                .keys
                .iter()
                .map(|key| key.position)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn extending_duration_after_a_pause_does_not_double_the_path() {
        let mut rig = CameraRig::preset("drone").unwrap();
        rig.duration_s = 10.0;
        rig.eye.keys = vec![
            PathKey::new(0.0, [0.0, 0.0, 500.0]),
            PathKey::new(25.0, [1000.0, 0.0, 500.0]),
            PathKey::new(50.0, [2000.0, 0.0, 500.0]),
        ];
        // The clip is shorter than the path: extending to the last key should
        // simply reveal it, not stretch it a second time.
        assert!(rig.stretch_duration(50.0));
        assert_eq!(rig.duration_s, 50.0);
        assert!((rig.eye.keys[2].time_s - 50.0).abs() < 1e-4);
    }

    #[test]
    fn presets_are_valid() {
        for preset in AircraftPreset::ALL {
            let rig = CameraRig::preset(preset.name()).expect("known preset");
            rig.validate().unwrap();
            assert_eq!(rig.body.preset, preset);
            assert!(rig.enabled);
        }
        assert!(CameraRig::preset("nope").is_none());
    }

    #[test]
    fn path_validation_rejects_bad_times() {
        let mut path = Path3D::default();
        path.keys[0].time_s = 5.0;
        assert!(path.validate().is_err());
    }

    #[test]
    fn new_keys_use_automatic_handles() {
        let key = PathKey::new(2.0, [10.0, 20.0, 30.0]);
        assert_eq!(key.in_handle, [0.0; 3]);
        assert_eq!(key.out_handle, [0.0; 3]);
        assert_eq!(PathKey::default().out_handle[0], 300.0);
    }

    #[test]
    fn default_rig_is_disabled_and_valid() {
        let rig = CameraRig::default();
        assert!(!rig.enabled);
        rig.validate().unwrap();
        assert_eq!(rig.frame_count(), 192);
    }
}

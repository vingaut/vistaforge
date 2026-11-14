//! Aerial camera flight settings: FPV drone and helicopter dynamics.
//!
//! The module does not store keyframes. A flight is described by a path
//! pattern, a speed, an altitude above ground and a handful of dynamics knobs;
//! the camera track is simulated deterministically at a fixed time step.

use serde::{Deserialize, Serialize};

/// Shape of the automatically generated flight path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FlightPattern {
    /// Circle around the terrain.
    #[default]
    Orbit,
    /// Oval racetrack, straights and banked turns.
    Racetrack,
    /// Low pass along a ridge line.
    RidgeRun,
    /// High entry, dive toward the centre, pull up.
    Dive,
}

/// Camera dynamics and path parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlightSettings {
    pub enabled: bool,
    /// Last applied preset, informational.
    pub preset: String,
    pub pattern: FlightPattern,
    /// Clip length, seconds.
    pub duration_s: f32,
    pub fps: u32,
    /// Cruise speed, metres per second.
    pub speed_mps: f32,
    /// Height above ground, metres.
    pub altitude_agl_m: f32,
    /// How far ahead the camera looks, seconds of travel.
    pub look_ahead_s: f32,
    /// Bank response, 0..1.
    pub bank_gain: f32,
    /// Maximum bank angle, degrees.
    pub max_bank_deg: f32,
    /// Attitude smoothing, 0 = twitchy, 1 = heavy.
    pub smoothness: f32,
    /// Fixed camera uptilt on the airframe, degrees.
    pub uptilt_deg: f32,
    /// Camera pitch bias, degrees. Negative looks down: the landscape framing
    /// knob.
    pub pitch_bias_deg: f32,
    /// Rotational shake, degrees RMS.
    pub shake_deg: f32,
    /// Gust strength, 0..1.
    pub turbulence: f32,
    /// Gimbal-stabilised camera: roll is levelled and pitch is damped.
    pub gimbal: bool,
    /// Aim the camera at the terrain centre instead of along the path. This is
    /// the classic orbit shot: the aircraft flies sideways while the gimbal
    /// keeps the subject framed.
    pub look_at_center: bool,
    pub seed: u64,
}

impl Default for FlightSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: "cinelifter".into(),
            pattern: FlightPattern::Orbit,
            duration_s: 8.0,
            fps: 24,
            speed_mps: 18.0,
            altitude_agl_m: 28.0,
            look_ahead_s: 0.18,
            bank_gain: 0.85,
            max_bank_deg: 45.0,
            smoothness: 0.28,
            uptilt_deg: 30.0,
            pitch_bias_deg: 0.0,
            shake_deg: 0.12,
            turbulence: 0.35,
            gimbal: false,
            look_at_center: false,
            seed: 7,
        }
    }
}

impl FlightSettings {
    pub fn preset(name: &str) -> Option<Self> {
        let key = name.to_ascii_lowercase();
        let mut f = Self {
            enabled: true,
            preset: key.clone(),
            ..Self::default()
        };
        match key.as_str() {
            "cinelifter" => {}
            "freestyle" => {
                f.speed_mps = 26.0;
                f.altitude_agl_m = 24.0;
                f.smoothness = 0.10;
                f.max_bank_deg = 70.0;
                f.uptilt_deg = 35.0;
                f.shake_deg = 0.24;
                f.turbulence = 0.55;
                f.pattern = FlightPattern::RidgeRun;
                f.pitch_bias_deg = -3.0;
            }
            "racing" => {
                f.speed_mps = 34.0;
                f.altitude_agl_m = 14.0;
                f.smoothness = 0.04;
                f.max_bank_deg = 78.0;
                f.uptilt_deg = 40.0;
                f.shake_deg = 0.30;
                f.turbulence = 0.65;
                f.pattern = FlightPattern::Racetrack;
            }
            "helicopter" => {
                f.speed_mps = 12.0;
                f.altitude_agl_m = 65.0;
                f.smoothness = 0.72;
                f.max_bank_deg = 22.0;
                f.uptilt_deg = 10.0;
                f.shake_deg = 0.08;
                f.turbulence = 0.45;
                f.gimbal = true;
                f.look_at_center = true;
                f.pattern = FlightPattern::Orbit;
                f.altitude_agl_m = 250.0;
                f.pitch_bias_deg = -8.0;
            }
            "cinematic" => {
                f.speed_mps = 9.0;
                f.altitude_agl_m = 90.0;
                f.smoothness = 0.86;
                f.max_bank_deg = 14.0;
                f.uptilt_deg = 4.0;
                f.shake_deg = 0.04;
                f.turbulence = 0.25;
                f.gimbal = true;
                f.look_at_center = true;
                f.pattern = FlightPattern::Orbit;
                f.altitude_agl_m = 700.0;
                f.pitch_bias_deg = -13.0;
            }
            "dive" => {
                f.speed_mps = 30.0;
                f.altitude_agl_m = 20.0;
                f.smoothness = 0.15;
                f.max_bank_deg = 60.0;
                f.uptilt_deg = 35.0;
                f.shake_deg = 0.18;
                f.turbulence = 0.5;
                f.pattern = FlightPattern::Dive;
                f.pitch_bias_deg = -6.0;
            }
            _ => return None,
        }
        Some(f)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(0.5..=600.0).contains(&self.duration_s) {
            return Err("flight duration must be in 0.5..=600 seconds".into());
        }
        if !(1..=120).contains(&self.fps) {
            return Err("flight fps must be in 1..=120".into());
        }
        for (name, value, lo, hi) in [
            ("speed_mps", self.speed_mps, 0.5, 90.0),
            ("altitude_agl_m", self.altitude_agl_m, 1.0, 3000.0),
            ("look_ahead_s", self.look_ahead_s, 0.0, 3.0),
            ("bank_gain", self.bank_gain, 0.0, 2.0),
            ("max_bank_deg", self.max_bank_deg, 0.0, 85.0),
            ("smoothness", self.smoothness, 0.0, 1.0),
            ("uptilt_deg", self.uptilt_deg, 0.0, 60.0),
            ("pitch_bias_deg", self.pitch_bias_deg, -45.0, 30.0),
            ("shake_deg", self.shake_deg, 0.0, 2.0),
            ("turbulence", self.turbulence, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(lo..=hi).contains(&value) {
                return Err(format!("flight {name} must be in {lo}..={hi}, got {value}"));
            }
        }
        Ok(())
    }

    /// Number of frames in the clip.
    pub fn frame_count(&self) -> usize {
        ((self.duration_s * self.fps as f32).round() as usize).clamp(1, 100_000)
    }
}

pub const FLIGHT_PRESETS: &[&str] = &[
    "cinelifter",
    "freestyle",
    "racing",
    "helicopter",
    "cinematic",
    "dive",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_distinct() {
        for name in FLIGHT_PRESETS {
            let f = FlightSettings::preset(name).expect("known preset");
            f.validate().unwrap();
            assert_eq!(f.preset, *name);
        }
        let a = FlightSettings::preset("racing").unwrap();
        let b = FlightSettings::preset("cinematic").unwrap();
        assert!(a.speed_mps > b.speed_mps);
        assert!(a.max_bank_deg > b.max_bank_deg);
        assert!(FlightSettings::preset("nope").is_none());
    }

    #[test]
    fn frame_count_matches_duration() {
        let f = FlightSettings {
            duration_s: 8.0,
            fps: 24,
            ..FlightSettings::default()
        };
        assert_eq!(f.frame_count(), 192);
    }
}

//! Sample an authored [`CameraRig`] into a camera.
//!
//! The simulation is deterministic and stateful: start at t=0 and advance in
//! fixed steps. The point of view follows the Bezier path through a
//! critically-damped spring whose acceleration is clamped per body axis; the
//! camera can lag on a second spring; bank comes from lateral acceleration and
//! the explicit roll curve. Ground clearance is corrected inside the loop, so
//! a low pass over a summit never tunnels through the terrain.

use glam::DVec3;
use vf_core::{
    AircraftPreset, Camera, CameraLink, CameraRig, HeightField, MotionProfile, Path3D, Seed,
    TargetMode,
};
use vf_procgen::Noise;

use crate::clearance::ClearanceField;

const UP: DVec3 = DVec3::Z;

#[derive(Debug, Clone)]
pub struct RigSample {
    pub camera: Camera,
    pub eye: DVec3,
    pub target: DVec3,
    pub speed_mps: f32,
    pub bank_deg: f32,
    pub roll_deg: f32,
    pub altitude_agl_m: f32,
    pub wall_distance_m: f32,
    pub focus_distance_m: f32,
    pub fstop: f32,
    pub dof_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct RigSimulation<'a> {
    rig: &'a CameraRig,
    hf: &'a HeightField,
    time: f32,
    dt: f32,
    eye_pos: DVec3,
    eye_vel: DVec3,
    eye_acc: DVec3,
    cam_pos: DVec3,
    cam_vel: DVec3,
    roll: f32,
    target_pos: DVec3,
    shake: Noise,
    clearance: ClearanceField,
}

impl<'a> RigSimulation<'a> {
    pub fn new(rig: &'a CameraRig, hf: &'a HeightField, seed: Seed) -> Result<Self, String> {
        rig.validate()?;
        if hf.width() < 2 || hf.height() < 2 {
            return Err("empty heightfield".into());
        }
        let eye_pos = sample_path(&rig.eye, 0.0);
        let target_pos = initial_target(rig, eye_pos);
        let cam_pos = eye_pos + DVec3::from_array(rig.optics.offset);
        let dt = (1.0 / 120.0f32).min(1.0 / rig.fps.max(1) as f32);
        Ok(Self {
            rig,
            hf,
            time: 0.0,
            dt,
            eye_pos,
            eye_vel: DVec3::ZERO,
            eye_acc: DVec3::ZERO,
            cam_pos,
            cam_vel: DVec3::ZERO,
            roll: 0.0,
            target_pos,
            shake: Noise::new(seed.derive("rig-shake")),
            clearance: ClearanceField::new(hf),
        })
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    /// Advance the simulation to `time_s` and sample the camera.
    ///
    /// Going backwards (timeline scrubbing) rewinds deterministically to t=0
    /// instead of returning a stale state.
    pub fn sample_at(&mut self, time_s: f32) -> RigSample {
        if time_s + 1e-4 < self.time {
            self.reset();
        }
        let end = time_s.clamp(0.0, self.rig.duration_s);
        // Never take a giant catch-up step: cap each loop to a handful of
        // frames so a jump in the timeline still lands deterministically.
        while self.time + self.dt <= end + 1e-6 {
            self.step(self.dt);
        }
        self.current_sample()
    }

    fn reset(&mut self) {
        self.time = 0.0;
        self.eye_pos = sample_path(&self.rig.eye, 0.0);
        self.eye_vel = DVec3::ZERO;
        self.eye_acc = DVec3::ZERO;
        self.cam_pos = self.eye_pos + DVec3::from_array(self.rig.optics.offset);
        self.cam_vel = DVec3::ZERO;
        self.roll = 0.0;
        self.target_pos = initial_target(self.rig, self.eye_pos);
    }

    fn step(&mut self, dt: f32) {
        let t = (self.time + dt).min(self.rig.duration_s);
        let dt64 = f64::from(dt);
        let desired = sample_path(&self.rig.eye, t);
        let path_velocity = path_derivative(&self.rig.eye, t);
        let body = &self.rig.body;
        let omega = response_omega(body.preset);
        // Position spring + velocity feed-forward. Without the feed-forward
        // term the spring fights the path velocity, and once the acceleration
        // clamp saturates the damping collapses: fast bodies then limit-cycle
        // around the curve instead of following it.
        let accel = (desired - self.eye_pos) * (omega * omega)
            + (path_velocity - self.eye_vel) * (2.0 * omega);
        // Manual mode ignores the spring and follows the authored path exactly.
        if body.manual_orientation {
            self.eye_vel = path_velocity;
            self.eye_pos = desired;
            self.eye_acc = DVec3::ZERO;
        } else {
            let forward = if self.eye_vel.length_squared() > 0.01 {
                self.eye_vel.normalize()
            } else {
                path_velocity.normalize_or_zero()
            };
            let forward = if forward.length_squared() < 0.5 {
                DVec3::X
            } else {
                forward
            };
            let right = forward.cross(UP).normalize_or_zero();
            let up = right.cross(forward).normalize_or_zero();
            let mut a_f = accel.dot(forward);
            let mut a_r = accel.dot(right);
            let mut a_u = accel.dot(up);
            a_f = a_f.clamp(
                -f64::from(body.accel_forward_mps2),
                f64::from(body.accel_forward_mps2),
            );
            a_r = a_r.clamp(
                -f64::from(body.accel_lateral_mps2),
                f64::from(body.accel_lateral_mps2),
            );
            a_u = a_u.clamp(
                -f64::from(body.accel_vertical_mps2),
                f64::from(body.accel_vertical_mps2),
            );
            let next = forward * a_f + right * a_r + up * a_u;
            let delta = (next - self.eye_acc).length();
            let max_delta = f64::from(body.jerk_mps3) * dt64;
            let next = if delta > max_delta && delta > 1e-6 {
                self.eye_acc + (next - self.eye_acc) * (max_delta / delta)
            } else {
                next
            };
            self.eye_acc = next;
            self.eye_vel += next * dt64;
            self.eye_vel *= (-f64::from(body.drag) * dt64).exp();
            let speed = self.eye_vel.length();
            if speed > f64::from(body.max_speed_mps) {
                self.eye_vel *= f64::from(body.max_speed_mps) / speed;
            }
            self.eye_pos += self.eye_vel * dt64;
        }

        // Ground / reference-plane clearance. `apply_eye` and friends decide
        // which objects are constrained.
        if self.rig.constraints.enabled && self.rig.constraints.apply_eye {
            let floor =
                self.floor_at(self.eye_pos) + f64::from(self.rig.constraints.ground_clearance_m);
            if self.eye_pos.z < floor {
                let soft = f64::from(self.rig.constraints.softness);
                self.eye_pos.z += (floor - self.eye_pos.z) * (1.0 - soft);
                if self.eye_vel.z < 0.0 {
                    self.eye_vel.z *= soft;
                }
            }
        }

        if self.rig.constraints.enabled && self.rig.constraints.wall_avoidance {
            let threshold = f64::from(self.rig.constraints.wall_distance_m);
            if threshold > 0.0 {
                let ahead =
                    self.eye_pos + self.eye_vel * f64::from(self.rig.constraints.look_ahead_s);
                let (distance, gx, gy) = self.clearance.sample(ahead.x, ahead.y);
                let distance = f64::from(distance);
                if distance.is_finite() && distance < threshold {
                    let pressure = (1.0 - distance / threshold).clamp(0.0, 1.0);
                    let push_dir =
                        DVec3::new(f64::from(gx), f64::from(gy), 0.0).normalize_or_zero();
                    // A lateral acceleration of up to ~18 m/s², applied to the
                    // velocity: smooth enough for a drone, strong enough to
                    // read as an evasive move.
                    let push = push_dir * (18.0 * pressure * dt64);
                    self.eye_vel += push;
                    if self.rig.constraints.apply_camera {
                        self.cam_pos += push * dt64;
                    }
                }
            }
        }

        self.target_pos = sample_target(self.rig, t, self.eye_pos, self.eye_vel);
        if self.rig.constraints.enabled && self.rig.constraints.apply_target {
            let floor =
                self.floor_at(self.target_pos) + f64::from(self.rig.constraints.ground_clearance_m);
            self.target_pos.z = self.target_pos.z.max(floor);
        }

        // Camera spring.
        let desired_cam = self.eye_pos + DVec3::from_array(self.rig.optics.offset);
        if self.rig.optics.link == CameraLink::Spring {
            let k = f64::from(self.rig.optics.spring_stiffness.max(0.1));
            let d = f64::from(self.rig.optics.spring_damping.max(0.0)) * 2.0 * k.sqrt();
            let a = (desired_cam - self.cam_pos) * k - self.cam_vel * d;
            self.cam_vel += a * dt64;
            self.cam_pos += self.cam_vel * dt64;
        } else {
            self.cam_pos = desired_cam;
            self.cam_vel = self.eye_vel;
        }
        if self.rig.constraints.enabled && self.rig.constraints.apply_camera {
            let floor =
                self.floor_at(self.cam_pos) + f64::from(self.rig.constraints.ground_clearance_m);
            self.cam_pos.z = self.cam_pos.z.max(floor);
        }

        // Bank from lateral acceleration, then the explicit roll and shake.
        let forward = (self.target_pos - self.cam_pos).normalize_or_zero();
        let right = forward.cross(UP).normalize_or_zero();
        let lateral = self.eye_acc.dot(right) as f32;
        let bank_target = if self.rig.body.preset == AircraftPreset::Manual {
            0.0
        } else {
            (lateral.atan2(9.81) * self.rig.body.bank_gain)
                .to_degrees()
                .clamp(-self.rig.body.max_bank_deg, self.rig.body.max_bank_deg)
        };
        let response = self.rig.body.roll_response.max(0.1) * dt;
        self.roll += (bank_target - self.roll) * response.min(1.0);

        self.time = t;
    }

    fn floor_at(&self, p: DVec3) -> f64 {
        let mut floor = f64::from(self.terrain_height(p.x, p.y));
        if self.rig.constraints.reference_enabled {
            floor = floor.max(f64::from(self.rig.constraints.reference_z_m));
        }
        floor
    }

    fn terrain_height(&self, x: f64, y: f64) -> f32 {
        let (ox, oy) = self.hf.origin();
        let (sx, sy) = self.hf.spacing();
        let gx = (x - ox) / sx;
        let gy = (y - oy) / sy;
        self.hf.sample_bilinear(gx, gy)
    }

    fn current_sample(&self) -> RigSample {
        let target = self.target_pos + DVec3::from_array(self.rig.target.offset);
        let mut forward = (target - self.cam_pos).normalize_or_zero();
        if forward.length_squared() < 0.5 {
            forward = DVec3::Y;
        }
        let mut right = forward.cross(UP).normalize_or_zero();
        if right.length_squared() < 0.5 {
            right = DVec3::X;
        }
        let mut up = right.cross(forward).normalize_or_zero();

        // Shake: deterministic low-frequency noise, applied as small yaw/pitch
        // perturbations before the roll.
        let shake = &self.rig.body;
        if shake.shake_deg > 0.0 && shake.turbulence > 0.0 {
            let t = self.time * 2.1;
            let yaw =
                f64::from(self.shake.fbm2(t, 0.0, 4, 2.0, 0.5) * shake.shake_deg.to_radians());
            let pitch = f64::from(
                self.shake.fbm2(t + 13.7, 4.2, 4, 2.0, 0.5) * shake.shake_deg.to_radians() * 0.7,
            );
            let (s, c) = yaw.sin_cos();
            let yaw_rot = DVec3::new(
                c * forward.x - s * forward.z,
                forward.y,
                s * forward.x + c * forward.z,
            );
            forward = yaw_rot;
            let (s, c) = pitch.sin_cos();
            let pitch_rot = DVec3::new(
                forward.x,
                c * forward.y - s * forward.z,
                s * forward.y + c * forward.z,
            );
            forward = pitch_rot;
            right = forward.cross(UP).normalize_or_zero();
            up = right.cross(forward).normalize_or_zero();
        }

        let roll = (self.roll + self.rig.optics.roll_deg).to_radians() as f64;
        let (s, c) = roll.sin_cos();
        let rolled_up = up * c + right * s;
        let rolled_right = right * c - up * s;
        let up = rolled_up.normalize_or_zero();
        let _ = rolled_right;
        let focus_distance = if self.rig.optics.focus_on_target {
            (target - self.cam_pos).length() as f32
        } else {
            self.rig.optics.focus_distance_m.max(0.1)
        };
        let altitude =
            self.eye_pos.z - f64::from(self.terrain_height(self.eye_pos.x, self.eye_pos.y));
        let (wall_distance, _, _) = self.clearance.sample(self.eye_pos.x, self.eye_pos.y);
        RigSample {
            camera: Camera {
                position: self.cam_pos,
                target: self.cam_pos + forward,
                up,
                fov_y_deg: self.rig.optics.fov_y_deg,
                near: self.rig.optics.near,
                far: self.rig.optics.far,
                focal_length_mm: None,
                sensor_mm: 36.0,
            },
            eye: self.eye_pos,
            target,
            speed_mps: self.eye_vel.length() as f32,
            bank_deg: self.roll,
            roll_deg: (self.roll + self.rig.optics.roll_deg),
            altitude_agl_m: altitude.max(0.0) as f32,
            wall_distance_m: wall_distance,
            focus_distance_m: focus_distance,
            fstop: self.rig.optics.fstop,
            dof_enabled: self.rig.optics.dof_enabled,
        }
    }
}

/// Convenience one-shot sample; use [`RigSimulation`] for sequences.
pub fn sample_rig(
    rig: &CameraRig,
    hf: &HeightField,
    seed: Seed,
    time_s: f32,
) -> Result<RigSample, String> {
    let mut simulation = RigSimulation::new(rig, hf, seed)?;
    Ok(simulation.sample_at(time_s))
}

fn response_omega(preset: AircraftPreset) -> f64 {
    let response_s = match preset {
        AircraftPreset::DroneFpv => 1.2,
        AircraftPreset::Bird => 1.6,
        AircraftPreset::Helicopter => 3.2,
        AircraftPreset::Airship => 7.0,
        AircraftPreset::FighterJet => 1.0,
        AircraftPreset::Manual => 0.02,
    };
    // `1 / response_s`, not `TAU / response_s`: the latter makes the spring
    // six times stiffer than the advertised response and the acceleration
    // clamp turns it into a bang-bang controller that limit-cycles.
    1.0 / response_s
}

fn initial_target(rig: &CameraRig, eye: DVec3) -> DVec3 {
    match rig.target.mode {
        TargetMode::Fixed => DVec3::from_array(rig.target.fixed),
        TargetMode::Path => sample_path(&rig.target.path, 0.0),
        TargetMode::EyeAhead | TargetMode::Manual => eye + DVec3::new(0.0, 400.0, -120.0),
    }
}

fn sample_target(rig: &CameraRig, t: f32, eye: DVec3, velocity: DVec3) -> DVec3 {
    match rig.target.mode {
        TargetMode::Fixed => DVec3::from_array(rig.target.fixed),
        TargetMode::Path => sample_path(&rig.target.path, t),
        TargetMode::EyeAhead => {
            let ahead = if velocity.length_squared() > 0.01 {
                velocity * f64::from(rig.target.lead_s)
            } else {
                DVec3::new(0.0, 400.0, -100.0)
            };
            eye + ahead
        }
        TargetMode::Manual => {
            if velocity.length_squared() > 0.01 {
                eye + velocity.normalize() * 500.0
            } else {
                eye + DVec3::new(0.0, 400.0, -100.0)
            }
        }
    }
}

/// Cubic Bezier sample by time.
pub fn sample_path(path: &Path3D, t: f32) -> DVec3 {
    let keys = &path.keys;
    if keys.is_empty() {
        return DVec3::ZERO;
    }
    if keys.len() == 1 || t <= keys[0].time_s {
        return DVec3::from_array(keys[0].position);
    }
    let last = &keys[keys.len() - 1];
    if t >= last.time_s {
        return DVec3::from_array(last.position);
    }
    let mut index = 0;
    for i in 0..keys.len() - 1 {
        if t >= keys[i].time_s && t <= keys[i + 1].time_s {
            index = i;
            break;
        }
    }
    let a = &keys[index];
    let b = &keys[index + 1];
    let span = (b.time_s - a.time_s).max(1e-5);
    let u = ((t - a.time_s) / span).clamp(0.0, 1.0);
    let u = u + (u * u * (3.0 - 2.0 * u) - u) * a.ease.clamp(0.0, 1.0);
    let p0 = DVec3::from_array(a.position);
    let p1 = DVec3::from_array(b.position);
    let (h0, h1) = auto_handles(keys, index);
    let c1 = p0
        + DVec3::from_array(if a.out_handle == [0.0; 3] {
            h0
        } else {
            a.out_handle
        });
    let c2 = p1
        + DVec3::from_array(if b.in_handle == [0.0; 3] {
            h1
        } else {
            b.in_handle
        });
    let u = u as f64;
    let v = 1.0 - u;
    p0 * (v * v * v) + c1 * (3.0 * v * v * u) + c2 * (3.0 * v * u * u) + p1 * (u * u * u)
}

fn auto_handles(keys: &[vf_core::PathKey], index: usize) -> ([f64; 3], [f64; 3]) {
    let p0 = DVec3::from_array(keys[index].position);
    let p1 = DVec3::from_array(keys[index + 1].position);
    let prev = if index > 0 {
        DVec3::from_array(keys[index - 1].position)
    } else {
        p0 * 2.0 - p1
    };
    let next = if index + 2 < keys.len() {
        DVec3::from_array(keys[index + 2].position)
    } else {
        p1 * 2.0 - p0
    };
    let m0 = (p1 - prev) * 0.5;
    let m1 = (next - p0) * 0.5;
    ((m0 / 3.0).to_array(), (-m1 / 3.0).to_array())
}

/// Approximate authored path length, metres.
///
/// The curve is sampled uniformly in time; for the smooth, non-looping paths
/// authored in the editor this is accurate enough to guide the duration (the
/// error is far below the difference between a drone and an airship).
pub fn path_length(path: &Path3D) -> f64 {
    let duration = path.duration_s();
    if !duration.is_finite() || duration <= 0.0 {
        return 0.0;
    }
    let samples = 256;
    let mut previous = sample_path(path, 0.0);
    let mut length = 0.0;
    for index in 1..=samples {
        let t = duration * index as f32 / samples as f32;
        let point = sample_path(path, t);
        length += (point - previous).length();
        previous = point;
    }
    length
}

/// Axis-aligned centre of the authored path, used to frame the design camera
/// on a short demonstration loop.
pub fn path_center(path: &Path3D) -> DVec3 {
    if path.keys.is_empty() {
        return DVec3::ZERO;
    }
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for key in &path.keys {
        let p = DVec3::from_array(key.position);
        min = min.min(p);
        max = max.max(p);
    }
    (min + max) * 0.5
}

/// Cruise speed used to translate a path length into a duration. It leaves
/// headroom for acceleration and banking instead of asking the body to fly at
/// its absolute maximum for the whole clip.
pub fn cruise_speed_mps(profile: &MotionProfile) -> f32 {
    (profile.max_speed_mps * 0.75).max(0.5)
}

/// Duration at which the current body can follow the authored path at cruise
/// speed. Clamped to the rig's legal range.
pub fn recommended_duration_s(rig: &CameraRig) -> f32 {
    let length = path_length(&rig.eye);
    let cruise = f64::from(cruise_speed_mps(&rig.body));
    (length / cruise).clamp(2.0, 600.0) as f32
}

/// Average speed the body would have to sustain to follow the path in the
/// current duration, m/s.
pub fn required_speed_mps(rig: &CameraRig) -> f32 {
    let length = path_length(&rig.eye);
    if rig.duration_s <= 1e-4 {
        return 0.0;
    }
    (length / f64::from(rig.duration_s)) as f32
}

/// How far the current duration is from a comfortable one, as a factor.
/// 1.0 means the path is already at cruise speed; 4.0 means it is four times
/// too fast (the body will saturate and lag behind the curve).
pub fn duration_pressure(rig: &CameraRig) -> f32 {
    let required = required_speed_mps(rig);
    let cruise = cruise_speed_mps(&rig.body);
    if cruise <= 0.0 {
        1.0
    } else {
        required / cruise
    }
}

/// Finite-difference derivative of the path, metres per second.
pub fn path_derivative(path: &Path3D, t: f32) -> DVec3 {
    let eps = 0.01;
    let t0 = (t - eps).max(0.0);
    let t1 = (t + eps).min(path.duration_s().max(eps));
    if t1 <= t0 {
        return DVec3::ZERO;
    }
    (sample_path(path, t1) - sample_path(path, t0)) / f64::from(t1 - t0)
}

/// Replace the rig path with a terrain-fitted pattern.
pub fn preset_path(rig: &mut CameraRig, hf: &HeightField, pattern: &str) -> Result<(), String> {
    let bounds = hf.bounds();
    let center = DVec3::new(
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        bounds.max[2],
    );
    let extent = (bounds.max[0] - bounds.min[0]).max(bounds.max[1] - bounds.min[1]);
    let radius = extent * 0.38;
    let base_z = bounds.min[2] + (bounds.max[2] - bounds.min[2]) * 0.55;
    let (keys, target) = match pattern.to_ascii_lowercase().as_str() {
        "orbit" => {
            let keys = vec![
                vf_core::PathKey::new(0.0, [center.x + radius, center.y, base_z + 300.0]),
                vf_core::PathKey::new(2.7, [center.x, center.y + radius, base_z + 420.0]),
                vf_core::PathKey::new(5.4, [center.x - radius, center.y, base_z + 300.0]),
                vf_core::PathKey::new(8.0, [center.x, center.y - radius, base_z + 420.0]),
            ];
            (keys, center + DVec3::new(0.0, 0.0, 100.0))
        }
        "ridge" => {
            let keys = vec![
                vf_core::PathKey::new(
                    0.0,
                    [center.x - radius, center.y - radius * 0.5, base_z + 150.0],
                ),
                vf_core::PathKey::new(4.0, [center.x, center.y, base_z + 250.0]),
                vf_core::PathKey::new(
                    8.0,
                    [center.x + radius, center.y + radius * 0.5, base_z + 150.0],
                ),
            ];
            (keys, center + DVec3::new(radius * 2.0, 0.0, -100.0))
        }
        "dive" => {
            let keys = vec![
                vf_core::PathKey::new(0.0, [center.x - radius, center.y - radius, base_z + 900.0]),
                vf_core::PathKey::new(4.0, [center.x, center.y, base_z + 250.0]),
                vf_core::PathKey::new(8.0, [center.x + radius, center.y + radius, base_z + 700.0]),
            ];
            (keys, center)
        }
        "demo" => {
            // A short S-curve over the centre of the terrain. Its length is
            // matched to the current body at cruise speed, so the comparison
            // is about inertia and cornering rather than about saturation.
            let extent = extent.max(1.0);
            let max_speed = f64::from(rig.body.max_speed_mps.max(0.5));
            // A slow airship gets a smaller S, a fighter a longer one, so
            // every demo lasts roughly the same handful of seconds at 90 % of
            // the body's top speed. Slow bodies then visibly lag behind.
            let span = (max_speed * 6.0).clamp(60.0, (extent * 0.05).clamp(60.0, 500.0));
            let sample_ground = |x: f64, y: f64| {
                let (ox, oy) = hf.origin();
                let (sx, sy) = hf.spacing();
                let gx = ((x - ox) / sx).clamp(0.0, (hf.width() - 1) as f64);
                let gy = ((y - oy) / sy).clamp(0.0, (hf.height() - 1) as f64);
                f64::from(hf.sample_bilinear(gx, gy))
            };
            let mut points = [
                (center.x - span, center.y - span * 1.05),
                (center.x - span * 0.3, center.y + span * 1.05),
                (center.x + span * 0.3, center.y - span * 1.05),
                (center.x + span, center.y + span * 1.05),
            ];
            let mut keys = Vec::with_capacity(4);
            for (index, (x, y)) in points.iter_mut().enumerate() {
                let ground = sample_ground(*x, *y);
                let altitude = 150.0 + index as f64 * 20.0;
                keys.push(vf_core::PathKey::new(
                    index as f32 / 3.0,
                    [*x, *y, ground + altitude],
                ));
            }
            let probe = Path3D {
                keys: keys.clone(),
                closed: false,
            };
            // Equal key spacing in time would make the S-curve accelerate and
            // brake at every control point. Allocate time by arc length so the
            // demo is a steady cruise and the profile comparison is readable.
            let mut segment_lengths = Vec::with_capacity(keys.len().saturating_sub(1));
            let mut total_length = 0.0;
            for index in 1..keys.len() {
                let t0 = probe.keys[index - 1].time_s;
                let t1 = probe.keys[index].time_s;
                let mut previous = sample_path(&probe, t0);
                let mut length = 0.0;
                for step in 1..=32 {
                    let t = t0 + (t1 - t0) * step as f32 / 32.0;
                    let point = sample_path(&probe, t);
                    length += (point - previous).length();
                    previous = point;
                }
                total_length += length;
                segment_lengths.push(length);
            }
            let duration = (total_length / (max_speed * 0.9)).clamp(6.0, 60.0) as f32;
            let mut elapsed = 0.0;
            keys[0].time_s = 0.0;
            for (index, length) in segment_lengths.iter().enumerate() {
                elapsed += length;
                let share = if total_length > 0.0 {
                    elapsed / total_length
                } else {
                    (index + 1) as f64 / segment_lengths.len() as f64
                };
                keys[index + 1].time_s = duration * share as f32;
            }
            for key in &mut keys {
                key.ease = 0.0;
            }
            rig.duration_s = duration;
            let ground = sample_ground(center.x, center.y);
            (keys, DVec3::new(center.x, center.y, ground + 70.0))
        }
        other => return Err(format!("unknown rig path '{other}'")),
    };
    rig.eye.keys = keys;
    if pattern.eq_ignore_ascii_case("demo") {
        rig.target.mode = TargetMode::Fixed;
        rig.target.fixed = target.to_array();
        rig.target.path.keys = vec![
            vf_core::PathKey::new(0.0, target.to_array()),
            vf_core::PathKey::new(rig.duration_s, target.to_array()),
        ];
    } else {
        rig.duration_s = 8.0;
        rig.target.mode = TargetMode::Path;
        rig.target.path.keys = vec![
            vf_core::PathKey::new(0.0, target.to_array()),
            vf_core::PathKey::new(8.0, target.to_array()),
        ];
    }
    rig.validate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{AircraftPreset, CameraRig, ClassicParams};

    fn field() -> HeightField {
        vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(129, 7)).unwrap()
    }

    #[test]
    fn path_hits_keys_and_is_deterministic() {
        let path = Path3D::default();
        let p0 = sample_path(&path, 0.0);
        let p4 = sample_path(&path, 4.0);
        assert!((p0 - DVec3::from_array(path.keys[0].position)).length() < 1e-6);
        assert!((p4 - DVec3::from_array(path.keys[1].position)).length() < 1e-6);
        assert_eq!(sample_path(&path, 2.3), sample_path(&path, 2.3));
    }

    #[test]
    fn rig_samples_are_deterministic_and_bank() {
        let hf = field();
        let mut rig = CameraRig::preset("helicopter").unwrap();
        preset_path(&mut rig, &hf, "orbit").unwrap();
        let a = sample_rig(&rig, &hf, Seed::new(3), 2.4).unwrap();
        let b = sample_rig(&rig, &hf, Seed::new(3), 2.4).unwrap();
        assert_eq!(a.camera.position, b.camera.position);
        assert_eq!(a.camera.target, b.camera.target);
        assert!(a.camera.position.is_finite());
        assert!(a.camera.target.is_finite());
        assert!(a.bank_deg.abs() <= rig.body.max_bank_deg + 0.1);
    }

    #[test]
    fn clearance_keeps_the_eye_above_the_terrain() {
        let hf = field();
        let mut rig = CameraRig::preset("drone").unwrap();
        preset_path(&mut rig, &hf, "ridge").unwrap();
        rig.constraints.enabled = true;
        rig.constraints.ground_clearance_m = 40.0;
        for step in 0..20 {
            let sample = sample_rig(&rig, &hf, Seed::new(1), step as f32 * 0.4).unwrap();
            assert!(sample.altitude_agl_m >= 30.0, "{}", sample.altitude_agl_m);
        }
    }

    #[test]
    fn presets_change_the_response() {
        let a = CameraRig::preset("fighter").unwrap();
        let b = CameraRig::preset("airship").unwrap();
        assert!(a.body.accel_forward_mps2 > b.body.accel_forward_mps2);
        assert!(a.body.max_bank_deg > b.body.max_bank_deg);
        assert_eq!(a.body.preset, AircraftPreset::FighterJet);
    }

    #[test]
    fn path_length_and_recommended_duration_are_consistent() {
        let path = Path3D::default();
        let length = path_length(&path);
        assert!(length > 1000.0);
        let mut rig = CameraRig::preset("drone").unwrap();
        rig.eye = path;
        rig.duration_s = 8.0;
        let recommended = recommended_duration_s(&rig);
        assert!(recommended > 8.0, "{recommended}");
        rig.duration_s = recommended;
        assert!((required_speed_mps(&rig) - cruise_speed_mps(&rig.body)).abs() < 1.0);
        assert!((duration_pressure(&rig) - 1.0).abs() < 0.05);
    }

    #[test]
    fn demo_path_is_short_and_fitted_to_the_body() {
        let hf = field();
        for preset in [
            AircraftPreset::DroneFpv,
            AircraftPreset::Helicopter,
            AircraftPreset::Airship,
        ] {
            let mut rig = CameraRig::preset(preset.name()).unwrap();
            preset_path(&mut rig, &hf, "demo").unwrap();
            assert!((6.0..=60.0).contains(&rig.duration_s));
            assert!(path_length(&rig.eye) < 2600.0);
            let pressure = duration_pressure(&rig);
            assert!((0.9..=1.5).contains(&pressure), "{preset:?}: {pressure}");
            assert_eq!(rig.target.mode, TargetMode::Fixed);
        }
    }
}
